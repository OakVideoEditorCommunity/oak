//! Adapter-layer hooks (plan §4): the three interception points the
//! execution layer owns itself, with no dependency on the OPP event face.
//!
//! * [`BeforeCommit`] / [`Policy`] — the **confirmation gate**. Every tool
//!   is graded into an [`OperationClass`] (read / edit / project / export)
//!   and the policy decides what happens before the transaction commits:
//!   `ProposalOnly` never touches the backend and only produces a
//!   [`Proposal`], `Confirm` asks the hook (`Approve` / `Reject` /
//!   `Proposed`), `Auto` runs the call straight through.
//! * [`RenderDone`] — the **readback assertion** trigger. A frame-returning
//!   tool's result is asserted (pixel checks, [`PixelReadback`]) *after* the
//!   backend returned it and *before* the undo group commits, so a frame
//!   that fails its assertions is rolled back like any other failure.
//! * [`EditFailed`] — the **structured feedback** path. Every failure is
//!   aborted first, then handed to the hook, which turns it into a
//!   [`Feedback`]: the existing `{code, message, tool, step, gap, txn}`
//!   error object plus the two advisory keys `retryable` / `hint`. No
//!   second error shape is introduced.
//!
//! # Why the gate sits where it sits
//!
//! The undo group is the only rollback primitive the execution layer has,
//! so the gate can only run *after* the body for classes whose whole effect
//! lives inside that group. Two classes do not:
//!
//! * `project.*` / `export.start` write files (a `.ove` document, an MP4)
//!   through the backend, which no undo group can take back;
//! * `edit.undo` has no transaction at all (OPP/1 §6).
//!
//! Those are therefore gated **before** the body runs ([`GatePoint::BeforeInvoke`]),
//! and so is `ProposalOnly`, which must never touch the backend at all.
//!
//! # Injection
//!
//! [`crate::dispatch::execute`] keeps its old signature and runs
//! `Policy::Auto` with [`NoHooks`] (exactly the pre-hooks behaviour). A
//! caller that wants a gate uses [`crate::dispatch::execute_with`]:
//!
//! ```
//! use oak_ai_tools::{dispatch, hooks, mock::MockBackend};
//! use serde_json::json;
//!
//! // Any type implementing the three callbacks can be injected.
//! struct Ui {
//!     confirmed: Vec<String>,
//! }
//! impl hooks::BeforeCommit for Ui {
//!     fn before_commit(&mut self, pending: &hooks::PendingEdit<'_>) -> hooks::Decision {
//!         self.confirmed.push(pending.tool.to_string());
//!         hooks::Decision::Approve
//!     }
//! }
//! impl hooks::RenderDone for Ui {
//!     fn render_done(&mut self, _event: &hooks::RenderDoneEvent<'_>) -> hooks::ReadbackVerdict {
//!         hooks::ReadbackVerdict::Pass
//!     }
//! }
//! impl hooks::EditFailed for Ui {
//!     fn edit_failed(&mut self, event: &hooks::FailureEvent<'_>) -> hooks::Feedback {
//!         hooks::Feedback::from_error(event.error.clone())
//!     }
//! }
//!
//! let mut backend = MockBackend::new();
//! dispatch::execute(&mut backend, "project.open", json!({"name": "hooks"})).unwrap();
//! let mut ui = Ui { confirmed: Vec::new() };
//! match dispatch::execute_with(
//!     &mut backend,
//!     "timeline.add_track",
//!     json!({"type": "video"}),
//!     hooks::Policy::Confirm,
//!     &mut ui,
//! ) {
//!     hooks::GatedOutcome::Applied { .. } => { /* committed */ }
//!     hooks::GatedOutcome::Proposed { .. } => { /* show, nothing applied */ }
//!     hooks::GatedOutcome::Rejected { reason, .. } => { /* refused */ }
//!     hooks::GatedOutcome::Failed { .. } => { /* re-plan with feedback */ }
//! }
//! assert_eq!(ui.confirmed, vec!["timeline.add_track"]);
//! ```

use serde_json::{Value, json};

use crate::backend::EngineBackend;
use crate::error::{
	CODE_BACKEND, CODE_INVALID_PARAMS, CODE_IO, CODE_NOTHING_TO_UNDO, CODE_NOT_FOUND,
	CODE_SCHEMA_VALIDATION, CODE_STATE, CODE_TIMECODE, CODE_TRANSACTION, CODE_UNKNOWN_TOOL,
	CODE_UNSUPPORTED, ToolError,
};
use crate::render::Frame;
use crate::txn::{self, GroupOutcome, Verdict};
use base64::Engine as _;

// ---------------------------------------------------------------------------
// hook-layer codes
//
// The `{code, message, tool, step, gap, txn}` shape is the one of
// `src/error.rs`; these constants only extend the *vocabulary* of `code`
// with the three outcomes the hook layer introduces (nothing outside this
// module invents fields).
// ---------------------------------------------------------------------------

/// The confirmation gate rejected the call (`Reject`). Nothing was applied.
pub const CODE_REJECTED: &str = "USER_REJECTED";
/// `ProposalOnly`: the call was not performed, a proposal was produced.
pub const CODE_PROPOSAL_ONLY: &str = "PROPOSAL_ONLY";
/// The frame came back but failed the readback assertions.
pub const CODE_READBACK_FAILED: &str = "READBACK_ASSERTION_FAILED";
/// The tool is in the registry but has no entry in the grading table.
pub const CODE_UNCLASSIFIED_TOOL: &str = "UNCLASSIFIED_TOOL";

// ---------------------------------------------------------------------------
// the grading table
// ---------------------------------------------------------------------------

/// What kind of change a tool makes, which is what the policy grades on.
///
/// Plan §4: reads are `auto`, edits are `confirm` by default, project and
/// export are `confirm`. The distinction between [`OperationClass::Edit`]
/// and the other two mutating classes is *not* about "how dangerous it
/// looks" but about whether the undo group is a complete rollback: only
/// `Edit` is fully covered by `txn_begin`/`txn_abort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationClass {
	/// No document state changes (`project.get_overview`, `render.*`, ...).
	Read,
	/// A timeline/node edit, fully reverted by the undo group on abort.
	Edit,
	/// Project-level I/O: open / save / footage import (writes outside the
	/// undo group).
	Project,
	/// `export.start`: spawns the encoder, writes a file.
	Export,
}

impl OperationClass {
	/// Whether the class changes the document at all.
	pub fn is_read(self) -> bool {
		matches!(self, Self::Read)
	}

	/// Whether the class changes the document.
	pub fn is_mutating(self) -> bool {
		!self.is_read()
	}

	/// Stable label used in proposals and logs.
	pub fn label(self) -> &'static str {
		match self {
			Self::Read => "read",
			Self::Edit => "edit",
			Self::Project => "project",
			Self::Export => "export",
		}
	}

	/// Where the confirmation gate runs for this class.
	///
	/// `transactional` comes from [`crate::dispatch::ToolDef::transactional`]
	/// so the registry stays the single source of truth about undo groups.
	///
	/// `Read` never reaches the gate (it is `auto` under every policy); the
	/// returned point for it is nevertheless well defined so table-driven
	/// tests can assert the whole matrix.
	pub fn gate_point(self, transactional: bool) -> GatePoint {
		match self {
			// A group is a complete rollback, so the gate can see the
			// outcome before committing it.
			Self::Edit if transactional => GatePoint::BeforeCommit,
			// Untransactional (`edit.undo`) or effects outside the group.
			_ => GatePoint::BeforeInvoke,
		}
	}
}

/// The grading table: one entry per frozen tool, in schema-file order.
///
/// Kept next to (not inside) the dispatch registry on purpose: the registry
/// says *how* a tool is called, this table says *who may call it*. The
/// `hooks` integration tests assert a bidirectional match against
/// `ai/schema/tools/*.json` **and** against
/// [`crate::dispatch::tool_names`], so a tool added to either side without
/// a grading entry turns the suite red.
pub const TOOL_CLASSES: [(&str, OperationClass); 22] = [
	("edit.undo", OperationClass::Edit),
	("export.start", OperationClass::Export),
	("media.import_footage", OperationClass::Project),
	("media.list_footage", OperationClass::Read),
	("media.probe", OperationClass::Read),
	("node.add_effect", OperationClass::Edit),
	("node.list_types", OperationClass::Read),
	("node.set_param", OperationClass::Edit),
	("project.get_overview", OperationClass::Read),
	("project.open", OperationClass::Project),
	("project.save", OperationClass::Project),
	("render.get_audio_levels", OperationClass::Read),
	("render.get_frame", OperationClass::Read),
	("render.get_thumbnails", OperationClass::Read),
	("timeline.add_marker", OperationClass::Edit),
	("timeline.add_track", OperationClass::Edit),
	("timeline.add_transition", OperationClass::Edit),
	("timeline.move_clip", OperationClass::Edit),
	("timeline.place_clip", OperationClass::Edit),
	("timeline.ripple_delete", OperationClass::Edit),
	("timeline.split_clip", OperationClass::Edit),
	("timeline.trim_clip", OperationClass::Edit),
];

/// The v2 face's grading table (T9.2). Kept separate from [`TOOL_CLASSES`]
/// so the frozen 22-tool table stays byte-identical; [`class_of_v2`]
/// resolves a v2 name, and the dispatcher tries it only for v2 tools.
pub const V2_TOOL_CLASSES: [(&str, OperationClass); 2] = [
	("sequence.get_settings", OperationClass::Read),
	("sequence.set_format", OperationClass::Edit),
];

/// The grading of a v2 `tool`, or `None` when the v2 table has no entry.
pub fn class_of_v2(tool: &str) -> Option<OperationClass> {
	V2_TOOL_CLASSES
		.iter()
		.find(|(name, _)| *name == tool)
		.map(|(_, class)| *class)
}

/// The grading of `tool`, or `None` when the table has no entry.
///
/// A `None` is **fail-closed**: [`crate::dispatch::execute_with`] refuses the
/// call with [`CODE_UNCLASSIFIED_TOOL`] instead of guessing a class.
pub fn class_of(tool: &str) -> Option<OperationClass> {
	TOOL_CLASSES
		.iter()
		.find(|(name, _)| *name == tool)
		.map(|(_, class)| *class)
}

/// Tools whose result carries frame pixels and therefore triggers
/// [`RenderDone`].
///
/// `render.get_audio_levels` is deliberately absent: it returns a level
/// envelope, not a frame, and the readback pipeline is a pixel assertion.
pub fn yields_frames(tool: &str) -> bool {
	matches!(tool, "render.get_frame" | "render.get_thumbnails")
}

// ---------------------------------------------------------------------------
// policy + gate
// ---------------------------------------------------------------------------

/// The confirmation policy (plan §4 / issue #77 question 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
	/// Never apply a mutating tool: produce a [`Proposal`] instead. The
	/// backend is not entered at all, so the document and the undo stack
	/// are untouched.
	ProposalOnly,
	/// Reads run through; mutating tools go to [`BeforeCommit`] and only
	/// run (or stay applied) when it answers [`Decision::Approve`].
	Confirm,
	/// Everything runs; the gate is not consulted.
	Auto,
}

impl Policy {
	/// Stable label for logs and the README matrix.
	pub fn label(self) -> &'static str {
		match self {
			Self::ProposalOnly => "proposal-only",
			Self::Confirm => "confirm",
			Self::Auto => "auto",
		}
	}
}

/// Where the gate runs for a given call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePoint {
	/// Before the backend body: nothing has run, so a rejection leaves no
	/// trace (no undo group is opened either).
	BeforeInvoke,
	/// After the body, before `txn_commit`: the hook sees the result and a
	/// rejection rolls the group back with `txn_abort`.
	BeforeCommit,
}

/// What the engine should do with a graded call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatePlan {
	/// No gate: run the body (reads under every policy, everything under
	/// `Auto`).
	Run,
	/// `ProposalOnly`: never run the body.
	Propose,
	/// `Confirm` at the given point.
	Confirm(GatePoint),
}

fn plan(class: OperationClass, transactional: bool, policy: Policy) -> GatePlan {
	if class.is_read() {
		// Plan §4: reads are auto in every mode — they cannot change state
		// and the agent needs them to plan at all.
		return GatePlan::Run;
	}
	match policy {
		Policy::Auto => GatePlan::Run,
		Policy::ProposalOnly => GatePlan::Propose,
		Policy::Confirm => GatePlan::Confirm(class.gate_point(transactional)),
	}
}

/// One graded tool call: the name, the validated arguments and the class
/// the policy grades on.
#[derive(Debug, Clone, Copy)]
pub struct ToolCall<'a> {
	/// Tool name (`timeline.split_clip`, ...).
	pub tool: &'a str,
	/// The arguments as they passed schema validation.
	pub args: &'a Value,
	/// The grading of `tool`.
	pub class: OperationClass,
}

/// The pending edit handed to [`BeforeCommit`].
pub struct PendingEdit<'a> {
	/// Tool name.
	pub tool: &'a str,
	/// Grading of the tool.
	pub class: OperationClass,
	/// Active policy (a hook may render differently per mode).
	pub policy: Policy,
	/// Where the gate ran; tells the hook whether `result` is available.
	pub point: GatePoint,
	/// Validated arguments.
	pub args: &'a Value,
	/// The body's result, present only at [`GatePoint::BeforeCommit`].
	pub result: Option<&'a Value>,
}

/// Answer of the confirmation gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
	/// Apply the call (commit the group / run the body).
	Approve,
	/// Refuse, with the reason shown to the model.
	Reject {
		/// Why it was refused; becomes the error message.
		reason: String,
	},
	/// Keep the call as a proposal instead of applying it. In
	/// `Policy::Confirm` this is the "show it, do not apply it" answer; in
	/// `Policy::ProposalOnly` every answer becomes a proposal.
	Proposed {
		/// Optional annotation for the proposal list.
		reason: Option<String>,
	},
}

impl Decision {
	/// `Proposed` without a reason (the common case).
	pub fn proposed() -> Self {
		Self::Proposed { reason: None }
	}

	/// `Reject` with a reason.
	pub fn reject(reason: impl Into<String>) -> Self {
		Self::Reject {
			reason: reason.into(),
		}
	}
}

/// A call that was described but not applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
	/// Tool that would have run.
	pub tool: String,
	/// Its grading.
	pub class: OperationClass,
	/// Its arguments.
	pub args: Value,
	/// Why it is a proposal rather than an applied edit.
	pub reason: Option<String>,
}

impl Proposal {
	/// Wire form: what a proposal list shows.
	pub fn to_json(&self) -> Value {
		let mut out = json!({
			"tool": self.tool,
			"class": self.class.label(),
			"args": self.args,
		});
		if let Some(reason) = &self.reason {
			out["reason"] = json!(reason);
		}
		out
	}
}

// ---------------------------------------------------------------------------
// the three callbacks
// ---------------------------------------------------------------------------

/// `before_commit`: the confirmation gate.
///
/// Called once per graded call, at the [`PendingEdit::point`] the class
/// prescribes. Never called for reads or under [`Policy::Auto`].
pub trait BeforeCommit {
	/// Decides what happens to the pending edit.
	fn before_commit(&mut self, pending: &PendingEdit<'_>) -> Decision;
}

/// The frame-returning result handed to [`RenderDone`].
pub struct RenderDoneEvent<'a> {
	/// Tool that produced the frames.
	pub tool: &'a str,
	/// Its arguments (e.g. the requested `max_size`).
	pub args: &'a Value,
	/// The whole tool result.
	pub result: &'a Value,
	/// The frame payloads: the result itself for `render.get_frame`, the
	/// `frames` array for `render.get_thumbnails`.
	pub frames: Vec<&'a Value>,
}

/// Answer of the readback assertion pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadbackVerdict {
	/// The frames are acceptable.
	Pass,
	/// The frames violated an assertion; the group is aborted and the
	/// reason is fed back as [`CODE_READBACK_FAILED`].
	Fail {
		/// What failed, verbatim for the model.
		reason: String,
	},
}

/// `render_done`: the readback assertion pipeline.
///
/// Runs after the backend returned the frame and before the undo group
/// commits. Pixel checks live here (see [`PixelReadback`]); an LLM
/// judgement would be the same hook with a different body — the execution
/// layer only needs a [`ReadbackVerdict`].
pub trait RenderDone {
	/// Asserts the returned frames.
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict;
}

/// The failure handed to [`EditFailed`], already aborted.
pub struct FailureEvent<'a> {
	/// Tool that failed.
	pub tool: &'a str,
	/// Its grading, `None` when the failure happened before grading
	/// (unknown tool).
	pub class: Option<OperationClass>,
	/// The structured error (the `{code, message, tool, step, gap, txn}`
	/// object of `src/error.rs`).
	pub error: &'a ToolError,
}

/// `edit_failed`: the structured feedback for the re-planning loop.
///
/// [`Feedback::to_json`] is the existing error object plus the two advisory
/// keys `retryable` and `hint`; the `ToolError` itself is untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
	/// The failure, unchanged (`code` / `step` / `txn` / `gap` kept).
	pub error: ToolError,
	/// Whether re-sending the *same* call could ever succeed.
	pub retryable: bool,
	/// What to change before re-planning.
	pub hint: String,
}

impl Feedback {
	/// Classifies a failure by its `code` (deterministic, no I/O).
	pub fn from_error(error: ToolError) -> Self {
		let (retryable, hint) = match error.code.as_str() {
			CODE_SCHEMA_VALIDATION => (
				true,
				"the arguments do not match this tool's schema; read the `message` \
				 (it names the offending key and path), fix the shape/types, then \
				 re-plan — do not re-send the same call",
			),
			CODE_TIMECODE => (
				true,
				"time values must be `{num, den}` rational seconds or an integer frame \
				 index; bare floats and strings are rejected",
			),
			CODE_NOT_FOUND => (
				true,
				"call project.get_overview (and media.list_footage) again: the id moved \
				 or was removed by an earlier edit",
			),
			CODE_INVALID_PARAMS => (
				true,
				"the arguments were refused; `message` names the offending field, key \
				 or value. If it is a type / shape problem, fix only that field and \
				 retry. If `message` says the placement overlaps or is out of range, \
				 call project.get_overview to re-read the current timeline and \
				 recompute this one item's position against it — do not move the \
				 other items",
			),
			CODE_STATE => (
				true,
				"the backend is not in the required state: check the `message` — it may \
				 mean no project is open, the project has no sequence, or no clip/node \
				 is selected; fix that precondition, then retry",
			),
			CODE_UNKNOWN_TOOL => (
				false,
				"the name is not in the registered tool face (the frozen 22 plus the \
				 v2 `sequence.*` tools); pick a name that appears in the tool list",
			),
			CODE_UNSUPPORTED => (
				false,
				"capability gap: this backend does not implement the tool. Do not retry; \
				 tell the user which gap (`gap` field) blocks the edit",
			),
			CODE_NOTHING_TO_UNDO => (false, "the undo stack is empty; there is nothing to revert"),
			CODE_TRANSACTION => (
				true,
				"the undo group failed to open/close; retry once, then report the failure",
			),
			CODE_BACKEND => (
				true,
				"the backend refused this call; the concrete cause is in `message`. \
				 If it mentions a transaction (`already held` / `not in transaction`), \
				 that is internal sequencing — retry once. If it mentions a capability \
				 or a limit, do NOT retry: explain the limitation to the user instead \
				 of changing the arguments",
			),
			CODE_IO => (true, "a file could not be read or written; check the path and retry"),
			CODE_REJECTED => (
				false,
				"the user rejected this edit; do not re-send it — propose a different \
				 approach (or ask what to change)",
			),
			CODE_PROPOSAL_ONLY => (
				false,
				"proposal-only mode: nothing was applied. The proposal list is the \
				 deliverable; wait for the user to switch mode",
			),
			CODE_READBACK_FAILED => (
				true,
				"the rendered frame failed its readback assertion; the edit was rolled \
				 back — inspect the reason, fix the edit, render again",
			),
			CODE_UNCLASSIFIED_TOOL => (
				false,
				"internal: the tool has no entry in the hooks grading table. Report this \
				 as a bug; do not retry",
			),
			_ => (
				false,
				"unrecognised failure code; report the error verbatim instead of retrying",
			),
		};
		Self {
			error,
			retryable,
			hint: hint.to_string(),
		}
	}

	/// Wire form: the error object of `src/error.rs` plus `retryable` and
	/// `hint`. Same keys, same shape — two additive fields, not a second
	/// error type.
	pub fn to_json(&self) -> Value {
		let mut out = self.error.to_json();
		out["retryable"] = json!(self.retryable);
		out["hint"] = json!(self.hint);
		out
	}
}

/// `edit_failed`: turns an aborted failure into feedback.
pub trait EditFailed {
	/// Called after the undo group was aborted (or after a pre-flight
	/// rejection), for every failure.
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> Feedback;
}

/// The three hooks as one injectable object.
///
/// Blanket-implemented, so any type implementing the three traits can be
/// handed to [`crate::dispatch::execute_with`].
pub trait Hooks: BeforeCommit + RenderDone + EditFailed {}

impl<T: BeforeCommit + RenderDone + EditFailed> Hooks for T {}

/// The neutral hook set: approve everything, assert nothing, classify
/// failures. This is what [`crate::dispatch::execute`] uses, which is why
/// the old entry point behaves exactly as it did before hooks existed.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHooks;

impl BeforeCommit for NoHooks {
	fn before_commit(&mut self, _pending: &PendingEdit<'_>) -> Decision {
		Decision::Approve
	}
}

impl RenderDone for NoHooks {
	fn render_done(&mut self, _event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		ReadbackVerdict::Pass
	}
}

impl EditFailed for NoHooks {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> Feedback {
		Feedback::from_error(event.error.clone())
	}
}

/// The built-in pixel half of the readback pipeline.
///
/// Decodes `png_base64` (or checks the `bgra8` payload), asserts that the
/// declared geometry matches the decoded pixels, that `png_bytes` matches
/// the decoded length, and — when a threshold is configured — that enough
/// pixels are non-black (a fully black frame is how a gap or a failed
/// composite shows up, see the SDK E2E invariants).
#[derive(Debug, Clone, Copy)]
pub struct PixelReadback {
	/// Minimum fraction of non-black pixels; `0.0` disables the check.
	pub min_non_black_ratio: f64,
}

impl PixelReadback {
	/// Structural checks only (decodes, geometry, byte count).
	pub fn structural() -> Self {
		Self {
			min_non_black_ratio: 0.0,
		}
	}

	/// Also requires at least `ratio` of the pixels to be non-black.
	pub fn opaque(ratio: f64) -> Self {
		Self {
			min_non_black_ratio: ratio.clamp(0.0, 1.0),
		}
	}
}

impl RenderDone for PixelReadback {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		if event.frames.is_empty() {
			return ReadbackVerdict::Fail {
				reason: format!("{}: the result carries no frames", event.tool),
			};
		}
		for (index, frame) in event.frames.iter().enumerate() {
			if let Err(reason) = check_frame(frame, self.min_non_black_ratio) {
				return ReadbackVerdict::Fail {
					reason: format!("{} frame {index}: {reason}", event.tool),
				};
			}
		}
		ReadbackVerdict::Pass
	}
}

fn check_frame(value: &Value, min_non_black: f64) -> Result<(), String> {
	let width = value
		.get("width")
		.and_then(Value::as_u64)
		.filter(|width| *width > 0)
		.ok_or_else(|| "missing or zero `width`".to_string())?;
	let height = value
		.get("height")
		.and_then(Value::as_u64)
		.filter(|height| *height > 0)
		.ok_or_else(|| "missing or zero `height`".to_string())?;

	if let Some(encoded) = value.get("png_base64").and_then(Value::as_str) {
		let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok().ok_or_else(|| "`png_base64` is not valid base64".to_string())?;
		if let Some(declared) = value.get("png_bytes").and_then(Value::as_u64) {
			if declared != bytes.len() as u64 {
				return Err(format!(
					"`png_bytes` says {declared} but the payload decodes to {} bytes",
					bytes.len()
				));
			}
		}
		let frame = Frame::from_png(&bytes).map_err(|err| format!("PNG does not decode: {err}"))?;
		if u64::from(frame.width) != width || u64::from(frame.height) != height {
			return Err(format!(
				"decoded geometry {}x{} != declared {width}x{height}",
				frame.width, frame.height
			));
		}
		let stats = frame.stats();
		if stats.non_black_ratio < min_non_black {
			return Err(format!(
				"non-black ratio {:.4} < required {min_non_black:.4} (the frame is (partly) black)",
				stats.non_black_ratio
			));
		}
		return Ok(());
	}

	if let Some(payload) = value.get("bgra8").and_then(Value::as_array) {
		let expected = (width * height * 4) as usize;
		if payload.len() != expected {
			return Err(format!(
				"`bgra8` carries {} values, want {expected} (width*height*4)",
				payload.len()
			));
		}
		return Ok(());
	}

	Err("no `png_base64` or `bgra8` payload to assert".to_string())
}

/// The frame payloads of a frame-returning result.
pub fn frame_payloads<'a>(tool: &str, result: &'a Value) -> Vec<&'a Value> {
	if tool == "render.get_thumbnails" {
		result
			.get("frames")
			.and_then(Value::as_array)
			.map(|frames| frames.iter().collect())
			.unwrap_or_default()
	} else {
		vec![result]
	}
}

// ---------------------------------------------------------------------------
// outcome + orchestration
// ---------------------------------------------------------------------------

/// What [`crate::dispatch::execute_with`] did with the call.
#[derive(Debug, Clone, PartialEq)]
pub enum GatedOutcome {
	/// Committed (or, for a read, simply returned).
	Applied {
		/// The tool result.
		result: Value,
	},
	/// Not applied: the call is a proposal (policy `ProposalOnly`, or the
	/// hook answered `Proposed`). The document is unchanged.
	Proposed {
		/// The proposal to show / log.
		proposal: Proposal,
	},
	/// The gate said no. Nothing is applied; for a `BeforeCommit` gate the
	/// group was aborted, for `BeforeInvoke` nothing was opened.
	Rejected {
		/// The reason the gate gave.
		reason: String,
		/// Structured error (`code = USER_REJECTED`).
		error: ToolError,
	},
	/// The call failed; the group was aborted and the hook was consulted.
	Failed {
		/// Structured error (unchanged `{code, message, tool, step, gap, txn}`).
		error: ToolError,
		/// Re-planning feedback built from it.
		feedback: Feedback,
	},
}

impl GatedOutcome {
	/// Whether the call changed the document.
	pub fn is_applied(&self) -> bool {
		matches!(self, Self::Applied { .. })
	}

	/// Collapses the outcome onto the pre-hooks `Result` surface:
	/// `Applied` is `Ok`, everything else is the structured error.
	pub fn into_result(self) -> Result<Value, ToolError> {
		match self {
			Self::Applied { result } => Ok(result),
			Self::Proposed { proposal } => {
				let mut error = ToolError::new(
					CODE_PROPOSAL_ONLY,
					format!(
						"{} was not applied: proposal-only mode produced a proposal",
						proposal.tool
					),
				)
				.with_tool(&proposal.tool)
				.with_step("confirm");
				if let Some(reason) = &proposal.reason {
					error.message = format!("{} ({reason})", error.message);
				}
				Err(error)
			}
			Self::Rejected { error, .. } | Self::Failed { error, .. } => Err(error),
		}
	}

	/// The failure path: consult `edit_failed` and wrap the result.
	pub fn failure(
		hooks: &mut dyn Hooks,
		tool: &str,
		class: Option<OperationClass>,
		error: ToolError,
	) -> Self {
		let event = FailureEvent {
			tool,
			class,
			error: &error,
		};
		let feedback = hooks.edit_failed(&event);
		Self::Failed { error, feedback }
	}
}

/// Why a group that already ran was aborted, kept for the outcome.
enum AbortReason {
	Readback(String),
	Rejected(String),
	Proposed(Option<String>),
}

fn propose(hooks: &mut dyn Hooks, call: &ToolCall<'_>, policy: Policy) -> GatedOutcome {
	let pending = PendingEdit {
		tool: call.tool,
		class: call.class,
		policy,
		point: GatePoint::BeforeInvoke,
		args: call.args,
		result: None,
	};
	// The hook may annotate the proposal; in this mode its answer can
	// never turn into an applied edit.
	let reason = match hooks.before_commit(&pending) {
		Decision::Approve => None,
		Decision::Reject { reason } => Some(reason),
		Decision::Proposed { reason } => reason,
	};
	GatedOutcome::Proposed {
		proposal: Proposal {
			tool: call.tool.to_string(),
			class: call.class,
			args: call.args.clone(),
			reason,
		},
	}
}

/// Runs a transactional call under the policy.
pub fn gated_transactional<T, F>(
	ops: &mut T,
	call: &ToolCall<'_>,
	policy: Policy,
	hooks: &mut dyn Hooks,
	body: F,
) -> GatedOutcome
where
	T: EngineBackend + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	match plan(call.class, true, policy) {
		GatePlan::Propose => propose(hooks, call, policy),
		GatePlan::Run => run_group(ops, call, hooks, body, false),
		GatePlan::Confirm(GatePoint::BeforeCommit) => run_group(ops, call, hooks, body, true),
		GatePlan::Confirm(GatePoint::BeforeInvoke) => {
			match hooks.before_commit(&PendingEdit {
				tool: call.tool,
				class: call.class,
				policy,
				point: GatePoint::BeforeInvoke,
				args: call.args,
				result: None,
			}) {
				Decision::Approve => run_group(ops, call, hooks, body, false),
				Decision::Reject { reason } => GatedOutcome::Rejected {
					error: ToolError::new(CODE_REJECTED, reason.clone())
						.with_tool(call.tool)
						.with_step("confirm"),
					reason,
				},
				Decision::Proposed { reason } => GatedOutcome::Proposed {
					proposal: Proposal {
						tool: call.tool.to_string(),
						class: call.class,
						args: call.args.clone(),
						reason,
					},
				},
			}
		}
	}
}

/// Runs a non-transactional call (`edit.undo`) under the policy. The gate
/// always runs before the body here: without a group there is nothing to
/// roll back.
pub fn gated_plain<T, F>(
	ops: &mut T,
	call: &ToolCall<'_>,
	policy: Policy,
	hooks: &mut dyn Hooks,
	body: F,
) -> GatedOutcome
where
	T: EngineBackend + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	match plan(call.class, false, policy) {
		GatePlan::Propose => propose(hooks, call, policy),
		GatePlan::Run => run_body(ops, call, hooks, body),
		GatePlan::Confirm(_) => match hooks.before_commit(&PendingEdit {
			tool: call.tool,
			class: call.class,
			policy,
			point: GatePoint::BeforeInvoke,
			args: call.args,
			result: None,
		}) {
			Decision::Approve => run_body(ops, call, hooks, body),
			Decision::Reject { reason } => GatedOutcome::Rejected {
				error: ToolError::new(CODE_REJECTED, reason.clone())
					.with_tool(call.tool)
					.with_step("confirm"),
				reason,
			},
			Decision::Proposed { reason } => GatedOutcome::Proposed {
				proposal: Proposal {
					tool: call.tool.to_string(),
					class: call.class,
					args: call.args.clone(),
					reason,
				},
			},
		},
	}
}

/// `begin → body → [readback] → [gate] → commit / abort`.
fn run_group<T, F>(
	ops: &mut T,
	call: &ToolCall<'_>,
	hooks: &mut dyn Hooks,
	body: F,
	gate_at_commit: bool,
) -> GatedOutcome
where
	T: EngineBackend + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	let mut aborted: Option<AbortReason> = None;
	let outcome = txn::run_gated(ops, call.tool, body, |value| {
		if yields_frames(call.tool) {
			let event = RenderDoneEvent {
				tool: call.tool,
				args: call.args,
				result: value,
				frames: frame_payloads(call.tool, value),
			};
			if let ReadbackVerdict::Fail { reason } = hooks.render_done(&event) {
				aborted = Some(AbortReason::Readback(reason));
				return Verdict::Abort;
			}
		}
		if gate_at_commit {
			match hooks.before_commit(&PendingEdit {
				tool: call.tool,
				class: call.class,
				policy: Policy::Confirm,
				point: GatePoint::BeforeCommit,
				args: call.args,
				result: Some(value),
			}) {
				Decision::Approve => {}
				Decision::Reject { reason } => {
					aborted = Some(AbortReason::Rejected(reason));
					return Verdict::Abort;
				}
				Decision::Proposed { reason } => {
					aborted = Some(AbortReason::Proposed(reason));
					return Verdict::Abort;
				}
			}
		}
		Verdict::Commit
	});

	match outcome {
		Ok(GroupOutcome::Committed(result)) => GatedOutcome::Applied { result },
		Ok(GroupOutcome::Aborted(_value)) => match aborted {
			Some(AbortReason::Readback(reason)) => GatedOutcome::failure(
				hooks,
				call.tool,
				Some(call.class),
				ToolError::new(CODE_READBACK_FAILED, reason)
					.with_tool(call.tool)
					.with_step("readback")
					.aborted(),
			),
			Some(AbortReason::Rejected(reason)) => GatedOutcome::Rejected {
				error: ToolError::new(CODE_REJECTED, reason.clone())
					.with_tool(call.tool)
					.with_step("confirm")
					.aborted(),
				reason,
			},
			Some(AbortReason::Proposed(reason)) => GatedOutcome::Proposed {
				proposal: Proposal {
					tool: call.tool.to_string(),
					class: call.class,
					args: call.args.clone(),
					reason,
				},
			},
			// `run_gated` only aborts when the decision said so.
			None => GatedOutcome::failure(
				hooks,
				call.tool,
				Some(call.class),
				ToolError::new(
					crate::error::CODE_INTERNAL,
					"undo group aborted without a recorded reason",
				)
				.with_tool(call.tool)
				.with_step("commit")
				.aborted(),
			),
		},
		Err(error) => GatedOutcome::failure(hooks, call.tool, Some(call.class), error),
	}
}

/// Runs a body with no transaction and the readback trigger.
fn run_body<T, F>(ops: &mut T, call: &ToolCall<'_>, hooks: &mut dyn Hooks, body: F) -> GatedOutcome
where
	T: EngineBackend + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	match body(ops) {
		Ok(value) => {
			if yields_frames(call.tool) {
				let event = RenderDoneEvent {
					tool: call.tool,
					args: call.args,
					result: &value,
					frames: frame_payloads(call.tool, &value),
				};
				if let ReadbackVerdict::Fail { reason } = hooks.render_done(&event) {
					return GatedOutcome::failure(
						hooks,
						call.tool,
						Some(call.class),
						ToolError::new(CODE_READBACK_FAILED, reason)
							.with_tool(call.tool)
							.with_step("readback"),
					);
				}
			}
			GatedOutcome::Applied { result: value }
		}
		Err(error) => GatedOutcome::failure(
			hooks,
			call.tool,
			Some(call.class),
			error.with_tool(call.tool).with_step("invoke"),
		),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn call<'a>(tool: &'a str, args: &'a Value) -> ToolCall<'a> {
		ToolCall {
			tool,
			args,
			class: class_of(tool).expect("graded tool"),
		}
	}

	#[test]
	fn the_grading_table_covers_the_frozen_face() {
		assert_eq!(TOOL_CLASSES.len(), 22);
		let mut names: Vec<&str> = TOOL_CLASSES.iter().map(|(name, _)| *name).collect();
		names.sort_unstable();
		let mut unique = names.clone();
		unique.dedup();
		assert_eq!(names.len(), unique.len(), "duplicate grading entries");
		assert_eq!(class_of("timeline.split_clip"), Some(OperationClass::Edit));
		assert_eq!(class_of("render.get_frame"), Some(OperationClass::Read));
		assert_eq!(class_of("project.save"), Some(OperationClass::Project));
		assert_eq!(class_of("export.start"), Some(OperationClass::Export));
		assert_eq!(class_of("timeline.teleport"), None);
	}

	#[test]
	fn the_gate_point_follows_the_rollback_coverage() {
		// Inside the undo group: gate at commit.
		assert_eq!(OperationClass::Edit.gate_point(true), GatePoint::BeforeCommit);
		// No group to roll back, or effects outside the group.
		assert_eq!(OperationClass::Edit.gate_point(false), GatePoint::BeforeInvoke);
		assert_eq!(OperationClass::Project.gate_point(true), GatePoint::BeforeInvoke);
		assert_eq!(OperationClass::Export.gate_point(true), GatePoint::BeforeInvoke);
	}

	#[test]
	fn reads_are_auto_under_every_policy() {
		for policy in [Policy::ProposalOnly, Policy::Confirm, Policy::Auto] {
			assert_eq!(plan(OperationClass::Read, true, policy), GatePlan::Run);
		}
		assert_eq!(
			plan(OperationClass::Edit, true, Policy::Auto),
			GatePlan::Run
		);
		assert_eq!(
			plan(OperationClass::Edit, true, Policy::ProposalOnly),
			GatePlan::Propose
		);
		assert_eq!(
			plan(OperationClass::Edit, true, Policy::Confirm),
			GatePlan::Confirm(GatePoint::BeforeCommit)
		);
		assert_eq!(
			plan(OperationClass::Export, true, Policy::Confirm),
			GatePlan::Confirm(GatePoint::BeforeInvoke)
		);
	}

	#[test]
	fn feedback_reuses_the_error_shape_and_adds_two_keys() {
		let error = ToolError::not_found("clip gone")
			.with_tool("timeline.split_clip")
			.with_step("invoke")
			.aborted();
		let feedback = Feedback::from_error(error.clone());
		assert!(feedback.retryable);
		assert!(feedback.hint.contains("overview"), "{}", feedback.hint);
		let wire = feedback.to_json();
		// The error object is unchanged ...
		assert_eq!(wire["code"], "NOT_FOUND");
		assert_eq!(wire["tool"], "timeline.split_clip");
		assert_eq!(wire["step"], "invoke");
		assert_eq!(wire["txn"], "aborted");
		// ... plus exactly the two advisory keys.
		assert_eq!(wire["retryable"], true);
		assert!(wire["hint"].is_string());

		let unsupported = Feedback::from_error(ToolError::unsupported("GAP_X", "nope"));
		assert!(!unsupported.retryable);
		assert!(unsupported.to_json()["gap"] == "GAP_X");
	}

	#[test]
	fn pixel_readback_decodes_the_png_and_can_reject_a_black_frame() {
		use crate::render::frame_summary;

		let coloured = Frame::from_rgba(2, 1, vec![1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]).unwrap();
		let black = Frame::black(2, 1);

		let mut strict = PixelReadback::opaque(0.9);
		let coloured_summary = frame_summary(&coloured, "png");
		let ok = RenderDoneEvent {
			tool: "render.get_frame",
			args: &json!({}),
			result: &json!({}),
			frames: vec![&coloured_summary],
		};
		assert_eq!(strict.render_done(&ok), ReadbackVerdict::Pass);

		let black_summary = frame_summary(&black, "png");
		let bad = RenderDoneEvent {
			tool: "render.get_frame",
			args: &json!({}),
			result: &json!({}),
			frames: vec![&black_summary],
		};
		match strict.render_done(&bad) {
			ReadbackVerdict::Fail { reason } => assert!(reason.contains("non-black"), "{reason}"),
			other => panic!("black frame must fail the readback, got {other:?}"),
		}

		// A lying byte count is caught structurally even with the ratio off.
		let mut summary = frame_summary(&coloured, "png");
		summary["png_bytes"] = json!(1);
		let mut structural = PixelReadback::structural();
		match structural.render_done(&RenderDoneEvent {
			tool: "render.get_frame",
			args: &json!({}),
			result: &json!({}),
			frames: vec![&summary],
		}) {
			ReadbackVerdict::Fail { reason } => assert!(reason.contains("png_bytes"), "{reason}"),
			other => panic!("byte-count mismatch must fail, got {other:?}"),
		}
	}

	#[test]
	fn proposals_serialise_tool_class_and_args() {
		let args = json!({"clip": "clip-1", "time": {"num": 4, "den": 1}});
		let proposal = Proposal {
			tool: "timeline.split_clip".to_string(),
			class: OperationClass::Edit,
			args: args.clone(),
			reason: Some("asking first".to_string()),
		};
		let wire = proposal.to_json();
		assert_eq!(wire["tool"], "timeline.split_clip");
		assert_eq!(wire["class"], "edit");
		assert_eq!(wire["args"], args);
		assert_eq!(wire["reason"], "asking first");
	}

	#[test]
	fn proposal_only_calls_map_to_a_structured_error_through_the_old_surface() {
		let outcome = GatedOutcome::Proposed {
			proposal: Proposal {
				tool: "timeline.add_track".to_string(),
				class: OperationClass::Edit,
				args: json!({"type": "video"}),
				reason: None,
			},
		};
		let error = outcome.clone().into_result().unwrap_err();
		assert_eq!(error.code, CODE_PROPOSAL_ONLY);
		assert_eq!(error.tool.as_deref(), Some("timeline.add_track"));
		assert_eq!(error.step.as_deref(), Some("confirm"));
		assert!(error.txn.is_none(), "nothing was opened, so nothing aborted");
		assert!(!outcome.is_applied());
	}

	#[test]
	fn the_tool_call_helper_round_trips_through_the_table() {
		let args = json!({});
		assert_eq!(call("edit.undo", &args).class, OperationClass::Edit);
	}
}

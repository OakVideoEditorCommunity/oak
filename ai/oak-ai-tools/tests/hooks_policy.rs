//! The three confirmation policies against the mock backend, plus the
//! drift gate between the hooks grading table and the frozen tool face.
//!
//! Every assertion here is about **observable backend state**, not about
//! the return value: a `Spy` wraps the mock and counts the mutating
//! [`EngineBackend`] calls and the transaction primitives, so "proposal-only
//! did not edit anything" is proven by `edits == 0 && begins == 0` plus an
//! unchanged `serialize()` and an unchanged undo depth.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use oak_ai_tools::backend::EngineBackend;
use oak_ai_tools::dispatch::{self, execute};
use oak_ai_tools::error::ToolError;
use oak_ai_tools::hooks::{
	self, BeforeCommit, Decision, EditFailed, FailureEvent, GatedOutcome, GatePoint, Hooks,
	OperationClass, PendingEdit, Policy, ReadbackVerdict, RenderDone, RenderDoneEvent,
};
use oak_ai_tools::mock::MockBackend;
use oak_ai_tools::timecode::{Rat, TimeContext};
use oak_ai_tools::txn::TxnOps;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// spy backend
// ---------------------------------------------------------------------------

/// Wraps the mock and counts what the execution layer asked it to do.
struct Spy<'a> {
	inner: &'a mut MockBackend,
	/// Mutating backend methods entered (all 15 of them).
	edits: usize,
	begins: usize,
	commits: usize,
	aborts: usize,
}

impl<'a> Spy<'a> {
	fn new(inner: &'a mut MockBackend) -> Self {
		Self {
			inner,
			edits: 0,
			begins: 0,
			commits: 0,
			aborts: 0,
		}
	}
}

impl TxnOps for Spy<'_> {
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
		self.begins += 1;
		self.inner.txn_begin(label)
	}
	fn txn_commit(&mut self) -> Result<(), ToolError> {
		self.commits += 1;
		self.inner.txn_commit()
	}
	fn txn_abort(&mut self) -> Result<(), ToolError> {
		self.aborts += 1;
		self.inner.txn_abort()
	}
}

macro_rules! spy_mutators {
	($($method:ident),* $(,)?) => {
		$(
			fn $method(&mut self, args: &Value) -> Result<Value, ToolError> {
				self.edits += 1;
				self.inner.$method(args)
			}
		)*
	};
}

macro_rules! spy_reads {
	($($method:ident),* $(,)?) => {
		$(
			fn $method(&mut self, args: &Value) -> Result<Value, ToolError> {
				self.inner.$method(args)
			}
		)*
	};
}

impl EngineBackend for Spy<'_> {
	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError> {
		self.inner.time_rate(ctx)
	}

	// --- mutations (counted) ------------------------------------------
	spy_mutators!(
		project_open,
		project_save,
		media_import_footage,
		timeline_add_track,
		timeline_place_clip,
		timeline_split_clip,
		timeline_trim_clip,
		timeline_move_clip,
		timeline_ripple_delete,
		timeline_add_transition,
		timeline_add_marker,
		node_add_effect,
		node_set_param,
		export_start,
		edit_undo,
	);

	// --- reads (not counted) ------------------------------------------
	spy_reads!(
		project_get_overview,
		media_probe,
		media_list_footage,
		node_list_types,
		render_get_frame,
		render_get_thumbnails,
		render_get_audio_levels,
	);
}

// ---------------------------------------------------------------------------
// recording hook
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct GateCall {
	tool: String,
	point: GatePoint,
	has_result: bool,
	policy: Policy,
}

#[derive(Default)]
struct Recording {
	gate: Vec<GateCall>,
	answers: VecDeque<Decision>,
	readbacks: Vec<(String, usize)>,
	failures: Vec<(String, String)>,
}

impl BeforeCommit for Recording {
	fn before_commit(&mut self, pending: &PendingEdit<'_>) -> Decision {
		self.gate.push(GateCall {
			tool: pending.tool.to_string(),
			point: pending.point,
			has_result: pending.result.is_some(),
			policy: pending.policy,
		});
		self.answers.pop_front().unwrap_or(Decision::Approve)
	}
}

impl RenderDone for Recording {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		self.readbacks.push((event.tool.to_string(), event.frames.len()));
		ReadbackVerdict::Pass
	}
}

impl EditFailed for Recording {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> oak_ai_tools::hooks::Feedback {
		self.failures
			.push((event.tool.to_string(), event.error.code.clone()));
		oak_ai_tools::hooks::Feedback::from_error(event.error.clone())
	}
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

/// A fresh fixture per call: the tests run in parallel in one process, so
/// a fixed name would let one test delete another's media file.
fn temp_media(tag: &str) -> std::path::PathBuf {
	use std::sync::atomic::{AtomicUsize, Ordering};
	static NEXT: AtomicUsize = AtomicUsize::new(0);
	let path = std::env::temp_dir().join(format!(
		"oak_hooks_{tag}_{}_{}.mp4",
		std::process::id(),
		NEXT.fetch_add(1, Ordering::Relaxed)
	));
	std::fs::write(&path, b"oak hooks media fixture").expect("write fixture");
	path
}

/// A project with one extra video track and one placed clip.
fn seed(backend: &mut MockBackend) -> String {
	execute(backend, "project.open", json!({"name": "hooks", "fps": 30})).expect("open");
	let media = temp_media("policy");
	let out = execute(
		backend,
		"media.import_footage",
		json!({"paths": [media.display().to_string()]}),
	)
	.expect("import");
	let footage = out["footage"][0]["id"].as_str().unwrap().to_string();
	let out = execute(backend, "timeline.add_track", json!({"type": "video"})).expect("track");
	let track = out["track"].as_str().unwrap().to_string();
	let out = execute(
		backend,
		"timeline.place_clip",
		json!({"track": track, "footage": footage, "in": 0}),
	)
	.expect("place");
	let _ = std::fs::remove_file(&media);
	out["clip"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------------------
// ProposalOnly: zero state change (the T4 acceptance point)
// ---------------------------------------------------------------------------

#[test]
fn proposal_only_never_enters_the_backend_for_a_mutating_tool() {
	let mut backend = MockBackend::new();
	let clip = seed(&mut backend);

	let state = backend.serialize();
	let snapshot = backend.serialize_timeline();
	let depth = backend.undo_depth();

	let mut hook = Recording::default();
	let mut results = Vec::new();
	{
		let mut spy = Spy::new(&mut backend);
		for (tool, args) in [
			(
				"timeline.split_clip",
				json!({"clip": clip, "time": {"num": 4, "den": 1}}),
			),
			(
				"project.save",
				json!({"path": "/tmp/oak-hooks-proposal.ove"}),
			),
			(
				"export.start",
				json!({"sequence": "sequence-1", "output_path": "/tmp/oak-hooks.mp4"}),
			),
		] {
			let outcome = dispatch::execute_with(&mut spy, tool, args, Policy::ProposalOnly, &mut hook);
			results.push((tool.to_string(), outcome));
		}
		// Nothing was invoked, not even a transaction was opened.
		assert_eq!(spy.edits, 0, "proposal-only must not enter a mutating method");
		assert_eq!(spy.begins, 0, "proposal-only must not open an undo group");
		assert_eq!(spy.commits, 0);
		assert_eq!(spy.aborts, 0, "there was nothing to abort");
	}

	for (tool, outcome) in results {
		match outcome {
			GatedOutcome::Proposed { proposal } => {
				assert_eq!(proposal.tool, tool);
				assert_eq!(proposal.class, hooks::class_of(&tool).unwrap());
				assert!(proposal.reason.is_none(), "no answer given → no reason");
				let wire = proposal.to_json();
				assert_eq!(wire["tool"], tool);
			}
			other => panic!("{tool} must be a proposal, got {other:?}"),
		}
	}

	// The document, the timeline and the undo stack are untouched.
	assert_eq!(backend.serialize(), state, "proposal-only changed state");
	assert_eq!(backend.serialize_timeline(), snapshot);
	assert_eq!(backend.undo_depth(), depth, "proposal-only pushed a group");

	// The gate was asked once per mutating call, always before the body.
	assert_eq!(hook.gate.len(), 3);
	for call in &hook.gate {
		assert_eq!(call.point, GatePoint::BeforeInvoke);
		assert!(!call.has_result, "nothing ran, so there is no result");
		assert_eq!(call.policy, Policy::ProposalOnly);
	}
	// A rejection from the hook is downgraded to a proposal reason, never
	// to an applied edit.
	assert!(hook.failures.is_empty(), "a proposal is not a failure");
}

#[test]
fn proposal_only_still_runs_reads() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let state = backend.serialize();
	let depth = backend.undo_depth();

	let mut hook = Recording::default();
	let mut spy = Spy::new(&mut backend);
	let outcome = dispatch::execute_with(
		&mut spy,
		"project.get_overview",
		json!({}),
		Policy::ProposalOnly,
		&mut hook,
	);
	assert!(outcome.is_applied(), "reads are auto in every mode: {outcome:?}");
	assert_eq!(spy.edits, 0);
	assert_eq!(spy.begins, 1, "a read is still wrapped in its undo group");
	drop(spy);
	assert!(hook.gate.is_empty(), "the gate never sees a read");
	assert_eq!(backend.serialize(), state);
	// A read commits its (empty) undo group, exactly as before hooks.
	assert_eq!(backend.undo_depth(), depth + 1);
}

#[test]
fn proposal_only_error_surface_is_structured() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let state = backend.serialize();

	// Through the old entry point a proposal is not "success": it must not
	// look like an applied edit.
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.add_track",
		json!({"type": "video"}),
		Policy::ProposalOnly,
		&mut Recording::default(),
	);
	let error = outcome.into_result().unwrap_err();
	assert_eq!(error.code, hooks::CODE_PROPOSAL_ONLY);
	assert_eq!(error.step.as_deref(), Some("confirm"));
	assert!(error.txn.is_none());
	assert_eq!(backend.serialize(), state);
}

// ---------------------------------------------------------------------------
// Confirm: the gate point matrix
// ---------------------------------------------------------------------------

#[test]
fn confirm_matrix_reads_run_edits_gate_at_commit_project_and_export_before() {
	let mut backend = MockBackend::new();
	let _clip = seed(&mut backend);
	let mut hook = Recording::default();

	// (tool, args, expected gate point or None when the gate is skipped)
	let cases: [(&str, Value, Option<GatePoint>); 5] = [
		("project.get_overview", json!({}), None),
		("timeline.add_track", json!({"type": "audio"}), Some(GatePoint::BeforeCommit)),
		(
			"project.save",
			json!({"path": "/tmp/oak-hooks-save.ove"}),
			Some(GatePoint::BeforeInvoke),
		),
		(
			"export.start",
			json!({"sequence": "sequence-1", "output_path": "/tmp/oak-hooks.mp4"}),
			Some(GatePoint::BeforeInvoke),
		),
		("edit.undo", json!({}), Some(GatePoint::BeforeInvoke)),
	];

	for (tool, args, expected) in cases {
		hook.gate.clear();
		let outcome = dispatch::execute_with(&mut backend, tool, args, Policy::Confirm, &mut hook);
		assert!(outcome.is_applied(), "{tool}: {outcome:?}");
		match expected {
			None => assert!(hook.gate.is_empty(), "{tool} must not reach the gate"),
			Some(point) => {
				assert_eq!(hook.gate.len(), 1, "{tool} must be gated exactly once");
				let call = &hook.gate[0];
				assert_eq!(call.tool, tool);
				assert_eq!(call.point, point, "{tool}");
				assert_eq!(call.policy, Policy::Confirm);
				assert_eq!(
					call.has_result,
					point == GatePoint::BeforeCommit,
					"{tool}: a result is only visible at commit time"
				);
			}
		}
	}
}

#[test]
fn auto_never_asks_the_gate_and_keeps_the_old_behaviour() {
	let mut backend = MockBackend::new();
	let clip = seed(&mut backend);
	let mut hook = Recording::default();
	let depth = backend.undo_depth();

	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": clip, "time": {"num": 4, "den": 1}}),
		Policy::Auto,
		&mut hook,
	);
	assert!(outcome.is_applied());
	assert!(hook.gate.is_empty());
	assert_eq!(backend.undo_depth(), depth + 1);

	// `hooks::NoHooks` + `Policy::Auto` is byte-for-byte `execute`.
	let mut other = MockBackend::new();
	let _ = seed(&mut other);
	let via_execute = execute(&mut other, "timeline.add_track", json!({"type": "video"}));
	assert!(via_execute.is_ok());
}

#[test]
fn reject_at_commit_aborts_and_restores_the_document() {
	let mut backend = MockBackend::new();
	let clip = seed(&mut backend);
	let state = backend.serialize();
	let depth = backend.undo_depth();

	let mut hook = Recording::default();
	hook.answers
		.push_back(Decision::reject("user said no to the split"));
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": clip, "time": {"num": 4, "den": 1}}),
		Policy::Confirm,
		&mut hook,
	);

	match &outcome {
		GatedOutcome::Rejected { reason, error } => {
			assert_eq!(reason, "user said no to the split");
			assert_eq!(error.code, hooks::CODE_REJECTED);
			assert_eq!(error.tool.as_deref(), Some("timeline.split_clip"));
			assert_eq!(error.step.as_deref(), Some("confirm"));
			assert_eq!(
				error.txn.as_deref(),
				Some("aborted"),
				"a commit-time rejection must abort the open group"
			);
			assert!(error.message.contains("user said no"), "{}", error.message);
		}
		other => panic!("expected a rejection, got {other:?}"),
	}
	// The body ran (the gate saw its result) but nothing survives.
	assert_eq!(hook.gate.len(), 1);
	assert!(hook.gate[0].has_result);
	assert_eq!(backend.serialize(), state, "abort must restore the state");
	assert_eq!(backend.undo_depth(), depth, "an aborted group is not pushed");
	// Rejections are not failures: `edit_failed` stays silent.
	assert!(hook.failures.is_empty());
}

#[test]
fn reject_before_invoke_leaves_no_trace_at_all() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let state = backend.serialize();
	let depth = backend.undo_depth();

	let mut hook = Recording::default();
	hook.answers.push_back(Decision::reject("no export"));
	let mut spy = Spy::new(&mut backend);
	let outcome = dispatch::execute_with(
		&mut spy,
		"export.start",
		json!({"sequence": "sequence-1", "output_path": "/tmp/oak-hooks.mp4"}),
		Policy::Confirm,
		&mut hook,
	);
	match &outcome {
		GatedOutcome::Rejected { error, .. } => {
			assert_eq!(error.code, hooks::CODE_REJECTED);
			assert_eq!(error.step.as_deref(), Some("confirm"));
			assert!(
				error.txn.is_none(),
				"nothing was opened, so there is no txn marker"
			);
		}
		other => panic!("expected a rejection, got {other:?}"),
	}
	assert_eq!(spy.edits, 0, "the encoder was never asked to start");
	assert_eq!(spy.begins, 0);
	assert_eq!(spy.aborts, 0);
	drop(spy);
	assert_eq!(backend.serialize(), state);
	assert_eq!(backend.undo_depth(), depth);
}

#[test]
fn proposed_at_commit_decides_without_applying() {
	let mut backend = MockBackend::new();
	let clip = seed(&mut backend);
	let state = backend.serialize();
	let depth = backend.undo_depth();

	let mut hook = Recording::default();
	hook.answers.push_back(Decision::Proposed {
		reason: Some("show me first".to_string()),
	});
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": clip, "time": {"num": 4, "den": 1}}),
		Policy::Confirm,
		&mut hook,
	);
	match outcome {
		GatedOutcome::Proposed { proposal } => {
			assert_eq!(proposal.tool, "timeline.split_clip");
			assert_eq!(proposal.reason.as_deref(), Some("show me first"));
			assert_eq!(proposal.args, json!({"clip": clip, "time": {"num": 4, "den": 1}}));
		}
		other => panic!("expected a proposal, got {other:?}"),
	}
	assert_eq!(hook.gate.len(), 1);
	assert!(hook.gate[0].has_result, "the proposal comes after the preview");
	assert_eq!(backend.serialize(), state);
	assert_eq!(backend.undo_depth(), depth);
}

#[test]
fn unknown_tool_fails_closed_without_touching_the_backend() {
	// Coverage note: this exercises the *unknown tool* branch of
	// `execute_with` (name not in the registry → UNKNOWN_TOOL at step
	// `lookup`, backend never entered) plus `class_of` returning `None`
	// for such a name.
	//
	// The `UNCLASSIFIED_TOOL` branch (a *registered* tool with no grading
	// entry) is unreachable on the real tree: all 22 tools are graded and
	// the drift test below keeps it that way, so triggering it would need
	// a registry mutation. Its behaviour was proven with a probe on a
	// copy in the acceptance record (/tmp/oak-reviews/t4-hooks.md) — not
	// here, because a test that cannot fail is not a test.
	assert_eq!(hooks::class_of("timeline.teleport"), None);
	let mut backend = MockBackend::new();
	let mut spy = Spy::new(&mut backend);
	let outcome = dispatch::execute_with(
		&mut spy,
		"timeline.teleport",
		json!({}),
		Policy::Auto,
		&mut Recording::default(),
	);
	match outcome {
		GatedOutcome::Failed { error, .. } => {
			assert_eq!(error.code, oak_ai_tools::error::CODE_UNKNOWN_TOOL);
			assert_eq!(error.step.as_deref(), Some("lookup"));
		}
		other => panic!("unknown tools must fail, got {other:?}"),
	}
	assert_eq!(spy.edits, 0);
	assert_eq!(spy.begins, 0);
}

// ---------------------------------------------------------------------------
// drift gate: grading table ⇄ schema dir ⇄ dispatch registry
// ---------------------------------------------------------------------------

fn schema_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai/schema/tools")
}

#[test]
fn the_grading_table_is_bidirectionally_equal_to_the_frozen_schema_face() {
	let mut schemas = BTreeSet::new();
	for entry in std::fs::read_dir(schema_dir()).expect("schema dir") {
		let path = entry.expect("entry").path();
		if path.extension().is_some_and(|ext| ext == "json") {
			schemas.insert(
				path.file_stem()
					.expect("stem")
					.to_string_lossy()
					.to_string(),
			);
		}
	}

	let graded: BTreeSet<String> = hooks::TOOL_CLASSES
		.iter()
		.map(|(name, _)| name.to_string())
		.collect();
	let registry: BTreeSet<String> = dispatch::tool_names()
		.iter()
		.map(|name| name.to_string())
		.collect();

	assert_eq!(schemas.len(), 22);
	assert_eq!(graded.len(), 22, "the grading table must hold 22 tools");
	assert_eq!(
		graded, schemas,
		"a schema without a grading entry (or the reverse) must fail here"
	);
	assert_eq!(graded, registry, "grading table and registry drifted apart");

	// Every class is represented, and the gate point follows the registry's
	// `transactional` flag rather than a second hand-maintained list.
	let mut seen: Vec<OperationClass> = Vec::new();
	for def in dispatch::registry() {
		let class = hooks::class_of(def.name)
			.unwrap_or_else(|| panic!("{} is not graded", def.name));
		if !seen.contains(&class) {
			seen.push(class);
		}
		let point = class.gate_point(def.transactional);
		if def.name == "edit.undo" {
			assert!(!def.transactional);
			assert_eq!(point, GatePoint::BeforeInvoke, "edit.undo has no group to roll back");
		}
		if class == OperationClass::Edit && def.transactional {
			assert_eq!(point, GatePoint::BeforeCommit, "{}", def.name);
		}
		if matches!(class, OperationClass::Project | OperationClass::Export) {
			assert_eq!(point, GatePoint::BeforeInvoke, "{}", def.name);
		}
	}
	assert_eq!(seen.len(), 4, "all four classes must be used: {seen:?}");
}

/// The trait bundle stays usable as a single injection point.
#[test]
fn hooks_can_be_injected_as_one_object() {
	fn run(_hooks: &mut dyn Hooks) {}
	let mut recording = Recording::default();
	run(&mut recording);
	run(&mut hooks::NoHooks);
}

// ---------------------------------------------------------------------------
// The v2 face's honest bottom line (T9.2, N2)
// ---------------------------------------------------------------------------

/// `Spy` implements the frozen 22 and takes the [`EngineExt`] **defaults** —
/// the exact situation `SdkBackend` is in today.
impl oak_ai_tools::backend::EngineExt for Spy<'_> {}

/// A backend that has not been taught the v2 face must answer with the
/// structured `UNSUPPORTED` + gap id, never a silent `Ok`.
///
/// This is the tripwire for the fail-closed promise: if someone ever
/// rewrites a default body into `Ok(json!({}))`, `SdkBackend` starts
/// claiming it can change a sequence's frame rate when it cannot — a lie
/// the agent would act on. Asserting the code *and* the gap id keeps both
/// halves of that contract.
#[test]
fn unimplemented_v2_tools_report_a_structured_unsupported_gap() {
	use oak_ai_tools::backend::EngineExt;
	use oak_ai_tools::error::CODE_UNSUPPORTED;

	let mut backend = MockBackend::new();
	let mut spy = Spy::new(&mut backend);

	let err = EngineExt::sequence_get_settings(&spy, serde_json::json!({}))
		.expect_err("the default body must refuse, not pretend");
	assert_eq!(err.code, CODE_UNSUPPORTED, "read gap must be UNSUPPORTED");
	assert_eq!(err.gap.as_deref(), Some("GAP_SEQUENCE_SETTINGS"));

	let err = EngineExt::sequence_set_format(&mut spy, serde_json::json!({ "fps": 24 }))
		.expect_err("the default body must refuse, not pretend");
	assert_eq!(err.code, CODE_UNSUPPORTED, "write gap must be UNSUPPORTED");
	assert_eq!(err.gap.as_deref(), Some("GAP_SEQUENCE_SET_FORMAT"));

	// Fail-closed also means "no side effect": the defaults touched nothing.
	assert_eq!(spy.edits, 0, "a refused v2 call must not mutate anything");
}

// ---------------------------------------------------------------------------
// Hint correctness (T9.3): the field bug's root cause was a hint that named
// the wrong cause, so the hints themselves are now asserted.
// ---------------------------------------------------------------------------

/// `INVALID_PARAMS` carries **two** meanings (shape error / content
/// conflict) and one code serves both, so the hint must carry both paths:
/// "fix that field" for a shape error and "re-read the timeline and
/// re-derive this item" for a conflict. Dropping either path turns one of
/// the two families into a dead end — the field bug was the shape half of
/// this, and deleting the re-read half would break the conflict half.
#[test]
fn invalid_params_hint_covers_both_meanings() {
	use oak_ai_tools::error::CODE_INVALID_PARAMS;
	use oak_ai_tools::hooks::Feedback;

	let hint = Feedback::from_error(ToolError::new(CODE_INVALID_PARAMS, "x")).hint;
	let lowered = hint.to_lowercase();
	assert!(
		lowered.contains("message"),
		"it must point at the concrete cause first: {hint}"
	);
	assert!(
		lowered.contains("field"),
		"the shape path must say to fix the offending field: {hint}"
	);
	assert!(
		lowered.contains("overview"),
		"the conflict path must still say to re-read the timeline: {hint}"
	);
	assert!(
		lowered.contains("overlaps") || lowered.contains("out of range"),
		"it must name the conflict symptoms to branch on: {hint}"
	);
}

/// The shape half has its own test: a wrong *type* must not be answered with
/// timeline advice. (The conflict half owns the re-read; see
/// `hooks_replan::a_conflicting_placement_is_fed_back_and_the_retry_succeeds`.)
#[test]
fn a_shape_error_is_answered_with_the_field_path() {
	use oak_ai_tools::error::CODE_INVALID_PARAMS;
	use oak_ai_tools::hooks::Feedback;

	// The exact shape that burned six rounds in the field.
	let hint = Feedback::from_error(ToolError::new(
		CODE_INVALID_PARAMS,
		"`fps` must be a number, a numeric string, or a {\"num\",\"den\"} object, got \"abc\"",
	))
	.hint;
	let lowered = hint.to_lowercase();
	// The branch the model must take for a shape error.
	assert!(
		lowered.contains("field") && (lowered.contains("type") || lowered.contains("shape")),
		"the shape branch must be present: {hint}"
	);
	// And it must be *conditional*, not a blanket instruction to re-read.
	assert!(
		lowered.contains("if "),
		"the two paths must be conditional, not both mandatory: {hint}"
	);
}

/// The backend fallback carries several host causes; it must not assume one.
#[test]
fn backend_hint_is_neutral_about_the_cause() {
	use oak_ai_tools::error::CODE_BACKEND;
	use oak_ai_tools::hooks::Feedback;

	let hint = Feedback::from_error(ToolError::new(CODE_BACKEND, "transaction already held")).hint;
	let lowered = hint.to_lowercase();
	assert!(
		lowered.contains("message"),
		"it must point at the message for the concrete cause: {hint}"
	);
	assert!(
		lowered.contains("transaction") && lowered.contains("capability"),
		"it must branch on the two cause families: {hint}"
	);
	assert!(
		!lowered.contains("adjust one input"),
		"the old one-size advice must be gone: {hint}"
	);
}

/// The stale wordings: the tool face is no longer "the frozen 22" only, and
/// a schema failure is about *that tool's* schema.
#[test]
fn stale_face_wordings_are_gone() {
	use oak_ai_tools::error::{CODE_SCHEMA_VALIDATION, CODE_STATE, CODE_UNKNOWN_TOOL};
	use oak_ai_tools::hooks::Feedback;

	let unknown = Feedback::from_error(ToolError::new(CODE_UNKNOWN_TOOL, "x")).hint;
	assert!(
		unknown.contains("frozen 22") && unknown.to_lowercase().contains("v2"),
		"the unknown-tool hint must name both faces: {unknown}"
	);
	let schema = Feedback::from_error(ToolError::new(CODE_SCHEMA_VALIDATION, "x")).hint;
	assert!(
		!schema.to_lowercase().contains("frozen"),
		"a schema failure is about this tool's schema: {schema}"
	);
	let state = Feedback::from_error(ToolError::new(CODE_STATE, "x")).hint;
	assert!(
		state.to_lowercase().contains("sequence") && state.to_lowercase().contains("select"),
		"the state hint must list the other precondition families: {state}"
	);
}

// ---------------------------------------------------------------------------
// v2 runtime schema gate (T9.3): `additionalProperties: false` must bite.
// ---------------------------------------------------------------------------

/// A misspelled key must be refused **at the gate**, before the transaction
/// opens, and the error must name the offending key so one turn is enough.
#[test]
fn v2_gate_rejects_unknown_keys_and_names_them() {
	use oak_ai_tools::dispatch::execute_v2_with;
	use oak_ai_tools::error::CODE_SCHEMA_VALIDATION;
	use oak_ai_tools::hooks::{GatedOutcome, NoHooks, Policy};

	let mut backend = MockBackend::new();
	EngineBackend::project_open(&mut backend, &serde_json::json!({ "name": "Gate" }))
		.expect("open");

	let outcome = execute_v2_with(
		&mut backend,
		"sequence.set_format",
		serde_json::json!({ "frameRate": 60 }),
		Policy::Auto,
		&mut NoHooks,
	);
	let GatedOutcome::Failed { error, .. } = outcome else {
		panic!("an unknown key must be refused, got {outcome:?}");
	};
	assert_eq!(error.code, CODE_SCHEMA_VALIDATION);
	assert!(
		error.message.contains("frameRate"),
		"the error must name the rejected key: {}",
		error.message
	);
	// No project edit landed: the refusal happened before the backend call.
	let overview = EngineBackend::project_get_overview(&mut backend, &serde_json::Value::Null)
		.expect("overview");
	assert_eq!(overview["sequences"][0]["name"], "Gate", "nothing changed");
}

/// **D2**: the gate must not be stricter than the parser. A numeric string
/// is judged by the parser (which answers `INVALID_PARAMS` with the field
/// path); the schema only checks that it *is* a string. Anything the parser
/// accepts — including `"60 "`, `"1e2"`, `"+60"` — passes the gate, and the
/// one authority for numeric validity is the parser.
#[test]
fn numeric_strings_are_judged_by_the_parser_not_the_gate() {
	use oak_ai_tools::dispatch::execute_v2_with;
	use oak_ai_tools::error::{CODE_INVALID_PARAMS, CODE_SCHEMA_VALIDATION};
	use oak_ai_tools::hooks::{GatedOutcome, NoHooks, Policy};

	for text in ["60", "60 ", "1e2", "+60", "29.97", "abc"] {
		let mut backend = MockBackend::new();
		EngineBackend::project_open(&mut backend, &serde_json::json!({ "name": "D2" }))
			.expect("open");
		let outcome = execute_v2_with(
			&mut backend,
			"sequence.set_format",
			serde_json::json!({ "fps": text }),
			Policy::Auto,
			&mut NoHooks,
		);
		if let GatedOutcome::Failed { error, .. } = outcome {
			assert_ne!(
				error.code, CODE_SCHEMA_VALIDATION,
				"the gate must not judge the number `{text}`: {}",
				error.message
			);
			assert_eq!(
				error.code, CODE_INVALID_PARAMS,
				"an unusable number is a parameter error for `{text}`"
			);
		}
	}
}

/// **O1**: the read/write round trip is the boundary. `get_settings`'s own
/// keys (`rate`, `id`, `duration`, `backend`, `undoable`) are accepted or
/// ignored, so writing a reading straight back works; a misspelled writable
/// key is still refused and named.
#[test]
fn the_read_only_echo_is_ignored_but_a_misspelled_writable_key_is_named() {
	use oak_ai_tools::backend::EngineExt;
	use oak_ai_tools::dispatch::execute_v2_with;
	use oak_ai_tools::error::CODE_SCHEMA_VALIDATION;
	use oak_ai_tools::hooks::{GatedOutcome, NoHooks, Policy};

	// A real `get_settings` payload, written straight back.
	let mut backend = MockBackend::new();
	EngineBackend::project_open(&mut backend, &serde_json::json!({ "name": "Echo" }))
		.expect("open");
	let settings = EngineExt::sequence_get_settings(&backend, serde_json::json!({}))
		.expect("settings");
	let mut write_back = settings.clone();
	if let Some(obj) = write_back.as_object_mut() {
		obj.insert("fps".to_string(), serde_json::json!(24));
	}
	let outcome = execute_v2_with(
		&mut backend,
		"sequence.set_format",
		write_back,
		Policy::Auto,
		&mut NoHooks,
	);
	assert!(
		matches!(outcome, GatedOutcome::Applied { .. }),
		"the read-only echo must not block a write-back: {outcome:?}"
	);

	// The same payload with a misspelled writable key is still refused.
	let mut bad = settings;
	if let Some(obj) = bad.as_object_mut() {
		obj.insert("frameRate".to_string(), serde_json::json!(30));
	}
	let outcome = execute_v2_with(
		&mut backend,
		"sequence.set_format",
		bad,
		Policy::Auto,
		&mut NoHooks,
	);
	let GatedOutcome::Failed { error, .. } = outcome else {
		panic!("a misspelled writable key must be refused, got {outcome:?}");
	};
	assert_eq!(error.code, CODE_SCHEMA_VALIDATION);
	assert!(
		error.message.contains("frameRate"),
		"the misspelled key must be named: {}",
		error.message
	);
}

/// The tolerated spellings the parser accepts must also pass the schema —
/// otherwise the gate would reject what the parser was taught to read.
#[test]
fn v2_gate_accepts_the_tolerated_spellings() {
	use oak_ai_tools::dispatch::execute_v2_with;
	use oak_ai_tools::error::CODE_SCHEMA_VALIDATION;
	use oak_ai_tools::hooks::{NoHooks, Policy};

	for args in [
		serde_json::json!({ "fps": 60 }),
		serde_json::json!({ "fps": "60" }),
		serde_json::json!({ "fps": { "num": 60 } }),
		serde_json::json!({ "sequence": 3, "fps": 60 }),
	] {
		let mut backend = MockBackend::new();
		EngineBackend::project_open(&mut backend, &serde_json::json!({ "name": "Gate" }))
		.expect("open");
		let outcome = execute_v2_with(
			&mut backend,
			"sequence.set_format",
			args.clone(),
			Policy::Auto,
			&mut NoHooks,
		);
		if let oak_ai_tools::hooks::GatedOutcome::Failed { error, .. } = &outcome {
			assert_ne!(
				error.code, CODE_SCHEMA_VALIDATION,
				"the schema must accept {args}: {}",
				error.message
			);
		}
	}
}

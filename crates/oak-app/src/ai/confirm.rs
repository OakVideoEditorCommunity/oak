//! Write-confirmation gate (capability group 1).
//!
//! [`Confirmer`] is the injectable seam: *should this write run?* The
//! execution layer already enforces the answer — under `Policy::Confirm`
//! a [`ConfirmAnswer::Deny`] never reaches the backend (the hooks gate
//! rejects before the body for `project`/`export` and aborts the undo
//! group for `edit`), so "not confirmed ⇒ not executed" is a property of
//! the existing gate, not of this module. What this module adds is:
//!
//! * a named abstraction a host can implement (panel asks the user,
//!   headless says yes, tests say no);
//! * **default policies** for the two non-interactive modes
//!   ([`ConfirmMode::Headless`] / [`ConfirmMode::Strict`]);
//! * an audit trail ([`ConfirmerHooks::asked`]) so a caller can prove
//!   what was asked and how it was answered.
//!
//! Reads never reach the confirmer: the hooks gate only consults
//! `BeforeCommit` for mutating classes.

use oak_ai_tools::hooks::{Hooks, 
	BeforeCommit, Decision, EditFailed, FailureEvent, Feedback, GatePoint, OperationClass,
	PendingEdit, PixelReadback, ReadbackVerdict, RenderDone, RenderDoneEvent,
};
use serde_json::Value;

/// What the gate asks about. Owned, so a host can stash it (UI logs,
/// audit trails) without borrowing the pending edit.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmRequest {
	/// Tool name (`timeline.place_clip`, …).
	pub tool: String,
	/// Its grading (read / edit / project / export).
	pub class: OperationClass,
	/// Where the gate ran (`BeforeInvoke` for project/export/edit.undo,
	/// `BeforeCommit` for transactional edits).
	pub point: GatePoint,
	/// The validated arguments.
	pub args: Value,
	/// Whether a result was visible (true only at `BeforeCommit`).
	pub has_result: bool,
}

/// The answer to one confirmation request.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAnswer {
	/// Run the write.
	Approve,
	/// Refuse it; `reason` is what the model is told.
	Deny {
		reason: String,
	},
	/// Keep it as a proposal instead of applying it.
	Propose {
		reason: Option<String>,
	},
}

/// The injectable confirmation seam: "may this write run?".
///
/// One implementation per host: a panel closure, a headless policy, a
/// test double. The agent itself never decides — it asks.
pub trait Confirmer {
	/// Decide one request. Called once per mutating tool call under
	/// `Policy::Confirm`; reads never arrive here.
	fn confirm(&mut self, request: &ConfirmRequest) -> ConfirmAnswer;
}

/// Headless default: there is no human to ask, so writes are approved.
///
/// The gate still runs (the audit trail records every ask), which is what
/// makes `Policy::Confirm` + [`AutoConfirm`] observably different from
/// `Policy::Auto`: the request list is the proof the gate was consulted.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoConfirm;

impl Confirmer for AutoConfirm {
	fn confirm(&mut self, _request: &ConfirmRequest) -> ConfirmAnswer {
		ConfirmAnswer::Approve
	}
}

/// Strict default: refuse every write. Reads are unaffected (they never
/// reach the gate), so an agent can still plan and inspect.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyWrites;

impl Confirmer for DenyWrites {
	fn confirm(&mut self, request: &ConfirmRequest) -> ConfirmAnswer {
		ConfirmAnswer::Deny {
			reason: format!(
				"`{}` is a write and this run refuses writes; ask the user to switch mode",
				request.tool
			),
		}
	}
}

/// Interactive seam: the host supplies the asking function (a panel's
/// "apply?" dialog, a CI gate file check, …).
///
/// ```
/// use oakapp::ai::confirm::{ConfirmAnswer, ConfirmRequest, Confirmer, Interactive};
///
/// let mut gate = Interactive(|req: &ConfirmRequest| {
///     if req.tool == "export.start" {
///         ConfirmAnswer::Deny { reason: "no exports in this run".into() }
///     } else {
///         ConfirmAnswer::Approve
///     }
/// });
/// let answer = gate.confirm(&ConfirmRequest {
///     tool: "export.start".into(),
///     class: oak_ai_tools::hooks::OperationClass::Export,
///     point: oak_ai_tools::hooks::GatePoint::BeforeInvoke,
///     args: serde_json::json!({}),
///     has_result: false,
/// });
/// assert!(matches!(answer, ConfirmAnswer::Deny { .. }));
/// ```
pub struct Interactive<F>(pub F);

impl<F> Confirmer for Interactive<F>
where
	F: FnMut(&ConfirmRequest) -> ConfirmAnswer,
{
	fn confirm(&mut self, request: &ConfirmRequest) -> ConfirmAnswer {
		(self.0)(request)
	}
}

/// The default policies for a run with no interactive host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmMode {
	/// Headless run: approve writes (nobody to ask), keep the audit trail.
	Headless,
	/// Refuse every write: the agent may read and plan, never mutate.
	Strict,
}

impl ConfirmMode {
	/// The confirmer this mode stands for.
	pub fn confirmer(self) -> Box<dyn Confirmer> {
		match self {
			Self::Headless => Box::new(AutoConfirm),
			Self::Strict => Box::new(DenyWrites),
		}
	}

	/// Stable label for logs/reports.
	pub fn label(self) -> &'static str {
		match self {
			Self::Headless => "headless",
			Self::Strict => "strict",
		}
	}
}

/// [`oak_ai_tools::hooks::Hooks`] = a [`Confirmer`] + the built-in pixel
/// readback + the standard failure feedback.
///
/// Hand it to [`crate::ai::agent::Agent::run_with`] together with
/// `Policy::Confirm`:
///
/// ```
/// use oakapp::ai::confirm::{AutoConfirm, ConfirmerHooks};
/// use oak_ai_tools::hooks::Policy;
///
/// let mut hooks = ConfirmerHooks::new(AutoConfirm);
/// // agent.run_with("cut this", &mut hooks)   // with AgentConfig { policy: Policy::Confirm, .. }
/// let _ = (Policy::Confirm, hooks.asked());
/// ```
pub struct ConfirmerHooks<C> {
	confirmer: C,
	readback: PixelReadback,
	asked: Vec<ConfirmRequest>,
}

impl<C> ConfirmerHooks<C> {
	/// Wraps `confirmer` with the structural pixel readback (geometry +
	/// byte count; no black-frame threshold — a gap frame is a legitimate
	/// answer in this product).
	pub fn new(confirmer: C) -> Self {
		Self {
			confirmer,
			readback: PixelReadback::structural(),
			asked: Vec::new(),
		}
	}

	/// Every request the gate asked, in order — the audit trail.
	pub fn asked(&self) -> &[ConfirmRequest] {
		&self.asked
	}

	/// Replace the pixel-readback preset used for `render_done`.
	pub fn with_readback(mut self, readback: PixelReadback) -> Self {
		self.readback = readback;
		self
	}
}

impl<C: Confirmer> BeforeCommit for ConfirmerHooks<C> {
	fn before_commit(&mut self, pending: &PendingEdit<'_>) -> Decision {
		let request = ConfirmRequest {
			tool: pending.tool.to_string(),
			class: pending.class,
			point: pending.point,
			args: pending.args.clone(),
			has_result: pending.result.is_some(),
		};
		let answer = self.confirmer.confirm(&request);
		self.asked.push(request);
		match answer {
			ConfirmAnswer::Approve => Decision::Approve,
			ConfirmAnswer::Deny { reason } => Decision::Reject { reason },
			ConfirmAnswer::Propose { reason } => Decision::Proposed { reason },
		}
	}
}

impl<C: Confirmer> RenderDone for ConfirmerHooks<C> {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		self.readback.render_done(event)
	}
}

impl<C: Confirmer> EditFailed for ConfirmerHooks<C> {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> Feedback {
		Feedback::from_error(event.error.clone())
	}
}

/// The production gate: the run's [`ConfirmMode`] confirmer is asked
/// before the caller's hooks — `Approve` falls through to the caller
/// (which may still veto, e.g. pixel checks), `Deny`/`Propose` stop the
/// call right there. Reads never reach it (the engine does not call
/// `before_commit` for them, and [`Policy::Auto`] never asks either).
///
/// `Agent::run` / `Agent::run_with` wrap the caller's hooks in this.
pub struct GatedHooks<'a> {
	confirmer: Box<dyn Confirmer>,
	inner: &'a mut dyn Hooks,
	asked: Vec<ConfirmRequest>,
}

impl<'a> GatedHooks<'a> {
	/// `mode` decides what happens to writes; `inner` keeps the caller's
	/// hooks in charge of render/failure feedback.
	pub fn new(mode: ConfirmMode, inner: &'a mut dyn Hooks) -> Self {
		Self {
			confirmer: mode.confirmer(),
			inner,
			asked: Vec::new(),
		}
	}

	/// Every request the gate asked, in order — the audit trail.
	pub fn asked(&self) -> &[ConfirmRequest] {
		&self.asked
	}
}

impl BeforeCommit for GatedHooks<'_> {
	fn before_commit(&mut self, pending: &PendingEdit<'_>) -> Decision {
		let request = ConfirmRequest {
			tool: pending.tool.to_string(),
			class: pending.class,
			point: pending.point,
			args: pending.args.clone(),
			has_result: pending.result.is_some(),
		};
		let answer = self.confirmer.confirm(&request);
		self.asked.push(request);
		match answer {
			ConfirmAnswer::Approve => self.inner.before_commit(pending),
			ConfirmAnswer::Deny { reason } => Decision::Reject { reason },
			ConfirmAnswer::Propose { reason } => Decision::Proposed { reason },
		}
	}
}

impl RenderDone for GatedHooks<'_> {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		self.inner.render_done(event)
	}
}

impl EditFailed for GatedHooks<'_> {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> Feedback {
		self.inner.edit_failed(event)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ai::agent::{Agent, AgentConfig};
	use crate::ai::llm::{ChatRequest, ChatResponse, LlmClient, LlmError};
	use oak_ai_tools::hooks::Policy;
	use oak_ai_tools::mock;
	use serde_json::json;
	use std::cell::RefCell;

	/// A model that asks for exactly one tool call and then answers.
	struct OneCall {
		tool: String,
		args: Value,
		asked: RefCell<usize>,
	}

	impl LlmClient for OneCall {
		fn complete(&self, _request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError> {
			let first = self.asked.replace(1) == 0;
			Ok(if first {
				ChatResponse {
					content: None,
					tool_calls: vec_tools(self.tool.clone(), self.args.clone()),
					finish_reason: Some("tool_calls".into()),
				}
			} else {
				ChatResponse {
					content: Some("done".into()),
					tool_calls: vec_tools(String::new(), json!({})),
					finish_reason: Some("stop".into()),
				}
			})
		}
	}

	fn vec_tools(name: String, args: Value) -> Vec<crate::ai::llm::ToolCall> {
		if name.is_empty() {
			Vec::new()
		} else {
			vec![crate::ai::llm::ToolCall {
				id: "call_1".into(),
				name,
				arguments: args,
			}]
		}
	}

	fn config(policy: Policy) -> AgentConfig {
		AgentConfig {
			max_rounds: 4,
			policy,
			model: "test-model".into(),
			confirm: ConfirmMode::Headless,
		}
	}

	/// A project with one track, so mutating calls have somewhere to go.
	fn seeded() -> mock::MockBackend {
		let mut backend = mock::MockBackend::new();
		dispatch_open(&mut backend);
		backend
	}

	fn dispatch_open(backend: &mut mock::MockBackend) {
		oak_ai_tools::dispatch::execute(
			backend,
			"project.open",
			json!({"name": "confirm-test", "fps": 30}),
		)
		.expect("open");
	}

	/// ① Headless: the gate is consulted and approves; the write runs.
	#[test]
	fn headless_mode_approves_and_the_write_runs() {
		let mut agent = Agent::new(
			OneCall {
				tool: "timeline.add_track".into(),
				args: json!({"type": "video"}),
				asked: RefCell::new(0),
			},
			seeded(),
			config(Policy::Confirm),
		);
		let mut hooks = ConfirmerHooks::new(AutoConfirm);
		let report = agent.run_with("add a track", &mut hooks).expect("run");

		assert_eq!(report.tools.len(), 1);
		assert_eq!(
			report.tools[0].outcome, "applied",
			"{}",
			report.tools[0].detail
		);
		// The gate really ran, exactly once, and saw the right request.
		assert_eq!(hooks.asked().len(), 1, "one write ⇒ one confirmation");
		let asked = &hooks.asked()[0];
		assert_eq!(asked.tool, "timeline.add_track");
		assert_eq!(asked.class, OperationClass::Edit);
		assert_eq!(
			asked.point,
			GatePoint::BeforeCommit,
			"a transactional edit is gated on its result, before the commit"
		);
		assert!(asked.has_result, "at BeforeCommit the result is visible");
	}

	/// ② Strict: the deny answer must mean **not executed** — the backend
	/// state and the undo stack are byte-identical afterwards.
	#[test]
	fn strict_mode_denies_and_nothing_is_executed() {
		let mut agent = Agent::new(
			OneCall {
				tool: "timeline.add_track".into(),
				args: json!({"type": "video"}),
				asked: RefCell::new(0),
			},
			seeded(),
			config(Policy::Confirm),
		);
		let before = agent.backend().serialize_timeline();
		let depth = agent.backend().undo_depth();

		let mut hooks = ConfirmerHooks::new(DenyWrites);
		let report = agent.run_with("add a track", &mut hooks).expect("run");

		assert_eq!(report.tools[0].outcome, "rejected", "{}", report.tools[0].detail);
		assert!(report.tools[0].detail.contains("refuses writes"), "{}", report.tools[0].detail);
		assert_eq!(hooks.asked().len(), 1);
		assert_eq!(
			agent.backend().serialize_timeline(),
			before,
			"a denied write must not touch the timeline"
		);
		assert_eq!(
			agent.backend().undo_depth(),
			depth,
			"a denied write must not push an undo group"
		);
	}

	/// ③ Reads are never asked about (the gate only sees mutating calls).
	#[test]
	fn reads_never_reach_the_confirmer() {
		let mut agent = Agent::new(
			OneCall {
				tool: "node.list_types".into(),
				args: json!({}),
				asked: RefCell::new(0),
			},
			seeded(),
			config(Policy::Confirm),
		);
		let mut hooks = ConfirmerHooks::new(DenyWrites);
		let report = agent.run_with("what nodes exist?", &mut hooks).expect("run");
		assert_eq!(report.tools[0].outcome, "applied", "{}", report.tools[0].detail);
		assert!(hooks.asked().is_empty(), "reads must not be confirmed");
	}

	/// ④ The interactive seam receives the full request.
	#[test]
	fn the_interactive_seam_sees_tool_class_and_args() {
		// Shared: the closure borrows the log mutably for as long as the
		// hooks live, so the assertions read a second handle.
		let seen: std::sync::Arc<std::sync::Mutex<Vec<ConfirmRequest>>> =
			std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
		let seen_in_closure = std::sync::Arc::clone(&seen);
		let mut hooks = ConfirmerHooks::new(Interactive(move |req: &ConfirmRequest| {
			seen_in_closure
				.lock()
				.expect("poisoned")
				.push(req.clone());
			ConfirmAnswer::Deny {
				reason: "user clicked cancel".into(),
			}
		}));
		let mut agent = Agent::new(
			OneCall {
				tool: "timeline.add_track".into(),
				args: json!({"type": "audio"}),
				asked: RefCell::new(0),
			},
			seeded(),
			config(Policy::Confirm),
		);
		let report = agent.run_with("add a track", &mut hooks).expect("run");
		assert_eq!(report.tools[0].outcome, "rejected");
		// The closure captured its own copy; the hook keeps its trail too.
		let seen = seen.lock().expect("poisoned");
		assert_eq!(seen.len(), 1);
		assert_eq!(seen[0].args, json!({"type": "audio"}));
		assert_eq!(seen[0].class, OperationClass::Edit);
		assert_eq!(hooks.asked().len(), 1);
		assert!(report.tools[0].detail.contains("user clicked cancel"));
	}

	/// ⑤ Under `Policy::Auto` the confirmer is not consulted at all —
	/// mode and policy are separate knobs.
	#[test]
	fn auto_policy_never_consults_the_confirmer() {
		let mut agent = Agent::new(
			OneCall {
				tool: "timeline.add_track".into(),
				args: json!({"type": "video"}),
				asked: RefCell::new(0),
			},
			seeded(),
			config(Policy::Auto),
		);
		let mut hooks = ConfirmerHooks::new(DenyWrites);
		let report = agent.run_with("add a track", &mut hooks).expect("run");
		assert_eq!(report.tools[0].outcome, "applied", "{}", report.tools[0].detail);
		assert!(hooks.asked().is_empty());
	}

	/// ⑥ The two default modes build their confirmers.
	#[test]
	fn confirm_mode_builds_its_confirmer() {
		let mut headless = ConfirmMode::Headless.confirmer();
		let mut strict = ConfirmMode::Strict.confirmer();
		let request = ConfirmRequest {
			tool: "project.save".into(),
			class: OperationClass::Project,
			point: GatePoint::BeforeInvoke,
			args: json!({}),
			has_result: false,
		};
		assert_eq!(headless.confirm(&request), ConfirmAnswer::Approve);
		assert!(matches!(strict.confirm(&request), ConfirmAnswer::Deny { .. }));
		assert_eq!(ConfirmMode::Headless.label(), "headless");
	}
}

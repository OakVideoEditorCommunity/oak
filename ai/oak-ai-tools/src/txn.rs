//! Transaction orchestration: **one tool call = one undo group**.
//!
//! The dispatcher wraps every transactional tool call in
//! `begin → invoke → commit`, and on any failure runs `abort` so the
//! backend is left exactly as it was before the call (iron rule: a failed
//! edit never leaves a half-applied edit behind). The failure is then
//! annotated into the structured `{code, message, tool, step}` shape the
//! re-planning loop consumes.
//!
//! `txn` is never exposed to the model — the adapter owns it
//! (ai-agent-design §3 / method-map table 3).

use serde_json::Value;

use crate::error::{ToolError, CODE_TRANSACTION};

/// The transactional half of a backend: begin / commit / abort.
///
/// Declared as a supertrait of [`crate::backend::EngineBackend`] so that
/// this module can be exercised against a three-method fake in tests and
/// against a real backend in production.
pub trait TxnOps {
	/// Opens an undo group labelled `label` (the tool name).
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError>;
	/// Closes the group, keeping it on the undo stack.
	fn txn_commit(&mut self) -> Result<(), ToolError>;
	/// Discards the group, restoring the pre-`begin` state.
	fn txn_abort(&mut self) -> Result<(), ToolError>;
}

/// Runs `call` as a single undo group.
///
/// * success: `begin → call → commit`, result passed through;
/// * failure: `begin → call → abort`, the error keeps its original
///   `code` and gains `tool` / `step` / `txn: "aborted"`.
///
/// A failing `begin` short-circuits (nothing to abort); a failing
/// `commit` is reported as `TRANSACTION_FAILED` at step `commit`.
pub fn run<T, F>(ops: &mut T, tool: &str, call: F) -> Result<Value, ToolError>
where
	T: TxnOps + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	ops.txn_begin(tool)
		.map_err(|err| err.with_tool(tool).with_step("begin"))?;
	match call(ops) {
		Ok(value) => {
			ops.txn_commit()
				.map_err(|err| err.with_tool(tool).with_step("commit"))?;
			Ok(value)
		}
		Err(err) => {
			// A failing abort must not mask the original failure: the
			// edit error is what the model needs to re-plan.
			let _ = ops.txn_abort();
			Err(err.with_tool(tool).with_step("invoke").aborted())
		}
	}
}

/// What to do with a group whose body succeeded.
///
/// Consumed by [`run_gated`]'s decision closure: `Commit` keeps the group,
/// `Abort` rolls it back (`txn_abort`), which is how the confirmation gate
/// (`crate::hooks`) refuses an edit *after* seeing its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
	/// Keep the group on the undo stack.
	Commit,
	/// Discard the group, restoring the pre-`begin` state.
	Abort,
}

/// Outcome of [`run_gated`]: the body's value plus what happened to the
/// group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupOutcome {
	/// The group was committed; the body's result is kept.
	Committed(Value),
	/// The decision said abort; the body's result is still returned (a
	/// proposal may want to show what it would have been), the state is
	/// restored.
	Aborted(Value),
}

/// Runs `call` as one undo group, but lets `decide` veto the commit after
/// the body ran.
///
/// Identical to [`run`] up to and including the body: `begin → call`, a
/// failing body is aborted and annotated exactly as in [`run`]. On success
/// `decide(&value)` picks [`Verdict::Commit`] (commit, keep the value) or
/// [`Verdict::Abort`] (abort, return the value). A failing `commit` is
/// reported as `TRANSACTION_FAILED` at step `commit`.
///
/// This is the primitive the adapter-layer confirmation gate needs: the
/// gate wants to see the edit *before* it lands, which is only possible
/// while the group is still open.
pub fn run_gated<T, F, D>(ops: &mut T, tool: &str, call: F, decide: D) -> Result<GroupOutcome, ToolError>
where
	T: TxnOps + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
	D: FnOnce(&Value) -> Verdict,
{
	ops.txn_begin(tool)
		.map_err(|err| err.with_tool(tool).with_step("begin"))?;
	let value = match call(ops) {
		Ok(value) => value,
		Err(err) => {
			// Same contract as `run`: a failing body never leaves a group.
			let _ = ops.txn_abort();
			return Err(err.with_tool(tool).with_step("invoke").aborted());
		}
	};
	match decide(&value) {
		Verdict::Commit => {
			ops.txn_commit()
				.map_err(|err| err.with_tool(tool).with_step("commit"))?;
			Ok(GroupOutcome::Committed(value))
		}
		Verdict::Abort => {
			let _ = ops.txn_abort();
			Ok(GroupOutcome::Aborted(value))
		}
	}
}

/// Runs a non-transactional call (currently only `edit.undo`, which per
/// OPP/1 §6 has no transaction of its own). Failures are annotated the
/// same way, minus the `txn` marker.
pub fn run_plain<T, F>(ops: &mut T, tool: &str, call: F) -> Result<Value, ToolError>
where
	T: TxnOps + ?Sized,
	F: FnOnce(&mut T) -> Result<Value, ToolError>,
{
	call(ops).map_err(|err| err.with_tool(tool).with_step("invoke"))
}

/// Guard used by backends that must not forget to close a group.
///
/// `finish` is `commit` on success; dropping an unfinished guard aborts,
/// so an early `?` inside a backend cannot leave an open undo group.
pub struct GroupGuard<'a, T: TxnOps + ?Sized> {
	ops: &'a mut T,
	finished: bool,
}

impl<'a, T: TxnOps + ?Sized> GroupGuard<'a, T> {
	/// Opens the group.
	pub fn begin(ops: &'a mut T, label: &str) -> Result<Self, ToolError> {
		ops.txn_begin(label)?;
		Ok(Self { ops, finished: false })
	}

	/// Closes the group successfully.
	pub fn commit(mut self) -> Result<(), ToolError> {
		self.finished = true;
		self.ops.txn_commit()
	}

	/// Aborts explicitly (the result is informational: the state is
	/// restored either way).
	pub fn abort(mut self) -> Result<(), ToolError> {
		self.finished = true;
		self.ops.txn_abort()
	}
}

impl<'a, T: TxnOps + ?Sized> Drop for GroupGuard<'a, T> {
	fn drop(&mut self) {
		if !self.finished {
			let _ = self.ops.txn_abort();
		}
	}
}

/// Wraps a backend failure that happened outside any tool invocation.
pub fn plumbing_error(message: impl Into<String>) -> ToolError {
	ToolError::new(CODE_TRANSACTION, message)
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	/// A three-method fake: records the call order and can be told to
	/// fail at a chosen point.
	#[derive(Default)]
	struct FakeTxn {
		calls: Vec<String>,
		fail_begin: bool,
		fail_commit: bool,
		aborted_state: u32,
		committed_state: u32,
	}

	impl TxnOps for FakeTxn {
		fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
			self.calls.push(format!("begin:{label}"));
			if self.fail_begin {
				return Err(ToolError::new(CODE_TRANSACTION, "begin failed"));
			}
			Ok(())
		}
		fn txn_commit(&mut self) -> Result<(), ToolError> {
			self.calls.push("commit".to_string());
			if self.fail_commit {
				return Err(ToolError::new(CODE_TRANSACTION, "commit failed"));
			}
			self.committed_state += 1;
			Ok(())
		}
		fn txn_abort(&mut self) -> Result<(), ToolError> {
			self.calls.push("abort".to_string());
			self.aborted_state += 1;
			Ok(())
		}
	}

	#[test]
	fn success_is_begin_commit() {
		let mut fake = FakeTxn::default();
		let out = run(&mut fake, "media.list_footage", |_| Ok(json!({"ok": true}))).unwrap();
		assert_eq!(out, json!({"ok": true}));
		assert_eq!(fake.calls, vec!["begin:media.list_footage", "commit"]);
		assert_eq!(fake.committed_state, 1);
		assert_eq!(fake.aborted_state, 0);
	}

	#[test]
	fn failure_is_begin_invoke_abort_and_keeps_the_code() {
		let mut fake = FakeTxn::default();
		let err = run(&mut fake, "timeline.split_clip", |_| {
			Err(ToolError::not_found("no such clip"))
		})
		.unwrap_err();
		assert_eq!(fake.calls, vec!["begin:timeline.split_clip", "abort"]);
		assert_eq!(err.code, crate::error::CODE_NOT_FOUND);
		assert_eq!(err.tool.as_deref(), Some("timeline.split_clip"));
		assert_eq!(err.step.as_deref(), Some("invoke"));
		assert_eq!(err.txn.as_deref(), Some("aborted"));
		let wire = err.to_json();
		assert_eq!(wire["code"], "NOT_FOUND");
		assert_eq!(wire["tool"], "timeline.split_clip");
		assert_eq!(wire["step"], "invoke");
		assert_eq!(wire["txn"], "aborted");
	}

	#[test]
	fn a_failing_begin_never_reaches_the_body_or_abort() {
		let mut fake = FakeTxn {
			fail_begin: true,
			..Default::default()
		};
		let err = run(&mut fake, "project.open", |_| Ok(json!({}))).unwrap_err();
		assert_eq!(err.step.as_deref(), Some("begin"));
		assert_eq!(fake.calls, vec!["begin:project.open"]);
	}

	#[test]
	fn a_failing_commit_is_reported_at_the_commit_step() {
		let mut fake = FakeTxn {
			fail_commit: true,
			..Default::default()
		};
		let err = run(&mut fake, "project.save", |_| Ok(json!({}))).unwrap_err();
		assert_eq!(err.code, CODE_TRANSACTION);
		assert_eq!(err.step.as_deref(), Some("commit"));
	}

	#[test]
	fn plain_calls_have_no_transaction_marker() {
		let mut fake = FakeTxn::default();
		let err = run_plain(&mut fake, "edit.undo", |_| {
			Err(ToolError::new(crate::error::CODE_NOTHING_TO_UNDO, "empty"))
		})
		.unwrap_err();
		assert_eq!(err.step.as_deref(), Some("invoke"));
		assert!(err.txn.is_none());
		assert!(fake.calls.is_empty());
	}

	#[test]
	fn group_guard_aborts_on_drop() {
		let mut fake = FakeTxn::default();
		{
			let _guard = GroupGuard::begin(&mut fake, "x").unwrap();
			// dropped without commit
		}
		assert_eq!(fake.calls, vec!["begin:x", "abort"]);
	}
}

//! `edit_failed`: fail → abort → structured feedback → re-plan → success.
//!
//! This is the closed loop plan §4 asks for: the failure is aborted (state
//! restored), fed back as the existing `{code, message, tool, step, txn}`
//! object plus `retryable` / `hint`, and the next attempt — built from the
//! hint — succeeds.

use oak_ai_tools::dispatch::{self, execute};
use oak_ai_tools::hooks::{
	self, BeforeCommit, Decision, EditFailed, FailureEvent, GatedOutcome, PendingEdit, Policy,
	ReadbackVerdict, RenderDone, RenderDoneEvent,
};
use oak_ai_tools::mock::MockBackend;
use serde_json::{Value, json};

#[derive(Default)]
struct Recording {
	gate: Vec<(String, Decision)>,
	answers: Vec<Decision>,
	failures: Vec<(String, String, bool)>,
}

impl BeforeCommit for Recording {
	fn before_commit(&mut self, pending: &PendingEdit<'_>) -> Decision {
		let answer = self.answers.first().cloned().unwrap_or(Decision::Approve);
		if !self.answers.is_empty() {
			self.answers.remove(0);
		}
		self.gate.push((pending.tool.to_string(), answer.clone()));
		answer
	}
}

impl RenderDone for Recording {
	fn render_done(&mut self, _event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		ReadbackVerdict::Pass
	}
}

impl EditFailed for Recording {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> hooks::Feedback {
		let feedback = hooks::Feedback::from_error(event.error.clone());
		self.failures.push((
			event.tool.to_string(),
			event.error.code.clone(),
			feedback.retryable,
		));
		feedback
	}
}

/// A fresh fixture per call: the tests run in parallel in one process.
fn temp_media(tag: &str) -> std::path::PathBuf {
	use std::sync::atomic::{AtomicUsize, Ordering};
	static NEXT: AtomicUsize = AtomicUsize::new(0);
	let path = std::env::temp_dir().join(format!(
		"oak_hooks_rp_{tag}_{}_{}.mp4",
		std::process::id(),
		NEXT.fetch_add(1, Ordering::Relaxed)
	));
	std::fs::write(&path, b"oak hooks replan media fixture").expect("write fixture");
	path
}

/// Opens a project with one video track holding one clip at t=0 (10 s).
/// Returns `(track id, clip id)`.
fn seed(backend: &mut MockBackend) -> (String, String) {
	execute(backend, "project.open", json!({"name": "replan", "fps": 30})).expect("open");
	let media = temp_media("replan");
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
	(track, out["clip"].as_str().unwrap().to_string())
}

/// Every clip of the active sequence, flattened across tracks.
fn clips(overview: &Value) -> Vec<Value> {
	overview["sequences"][0]["tracks"]
		.as_array()
		.expect("tracks")
		.iter()
		.flat_map(|track| {
			track["clips"]
				.as_array()
				.cloned()
				.unwrap_or_default()
		})
		.collect()
}

fn seconds(value: &Value) -> f64 {
	let num = value["num"].as_f64().expect("num");
	let den = value["den"].as_f64().expect("den");
	num / den
}

#[test]
fn a_missing_clip_is_aborted_fed_back_and_the_retry_succeeds() {
	let mut backend = MockBackend::new();
	let (track, clip) = seed(&mut backend);
	let state = backend.serialize();
	let snapshot = backend.serialize_timeline();
	let depth = backend.undo_depth();
	let mut hook = Recording::default();

	// --- attempt 1: stale id → NOT_FOUND ------------------------------
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": "clip-404", "time": {"num": 4, "den": 1}}),
		Policy::Auto,
		&mut hook,
	);
	let feedback = match outcome {
		GatedOutcome::Failed { error, feedback } => {
			assert_eq!(error.code, oak_ai_tools::error::CODE_NOT_FOUND);
			assert_eq!(error.tool.as_deref(), Some("timeline.split_clip"));
			assert_eq!(error.step.as_deref(), Some("invoke"));
			assert_eq!(error.txn.as_deref(), Some("aborted"));
			feedback
		}
		other => panic!("expected a failure, got {other:?}"),
	};

	// Abort means no trace: same document, same undo stack.
	assert_eq!(backend.serialize(), state);
	assert_eq!(backend.undo_depth(), depth);
	// Feedback keeps the error object and adds the two advisory keys.
	assert!(feedback.retryable, "a stale id is retryable after a re-read");
	assert!(feedback.hint.contains("overview"), "{}", feedback.hint);
	let wire = feedback.to_json();
	assert_eq!(wire["code"], "NOT_FOUND");
	assert_eq!(wire["txn"], "aborted");
	assert_eq!(wire["retryable"], true);
	assert!(wire["hint"].as_str().unwrap().contains("overview"));
	assert_eq!(
		hook.failures,
		vec![("timeline.split_clip".to_string(), "NOT_FOUND".to_string(), true)]
	);

	// --- re-plan: follow the hint -------------------------------------
	let overview = execute(&mut backend, "project.get_overview", json!({}))
		.expect("overview after the failure");
	let live = clips(&overview)
		.into_iter()
		.find(|item| item["id"] == clip.as_str())
		.expect("the real clip is in the overview");
	let start = seconds(&live["in"]);
	let end = seconds(&live["out"]);
	let midpoint = ((start + end) / 2.0).round();
	assert!(midpoint > start && midpoint < end);

	// --- attempt 2: the same intent, corrected from the readback ------
	hook.answers.clear();
	let retry = dispatch::execute_with(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": clip, "time": {"num": midpoint as i64, "den": 1}}),
		Policy::Auto,
		&mut hook,
	);
	let new_clip = match retry {
		GatedOutcome::Applied { result } => {
			result["new_clip"].as_str().unwrap().to_string()
		}
		other => panic!("the corrected retry must succeed, got {other:?}"),
	};
	assert_ne!(new_clip, clip);
	assert_eq!(
		backend.undo_depth(),
		depth + 2,
		"the overview read and the successful split each committed a group"
	);
	let after = clips(&execute(&mut backend, "project.get_overview", json!({})).unwrap());
	assert_eq!(after.len(), 2, "the clip was actually split");
	// The failed attempt left no history row behind (depth +2, not +3).
	assert_eq!(hook.failures.len(), 1, "a success does not report a failure");

	assert_ne!(
		backend.serialize_timeline(),
		snapshot,
		"the successful split changed the timeline"
	);
	assert!(hook.gate.is_empty(), "Policy::Auto must not consult the gate");
	let _ = track;
}

#[test]
fn a_conflicting_placement_is_fed_back_and_the_retry_succeeds() {
	let mut backend = MockBackend::new();
	let (track, _clip) = seed(&mut backend);
	let depth = backend.undo_depth();
	let mut hook = Recording::default();

	// Overlapping placement → INVALID_PARAMS.
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.place_clip",
		json!({"track": track, "footage": "footage-1", "in": 5}),
		Policy::Auto,
		&mut hook,
	);
	let feedback = match outcome {
		GatedOutcome::Failed { error, feedback } => {
			assert_eq!(error.code, oak_ai_tools::error::CODE_INVALID_PARAMS);
			assert_eq!(error.txn.as_deref(), Some("aborted"));
			feedback
		}
		other => panic!("expected a conflict, got {other:?}"),
	};
	assert!(feedback.retryable);
	assert!(feedback.hint.contains("overview"), "{}", feedback.hint);
	assert_eq!(backend.undo_depth(), depth);

	// Re-read, find the first free second after the existing clip.
	let overview = execute(&mut backend, "project.get_overview", json!({})).unwrap();
	let busy_until = clips(&overview)
		.iter()
		.map(|item| seconds(&item["out"]))
		.fold(0.0f64, f64::max);
	assert_eq!(busy_until, 10.0);

	let retry = dispatch::execute_with(
		&mut backend,
		"timeline.place_clip",
		json!({"track": track, "footage": "footage-1", "in": {"num": busy_until as i64, "den": 1}}),
		Policy::Auto,
		&mut hook,
	);
	assert!(matches!(retry, GatedOutcome::Applied { .. }), "{retry:?}");
	assert_eq!(clips(&execute(&mut backend, "project.get_overview", json!({})).unwrap()).len(), 2);
	assert!(hook.gate.is_empty(), "Policy::Auto must not consult the gate");
}

#[test]
fn a_user_rejection_is_not_fed_back_as_a_failure() {
	let mut backend = MockBackend::new();
	let (_, _clip) = seed(&mut backend);
	let depth = backend.undo_depth();
	let mut hook = Recording::default();
	hook.answers.push(Decision::reject("try another track"));
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.add_track",
		json!({"type": "audio"}),
		Policy::Confirm,
		&mut hook,
	);
	match &outcome {
		GatedOutcome::Rejected { reason, error } => {
			assert_eq!(reason, "try another track");
			assert_eq!(error.code, hooks::CODE_REJECTED);
			assert_eq!(error.step.as_deref(), Some("confirm"));
			assert_eq!(error.txn.as_deref(), Some("aborted"));
		}
		other => panic!("expected a rejection, got {other:?}"),
	}
	assert!(hook.failures.is_empty(), "a rejection is a decision, not a failure");
	assert_eq!(backend.undo_depth(), depth, "the rejected group was rolled back");

	// The model can re-plan and be approved this time.
	let retry = dispatch::execute_with(
		&mut backend,
		"timeline.add_track",
		json!({"type": "video"}),
		Policy::Confirm,
		&mut hook,
	);
	assert!(retry.is_applied(), "{retry:?}");
	assert_eq!(backend.undo_depth(), depth + 1);
}

#[test]
fn every_failure_code_carries_a_retry_verdict_and_a_hint() {
	// Deterministic classification: no I/O, no state, so the same input
	// always yields the same advice.
	for (code, retryable) in [
		(oak_ai_tools::error::CODE_SCHEMA_VALIDATION, true),
		(oak_ai_tools::error::CODE_TIMECODE, true),
		(oak_ai_tools::error::CODE_NOT_FOUND, true),
		(oak_ai_tools::error::CODE_INVALID_PARAMS, true),
		(oak_ai_tools::error::CODE_STATE, true),
		(oak_ai_tools::error::CODE_BACKEND, true),
		(oak_ai_tools::error::CODE_IO, true),
		(oak_ai_tools::error::CODE_UNKNOWN_TOOL, false),
		(oak_ai_tools::error::CODE_UNSUPPORTED, false),
		(oak_ai_tools::error::CODE_NOTHING_TO_UNDO, false),
		(hooks::CODE_REJECTED, false),
		(hooks::CODE_PROPOSAL_ONLY, false),
		(hooks::CODE_READBACK_FAILED, true),
	] {
		let feedback = hooks::Feedback::from_error(oak_ai_tools::error::ToolError::new(code, "x"));
		assert_eq!(feedback.retryable, retryable, "{code}");
		assert!(!feedback.hint.is_empty(), "{code}");
		assert_eq!(feedback.to_json()["code"], code);
	}
}

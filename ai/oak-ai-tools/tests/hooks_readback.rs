//! `render_done`: the readback assertion trigger.
//!
//! The hook runs after the backend returned the frame and before the undo
//! group commits, so a frame that violates the assertions is rolled back and
//! reported as `READBACK_ASSERTION_FAILED` — the same structured failure
//! path the re-planning loop consumes.

use oak_ai_tools::dispatch::{self, execute};
use oak_ai_tools::hooks::{
	self, BeforeCommit, Decision, EditFailed, FailureEvent, GatedOutcome, PendingEdit, PixelReadback,
	Policy, ReadbackVerdict, RenderDone, RenderDoneEvent,
};
use oak_ai_tools::mock::{MockBackend, footage_rgb};
use oak_ai_tools::render::Frame;
use base64::Engine as _;
use serde_json::{Value, json};

/// Records every trigger, optionally checks the pixels itself, and then
/// delegates the verdict to the built-in [`PixelReadback`].
struct Probe {
	pixel: PixelReadback,
	seen: Vec<(String, usize)>,
	failures: Vec<String>,
	expected_rgb: Option<[f32; 4]>,
}

impl Probe {
	fn new(pixel: PixelReadback) -> Self {
		Self {
			pixel,
			seen: Vec::new(),
			failures: Vec::new(),
			expected_rgb: None,
		}
	}

	fn off() -> Self {
		Self {
			pixel: PixelReadback::structural(),
			seen: Vec::new(),
			failures: Vec::new(),
			expected_rgb: None,
		}
	}
}

impl BeforeCommit for Probe {
	fn before_commit(&mut self, _pending: &PendingEdit<'_>) -> Decision {
		Decision::Approve
	}
}

impl RenderDone for Probe {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		self.seen.push((event.tool.to_string(), event.frames.len()));
		if let Some(expected) = self.expected_rgb {
			for frame in &event.frames {
				let encoded = frame
					.get("png_base64")
					.and_then(Value::as_str)
					.expect("the event carries the PNG payload");
				let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).expect("base64");
				let decoded = Frame::from_png(&bytes).expect("PNG");
				let centre = decoded.pixel(decoded.width / 2, decoded.height / 2);
				for channel in 0..3 {
					if (centre[channel] - expected[channel]).abs() > 1.0 / 255.0 {
						return ReadbackVerdict::Fail {
							reason: format!(
								"centre pixel channel {channel} = {}, want {}",
								centre[channel], expected[channel]
							),
						};
					}
				}
			}
		}
		self.pixel.render_done(event)
	}
}

impl EditFailed for Probe {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> hooks::Feedback {
		self.failures.push(event.error.code.clone());
		hooks::Feedback::from_error(event.error.clone())
	}
}

/// A fresh fixture per call: the tests run in parallel in one process.
fn temp_media(tag: &str) -> std::path::PathBuf {
	use std::sync::atomic::{AtomicUsize, Ordering};
	static NEXT: AtomicUsize = AtomicUsize::new(0);
	let path = std::env::temp_dir().join(format!(
		"oak_hooks_rb_{tag}_{}_{}.mp4",
		std::process::id(),
		NEXT.fetch_add(1, Ordering::Relaxed)
	));
	std::fs::write(&path, b"oak hooks readback media fixture").expect("write fixture");
	path
}

/// A project with a single clip covering the whole 10 s fixture at t=0.
/// Returns `(clip id, media path)`.
fn seed(backend: &mut MockBackend) -> (String, String) {
	execute(backend, "project.open", json!({"name": "readback", "fps": 30})).expect("open");
	let media = temp_media("clip");
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
	(out["clip"].as_str().unwrap().to_string(), media.display().to_string())
}

#[test]
fn render_done_fires_after_the_frame_returns_and_sees_the_pixels() {
	let mut backend = MockBackend::new();
	let (_, media) = seed(&mut backend);
	let depth = backend.undo_depth();

	let mut probe = Probe::new(PixelReadback::opaque(0.9));
	probe.expected_rgb = Some(footage_rgb(&media));
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 7, "den": 1},
			"format": "png",
			"max_size": {"width": 32, "height": 18},
		}),
		Policy::Confirm,
		&mut probe,
	);
	assert!(outcome.is_applied(), "{outcome:?}");
	assert_eq!(probe.seen, vec![("render.get_frame".to_string(), 1)]);
	assert!(probe.failures.is_empty());
	assert_eq!(backend.undo_depth(), depth + 1, "a read still commits its group");
}

#[test]
fn render_done_fires_once_per_thumbnail_frame() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let depth = backend.undo_depth();
	let mut probe = Probe::off();
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_thumbnails",
		json!({
			"sequence": "sequence-1",
			"range": {"in": {"num": 0, "den": 1}, "out": {"num": 10, "den": 1}},
			"count": 3,
			"height": 8,
		}),
		Policy::Confirm,
		&mut probe,
	);
	assert!(outcome.is_applied(), "{outcome:?}");
	assert_eq!(probe.seen.len(), 1, "one trigger per tool call");
	assert_eq!(probe.seen[0].1, 3, "all three frames are asserted");
	assert_eq!(backend.undo_depth(), depth + 1);
}

#[test]
fn a_failed_readback_aborts_the_group_and_feeds_the_failure_back() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let state = backend.serialize();
	let depth = backend.undo_depth();

	// t=100 s is outside every clip, so the mock renders a black frame;
	// the opaque() preset rejects it.
	let mut probe = Probe::new(PixelReadback::opaque(0.99));
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 100, "den": 1},
			"format": "png",
			"max_size": {"width": 16, "height": 16},
		}),
		Policy::Auto,
		&mut probe,
	);

	match &outcome {
		GatedOutcome::Failed { error, feedback } => {
			assert_eq!(error.code, hooks::CODE_READBACK_FAILED);
			assert_eq!(error.tool.as_deref(), Some("render.get_frame"));
			assert_eq!(error.step.as_deref(), Some("readback"));
			assert_eq!(error.txn.as_deref(), Some("aborted"));
			assert!(error.message.contains("non-black"), "{}", error.message);
			// The re-planning feedback reuses the error object.
			assert!(feedback.retryable, "the agent can fix the time and retry");
			assert!(feedback.hint.contains("readback"), "{}", feedback.hint);
			let wire = feedback.to_json();
			assert_eq!(wire["code"], "READBACK_ASSERTION_FAILED");
			assert_eq!(wire["step"], "readback");
			assert_eq!(wire["txn"], "aborted");
		}
		other => panic!("expected a readback failure, got {other:?}"),
	}
	assert_eq!(probe.seen, vec![("render.get_frame".to_string(), 1)]);
	assert_eq!(
		probe.failures,
		vec![hooks::CODE_READBACK_FAILED.to_string()],
		"edit_failed is called for a readback failure"
	);
	// The group was aborted, so the failed render leaves no history row.
	assert_eq!(backend.serialize(), state);
	assert_eq!(backend.undo_depth(), depth);

	// And the assertion is the only thing standing in the way: the same
	// frame is accepted by the structural preset.
	let mut lenient = Probe::off();
	let retry = dispatch::execute_with(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 100, "den": 1},
			"format": "png",
			"max_size": {"width": 16, "height": 16},
		}),
		Policy::Auto,
		&mut lenient,
	);
	assert!(retry.is_applied(), "the pixels are fine, the assertion was the gate");
	assert_eq!(backend.undo_depth(), depth + 1);
}

#[test]
fn the_bgra_path_is_structurally_checked() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let mut probe = Probe::off();
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 1, "den": 1},
			"format": "bgra8",
			"max_size": {"width": 4, "height": 3},
		}),
		Policy::Auto,
		&mut probe,
	);
	assert!(outcome.is_applied(), "{outcome:?}");
	assert_eq!(probe.seen, vec![("render.get_frame".to_string(), 1)]);
}

#[test]
fn audio_levels_do_not_trigger_a_pixel_readback() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let mut probe = Probe::off();
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_audio_levels",
		json!({
			"sequence": "sequence-1",
			"range": {"in": {"num": 0, "den": 1}, "out": {"num": 10, "den": 1}},
			"resolution": 8,
		}),
		Policy::Auto,
		&mut probe,
	);
	assert!(outcome.is_applied(), "{outcome:?}");
	assert!(
		probe.seen.is_empty(),
		"an envelope is not a frame; the pixel pipeline must not fire"
	);
}

#[test]
fn a_policy_rejection_before_commit_skips_the_readback() {
	// The gate and the readback are ordered: a rejected call never reaches
	// the assertion pipeline because the body never ran.
	let mut backend = MockBackend::new();
	seed(&mut backend);

	struct Rejecting(Probe);
	impl BeforeCommit for Rejecting {
		fn before_commit(&mut self, _pending: &PendingEdit<'_>) -> Decision {
			Decision::Reject {
				reason: "not now".to_string(),
			}
		}
	}
	impl RenderDone for Rejecting {
		fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
			self.0.render_done(event)
		}
	}
	impl EditFailed for Rejecting {
		fn edit_failed(&mut self, event: &FailureEvent<'_>) -> hooks::Feedback {
			self.0.edit_failed(event)
		}
	}

	// `render.get_frame` is a read, so the gate does not see it at all;
	// use an edit instead to prove the ordering on a rejected *edit*.
	let mut hook = Rejecting(Probe::off());
	let outcome = dispatch::execute_with(
		&mut backend,
		"timeline.add_track",
		json!({"type": "video"}),
		Policy::Confirm,
		&mut hook,
	);
	assert!(matches!(outcome, GatedOutcome::Rejected { .. }));
	assert!(hook.0.seen.is_empty());
}

/// A failure inside the body never reaches the readback either.
#[test]
fn a_failing_body_never_reaches_the_readback() {
	let mut backend = MockBackend::new();
	seed(&mut backend);
	let mut probe = Probe::off();
	let outcome = dispatch::execute_with(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-404",
			"time": {"num": 1, "den": 1},
		}),
		Policy::Auto,
		&mut probe,
	);
	match outcome {
		GatedOutcome::Failed { error, feedback } => {
			assert_eq!(error.code, oak_ai_tools::error::CODE_NOT_FOUND);
			assert_eq!(error.txn.as_deref(), Some("aborted"));
			assert!(feedback.retryable);
		}
		other => panic!("expected a failure, got {other:?}"),
	}
	assert!(probe.seen.is_empty(), "no frame came back, nothing to assert");
	assert_eq!(probe.failures.len(), 1);
}

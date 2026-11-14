//! T6 replay: drive the frozen 22-tool face with a recorded tool-call
//! sequence (NDJSON fixture) and prove the result is reproducible.
//!
//! Three things are asserted here, in one process, with **no key, no
//! network and no engine** (default features → `MockBackend`):
//!
//! 1. **Session replay log** — every call, its outcome, the transaction
//!    state, and the PNG sha256 of every frame it returned, written to a
//!    NDJSON log. Running the same fixture twice yields a byte-identical
//!    log, so CI can diff two runs instead of trusting a human.
//! 2. **Golden-frame invariants** — size matches the request, the PNG
//!    decodes, and the frame is not black (checked by the test *and* by
//!    the T4 `render_done` readback hook through `execute_with`, so the
//!    invariants are enforced inside the pipeline, not only around it).
//! 3. **Zero MCP drift** — the frame's sha256 is the same whether the
//!    call goes through `dispatch::execute` or through the real MCP
//!    server binary (`ai/oak-ai-mcp`, backend mock) over stdio.
//!
//! Determinism caveat (documented, not hidden): the PNG sha256 is stable
//! only while the fixture (media paths) and the crate versions stay the
//! same — `MockBackend` colours frames from the footage path hash. The
//! test therefore compares **run against run**, never against a golden
//! constant committed to the repo.

use base64::Engine as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use oak_ai_tools::dispatch::execute_with;
use oak_ai_tools::hooks::{
	self, BeforeCommit, Decision, EditFailed, FailureEvent, GatedOutcome, PendingEdit,
	PixelReadback, Policy, RenderDone, RenderDoneEvent,
};
use oak_ai_tools::mock::MockBackend;
use oak_ci_replay::{FrameLog, ReplayEvent, canonical_sha256, frame_values, png_sha256};

/// Hooks that record the golden-frame checks: T4's `PixelReadback`
/// (structural + non-black) decides, this wrapper only remembers the
/// outcome so the test can prove the hook actually ran.
struct ReadbackProbe {
	pixel: PixelReadback,
	checked: usize,
	failures: Vec<String>,
}

impl ReadbackProbe {
	fn new(min_non_black: f64) -> Self {
		Self {
			pixel: PixelReadback::opaque(min_non_black),
			checked: 0,
			failures: Vec::new(),
		}
	}
}

impl BeforeCommit for ReadbackProbe {
	fn before_commit(&mut self, _pending: &PendingEdit<'_>) -> Decision {
		Decision::Approve
	}
}

impl RenderDone for ReadbackProbe {
	fn render_done(&mut self, event: &RenderDoneEvent<'_>) -> hooks::ReadbackVerdict {
		self.checked += event.frames.len();
		let verdict = self.pixel.render_done(event);
		if let hooks::ReadbackVerdict::Fail { reason } = &verdict {
			self.failures.push(reason.clone());
		}
		verdict
	}
}

impl EditFailed for ReadbackProbe {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> hooks::Feedback {
		hooks::Feedback::from_error(event.error.clone())
	}
}

fn repo_root() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../..")
		.canonicalize()
		.expect("repo root")
}

fn fixture(name: &str) -> String {
	let path = repo_root().join("ai/ci/fixtures").join(name);
	std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

fn mcp_binary() -> PathBuf {
	repo_root().join("ai/oak-ai-mcp/target/debug/oak-ai-mcp")
}

fn temp_media(tag: &str) -> PathBuf {
	// The mock backend colours frames from the footage *path* hash, so the
	// name must be identical across processes (in-process run, MCP subprocess,
	// and a second `cargo test` invocation) — a pid suffix would change the
	// colour per process and break the cross-process log diff. Tests that
	// share a tag are serialised by `chain_lock()`.
	let path = std::env::temp_dir().join(format!("oak_ci_replay_{tag}.mp4"));
	// MockBackend colours frames from the path hash, so the bytes never
	// matter; they must exist because `media.import_footage` opens the file.
	std::fs::write(&path, b"oak ci replay media fixture").expect("write fixture");
	path
}

/// A never-before-used media path: needed by the black-frame ablation so
/// the final render has no clip covering it (a leftover file from a prior
/// run would otherwise make the frame non-black and the ablation pass).
fn fresh_media(tag: &str) -> PathBuf {
	use std::sync::atomic::{AtomicUsize, Ordering};
	static NEXT: AtomicUsize = AtomicUsize::new(0);
	let path = std::env::temp_dir().join(format!(
		"oak_ci_replay_{tag}_{}_{}.mp4",
		std::process::id(),
		NEXT.fetch_add(1, Ordering::Relaxed)
	));
	std::fs::write(&path, b"oak ci replay media fixture").expect("write fixture");
	path
}

/// Serialises tests that share one media path (the mock colours frames from
/// the path hash, so two concurrent runs on the same file would race and mix
/// colours). Returns the guard; dropping it releases the lock.
fn chain_lock() -> std::fs::File {
	use std::os::unix::io::AsRawFd;
	let path = std::env::temp_dir().join("oak_ci_replay_chain.lock");
	let file = std::fs::OpenOptions::new()
		.create(true)
		.write(true)
		.truncate(false)
		.open(&path)
		.expect("open chain lock");
	// SAFETY: plain flock(2) on a plain file descriptor (Linux/macOS).
	let rc = unsafe {
		flock(file.as_raw_fd(), 2 /* LOCK_EX */)
	};
	assert_eq!(rc, 0, "flock failed");
	file
}

unsafe extern "C" {
	fn flock(fd: i32, operation: i32) -> i32;
}

fn read_steps(fixture: &str) -> Vec<ReplayEvent> {
	fixture
		.lines()
		.map(|line| line.trim())
		.filter(|line| !line.is_empty() && !line.starts_with('#'))
		.map(|line| serde_json::from_str(line).expect("fixture line is valid JSON"))
		.collect()
}

/// One replay of the recorded fixture against a fresh `MockBackend`:
/// returns the session log, every golden frame, and how many frames the
/// T4 readback hook saw.
fn run_fixture(fixture: &str) -> (String, Vec<FrameLog>, usize) {
	let steps = read_steps(fixture);
	// The fixture's media path is a token; every run materialises the same
	// bytes at the same path so the footage-path colour (and therefore the
	// PNG sha256) is identical across runs and machines.
	let media = temp_media("chain");

	let mut backend = MockBackend::new();
	let mut probe = ReadbackProbe::new(0.5);
	let mut log = String::new();
	let mut frames = Vec::new();

	for step in steps {
		let args = serde_json::to_string(&step.args).expect("args");
		let args: serde_json::Value =
			serde_json::from_str(&args.replace("OAK_CI_MEDIA", &media.display().to_string()))
				.expect("args after substitution");
		let outcome = execute_with(
			&mut backend,
			&step.call,
			args.clone(),
			Policy::Auto,
			&mut probe,
		);

		let (result, error) = match outcome {
			GatedOutcome::Applied { result } => (Some(result), None),
			other => {
				let err = match other {
					GatedOutcome::Rejected { error, .. } | GatedOutcome::Failed { error, .. } => {
						error
					}
					_ => unreachable!("Applied handled above"),
				};
				(None, Some(err))
			}
		};

		// Golden-frame invariants + per-frame sha256 for the log.
		let mut this_call_frames = Vec::new();
		if let Some(result) = &result {
			if step.call.starts_with("render.") {
				for frame in frame_values(&step.call, result) {
					let width = frame["width"].as_u64().expect("width") as u32;
					let height = frame["height"].as_u64().expect("height") as u32;
					assert!(width > 0 && height > 0, "zero-sized frame: {frame}");
					let decoded = oak_ai_tools::render::Frame::from_png(
						&base64::engine::general_purpose::STANDARD.decode(
							frame["png_base64"].as_str().expect("png payload"),
						)
						.expect("base64"),
					)
					.expect("PNG decodes");
					let stats = decoded.stats();
					assert!(
						stats.non_black_ratio > 0.5,
						"{}: golden-frame invariant broken (non-black ratio {})",
						step.call,
						stats.non_black_ratio
					);
					this_call_frames.push(FrameLog {
						tool: step.call.clone(),
						sha256: png_sha256(frame).expect("png payload"),
						width,
						height,
						non_black_ratio: stats.non_black_ratio,
					});
				}
			}
		}
		frames.extend(this_call_frames.iter().cloned());

		// The transaction state the log must carry: applied calls sit on
		// the undo stack, failed transactional calls aborted their group.
		let txn = match (&result, &error) {
			(Some(_), _) if backend.can_undo() => "committed",
			(Some(_), _) => "read",
			(None, _) => "aborted",
		};

		let media_str = media.display().to_string();
		let scrub = |text: String| {
			text.replace(&media_str, "OAK_CI_MEDIA")
				.replace(&media_str.replace('\\', "\\\\"), "OAK_CI_MEDIA")
		};
		log.push_str(
			&serde_json::json!({
				"kind": "call",
				"call": step.call,
				"args": scrub(serde_json::to_string(&args).expect("args")),
				"ok": error.is_none(),
				"error": error.as_ref().map(|e| serde_json::json!({
					"code": e.code, "step": e.step, "txn": e.txn,
				})),
				"txn": txn,
				"undo_depth": backend.undo_depth(),
				"result_sha256": result.as_ref().map(canonical_sha256).map(scrub),
				"frames": this_call_frames,
			})
			.to_string(),
		);
		log.push('\n');
	}

	// The caller owns the media file: the MCP half needs the same path and
	// bytes after this run (see mcp_binary_agrees_with_in_process_replay).
	(log, frames, probe.checked)
}

#[test]
fn replay_is_reproducible_and_frames_are_golden() {
	let _lock = chain_lock();
	let fx = fixture("edit_chain.ndjson");
	let (log1, frames1, checked1) = run_fixture(&fx);
	let (log2, _frames2, checked2) = run_fixture(&fx);

	// 1. Same fixture, two runs → byte-identical session log.
	assert_eq!(log1, log2, "two replays of the same fixture differ");

	// 2. The chain really rendered frames, and the T4 readback hook saw
	//    every one of them (the invariants are enforced in-pipeline).
	assert!(!frames1.is_empty(), "fixture must produce frames");
	assert_eq!(
		checked1,
		frames1.len(),
		"render_done hook ran once per frame ({checked1} vs {})",
		frames1.len()
	);
	assert_eq!(checked1, checked2);

	// 3. Golden-frame invariants per frame (size + non-black).
	for f in &frames1 {
		assert!(f.width > 0 && f.height > 0, "zero-sized frame: {f:?}");
		assert!(
			f.non_black_ratio > 0.5,
			"black frame escaped the invariant: {f:?}"
		);
	}

	// 4. The log carries the T6 contract: tool_call + frame hashes +
	//    transaction state per call.
	assert!(log1.contains("\"sha256\""), "frame hashes missing from log");
	assert!(log1.contains("\"txn\":\"committed\""), "txn state missing");
	let calls = log1
		.lines()
		.filter(|l| l.contains("\"kind\":\"call\""))
		.count();
	assert!(
		calls >= 6,
		"expected the whole chain in the log, got {calls}"
	);

	// Leave the log on disk for humans when asked to.
	if let Ok(dir) = std::env::var("OAK_CI_LOG_DIR") {
		let path = Path::new(&dir).join("replay-a.ndjson");
		std::fs::create_dir_all(&dir).expect("log dir");
		std::fs::write(&path, &log1).expect("write log");
		eprintln!("session log written to {}", path.display());
	}
}

/// Zero-drift: the same recorded call, executed through the real MCP
/// server binary, returns the frame with the same sha256 the in-process
/// replay computed. Also proves the stdio framing end to end.
#[test]
fn mcp_binary_agrees_with_in_process_replay() {
	let _lock = chain_lock();
	let fx = fixture("edit_chain.ndjson");
	let media = temp_media("chain");
	// run_fixture substitutes OAK_CI_MEDIA itself; the same path (and the
	// canonical bytes temp_media just wrote) is then reused for the
	// subprocess so both halves colour frames identically.
	let (_log, frames, _checked) = run_fixture(&fx);
	let in_process_sha = frames
		.iter()
		.find(|f| f.tool == "render.get_frame")
		.expect("fixture renders a frame")
		.sha256
		.clone();

	let bin = mcp_binary();
	if !bin.exists() {
		// The workflow builds it first; locally a missing binary means the
		// caller skipped `build-mcp`. Skip loudly instead of failing: the
		// in-process half above already ran.
		eprintln!("SKIP: {:?} not built (run ai/ci/run.sh)", bin);
		return;
	}

	// Same media path as the in-process run above (same bytes → same colour).
	// Re-create it with the canonical bytes so a stale file from an earlier
	// run cannot make the subprocess import fail.
	let fx = fx.replace(
		"OAK_CI_MEDIA",
		&media.display().to_string().replace('\\', "\\\\"),
	);
	std::fs::write(&media, b"oak ci replay media fixture").expect("rewrite media");

	let mut frames_in = String::new();
	frames_in.push_str(
		"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"oak-ci\",\"version\":\"1\"}}}\n",
	);
	frames_in.push_str("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
	for (i, line) in fx
		.lines()
		.filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
		.enumerate()
	{
		let step: ReplayEvent = serde_json::from_str(line).expect("fixture line");
		frames_in.push_str(
			&serde_json::json!({
				"jsonrpc": "2.0",
				"id": (i + 2) as u64,
				"method": "tools/call",
				"params": { "name": step.call, "arguments": step.args },
			})
			.to_string(),
		);
		frames_in.push('\n');
	}

	let mut child = std::process::Command::new(&bin)
		.args(["--backend", "mock"])
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::null())
		.spawn()
		.expect("spawn oak-ai-mcp");
	{
		let mut stdin = child.stdin.take().expect("stdin");
		stdin.write_all(frames_in.as_bytes()).expect("write frames");
		// Dropping stdin closes the pipe so the server sees EOF and exits.
	}
	let out = child.wait_with_output().expect("wait for oak-ai-mcp");
	assert!(out.status.success(), "mcp server exited: {}", out.status);
	let _ = std::fs::remove_file(&media);

	let mut mcp_sha = None;
	for line in String::from_utf8_lossy(&out.stdout).lines() {
		let line = line.trim();
		if !line.starts_with('{') {
			continue;
		}
		let msg: serde_json::Value = serde_json::from_str(line).expect("frame is JSON");
		if msg.get("method").is_some() {
			continue; // requests / notifications, not replies
		}
		// The reply is keyed by the request id we assigned to the
		// render.get_frame step (5th tools/call → id 6).
		// Only the render.get_frame reply: match by id AND by the
		// tool-shaped result (this call is the 5th tools/call → id 6).
		if msg["id"] != 6 || msg["result"]["structuredContent"]["format"] != "png" {
			continue;
		}
		if let Some(sha) = png_sha256(&msg["result"]["structuredContent"]) {
			mcp_sha = Some(sha);
		}
	}
	let mcp_sha = mcp_sha.expect("MCP server returned a render.get_frame frame");
	assert_eq!(
		mcp_sha, in_process_sha,
		"MCP path and in-process path disagree on the frame sha256"
	);
}

/// Ablation harness: `OAK_CI_BAD_FIXTURE=1` replays a fixture whose final
/// frame is black. The golden-frame invariant must reject it — CI wires
/// this in as a deliberate red run (see ai/ci/README.md "消融").
#[test]
#[ignore = "ablation only: run with `cargo test -- --ignored`"]
fn ablation_black_frame_is_rejected() {
	let bad = std::env::var("OAK_CI_BAD_FIXTURE").unwrap_or_default();
	let name = if bad == "1" {
		"bad_black.ndjson"
	} else {
		"edit_chain.ndjson"
	};
	let fx = fixture(name);
	// run_fixture panics on a black frame — that is the expected failure.
	// The bad fixture carries no media token; a fresh path keeps this run
	// independent of leftovers from other runs.
	let _fresh = fresh_media("ablation");
	let (log, frames, _checked) = run_fixture(&fx);
	// Reaching here means the invariant did NOT fire. That is the expected
	// outcome for the good fixture, and the ablation's failure for the bad
	// one (run_fixture panics on a black frame before this point).
	assert!(
		bad != "1",
		"ablation did not trip: the black frame passed the golden-frame invariant 		 (log={} lines, {} frames)",
		log.lines().count(),
		frames.len()
	);
}

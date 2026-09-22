// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Boundary tests for the codec task submitter bridge: register/unregister
//! state handling and the synchronous `submit_codec_task` dispatch for
//! conform (real clip success, missing media, bad output name, failed
//! rename) and proxy (missing source) requests.
//!
//! The bridge collapses inner task failures into one codec error per kind
//! (the interim synchronous contract), so the failure-path tests assert
//! that outer variant/message here and the distinguishing reason on the
//! task base through `conform_directly`/`proxy_directly`.
//!
//! The oakcodec submit registry is process-global, so the tests in this
//! file serialize on one mutex.

use std::sync::{Mutex, MutexGuard};

use oak_codec::task::{submit_task, TaskKind, TaskRequest};
use oak_task::codecbridge::{
	is_codec_task_submitter_registered, register_codec_task_submitter,
	unregister_codec_task_submitter,
};
use oak_task::conform::ConformTask;
use oak_task::error::Error as TaskError;
use oak_task::proxy::{ProxyParams, ProxyTask};
use oak_task::task::{Task, TaskBehavior};

/// Tests that mutate oakcodec's global submit callback must not race.
static REG_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
	REG_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run a conform request directly through [`ConformTask`], returning
/// `(result, task error message)`.
///
/// The bridge collapses every inner failure into one codec error (the
/// interim synchronous contract's "conform task failed"), so the
/// distinguishing reason is asserted here, on the task base the behavior
/// writes — a bad output name must fail the name contract, not the probe.
fn conform_directly(input: &str, output: &str) -> (oak_task::error::Result<()>, Option<String>) {
	let req = request(TaskKind::Conform, input, output);
	let mut inner = ConformTask::new(&req);
	let mut driver = Task::new("conform", None);
	let result = inner.run(&mut driver);
	(result, driver.error().map(str::to_string))
}

/// Run a proxy request directly through [`ProxyTask`] (missing sources fail
/// in the ffmpeg launch/run), returning the task error message.
fn proxy_directly(input: &str, output: &str) -> Option<String> {
	let req = request(TaskKind::Proxy, input, output);
	let params = ProxyParams {
		width: 320,
		height: 180,
		divider: 1,
		version: 1,
		crf: 23,
		include_audio: false,
		extension: "mp4".to_string(),
		preset: "veryfast".to_string(),
	};
	let mut inner = ProxyTask::new(&req, params);
	let mut driver = Task::new("proxy", None);
	let _ = inner.run(&mut driver);
	driver.error().map(str::to_string)
}

/// Assert the bridge rejected a submission with the expected `Failed`
/// message (the codec error variant is always `Failed`; the per-path
/// reason is asserted through [`conform_directly`]/[`proxy_directly`]).
fn assert_bridge_failed(context: &str, result: oak_codec::error::Result<bool>) -> String {
	match result {
		Err(oak_codec::error::Error::Failed(message)) => message,
		other => panic!("{context}: expected a Failed submission error, got {other:?}"),
	}
}

fn work_dir(tag: &str) -> std::path::PathBuf {
	let path = std::env::temp_dir().join(format!("oaktask_bridge_{tag}_{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&path);
	std::fs::create_dir_all(&path).expect("work dir");
	path
}

fn write_clip(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
	let path = dir.join(name);
	oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("test clip generation");
	path
}

fn request<'a>(kind: TaskKind, input: &'a str, output: &'a str) -> TaskRequest<'a> {
	TaskRequest {
		kind,
		input_filename: input,
		output_filename: output,
		stream_index: 1,
		sample_rate: 48000,
		channel_layout: 0x3,
		sample_format: 4,
		proxy_width: 0,
		proxy_height: 0,
	}
}

// ---- registration -----------------------------------------------------

/// Given no callback, `submit_task` reports "nothing submitted"; the bridge
/// registers exactly once and unregisters idempotently.
#[test]
fn submitter_registration_lifecycle() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	assert!(!is_codec_task_submitter_registered());

	let req = request(TaskKind::Conform, "in.mp4", "out.0.pcm");
	assert!(
		!submit_task(&req).expect("no callback is not an error"),
		"nothing is submitted without a callback"
	);

	register_codec_task_submitter().expect("first registration");
	assert!(is_codec_task_submitter_registered());
	assert!(matches!(
		register_codec_task_submitter(),
		Err(TaskError::State)
	));

	unregister_codec_task_submitter().expect("unregister");
	assert!(!is_codec_task_submitter_registered());
	unregister_codec_task_submitter().expect("idempotent unregister");
	assert!(!submit_task(&req).expect("no callback"));
}

// ---- conform ----------------------------------------------------------

/// A real test clip conforms into per-channel PCM files through the bridge.
#[test]
fn conform_submission_writes_pcm_files() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	register_codec_task_submitter().expect("register");

	let dir = work_dir("conform_ok");
	let clip = write_clip(&dir, "clip.mp4");
	let out = dir.join("audio.0.pcm");
	let clip_str = clip.to_string_lossy();
	let out_str = out.to_string_lossy();
	let req = request(TaskKind::Conform, &clip_str, &out_str);
	let submitted = submit_task(&req);
	assert!(submitted.is_ok(), "conform must be accepted: {submitted:?}");

	// The final per-channel files exist (renamed from the working names).
	for channel in 0..2 {
		let final_name = dir.join(format!("audio.{channel}.pcm"));
		let meta = std::fs::metadata(&final_name)
			.unwrap_or_else(|e| panic!("{} missing: {e}", final_name.display()));
		assert!(meta.len() > 0, "conform output is empty");
		assert!(
			!dir.join(format!("audio.{channel}.pcm.working")).exists(),
			"working files are renamed away"
		);
	}

	unregister_codec_task_submitter().expect("unregister");
	let _ = std::fs::remove_dir_all(&dir);
}

/// A missing source file is rejected (no decoder can probe it); the
/// path-specific reason is distinct from a bad-output-name failure.
#[test]
fn conform_submission_rejects_missing_media() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	register_codec_task_submitter().expect("register");

	let dir = work_dir("conform_missing");
	let out = dir.join("audio.0.pcm");
	let out_str = out.to_string_lossy();
	let req = request(TaskKind::Conform, "/definitely/not/here.mp4", &out_str);
	assert_eq!(
		assert_bridge_failed("missing media", submit_task(&req)),
		"conform task failed"
	);

	// The reason is the probe: no registered decoder accepts the input.
	let (result, error) = conform_directly("/definitely/not/here.mp4", &out_str);
	assert!(result.is_err());
	assert_eq!(error.as_deref(), Some("Failed to create decoder"));

	unregister_codec_task_submitter().expect("unregister");
	let _ = std::fs::remove_dir_all(&dir);
}

/// An output name that does not follow the `.0.pcm` contract is rejected
/// before any media work: the failure is the name contract, not a probe
/// failure.
#[test]
fn conform_submission_rejects_a_bad_output_name() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	register_codec_task_submitter().expect("register");

	let dir = work_dir("conform_bad_name");
	let clip = write_clip(&dir, "clip.mp4");
	let out = dir.join("audio.pcm");
	let clip_str = clip.to_string_lossy();
	let out_str = out.to_string_lossy();
	let req = request(TaskKind::Conform, &clip_str, &out_str);
	assert_eq!(
		assert_bridge_failed("bad output name", submit_task(&req)),
		"conform task failed"
	);

	// Directly: the rejection names the filename contract, so a probe
	// regression could not masquerade as this failure path.
	let (result, error) = conform_directly(&clip_str, &out_str);
	assert!(result.is_err());
	assert_eq!(error.as_deref(), Some("Invalid conform output filename"));

	unregister_codec_task_submitter().expect("unregister");
	let _ = std::fs::remove_dir_all(&dir);
}

/// A rename failure while moving the working files into place fails the
/// task after the audio was conformed — a third, distinct reason.
#[test]
fn conform_submission_reports_a_failed_rename() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	register_codec_task_submitter().expect("register");

	let dir = work_dir("conform_rename");
	let clip = write_clip(&dir, "clip.mp4");
	// The final first-channel path is an existing directory: the file
	// rename onto it must fail.
	std::fs::create_dir(dir.join("audio.0.pcm")).expect("blocking directory");
	let out = dir.join("audio.0.pcm");
	let clip_str = clip.to_string_lossy();
	let out_str = out.to_string_lossy();
	let req = request(TaskKind::Conform, &clip_str, &out_str);
	assert_eq!(
		assert_bridge_failed("failed rename", submit_task(&req)),
		"conform task failed"
	);

	// Directly: the conform succeeded and only the rename failed.
	let (result, error) = conform_directly(&clip_str, &out_str);
	assert!(result.is_err());
	assert_eq!(
		error.as_deref(),
		Some("Failed to move conformed audio into place")
	);

	unregister_codec_task_submitter().expect("unregister");
	let _ = std::fs::remove_dir_all(&dir);
}

// ---- proxy ------------------------------------------------------------

/// A proxy request with a missing source is rejected: the proxy params
/// conversion runs, then the transcode (or the ffmpeg lookup) fails with
/// an ffmpeg-side reason.
#[test]
fn proxy_submission_rejects_a_missing_source() {
	let _guard = serial();
	unregister_codec_task_submitter().expect("clean registry");
	register_codec_task_submitter().expect("register");

	let dir = work_dir("proxy_missing");
	let out = dir.join("proxy.mp4");
	let out_str = out.to_string_lossy();
	let req = request(TaskKind::Proxy, "/definitely/not/here.mp4", &out_str);
	assert_eq!(
		assert_bridge_failed("missing source", submit_task(&req)),
		"proxy task failed"
	);

	// Directly: the reason names the ffmpeg failure (missing binary or a
	// non-zero transcode), never a conform-side message.
	let error = proxy_directly("/definitely/not/here.mp4", &out_str);
	assert!(
		error
			.as_deref()
			.is_some_and(|message| message.contains("ffmpeg")),
		"the proxy failure reason is ffmpeg-side: {error:?}"
	);

	unregister_codec_task_submitter().expect("unregister");
	let _ = std::fs::remove_dir_all(&dir);
}

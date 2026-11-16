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

//! End-to-end tests for the built `oak-cli` binary (the C++ ctest suite
//! `oak_cli_info`/`oak_cli_render`/`oak_cli_probe`/`oak_cli_transcode`
//! equivalents).
//!
//! The binary links the oak* module crates directly (M14 R2; see
//! src/engine.rs) — no `liboakengine` dylib is needed at build or run
//! time, so `cargo test -p oak-cli` stands alone.
//!
//! The data-producing paths run against the repo fixtures
//! (`tests/project_with_footage.ove`, `tests/demo.mp4` — real H.264/AAC
//! media, `tests/img.png`), the failure paths assert the documented exit
//! codes (0 success, 1 general error, 2 rendering unavailable, 64 usage
//! error), and the argument-validation paths assert the C++ messages.
//!
//! The module probe records the decoder id but drops the codec's stream
//! descriptions (oaknode module gap), so the probe output carries real
//! stream counts of 0 and a 0 duration until the module fills them in —
//! the assertions pin the real contract.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
	env!("CARGO_BIN_EXE_oak-cli")
}

/// The fixture `.ove` file (relative to the workspace root).
fn fixture_project() -> PathBuf {
	// The fixtures live with the app crate (moved there in the workspace
	// restructure); oak-cli sits one level below oak-app.
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../oak-app/tests")
		.join("project_with_footage.ove")
}

/// The real media fixture.
fn fixture_media() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../oak-app/tests")
		.join("demo.mp4")
}

/// A single still-image fixture (1920x1080 RGBA PNG): one video stream,
/// no audio — fast end-to-end transcode coverage.
fn fixture_image() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../oak-app/tests")
		.join("img.png")
}

fn run(args: &[&str]) -> (i32, String, String) {
	let out = Command::new(bin())
		.args(args)
		.output()
		.expect("spawn oak-cli");
	(
		out.status.code().expect("exit code"),
		String::from_utf8_lossy(&out.stdout).into_owned(),
		String::from_utf8_lossy(&out.stderr).into_owned(),
	)
}

/// stderr, kept for the historical dyld objc notices (the module-linked
/// binary no longer embeds FFmpeg's libavdevice, so it is a pass-through).
fn real_errors(stderr: &str) -> String {
	stderr
		.lines()
		.filter(|l| !l.starts_with("objc["))
		.collect::<Vec<_>>()
		.join("\n")
}

/// A scratch directory removed when the guard drops.
struct TempDir(PathBuf);

impl TempDir {
	fn new(tag: &str) -> Self {
		let dir = env::temp_dir().join(format!("oak_cli_test_{tag}_{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		TempDir(dir)
	}
}

impl Drop for TempDir {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.0);
	}
}

#[test]
fn help_prints_the_cpp_usage_text_and_exits_zero() {
	let (code, stdout, stderr) = run(&["--help"]);
	assert_eq!(code, 0);
	// stderr may carry dyld's objc class-duplication notices only.
	assert!(!real_errors(&stderr).contains("error:"), "stderr: {stderr}");
	assert!(stdout
		.starts_with("oak-cli - headless consumer of the oak editor modules (direct Rust ABI)\n"));
	assert!(stdout.contains("oak-cli transcode <input_media> <out> [width] [--format ppm|mp4]"));
	assert!(stdout.contains("Exit codes:"));
	assert!(stdout.contains("64  usage error"));
}

#[test]
fn no_arguments_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&[]);
	assert_eq!(code, 64);
	assert!(stderr.contains("Usage:"));
}

#[test]
fn unknown_command_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["frobnicate"]);
	assert_eq!(code, 64);
	assert!(stderr.contains("error: unknown command \"frobnicate\""));
}

#[test]
fn info_with_missing_argument_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["info"]);
	assert_eq!(code, 64);
	assert!(stderr.contains("Usage:"));
}

#[test]
fn info_on_a_missing_project_is_an_error() {
	let (code, _stdout, stderr) = run(&["info", "no-such-project.ove"]);
	assert_eq!(code, 1);
	assert!(
		real_errors(&stderr).contains("error: info:"),
		"stderr: {stderr}"
	);
}

#[test]
fn info_on_the_fixture_prints_the_project() {
	let project = fixture_project();
	let (code, stdout, stderr) = run(&["info", project.to_str().unwrap()]);
	assert_eq!(code, 0, "stderr: {stderr}");
	assert!(stdout.contains("Project: project_with_footage"), "{stdout}");
	assert!(stdout.contains("Modified: no"), "{stdout}");
	assert!(stdout.contains("Sequences: 1"), "{stdout}");
	assert!(stdout.contains("[0] \"Fixture Sequence\""), "{stdout}");
	assert!(
		stdout.contains("frame rate: 30000/1001 (29.970 fps)"),
		"{stdout}"
	);
	assert!(stdout.contains("Footage: 1"), "{stdout}");
	// The C++ fixture stores the footage path relative to the project;
	// the CLI resolves it against the project directory and reports it
	// online.
	assert!(stdout.contains("demo.mp4"), "{stdout}");
	assert!(stdout.contains("online"), "{stdout}");
}

#[test]
fn probe_on_a_missing_file_is_an_error() {
	let (code, stdout, stderr) = run(&["probe", "no-such-file.mp4"]);
	assert_eq!(code, 1);
	assert!(stdout.is_empty());
	assert!(
		real_errors(&stderr).contains("error: probe: file does not exist: no-such-file.mp4"),
		"stderr: {stderr}"
	);
}

#[test]
fn probe_on_the_media_fixture_prints_streams() {
	let media = fixture_media();
	let (code, stdout, stderr) = run(&["probe", media.to_str().unwrap()]);
	assert_eq!(code, 0, "stderr: {stderr}");
	// tests/demo.mp4 through the engine: the probe records the ffmpeg
	// decoder and the full stream inventory (1920x1080 @ 25 fps video,
	// 48 kHz stereo audio, 17 s each).
	assert!(stdout.contains("Decoder: ffmpeg"), "{stdout}");
	assert!(stdout.contains("Duration: 17.000000 s"), "{stdout}");
	assert!(stdout.contains("Video streams: 1"), "{stdout}");
	assert!(stdout.contains("1920x1080"), "{stdout}");
	assert!(stdout.contains("25/1 fps"), "{stdout}");
	assert!(stdout.contains("Audio streams: 1"), "{stdout}");
	assert!(stdout.contains("48000 Hz, 2 channels"), "{stdout}");
	assert!(stdout.contains("Subtitle streams: 0"), "{stdout}");
}

#[test]
fn render_bad_seconds_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["render", "p.ove", "abc", "1", "out"]);
	assert_eq!(code, 64);
	assert!(
		real_errors(&stderr).contains("error: invalid start seconds \"abc\""),
		"stderr: {stderr}"
	);
}

#[test]
fn render_end_not_after_start_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["render", "p.ove", "2", "1", "out"]);
	assert_eq!(code, 64);
	assert!(
		real_errors(&stderr).contains("error: invalid end seconds \"1\""),
		"stderr: {stderr}"
	);
}

#[test]
fn render_on_a_missing_project_is_an_error() {
	let (code, _stdout, stderr) = run(&["render", "no-such.ove", "0", "1", "out"]);
	assert_eq!(code, 1);
	assert!(
		real_errors(&stderr).contains("error: render:"),
		"stderr: {stderr}"
	);
}

#[test]
fn render_the_fixture_writes_ppm_frames_and_a_wav() {
	let dir = TempDir::new("render");
	let project = fixture_project();
	let out = dir.0.to_str().unwrap();
	let (code, _stdout, stderr) = run(&["render", project.to_str().unwrap(), "0", "0.1", out]);
	assert_eq!(code, 0, "stderr: {stderr}");
	// The engine reports the sequence rate as 30/1: frames at 0, 1/30
	// and 2/30 before 0.1 s.
	assert!(dir.0.join("frame_00000.ppm").is_file());
	assert!(dir.0.join("frame_00001.ppm").is_file());
	assert!(dir.0.join("frame_00002.ppm").is_file());
	assert!(dir.0.join("audio.wav").is_file());
	// PPM header of the first frame (P6).
	let first = fs::read(dir.0.join("frame_00000.ppm")).unwrap();
	assert!(first.starts_with(b"P6\n"), "PPM header");
}

#[test]
fn transcode_bad_width_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["transcode", "in.mp4", "out.mp4", "banana"]);
	assert_eq!(code, 64);
	assert!(
		real_errors(&stderr).contains("error: invalid width \"banana\""),
		"stderr: {stderr}"
	);
}

#[test]
fn transcode_nonpositive_width_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["transcode", "in.mp4", "out.mp4", "0"]);
	assert_eq!(code, 64);
	assert!(
		real_errors(&stderr).contains("error: invalid width \"0\""),
		"stderr: {stderr}"
	);
}

#[test]
fn transcode_unknown_format_is_a_usage_error() {
	let (code, _stdout, stderr) = run(&["transcode", "in.mp4", "out.mp4", "--format", "webm"]);
	assert_eq!(code, 64);
	assert!(
		real_errors(&stderr).contains("error: unknown --format \"webm\" (ppm|mp4)"),
		"stderr: {stderr}"
	);
}

#[test]
fn transcode_on_a_missing_input_is_an_error() {
	let (code, _stdout, stderr) = run(&["transcode", "no-such.mp4", "out.mp4"]);
	assert_eq!(code, 1);
	assert!(
		real_errors(&stderr).contains("error: transcode: file does not exist: no-such.mp4"),
		"stderr: {stderr}"
	);
}

#[test]
fn transcode_the_image_fixture_to_ppm_frames() {
	let dir = TempDir::new("transcode_ppm");
	let image = fixture_image();
	let out = dir.0.to_str().unwrap();
	let (code, _stdout, stderr) = run(&[
		"transcode",
		image.to_str().unwrap(),
		out,
		"160",
		"--format",
		"ppm",
	]);
	assert_eq!(code, 0, "stderr: {stderr}");
	// A still image counts as one frame.
	assert!(dir.0.join("frame_00000.ppm").is_file());
	assert!(!dir.0.join("frame_00001.ppm").exists());
	let first = fs::read(dir.0.join("frame_00000.ppm")).unwrap();
	assert!(first.starts_with(b"P6\n160 90\n255\n"), "160x90 P6 header");
	// No audio stream in the fixture: no WAV.
	assert!(!dir.0.join("audio.wav").exists());
}

#[test]
fn transcode_mp4_writes_a_real_mp4() {
	// The module export task (crate::engine::export_sequence) drives the
	// mp4 path end to end and writes a real file.
	let dir = TempDir::new("transcode_mp4");
	let image = fixture_image();
	let out = dir.0.join("out.mp4");
	let (code, _stdout, stderr) = run(&[
		"transcode",
		image.to_str().unwrap(),
		out.to_str().unwrap(),
		"160",
	]);
	assert_eq!(code, 0, "stderr: {stderr}");
	let head = fs::read(&out).expect("mp4 written");
	assert_eq!(&head[4..8], b"ftyp", "mp4 container");
}

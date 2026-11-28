//! Minimal `ffmpeg` process wrapper: one invocation per call, every
//! argument passed individually through [`std::process::Command::arg`]
//! (never through a shell — the tool face runs on paths a model
//! supplied, so a string command line would be a command-injection hole).
//!
//! The five analysis filters are parsed from the raw `stderr`/`stdout`
//! ffmpeg prints; this module only *runs* and *classifies*, the parsers
//! live in [`crate::tools::analysis`].

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::json;

use crate::error::ToolError;

/// The gap id reported when `ffmpeg` is absent from `PATH`.
pub const GAP_FFMPEG_MISSING: &str = "GAP_ANALYSIS_FFMPEG";

/// True when an `ffmpeg -version` probe succeeds on this machine.
pub fn ffmpeg_available() -> bool {
	Command::new("ffmpeg")
		.arg("-version")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.map(|status| status.success())
		.unwrap_or(false)
}

/// The error every analysis tool returns when ffmpeg cannot be spawned:
/// a *structured* capability gap, never a fabricated success.
pub fn missing_ffmpeg() -> ToolError {
	ToolError::unsupported(
		GAP_FFMPEG_MISSING,
		"ffmpeg was not found on PATH; install ffmpeg or expose it to the process \
		 to use the analysis.* tools",
	)
}

/// Input-file gate: exists, is a regular file, is readable.
pub fn check_input(src: &str) -> Result<(), ToolError> {
	let path = Path::new(src);
	if !path.exists() {
		return Err(ToolError::not_found(format!("input file not found: {src}")));
	}
	if !path.is_file() {
		return Err(ToolError::invalid_params(format!(
			"input is not a regular file: {src}"
		)));
	}
	if !path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
		return Err(ToolError::invalid_params(format!(
			"input file is empty: {src}"
		)));
	}
	Ok(())
}

/// Captured result of one ffmpeg run: exit status, stdout, stderr.
pub struct Capture {
	/// Process exit code (`None` when killed by a signal).
	pub code: Option<i32>,
	/// Everything ffmpeg wrote to stdout.
	pub out: String,
	/// Everything ffmpeg wrote to stderr (the log channel the filters log to).
	pub err: String,
}

/// Runs `ffmpeg` with `args` (each passed as one argv entry, no shell) and
/// captures both streams.
///
/// A spawn failure is either the missing-binary gap (spawn error on
/// `ffmpeg`) or an IO error; a non-zero exit is *not* turned into an error
/// here — callers classify the code together with the captured text
/// (ffmpeg's exit codes are wide: 254 = no such input, 251 = output
/// open failed, 234 = no stream for the filtergraph).
pub fn run(args: &[&str]) -> Result<Capture, ToolError> {
	let mut cmd = Command::new("ffmpeg");
	// Never `-c`: argv is built entry by entry, paths with spaces or
	// shell metacharacters stay inert data.
	for arg in args {
		cmd.arg(arg);
	}
	let output = cmd
		.stdin(Stdio::null())
		.output()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				missing_ffmpeg()
			} else {
				ToolError::new(
					crate::error::CODE_IO,
					format!("failed to spawn ffmpeg: {err}"),
				)
			}
		})?;
	Ok(Capture {
		code: output.status.code(),
		out: String::from_utf8_lossy(&output.stdout).into_owned(),
		err: String::from_utf8_lossy(&output.stderr).into_owned(),
	})
}

/// Runs ffmpeg and requires a zero exit; on failure the message carries the
/// code plus the last stderr lines so the model can act on it.
pub fn run_checked(args: &[&str], tool: &str) -> Result<Capture, ToolError> {
	let cap = run(args)?;
	if cap.code == Some(0) {
		return Ok(cap);
	}
	Err(ToolError::new(
		crate::error::CODE_BACKEND,
		format!(
			"ffmpeg failed with exit code {:?} for '{tool}': {}",
			cap.code,
			tail(&cap.err, 6)
		),
	))
	.map_err(|e| e.with_tool(tool))
}

/// Last `n` non-empty lines of `text`, joined — the part of ffmpeg's log
/// that names the actual failure.
pub fn tail(text: &str, n: usize) -> String {
	let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
	let start = lines.len().saturating_sub(n);
	lines[start..].join(" | ")
}

/// Structured "ffmpeg ran but produced nothing for this filter" — raised
/// when a filter that must emit markers printed none *and* the run cannot
/// be explained by "this input has no such stream" (checked by the caller
/// against the input's streams).
pub fn no_markers(tool: &str, detail: &str) -> ToolError {
	ToolError::new(
		crate::error::CODE_BACKEND,
		format!("'{tool}' found no markers: {detail}"),
	)
	.with_tool(tool)
}

/// Input stream probe via `ffprobe` — used to give an honest error when a
/// filter found nothing because the stream type is absent (a silent clip
/// really has no silence; a video with no audio track has no loudness).
pub fn probe_has_audio(src: &str) -> Result<bool, ToolError> {
	probe_stream(src, "a")
}

/// Same probe for a video stream.
pub fn probe_has_video(src: &str) -> Result<bool, ToolError> {
	probe_stream(src, "v")
}

fn probe_stream(src: &str, kind: &str) -> Result<bool, ToolError> {
	let output = Command::new("ffprobe")
		.args([
			"-v",
			"error",
			"-select_streams",
			kind,
			"-show_entries",
			"stream=index",
			"-of",
			"csv=p=0",
			src,
		])
		.stdin(Stdio::null())
		.output()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				missing_ffmpeg()
			} else {
				ToolError::new(
					crate::error::CODE_IO,
					format!("failed to spawn ffprobe: {err}"),
				)
			}
		})?;
	let code = output.status.code();
	if code != Some(0) {
		// ffprobe exits 1 on a missing file; the input gate already ran,
		// so this is an unreadable/corrupt input.
		return Err(ToolError::new(
			crate::error::CODE_IO,
			format!(
				"ffprobe failed for '{src}': {}",
				tail(&String::from_utf8_lossy(&output.stderr), 3)
			),
		));
	}
	Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

/// Small helper so failure payloads can name the tool uniformly.
pub fn error_json(err: &ToolError) -> serde_json::Value {
	json!({ "code": err.code, "message": err.message })
}

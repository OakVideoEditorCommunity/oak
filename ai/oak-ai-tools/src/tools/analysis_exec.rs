//! Executors for the five `analysis.*` v2 tools.
//!
//! Each tool shells out to `ffmpeg` exactly once (or twice when a frame
//! rate is needed to resolve frame-index time values), passing every
//! argument individually through [`std::process::Command::arg`] — never a
//! shell — and parses the filter's own log lines into a fixed JSON shape
//! documented in [`crate::schema_v2`].
//!
//! Failure discipline: missing ffmpeg → `UNSUPPORTED` + gap id; missing
//! input → `NOT_FOUND`; ffmpeg non-zero exit → `BACKEND_FAILED` carrying
//! its code and the tail of stderr. A filter that legitimately found
//! nothing on an input that *has* the stream returns an empty list, not an
//! error — and the "no such stream" case is detected up front so it can be
//! named instead of silently empty.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::error::ToolError;
use crate::ffmpeg;
use crate::timecode::Rat;
use crate::tools::analysis::{
	self, DetectBlack, DetectScenes, DetectSilence, ExportThumbnails, Loudness,
	intervals_json, points_json,
};

/// Frame rate of the source, when it has a video stream: `{num, den}` from
/// ffprobe (`r_frame_rate`), `None` for audio-only input.
fn source_rate(src: &str) -> Result<Option<Rat>, ToolError> {
	let output = std::process::Command::new("ffprobe")
		.args([
			"-v",
			"error",
			"-select_streams",
			"v:0",
			"-show_entries",
			"stream=r_frame_rate",
			"-of",
			"csv=p=0",
			src,
		])
		.stdin(std::process::Stdio::null())
		.output()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				ffmpeg::missing_ffmpeg()
			} else {
				ToolError::new(crate::error::CODE_IO, format!("failed to spawn ffprobe: {err}"))
			}
		})?;
	if output.status.code() != Some(0) {
		return Err(ToolError::new(
			crate::error::CODE_IO,
			format!(
				"ffprobe failed for '{src}': {}",
				ffmpeg::tail(&String::from_utf8_lossy(&output.stderr), 3)
			),
		));
	}
	let text = String::from_utf8_lossy(&output.stdout);
	let text = text.trim();
	if text.is_empty() || text.starts_with("N/A") {
		return Ok(None);
	}
	let (num, den) = text
		.split_once('/')
		.ok_or_else(|| ToolError::new(crate::error::CODE_BACKEND, format!("bad r_frame_rate '{text}'")))?;
	let num: i64 = num
		.trim()
		.parse()
		.map_err(|_| ToolError::new(crate::error::CODE_BACKEND, format!("bad r_frame_rate '{text}'")))?;
	let den: i64 = den
		.trim()
		.parse()
		.map_err(|_| ToolError::new(crate::error::CODE_BACKEND, format!("bad r_frame_rate '{text}'")))?;
	if num <= 0 || den <= 0 {
		return Ok(None);
	}
	Ok(Some(Rat::new(num, den)?))
}

/// A `silence_start: <sec>` / `silence_end: <sec>` pair from silencedetect's
/// stderr, seconds as f64.
///
/// A trailing silence (start with no end until the file ends) is left
/// open for the caller to close against the container duration.
fn parse_silence(stderr: &str) -> Vec<(f64, f64)> {
	let mut intervals = Vec::new();
	let mut open: Option<f64> = None;
	for line in stderr.lines() {
		// ffmpeg prefixes every filter line with `[filter @ 0x..] `; the
		// split below operates on the message after the `] ` so the
		// bracket never lands inside the value token.
		let line = line.trim();
		let msg = match line.find("] ") {
			Some(i) => &line[i + 2..],
			None => line,
		};
		let Some(rest) = msg.split("silence_start:").nth(1) else {
			if let Some(rest) = msg.split("silence_end:").nth(1) {
				let value = rest
					.trim()
					.split(|c: char| c == '|' || c.is_whitespace())
					.next()
					.and_then(|v| v.parse::<f64>().ok());
				if let (Some(start), Some(end)) = (open.take(), value) {
					intervals.push((start, end));
				}
			}
			continue;
		};
		let value = rest
			.trim()
			.split(|c: char| c == '|' || c.is_whitespace())
			.next()
			.and_then(|v| v.parse::<f64>().ok());
		if let Some(v) = value {
			open.get_or_insert(v);
		}
	}
	intervals
}

/// `analysis.detect_silence` executor.
pub fn detect_silence(parsed: &DetectSilence) -> Result<Value, ToolError> {
	ffmpeg::check_input(&parsed.src)?;
	if !ffmpeg::ffmpeg_available() {
		return Err(ffmpeg::missing_ffmpeg());
	}
	let noise = format!("{}dB", trim_float(parsed.noise_db));
	let min_dur = trim_float(parsed.min_dur);
	let cap = ffmpeg::run_checked(
		&[
			"-hide_banner",
			"-nostats",
			"-loglevel",
			"info",
			"-i",
			&parsed.src,
			"-af",
			&format!("silencedetect=noise={noise}:d={min_dur}"),
			"-f",
			"null",
			"-",
		],
		"analysis.detect_silence",
	)?;
	let mut intervals = parse_silence(&cap.err);
	// A silence that runs to the end of the file has no silence_end line.
	// Close it at the duration ffprobe reports.
	if let Some(open) = open_silence(&cap.err) {
		if let Some(dur) = container_duration(&parsed.src)? {
			if open < dur {
				intervals.push((open, dur));
			}
		}
	}
	intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
	// No audio stream at all: name it instead of reporting "no silence".
	if intervals.is_empty() && !ffmpeg::probe_has_audio(&parsed.src)? {
		return Err(ToolError::new(
			crate::error::CODE_BACKEND,
			format!("input has no audio stream: {}", parsed.src),
		)
		.with_tool("analysis.detect_silence"));
	}
	Ok(intervals_json(&intervals))
}

/// The `silence_start` left open when ffmpeg printed no matching end.
fn open_silence(stderr: &str) -> Option<f64> {
	let mut open = None;
	for line in stderr.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("silence_start:") {
			if let Some(v) = rest
				.trim()
				.split_whitespace()
				.next()
				.and_then(|v| v.parse::<f64>().ok())
			{
				open = Some(v);
			}
		} else if line.starts_with("silence_end:") {
			open = None;
		}
	}
	open
}

/// Container duration in seconds via ffprobe (`format=duration`).
fn container_duration(src: &str) -> Result<Option<f64>, ToolError> {
	let output = std::process::Command::new("ffprobe")
		.args([
			"-v",
			"error",
			"-show_entries",
			"format=duration",
			"-of",
			"csv=p=0",
			src,
		])
		.stdin(std::process::Stdio::null())
		.output()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				ffmpeg::missing_ffmpeg()
			} else {
				ToolError::new(crate::error::CODE_IO, format!("ffprobe spawn: {err}"))
			}
		})?;
	if output.status.code() != Some(0) {
		return Ok(None);
	}
	let text = String::from_utf8_lossy(&output.stdout);
	let text = text.trim();
	if text.is_empty() || text.starts_with("N/A") {
		return Ok(None);
	}
	text.parse::<f64>()
		.ok()
		.filter(|d| d.is_finite() && *d > 0.0)
		.map(Some)
		.map_or_else(|| Ok(None), Ok)
}

/// `analysis.detect_scenes` executor — parses `scdet` log lines:
/// `[scdet ...] lavfi.scd.score: 0.923, lavfi.scd.time: 0.04`.
pub fn detect_scenes(parsed: &DetectScenes) -> Result<Value, ToolError> {
	ffmpeg::check_input(&parsed.src)?;
	if !ffmpeg::ffmpeg_available() {
		return Err(ffmpeg::missing_ffmpeg());
	}
	let threshold = trim_float(parsed.threshold);
	let cap = ffmpeg::run_checked(
		&[
			"-hide_banner",
			"-nostats",
			"-loglevel",
			"info",
			"-i",
			&parsed.src,
			"-vf",
			&format!("scdet=threshold={threshold}"),
			"-f",
			"null",
			"-",
		],
		"analysis.detect_scenes",
	)?;
	let mut points: Vec<(f64, Option<f64>)> = Vec::new();
	for line in cap.err.lines() {
		let Some(pos) = line.find("lavfi.scd.time:") else {
			continue;
		};
		let head = &line[..pos];
		let tail = &line[pos + "lavfi.scd.time:".len()..];
		let time = tail
			.trim()
			.split(|c: char| c == ',' || c.is_whitespace())
			.next()
			.and_then(|v| v.parse::<f64>().ok());
		let score = head
			.rfind("lavfi.scd.score:")
			.map(|i| &head[i + "lavfi.scd.score:".len()..])
			.and_then(|v| v.trim().split(',').next())
			.and_then(|v| v.parse::<f64>().ok());
		if let Some(t) = time {
			points.push((t, score));
		}
	}
	if points.is_empty() && !ffmpeg::probe_has_video(&parsed.src)? {
		return Err(ToolError::new(
			crate::error::CODE_BACKEND,
			format!("input has no video stream: {}", parsed.src),
		)
		.with_tool("analysis.detect_scenes"));
	}
	points.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
	Ok(points_json(&points))
}

/// `analysis.detect_black` executor — parses `blackdetect` lines:
/// `black_start:0 black_end:0.96 black_duration:0.96`.
pub fn detect_black(parsed: &DetectBlack) -> Result<Value, ToolError> {
	ffmpeg::check_input(&parsed.src)?;
	if !ffmpeg::ffmpeg_available() {
		return Err(ffmpeg::missing_ffmpeg());
	}
	let min_dur = trim_float(parsed.min_dur);
	let pix_th = trim_float(parsed.pix_th);
	let cap = ffmpeg::run_checked(
		&[
			"-hide_banner",
			"-nostats",
			"-loglevel",
			"info",
			"-i",
			&parsed.src,
			"-vf",
			&format!("blackdetect=d={min_dur}:pix_th={pix_th}"),
			"-f",
			"null",
			"-",
		],
		"analysis.detect_black",
	)?;
	let mut intervals: Vec<(f64, f64)> = Vec::new();
	for line in cap.err.lines() {
		if !line.contains("black_start:") {
			continue;
		}
		let start = field_after(line, "black_start:").and_then(|v| v.parse::<f64>().ok());
		let end = field_after(line, "black_end:").and_then(|v| v.parse::<f64>().ok());
		if let (Some(s), Some(e)) = (start, end) {
			intervals.push((s, e));
		}
	}
	if intervals.is_empty() && !ffmpeg::probe_has_video(&parsed.src)? {
		return Err(ToolError::new(
			crate::error::CODE_BACKEND,
			format!("input has no video stream: {}", parsed.src),
		)
		.with_tool("analysis.detect_black"));
	}
	Ok(intervals_json(&intervals))
}

/// Value after `key` up to the next comma/space (handles `key:1.5` and
/// `key: 1.5` spellings; the key itself may appear anywhere in the line,
/// so a `foo_key:` suffix does not match).
fn field_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
	let pos = line.find(key)?;
	// Require the key to start at a token boundary: the previous char is
	// not alphanumeric (so `black_start:` does not match `xblack_start:`).
	if pos > 0 {
		let prev = line[..pos].chars().last().expect("pos > 0");
		if prev.is_alphanumeric() || prev == '_' {
			return None;
		}
	}
	let rest = &line[pos + key.len()..];
	let rest = rest.trim_start();
	let end = rest
		.find(|c: char| c == ',' || c.is_whitespace())
		.unwrap_or(rest.len());
	let token = &rest[..end];
	if token.is_empty() {
		None
	} else {
		Some(token)
	}
}

/// `analysis.loudness` executor — parses the ebur128 summary block:
/// `I: -21.8 LUFS`, `LRA: 0.0 LU`, `Peak: -18.1 dBFS`.
pub fn loudness(parsed: &Loudness) -> Result<Value, ToolError> {
	ffmpeg::check_input(&parsed.src)?;
	if !ffmpeg::ffmpeg_available() {
		return Err(ffmpeg::missing_ffmpeg());
	}
	// The `Summary:` block is emitted at `info` when the stream ends; it
	// never appears at `warning`, so this one stays `info`.
	let cap = ffmpeg::run_checked(
		&[
			"-hide_banner",
			"-nostats",
			"-loglevel",
			"info",
			"-i",
			&parsed.src,
			"-af",
			"ebur128=peak=true",
			"-f",
			"null",
			"-",
		],
		"analysis.loudness",
	)?;
	let int_lufs = summary_field(&cap.err, "Integrated loudness:", "I:")
		.and_then(|v| v.parse::<f64>().ok());
	let lra = summary_field(&cap.err, "Loudness range:", "LRA:")
		.and_then(|v| v.parse::<f64>().ok());
	let true_peak = summary_field(&cap.err, "True peak:", "Peak:")
		.and_then(|v| v.parse::<f64>().ok());
	let (Some(i), Some(l), Some(p)) = (int_lufs, lra, true_peak) else {
		if !ffmpeg::probe_has_audio(&parsed.src)? {
			return Err(ToolError::new(
				crate::error::CODE_BACKEND,
				format!("input has no audio stream: {}", parsed.src),
			)
			.with_tool("analysis.loudness"));
		}
		return Err(ffmpeg::no_markers(
			"analysis.loudness",
			&format!("ebur128 printed no summary: {}", ffmpeg::tail(&cap.err, 4)),
		));
	};
	Ok(json!({
		"integrated_lufs": i,
		"lra_lu": l,
		"true_peak_dbfs": p,
	}))
}

/// First `key` value after the `section` heading in the summary block.
fn summary_field(stderr: &str, section: &str, key: &str) -> Option<String> {
	let pos = stderr.find(section)?;
	let after = &stderr[pos + section.len()..];
	let line = after.lines().find(|l| l.contains(key))?;
	let value = field_after(line, key)?;
	Some(value.trim().to_string())
}

/// `analysis.export_thumbnails` executor: one ffmpeg run per output when
/// explicit times are given, one run with a `fps` filter for intervals.
pub fn export_thumbnails(parsed: &ExportThumbnails) -> Result<Value, ToolError> {
	ffmpeg::check_input(&parsed.src)?;
	if !ffmpeg::ffmpeg_available() {
		return Err(ffmpeg::missing_ffmpeg());
	}
	let outdir = Path::new(&parsed.outdir);
	if outdir.exists() && !outdir.is_dir() {
		return Err(ToolError::invalid_params(format!(
			"outdir exists and is not a directory: {}",
			parsed.outdir
		)));
	}
	fs::create_dir_all(outdir).map_err(|err| {
		ToolError::new(
			crate::error::CODE_IO,
			format!("cannot create outdir {}: {err}", parsed.outdir),
		)
	})?;

	let width = parsed.width.unwrap_or(320);
	let scale = format!("scale={width}:-2");

	let mut paths: Vec<String> = Vec::new();
	if let Some(times) = &parsed.times {
		let rate = source_rate(&parsed.src)?;
		let seconds = analysis::resolve_times(times, rate)?;
		// ffmpeg's image2 muxer refuses the same output name twice in one
		// run, so a single invocation writes a `%04d` sequence (one image
		// per second, fps=1/1) and the requested points are picked out of
		// it by their rounded second. `%04d` is what makes it a sequence
		// — `%04` alone is not, and image2 then refuses to write a second
		// frame under one name. Points beyond the file's duration are
		// simply absent — reported as such, never faked.
		let pattern = outdir.join("exp_%04d.jpg");
		let pattern_str = pattern.to_string_lossy().into_owned();
		ffmpeg::run_checked(
			&[
				"-hide_banner",
				"-nostats",
				"-loglevel",
				"error",
				"-y",
				"-i",
				&parsed.src,
				"-vf",
				&format!("fps=1,{scale}"),
				&pattern_str,
			],
			"analysis.export_thumbnails",
		)?;
		for (index, t) in seconds.iter().enumerate() {
			let file = outdir.join(format!("exp_{:04}.jpg", t.round() as u64));
			if file.is_file() {
				let dest = outdir.join(format!("thumb_{index:04}.jpg"));
				fs::rename(&file, &dest).map_err(|err| {
					ToolError::new(
						crate::error::CODE_IO,
						format!("cannot rename {} : {err}", file.display()),
					)
				})?;
				paths.push(dest.to_string_lossy().into_owned());
			}
		}
	} else {
		let interval = parsed.interval_sec.expect("validated");
		let step = trim_float(interval);
		// `%04d` = image2 sequence pattern (one file per interval tick).
		let file = outdir.join("thumb_%04d.jpg");
		let file_str = file.to_string_lossy().into_owned();
		ffmpeg::run_checked(
			&[
				"-hide_banner",
				"-nostats",
				"-loglevel",
				"error",
				"-y",
				"-i",
				&parsed.src,
				"-vf",
				&format!("fps=1/{step},{scale}"),
				&file_str,
			],
			"analysis.export_thumbnails",
		)?;
		// Glob the produced files in order.
		let mut found: Vec<String> = fs::read_dir(outdir)
			.map_err(|err| {
				ToolError::new(crate::error::CODE_IO, format!("read outdir: {err}"))
			})?
			.filter_map(|e| e.ok())
			.map(|e| e.path())
			.filter(|p| {
				p.file_name()
					.and_then(|n| n.to_str())
					.map(|n| n.starts_with("thumb_") && n.ends_with(".jpg"))
					.unwrap_or(false)
			})
			.filter(|p| {
				// Only the files this run wrote: the dir may already hold
				// exports from an earlier call.
				p.file_name()
					.and_then(|n| n.to_str())
					.map(|n| n.len() == "thumb_".len() + 4 + ".jpg".len())
					.unwrap_or(false)
			})
			.map(|p| p.to_string_lossy().into_owned())
			.collect();
		found.sort();
		paths = found;
	}

	// Every advertised file must really exist on disk (no fake success).
	for path in &paths {
		if !Path::new(path).is_file() {
			return Err(ToolError::new(
				crate::error::CODE_IO,
				format!("thumbnail was not written: {path}"),
			));
		}
	}
	if paths.is_empty() {
		return Err(ffmpeg::no_markers(
			"analysis.export_thumbnails",
			"ffmpeg produced no thumbnails for the requested interval",
		));
	}
	Ok(json!({ "paths": paths, "count": paths.len() }))
}

/// Formats a float the way filter arguments need it: `30` not `30.0`,
/// `-30` not `-30.0`, but `0.3` stays `0.3`.
fn trim_float(v: f64) -> String {
	if v == v.trunc() && v.abs() < 1e15 {
		format!("{}", v as i64)
	} else {
		format!("{v}")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn trim_float_spellings() {
		assert_eq!(trim_float(30.0), "30");
		assert_eq!(trim_float(-30.0), "-30");
		assert_eq!(trim_float(0.3), "0.3");
		assert_eq!(trim_float(0.5), "0.5");
	}

	#[test]
	fn silence_lines_parse() {
		let stderr = "[silencedetect @ 0x1] silence_start: 0\n\
		              [silencedetect @ 0x1] silence_end: 1.021701 | silence_duration: 1.021701\n";
		let intervals = parse_silence(stderr);
		assert_eq!(intervals.len(), 1);
		assert!((intervals[0].0 - 0.0).abs() < 1e-9);
		assert!((intervals[0].1 - 1.021701).abs() < 1e-6);
	}

	#[test]
	fn black_lines_parse_both_spellings() {
		let line = "[blackdetect @ 0x1] black_start:0 black_end:0.96 black_duration:0.96";
		let start = field_after(line, "black_start:").expect("start");
		assert_eq!(start, "0");
		let line2 = "[blackdetect @ 0x1] black_start: 1.5, black_end: 2.25";
		assert_eq!(field_after(line2, "black_start:"), Some("1.5"));
	}

	#[test]
	fn scdet_lines_parse() {
		let line = "[scdet @ 0x1] lavfi.scd.score: 0.923, lavfi.scd.time: 0.04";
		let time_pos = line.find("lavfi.scd.time:").expect("time");
		let tail = &line[time_pos + "lavfi.scd.time:".len()..];
		let time: f64 = tail
			.trim()
			.split(|c: char| c == ',' || c.is_whitespace())
			.next()
			.expect("token")
			.parse()
			.expect("float");
		assert!((time - 0.04).abs() < 1e-9);
	}
}

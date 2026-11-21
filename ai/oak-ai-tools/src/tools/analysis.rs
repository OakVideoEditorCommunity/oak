//! Typed argument parsing for the five `analysis.*` v2 tools.
//!
//! Parsing runs after JSON Schema validation (the schema is the contract),
//! exactly like the frozen 22 and the `sequence.*` v2 tools: a shape
//! problem becomes a structured [`ToolError`] with the three-part text
//! (what arrived / what is wrong / what to send instead).
//!
//! The five tools are pure reads over a media file on disk — no project
//! state, no undo — so every entry point returns a plain `Value` and the
//! dispatcher grades them `Read` (see `hooks::V2_TOOL_CLASSES`).

use serde_json::{Value, json};

use crate::error::ToolError;
use crate::timecode::Rat;

// ---------------------------------------------------------------------------
// Three-part shape errors (same discipline as tools/v2/sequence.rs)
// ---------------------------------------------------------------------------

/// What arrived, what is wrong, and the shape to send instead.
fn shape_error(args: &Value, detail: String, example: &str) -> ToolError {
	ToolError::invalid_params(format!("{detail}. Received: {args}. Send e.g. {example}"))
}

/// A required non-empty string (a file path).
fn req_path(args: &Value, key: &str) -> Result<String, ToolError> {
	match args.get(key) {
		Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
		Some(other) => Err(shape_error(
			args,
			format!("`{key}` must be a non-empty file path string, got {other}"),
			"{\"src\": \"/path/to/clip.mp4\"}",
		)),
		_ => Err(shape_error(
			args,
			format!("missing required field `{key}`"),
			"{\"src\": \"/path/to/clip.mp4\"}",
		)),
	}
}

/// An optional finite number with a range check.
fn opt_number(
	args: &Value,
	key: &str,
	default: f64,
	// (low, high] or open-ended when high is infinite
	low: f64,
	high: f64,
	example: &str,
) -> Result<f64, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(default),
		Some(v) => match v.as_f64() {
			Some(n) if n.is_finite() && n >= low && n <= high => Ok(n),
			_ => Err(shape_error(
				args,
				format!("`{key}` must be a finite number in ({low}, {high}], got {v}"),
				example,
			)),
		},
	}
}

// ---------------------------------------------------------------------------
// Timecode-style inputs: {num, den} rational seconds or integer frames
// ---------------------------------------------------------------------------

/// Seconds a time value denotes, resolved against the source's frame grid.
///
/// A bare float is rejected here the same way the rest of the tool face
/// rejects it; a frame index needs the grid, which `ffprobe` supplies via
/// [`crate::ffmpeg`]-independent probe in the executor.
fn time_seconds(raw: &Value, rate: Option<Rat>, key: &str) -> Result<f64, ToolError> {
	match raw {
		Value::Number(n) if n.as_u64().is_some() => {
			let frames = n.as_u64().expect("integer");
			let rate = rate.ok_or_else(|| {
				ToolError::timecode(format!(
					"field '{key}' is a frame index but the source has no frame rate"
				))
			})?;
			Ok(frames as f64 * rate.to_f64().recip())
		}
		Value::Object(map) => {
			let num = map
				.get("num")
				.and_then(Value::as_i64)
				.ok_or_else(|| {
					shape_error(
						&Value::Object(map.clone()),
						format!("`{key}` rational needs an integer `num`"),
						"{\"num\": 1, \"den\": 2}",
					)
				})?;
			let den = map.get("den").and_then(Value::as_i64).unwrap_or(1);
			let rat = Rat::new(num, den)?;
			Ok(rat.to_f64())
		}
		other => Err(ToolError::timecode(format!(
			"field '{key}' must be rational seconds {{num, den}} or a non-negative \
			 integer frame index; bare floating-point seconds are rejected, got {other}"
		))),
	}
}

// ---------------------------------------------------------------------------
// Parsed arguments
// ---------------------------------------------------------------------------

/// `analysis.detect_silence` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectSilence {
	pub src: String,
	/// dB threshold, default -30.
	pub noise_db: f64,
	/// Minimum silence length in seconds, default 0.5.
	pub min_dur: f64,
}

/// `analysis.detect_scenes` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectScenes {
	pub src: String,
	/// Scene-change threshold in [0, 1], default 0.3.
	pub threshold: f64,
}

/// `analysis.detect_black` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectBlack {
	pub src: String,
	/// Minimum black duration in seconds, default 0.3.
	pub min_dur: f64,
	/// Pixel luminance threshold in [0, 1], default 0.10.
	pub pix_th: f64,
}

/// `analysis.loudness` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Loudness {
	pub src: String,
}

/// `analysis.export_thumbnails` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportThumbnails {
	pub src: String,
	/// Seconds between thumbnails when `times` is absent.
	pub interval_sec: Option<f64>,
	/// Explicit time points (rational seconds or frame indices).
	pub times: Option<Vec<Value>>,
	pub outdir: String,
	/// Output width in pixels; height follows the source aspect ratio.
	pub width: Option<u64>,
}

fn example_detect() -> &'static str {
	"{\"src\": \"/media/clip.mp4\", \"noise_db\": -30, \"min_dur\": 0.5}"
}

/// `analysis.detect_silence`.
pub fn parse_detect_silence(args: &Value) -> Result<DetectSilence, ToolError> {
	Ok(DetectSilence {
		src: req_path(args, "src")?,
		noise_db: opt_number(args, "noise_db", -30.0, -128.0, 0.0, example_detect())?,
		min_dur: opt_number(args, "min_dur", 0.5, 0.0, 3600.0, example_detect())?,
	})
}

/// `analysis.detect_scenes`.
pub fn parse_detect_scenes(args: &Value) -> Result<DetectScenes, ToolError> {
	Ok(DetectScenes {
		src: req_path(args, "src")?,
		threshold: opt_number(
			args,
			"threshold",
			0.3,
			0.0,
			1.0,
			"{\"src\": \"/media/clip.mp4\", \"threshold\": 0.3}",
		)?,
	})
}

/// `analysis.detect_black`.
pub fn parse_detect_black(args: &Value) -> Result<DetectBlack, ToolError> {
	Ok(DetectBlack {
		src: req_path(args, "src")?,
		min_dur: opt_number(args, "min_dur", 0.3, 0.0, 3600.0, example_detect())?,
		pix_th: opt_number(
			args,
			"pix_th",
			0.10,
			0.0,
			1.0,
			"{\"src\": \"/media/clip.mp4\", \"min_dur\": 0.3, \"pix_th\": 0.1}",
		)?,
	})
}

/// `analysis.loudness`.
pub fn parse_loudness(args: &Value) -> Result<Loudness, ToolError> {
	Ok(Loudness {
		src: req_path(args, "src")?,
	})
}

/// `analysis.export_thumbnails`.
pub fn parse_export_thumbnails(args: &Value) -> Result<ExportThumbnails, ToolError> {
	let interval = match args.get("interval_sec") {
		None | Some(Value::Null) => None,
		Some(v) => Some(match v.as_f64() {
			Some(n) if n.is_finite() && n > 0.0 => n,
			_ => {
				return Err(shape_error(
					args,
					format!("`interval_sec` must be a positive finite number, got {v}"),
					"{\"src\": \"/media/clip.mp4\", \"interval_sec\": 2, \
					 \"outdir\": \"/tmp/thumbs\"}",
				));
			}
		}),
	};
	let times = match args.get("times") {
		None | Some(Value::Null) => None,
		Some(Value::Array(items)) => {
			if items.is_empty() {
				return Err(shape_error(
					args,
					"`times` must not be an empty array".to_string(),
					"{\"times\": [{\"num\": 1, \"den\": 2}, 3]}",
				));
			}
			Some(items.clone())
		}
		Some(other) => {
			return Err(shape_error(
				args,
				format!("`times` must be an array of time values, got {other}"),
				"{\"times\": [{\"num\": 1, \"den\": 2}, 3]}",
			));
		}
	};
	if interval.is_none() && times.is_none() {
		return Err(shape_error(
			args,
			"one of `interval_sec` or `times` is required".to_string(),
			"{\"src\": \"/media/clip.mp4\", \"interval_sec\": 2, \"outdir\": \"/tmp/t\"}",
		));
	}
	Ok(ExportThumbnails {
		src: req_path(args, "src")?,
		interval_sec: interval,
		times,
		outdir: req_path(args, "outdir")?,
		width: match args.get("width") {
			None | Some(Value::Null) => None,
			Some(v) => match v.as_u64() {
				Some(n) if n >= 16 && n <= 8192 => Some(n),
				_ => {
					return Err(shape_error(
						args,
						format!("`width` must be an integer in [16, 8192], got {v}"),
						"{\"width\": 320}",
					));
				}
			},
		},
	})
}

/// Resolves `times` entries against the source's frame rate.
pub fn resolve_times(times: &[Value], rate: Option<Rat>) -> Result<Vec<f64>, ToolError> {
	times
		.iter()
		.map(|t| time_seconds(t, rate, "times"))
		.collect()
}

// ---------------------------------------------------------------------------
// Result shapes — fixed, and mirrored in the schema_v2 documents
// ---------------------------------------------------------------------------

/// `{"intervals": [{"start": .., "end": .., "duration": ..}], "count": n}`
pub fn intervals_json(intervals: &[(f64, f64)]) -> Value {
	let items: Vec<Value> = intervals
		.iter()
		.map(|(s, e)| {
			json!({
				"start": Rat::new((s * 1_000_000.0).round() as i64, 1_000_000).expect("rational")
					.to_json(),
				"end": Rat::new((e * 1_000_000.0).round() as i64, 1_000_000).expect("rational")
					.to_json(),
				"duration": Rat::new(((e - s) * 1_000_000.0).round() as i64, 1_000_000)
					.expect("rational")
					.to_json(),
			})
		})
		.collect();
	json!({ "intervals": items, "count": items.len() })
}

/// `{"points": [{"time": .., "score": ..}], "count": n}`
pub fn points_json(points: &[(f64, Option<f64>)]) -> Value {
	let items: Vec<Value> = points
		.iter()
		.map(|(t, score)| {
			let mut obj = json!({
				"time": Rat::new((t * 1_000_000.0).round() as i64, 1_000_000)
					.expect("rational")
					.to_json(),
			});
			if let Some(s) = score {
				obj["score"] = json!(s);
			}
			obj
		})
		.collect();
	json!({ "points": items, "count": items.len() })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn detect_silence_defaults() {
		let parsed = parse_detect_silence(&json!({"src": "/tmp/a.mp4"})).expect("parse");
		assert_eq!(parsed.noise_db, -30.0);
		assert_eq!(parsed.min_dur, 0.5);
	}

	#[test]
	fn bare_float_time_is_rejected() {
		let err = time_seconds(&json!(1.5), None, "times").expect_err("must reject");
		assert_eq!(err.code, crate::error::CODE_TIMECODE);
	}

	#[test]
	fn rational_and_frame_times_resolve() {
		let half = time_seconds(&json!({"num": 1, "den": 2}), None, "times").expect("rational");
		assert!((half - 0.5).abs() < 1e-9);
		let frame =
			time_seconds(&json!(50), Some(Rat::new(25, 1).expect("rate")), "times").expect("frame");
		assert!((frame - 2.0).abs() < 1e-9);
	}

	#[test]
	fn export_requires_interval_or_times() {
		let err = parse_export_thumbnails(&json!({"src": "/a", "outdir": "/b"}))
			.expect_err("one of the two is required");
		assert_eq!(err.code, crate::error::CODE_INVALID_PARAMS);
	}
}

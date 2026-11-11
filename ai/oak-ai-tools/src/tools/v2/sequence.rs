//! Parsers for the `sequence.*` v2 tools.
//!
//! Parsing runs after schema validation and before the backend call, so a
//! shape problem becomes a structured [`ToolError`] instead of a panic.
//! Field names mirror the schema in [`crate::schema_v2`].

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::Rat;

/// `sequence.get_settings` arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetSettings {
	/// `None` = the current (first) sequence.
	pub sequence: Option<String>,
}

/// `sequence.set_format` arguments — every field optional (**partial**
/// update: absent fields keep the engine's current value).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetFormat {
	pub sequence: Option<String>,
	pub name: Option<String>,
	pub width: Option<u32>,
	pub height: Option<u32>,
	/// Frames per second; a `{"num","den"}` object is exact, a bare number
	/// is mapped onto the standard rate it names (see [`fps_from_value`]).
	pub fps: Option<Rat>,
	pub interlaced: Option<bool>,
}

/// The three-part error every v2 parser returns (T9.3): what arrived, what
/// is wrong, and the shape to send instead.
///
/// The model sees only this text, so a message that stops at "invalid" makes
/// it retry the same call until the round cap — exactly the 6-round failure
/// this helper exists to prevent.
fn shape_error(args: &Value, detail: String, example: &str) -> ToolError {
	ToolError::invalid_params(format!(
		"{detail}. Received: {args}. Send e.g. {example}"
	))
}

/// A string field, also accepting a bare number (models quote numbers).
fn opt_id_string(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
		// Tolerated form: a numeric id (`{"sequence": 3}`). Only integers are
		// accepted — a fractional id is not an id in any spelling.
		Some(Value::Number(n)) => match n.as_i64() {
			Some(v) => Ok(Some(v.to_string())),
			None => Err(shape_error(
				args,
				format!("`{key}` is a numeric id but not an integer: {n}"),
				&format!("{{\"{key}\": \"3\", \"fps\": 60}}"),
			)),
		},
		Some(other) => Err(shape_error(
			args,
			format!("`{key}` must be a non-empty string (or a numeric id), got {other}"),
			&format!("{{\"{key}\": \"3\"}}"),
		)),
	}
}

fn opt_string(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
	opt_id_string(args, key)
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(v) => match v.as_u64() {
			Some(n) if n > 0 && n <= u32::MAX as u64 => Ok(Some(n as u32)),
			_ => Err(shape_error(
				args,
				format!("`{key}` must be a positive integer, got {v}"),
				"{\"width\": 1920, \"height\": 1080}",
			)),
		},
	}
}

/// The standard rates a bare `fps` number may name, with the tolerance the
/// broadcast rates need (`23.976`, `29.97`, `59.94` are the NTSC forms).
pub const STANDARD_RATES: [(f64, i64, i64); 8] = [
	(23.976, 24000, 1001),
	(24.0, 24, 1),
	(25.0, 25, 1),
	(29.97, 30000, 1001),
	(30.0, 30, 1),
	(50.0, 50, 1),
	(59.94, 60000, 1001),
	(60.0, 60, 1),
];

/// Highest numerator that may be read as a plain frames-per-second value
/// when a `{"num": n}` object omits `den` (首辅 ruling, T9.3).
///
/// The guard exists so that `{"num": 60000}` — an NTSC numerator whose
/// meaning is `/1001` — is **refused** instead of being written as 60000 fps.
pub const MAX_IMPLIED_FPS: i64 = 1000;

const FPS_EXAMPLE: &str = "\"fps\": 60, or \"fps\": {\"num\": 60, \"den\": 1} \
                          (NTSC: \"fps\": 29.97, or \"fps\": {\"num\": 30000, \"den\": 1001})";

/// Maps `fps` onto a rational rate.
///
/// Accepted spellings, all unambiguous:
/// * `{"num":n,"den":d}` — taken verbatim (the exact form);
/// * `{"num":n}` — `den = 1`, but only for `1 <= n <= MAX_IMPLIED_FPS`;
///   a larger numerator usually means an NTSC rate missing its `1001`
///   denominator, so it is refused with the exact form suggested;
/// * a bare number — matched against [`STANDARD_RATES`] within 5 ms;
/// * a numeric string (`"60"`, `"29.97"`) — parsed as that number.
///
/// Anything else is refused: inventing a rate would silently misplace every
/// frame.
pub fn fps_from_value(v: &Value) -> Result<Rat, ToolError> {
	let example = FPS_EXAMPLE;
	if let Some(obj) = v.as_object() {
		let num = obj.get("num").and_then(Value::as_i64);
		let den = obj.get("den").and_then(Value::as_i64);
		return match (num, den) {
			(Some(num), Some(den)) if num > 0 && den > 0 => Rat::new(num, den),
			(Some(num), None) if (1..=MAX_IMPLIED_FPS).contains(&num) => Rat::new(num, 1),
			(Some(num), None) => Err(ToolError::invalid_params(format!(
				"`fps` object {v} omits `den`, and `num` {num} is out of the \
				 unambiguous range 1..={MAX_IMPLIED_FPS}; a numerator like 60000 \
				 normally means 60000/1001, so send the denominator too. \
				 Send e.g. {example}"
			))),
			_ => Err(ToolError::invalid_params(format!(
				"`fps` rational must have positive integer num and den, got {v}. \
				 Send e.g. {example}"
			))),
		};
	}
	let number = if let Some(f) = v.as_f64() {
		Some(f)
	} else if let Some(text) = v.as_str() {
		// Tolerated form: a quoted number (`"60"`, `"29.97"`). Only a plain
		// decimal is accepted; anything else keeps its original spelling in
		// the message so the model can see what it sent.
		text.trim().parse::<f64>().ok()
	} else {
		None
	};
	let Some(f) = number else {
		return Err(ToolError::invalid_params(format!(
			"`fps` must be a number, a numeric string, or a {{\"num\",\"den\"}} \
			 object, got {v}. Send e.g. {example}"
		)));
	};
	if f <= 0.0 {
		return Err(ToolError::invalid_params(format!(
			"`fps` must be positive, got {f}. Send e.g. {example}"
		)));
	}
	for (named, num, den) in STANDARD_RATES {
		if (f - named).abs() < 0.005 {
			return Rat::new(num, den);
		}
	}
	Err(ToolError::invalid_params(format!(
		"`fps` {f} is not a standard rate; pass the exact rational, e.g. \
		 {{\"fps\": {{\"num\":{},\"den\":1001}}}} (known: {})",
		(f * 1001.0).round() as i64,
		STANDARD_RATES
			.iter()
			.map(|(n, _, _)| format!("{n}"))
			.collect::<Vec<_>>()
			.join(", ")
	)))
}

/// `sequence.get_settings` parser.
pub fn parse_get_settings(args: &Value) -> Result<GetSettings, ToolError> {
	Ok(GetSettings {
		sequence: opt_id_string(args, "sequence")?,
	})
}

/// Keys `sequence.get_settings` returns that carry no writable meaning:
/// accepted and silently ignored, so a model can read the settings and write
/// them straight back (the field's most natural behaviour, and the one that
/// used to cost a round per key).
///
/// Misspelled *writable* keys (`frameRate`) are **not** in this list: they
/// are refused by the schema and named in the error. Ignorance is licensed
/// only for the read-only echo.
pub const READ_ONLY_ECHO_KEYS: [&str; 4] = ["id", "duration", "backend", "undoable"];

/// `sequence.set_format` parser.
pub fn parse_set_format(args: &Value) -> Result<SetFormat, ToolError> {
	// `rate` is the name `sequence.get_settings` reports the frame rate
	// under, so it is accepted as a full alias of `fps`: a model that reads
	// and writes back must not have to rename the field. `fps` wins if both
	// arrive.
	let rate_value = match args.get("fps") {
		Some(Value::Null) | None => args.get("rate"),
		Some(v) => Some(v),
	};
	let fps = match rate_value {
		None | Some(Value::Null) => None,
		Some(v) => Some(fps_from_value(v).map_err(|e| {
			// Keep fps_from_value's precise wording, add what arrived.
			ToolError::invalid_params(format!("{}. Received: {args}", e.message))
		})?),
	};
	let parsed = SetFormat {
		sequence: opt_id_string(args, "sequence")?,
		name: opt_string(args, "name")?,
		width: opt_u32(args, "width")?,
		height: opt_u32(args, "height")?,
		fps,
		interlaced: args.get("interlaced").and_then(Value::as_bool),
	};
	if parsed.name.is_none()
		&& parsed.width.is_none()
		&& parsed.height.is_none()
		&& parsed.fps.is_none()
		&& parsed.interlaced.is_none()
	{
		return Err(shape_error(
			args,
			"`sequence.set_format` needs at least one of name / width / height / fps / \
			 interlaced"
				.to_string(),
			FPS_EXAMPLE,
		));
	}
	Ok(parsed)
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn ntsc_bare_numbers_map_onto_their_exact_rationals() {
		let r = fps_from_value(&json!(29.97)).expect("29.97 is a standard rate");
		assert_eq!((r.num, r.den), (30000, 1001));
		let r = fps_from_value(&json!(23.976)).expect("23.976 is a standard rate");
		assert_eq!((r.num, r.den), (24000, 1001));
		let r = fps_from_value(&json!(24)).expect("24 is a standard rate");
		assert_eq!((r.num, r.den), (24, 1));
	}

	#[test]
	fn an_invented_rate_is_refused_with_the_exact_form() {
		let err = fps_from_value(&json!(17.3)).unwrap_err();
		assert_eq!(err.code, crate::error::CODE_INVALID_PARAMS);
		assert!(err.message.contains("num"), "{}", err.message);
	}

	#[test]
	fn exact_rationals_are_taken_verbatim() {
		let r = fps_from_value(&json!({"num": 30000, "den": 1001})).unwrap();
		assert_eq!((r.num, r.den), (30000, 1001));
	}

	/// T9.3: the three shapes the field failure produced must now parse.
	#[test]
	fn the_three_reported_shapes_now_parse() {
		// (3) a quoted number
		let p = parse_set_format(&json!({ "fps": "60" })).expect("quoted number");
		assert_eq!(p.fps.map(|r| (r.num, r.den)), Some((60, 1)));
		let p = parse_set_format(&json!({ "fps": "29.97" })).expect("quoted NTSC");
		assert_eq!(p.fps.map(|r| (r.num, r.den)), Some((30000, 1001)));
		// (5) `{num}` with a plausible numerator implies den = 1
		let p = parse_set_format(&json!({ "fps": { "num": 60 } })).expect("implied den");
		assert_eq!(p.fps.map(|r| (r.num, r.den)), Some((60, 1)));
		// (7) a numeric sequence id
		let p = parse_set_format(&json!({ "sequence": 3, "fps": 60 })).expect("numeric id");
		assert_eq!(p.sequence.as_deref(), Some("3"));
	}

	/// The guard on the implied denominator: `{"num": 60000}` means
	/// 60000/1001, so it must be **refused**, never written as 60000 fps.
	#[test]
	fn an_implied_denominator_is_refused_beyond_the_plain_range() {
		let err = parse_set_format(&json!({ "fps": { "num": 60000 } })).unwrap_err();
		assert_eq!(err.code, crate::error::CODE_INVALID_PARAMS);
		assert!(
			err.message.contains("60000/1001") && err.message.contains("den"),
			"the refusal must suggest the exact form: {}",
			err.message
		);
	}

	/// Every parse error is three-part: received args + what is wrong + an
	/// example, which is what lets the model fix it in one turn.
	#[test]
	fn parse_errors_are_actionable() {
		let err = parse_set_format(&json!({ "fps": "sixty" })).unwrap_err();
		assert!(err.message.contains("Received:"), "{}", err.message);
		assert!(err.message.contains("\"fps\""), "{}", err.message);
		assert!(
			err.message.contains("num") && err.message.contains("den"),
			"the example must show the exact form: {}",
			err.message
		);
	}

	#[test]
	fn set_format_without_any_field_is_refused() {
		let err = parse_set_format(&json!({})).unwrap_err();
		assert_eq!(err.code, crate::error::CODE_INVALID_PARAMS);
	}

	#[test]
	fn partial_fields_parse_independently() {
		let p = parse_set_format(&json!({"fps": 24})).unwrap();
		assert_eq!(p.fps.map(|r| (r.num, r.den)), Some((24, 1)));
		assert!(p.width.is_none() && p.height.is_none() && p.name.is_none());
	}
}

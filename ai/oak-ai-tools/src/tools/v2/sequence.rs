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

fn opt_string(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
		Some(other) => Err(ToolError::invalid_params(format!(
			"`{key}` must be a non-empty string, got {other}"
		))),
	}
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(v) => match v.as_u64() {
			Some(n) if n > 0 && n <= u32::MAX as u64 => Ok(Some(n as u32)),
			_ => Err(ToolError::invalid_params(format!(
				"`{key}` must be a positive integer, got {v}"
			))),
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

/// Maps `fps` onto a rational rate.
///
/// `{"num":30000,"den":1001}` is taken verbatim (the exact form). A bare
/// number is matched against [`STANDARD_RATES`] within 5 ms so `29.97`
/// becomes `30000/1001` rather than `2997/100` — an invented rate would
/// silently misplace every frame; anything else is refused with the exact
/// form suggested.
pub fn fps_from_value(v: &Value) -> Result<Rat, ToolError> {
	if let Some(obj) = v.as_object() {
		let num = obj.get("num").and_then(Value::as_i64);
		let den = obj.get("den").and_then(Value::as_i64);
		if let (Some(num), Some(den)) = (num, den) {
			if num > 0 && den > 0 {
				return Rat::new(num, den);
			}
		}
		return Err(ToolError::invalid_params(format!(
			"`fps` rational must have positive num and den, got {v}"
		)));
	}
	let Some(f) = v.as_f64() else {
		return Err(ToolError::invalid_params(format!(
			"`fps` must be a number or a {{\"num\",\"den\"}} object, got {v}"
		)));
	};
	if f <= 0.0 {
		return Err(ToolError::invalid_params(format!(
			"`fps` must be positive, got {f}"
		)));
	}
	for (named, num, den) in STANDARD_RATES {
		if (f - named).abs() < 0.005 {
			return Rat::new(num, den);
		}
	}
	Err(ToolError::invalid_params(format!(
		"`fps` {f} is not a standard rate; pass the exact rational, e.g. \
		 {{\"num\":{},\"den\":1001}} (known: {})",
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
		sequence: opt_string(args, "sequence")?,
	})
}

/// `sequence.set_format` parser.
pub fn parse_set_format(args: &Value) -> Result<SetFormat, ToolError> {
	let fps = match args.get("fps") {
		None | Some(Value::Null) => None,
		Some(v) => Some(fps_from_value(v)?),
	};
	let parsed = SetFormat {
		sequence: opt_string(args, "sequence")?,
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
		return Err(ToolError::invalid_params(
			"`sequence.set_format` needs at least one of name / width / height / fps / interlaced",
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

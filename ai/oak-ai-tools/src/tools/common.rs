//! Shared argument extraction helpers.

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::{TimeValue, time_arg};

/// Required non-empty string field.
pub fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
	let value = args
		.get(key)
		.and_then(Value::as_str)
		.filter(|s| !s.is_empty())
		.ok_or_else(|| ToolError::invalid_params(format!("field '{key}' must be a non-empty string")))?;
	Ok(value)
}

/// Optional non-empty string field.
pub fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(value) => value
			.as_str()
			.filter(|s| !s.is_empty())
			.map(Some)
			.ok_or_else(|| ToolError::invalid_params(format!("field '{key}' must be a non-empty string"))),
	}
}

/// Required non-negative integer field.
pub fn req_usize(args: &Value, key: &str) -> Result<usize, ToolError> {
	let value = args
		.get(key)
		.and_then(Value::as_u64)
		.ok_or_else(|| ToolError::invalid_params(format!("field '{key}' must be a non-negative integer")))?;
	usize::try_from(value)
		.map_err(|_| ToolError::invalid_params(format!("field '{key}' is out of range")))
}

/// Optional non-negative integer field.
pub fn opt_usize(args: &Value, key: &str) -> Result<Option<usize>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(_) => req_usize(args, key).map(Some),
	}
}

/// Optional positive integer field (counts, sizes).
pub fn opt_positive(args: &Value, key: &str) -> Result<Option<u64>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(_) => {
			let value = args
				.get(key)
				.and_then(Value::as_u64)
				.filter(|v| *v >= 1)
				.ok_or_else(|| ToolError::invalid_params(format!("field '{key}' must be >= 1")))?;
			Ok(Some(value))
		}
	}
}

/// A left-closed right-open `{in, out}` interval of time values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeArg {
	/// Start (inclusive).
	pub in_: TimeValue,
	/// End (exclusive).
	pub out: TimeValue,
}

/// Reads a required `range: {in, out}` object.
pub fn req_range(args: &Value, key: &str) -> Result<RangeArg, ToolError> {
	let range = args
		.get(key)
		.ok_or_else(|| ToolError::invalid_params(format!("missing field '{key}'")))?;
	if !range.is_object() {
		return Err(ToolError::invalid_params(format!(
			"field '{key}' must be an object with 'in' and 'out'"
		)));
	}
	Ok(RangeArg {
		in_: time_arg(range, "in")?,
		out: time_arg(range, "out")?,
	})
}

/// Reads an optional `max_size: {width, height}` object.
pub fn opt_max_size(args: &Value, key: &str) -> Result<Option<(u32, u32)>, ToolError> {
	let Some(value) = args.get(key) else {
		return Ok(None);
	};
	if value.is_null() {
		return Ok(None);
	}
	let width = value
		.get("width")
		.and_then(Value::as_u64)
		.filter(|v| *v >= 1)
		.ok_or_else(|| ToolError::invalid_params(format!("'{key}.width' must be an integer >= 1")))?;
	let height = value
		.get("height")
		.and_then(Value::as_u64)
		.filter(|v| *v >= 1)
		.ok_or_else(|| ToolError::invalid_params(format!("'{key}.height' must be an integer >= 1")))?;
	Ok(Some((
		u32::try_from(width).unwrap_or(u32::MAX),
		u32::try_from(height).unwrap_or(u32::MAX),
	)))
}

/// Resolves `value` against the backend's frame grid, mapping failures
/// onto `TIMECODE_INVALID` with the entity in the message.
pub fn resolve(
	backend: &dyn crate::backend::EngineBackend,
	ctx: crate::timecode::TimeContext,
	value: TimeValue,
) -> Result<crate::timecode::Rat, ToolError> {
	let described = format!("{value} on {ctx}");
	backend.rational_at(ctx, value).map_err(|err| {
		ToolError::timecode(format!("{described}: {}", err.message))
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn string_and_integer_extraction() {
		let args = json!({"clip": "clip-1", "count": 4, "index": 0});
		assert_eq!(req_str(&args, "clip").unwrap(), "clip-1");
		assert!(req_str(&args, "missing").is_err());
		assert_eq!(req_usize(&args, "count").unwrap(), 4);
		assert_eq!(opt_usize(&args, "index").unwrap(), Some(0));
		// Present-and-valid through the optional accessor, absent otherwise.
		assert_eq!(opt_usize(&args, "count").unwrap(), Some(4));
		assert_eq!(opt_usize(&args, "nope").unwrap(), None);
		let negative = json!({"count": -1});
		assert!(opt_usize(&negative, "count").is_err());
	}

	#[test]
	fn range_parsing() {
		let args = json!({"range": {"in": {"num": 1, "den": 2}, "out": 30}});
		let range = req_range(&args, "range").unwrap();
		assert_eq!(range.in_, TimeValue::Rational(crate::timecode::Rat::new(1, 2).unwrap()));
		assert_eq!(range.out, TimeValue::Frame(30));
		let bad = json!({"range": {"in": 1.5, "out": 3}});
		assert!(req_range(&bad, "range").is_err());
	}

	#[test]
	fn max_size_parsing() {
		let args = json!({"max_size": {"width": 320, "height": 180}});
		assert_eq!(opt_max_size(&args, "max_size").unwrap(), Some((320, 180)));
		let bad = json!({"max_size": {"width": 0, "height": 180}});
		assert!(opt_max_size(&bad, "max_size").is_err());
	}
}

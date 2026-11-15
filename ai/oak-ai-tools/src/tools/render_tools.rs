//! Arguments for `render.*`.

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::{TimeValue, time_arg};
use crate::tools::common::{opt_positive, opt_str, req_range, req_str, RangeArg};

/// Which target a render call addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameTarget {
	/// A sequence (composed timeline frame).
	Sequence(String),
	/// A raw footage file.
	Footage(String),
}

impl FrameTarget {
	/// Resolves `sequence` XOR `footage` (the frozen schema rejects both
	/// and neither; this repeats the check for direct backend calls).
	pub fn from_args(args: &Value) -> Result<FrameTarget, ToolError> {
		let sequence = opt_str(args, "sequence")?;
		let footage = opt_str(args, "footage")?;
		match (sequence, footage) {
			(Some(_), Some(_)) => Err(ToolError::invalid_params(
				"exactly one of 'sequence' / 'footage' must be given, not both",
			)),
			(Some(seq), None) => Ok(FrameTarget::Sequence(seq.to_string())),
			(None, Some(foot)) => Ok(FrameTarget::Footage(foot.to_string())),
			(None, None) => Err(ToolError::invalid_params(
				"exactly one of 'sequence' / 'footage' must be given",
			)),
		}
	}

	/// The id the caller named.
	pub fn id(&self) -> &str {
		match self {
			FrameTarget::Sequence(id) => id,
			FrameTarget::Footage(id) => id,
		}
	}
}

/// `render.get_frame`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetFrameArgs {
	/// Sequence or footage target.
	pub target: FrameTarget,
	/// When to render.
	pub time: TimeValue,
	/// Downscale bound.
	pub max_size: Option<(u32, u32)>,
	/// `png` (default) or `bgra8`.
	pub format: String,
}

/// Parses `render.get_frame`.
pub fn parse_get_frame(args: &Value) -> Result<GetFrameArgs, ToolError> {
	let format = opt_str(args, "format")?.unwrap_or("png").to_string();
	if format != "png" && format != "bgra8" {
		return Err(ToolError::invalid_params(format!(
			"format must be 'png' or 'bgra8', got '{format}'"
		)));
	}
	Ok(GetFrameArgs {
		target: FrameTarget::from_args(args)?,
		time: time_arg(args, "time")?,
		max_size: crate::tools::common::opt_max_size(args, "max_size")?,
		format,
	})
}

/// `render.get_thumbnails`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetThumbnailsArgs {
	/// Sequence or footage target.
	pub target: FrameTarget,
	/// Sampling interval (left-closed right-open).
	pub range: RangeArg,
	/// Number of samples (1..=64, frozen).
	pub count: usize,
	/// Thumbnail height in pixels.
	pub height: Option<u32>,
}

/// Parses `render.get_thumbnails`.
pub fn parse_get_thumbnails(args: &Value) -> Result<GetThumbnailsArgs, ToolError> {
	let count = crate::tools::common::req_usize(args, "count")?;
	if !(1..=64).contains(&count) {
		return Err(ToolError::invalid_params(format!(
			"count must be within 1..=64, got {count}"
		)));
	}
	let height = opt_positive(args, "height")?
		.map(|v| u32::try_from(v).unwrap_or(u32::MAX));
	Ok(GetThumbnailsArgs {
		target: FrameTarget::from_args(args)?,
		range: req_range(args, "range")?,
		count,
		height,
	})
}

/// `render.get_audio_levels`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAudioLevelsArgs {
	/// Sequence to analyse (required by OPP/1 section 8.6).
	pub sequence: String,
	/// Analysis interval.
	pub range: RangeArg,
	/// Resolution of the level buckets (>= 1).
	pub resolution: Option<u64>,
}

/// Parses `render.get_audio_levels`.
pub fn parse_get_audio_levels(args: &Value) -> Result<GetAudioLevelsArgs, ToolError> {
	Ok(GetAudioLevelsArgs {
		sequence: req_str(args, "sequence")?.to_string(),
		range: req_range(args, "range")?,
		resolution: opt_positive(args, "resolution")?,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use crate::timecode::Rat;

	#[test]
	fn target_is_exactly_one_of_two() {
		assert!(matches!(
			FrameTarget::from_args(&json!({"sequence": "seq-1"})).unwrap(),
			FrameTarget::Sequence(_)
		));
		assert!(matches!(
			FrameTarget::from_args(&json!({"footage": "footage-1"})).unwrap(),
			FrameTarget::Footage(_)
		));
		assert!(FrameTarget::from_args(&json!({})).is_err());
		assert!(
			FrameTarget::from_args(&json!({"sequence": "s", "footage": "f"})).is_err()
		);
	}

	#[test]
	fn frame_args_defaults_to_png() {
		let args = json!({"sequence": "seq-1", "time": {"num": 1, "den": 2}});
		let parsed = parse_get_frame(&args).unwrap();
		assert_eq!(parsed.format, "png");
		assert_eq!(parsed.time, TimeValue::Rational(Rat::new(1, 2).unwrap()));
		let bad = json!({"sequence": "seq-1", "time": 1.5});
		assert!(parse_get_frame(&bad).is_err());
	}

	#[test]
	fn thumbnails_count_is_bounded() {
		let base = json!({"footage": "f", "range": {"in": 0, "out": 10}});
		let mut with_count = base.clone();
		with_count["count"] = json!(64);
		assert!(parse_get_thumbnails(&with_count).is_ok());
		with_count["count"] = json!(65);
		assert!(parse_get_thumbnails(&with_count).is_err());
		with_count["count"] = json!(0);
		assert!(parse_get_thumbnails(&with_count).is_err());
	}
}

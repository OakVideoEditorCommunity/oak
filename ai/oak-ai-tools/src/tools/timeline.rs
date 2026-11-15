//! Arguments for `timeline.*`.

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::{TimeValue, opt_time_arg, time_arg};
use crate::tools::common::{opt_str, req_str, opt_usize};

/// Which edge of a clip an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
	/// Head of the clip.
	In,
	/// Tail of the clip.
	Out,
}

impl Side {
	/// The wire spelling.
	pub fn as_str(self) -> &'static str {
		match self {
			Side::In => "in",
			Side::Out => "out",
		}
	}

	/// Parses `in` / `out`.
	pub fn parse(raw: &str) -> Result<Side, ToolError> {
		match raw {
			"in" => Ok(Side::In),
			"out" => Ok(Side::Out),
			other => Err(ToolError::invalid_params(format!(
				"side must be 'in' or 'out', got '{other}'"
			))),
		}
	}
}

/// Engine transition shader ids (frozen enum in the T0 schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionType {
	/// `crossdissolve`
	Crossdissolve,
	/// `fade`
	Fade,
	/// `wipe`
	Wipe,
	/// `slide`
	Slide,
}

impl TransitionType {
	/// The wire spelling (the engine shader id).
	pub fn as_str(self) -> &'static str {
		match self {
			TransitionType::Crossdissolve => "crossdissolve",
			TransitionType::Fade => "fade",
			TransitionType::Wipe => "wipe",
			TransitionType::Slide => "slide",
		}
	}

	/// Parses one of the four frozen ids.
	pub fn parse(raw: &str) -> Result<TransitionType, ToolError> {
		match raw {
			"crossdissolve" => Ok(TransitionType::Crossdissolve),
			"fade" => Ok(TransitionType::Fade),
			"wipe" => Ok(TransitionType::Wipe),
			"slide" => Ok(TransitionType::Slide),
			other => Err(ToolError::invalid_params(format!(
				"unknown transition type '{other}' (expected crossdissolve|fade|wipe|slide)"
			))),
		}
	}
}

/// `timeline.add_track`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddTrackArgs {
	/// Target sequence (`None` = active sequence).
	pub sequence: Option<String>,
	/// `video` or `audio`.
	pub track_type: String,
	/// Insert position (`None` = append).
	pub index: Option<usize>,
}

/// Parses `timeline.add_track`.
pub fn parse_add_track(args: &Value) -> Result<AddTrackArgs, ToolError> {
	let track_type = req_str(args, "type")?.to_string();
	if track_type != "video" && track_type != "audio" {
		return Err(ToolError::invalid_params(format!(
			"type must be 'video' or 'audio', got '{track_type}'"
		)));
	}
	Ok(AddTrackArgs {
		sequence: opt_str(args, "sequence")?.map(str::to_string),
		track_type,
		index: opt_usize(args, "index")?,
	})
}

/// `timeline.place_clip`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceClipArgs {
	/// Target sequence (`None` = active sequence).
	pub sequence: Option<String>,
	/// Destination track id.
	pub track: String,
	/// Source footage id.
	pub footage: String,
	/// Timeline position of the new clip head.
	pub in_: TimeValue,
	/// Media in-point (`None` = 0).
	pub media_in: Option<TimeValue>,
}

/// Parses `timeline.place_clip`.
pub fn parse_place_clip(args: &Value) -> Result<PlaceClipArgs, ToolError> {
	Ok(PlaceClipArgs {
		sequence: opt_str(args, "sequence")?.map(str::to_string),
		track: req_str(args, "track")?.to_string(),
		footage: req_str(args, "footage")?.to_string(),
		in_: time_arg(args, "in")?,
		media_in: opt_time_arg(args, "media_in")?,
	})
}

/// `timeline.split_clip`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitArgs {
	/// Clip to split.
	pub clip: String,
	/// Split point (strictly inside the clip).
	pub time: TimeValue,
}

/// Parses `timeline.split_clip`.
pub fn parse_split(args: &Value) -> Result<SplitArgs, ToolError> {
	Ok(SplitArgs {
		clip: req_str(args, "clip")?.to_string(),
		time: time_arg(args, "time")?,
	})
}

/// `timeline.trim_clip`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimArgs {
	/// Clip to trim.
	pub clip: String,
	/// Which edge moves.
	pub side: Side,
	/// New edge position.
	pub time: TimeValue,
}

/// Parses `timeline.trim_clip`.
pub fn parse_trim(args: &Value) -> Result<TrimArgs, ToolError> {
	Ok(TrimArgs {
		clip: req_str(args, "clip")?.to_string(),
		side: Side::parse(req_str(args, "side")?)?,
		time: time_arg(args, "time")?,
	})
}

/// `timeline.move_clip`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveArgs {
	/// Clip to move.
	pub clip: String,
	/// New head position.
	pub in_: TimeValue,
	/// Destination track (`None` = keep the current one).
	pub track: Option<String>,
}

/// Parses `timeline.move_clip`.
pub fn parse_move(args: &Value) -> Result<MoveArgs, ToolError> {
	Ok(MoveArgs {
		clip: req_str(args, "clip")?.to_string(),
		in_: time_arg(args, "in")?,
		track: opt_str(args, "track")?.map(str::to_string),
	})
}

/// `timeline.ripple_delete`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RippleDeleteArgs {
	/// Clip to remove (the gap it leaves is closed).
	pub clip: String,
}

/// Parses `timeline.ripple_delete`.
pub fn parse_ripple_delete(args: &Value) -> Result<RippleDeleteArgs, ToolError> {
	Ok(RippleDeleteArgs {
		clip: req_str(args, "clip")?.to_string(),
	})
}

/// `timeline.add_transition`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddTransitionArgs {
	/// Clip owning the transition edge.
	pub clip: String,
	/// Which edge.
	pub side: Side,
	/// Transition type (engine shader id).
	pub transition_type: TransitionType,
	/// Consumed duration on the sequence timeline.
	pub duration: TimeValue,
}

/// Parses `timeline.add_transition`.
pub fn parse_add_transition(args: &Value) -> Result<AddTransitionArgs, ToolError> {
	Ok(AddTransitionArgs {
		clip: req_str(args, "clip")?.to_string(),
		side: Side::parse(req_str(args, "side")?)?,
		transition_type: TransitionType::parse(req_str(args, "type")?)?,
		duration: time_arg(args, "duration")?,
	})
}

/// `timeline.add_marker`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddMarkerArgs {
	/// Target sequence (`None` = active sequence).
	pub sequence: Option<String>,
	/// Marker position.
	pub time: TimeValue,
	/// Optional label.
	pub name: Option<String>,
	/// Optional `#RRGGBB` / `#RRGGBBAA` colour.
	pub color: Option<String>,
}

/// Parses `timeline.add_marker`.
pub fn parse_add_marker(args: &Value) -> Result<AddMarkerArgs, ToolError> {
	let color = opt_str(args, "color")?.map(str::to_string);
	if let Some(color) = &color {
		if !is_hex_color(color) {
			return Err(ToolError::invalid_params(format!(
				"color must be #RRGGBB or #RRGGBBAA, got '{color}'"
			)));
		}
	}
	Ok(AddMarkerArgs {
		sequence: opt_str(args, "sequence")?.map(str::to_string),
		time: time_arg(args, "time")?,
		name: opt_str(args, "name")?.map(str::to_string),
		color,
	})
}

/// `#RRGGBB` or `#RRGGBBAA` (mirrors the frozen schema pattern).
pub fn is_hex_color(value: &str) -> bool {
	let bytes = value.as_bytes();
	if bytes.first() != Some(&b'#') {
		return false;
	}
	let body = &bytes[1..];
	(body.len() == 6 || body.len() == 8)
		&& body.iter().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use crate::timecode::Rat;

	#[test]
	fn side_and_transition_parsing() {
		assert_eq!(Side::parse("in").unwrap(), Side::In);
		assert_eq!(Side::parse("out").unwrap(), Side::Out);
		assert!(Side::parse("middle").is_err());
		assert_eq!(
			TransitionType::parse("crossdissolve").unwrap(),
			TransitionType::Crossdissolve
		);
		assert!(TransitionType::parse("dissolve").is_err());
	}

	#[test]
	fn place_clip_parses_both_time_shapes() {
		let args = json!({
			"track": "track-1", "footage": "footage-1",
			"in": {"num": 0, "den": 1},
			"media_in": 15
		});
		let parsed = parse_place_clip(&args).unwrap();
		assert_eq!(parsed.in_, TimeValue::Rational(Rat::new(0, 1).unwrap()));
		assert_eq!(parsed.media_in, Some(TimeValue::Frame(15)));
		// Bare floats die here even if a schema gate were bypassed.
		let bad = json!({"track": "t", "footage": "f", "in": 1.5});
		assert!(parse_place_clip(&bad).is_err());
	}

	#[test]
	fn marker_colour_shape() {
		assert!(is_hex_color("#1a2b3c"));
		assert!(is_hex_color("#1a2b3c4d"));
		assert!(!is_hex_color("1a2b3c"));
		assert!(!is_hex_color("#1a2b3"));
		let ok = json!({"time": 0, "color": "#ff0000"});
		assert!(parse_add_marker(&ok).is_ok());
		let bad = json!({"time": 0, "color": "red"});
		assert!(parse_add_marker(&bad).is_err());
	}

	#[test]
	fn add_track_rejects_unknown_type() {
		assert!(parse_add_track(&json!({"type": "video"})).is_ok());
		assert!(parse_add_track(&json!({"type": "subtitle"})).is_err());
	}
}

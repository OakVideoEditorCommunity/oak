//! Native data carriers and results shared by the command ABI and dispatcher.

use oak_core::{Rational, TimeRange};
use oak_node::id::NodeId;
use oak_node::track::TrackType;

use crate::editor_commands as ops;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// C-compatible rational number. A zero denominator represents a null value
/// where a command documents its rational output as optional.
pub struct WireRational {
	pub num: i64,
	pub den: i64,
}

impl WireRational {
	/// Converts to the domain rational without changing numerator/denominator.
	/// Domain code interprets a zero denominator according to its own contract.
	pub(crate) fn native(self) -> Rational {
		Rational::new(self.num, self.den)
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// Half-open time range carried as two native rationals.
pub struct WireTimeRange {
	pub in_: WireRational,
	pub out: WireRational,
}

impl WireTimeRange {
	/// Converts both endpoints while preserving their order and null markers.
	pub(crate) fn native(self) -> TimeRange {
		TimeRange::new(self.in_.native(), self.out.native())
	}
}

#[repr(C)]
#[derive(Clone, Debug)]
/// Clipboard entry returned to a caller through its preallocated buffer.
/// Timestamp fields use the source sequence's frame timebase.
pub struct WireClipboardClip {
	pub footage: u64,
	pub kind: i32,
	pub track_index: usize,
	pub media_in_ts: i64,
	pub start_ts: i64,
	pub length_ts: i64,
	pub speed: f64,
}

impl WireClipboardClip {
	/// Converts an owned domain clipboard entry without borrowing project data.
	pub(crate) fn wire(value: ops::ClipboardClip) -> Self {
		Self {
			footage: value.footage.identity(),
			kind: value.kind.to_c(),
			track_index: value.track_index,
			media_in_ts: value.media_in_ts,
			start_ts: value.start_ts,
			length_ts: value.length_ts,
			speed: value.speed,
		}
	}
}

/// Resolves a raw generational identity, rejecting the invalid-ID sentinel.
pub(crate) fn id(raw: u64) -> Result<NodeId, String> {
	NodeId::from_identity(raw).ok_or_else(|| format!("invalid node id: {raw}"))
}

/// Decodes the C track-kind discriminant instead of silently defaulting it.
pub(crate) fn track_kind(raw: i32) -> Result<TrackType, String> {
	TrackType::from_c(raw).ok_or_else(|| format!("invalid track kind: {raw}"))
}

/// Engine-side command output before it is copied into caller-owned buffers.
#[derive(Clone, Debug, Default)]
pub struct EditResult {
	/// Primary created/affected node identity.
	pub primary: Option<u64>,
	/// Ordered identities for multi-node operations.
	pub ids: Vec<u64>,
	/// Operation-specific count or index.
	pub value: i64,
	/// Operation-specific rational value, such as a clamped duration.
	pub rational: Option<WireRational>,
	/// Reserved for the ABI result; dispatch leaves revision collection to the wrapper.
	pub revision: i64,
	/// Session clipboard snapshot returned by copy.
	pub clipboard: Vec<WireClipboardClip>,
}

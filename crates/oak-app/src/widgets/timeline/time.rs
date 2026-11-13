//! Frame-exact time types for the timeline widget.
//!
//! The timeline's canonical time unit is the **frame**, expressed as an
//! [`i64`] inside [`Frame`]. Durations and positions are never stored as
//! floating-point seconds: floats drift and accumulate rounding error, which
//! is unacceptable for an editing tool where an off-by-one frame is a visible
//! bug. The only place seconds appear is at the edges of the system —
//! converting wall-clock playback time into frames (see
//! [`super::playhead::PlayheadTicker`]) and formatting human-readable labels
//! via [`format_timecode`].
//!
//! This mirrors the Oak engine's time model, where canonical time is a
//! rational `int64 num/den` and the ABI exchanges `int64` frame timestamps
//! together with a per-sequence [`FrameRate`] such as `30000/1001`.
//!
//! # Invariants
//!
//! * A [`FrameRate`] is always normalized to a positive, non-zero numerator
//!   and denominator by [`FrameRate::new`].
//! * All conversions that produce frames from floats round deterministically;
//!   see [`seconds_to_frame`] for the exact rounding policy.
//! * [`FrameRange`] is half-open: `[start, end)`.

use gpui_kit::{Pixels, px};

/// A rational frame rate: `num / den` frames per second.
///
/// Frame rates in professional video are frequently *not* integers; the
/// classic example is NTSC 29.97 fps, which is exactly `30000/1001`. Storing
/// the rate as a pair of integers keeps every downstream computation exact.
///
/// # Invariants
///
/// Both `num` and `den` are guaranteed non-zero after construction through
/// [`FrameRate::new`]. Constructing the struct literal directly is possible
/// (the fields are public so the type is usable in `const` contexts) but
/// callers must uphold the non-zero invariant themselves.
///
/// # Examples
///
/// ```
/// # use oak_widgets::timeline::FrameRate;
/// let ntsc = FrameRate::new(30000, 1001);
/// assert!((ntsc.as_f64() - 29.97002997).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
	/// Numerator of the rate (frames per `den` seconds). Must be non-zero.
	pub num: u32,
	/// Denominator of the rate. Must be non-zero.
	pub den: u32,
}

impl FrameRate {
	/// NTSC "29.97" fps, exactly `30000/1001`.
	pub const NTSC_2997: FrameRate = FrameRate {
		num: 30000,
		den: 1001,
	};

	/// NTSC "23.976" fps, exactly `24000/1001`.
	pub const NTSC_23976: FrameRate = FrameRate {
		num: 24000,
		den: 1001,
	};

	/// Creates a frame rate from a numerator and denominator.
	///
	/// # Panics
	///
	/// Panics if either `num` or `den` is zero — a zero frame rate is
	/// meaningless and would cause division by zero in every conversion.
	///
	/// # Examples
	///
	/// ```
	/// # use oak_widgets::timeline::FrameRate;
	/// let pal = FrameRate::new(25, 1);
	/// assert_eq!(pal.as_f64(), 25.0);
	/// ```
	pub fn new(num: u32, den: u32) -> Self {
		assert!(
			num != 0 && den != 0,
			"frame rate components must be non-zero"
		);
		FrameRate { num, den }
	}

	/// Returns the rate as a floating-point frames-per-second value.
	///
	/// Intended for display and for one-shot wall-clock conversions only;
	/// never store positions or durations derived from this value.
	pub fn as_f64(self) -> f64 {
		self.num as f64 / self.den as f64
	}
}

/// An absolute position or duration on the timeline, in frames.
///
/// Negative values are representable (they occasionally arise during drag
/// interactions before clamping) but are never valid as final positions in a
/// sequence; consumers should clamp to `[Frame(0), sequence_length)`.
///
/// `Frame` is `Ord`, so collections of frames (and of [`ClipId`](super::ClipId)s
/// keyed by frame) sort naturally.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frame(pub i64);

impl Frame {
	/// Frame zero, the start of every sequence.
	pub const ZERO: Frame = Frame(0);

	/// Returns the raw frame number.
	pub fn number(self) -> i64 {
		self.0
	}

	/// Returns this frame as a [`Pixels`] offset at the given zoom
	/// (pixels per frame). Pure scaling, no scroll compensation — see
	/// [`TimelineState::point_at_frame`](super::TimelineState::point_at_frame)
	/// for the scroll-aware variant.
	pub fn to_pixels(self, zoom: f32) -> Pixels {
		Pixels::from(self.0 as f32 * zoom)
	}
}

impl std::ops::Add for Frame {
	type Output = Frame;

	fn add(self, rhs: Frame) -> Frame {
		Frame(self.0 + rhs.0)
	}
}

impl std::ops::Sub for Frame {
	type Output = Frame;

	fn sub(self, rhs: Frame) -> Frame {
		Frame(self.0 - rhs.0)
	}
}

impl std::ops::AddAssign for Frame {
	fn add_assign(&mut self, rhs: Frame) {
		self.0 += rhs.0;
	}
}

/// A half-open range of frames, `[start, end)`.
///
/// Half-open semantics match the Oak engine's block model: a clip occupying
/// frames `[10, 20)` has length 10 and abuts a clip starting at frame 20 with
/// no overlap and no gap. An empty range (`start == end`) is legal and
/// represents zero duration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRange {
	/// First frame of the range (inclusive).
	pub start: Frame,
	/// End of the range (exclusive).
	pub end: Frame,
}

impl FrameRange {
	/// Creates a range from `start` (inclusive) to `end` (exclusive).
	///
	/// # Panics
	///
	/// Panics if `end < start`.
	pub fn new(start: Frame, end: Frame) -> Self {
		assert!(end >= start, "frame range end must not precede start");
		FrameRange { start, end }
	}

	/// The number of frames in the range. Zero for an empty range.
	pub fn len(&self) -> Frame {
		Frame(self.end.0 - self.start.0)
	}

	/// Whether the range contains no frames.
	pub fn is_empty(&self) -> bool {
		self.start == self.end
	}

	/// Whether `frame` lies inside the range (`start <= frame < end`).
	pub fn contains(&self, frame: Frame) -> bool {
		self.start <= frame && frame < self.end
	}

	/// Whether two ranges share at least one frame.
	///
	/// Half-open semantics: abutting ranges (`self.end == other.start`, or
	/// the reverse) do not overlap, and an empty range never overlaps
	/// anything. The check is symmetric in `self`/`other`.
	pub fn overlaps(&self, other: &FrameRange) -> bool {
		self.start < other.end && other.start < self.end
	}
}

/// How time should be presented to the user in rulers and inspectors.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeDisplay {
	/// `HH:MM:SS:FF` non-drop-frame timecode, the standard for integer frame
	/// rates (24, 25, 30, 50, 60) and the fallback for NTSC rates when
	/// wall-clock alignment is not required.
	///
	/// This is the default and the professional-video standard for integer
	/// rates.
	#[default]
	Timecode,
	/// `HH:MM:SS;FF` SMPTE drop-frame timecode for NTSC-derived rates
	/// (29.97, 23.976, 59.94, 119.88): frame numbers `00`/`01` (or `00`..`03`
	/// at 59.94/119.88) are skipped at the start of every minute except the
	/// tenth, keeping the timecode in lockstep with wall-clock time. For
	/// non-NTSC rates [`format_timecode`] falls back to non-drop-frame
	/// output. See [`format_timecode`] for the exact algorithm.
	TimecodeDropFrame,
	/// A plain frame counter, e.g. `1048576`.
	Frames,
	/// Seconds with millisecond precision, e.g. `83.708`.
	Seconds,
}

/// Converts a frame position to floating-point seconds at `rate`.
///
/// This is a pure, one-shot conversion: `frame * den / num`. It is exact for
/// any frame whose magnitude keeps `frame * den` within `f64`'s 53-bit
/// mantissa (far beyond any realistic sequence length).
///
/// # Examples
///
/// ```
/// # use oak_widgets::timeline::{Frame, FrameRate, frame_to_seconds};
/// let rate = FrameRate::new(30000, 1001);
/// // Frame 30 at ~29.97 fps is just over one second.
/// assert!((frame_to_seconds(Frame(30), rate) - 1.001).abs() < 1e-9);
/// ```
pub fn frame_to_seconds(frame: Frame, rate: FrameRate) -> f64 {
	frame.0 as f64 * rate.den as f64 / rate.num as f64
}

/// Converts floating-point seconds to the nearest frame at `rate`.
///
/// # Rounding policy
///
/// The result is rounded to the **nearest frame, ties away from zero**
/// (`f64::round` semantics). This makes [`seconds_to_frame`] and
/// [`frame_to_seconds`] approximate inverses for any input that is already
/// near a frame boundary, and it is deterministic across platforms.
///
/// This function exists for converting wall-clock durations (mouse drags
/// measured in seconds, playback elapsed time) into frames. It must never be
/// used to *store* time — convert once, then keep the [`Frame`].
///
/// # Examples
///
/// ```
/// # use oak_widgets::timeline::{Frame, FrameRate, seconds_to_frame};
/// let rate = FrameRate::new(24, 1);
/// assert_eq!(seconds_to_frame(rate, 1.0), Frame(24));
/// assert_eq!(seconds_to_frame(rate, 1.02), Frame(24)); // rounds to nearest
/// assert_eq!(seconds_to_frame(rate, 1.03), Frame(25));
/// ```
pub fn seconds_to_frame(rate: FrameRate, seconds: f64) -> Frame {
	Frame((seconds * rate.num as f64 / rate.den as f64).round() as i64)
}

/// Formats `frame` for display according to `display`.
///
/// * [`TimeDisplay::Frames`] — the raw frame number.
/// * [`TimeDisplay::Seconds`] — seconds with millisecond precision.
/// * [`TimeDisplay::Timecode`] — non-drop-frame `HH:MM:SS:FF`. The frame
///   component width derives from the frame rate (two digits for rates below
///   100 fps).
/// * [`TimeDisplay::TimecodeDropFrame`] — SMPTE drop-frame `HH:MM:SS;FF`
///   (semicolon separator). See below.
///
/// # Drop-frame timecode
///
/// NTSC-derived rates (29.97, 23.976, 59.94, 119.88 — any rate whose
/// [`FrameRate`] denominator is `1001`) run slightly slower than their
/// nominal integer rate, so non-drop-frame timecode drifts from wall-clock
/// time (≈3.6 s/hour at 29.97). Drop-frame timecode compensates by skipping
/// frame numbers at the start of every minute except the tenth — two frames
/// (`00`, `01`) per skipped minute at 29.97/23.976, four at 59.94, eight at
/// 119.88 — so the label stays within a frame of wall-clock time.
///
/// The conversion is the classic SMPTE algorithm:
///
/// 1. Split the real frame count into whole 10-minute blocks (`d`) and a
///    remainder (`m`). Each full block contributes `drop * 9` skipped frames
///    (every minute of the block except the tenth).
/// 2. Within the remainder, each complete minute contributes `drop` skipped
///    frames.
/// 3. Add the total skipped count to the real frame count, then format the
///    result at the nominal rate with `;` before the frame component.
///
/// Rates with a denominator other than `1001` have no drop-frame convention;
/// [`TimeDisplay::TimecodeDropFrame`] then falls back to non-drop-frame
/// output (the two variants produce identical strings).
///
/// Negative frames are formatted with a leading `-` applied to the whole
/// timecode (e.g. `-00:00:01;12`).
///
/// # Examples
///
/// ```
/// # use oak_widgets::timeline::{Frame, FrameRate, TimeDisplay, format_timecode};
/// let rate = FrameRate::new(24, 1);
/// assert_eq!(format_timecode(Frame(0), rate, TimeDisplay::Timecode), "00:00:00:00");
/// assert_eq!(
///     format_timecode(Frame(24 * 60 * 60 + 24 * 60 + 24 + 12), rate, TimeDisplay::Timecode),
///     "01:01:01:12",
/// );
/// assert_eq!(format_timecode(Frame(42), rate, TimeDisplay::Frames), "42");
///
/// // NTSC 29.97: frame 1800 is just past the first minute boundary, where
/// // frames 00 and 01 of the minute are skipped.
/// let ntsc = FrameRate::NTSC_2997;
/// assert_eq!(format_timecode(Frame(1800), ntsc, TimeDisplay::TimecodeDropFrame), "00:01:00;02");
/// ```
pub fn format_timecode(frame: Frame, rate: FrameRate, display: TimeDisplay) -> String {
	match display {
		TimeDisplay::Frames => frame.0.to_string(),
		TimeDisplay::Seconds => format!("{:.3}", frame_to_seconds(frame, rate)),
		TimeDisplay::Timecode => {
			let negative = frame.0 < 0;
			let mut n = frame.0.unsigned_abs();
			// Nominal (integer) frame count per second, matching the
			// non-drop-frame convention: NTSC 29.97 uses 30 frames/sec.
			let fps = rate.as_f64().round() as u64;
			let frames = n % fps;
			n /= fps;
			let seconds = n % 60;
			n /= 60;
			let minutes = n % 60;
			let hours = n / 60;
			format!(
				"{}{:02}:{:02}:{:02}:{:02}",
				if negative { "-" } else { "" },
				hours,
				minutes,
				seconds,
				frames
			)
		}
		TimeDisplay::TimecodeDropFrame => {
			if rate.den != 1001 {
				// There is no drop-frame convention outside NTSC-derived
				// rates; fall back to the ordinary non-drop string (including
				// its `:` separator) so the two variants agree.
				return format_timecode(frame, rate, TimeDisplay::Timecode);
			}
			let negative = frame.0 < 0;
			let n = frame.0.unsigned_abs();
			let (nominal, adjusted) = drop_frame_adjust(n, rate);
			let mut n = adjusted;
			let frames = n % nominal;
			n /= nominal;
			let seconds = n % 60;
			n /= 60;
			let minutes = n % 60;
			let hours = n / 60;
			format!(
				"{}{:02}:{:02}:{:02};{:02}",
				if negative { "-" } else { "" },
				hours,
				minutes,
				seconds,
				frames
			)
		}
	}
}

/// Applies the SMPTE drop-frame adjustment to a real frame count at an
/// NTSC-derived rate.
///
/// Returns `(nominal_fps, adjusted_count)`, where `adjusted_count` is the
/// timecode frame count with the skipped frame numbers re-inserted.
fn drop_frame_adjust(frame: u64, rate: FrameRate) -> (u64, u64) {
	debug_assert!(
		rate.den == 1001,
		"drop-frame adjustment is only defined for NTSC-derived rates (denominator 1001)"
	);
	let nominal = rate.as_f64().round() as u64;

	// Skipped frame numbers per non-10th minute: two per 30 fps of nominal
	// rate (2 at 29.97/23.976, 4 at 59.94, 8 at 119.88).
	let drop = ((nominal as f64) * 2.0 / 30.0).round() as u64;
	// Real frame counts per minute and per 10 minutes at this rate.
	let frames_per_10_min = (rate.as_f64() * 600.0).round() as u64;
	let frames_per_min = (rate.as_f64() * 60.0).round() as u64;

	let ten_minute_blocks = frame / frames_per_10_min;
	let within_block = frame % frames_per_10_min;

	let mut adjusted = frame + drop * 9 * ten_minute_blocks;
	if within_block > drop {
		adjusted += drop * ((within_block - drop) / frames_per_min);
	}
	(nominal, adjusted)
}

/// What produced a [`SnapPoint`]. Used by the UI to pick an indicator style
/// and by tests to assert snapping priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapKind {
	/// The start edge of a clip.
	ClipStart,
	/// The end edge of a clip.
	ClipEnd,
	/// The playhead.
	Playhead,
	/// An edge of the work area (in or out point).
	WorkAreaEdge,
	/// A user or chapter marker.
	Marker,
}

/// A frame position that dragged elements can snap to.
///
/// Snap points are gathered fresh on every drag move from the current
/// [`TimelineDataSource`](super::TimelineDataSource): clip edges, the
/// playhead, work-area edges and markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapPoint {
	/// The frame to snap to.
	pub frame: Frame,
	/// What this point represents.
	pub kind: SnapKind,
}

/// The outcome of a successful [`snap`] query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapResult {
	/// The snapped frame — equal to the winning [`SnapPoint`]'s frame.
	pub frame: Frame,
	/// The kind of the winning snap point.
	pub kind: SnapKind,
	/// On-screen distance between the drag position and the snap point, in
	/// pixels. Always `<=` the threshold passed to [`snap`]. Useful for
	/// fading the snap indicator as the cursor approaches.
	pub distance: Pixels,
}

/// Finds the best snap target for a dragged position, or `None` if nothing
/// is close enough.
///
/// # Contract
///
/// * `target` is the unsnapped frame position of the dragged edge or clip.
/// * `points` is evaluated lazily; the iterator may be cheap to reconstruct
///   per mouse-move, so implementations should consume it exactly once.
/// * `threshold_px` is the maximum on-screen distance at which snapping
///   engages, converted to frames internally via `zoom` (pixels per frame).
///   A threshold of `0` disables snapping.
/// * When several points are within range, the **nearest on screen** wins.
///   Ties are broken by the **earlier frame** first, then by [`SnapKind`]
///   priority — `Playhead` first, then `ClipStart`/`ClipEnd`, then
///   `WorkAreaEdge`, then `Marker` — so the behavior is deterministic
///   regardless of iterator order.
///
/// The returned frame is always exactly one of the provided snap points'
/// frames; this function never invents intermediate positions.
pub fn snap(
	target: Frame,
	points: impl Iterator<Item = SnapPoint>,
	threshold_px: Pixels,
	zoom: f32,
) -> Option<SnapResult> {
	if threshold_px.as_f32() <= 0.0 || zoom <= 0.0 {
		return None;
	}

	// Work in frame space for the distance comparison: a threshold given in
	// pixels is `threshold_px / zoom` frames at this zoom, and a point's
	// on-screen distance is `|point - target| * zoom`. Both quantities scale
	// by the same positive zoom, so ordering is preserved — we compare in
	// frame space and only convert the winning distance back to pixels.
	let threshold_frames = threshold_px.as_f32() / zoom;

	// Priority per [`SnapKind`] for deterministic tie-breaking: `Playhead`
	// first, then `ClipStart`/`ClipEnd`, then `WorkAreaEdge`, then `Marker`.
	let kind_rank = |kind: SnapKind| match kind {
		SnapKind::Playhead => 0,
		SnapKind::ClipStart | SnapKind::ClipEnd => 1,
		SnapKind::WorkAreaEdge => 2,
		SnapKind::Marker => 3,
	};

	// Best candidate, compared lexicographically: (screen distance in frames,
	// frame number, kind rank). Smaller is better on every component. The
	// frame-number tiebreak prefers the earlier snap point when two points are
	// equally close (so the result is always exactly one of the given frames,
	// never an interpolated position), and the kind rank breaks ties between
	// points sharing a frame; both keep the outcome independent of iterator
	// order.
	let mut best: Option<(f32, i64, u8)> = None;
	let mut best_kind = SnapKind::Marker;
	for point in points {
		let dist = (point.frame.0 - target.0).unsigned_abs() as f32;
		if dist > threshold_frames {
			continue;
		}
		let candidate = (dist, point.frame.0, kind_rank(point.kind));
		if best.map_or(true, |current| candidate < current) {
			best = Some(candidate);
			best_kind = point.kind;
		}
	}

	best.map(|(dist, frame, _)| SnapResult {
		frame: Frame(frame),
		kind: best_kind,
		distance: px(dist * zoom),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn frame_range_is_half_open() {
		let range = FrameRange::new(Frame(10), Frame(20));
		assert_eq!(range.len(), Frame(10));
		assert!(range.contains(Frame(10)));
		assert!(!range.contains(Frame(20)));
		assert!(range.overlaps(&FrameRange::new(Frame(19), Frame(30))));
		assert!(!range.overlaps(&FrameRange::new(Frame(20), Frame(30))));
	}

	#[test]
	fn seconds_round_trips_to_nearest_frame() {
		let rate = FrameRate::NTSC_2997;
		let frame = seconds_to_frame(rate, 10.0);
		assert!((frame_to_seconds(frame, rate) - 10.0).abs() < 0.02);
	}

	#[test]
	fn drop_frame_timecode_skips_minute_boundary_frames() {
		let rate = FrameRate::NTSC_2997;
		let tc = |frame| format_timecode(Frame(frame), rate, TimeDisplay::TimecodeDropFrame);
		// Within the first minute nothing is dropped.
		assert_eq!(tc(0), "00:00:00;00");
		assert_eq!(tc(30), "00:00:01;00");
		assert_eq!(tc(1798), "00:00:59;28");
		assert_eq!(tc(1799), "00:00:59;29");
		// Frame 1800 is just past the first minute boundary; frames 00 and 01
		// of that minute are skipped, so the label jumps to ...;02.
		assert_eq!(tc(1800), "00:01:00;02");
	}

	#[test]
	fn drop_frame_timecode_preserves_tenth_minute() {
		let rate = FrameRate::NTSC_2997;
		let tc = |frame| format_timecode(Frame(frame), rate, TimeDisplay::TimecodeDropFrame);
		// 10 minutes of real time at 29.97 is 17982 frames; no frames are
		// dropped at the start of the tenth minute.
		assert_eq!(tc(17982), "00:10:00;00");
		assert_eq!(tc(17984), "00:10:00;02");
		// 20 minutes: two 10-minute blocks.
		assert_eq!(tc(17982 * 2), "00:20:00;00");
	}

	#[test]
	fn drop_frame_timecode_aligns_with_wall_clock_at_one_hour() {
		// One real hour at 29.97 = 107892 frames; drop-frame timecode reads
		// exactly 01:00:00;00 (non-drop would read 01:00:02;12, the ~3.6s/h
		// drift the convention exists to cancel).
		let rate = FrameRate::NTSC_2997;
		assert_eq!(
			format_timecode(Frame(107892), rate, TimeDisplay::TimecodeDropFrame),
			"01:00:00;00"
		);
	}

	#[test]
	fn drop_frame_timecode_at_59_94_drops_four_frames() {
		let rate = FrameRate::new(60000, 1001);
		let tc = |frame| format_timecode(Frame(frame), rate, TimeDisplay::TimecodeDropFrame);
		assert_eq!(tc(0), "00:00:00;00");
		// Real frames per minute at 59.94: round(3596.4) = 3596. Just past
		// the first minute the four skipped numbers (00..03) are visible.
		assert_eq!(tc(3596), "00:00:59;56");
		assert_eq!(tc(3600), "00:01:00;04");
		// One real hour at 59.94 = round(59.94 * 3600) = 215784 frames.
		assert_eq!(tc(215784), "01:00:00;00");
	}

	#[test]
	fn drop_frame_timecode_falls_back_for_non_ntsc_rates() {
		// 24 fps is not NTSC-derived (denominator 1): drop-frame output must
		// be identical to non-drop output.
		let rate = FrameRate::new(24, 1);
		let frame = Frame(24 * 3600 + 24 * 60 + 24 + 12);
		assert_eq!(
			format_timecode(frame, rate, TimeDisplay::TimecodeDropFrame),
			format_timecode(frame, rate, TimeDisplay::Timecode),
		);
		assert_eq!(
			format_timecode(frame, rate, TimeDisplay::TimecodeDropFrame),
			"01:01:01:12",
		);
	}

	#[test]
	fn drop_frame_timecode_handles_negative_frames() {
		let rate = FrameRate::NTSC_2997;
		assert_eq!(
			format_timecode(Frame(-1800), rate, TimeDisplay::TimecodeDropFrame),
			"-00:01:00;02",
		);
	}

	#[test]
	fn snap_disabled_by_zero_threshold() {
		let points = [SnapPoint {
			frame: Frame(10),
			kind: SnapKind::Playhead,
		}];
		assert!(snap(Frame(12), points.into_iter(), px(0.0), 1.0).is_none());
	}

	#[test]
	fn snap_requires_positive_zoom() {
		let points = [SnapPoint {
			frame: Frame(10),
			kind: SnapKind::Playhead,
		}];
		assert!(snap(Frame(10), points.into_iter(), px(10.0), 0.0).is_none());
	}

	#[test]
	fn snap_returns_none_when_nothing_is_within_range() {
		let points = [SnapPoint {
			frame: Frame(100),
			kind: SnapKind::Playhead,
		}];
		// At zoom 1.0 a 5-px threshold is 5 frames; target 92 is 8 frames away.
		assert!(snap(Frame(92), points.into_iter(), px(5.0), 1.0).is_none());
	}

	#[test]
	fn snap_prefers_the_nearest_point() {
		let points = [
			SnapPoint {
				frame: Frame(90),
				kind: SnapKind::Marker,
			},
			SnapPoint {
				frame: Frame(95),
				kind: SnapKind::Marker,
			},
		];
		let result = snap(Frame(92), points.into_iter(), px(50.0), 1.0).unwrap();
		assert_eq!(result.frame, Frame(90));
		assert_eq!(result.kind, SnapKind::Marker);
		assert_eq!(result.distance, px(2.0));
	}

	#[test]
	fn snap_ties_break_by_kind_priority_regardless_of_iteration_order() {
		let priority_points = [
			SnapPoint {
				frame: Frame(100),
				kind: SnapKind::Playhead,
			},
			SnapPoint {
				frame: Frame(100),
				kind: SnapKind::ClipStart,
			},
		];
		// Equal distance; higher-priority kind wins even though it appears
		// first in the iterator.
		let result = snap(Frame(100), priority_points.into_iter(), px(10.0), 1.0).unwrap();
		assert_eq!(result.kind, SnapKind::Playhead);
		assert_eq!(result.frame, Frame(100));

		// Reversed iteration order changes nothing.
		let reversed = [
			SnapPoint {
				frame: Frame(100),
				kind: SnapKind::ClipStart,
			},
			SnapPoint {
				frame: Frame(100),
				kind: SnapKind::Playhead,
			},
		];
		let result = snap(Frame(100), reversed.into_iter(), px(10.0), 1.0).unwrap();
		assert_eq!(result.kind, SnapKind::Playhead);
	}

	#[test]
	fn snap_result_frame_is_always_a_snap_point_frame() {
		// Two points at equal distance on opposite sides; the earlier frame
		// wins via the frame-number tiebreak, never an interpolated position.
		let points = [
			SnapPoint {
				frame: Frame(98),
				kind: SnapKind::WorkAreaEdge,
			},
			SnapPoint {
				frame: Frame(102),
				kind: SnapKind::ClipEnd,
			},
		];
		let result = snap(Frame(100), points.into_iter(), px(10.0), 1.0).unwrap();
		assert_eq!(result.frame, Frame(98));
		assert_eq!(result.distance, px(2.0));
	}

	#[test]
	fn snap_threshold_scales_with_zoom() {
		// At zoom 2.0 the same 10-px threshold covers only 5 frames.
		let points = [SnapPoint {
			frame: Frame(50),
			kind: SnapKind::ClipEnd,
		}];
		assert!(snap(Frame(56), points.into_iter(), px(10.0), 2.0).is_none());
		let result = snap(Frame(55), points.into_iter(), px(10.0), 2.0).unwrap();
		assert_eq!(result.frame, Frame(50));
		assert_eq!(result.distance, px(10.0));
	}
}

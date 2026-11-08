// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! App-level time display: timecode, duration, frame-rate and resolution
//! labels for the viewers, the timeline and the status bar.
//!
//! Canonical time stays in frames ([`Frame`]); formatting to human-readable
//! text happens only here, at the display edge. This module is pure Rust
//! (only the gpui *types* [`Frame`]/[`FrameRate`] are used) so it is fully
//! unit-testable.
//!
//! # Timecode convention
//!
//! `HH:MM:SS:FF`, non-drop-frame, matching `crate::widgets::timeline::format_timecode`
//! and Oak's existing viewer widgets. Drop-frame timecode for NTSC rates is
//! future work (see `crate::widgets::timeline::time`).

use crate::widgets::timeline::{Frame, FrameRate};

/// Formats `frame` as non-drop-frame timecode `HH:MM:SS:FF`.
///
/// The frame component uses two digits for rates below 100 fps (the nominal
/// integer fps of the rate — 25 for 25/1, 30 for 30000/1001), matching the
/// broadcast non-drop-frame convention. Negative frames get a leading `-`.
///
/// # Examples
///
/// ```text
/// format_timecode(Frame(0), 25fps)    → "00:00:00:00"
/// format_timecode(Frame(25), 25fps)   → "00:00:01:00"
/// format_timecode(Frame(6468), 25fps) → "00:04:18:18"
/// ```
pub fn format_timecode(frame: Frame, rate: FrameRate) -> String {
	let negative = frame.0 < 0;
	let mut n = frame.0.unsigned_abs();
	let fps = nominal_fps(rate);
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

/// Formats a duration (a number of frames) as `HH:MM:SS:FF`.
///
/// This is [`format_timecode`] under a duration-shaped name, so call sites
/// read as what they mean (the status bar shows "timecode / duration").
pub fn format_duration(frames: Frame, rate: FrameRate) -> String {
	format_timecode(frames, rate)
}

/// Formats a frame rate as its conventional label: `25` for whole rates,
/// `29.97` for NTSC 30000/1001.
///
/// # Examples
///
/// ```text
/// format_fps(FrameRate::new(25, 1))   → "25"
/// format_fps(FrameRate::NTSC_2997)    → "29.97"
/// ```
pub fn format_fps(rate: FrameRate) -> String {
	let fps = rate.num as f64 / rate.den as f64;
	if (fps - fps.round()).abs() < 1e-9 {
		format!("{:.0}", fps)
	} else {
		format!("{:.2}", fps)
	}
}

/// Formats a resolution as `WIDTH×HEIGHT` using U+00D7 (`×`), e.g. `1920×1080`.
///
/// `width`/`height` are pixel dimensions and are formatted verbatim — no rounding,
/// validation, or unit suffix — so callers pass already display-ready values. Used
/// for the viewer and status-bar resolution chips; pairs with [`format_fps`] in the
/// same overlays.
pub fn format_resolution(width: u32, height: u32) -> String {
	format!("{width}×{height}")
}

/// The nominal integer frames-per-second used by non-drop-frame timecode:
/// the rounded frame rate (`25` for 25/1, `30` for 30000/1001).
fn nominal_fps(rate: FrameRate) -> u64 {
	rate.as_f64().round().max(1.0) as u64
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn zero_is_zeros() {
		assert_eq!(
			format_timecode(Frame(0), FrameRate::new(25, 1)),
			"00:00:00:00"
		);
	}

	#[test]
	fn whole_seconds_at_25fps() {
		let rate = FrameRate::new(25, 1);
		assert_eq!(format_timecode(Frame(25), rate), "00:00:01:00");
		assert_eq!(format_timecode(Frame(60 * 25), rate), "00:01:00:00");
		assert_eq!(format_timecode(Frame(3600 * 25), rate), "01:00:00:00");
	}

	#[test]
	fn design_reference_duration() {
		// The design's sequence duration chip: 00:04:18:18 @ 25 fps.
		assert_eq!(
			format_timecode(Frame(6468), FrameRate::new(25, 1)),
			"00:04:18:18"
		);
	}

	#[test]
	fn fractional_frame_component() {
		// Frame 23 of the 24th second: 00:00:23:23 @ 25 fps.
		let rate = FrameRate::new(25, 1);
		assert_eq!(format_timecode(Frame(23 * 25 + 23), rate), "00:00:23:23");
	}

	#[test]
	fn negative_frames_get_a_leading_minus() {
		let rate = FrameRate::new(25, 1);
		assert_eq!(format_timecode(Frame(-25), rate), "-00:00:01:00");
	}

	#[test]
	fn ntsc_uses_nominal_30fps_frames() {
		// At 30000/1001 the nominal rate is 30, so one second is frame 30.
		let rate = FrameRate::NTSC_2997;
		assert_eq!(format_timecode(Frame(30), rate), "00:00:01:00");
	}

	#[test]
	fn duration_aliases_timecode() {
		let rate = FrameRate::new(25, 1);
		assert_eq!(
			format_duration(Frame(6468), rate),
			format_timecode(Frame(6468), rate)
		);
	}

	#[test]
	fn fps_labels() {
		assert_eq!(format_fps(FrameRate::new(25, 1)), "25");
		assert_eq!(format_fps(FrameRate::new(30, 1)), "30");
		assert_eq!(format_fps(FrameRate::NTSC_2997), "29.97");
		assert_eq!(format_fps(FrameRate::NTSC_23976), "23.98");
	}

	#[test]
	fn resolution_labels() {
		assert_eq!(format_resolution(1920, 1080), "1920×1080");
		assert_eq!(format_resolution(1280, 720), "1280×720");
	}
}

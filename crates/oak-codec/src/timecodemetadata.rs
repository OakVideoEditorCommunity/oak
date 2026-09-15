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

//! `olive::TimecodeMetadata` — parse source timecode strings.
//!
//! Mirrors `src/codec/src/timecodemetadata.h`: parse an SMPTE timecode
//! string or a BWF `time reference` chunk into a rational media timestamp.

use oak_core::Rational;

/// `TimecodeMetadata::SourceTime` — the parsed result.
pub struct SourceTime {
	/// Media timestamp, rational seconds.
	pub time: Rational,
	/// Source string (normalized form, or the raw input on failure).
	pub source: String,
	/// Parse succeeded.
	pub valid: bool,
}

/// Trim the same whitespace set as the C++ `trimmed()` helper
/// (`" \t\n\r\f\v"`): space, tab, LF, CR, form feed, vertical tab.
fn trimmed(s: &str) -> String {
	s.trim_matches(|c: char| {
		c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\u{000c}' || c == '\u{000b}'
	})
	.to_string()
}

/// Port of `str_to_int64_empty_tolerant`: an empty field is a valid `0`,
/// any other unparseable field is an error (`None`).
fn str_to_int64_empty_tolerant(s: &str) -> Option<i64> {
	if s.is_empty() {
		Some(0)
	} else {
		s.parse::<i64>().ok()
	}
}

/// `std::llround` (round half away from zero, truncate to `i64`).
fn llround(x: f64) -> i64 {
	x.round() as i64
}

/// `timebase.flipped().to_double()` — seconds-per-frame to frames-per-second.
fn frame_rate(timebase: &Rational) -> f64 {
	timebase.denominator() as f64 / timebase.numerator() as f64
}

/// Port of `olive::core::Timecode::timecode_to_time` for the
/// `k_timecode_non_drop_frame` / `k_timecode_drop_frame` displays.
///
/// `drop_frame` is true when the (already trimmed) string contains a `;`.
/// Returns `None` on any parse failure.
fn timecode_to_time(timecode: &str, timebase: &Rational, drop_frame: bool) -> Option<Rational> {
	let mut tokens: Vec<&str> = timecode.split([':', ';']).collect();
	let element_count = 4;

	// Keep only the leading `HH:MM:SS:FF` tokens.
	if tokens.len() > element_count {
		tokens.truncate(element_count);
	}
	// Pad missing leading fields with empty strings (which parse to 0).
	while tokens.len() < element_count {
		tokens.insert(0, "");
	}

	let negative = timecode.starts_with('-');

	let hours = str_to_int64_empty_tolerant(tokens[0])?;
	let mins = str_to_int64_empty_tolerant(tokens[1])?;
	let secs = str_to_int64_empty_tolerant(tokens[2])?;
	let frames = str_to_int64_empty_tolerant(tokens[3])?;

	let fr = frame_rate(timebase);
	let rounded_frame_rate = llround(fr);

	let sec_count = hours * 3600 + mins * 60 + secs;
	let mut frame_count = sec_count * rounded_frame_rate + frames;

	if drop_frame && timebase.numerator() != 1 {
		// `timebase_is_drop_frame(timebase)`: numerator != 1.
		// Number of frames dropped on the minute marks ≈ 6% of the framerate.
		let drop_frames = llround(fr * (2.0 / 30.0));

		// `d` and `m` are derived from the real (non-rounded) framerate.
		let real_fr_ts = llround(sec_count as f64 * fr) + frames;

		let frames_per10_minutes = llround(fr * 600.0);
		let d = real_fr_ts / frames_per10_minutes;
		let m = real_fr_ts % frames_per10_minutes;

		if m > drop_frames {
			frame_count -= drop_frames * ((m - drop_frames) / (llround(fr) * 60 - drop_frames));
		}
		frame_count -= drop_frames * 9 * d;
	}

	// `timestamp_to_time`: `timebase.num * frame_count / timebase.den`, reduced.
	let mut time = timebase.timestamp_to_time(frame_count);

	if negative {
		time = time * Rational::new(-1, 1);
	}

	Some(time)
}

/// Signed Euclidean GCD on absolute values (mirrors `i64_gcd`).
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
	while b != 0 {
		let t = a % b;
		a = b;
		b = t;
	}
	a
}

impl SourceTime {
	/// New invalid (empty) source time.
	pub fn invalid() -> Self {
		SourceTime {
			time: Rational::NULL,
			source: String::new(),
			valid: false,
		}
	}

	/// Parse an SMPTE timecode string at the given timebase
	/// (`from_timecode_string`).
	///
	/// The string is trimmed; an empty result is invalid. A `;` separator
	/// selects drop-frame, otherwise non-drop-frame. On any parse failure
	/// the result is invalid and `source` holds the trimmed raw input.
	pub fn from_timecode_string(timecode: &str, timebase: &Rational) -> SourceTime {
		let trimmed_tc = trimmed(timecode);
		if trimmed_tc.is_empty() {
			return SourceTime::invalid();
		}

		let drop_frame = trimmed_tc.contains(';');
		match timecode_to_time(&trimmed_tc, timebase, drop_frame) {
			Some(time) => SourceTime {
				time,
				source: "timecode".to_string(),
				valid: true,
			},
			None => SourceTime {
				time: Rational::NULL,
				source: trimmed_tc,
				valid: false,
			},
		}
	}

	/// Parse a BWF `time reference` chunk into a timestamp
	/// (`from_bwf_time_reference`).
	///
	/// A non-positive `sample_rate`, or a string that is not a single
	/// base-10 unsigned integer, yields an invalid result. The parsed
	/// sample count over `sample_rate` is reduced by their GCD; if the
	/// reduced numerator or denominator exceed `i32::MAX` the value falls
	/// back to the (capped, reduced) `Rational::new(samples, sample_rate)`
	/// approximation, since `oak_core::Rational` exposes no `from_double`.
	pub fn from_bwf_time_reference(time_reference: &str, sample_rate: i32) -> SourceTime {
		if sample_rate <= 0 {
			return SourceTime::invalid();
		}

		let trimmed_ref = trimmed(time_reference);
		// `std::strtoull` base 10 with a "whole string consumed" check:
		// at least one digit, no leading/trailing junk.
		let samples: u64 = match trimmed_ref.parse() {
			Ok(v) => v,
			Err(_) => return SourceTime::invalid(),
		};

		let mut numerator = samples;
		let mut denominator = sample_rate as u64;
		let divisor = gcd_u64(numerator, denominator);
		numerator /= divisor;
		denominator /= divisor;

		let rational_limit = i32::MAX as u64;
		let time = if numerator <= rational_limit && denominator <= rational_limit {
			Rational::new(numerator as i64, denominator as i64)
		} else {
			// `Rational::from_double` is not part of the oakcore_rs public
			// API; `Rational::new` applies the same INT_MAX-capped reduction
			// (FFmpeg `av_reduce`), which is the intended approximation.
			let n = i64::try_from(samples).unwrap_or(i64::MAX);
			Rational::new(n, sample_rate as i64)
		};

		SourceTime {
			time,
			source: "bwf_time_reference".to_string(),
			valid: true,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn tc(s: &str, timebase: &Rational) -> SourceTime {
		SourceTime::from_timecode_string(s, timebase)
	}

	#[test]
	fn invalid_is_null() {
		let inv = SourceTime::invalid();
		assert!(inv.time.is_null());
		assert!(inv.source.is_empty());
		assert!(!inv.valid);
	}

	#[test]
	fn empty_or_whitespace_is_invalid() {
		let tb = Rational::new(1, 25);
		assert!(!tc("", &tb).valid);
		assert!(!tc("   \t\n", &tb).valid);
	}

	#[test]
	fn non_drop_integral_fps() {
		let tb = Rational::new(1, 25);
		let r = tc("00:00:00:10", &tb);
		assert!(r.valid);
		assert_eq!(r.source, "timecode");
		assert_eq!(r.time, Rational::new(2, 5)); // 10 frames @ 25fps = 0.4s
	}

	#[test]
	fn non_drop_leading_zeros_and_whitespace() {
		let tb = Rational::new(1, 25);
		let r = tc("  01:02:03:04  ", &tb);
		assert!(r.valid);
		// 1*3600+2*60+3 = 3723s * 25 + 4 frames = 93079 frames
		assert_eq!(r.time, Rational::new(93079, 25));
	}

	#[test]
	fn missing_leading_fields_pad_from_front() {
		let tb = Rational::new(1, 25);
		let r = tc("00:01:02", &tb);
		assert!(r.valid);
		// C++ pads missing leading fields at the front, so 3 fields become
		// ["",00,01,02] = HH=0, MM=0, SS=1, FF=2 -> 27 frames @25fps.
		assert_eq!(r.time, Rational::new(27, 25));
	}

	#[test]
	fn negative_timecode_is_negated() {
		let tb = Rational::new(1, 25);
		let r = tc("-00:00:00:05", &tb);
		assert!(r.valid);
		assert_eq!(r.time, Rational::new(-1, 5));
	}

	#[test]
	fn parse_failure_is_invalid_with_raw_source() {
		let tb = Rational::new(1, 25);
		let r = tc("abc:def", &tb);
		assert!(!r.valid);
		assert!(r.time.is_null());
		assert_eq!(r.source, "abc:def");
	}

	#[test]
	fn drop_frame_29_97_first_minute_no_correction() {
		// 29.97fps -> timebase 1001/30000.
		let tb = Rational::new(1001, 30000);
		let r = tc("00:01:00;00", &tb);
		assert!(r.valid);
		// No frames dropped in the first minute: `frame_count` stays at
		// 60s * 30fps = 1800 frames (a minute boundary at 29.97 is 60.06s).
		assert_eq!(r.time, tb.timestamp_to_time(1800));
	}

	#[test]
	fn drop_frame_29_97_later_minute_corrects() {
		let tb = Rational::new(1001, 30000);
		// At 10 minutes of drop-frame timecode the running correction is
		// 2 frames dropped per minute for 9 of the 10 minutes (18 frames).
		let r = tc("00:10:00;00", &tb);
		assert!(r.valid);
		// NDF would be 10*60*30 = 18000 frames; 18 dropped -> 17982 frames.
		assert_eq!(r.time, tb.timestamp_to_time(17982));
	}

	#[test]
	fn non_drop_uses_colon() {
		let tb = Rational::new(1001, 30000);
		let r = tc("00:10:00:00", &tb);
		assert!(r.valid);
		// Non-drop: 18000 frames, no correction.
		assert_eq!(r.time, tb.timestamp_to_time(18000));
	}

	#[test]
	fn bwf_valid_reduces() {
		let r = SourceTime::from_bwf_time_reference("48000", 48000);
		assert!(r.valid);
		assert_eq!(r.source, "bwf_time_reference");
		assert_eq!(r.time, Rational::new(1, 1));
	}

	#[test]
	fn bwf_valid_whitespace_trimmed() {
		let r = SourceTime::from_bwf_time_reference("  24000  ", 48000);
		assert!(r.valid);
		assert_eq!(r.time, Rational::new(1, 2));
	}

	#[test]
	fn bwf_bad_sample_rate_is_invalid() {
		for sr in [0, -1] {
			let r = SourceTime::from_bwf_time_reference("100", sr);
			assert!(!r.valid);
			assert!(r.time.is_null());
		}
	}

	#[test]
	fn bwf_unparseable_is_invalid() {
		for s in ["", "   ", "abc", "12x", "1.5", "-5", "1 2"] {
			let r = SourceTime::from_bwf_time_reference(s, 48000);
			assert!(!r.valid, "should reject {:?}", s);
			assert!(r.time.is_null());
		}
	}

	#[test]
	fn bwf_no_common_divisor() {
		let r = SourceTime::from_bwf_time_reference("3", 48000);
		assert!(r.valid);
		assert_eq!(r.time, Rational::new(1, 16000));
	}
}

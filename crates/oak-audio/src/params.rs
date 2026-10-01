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

//! Audio value types: `AudioParams` and the `SampleFormat` enum
//! (re-exported from oakcore-rs; planar-first ordering, values identical
//! to `olive::core::SampleFormat::Format` — `// CPP-PARITY:
//! core/include/olive/core/render/sampleformat.h:33`).
//!
//! These integer values cross the C ABI as `int`, so they MUST match the
//! authoritative C++ enums bit-for-bit.

use oak_core::Rational;

/// Audio sample format: re-exported from oakcore-rs (planar-first,
/// values identical to `olive::core::SampleFormat::Format`).
/// `// CPP-PARITY: core/include/olive/core/render/sampleformat.h:33`
pub use oak_core::SampleFormat;

/// Audio stream parameters, mirroring `olive::core::AudioParams`
/// (core/include/olive/core/render/audioparams.h). A plain value type;
/// never bridged through a C ABI handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioParams {
	/// Sample rate in Hz.
	pub sample_rate: i32,
	/// ffmpeg-style channel layout mask (0 = unknown/unspecified).
	pub channel_layout: u64,
	/// Sample format (see [`SampleFormat`]).
	pub format: SampleFormat,
}

impl AudioParams {
	/// Channel count of the current layout mask (popcount).
	///
	/// `// CPP-PARITY: core/src/render/audioparams.cpp:150`
	/// (`AudioParams::calculate_channel_count` —
	/// `channel_layout_mask_channel_count`); a layout mask of 0 yields 0,
	/// NOT the stereo fallback (the fallback lives in the processor's
	/// `fix_channel_layout`).
	pub fn channel_count(&self) -> i32 {
		self.channel_layout.count_ones() as i32
	}

	/// Bytes per sample per channel for the format.
	///
	/// `// CPP-PARITY: core/src/render/audioparams.cpp`
	/// (`AudioParams::bytes_per_sample_per_channel`).
	pub fn bytes_per_sample_per_channel(&self) -> i64 {
		self.format.bytes_per_sample() as i64
	}

	/// Byte count of `samples` frames across all channels.
	///
	/// `// CPP-PARITY: core/src/render/audioparams.cpp:93`
	/// (`AudioParams::samples_to_bytes`).
	pub fn samples_to_bytes(&self, samples: i64) -> i64 {
		samples * self.bytes_per_sample_per_channel() * i64::from(self.channel_count())
	}
}

/// Rebuild a [`SampleFormat`] from the `int` that crossed the C ABI.
///
/// `// CPP-PARITY: src/audio/c_api/manager.cpp:130` — the C++ layers cast
/// the raw `int` straight onto `SampleFormat::Format`, which is UB for
/// out-of-range values but in practice wraps to whatever the enum width
/// holds. The Rust side maps unknown values to `Invalid` (a safe
/// equivalent; no contract test pins the wrapped value).
pub fn sample_format_from_i32(value: i32) -> SampleFormat {
	match value {
		0 => SampleFormat::U8Planar,
		1 => SampleFormat::S16Planar,
		2 => SampleFormat::S32Planar,
		3 => SampleFormat::S64Planar,
		4 => SampleFormat::F32Planar,
		5 => SampleFormat::F64Planar,
		6 => SampleFormat::U8,
		7 => SampleFormat::S16,
		8 => SampleFormat::S32,
		9 => SampleFormat::S64,
		10 => SampleFormat::F32,
		11 => SampleFormat::F64,
		_ => SampleFormat::Invalid,
	}
}

/// Seconds elapsed at `frames` frames given `sample_rate`
/// (`frames/sample_rate` as a rational).
pub fn frames_to_rational(frames: i64, sample_rate: i32) -> Rational {
	Rational::new(frames, i64::from(sample_rate))
}

/// Sample index of `time` seconds at `sample_rate` (rounded to nearest,
/// half away from zero).
///
/// `// CPP-PARITY: core/src/render/audioparams.cpp:81`
/// (`AudioParams::time_to_samples` uses `std::round`, not truncation).
pub fn rational_to_samples(time: Rational, sample_rate: i32) -> i64 {
	(time.to_f64() * f64::from(sample_rate)).round() as i64
}

/// Continued-fraction double → rational conversion.
///
/// `// CPP-PARITY: core/src/util/rational.cpp:39`
/// (`Rational::from_double`): NaN/out-of-range → the null sentinel (0/0);
/// tiny values retried at INT64_MAX precision.
pub fn rational_from_double(flt: f64) -> Rational {
	if flt.is_nan() {
		return Rational::NULL;
	}
	if flt.abs() > f64::from(i32::MAX) + 3.0 {
		return Rational::NULL;
	}

	// frexp: flt = f * 2^exp with f in [0.5, 1)
	let (mut exponent, _frac) = {
		if flt == 0.0 {
			(0i32, 0.0)
		} else {
			let bits = flt.abs().to_bits();
			let e = (((bits >> 52) & 0x7ff) as i32) - 1022;
			(e, flt)
		}
	};
	exponent = (exponent - 1).max(0);
	let den: i64 = 1i64 << (62 - exponent);
	let num: i64 = (flt * den as f64 + 0.5).floor() as i64;

	let mut r = Rational::new(num, den);
	if r.is_null() && flt != 0.0 {
		// Too small to represent above; retry with maximum precision.
		r = Rational::new((flt * i64::MAX as f64) as i64, i64::MAX);
	}
	r
}

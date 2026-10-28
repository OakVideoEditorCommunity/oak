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

//! AudioVisualWaveform contract tests (waveform.rs), calling the public
//! Rust API.

mod common;

use std::ffi::CString;

use common::write_wav;
use oak_core::Rational;
use oak_audio::waveform::{extract, AudioVisualWaveform, SamplePerChannel};

/// Fill `w` with 100 samples/channel of a 0..0.99 ramp at 100 Hz (1 s).
fn fill_ramp(w: &mut AudioVisualWaveform) {
	let ch0: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
	let ch1: Vec<f32> = (0..100).map(|i| -(i as f32) * 0.01).collect();
	let planes = [ch0.as_slice(), ch1.as_slice()];
	w.set_channel_count(2);
	w.overwrite_samples(&planes, 100, Rational::new(0, 1));
}

/// A one-point-per-channel summary over `start..start+length`.
fn summary(w: &AudioVisualWaveform, start: Rational, length: Rational) -> Vec<SamplePerChannel> {
	w.get_summary_from_time(start, length)
}

/// set_channel_count then overwrite_samples writes planar data at the given
/// start; length() reflects the covered span and get_summary returns
/// channel-interleaved min/max pairs.
#[test]
fn overwrite_samples_and_length() {
	let mut w = AudioVisualWaveform::new();
	fill_ramp(&mut w);

	assert_eq!(w.length(), Rational::new(1, 1));

	let out = summary(&w, Rational::new(0, 1), Rational::new(1, 1));
	assert_eq!(out.len(), 2);
	assert_eq!(out[0].min, 0.0);
	assert!((out[0].max - 0.99).abs() < 1e-5, "max = {}", out[0].max);
	assert!((out[1].min + 0.99).abs() < 1e-5, "min = {}", out[1].min);
	assert_eq!(out[1].max, 0.0);
}

/// overwrite_sums copies channel-interleaved pairs from another waveform
/// into a dest range; a null length copies all of src.
#[test]
fn overwrite_sums_range_copy() {
	let mut src = AudioVisualWaveform::new();
	fill_ramp(&mut src);

	let mut dst = AudioVisualWaveform::new();
	dst.set_channel_count(2);
	dst.overwrite_sums(&src, Rational::new(0, 1), Rational::new(0, 1), Rational::NULL);
	// A null length means "copy everything": dst matches src exactly.
	let out = summary(&dst, Rational::new(0, 1), Rational::new(1, 1));
	assert_eq!(out.len(), 2);
	assert_eq!(out[0].min, 0.0);
	assert!((out[0].max - 0.99).abs() < 1e-5, "max = {}", out[0].max);
	assert!((out[1].min + 0.99).abs() < 1e-5, "min = {}", out[1].min);
	assert_eq!(out[1].max, 0.0);
	assert_eq!(dst.length(), Rational::new(1, 1));
}

/// overwrite_silence zeroes min/max over a range without changing length.
#[test]
fn overwrite_silence() {
	let mut w = AudioVisualWaveform::new();
	fill_ramp(&mut w);
	w.overwrite_silence(Rational::new(0, 1), Rational::new(1, 2));

	// First half is silenced; second half retains the ramp data.
	let out = summary(&w, Rational::new(0, 1), Rational::new(1, 2));
	assert_eq!((out[0].min, out[0].max), (0.0, 0.0));
	assert_eq!((out[1].min, out[1].max), (0.0, 0.0));

	let out = summary(&w, Rational::new(1, 2), Rational::new(1, 2));
	assert!(
		out[0].max > 0.5,
		"second half must keep ramp data, got {:?}",
		out[0]
	);
	assert!(out[1].min < -0.5);

	assert_eq!(
		w.length(),
		Rational::new(1, 1),
		"overwrite_silence must not change length"
	);
}

/// trim_in/trim_range/resize adjust length and drop or pad data; a negative
/// trim_in prepends silence (C++ semantics).
#[test]
fn trim_and_resize() {
	let mut w = AudioVisualWaveform::new();
	fill_ramp(&mut w);

	// Negative trim_in prepends silence: absolute end (length) unchanged.
	w.trim_in(Rational::new(-1, 2));
	assert_eq!(w.length(), Rational::new(1, 1));

	// Resize extends to 2 s.
	w.resize(Rational::new(2, 1));
	assert_eq!(w.length(), Rational::new(2, 1));

	// trim_range keeps 0.5 s from the (prepended) start.
	w.trim_range(Rational::new(0, 1), Rational::new(1, 2));
	assert_eq!(w.length(), Rational::new(1, 2));
}

/// sum_samples reduces planar samples into one min/max pair per channel;
/// re_sum_samples merges channel-interleaved entries into one pair per
/// channel. Both match golden vectors from the C++ implementation.
#[test]
fn sum_and_resum_golden() {
	let ch0 = [1.0f32, -2.0, 3.0];
	let ch1 = [4.0f32, -5.0, 6.0];
	let planes = [ch0.as_slice(), ch1.as_slice()];
	let out = AudioVisualWaveform::sum_samples(&planes, 0, 3);
	assert_eq!((out[0].min, out[0].max), (-2.0, 3.0));
	assert_eq!((out[1].min, out[1].max), (-5.0, 6.0));

	// re_sum_samples over 4 interleaved entries, 2 channels -> one pair per
	// channel merging both points.
	let input = [
		SamplePerChannel { min: 1.0, max: 2.0 },
		SamplePerChannel { min: 3.0, max: 4.0 },
		SamplePerChannel { min: 5.0, max: 6.0 },
		SamplePerChannel { min: 7.0, max: 8.0 },
	];
	let out = AudioVisualWaveform::re_sum_samples(&input, 4, 2);
	assert_eq!((out[0].min, out[0].max), (1.0, 6.0));
	assert_eq!((out[1].min, out[1].max), (3.0, 8.0));
}

/// extract probes through oakcodec's decoder registry; a missing file
/// returns NotFound. The valid-file path decodes the fixture WAV with
/// oakcodec's in-process FFmpeg decoder and reduces it to min/max points.
#[test]
fn extract_file_and_notfound() {
	let path = std::env::temp_dir().join(format!("oakaudio_extract_{}.wav", std::process::id()));
	// 8 frames of stereo ramp, 48000 Hz.
	let mut samples = Vec::with_capacity(16);
	for i in 0..8i16 {
		samples.push(i * 1000);
		samples.push(-(i * 1000));
	}
	write_wav(&path, 2, 48000, &samples).unwrap();

	// Missing file -> NotFound.
	let missing = std::env::temp_dir().join(format!("oakaudio_missing_{}.wav", std::process::id()));
	let _ = std::fs::remove_file(&missing);
	let missing_c = CString::new(missing.to_str().unwrap()).unwrap();
	assert!(extract(&missing_c, 0, 4).is_err());

	// Real decode: 8 frames at 4 samples/point -> 2 points, 2 channels.
	let c_path = CString::new(path.to_str().unwrap()).unwrap();
	let outcome = extract(&c_path, 0, 4).unwrap();
	assert_eq!(outcome.channels, 2);
	assert_eq!(outcome.sample_rate, 48_000);
	assert_eq!(outcome.points.len(), 4, "2 points x 2 channels");
	let out = &outcome.points;

	// s16 -> f32 is /32768; the ramp is exact in both formats.
	let eps = 1e-6;
	// Point 0 covers frames 0..4: left 0..3000, right 0..-3000.
	assert!((out[0].min - 0.0).abs() < eps);
	assert!((out[0].max - 3000.0 / 32768.0).abs() < eps);
	assert!((out[1].min - -3000.0 / 32768.0).abs() < eps);
	assert!((out[1].max - 0.0).abs() < eps);
	// Point 1 covers frames 4..8: left 4000..7000, right -4000..-7000.
	assert!((out[2].min - 4000.0 / 32768.0).abs() < eps);
	assert!((out[2].max - 7000.0 / 32768.0).abs() < eps);
	assert!((out[3].min - -7000.0 / 32768.0).abs() < eps);
	assert!((out[3].max - -4000.0 / 32768.0).abs() < eps);

	std::fs::remove_file(&path).ok();
}

/// Overwriting with channel count 0 is a logged no-op (the C ABI's state
/// error was FFI-level validation; the Rust API keeps the C++ semantics of
/// returning without touching the data).
#[test]
fn no_channel_count_is_noop() {
	let mut w = AudioVisualWaveform::new();
	let data = [0.5f32; 8];
	let planes = [data.as_slice()];
	w.overwrite_samples(&planes, 48000, Rational::new(0, 1));
	assert!(w.length().is_null());
	assert_eq!(w.channel_count(), 0);

	w.set_channel_count(2);
	assert_eq!(w.channel_count(), 2);
}

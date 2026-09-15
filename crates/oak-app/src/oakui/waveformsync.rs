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

//! Windowed peak envelopes over the cached clip waveforms, the input of
//! `oak_audio::waveformsync`'s offset / stretch correlation (the app-side
//! mirror of the C++ `extract_waveform_cache_envelope` in
//! `timelinewidgetwaveformsync.cpp`).
//!
//! Two deliberate Rust-side adaptations:
//!
//! * The Rust [`super::waveform::WaveformCache`] extracts the WHOLE media
//!   file per clip (no trimmed, partially validated cache regions), so the
//!   envelope windows map absolute media-file time onto the cached peaks
//!   and every window is considered validated (the mask is all-true).
//! * The window peak is the max of `max(|min|, |max|)` over every cached
//!   peak point overlapping the window (the C++
//!   `get_summary_from_time()` equivalent), taken from the first channel
//!   the cache keeps.

use super::waveform::ClipWaveform;

/// Whether a clip carrying this cached waveform can take part in waveform
/// sync (the C++ `get_waveform_sync_clip` gate: a waveform exists, its
/// sample rate is valid and the media range is non-null — the whole-file
/// Rust cache always "intersects" the clip's media range once present).
pub fn waveform_sync_eligible(waveform: &ClipWaveform, media_len_s: f64) -> bool {
	waveform.sample_rate > 0 && !waveform.peaks.is_empty() && media_len_s > 0.0
}

/// Extract the windowed peak envelope of the clip's media range
/// `[media_in_s, media_in_s + media_len_s)` (seconds of media-file time)
/// from the cached peaks.
///
/// Each window is `window_samples` samples wide; the returned envelope
/// holds one peak per window and the mask one validity flag per window
/// (all-true — see the module docs). The mapping runs in integer sample
/// space (the boundaries round like the C++ `time_to_timestamp(...,
/// k_round)`), so whole-second ranges at the cache sample rate land on
/// exact window counts.
///
/// CPP-PARITY: app/widget/timelinewidget/timelinewidgetwaveformsync.cpp
/// (`extract_waveform_cache_envelope`)
pub fn extract_cache_envelope(
	waveform: &ClipWaveform,
	media_in_s: f64,
	media_len_s: f64,
	window_samples: usize,
) -> (Vec<f64>, Vec<bool>) {
	let mut envelope: Vec<f64> = Vec::new();
	let mut valid: Vec<bool> = Vec::new();
	let sample_rate = waveform.sample_rate;
	if sample_rate <= 0 || window_samples == 0 || media_len_s <= 0.0 {
		return (envelope, valid);
	}

	let spp = waveform.samples_per_point.max(1) as i64;
	let start_sample = (media_in_s * f64::from(sample_rate)).round() as i64;
	let end_sample = ((media_in_s + media_len_s) * f64::from(sample_rate)).round() as i64;
	let window = window_samples as i64;

	let mut sample = start_sample.max(0);
	while sample < end_sample {
		let window_end = (sample + window).min(end_sample);
		// The cached peak points overlapping [sample, window_end): a
		// point i covers samples [i * spp, (i + 1) * spp).
		let first_point = sample / spp;
		let last_point = ((window_end + spp - 1) / spp).min(waveform.peaks.len() as i64);
		let mut peak = 0.0f64;
		for i in first_point.max(0)..last_point {
			let p = &waveform.peaks[i as usize];
			let amplitude = f64::max(f64::from(p.min).abs(), f64::from(p.max).abs());
			peak = peak.max(amplitude);
		}
		envelope.push(peak);
		valid.push(true);
		sample += window;
	}
	(envelope, valid)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::waveform::MinMax;

	/// A synthetic 3 s cache at 48 kHz whose peaks are 1.0 exactly inside
	/// `[1, 2)` seconds and 0 elsewhere (240 samples per point makes the
	/// 1/20 s windows land on whole peak runs: 10 peaks per window).
	fn synthetic_waveform() -> ClipWaveform {
		const SAMPLE_RATE: i32 = 48_000;
		const SPP: i32 = 240;
		let points = (SAMPLE_RATE * 3) / SPP; // 600 points for 3 s
		let peaks: Vec<MinMax> = (0..points)
			.map(|i| {
				let sample = i64::from(i) * i64::from(SPP);
				let inside = (48_000..96_000).contains(&sample);
				if inside {
					MinMax { min: -1.0, max: 1.0 }
				} else {
					MinMax::default()
				}
			})
			.collect();
		ClipWaveform {
			peaks,
			channel_count: 1,
			sample_rate: SAMPLE_RATE,
			samples_per_point: SPP,
			duration_frames: 60,
		}
	}

	#[test]
	fn envelope_shape_follows_the_cached_peaks() {
		let waveform = synthetic_waveform();
		// Window = 1/20 s at 48 kHz -> 2400 samples -> 60 windows for 3 s.
		let (envelope, valid) = extract_cache_envelope(&waveform, 0.0, 3.0, 2400);
		assert_eq!(envelope.len(), 60, "3 s at 20 windows/s");
		assert_eq!(valid.len(), 60);
		for (i, (peak, ok)) in envelope.iter().zip(valid.iter()).enumerate() {
			let expected = if (20..40).contains(&i) { 1.0 } else { 0.0 };
			assert_eq!(*peak, expected, "window {i}");
			assert!(*ok, "window {i} validated (whole-file cache)");
		}
	}

	#[test]
	fn envelope_respects_the_media_in_offset() {
		let waveform = synthetic_waveform();
		// The clip's trimmed media range [1, 2) covers only the loud run.
		let (envelope, _) = extract_cache_envelope(&waveform, 1.0, 1.0, 2400);
		assert_eq!(envelope.len(), 20);
		assert!(envelope.iter().all(|&p| p == 1.0), "all windows loud");
	}

	#[test]
	fn envelope_edges_are_empty_or_silent() {
		let waveform = synthetic_waveform();
		let (envelope, _) = extract_cache_envelope(&waveform, 0.0, 0.0, 2400);
		assert!(envelope.is_empty(), "null media range -> no windows");

		// A range past the cached peaks yields zero windows' peaks.
		let (envelope, _) = extract_cache_envelope(&waveform, 10.0, 1.0, 2400);
		assert_eq!(envelope.len(), 20);
		assert!(envelope.iter().all(|&p| p == 0.0));
	}

	#[test]
	fn eligibility_requires_peaks_rate_and_length() {
		let mut waveform = synthetic_waveform();
		assert!(waveform_sync_eligible(&waveform, 3.0));
		assert!(!waveform_sync_eligible(&waveform, 0.0), "null media range");
		waveform.sample_rate = 0;
		assert!(!waveform_sync_eligible(&waveform, 3.0), "invalid rate");
		waveform.sample_rate = 48_000;
		waveform.peaks.clear();
		assert!(!waveform_sync_eligible(&waveform, 3.0), "no peaks");
	}
}

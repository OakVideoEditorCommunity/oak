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

//! M12 P4: timeline audio waveforms.
//!
//! The [`WaveformCache`] holds per-clip min/max peak data extracted
//! through `oak_audio::waveform::extract` (the real FFmpeg-backed
//! extraction; M14 R3: a direct module call, no facade); the engine
//! refreshes it when the timeline rebuilds. The [`OakClipDecorator`] draws
//! the peaks into the timeline's clip body.
//!
//! The extraction is keyed by `(clip id, filename)` and cached for the
//! engine lifetime; the plan's disk cache is a later refinement (the
//! extractor itself is the real implementation).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::timeline::clip::ClipDecorator;
use gpui::timeline::ClipId;
use gpui::timeline::FrameRange;
use gpui::{Bounds, Hsla, Pixels, Window};

/// One min/max amplitude pair (mirror of the oakaudio waveform C ABI
/// `MinMax`: two f32s).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MinMax {
	/// Minimum amplitude in the window.
	pub min: f32,
	/// Maximum amplitude in the window.
	pub max: f32,
}

/// The cached waveform of one clip.
#[derive(Debug, Clone)]
pub struct ClipWaveform {
	/// Per-point min/max (first channel).
	pub peaks: Vec<MinMax>,
	/// Channel count of the media.
	pub channel_count: i32,
	/// Media sample rate.
	pub sample_rate: i32,
	/// Samples per point at extraction time.
	pub samples_per_point: i32,
	/// The clip's length in timeline frames.
	pub duration_frames: i64,
}

/// Per-clip waveform cache (thread-safe; filled by the engine).
pub struct WaveformCache {
	/// Peaks by clip id.
	map: Mutex<HashMap<u64, Arc<ClipWaveform>>>,
	/// Timeline frame rate (frames per second) for frame→time mapping.
	fps: f32,
	/// Bumped on every insert so the UI can repaint when a background
	/// extraction lands (the extraction itself runs off the UI thread —
	/// a 40-minute file's full audio decode blocks project open for
	/// seconds when run inline).
	version: std::sync::atomic::AtomicU64,
}

impl WaveformCache {
	/// Create an empty cache at the given frame rate.
	pub fn new(fps: f32) -> Arc<WaveformCache> {
		Arc::new(WaveformCache {
			map: Mutex::new(HashMap::new()),
			fps: if fps > 0.0 { fps } else { 25.0 },
			version: std::sync::atomic::AtomicU64::new(0),
		})
	}

	/// Monotonic insert counter (repaint trigger for async refresh).
	pub fn version(&self) -> u64 {
		self.version.load(std::sync::atomic::Ordering::Relaxed)
	}

	/// The cached waveform of `clip`, if extracted.
	pub fn get(&self, clip: u64) -> Option<Arc<ClipWaveform>> {
		self.map
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.get(&clip)
			.cloned()
	}

	/// Ensure `clip` (with media `filename`, `duration_frames` long) has a
	/// waveform, extracting it through the real codec-backed extractor
	/// when missing. Failures (missing media, no audio stream) leave the
	/// clip silent (no waveform drawn).
	pub fn refresh(&self, clip: u64, filename: &str, duration_frames: i64) {
		if self.get(clip).is_some() {
			return;
		}
		let Some(waveform) = extract(filename, duration_frames, self.fps) else {
			return;
		};
		let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
		if let std::collections::hash_map::Entry::Vacant(e) = map.entry(clip) {
			e.insert(Arc::new(waveform));
			self.version
				.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		}
	}
}

/// Extract a clip's waveform (first channel) via oakaudio's waveform
/// extractor.
fn extract(filename: &str, duration_frames: i64, _fps: f32) -> Option<ClipWaveform> {
	let cname = std::ffi::CString::new(filename).ok()?;
	const SAMPLES_PER_POINT: i32 = 256;
	// Audio-stream index 0 (probe numbering).
	let outcome = oak_audio::waveform::extract(&cname, 0, SAMPLES_PER_POINT).ok()?;
	if outcome.channels <= 0 || outcome.points.is_empty() {
		return None;
	}
	// Keep only the first channel: the points are channel-interleaved
	// (point i channel c at index i * channels + c).
	let channel_count = outcome.channels.max(1) as usize;
	let count = outcome.points.len() / channel_count;
	let mut peaks = Vec::with_capacity(count);
	peaks.extend((0..count).map(|i| {
		let p = outcome.points[i * channel_count];
		MinMax {
			min: p.min,
			max: p.max,
		}
	}));
	Some(ClipWaveform {
		peaks,
		channel_count: outcome.channels.max(1),
		sample_rate: 48000,
		samples_per_point: SAMPLES_PER_POINT,
		duration_frames,
	})
}

/// Extract an audio RMS peak envelope directly from a media file's
/// audio stream — the multicam wizard's sync correlation input. Uses
/// the oakaudio waveform extractor's min/max points (per 256 source
/// samples), keeps the first channel, and maps each point onto a tiny
/// norm that the envelope correlator can compare across angles (the
/// point's peak = max(|min|, |max|), windowed 1/10 s).
///
/// Returns `None` when the file has no decodable audio (the wizard then
/// falls back to timecode alignment).
pub fn extract_audio_envelope(filename: &str, stream_index: i32) -> Option<Vec<f64>> {
	let cname = std::ffi::CString::new(filename).ok()?;
	const SAMPLES_PER_POINT: i32 = 256;
	const WINDOW_POINTS: usize = 10; // ~100 ms windows at 48 kHz
	let outcome = oak_audio::waveform::extract(&cname, stream_index, SAMPLES_PER_POINT).ok()?;
	if outcome.channels <= 0 || outcome.points.is_empty() {
		return None;
	}
	let channel_count = outcome.channels.max(1) as usize;
	let count = outcome.points.len() / channel_count;
	// Peaks of the first channel.
	let mut peaks: Vec<f64> = (0..count)
		.map(|i| {
			let p = outcome.points[i * channel_count];
			f64::max(f64::from(p.min).abs(), f64::from(p.max).abs())
		})
		.collect();
	// Window: every WINDOW_POINTS points collapses to one max (the
	// envelope the correlator slides).
	let mut envelope = Vec::with_capacity(count.div_ceil(WINDOW_POINTS));
	for chunk in peaks.chunks_mut(WINDOW_POINTS) {
		envelope.push(chunk.iter().copied().fold(0.0, f64::max));
	}
	Some(envelope)
}

/// The timeline's clip decorator: draws extracted waveforms into audio
/// clips (M12 P4). Reads the engine-populated cache.
pub struct OakClipDecorator {
	/// The waveform cache.
	pub cache: Arc<WaveformCache>,
}

impl ClipDecorator for OakClipDecorator {
	fn paint_waveform(
		&mut self,
		window: &mut Window,
		clip: ClipId,
		_visible_range: FrameRange,
		bounds: Bounds<Pixels>,
	) {
		let Some(waveform) = self.cache.get(clip.0) else {
			return;
		};
		if waveform.peaks.is_empty() || bounds.size.width.as_f32() <= 0.0 {
			return;
		}
		let width = bounds.size.width.as_f32() as usize;
		if width == 0 {
			return;
		}
		let height = bounds.size.height.as_f32();
		let mid = bounds.origin.y.as_f32() + height / 2.0;
		let half = (height / 2.0).max(1.0) * 0.9;

		// Media seconds covered by one waveform point.
		let secs_per_point =
			waveform.samples_per_point.max(1) as f32 / waveform.sample_rate.max(1) as f32;
		let frames_per_point = secs_per_point * self.cache.fps;

		// Sample one peak per horizontal pixel column. The design draws the
		// waveform as a darker green against the clip's green body.
		let color = Hsla {
			h: 0.38,
			s: 0.5,
			l: 0.28,
			a: 0.9,
		};
		for x in 0..width {
			let t = x as f32 / width as f32;
			let frame = (t * waveform.duration_frames as f32) as usize;
			let point_index = (frame as f32 / frames_per_point.max(0.0001)) as usize;
			let point_index = point_index.min(waveform.peaks.len() - 1);
			let p = waveform.peaks[point_index];
			let y_top = mid - p.max.max(0.0) * half;
			let y_bot = mid - p.min.min(0.0) * half;
			let x_px = bounds.origin.x.as_f32() + x as f32;
			window.paint_quad(gpui::fill(
				gpui::Bounds::from_corners(
					gpui::point(Pixels::from(x_px), Pixels::from(y_top)),
					gpui::point(Pixels::from(x_px + 1.0), Pixels::from(y_bot.max(y_top))),
				),
				color,
			));
		}
	}
}

//! Safe, owned audio-analysis DTOs over the engine C ABI.

use std::ffi::c_void;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_waveform_add_ref(waveform: *const c_void);
	fn oak_waveform_release(waveform: *const c_void);
	fn oak_waveform_info(waveform: *const c_void, out: *mut WaveformInfo) -> bool;
	fn oak_waveform_point_at(
		waveform: *const c_void,
		point: usize,
		channel: i32,
		out: *mut MinMax,
	) -> bool;
	fn oak_audio_waveform_extract(
		filename: *const u8,
		filename_len: usize,
		stream_index: i32,
		samples_per_point: i32,
	) -> *const c_void;
	fn oak_audio_estimate_envelope_offset(
		reference: *const f64,
		reference_len: usize,
		candidate: *const f64,
		candidate_len: usize,
		reference_valid: *const bool,
		reference_valid_len: usize,
		candidate_valid: *const bool,
		candidate_valid_len: usize,
		window_samples: usize,
		max_offset_windows: i64,
	) -> OffsetEstimate;
	fn oak_audio_estimate_stretch_offset(
		reference: *const f64,
		reference_len: usize,
		candidate: *const f64,
		candidate_len: usize,
		reference_valid: *const bool,
		reference_valid_len: usize,
		candidate_valid: *const bool,
		candidate_valid_len: usize,
		window_samples: usize,
		max_offset_windows: i64,
		min_rate: f64,
		max_rate: f64,
		rate_step: f64,
	) -> StretchEstimate;
}

/// Candidate offset and normalized correlation confidence.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OffsetEstimate {
	pub offset_samples: i64,
	pub confidence: f64,
	pub valid: bool,
}

/// Candidate playback-rate correction plus offset and confidence.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StretchEstimate {
	pub rate: f64,
	pub offset_samples: i64,
	pub confidence: f64,
	pub valid: bool,
}

/// Correlates two envelopes, optionally excluding invalid windows.
///
/// `window_samples` converts the winning window lag into `offset_samples`.
/// Empty masks mean all windows valid. The computation is pure and synchronous.
pub fn estimate_envelope_offset(
	reference: &[f64],
	candidate: &[f64],
	reference_valid: &[bool],
	candidate_valid: &[bool],
	window_samples: usize,
	max_offset_windows: i64,
) -> OffsetEstimate {
	unsafe {
		oak_audio_estimate_envelope_offset(
			reference.as_ptr(),
			reference.len(),
			candidate.as_ptr(),
			candidate.len(),
			reference_valid.as_ptr(),
			reference_valid.len(),
			candidate_valid.as_ptr(),
			candidate_valid.len(),
			window_samples,
			max_offset_windows,
		)
	}
}

/// Searches a playback-rate correction and offset over two envelopes.
#[allow(clippy::too_many_arguments)]
pub fn estimate_stretch_offset(
	reference: &[f64],
	candidate: &[f64],
	reference_valid: &[bool],
	candidate_valid: &[bool],
	window_samples: usize,
	max_offset_windows: i64,
	min_rate: f64,
	max_rate: f64,
	rate_step: f64,
) -> StretchEstimate {
	unsafe {
		oak_audio_estimate_stretch_offset(
			reference.as_ptr(),
			reference.len(),
			candidate.as_ptr(),
			candidate.len(),
			reference_valid.as_ptr(),
			reference_valid.len(),
			candidate_valid.as_ptr(),
			candidate_valid.len(),
			window_samples,
			max_offset_windows,
			min_rate,
			max_rate,
			rate_step,
		)
	}
}

/// Waveform layout and point-count metadata.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveformInfo {
	/// Number of interleaved channels in the decoded stream.
	pub channels: i32,
	/// Actual media sample rate, not a UI assumed constant.
	pub sample_rate: i32,
	/// Source samples summarized by each point.
	pub samples_per_point: i32,
	/// Number of windows per channel (not the interleaved element count).
	pub point_count: usize,
}

/// One channel's minimum and maximum amplitude within one window.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MinMax {
	pub min: f32,
	pub max: f32,
}

/// Reference-counted immutable waveform result.
///
/// Cloning shares the engine allocation; all accessors copy POD values. The
/// handle is thread-safe because no mutation API exists.
pub struct Waveform {
	ptr: *const c_void,
}

// SAFETY: the engine object is reference counted and immutable after creation.
unsafe impl Send for Waveform {}
// SAFETY: concurrent readers only copy fields from the immutable allocation.
unsafe impl Sync for Waveform {}

impl Waveform {
	pub(crate) fn from_raw(ptr: *const c_void) -> Option<Self> {
		(!ptr.is_null()).then_some(Self { ptr })
	}

	/// Returns stream format and per-channel point count.
	pub fn info(&self) -> WaveformInfo {
		let mut info = WaveformInfo::default();
		let ok = unsafe { oak_waveform_info(self.ptr, &mut info) };
		ok.then_some(info).unwrap_or_default()
	}

	/// Copies one point; `None` for an out-of-range point/channel.
	pub fn point(&self, point: usize, channel: i32) -> Option<MinMax> {
		let mut out = MinMax::default();
		unsafe { oak_waveform_point_at(self.ptr, point, channel, &mut out) }.then_some(out)
	}

	/// Copies every point of one channel in timeline order.
	pub fn peaks(&self, channel: i32) -> Vec<MinMax> {
		let info = self.info();
		if channel < 0 || channel >= info.channels {
			return Vec::new();
		}
		(0..info.point_count)
			.filter_map(|point| self.point(point, channel))
			.collect()
	}

	/// Extracts on the calling thread for hosts that already own a background
	/// media worker. Prefer [`crate::task::extract_waveform`] on UI threads.
	pub fn extract(filename: &str, stream_index: i32, samples_per_point: i32) -> Option<Self> {
		Self::from_raw(unsafe {
			oak_audio_waveform_extract(
				filename.as_ptr(),
				filename.len(),
				stream_index,
				samples_per_point,
			)
		})
	}
}

impl Clone for Waveform {
	fn clone(&self) -> Self {
		unsafe { oak_waveform_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for Waveform {
	fn drop(&mut self) {
		unsafe { oak_waveform_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn envelope_offset_and_stretch_use_engine_correlation() {
		let reference: Vec<f64> = (0..10).map(|index| index as f64).collect();
		let mut candidate = vec![0.0; 10];
		candidate[2..].copy_from_slice(&reference[..8]);
		let offset = super::estimate_envelope_offset(&reference, &candidate, &[], &[], 100, 10);
		assert!(offset.valid);
		assert_eq!(offset.offset_samples, 200);
		assert!((offset.confidence - 1.0).abs() < 1e-9);
		let invalid = vec![false; 10];
		assert!(
			!super::estimate_envelope_offset(&reference, &candidate, &invalid, &invalid, 100, 10,)
				.valid
		);

		let reference: Vec<f64> = (0..10)
			.map(|index| (2.0 * std::f64::consts::PI * 0.7 * index as f64).sin())
			.collect();
		let mut stretched = Vec::new();
		for index in 0..reference.len() {
			stretched.push(reference[index]);
			if index + 1 < reference.len() {
				stretched.push((reference[index] + reference[index + 1]) / 2.0);
			}
		}
		let stretch = super::estimate_stretch_offset(
			&reference,
			&stretched,
			&[],
			&[],
			100,
			10,
			0.5,
			3.0,
			0.1,
		);
		assert!(stretch.valid);
		assert!((stretch.rate - 2.0).abs() < 0.15);
		assert!(stretch.confidence > 0.99);
	}

	#[test]
	fn envelope_offset_recovers_multicam_lags_and_tolerates_short_inputs() {
		let mut state = 0x2545_F491_4F6C_DD1Du64;
		let reference: Vec<f64> = (0..200)
			.map(|_| {
				state = state
					.wrapping_mul(6364136223846793005)
					.wrapping_add(1442695040888963407);
				(((state >> 33) as f64 / ((1u64 << 31) as f64)) < 0.5) as u8 as f64
			})
			.collect();
		let shifted = |lag: isize| {
			(0..200)
				.map(|index| {
					let source = index as isize + lag;
					if (0..200).contains(&source) {
						reference[source as usize]
					} else {
						0.0
					}
				})
				.collect::<Vec<_>>()
		};
		for (candidate, expected) in [(shifted(20), -20), (shifted(-30), 30)] {
			let estimate = super::estimate_envelope_offset(&reference, &candidate, &[], &[], 1, 50);
			assert!(estimate.valid);
			assert_eq!(estimate.offset_samples, expected);
		}
		for (reference, candidate) in [(&[][..], &[][..]), (&[1.0][..], &[2.0, 3.0][..])] {
			let estimate = super::estimate_envelope_offset(reference, candidate, &[], &[], 1, 50);
			assert!(!estimate.valid);
			assert_eq!(estimate.offset_samples, 0);
		}
	}

	#[test]
	fn waveform_task_transfers_real_metadata_and_peaks_once() {
		let media = crate::test_support::make_test_clip_with_audio("waveform_task");
		let task = crate::task::extract_waveform(&media, 0, 256).expect("waveform task");
		assert!(task.title().contains("Analyzing Audio"));
		assert!(task.wait(), "waveform extraction failed: {}", task.error());
		assert_eq!(task.state(), crate::task::TaskState::Succeeded);
		let waveform = task.take_waveform().expect("owned waveform");
		assert!(
			task.take_waveform().is_none(),
			"a result transfers only once"
		);
		let info = waveform.info();
		assert_eq!(info.channels, 2);
		assert_eq!(info.sample_rate, 48_000);
		assert_eq!(info.samples_per_point, 256);
		assert!(info.point_count > 0);
		let peaks = waveform.peaks(0);
		assert_eq!(peaks.len(), info.point_count);
		assert!(peaks.iter().any(|point| point.min < 0.0 && point.max > 0.0));
		assert!(waveform.point(info.point_count, 0).is_none());
		assert!(waveform.peaks(info.channels).is_empty());
		let cloned = waveform.clone();
		assert_eq!(cloned.info(), info);
		drop(cloned);
		assert_eq!(waveform.info(), info);

		let missing = crate::task::extract_waveform("/nonexistent/oak-waveform-input.mp4", 0, 256)
			.expect("invalid media fails asynchronously");
		assert!(!missing.wait());
		assert!(!missing.error().is_empty());
		assert!(missing.take_waveform().is_none());
		let _ = std::fs::remove_file(media);
	}
}

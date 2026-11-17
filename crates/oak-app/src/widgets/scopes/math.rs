//! Pure math for the scope widgets: histogram binning, waveform envelopes
//! and vectorscope chroma projection. No gpui coupling, unit-tested.

/// Bin luma samples (`0..1`) into `bins` histogram buckets, returning the
/// count per bucket. Samples outside `0..1` clamp to the edges.
pub fn histogram_bins(samples: &[f32], bins: usize) -> Vec<u32> {
	if bins == 0 {
		return Vec::new();
	}
	let mut out = vec![0u32; bins];
	if samples.is_empty() {
		return out;
	}
	for &sample in samples {
		if !sample.is_finite() {
			continue;
		}
		let clamped = sample.clamp(0.0, 1.0);
		let index = ((clamped * bins as f32) as usize).min(bins - 1);
		out[index] += 1;
	}
	out
}

/// Compute the min/max envelope of `samples` over `columns` vertical slices.
/// Each column covers a contiguous slice of the input; empty columns report
/// `(0.0, 0.0)`.
pub fn waveform_envelope(samples: &[f32], columns: usize) -> Vec<(f32, f32)> {
	let mut out = vec![(0.0f32, 0.0f32); columns.max(1)];
	if columns == 0 || samples.is_empty() {
		return out;
	}
	for column in 0..columns {
		let start = column * samples.len() / columns;
		let end = ((column + 1) * samples.len() / columns)
			.max(start + 1)
			.min(samples.len());
		let mut min = f32::MAX;
		let mut max = f32::MIN;
		let mut has_finite = false;
		for &sample in &samples[start..end] {
			if sample.is_finite() {
				min = min.min(sample);
				max = max.max(sample);
				has_finite = true;
			}
		}
		if end > start && has_finite {
			out[column] = (min, max);
		}
	}
	out
}

/// A chroma sample pair (e.g. `u`, `v` centered on `0.5`).
pub type ChromaSample = (f32, f32);

/// Project chroma samples onto the vectorscope's two axes
/// (`u - 0.5`, `v - 0.5`, normalized to `-0.5..0.5`). Out-of-range values
/// clamp.
pub fn vectorscope_points(samples: &[ChromaSample]) -> Vec<(f32, f32)> {
	samples
		.iter()
		.filter(|(u, v)| u.is_finite() && v.is_finite())
		.map(|&(u, v)| ((u - 0.5).clamp(-0.5, 0.5), (v - 0.5).clamp(-0.5, 0.5)))
		.collect()
}

/// Map a normalized `0..1` level to a meter segment's lit count: `segments`
/// segments, the lit portion is proportional to the level.
pub fn meter_lit_segments(level: f32, segments: usize) -> usize {
	if level.is_nan() {
		return 0;
	}
	let lit = (level.clamp(0.0, 1.0) * segments as f32).round() as usize;
	lit.min(segments)
}

/// Peak-hold decay: `peak` decays toward `level` at `decay_per_frame`.
pub fn decay_peak(peak: f32, level: f32, decay_per_frame: f32) -> f32 {
	let peak = if peak.is_finite() { peak } else { 0.0 };
	let level = if level.is_finite() { level } else { 0.0 };
	let decay = if decay_per_frame.is_finite() && decay_per_frame >= 0.0 {
		decay_per_frame
	} else {
		0.0
	};
	if level >= peak {
		level
	} else {
		(peak - decay).max(level)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn histogram_bins_correctly() {
		let samples = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
		let bins = histogram_bins(&samples, 4);
		// Buckets: [0,0.25): 0.0, 0.1; [0.25,0.5): 0.25; [0.5,0.75): 0.5;
		// [0.75,1]: 0.75, 0.9, 1.0 (1.0 clamps into the last bucket).
		assert_eq!(bins, vec![2, 1, 1, 3]);
	}

	#[test]
	fn histogram_clamps_out_of_range() {
		let bins = histogram_bins(&[-1.0, 0.5, 2.0], 2);
		assert_eq!(bins, vec![1, 2]);
	}

	#[test]
	fn histogram_handles_empty_and_zero_bins() {
		assert_eq!(histogram_bins(&[], 4), vec![0u32; 4]);
		assert_eq!(histogram_bins(&[0.5], 0), Vec::<u32>::new());
	}

	#[test]
	fn waveform_envelope_slices() {
		let samples: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
		let env = waveform_envelope(&samples, 10);
		assert_eq!(env.len(), 10);
		// First column covers [0, 10): min 0.0, max 0.09.
		assert!((env[0].0 - 0.0).abs() < 0.001);
		assert!((env[0].1 - 0.09).abs() < 0.001);
		// Last column covers [90, 100): min 0.9, max 0.99.
		assert!((env[9].0 - 0.9).abs() < 0.001);
		assert!((env[9].1 - 0.99).abs() < 0.001);
	}

	#[test]
	fn vectorscope_projection_centers() {
		let points = vectorscope_points(&[(0.5, 0.5), (1.0, 0.0), (0.0, 1.0)]);
		assert_eq!(points[0], (0.0, 0.0));
		assert_eq!(points[1], (0.5, -0.5));
		assert_eq!(points[2], (-0.5, 0.5));
		// Out-of-range clamps.
		let clamped = vectorscope_points(&[(2.0, -1.0)]);
		assert_eq!(clamped[0], (0.5, -0.5));
	}

	#[test]
	fn meter_lit_and_peak_decay() {
		assert_eq!(meter_lit_segments(0.0, 8), 0);
		assert_eq!(meter_lit_segments(0.5, 8), 4);
		assert_eq!(meter_lit_segments(1.0, 8), 8);
		assert_eq!(meter_lit_segments(1.5, 8), 8);

		assert_eq!(decay_peak(0.8, 0.5, 0.1), 0.7);
		assert_eq!(decay_peak(0.8, 0.9, 0.1), 0.9);
		assert_eq!(decay_peak(0.8, 0.78, 0.1), 0.78);
	}

	#[test]
	fn histogram_filters_nan_and_inf() {
		let samples = [f32::NAN, 0.5, f32::INFINITY, -f32::INFINITY];
		let bins = histogram_bins(&samples, 2);
		// 0.5 falls into bucket [0.5, 1.0] (index 1), all non-finite skipped
		assert_eq!(bins, vec![0, 1]);
	}

	#[test]
	fn waveform_envelope_handles_nan_and_inf() {
		let samples = [f32::NAN, f32::INFINITY, -f32::INFINITY];
		let env = waveform_envelope(&samples, 1);
		assert_eq!(env, vec![(0.0, 0.0)]);

		let mixed = [f32::NAN, 0.2, 0.8, f32::INFINITY];
		let env_mixed = waveform_envelope(&mixed, 1);
		assert_eq!(env_mixed, vec![(0.2, 0.8)]);
	}

	#[test]
	fn vectorscope_filters_nan_and_inf() {
		let samples = [
			(0.5, 0.5),
			(f32::NAN, 0.5),
			(0.5, f32::INFINITY),
			(1.0, 0.0),
		];
		let points = vectorscope_points(&samples);
		assert_eq!(points.len(), 2);
		assert_eq!(points[0], (0.0, 0.0));
		assert_eq!(points[1], (0.5, -0.5));
	}

	#[test]
	fn meter_and_decay_nan_and_inf() {
		assert_eq!(meter_lit_segments(f32::NAN, 8), 0);
		assert_eq!(meter_lit_segments(f32::INFINITY, 8), 8);
		assert_eq!(meter_lit_segments(-f32::INFINITY, 8), 0);

		// decay_peak with NaN and Inf
		assert_eq!(decay_peak(0.8, f32::NAN, 0.1), 0.7);
		assert_eq!(decay_peak(f32::NAN, 0.5, 0.1), 0.5);
		assert_eq!(decay_peak(0.8, f32::INFINITY, 0.1), 0.7);
	}
}

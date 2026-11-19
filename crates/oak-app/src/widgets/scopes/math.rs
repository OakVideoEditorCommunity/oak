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

/// Compute the min/max luma envelope across `columns` output columns for a
/// row-major `width * height` frame. Each output column aggregates every `Y`
/// row at its corresponding image `X` columns; empty frames, zero dimensions,
/// and non-finite samples fall back safely to `(0.0, 0.0)`.
pub fn waveform_envelope(
	samples: &[f32],
	width: usize,
	height: usize,
	columns: usize,
) -> Vec<(f32, f32)> {
	let mut out = vec![(0.0f32, 0.0f32); columns.max(1)];
	if columns == 0 || width == 0 || height == 0 || samples.is_empty() {
		return out;
	}

	let ranges: Vec<(usize, usize)> = (0..columns)
		.map(|column| {
			let x_start = ((column as u128 * width as u128) / columns as u128) as usize;
			let x_end = (((column as u128 + 1) * width as u128) / columns as u128)
				.max(x_start as u128 + 1)
				.min(width as u128) as usize;
			(x_start, x_end)
		})
		.collect();

	let mut acc = vec![(f32::MAX, f32::MIN, false); columns];
	for row in samples.chunks(width).take(height) {
		for (column, &(x_start, x_end)) in ranges.iter().enumerate() {
			let row_end = x_end.min(row.len());
			if x_start >= row_end {
				continue;
			}
			let (min, max, has_finite) = &mut acc[column];
			for &sample in &row[x_start..row_end] {
				if sample.is_finite() {
					*min = min.min(sample);
					*max = max.max(sample);
					*has_finite = true;
				}
			}
		}
	}

	for (dst, (min, max, has_finite)) in out.iter_mut().zip(acc) {
		if has_finite {
			*dst = (min, max);
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
	fn waveform_envelope_by_x_column_cases() {
		struct Case {
			name: &'static str,
			width: usize,
			height: usize,
			columns: usize,
			samples: Vec<f32>,
			expected: Vec<(f32, f32)>,
		}

		let cases = [
			Case {
				name: "black top half, white bottom half spans (0, 1) in every X column",
				width: 4,
				height: 2,
				columns: 4,
				samples: vec![
					0.0, 0.0, 0.0, 0.0, // y = 0
					1.0, 1.0, 1.0, 1.0, // y = 1
				],
				expected: vec![(0.0, 1.0); 4],
			},
			Case {
				name: "black left half, white right half keeps columns distinct",
				width: 4,
				height: 2,
				columns: 2,
				samples: vec![
					0.0, 0.0, 1.0, 1.0, // y = 0
					0.0, 0.0, 1.0, 1.0, // y = 1
				],
				expected: vec![(0.0, 0.0), (1.0, 1.0)],
			},
			Case {
				name: "per-column gradients aggregate vertically by X",
				width: 3,
				height: 2,
				columns: 3,
				samples: vec![
					0.1, 0.5, 0.9, // y = 0
					0.2, 0.5, 0.8, // y = 1
				],
				expected: vec![(0.1, 0.2), (0.5, 0.5), (0.8, 0.9)],
			},
			Case {
				name: "non-divisible width partitions X columns cleanly",
				width: 5,
				height: 2,
				columns: 2,
				// X partitions: [0..2] -> (0.1, 0.3), [2..5] -> (0.6, 0.9)
				samples: vec![
					0.1, 0.2, 0.6, 0.7, 0.9, // y = 0
					0.3, 0.2, 0.8, 0.6, 0.7, // y = 1
				],
				expected: vec![(0.1, 0.3), (0.6, 0.9)],
			},
			Case {
				name: "tiny image narrower than output columns repeats nearest X columns",
				width: 2,
				height: 2,
				columns: 4,
				samples: vec![
					0.2, 0.8, // y = 0
					0.4, 0.9, // y = 1
				],
				expected: vec![(0.2, 0.4), (0.2, 0.4), (0.8, 0.9), (0.8, 0.9)],
			},
			Case {
				name: "1x1 tiny image expands safely across columns",
				width: 1,
				height: 1,
				columns: 3,
				samples: vec![0.42],
				expected: vec![(0.42, 0.42); 3],
			},
			Case {
				name: "empty frame returns zeroed envelopes",
				width: 4,
				height: 2,
				columns: 3,
				samples: vec![],
				expected: vec![(0.0, 0.0); 3],
			},
			Case {
				name: "zero columns returns single zeroed fallback slot",
				width: 2,
				height: 2,
				columns: 0,
				samples: vec![0.5; 4],
				expected: vec![(0.0, 0.0)],
			},
			Case {
				name: "zero image width or height returns zeroed envelopes",
				width: 0,
				height: 2,
				columns: 3,
				samples: vec![0.5; 4],
				expected: vec![(0.0, 0.0); 3],
			},
			Case {
				name: "short buffer does not panic and aggregates available pixels",
				width: 4,
				height: 2,
				columns: 2,
				samples: vec![0.1, 0.2, 0.7, 0.9, 0.3],
				expected: vec![(0.1, 0.3), (0.7, 0.9)],
			},
		];

		for case in cases {
			let env = waveform_envelope(&case.samples, case.width, case.height, case.columns);
			assert_eq!(env.len(), case.expected.len(), "case: {}", case.name);
			for (col, (&got, &want)) in env.iter().zip(case.expected.iter()).enumerate() {
				assert!(
					(got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
					"case '{}' column {}: got {:?}, expected {:?}",
					case.name,
					col,
					got,
					want
				);
			}
		}
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
		let samples = [f32::NAN, f32::INFINITY, -f32::INFINITY, f32::NAN];
		let env = waveform_envelope(&samples, 2, 2, 1);
		assert_eq!(env, vec![(0.0, 0.0)]);

		let mixed = [
			f32::NAN,
			0.2, // y = 0
			0.8,
			f32::INFINITY, // y = 1
		];
		let env_mixed = waveform_envelope(&mixed, 2, 2, 2);
		assert_eq!(env_mixed, vec![(0.8, 0.8), (0.2, 0.2)]);
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

//! Pure curve geometry for the [`CurveEditor`](super::CurveEditor): cubic
//! bezier keyframe curves with in/out control handles. No gpui dependency.

/// A 2D point in normalized curve space (`x` and `y` in `0..1`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CurveVec2 {
	/// Horizontal position.
	pub x: f64,
	/// Vertical position.
	pub y: f64,
}

impl CurveVec2 {
	/// Creates a point at (`x`, `y`) in normalized curve space.
	///
	/// Both coordinates are expected in `0..1` (x = input position, y = output
	/// value) but are stored verbatim — nothing is clamped or validated. Being a
	/// `const fn`, it can initialise constants and `static` keyframe tables.
	pub const fn new(x: f64, y: f64) -> Self {
		Self { x, y }
	}
}

/// Which control handle of a point is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleSide {
	/// The handle leading into the point (from the previous point).
	In,
	/// The handle leaving the point (toward the next point).
	Out,
}

/// A keyframe point with optional bezier control handles (offsets from the
/// point, in normalized units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
	/// Input position, `0..1`.
	pub x: f64,
	/// Output value, `0..1`.
	pub y: f64,
	/// Control point offset for the incoming segment.
	pub handle_in: Option<CurveVec2>,
	/// Control point offset for the outgoing segment.
	pub handle_out: Option<CurveVec2>,
}

impl CurvePoint {
	/// Creates a keyframe at (`x`, `y`) with no control handles.
	///
	/// `x` is the input position and `y` the output value, both normalized to
	/// `0..1` and stored verbatim (no clamping). Without handles the neighbouring
	/// segments are linear: `segment_controls` falls back to midpoint x control
	/// points. Use `with_handles` for a curved keyframe; both constructors are
	/// `const fn`.
	pub const fn new(x: f64, y: f64) -> Self {
		Self {
			x,
			y,
			handle_in: None,
			handle_out: None,
		}
	}

	/// Creates a keyframe at (`x`, `y`) with both handles set to `offset`.
	///
	/// `offset` is relative to the point, in normalized units, so the in and out
	/// handles start coincident and symmetric (a smooth keyframe) and the editor can
	/// move each side independently afterwards. `x`/`y` are stored verbatim.
	/// Counterpart of `CurvePoint::new`, which leaves the handles absent.
	pub const fn with_handles(x: f64, y: f64, offset: CurveVec2) -> Self {
		Self {
			x,
			y,
			handle_in: Some(offset),
			handle_out: Some(offset),
		}
	}
}

/// Evaluates one cubic bezier segment at parameter `t`.
///
/// `p0`/`p1` are the segment endpoints and `c1`/`c2` the control points, all in
/// normalized curve space. `t` is the bezier parameter, not an x coordinate: `0`
/// returns `p0`, `1` returns `p1`, and values outside `0..1` extrapolate along
/// the polynomial. Callers that need the output at a given x should go through
/// `sample_curve`, which inverts x numerically, instead of choosing `t`.
pub fn cubic_bezier(
	p0: CurveVec2,
	c1: CurveVec2,
	c2: CurveVec2,
	p1: CurveVec2,
	t: f64,
) -> CurveVec2 {
	let u = 1.0 - t;
	CurveVec2::new(
		u * u * u * p0.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * p1.x,
		u * u * u * p0.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * p1.y,
	)
}

/// The control points of the segment from `p0` to `p1` (linear when handles
/// are absent).
pub fn segment_controls(p0: &CurvePoint, p1: &CurvePoint) -> (CurveVec2, CurveVec2) {
	let c1 = match p0.handle_out {
		Some(h) => CurveVec2::new(p0.x + h.x, p0.y + h.y),
		None => CurveVec2::new((p0.x + p1.x) / 2.0, p0.y),
	};
	let c2 = match p1.handle_in {
		Some(h) => CurveVec2::new(p1.x + h.x, p1.y + h.y),
		None => CurveVec2::new((p0.x + p1.x) / 2.0, p1.y),
	};
	(c1, c2)
}

/// Sample the curve at `x`, returning the output value. `x` is clamped to
/// the point range; values before the first (after the last) point clamp to
/// the first (last) point's output.
pub fn sample_curve(points: &[CurvePoint], x: f64) -> f64 {
	if points.is_empty() {
		return 0.0;
	}
	if points.len() == 1 {
		return points[0].y;
	}
	let x = x.clamp(points[0].x, points[points.len() - 1].x);
	let index = points
		.windows(2)
		.position(|w| x >= w[0].x && x <= w[1].x)
		.unwrap_or(points.len() - 2);
	let p0 = &points[index];
	let p1 = &points[index + 1];
	let (c1, c2) = segment_controls(p0, p1);
	sample_segment(
		CurveVec2::new(p0.x, p0.y),
		c1,
		c2,
		CurveVec2::new(p1.x, p1.y),
		x,
	)
}

/// Sample one segment for the `y` at a given `x`, by finding the `t` whose
/// bezier x coordinate matches (binary search, since bezier x is monotonic
/// for well-formed curves).
fn sample_segment(p0: CurveVec2, c1: CurveVec2, c2: CurveVec2, p1: CurveVec2, x: f64) -> f64 {
	let mut lo = 0.0;
	let mut hi = 1.0;
	for _ in 0..24 {
		let mid = (lo + hi) / 2.0;
		let px = cubic_bezier(p0, c1, c2, p1, mid).x;
		if px < x {
			lo = mid;
		} else {
			hi = mid;
		}
	}
	let t = (lo + hi) / 2.0;
	cubic_bezier(p0, c1, c2, p1, t).y
}

/// Approximate the whole curve as a polyline (for painting). Each segment is
/// sampled `samples` times.
pub fn polyline(points: &[CurvePoint], samples: usize) -> Vec<CurveVec2> {
	let mut out = Vec::new();
	if points.is_empty() {
		return out;
	}
	out.push(CurveVec2::new(points[0].x, points[0].y));
	for window in points.windows(2) {
		let (p0, p1) = (&window[0], &window[1]);
		let (c1, c2) = segment_controls(p0, p1);
		let p0v = CurveVec2::new(p0.x, p0.y);
		let p1v = CurveVec2::new(p1.x, p1.y);
		for i in 1..=samples {
			let t = i as f64 / samples as f64;
			out.push(cubic_bezier(p0v, c1, c2, p1v, t));
		}
	}
	out
}

/// Finds the index of the point nearest to `pos` within `threshold`.
///
/// Distance is Euclidean in normalized curve space, so `threshold` is in the
/// same units (an editor pixel tolerance divided by the canvas size). Returns
/// `None` when no point is within reach, and always `None` for an empty slice.
/// Ties resolve to the later point scanned, i.e. the highest index among
/// equidistant candidates. Pair with `hit_test_handle`, which prefers handles
/// when both are in reach.
pub fn hit_test_point(points: &[CurvePoint], pos: CurveVec2, threshold: f64) -> Option<usize> {
	let mut best = None;
	let mut best_dist = threshold;
	for (index, point) in points.iter().enumerate() {
		let dx = point.x - pos.x;
		let dy = point.y - pos.y;
		let dist = (dx * dx + dy * dy).sqrt();
		if dist <= best_dist {
			best_dist = dist;
			best = Some(index);
		}
	}
	best
}

/// Find the handle closest to `pos` within `threshold`, preferring handles
/// over points when both are within reach.
pub fn hit_test_handle(
	points: &[CurvePoint],
	pos: CurveVec2,
	threshold: f64,
) -> Option<(usize, HandleSide)> {
	let mut best = None;
	let mut best_dist = threshold;
	for (index, point) in points.iter().enumerate() {
		for (side, handle) in [
			(HandleSide::In, point.handle_in),
			(HandleSide::Out, point.handle_out),
		] {
			if let Some(h) = handle {
				let hp = CurveVec2::new(point.x + h.x, point.y + h.y);
				let dx = hp.x - pos.x;
				let dy = hp.y - pos.y;
				let dist = (dx * dx + dy * dy).sqrt();
				if dist <= best_dist {
					best_dist = dist;
					best = Some((index, side));
				}
			}
		}
	}
	best
}

#[cfg(test)]
mod tests {
	use super::*;

	fn approx(a: f64, b: f64) -> bool {
		(a - b).abs() < 1e-6
	}

	#[test]
	fn bezier_endpoints() {
		let p0 = CurveVec2::new(0.0, 0.0);
		let p1 = CurveVec2::new(1.0, 1.0);
		assert_eq!(cubic_bezier(p0, p0, p1, p1, 0.0), p0);
		assert_eq!(cubic_bezier(p0, p0, p1, p1, 1.0), p1);
		// A straight-line bezier at t=0.5 is the midpoint.
		let mid = cubic_bezier(p0, p0, p1, p1, 0.5);
		assert!(approx(mid.x, 0.5) && approx(mid.y, 0.5));
	}

	#[test]
	fn linear_curve_samples_exactly() {
		let points = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
		assert!(approx(sample_curve(&points, 0.0), 0.0));
		assert!(approx(sample_curve(&points, 0.5), 0.5));
		assert!(approx(sample_curve(&points, 1.0), 1.0));
		// Clamps outside the range.
		assert!(approx(sample_curve(&points, 2.0), 1.0));
		assert!(approx(sample_curve(&points, -1.0), 0.0));
	}

	#[test]
	fn stepped_curve_clamps_to_segments() {
		let points = vec![
			CurvePoint::new(0.0, 0.0),
			CurvePoint::new(0.5, 0.0),
			CurvePoint::new(1.0, 1.0),
		];
		assert!(approx(sample_curve(&points, 0.25), 0.0));
		assert!(approx(sample_curve(&points, 0.75), 0.5));
	}

	#[test]
	fn single_point_is_constant() {
		let points = vec![CurvePoint::new(0.5, 0.7)];
		assert!(approx(sample_curve(&points, 0.0), 0.7));
		assert!(approx(sample_curve(&points, 0.9), 0.7));
	}

	#[test]
	fn bezier_handles_bend_the_curve() {
		// A curve whose outgoing handle pushes straight up at the start must
		// start with output above the linear interpolation.
		let points = vec![
			CurvePoint::with_handles(0.0, 0.0, CurveVec2::new(0.0, 1.0)),
			CurvePoint::new(1.0, 1.0),
		];
		let linear = sample_curve(
			&[CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)],
			0.25,
		);
		let bent = sample_curve(&points, 0.25);
		assert!(bent > linear, "bent={bent} linear={linear}");
	}

	#[test]
	fn polyline_has_expected_length() {
		let points = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
		let line = polyline(&points, 8);
		assert_eq!(line.len(), 9);
		assert_eq!(line.first().unwrap(), &CurveVec2::new(0.0, 0.0));
		assert_eq!(line.last().unwrap(), &CurveVec2::new(1.0, 1.0));
		assert_eq!(polyline(&[], 8).len(), 0);
	}

	#[test]
	fn hit_test_finds_nearest_point() {
		let points = vec![
			CurvePoint::new(0.1, 0.1),
			CurvePoint::new(0.5, 0.5),
			CurvePoint::new(0.9, 0.9),
		];
		assert_eq!(
			hit_test_point(&points, CurveVec2::new(0.52, 0.52), 0.1),
			Some(1)
		);
		assert_eq!(
			hit_test_point(&points, CurveVec2::new(0.1, 0.1), 0.1),
			Some(0)
		);
		// Beyond the threshold.
		assert_eq!(
			hit_test_point(&points, CurveVec2::new(0.3, 0.3), 0.05),
			None
		);
	}

	#[test]
	fn hit_test_finds_handle() {
		let points = vec![CurvePoint {
			x: 0.2,
			y: 0.5,
			handle_in: Some(CurveVec2::new(-0.1, -0.1)),
			handle_out: Some(CurveVec2::new(0.1, 0.1)),
		}];
		// The outgoing handle endpoint sits at (0.3, 0.6).
		let hit = hit_test_handle(&points, CurveVec2::new(0.31, 0.61), 0.05);
		assert_eq!(hit, Some((0, HandleSide::Out)));
		// The incoming handle endpoint sits at (0.1, 0.4).
		let hit = hit_test_handle(&points, CurveVec2::new(0.09, 0.39), 0.05);
		assert_eq!(hit, Some((0, HandleSide::In)));
		assert_eq!(
			hit_test_handle(&points, CurveVec2::new(0.9, 0.9), 0.05),
			None
		);
	}
}

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

//! Keyframes and interpolation (C++ `NodeKeyframe` + track logic).
//!
//! Interpolation parity: [`KeyframeTrack::value_at`] ports C++
//! `Node::get_split_value_at_time_on_track` (`src/node/src/node.cpp`)
//! with the cubic/quadratic bezier solvers from
//! `core/src/oliveimpl/util/bezier.h`. Unlike C++ (one track per
//! component), a Rust track holds whole values; vector/color values
//! interpolate component-wise.

use oak_core::Rational;

use crate::value::{NodeValue, ValueType};

/// Interpolation mode (values match the C++ `NodeKeyframe::Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
	/// Constant hold.
	Hold,
	/// Linear.
	Linear,
	/// Cubic bezier (control points on the keyframe).
	Bezier,
}

/// A single keyframe.
#[derive(Clone, Debug)]
pub struct Keyframe {
	/// Time.
	pub time: Rational,
	/// Value.
	pub value: NodeValue,
	/// Interpolation to the next keyframe.
	pub interpolation: Interpolation,
	/// Bezier control points (used when `interpolation == Bezier`).
	pub bezier_in: (f64, f64),
	/// Bezier out control point.
	pub bezier_out: (f64, f64),
}

/// Sorted keyframe track for one (input, element).
#[derive(Default, Debug, Clone)]
pub struct KeyframeTrack {
	keys: Vec<Keyframe>,
}

impl KeyframeTrack {
	/// Insert or replace the keyframe at `time` (keeps sort order).
	pub fn set_key(&mut self, key: Keyframe) {
		match self.keys.binary_search_by(|k| k.time.cmp(&key.time)) {
			Ok(i) => self.keys[i] = key,
			Err(i) => self.keys.insert(i, key),
		}
	}

	/// Replace the value of the key at `time` (preserving its
	/// interpolation and bezier handles); false when absent.
	pub fn set_key_value(&mut self, time: Rational, value: NodeValue) -> bool {
		if let Some(k) = self.keys.iter_mut().find(|k| k.time == time) {
			k.value = value;
			true
		} else {
			false
		}
	}

	/// Remove the keyframe at `time`; false when absent.
	pub fn remove_key(&mut self, time: Rational) -> bool {
		match self.keys.binary_search_by(|k| k.time.cmp(&time)) {
			Ok(i) => {
				self.keys.remove(i);
				true
			}
			Err(_) => false,
		}
	}

	/// Interpolated value at `time`; `None` on an empty track.
	/// Interpolation math must match the C++ lerp/bezier exactly
	/// (`// CPP-PARITY: node.cpp:465` `get_split_value_at_time_on_track`,
	/// `// CPP-PARITY: core/src/oliveimpl/util/bezier.h`).
	pub fn value_at(&self, time: Rational) -> Option<NodeValue> {
		let keys = &self.keys;
		if keys.is_empty() {
			return None;
		}

		// This time precedes any keyframe -> first value.
		if keys[0].time >= time {
			return Some(keys[0].value.clone());
		}

		// This time is after any keyframes -> last value.
		if keys[keys.len() - 1].time <= time {
			return Some(keys[keys.len() - 1].value.clone());
		}

		// The time must be somewhere in between: binary search for the
		// bracketing pair (C++ low/high loop).
		let mut low = 0usize;
		let mut high = keys.len() - 1;
		let mut before: Option<(usize, usize)> = None;
		while low <= high {
			let mid = low + (high - low) / 2;
			let mid_key = &keys[mid];
			let next_key = &keys[mid + 1];

			if mid_key.time <= time && next_key.time > time {
				before = Some((mid, mid + 1));
				break;
			} else if mid_key.time < time {
				low = mid + 1;
			} else {
				high = mid - 1;
			}
		}

		let (b, a) = before?;
		let before_key = &keys[b];
		let after_key = &keys[a];

		if before_key.time == time {
			return Some(before_key.value.clone());
		}

		if !before_key.value.can_interpolate() || before_key.interpolation == Interpolation::Hold {
			// Non-interpolable or hold: the value stays at `before` until
			// the next keyframe (C++: `after->time() > time` guaranteed here).
			return Some(before_key.value.clone());
		}

		if after_key.time == time {
			return Some(after_key.value.clone());
		}

		if before_key.time < time && after_key.time > time {
			return Some(interpolate(before_key, after_key, time));
		}

		None
	}

	/// Sorted keyframes view.
	pub fn keys(&self) -> &[Keyframe] {
		&self.keys
	}
}

/// Interpolate between two keyframes at `time` (strictly between their
/// times). Ports the C++ three-way branch: cubic bezier (both bezier),
/// quadratic bezier (one bezier), linear (both linear).
///
/// The C++ path interpolates per component track along a shared
/// parametric `t` derived from the time axis; the Rust track holds whole
/// values, so the components are split, evaluated at `t` with the shared
/// handle y-offsets, and recombined.
fn interpolate(before: &Keyframe, after: &Keyframe, time: Rational) -> NodeValue {
	let declared = before.value.value_type();
	let before_val = before.value.to_double();
	let after_val = after.value.to_double();

	let both_bezier = before.interpolation == Interpolation::Bezier
		&& after.interpolation == Interpolation::Bezier;
	let one_bezier = before.interpolation == Interpolation::Bezier
		|| after.interpolation == Interpolation::Bezier;

	if !both_bezier && !one_bezier {
		// Both linear.
		let period_progress =
			(time.to_f64() - before.time.to_f64()) / (after.time.to_f64() - before.time.to_f64());
		return before.value.lerp(&after.value, period_progress);
	}

	// Shared parametric t from the time axis, plus the handle y-offsets.
	let (t, bcp_y, acp_y, quad_before) = if both_bezier {
		let (cp1_x, cp1_y) = valid_bezier_out(before, after.time);
		let (cp2_x, cp2_y) = valid_bezier_in(after, before.time);
		let t = cubic_xto_t(
			time.to_f64(),
			before.time.to_f64(),
			before.time.to_f64() + cp1_x,
			after.time.to_f64() + cp2_x,
			after.time.to_f64(),
		);
		(t, cp1_y, cp2_y, false)
	} else if before.interpolation == Interpolation::Bezier {
		let (x, y) = valid_bezier_out(before, after.time);
		let t = quadratic_xto_t(
			time.to_f64(),
			before.time.to_f64(),
			before.time.to_f64() + x,
			after.time.to_f64(),
		);
		(t, y, 0.0, true)
	} else {
		let (x, y) = valid_bezier_in(after, before.time);
		let t = quadratic_xto_t(
			time.to_f64(),
			before.time.to_f64(),
			after.time.to_f64() + x,
			after.time.to_f64(),
		);
		(t, 0.0, y, true)
	};

	let eval = |bv: f64, av: f64| -> f64 {
		if both_bezier {
			cubic_tto_y(bv, bv + bcp_y, av + acp_y, av, t)
		} else if quad_before {
			quadratic_tto_y(bv, bv + bcp_y, av, t)
		} else {
			quadratic_tto_y(bv, av + acp_y, av, t)
		}
	};

	if declared == ValueType::Rational {
		// Rational inputs re-quantize through from_double (C++ k_rational
		// path).
		let y = eval(before_val, after_val);
		return NodeValue::Rational(Rational::from_double(y));
	}

	// Split into per-component tracks, evaluate each, recombine.
	let b_tracks = before.value.split_into_tracks(declared);
	let a_tracks = after.value.split_into_tracks(declared);
	let out: Vec<NodeValue> = b_tracks
		.iter()
		.zip(a_tracks.iter())
		.map(|(bv, av)| {
			if declared == ValueType::Color
				|| declared == ValueType::Vec2
				|| declared == ValueType::Vec3
				|| declared == ValueType::Vec4
			{
				NodeValue::Float(eval(bv.to_double(), av.to_double()))
			} else {
				// Scalar types: the whole value is the single component.
				before
					.value
					.lerp(&after.value, 0.0)
					.with_scalar(declared, eval(bv.to_double(), av.to_double()))
			}
		})
		.collect();
	NodeValue::combine_tracks(&out, declared)
}

/// The keyframe's out-handle clamped so the curve never overlaps the
/// next keyframe's time (C++
/// `NodeKeyframe::valid_bezier_control_out`).
fn valid_bezier_out(key: &Keyframe, next_time: Rational) -> (f64, f64) {
	let t = key.time.to_f64();
	let adjusted_x = (t + key.bezier_out.0).min(next_time.to_f64());
	(adjusted_x - t, key.bezier_out.1)
}

/// The keyframe's in-handle clamped so the curve never overlaps the
/// previous keyframe's time (C++
/// `NodeKeyframe::valid_bezier_control_in`).
fn valid_bezier_in(key: &Keyframe, prev_time: Rational) -> (f64, f64) {
	let t = key.time.to_f64();
	let adjusted_x = (t + key.bezier_in.0).max(prev_time.to_f64());
	(adjusted_x - t, key.bezier_in.1)
}

/// Bezier solver helpers (ported verbatim from
/// `core/src/oliveimpl/util/bezier.h` / `core/src/util/bezier.cpp`).

/// Cubic `x(t)` -> `t` by binary search (`calculate_t_from_x`, cubic).
fn cubic_xto_t(x: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
	// Clamp to prevent infinite loop.
	let x = x.clamp(a.min(d), a.max(d));
	calculate_t_from_x(true, x, a, b, c, d)
}

/// Quadratic `x(t)` -> `t` by binary search (`calculate_t_from_x`,
/// quadratic).
fn quadratic_xto_t(x: f64, a: f64, b: f64, c: f64) -> f64 {
	let x = x.clamp(a.min(c), a.max(c));
	calculate_t_from_x(false, x, a, b, c, 0.0)
}

/// The C++ `Bezier::calculate_t_from_x` bisection loop.
fn calculate_t_from_x(cubic: bool, x: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
	let mut bottom = 0.0;
	let mut top = 1.0;

	loop {
		if bottom == top {
			return bottom;
		}

		let mid = (bottom + top) * 0.5;
		let test = if cubic {
			cubic_tto_y(a, b, c, d, mid)
		} else {
			quadratic_tto_y(a, b, c, mid)
		};

		if (test - x).abs() < 0.000001 {
			return mid;
		} else if x > test {
			bottom = mid;
		} else {
			top = mid;
		}
	}
}

/// `(1-t)^2*a + 2*(1-t)*t*b + t^2*c`.
fn quadratic_tto_y(a: f64, b: f64, c: f64, t: f64) -> f64 {
	(1.0 - t).powi(2) * a + 2.0 * (1.0 - t) * t * b + t.powi(2) * c
}

/// `(1-t)^3*a + 3*(1-t)^2*t*b + 3*(1-t)*t^2*c + t^3*d`.
fn cubic_tto_y(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
	(1.0 - t).powi(3) * a
		+ 3.0 * (1.0 - t).powi(2) * t * b
		+ 3.0 * (1.0 - t) * t.powi(2) * c
		+ t.powi(3) * d
}

//! Pure slider state machine: range, stepping, clamping and drag math.
//!
//! This module has no gpui coupling so every behavior is unit-testable
//! (see the test module at the bottom). The [`Slider`](crate::widgets::slider::Slider)
//! view drives these methods from mouse/wheel/keyboard gestures.

use crate::widgets::value::{RationalValue, SliderValue, ValueKind};

/// The pure state of a slider.
///
/// The canonical numeric position is [`SliderModel::raw`]. For
/// [`ValueKind::Rational`] the raw value is the *numerator* over
/// [`SliderModel::rational_den`], so rational values never pass through
/// floating point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderModel {
	/// The family of values this slider edits.
	pub kind: ValueKind,
	/// Canonical numeric position (for `Rational`: the numerator).
	pub raw: f64,
	/// Inclusive lower bound of `raw`.
	pub min: f64,
	/// Inclusive upper bound of `raw`.
	pub max: f64,
	/// Coarse increment (wheel notch / arrow key).
	pub step: f64,
	/// Fine increment used with a fine-adjust modifier.
	pub fine_step: f64,
	/// Value restored by a middle-click reset.
	pub default_raw: f64,
	/// Denominator for `Rational` values (unused otherwise).
	pub rational_den: i64,
}

impl SliderModel {
	/// Create a slider over `[min, max]` with `step` increments.
	///
	/// For `Rational` sliders `min`, `max`, `step` and `default_raw` are in
	/// numerator units; pair with [`Self::with_rational_den`].
	pub fn new(kind: ValueKind, min: f64, max: f64, step: f64, default_raw: f64) -> Self {
		let raw = default_raw.clamp(min, max);
		Self {
			kind,
			raw,
			min,
			max,
			step,
			fine_step: step / 10.0,
			default_raw: default_raw.clamp(min, max),
			rational_den: 1,
		}
	}

	/// Set the denominator of a `Rational` slider (a no-op otherwise).
	pub fn with_rational_den(mut self, den: i64) -> Self {
		if self.kind == ValueKind::Rational {
			self.rational_den = den.max(1);
		}
		self
	}

	/// Overrides the fine-adjust increment.
	///
	/// `fine_step` replaces the default derived in [`Self::new`]
	/// (`step / 10`) and is the increment used while a fine-adjust modifier is
	/// held. The value is stored as given: zero or negative makes fine
	/// snapping a no-op (the position is only clamped). Consumes and returns
	/// `self` so it can be chained onto [`Self::new`].
	pub fn with_fine_step(mut self, fine_step: f64) -> Self {
		self.fine_step = fine_step;
		self
	}

	/// The current value.
	pub fn value(&self) -> SliderValue {
		match self.kind {
			ValueKind::Float => SliderValue::Float(self.raw),
			ValueKind::Integer => SliderValue::Integer(self.raw as i64),
			ValueKind::Rational => SliderValue::Rational(RationalValue::new_or_one(
				self.raw as i64,
				self.rational_den,
			)),
			ValueKind::Angle => SliderValue::Angle(self.raw),
		}
	}

	/// Whether `v` lies within `[min, max]`.
	///
	/// Both bounds are inclusive and `v` must be finite, so NaN and ±infinity
	/// are always reported as out of range. Used to vet externally supplied
	/// values (typed input, algebra results) before they are snapped and
	/// stored.
	pub fn in_range(&self, v: f64) -> bool {
		v.is_finite() && v >= self.min && v <= self.max
	}

	/// Clamps a raw position into `[min, max]`.
	///
	/// Both bounds are inclusive, and values outside the range come back as
	/// exactly the bound. Used by [`Self::snap_with_step`] and by the drag and
	/// wheel handlers before a position is committed.
	///
	/// # Panics
	///
	/// Panics (like `f64::clamp`) when `min > max` or either bound is NaN;
	/// [`Self::new`] does not validate the bounds.
	pub fn clamp(&self, raw: f64) -> f64 {
		raw.clamp(self.min, self.max)
	}

	/// Snap a raw position to the nearest multiple of `step`, measured from
	/// `min`. Positions outside the range snap back to the bounds.
	fn snap_with_step(&self, raw: f64, step: f64) -> f64 {
		let raw = self.clamp(raw);
		if step <= 0.0 || self.spanned() == 0.0 {
			return raw;
		}
		let steps = ((raw - self.min) / step).round();
		self.clamp(self.min + steps * step)
	}

	/// Snap a raw position to the nearest coarse step, measured from `min`.
	pub fn snap(&self, raw: f64) -> f64 {
		self.snap_with_step(raw, self.step)
	}

	/// The width of the range (`max - min`, never negative).
	pub fn spanned(&self) -> f64 {
		(self.max - self.min).max(0.0)
	}

	/// Set the position from a raw value: clamps and snaps to `step`,
	/// returns whether the position changed.
	pub fn apply_with_step(&mut self, raw: f64, step: f64) -> bool {
		if !raw.is_finite() {
			return false;
		}
		let snapped = self.snap_with_step(raw, step);
		if (snapped - self.raw).abs() > f64::EPSILON {
			self.raw = snapped;
			true
		} else {
			false
		}
	}

	/// Set the position from a raw value: clamps and snaps to the coarse
	/// step, returns whether the position changed.
	pub fn apply_raw(&mut self, raw: f64) -> bool {
		self.apply_with_step(raw, self.step)
	}

	/// Set the value, converting between representations where possible.
	/// Returns whether the position changed. Non-finite input is rejected.
	pub fn set_value(&mut self, value: SliderValue) -> bool {
		let finite = match value {
			SliderValue::Float(v) | SliderValue::Angle(v) => v.is_finite(),
			SliderValue::Integer(_) | SliderValue::Rational(_) => true,
		};
		if !finite {
			return false;
		}
		let raw = match (self.kind, value) {
			(ValueKind::Float, SliderValue::Float(v))
			| (ValueKind::Angle, SliderValue::Angle(v)) => v,
			(ValueKind::Integer, SliderValue::Integer(v)) => v as f64,
			(ValueKind::Rational, SliderValue::Rational(v)) => {
				(v.num() as f64) * (self.rational_den as f64) / (v.den() as f64)
			}
			// Cross-kind conversions go through the numeric projection.
			(kind, value) => {
				let v = value.to_f64();
				match kind {
					ValueKind::Integer => v.trunc(),
					ValueKind::Rational => {
						(v * self.rational_den as f64).round() / self.rational_den as f64
					}
					_ => v,
				}
			}
		};
		self.apply_raw(raw)
	}

	/// The next position `delta` coarse (or fine) steps away from `from`,
	/// clamped to the range.
	pub fn step_from(&self, from: f64, delta: i32, fine: bool) -> f64 {
		let step = if fine { self.fine_step } else { self.step };
		if step <= 0.0 {
			return self.clamp(from);
		}
		self.clamp(from + delta as f64 * step)
	}

	/// Step the current position by `delta` steps, snapping to the effective
	/// (coarse or fine) step.
	pub fn apply_step(&mut self, delta: i32, fine: bool) -> bool {
		let step = if fine { self.fine_step } else { self.step };
		self.apply_with_step(self.step_from(self.raw, delta, fine), step)
	}

	/// Reset to the default position; returns whether it changed.
	pub fn reset(&mut self) -> bool {
		let snapped = self.snap(self.default_raw);
		if (snapped - self.raw).abs() > f64::EPSILON {
			self.raw = snapped;
			true
		} else {
			false
		}
	}

	/// Whether the current position equals the (snapped) default.
	pub fn is_at_default(&self) -> bool {
		(self.snap(self.default_raw) - self.raw).abs() <= f64::EPSILON
	}

	/// Normalized position in `0..=1` for painting. Returns `0.5` when the
	/// range is empty so the handle never leaves the track.
	pub fn fraction(&self) -> f64 {
		if self.spanned() == 0.0 {
			return 0.5;
		}
		((self.raw - self.min) / self.spanned()).clamp(0.0, 1.0)
	}

	/// Set the position from a normalized `0..=1` fraction of the range.
	pub fn set_fraction(&mut self, t: f64) -> bool {
		if !t.is_finite() {
			return false;
		}
		let raw = self.min + t.clamp(0.0, 1.0) * self.spanned();
		self.apply_raw(raw)
	}

	/// Apply a vertical drag of `dy_px` pixels over a track that spans
	/// `range_px` pixels. Dragging up (`dy_px < 0`) increases the value.
	/// `fine` scales the motion by `1/10` and snaps to the fine step.
	pub fn drag_delta(&mut self, dy_px: f32, range_px: f32, fine: bool) -> bool {
		if range_px <= 0.0 {
			return false;
		}
		let scale = if fine { 0.1 } else { 1.0 };
		let step = if fine { self.fine_step } else { self.step };
		let delta = -(dy_px as f64) / range_px as f64 * self.spanned() * scale;
		self.apply_with_step(self.raw + delta, step)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn float_slider() -> SliderModel {
		SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.1, 0.5)
	}

	#[test]
	fn new_clamps_default() {
		let model = SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.1, 5.0);
		assert_eq!(model.raw, 1.0);
		assert!(model.is_at_default());
	}

	#[test]
	fn clamps_to_range() {
		let model = float_slider();
		assert_eq!(model.clamp(-2.0), 0.0);
		assert_eq!(model.clamp(2.0), 1.0);
		assert!(model.in_range(0.5));
		assert!(!model.in_range(1.5));
		assert!(!model.in_range(f64::NAN));
	}

	#[test]
	fn snaps_to_step_from_min() {
		let model = float_slider();
		assert!((model.snap(0.27) - 0.3).abs() < 1e-9);
		assert!((model.snap(0.24) - 0.2).abs() < 1e-9);
		// Off-range values snap back to the bounds.
		assert!((model.snap(-5.0) - 0.0).abs() < 1e-9);
		assert!((model.snap(5.0) - 1.0).abs() < 1e-9);
	}

	#[test]
	fn snaps_relative_to_nonzero_min() {
		let model = SliderModel::new(ValueKind::Float, 0.5, 2.5, 0.5, 1.0);
		// (1.15 - 0.5) / 0.5 = 1.3 -> round to 1 -> 0.5 + 0.5 = 1.0
		assert!((model.snap(1.15) - 1.0).abs() < 1e-9);
		// (1.35 - 0.5) / 0.5 = 1.7 -> round to 2 -> 1.5
		assert!((model.snap(1.35) - 1.5).abs() < 1e-9);
	}

	#[test]
	fn apply_raw_rejects_nan() {
		let mut model = float_slider();
		assert!(!model.apply_raw(f64::NAN));
		assert_eq!(model.raw, 0.5);
	}

	#[test]
	fn step_from_clamps() {
		let model = float_slider();
		assert!((model.step_from(0.95, 1, false) - 1.0).abs() < 1e-9);
		assert!((model.step_from(0.05, -1, false) - 0.0).abs() < 1e-9);
		assert!((model.step_from(0.5, 1, false) - 0.6).abs() < 1e-9);
		// Fine step is 1/10 of the coarse step.
		assert!((model.step_from(0.5, 1, true) - 0.51).abs() < 1e-9);
	}

	#[test]
	fn reset_restores_default() {
		let mut model = float_slider();
		model.apply_raw(0.9);
		assert!(!model.is_at_default());
		assert!(model.reset());
		assert_eq!(model.raw, 0.5);
		assert!(model.is_at_default());
	}

	#[test]
	fn fraction_and_set_fraction_are_inverse() {
		let mut model = float_slider();
		assert_eq!(model.fraction(), 0.5);
		model.set_fraction(0.0);
		assert_eq!(model.raw, 0.0);
		model.set_fraction(1.0);
		assert_eq!(model.raw, 1.0);
		// Positions are quantized to the step: 0.25 lands on the nearest
		// representable value (0.3 with a 0.1 step).
		model.set_fraction(0.25);
		assert!((model.raw - 0.3).abs() < 1e-9);
		assert!((model.fraction() - 0.3).abs() < 1e-9);
	}

	#[test]
	fn fraction_when_range_is_empty() {
		let model = SliderModel::new(ValueKind::Float, 1.0, 1.0, 0.1, 1.0);
		assert_eq!(model.fraction(), 0.5);
	}

	#[test]
	fn drag_up_increases_value() {
		let mut model = float_slider();
		// 10px up over a 100px track spans 10% of the range.
		assert!(model.drag_delta(-10.0, 100.0, false));
		assert!((model.raw - 0.6).abs() < 1e-9);
		// Fine drag moves 10x less.
		let mut fine = float_slider();
		fine.drag_delta(-10.0, 100.0, true);
		assert!((fine.raw - 0.51).abs() < 1e-9);
		// Dragging up past the max clamps.
		let mut model = float_slider();
		model.drag_delta(-1000.0, 100.0, false);
		assert_eq!(model.raw, 1.0);
	}

	#[test]
	fn integer_kind_round_trips_exactly() {
		let mut model = SliderModel::new(ValueKind::Integer, -10.0, 10.0, 1.0, 0.0);
		assert_eq!(model.value(), SliderValue::Integer(0));
		model.set_value(SliderValue::Integer(5));
		assert_eq!(model.value(), SliderValue::Integer(5));
		model.apply_step(1, false);
		assert_eq!(model.value(), SliderValue::Integer(6));
		// A float value converts by truncation.
		model.set_value(SliderValue::Float(3.9));
		assert_eq!(model.value(), SliderValue::Integer(3));
	}

	#[test]
	fn rational_kind_stays_exact() {
		let mut model =
			SliderModel::new(ValueKind::Rational, 0.0, 24.0, 1.0, 0.0).with_rational_den(24);
		assert_eq!(
			model.value(),
			SliderValue::Rational(RationalValue::new(0, 1).unwrap())
		);
		model.apply_raw(1.0);
		assert_eq!(
			model.value(),
			SliderValue::Rational(RationalValue::new(1, 24).unwrap())
		);
		model.apply_raw(24.0);
		assert_eq!(
			model.value(),
			SliderValue::Rational(RationalValue::new(1, 1).unwrap())
		);
		// Setting a rational with a different denominator rescales.
		model.set_value(SliderValue::Rational(RationalValue::new(1, 2).unwrap()));
		assert_eq!(
			model.value(),
			SliderValue::Rational(RationalValue::new(1, 2).unwrap())
		);
	}

	#[test]
	fn angle_kind_round_trips() {
		let mut model = SliderModel::new(ValueKind::Angle, 0.0, 360.0, 0.5, 45.0);
		assert_eq!(model.value(), SliderValue::Angle(45.0));
		model.set_value(SliderValue::Angle(270.5));
		assert_eq!(model.value(), SliderValue::Angle(270.5));
	}

	#[test]
	fn set_value_rejects_non_finite() {
		let mut model = float_slider();
		assert!(!model.set_value(SliderValue::Float(f64::NAN)));
		assert!(!model.set_value(SliderValue::Angle(f64::INFINITY)));
		assert_eq!(model.raw, 0.5);
	}

	#[test]
	fn integer_fine_step_is_whole() {
		let model = SliderModel::new(ValueKind::Integer, 0.0, 10.0, 1.0, 5.0);
		// Fine step for integers is 1/10 by default; snapping keeps it whole.
		let snapped = model.snap(model.step_from(5.0, 1, true));
		assert_eq!(snapped, 5.0);
	}
}

//! Pure numeric value types shared by the form controls.
//!
//! These types carry no gpui dependency: they are the unit-testable core of
//! the slider / spinbox / color widgets. The key invariant is that a
//! [`SliderValue`] round-trips through [`SliderValue::to_f64`] and
//! [`SliderValue::from_f64`] losslessly for the kinds where exactness matters
//! (integers and rationals), so repeated edits cannot accumulate drift.

use gpui_kit::SharedString;

/// The family of values a control can edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
	/// A real number.
	Float,
	/// A whole number.
	Integer,
	/// An exact fraction `num / den`.
	Rational,
	/// An angle in degrees.
	Angle,
}

/// The value edited by a form control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliderValue {
	/// A real number.
	Float(f64),
	/// A whole number.
	Integer(i64),
	/// An exact fraction.
	Rational(RationalValue),
	/// An angle in degrees.
	Angle(f64),
}

impl SliderValue {
	/// The [`ValueKind`] of this value.
	pub fn kind(self) -> ValueKind {
		match self {
			SliderValue::Float(_) => ValueKind::Float,
			SliderValue::Integer(_) => ValueKind::Integer,
			SliderValue::Rational(_) => ValueKind::Rational,
			SliderValue::Angle(_) => ValueKind::Angle,
		}
	}

	/// The numeric projection used for painting and interpolation.
	///
	/// For [`SliderValue::Rational`] this is `num / den` as `f64`; the exact
	/// pair is preserved by [`SliderValue::Rational`] itself.
	pub fn to_f64(self) -> f64 {
		match self {
			SliderValue::Float(v) => v,
			SliderValue::Integer(v) => v as f64,
			SliderValue::Rational(v) => v.to_f64(),
			SliderValue::Angle(v) => v,
		}
	}

	/// Rebuild a value from its numeric projection.
	///
	/// [`SliderValue::Integer`] truncates toward zero and
	/// [`SliderValue::Rational`] reconstructs the closest fraction with a
	/// bounded denominator, so this is only lossless when the source value
	/// was exact to begin with.
	pub fn from_f64(kind: ValueKind, value: f64) -> Self {
		match kind {
			ValueKind::Float => SliderValue::Float(value),
			ValueKind::Integer => SliderValue::Integer(value.trunc() as i64),
			ValueKind::Rational => SliderValue::Rational(RationalValue::from_f64(value, 1_000_000)),
			ValueKind::Angle => SliderValue::Angle(value),
		}
	}
}

/// An exact rational number, kept normalized (`den > 0`, `gcd(|num|, den) == 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RationalValue {
	num: i64,
	den: i64,
}

/// Error returned when a rational cannot be constructed from raw parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RationalError {
	/// The denominator must not be zero.
	#[error("rational denominator must not be zero")]
	ZeroDenominator,
}

impl RationalValue {
	/// The zero rational `0/1`.
	pub const ZERO: Self = Self { num: 0, den: 1 };
	/// The one rational `1/1`.
	pub const ONE: Self = Self { num: 1, den: 1 };

	/// Construct a normalized rational, rejecting a zero denominator.
	///
	/// # Arguments
	///
	/// * `num` - Numerator; may be negative.
	/// * `den` - Denominator; must be nonzero. A negative denominator is moved
	///   onto the numerator before reduction.
	///
	/// # Errors
	///
	/// Returns [`RationalError::ZeroDenominator`] when `den == 0`. On success
	/// the fraction is fully reduced with a positive denominator, so equal
	/// fractions compare equal (e.g. `4/6` == `2/3`); use
	/// [`RationalValue::new_or_one`] when a zero denominator should be clamped
	/// rather than reported.
	pub fn new(num: i64, den: i64) -> Result<Self, RationalError> {
		if den == 0 {
			return Err(RationalError::ZeroDenominator);
		}
		Ok(Self::new_unchecked(num, den))
	}

	/// Construct a normalized rational like [`RationalValue::new`], but map a
	/// zero denominator to `1` instead of failing.
	///
	/// # Arguments
	///
	/// * `num` - Numerator; may be negative.
	/// * `den` - Denominator; `0` is clamped to `1`, so `new_or_one(n, 0)`
	///   becomes the integer rational `n/1`.
	///
	/// Used on paths where a missing or zero denominator must degrade
	/// gracefully rather than surface [`RationalError::ZeroDenominator`]; the
	/// result is always normalized like [`RationalValue::new`].
	pub fn new_or_one(num: i64, den: i64) -> Self {
		if den == 0 {
			return Self { num, den: 1 };
		}
		Self::new_unchecked(num, den)
	}

	/// Normalizes `num/den` without the zero-denominator check: a negative
	/// denominator is moved onto the numerator, then both are divided by their
	/// gcd so the result has a positive denominator and is fully reduced.
	///
	/// Private back end of [`RationalValue::new`] and
	/// [`RationalValue::new_or_one`]; `den` must therefore be nonzero. A zero
	/// denominator would divide by the zero gcd and panic, so callers validate
	/// first.
	fn new_unchecked(num: i64, den: i64) -> Self {
		let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
		let gcd = num.unsigned_abs().gcd(den as u64) as i64;
		Self {
			num: num / gcd,
			den: den / gcd,
		}
	}

	/// The numerator of the normalized fraction.
	pub fn num(self) -> i64 {
		self.num
	}

	/// The denominator of the normalized fraction (always positive).
	pub fn den(self) -> i64 {
		self.den
	}

	/// The rational's numeric value as `f64` (`num / den`), used for painting,
	/// interpolation and [`SliderValue::to_f64`].
	///
	/// The division is inexact for most fractions (`1/3` becomes the nearest
	/// `f64`), so a `f64` round-trip through [`RationalValue::from_f64`] is
	/// lossy; the exact pair stays available via [`RationalValue::num`] /
	/// [`RationalValue::den`].
	pub fn to_f64(self) -> f64 {
		self.num as f64 / self.den as f64
	}

	/// The closest rational with `1 <= den <= max_den`, via continued
	/// fractions. `NaN` and infinities map to the zero rational; negative
	/// values keep their sign.
	pub fn from_f64(value: f64, max_den: i64) -> Self {
		if !value.is_finite() {
			return Self::ZERO;
		}
		let max_den = max_den.max(1);
		let negative = value < 0.0;
		let value = value.abs();
		// Continued fraction convergents with bounded denominator.
		let mut n0 = 0i64;
		let mut d0 = 1i64;
		let mut n1 = 1i64;
		let mut d1 = 0i64;
		let mut remainder = value;
		for _ in 0..64 {
			let a = remainder.floor() as i64;
			let n2 = a.saturating_mul(n1).saturating_add(n0);
			let d2 = a.saturating_mul(d1).saturating_add(d0);
			if d2 > max_den {
				break;
			}
			n0 = n1;
			d0 = d1;
			n1 = n2;
			d1 = d2;
			let frac = remainder - remainder.floor();
			if frac.abs() < 1e-12 {
				break;
			}
			remainder = 1.0 / frac;
		}
		let (num, den) = if negative { (-n1, d1) } else { (n1, d1) };
		Self::new_unchecked(num, den)
	}
}

/// A failure to parse a value from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseValueError {
	/// The text did not match the expected format for this value kind.
	#[error("could not parse {0:?} as {1:?}")]
	InvalidFormat(SharedString, ValueKind),
}

/// Formats and parses a [`SliderValue`] for display and direct entry.
///
/// Implementations are injected into controls so hosts can localize or
/// specialize the representation (e.g. timecode instead of frames, or
/// fractions with a fixed denominator).
pub trait ValueFormatter: 'static {
	/// Render a value as text.
	fn format(&self, value: SliderValue) -> SharedString;
	/// Parse text into a value. Must reject malformed input with
	/// [`ParseValueError`]; controls refuse to apply rejected input.
	fn parse(&self, text: &str) -> Result<SliderValue, ParseValueError>;
}

/// The default formatter:
///
/// * `Float` — up to three decimal digits, trailing zeros trimmed.
/// * `Integer` — plain decimal.
/// * `Rational` — `num/den`.
/// * `Angle` — degrees with a trailing `°`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultFormatter;

impl ValueFormatter for DefaultFormatter {
	fn format(&self, value: SliderValue) -> SharedString {
		match value {
			SliderValue::Float(v) => {
				if !v.is_finite() {
					return "0".into();
				}
				let rounded = (v * 1000.0).round() / 1000.0;
				let mut text = format!("{rounded}");
				if text.contains('.') {
					while text.ends_with('0') {
						text.pop();
					}
					if text.ends_with('.') {
						text.pop();
					}
				}
				text.into()
			}
			SliderValue::Integer(v) => format!("{v}").into(),
			SliderValue::Rational(v) => format!("{}/{}", v.num(), v.den()).into(),
			SliderValue::Angle(v) => {
				let rounded = (v * 1000.0).round() / 1000.0;
				format!("{rounded}°").into()
			}
		}
	}

	fn parse(&self, text: &str) -> Result<SliderValue, ParseValueError> {
		let text = text.trim();
		let kind = |reason: SharedString| ParseValueError::InvalidFormat(reason, ValueKind::Float);
		if let Some((num, den)) = text.split_once('/') {
			let num: i64 = num
				.trim()
				.parse()
				.map_err(|_| ParseValueError::InvalidFormat(text.into(), ValueKind::Rational))?;
			let den: i64 = den
				.trim()
				.parse()
				.map_err(|_| ParseValueError::InvalidFormat(text.into(), ValueKind::Rational))?;
			return RationalValue::new(num, den)
				.map(SliderValue::Rational)
				.map_err(|_| ParseValueError::InvalidFormat(text.into(), ValueKind::Rational));
		}
		let (body, is_angle) = text
			.strip_suffix('°')
			.map(|body| (body, true))
			.or_else(|| {
				text.strip_suffix("deg")
					.or_else(|| text.strip_suffix("DEG"))
					.map(|body| (body, true))
			})
			.unwrap_or((text, false));
		if is_angle {
			let value: f64 = body
				.trim()
				.parse()
				.map_err(|_| ParseValueError::InvalidFormat(text.into(), ValueKind::Angle))?;
			return Ok(SliderValue::Angle(value));
		}
		// An exact decimal with no fractional part parses as an integer when
		// there are no digits after the point; otherwise as a float.
		let value: f64 = body.trim().parse().map_err(|_| kind(text.into()))?;
		if value.fract() == 0.0 && value.abs() < i64::MAX as f64 {
			Ok(SliderValue::Integer(value as i64))
		} else {
			Ok(SliderValue::Float(value))
		}
	}
}

/// Extension: `u64::gcd` used by [`RationalValue`].
trait Gcd {
	fn gcd(self, other: Self) -> Self;
}

impl Gcd for u64 {
	fn gcd(mut self, mut other: Self) -> Self {
		while other != 0 {
			(self, other) = (other, self % other);
		}
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rational_normalizes() {
		assert_eq!(
			RationalValue::new(4, 6).unwrap(),
			RationalValue::new(2, 3).unwrap()
		);
		assert_eq!(RationalValue::new(2, 3).unwrap().num(), 2);
		assert_eq!(RationalValue::new(2, 3).unwrap().den(), 3);
		assert_eq!(
			RationalValue::new(-2, 4).unwrap(),
			RationalValue::new(-1, 2).unwrap()
		);
		assert_eq!(
			RationalValue::new(1, -2).unwrap(),
			RationalValue::new(-1, 2).unwrap()
		);
	}

	#[test]
	fn rational_rejects_zero_denominator() {
		assert_eq!(
			RationalValue::new(1, 0),
			Err(RationalError::ZeroDenominator)
		);
		assert_eq!(
			RationalValue::new_or_one(1, 0),
			RationalValue::new(1, 1).unwrap()
		);
	}

	#[test]
	fn rational_from_f64_exact() {
		assert_eq!(
			RationalValue::from_f64(0.5, 1000),
			RationalValue::new(1, 2).unwrap()
		);
		assert_eq!(
			RationalValue::from_f64(0.25, 1000),
			RationalValue::new(1, 4).unwrap()
		);
		assert_eq!(
			RationalValue::from_f64(2.0, 1000),
			RationalValue::new(2, 1).unwrap()
		);
		assert_eq!(
			RationalValue::from_f64(-1.5, 1000),
			RationalValue::new(-3, 2).unwrap()
		);
		assert_eq!(
			RationalValue::from_f64(0.3333333333, 1000),
			RationalValue::new(1, 3).unwrap()
		);
		assert_eq!(RationalValue::from_f64(f64::NAN, 1000), RationalValue::ZERO);
		assert_eq!(
			RationalValue::from_f64(f64::INFINITY, 1000),
			RationalValue::ZERO
		);
	}

	#[test]
	fn rational_from_f64_bounded_denominator() {
		// 1/999983 needs a huge denominator; the bounded approximation may be
		// 0 but must never exceed the bound or produce a negative denominator.
		let value = RationalValue::from_f64(1.0 / 999983.0, 1000);
		assert!(value.den() <= 1000);
		assert!(value.den() > 0);
		// A friendlier irrational keeps a nonzero approximation.
		let approx = RationalValue::from_f64(std::f64::consts::FRAC_1_PI, 1000);
		assert!(approx.num() > 0);
		assert!(approx.den() <= 1000);
		assert!((approx.to_f64() - std::f64::consts::FRAC_1_PI).abs() < 1e-3);
	}

	#[test]
	fn value_kind_projection() {
		assert_eq!(SliderValue::Integer(-3).to_f64(), -3.0);
		assert_eq!(
			SliderValue::Rational(RationalValue::new(1, 3).unwrap()).to_f64(),
			1.0 / 3.0
		);
		assert_eq!(
			SliderValue::from_f64(ValueKind::Integer, 3.9),
			SliderValue::Integer(3)
		);
		assert_eq!(
			SliderValue::from_f64(ValueKind::Rational, 0.5),
			SliderValue::Rational(RationalValue::new(1, 2).unwrap())
		);
		assert_eq!(
			SliderValue::from_f64(ValueKind::Angle, 90.0),
			SliderValue::Angle(90.0)
		);
	}

	#[test]
	fn default_formatter_round_trips() {
		let formatter = DefaultFormatter;
		for value in [
			SliderValue::Float(1.5),
			SliderValue::Integer(-42),
			SliderValue::Rational(RationalValue::new(1, 3).unwrap()),
			SliderValue::Angle(90.0),
		] {
			let text = formatter.format(value);
			let parsed = formatter.parse(text.as_ref()).unwrap();
			assert_eq!(parsed.kind(), value.kind());
		}
	}

	#[test]
	fn default_formatter_trims_and_truncates() {
		let formatter = DefaultFormatter;
		assert_eq!(formatter.format(SliderValue::Float(1.5)), "1.5");
		assert_eq!(formatter.format(SliderValue::Float(0.123456)), "0.123");
		assert_eq!(formatter.format(SliderValue::Float(2.0)), "2");
		assert_eq!(formatter.format(SliderValue::Angle(45.5)), "45.5°");
		assert_eq!(formatter.format(SliderValue::Integer(7)), "7");
	}

	#[test]
	fn default_formatter_rejects_invalid_input() {
		let formatter = DefaultFormatter;
		assert!(formatter.parse("abc").is_err());
		assert!(formatter.parse("1/0").is_err());
		assert!(formatter.parse("12/ab").is_err());
		assert!(formatter.parse("").is_err());
		assert!(formatter.parse("90°abc").is_err());
	}

	#[test]
	fn default_formatter_parses_angle_suffix() {
		let formatter = DefaultFormatter;
		assert_eq!(formatter.parse("90°").unwrap(), SliderValue::Angle(90.0));
		assert_eq!(formatter.parse("45 deg").unwrap(), SliderValue::Angle(45.0));
		assert_eq!(
			formatter.parse("180 DEG").unwrap(),
			SliderValue::Angle(180.0)
		);
	}
}

//! Timecode discipline for the tool face.
//!
//! Two accepted shapes only (OPP/1 section 5 + the T0 schema freeze):
//!
//! * rational seconds `{num, den}` — reduced, `den > 0` after sign
//!   normalisation (`den == 0` is a hard rejection);
//! * a non-negative integer frame index on the target's frame grid.
//!
//! Bare floating-point seconds (`1.5`, `29.97`), timecode strings
//! (`"00:00:01.000"`) and any other shape are rejected **at the parsing
//! layer** — the [`Deserialize`] impl refuses `f64` outright — which is a
//! second gate behind the frozen JSON Schema (draft-07 `oneOf`), not a
//! replacement for it. Note that JSON Schema cannot tell `1` from `1.0`,
//! so this layer is where `1.0` actually dies.

use std::cmp::Ordering;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ToolError;

/// Greatest common divisor (sign-normalising, `i128` so the intermediate
/// products of two `i64` rationals cannot overflow).
fn gcd(mut a: i128, mut b: i128) -> i128 {
	a = a.abs();
	b = b.abs();
	while b != 0 {
		let t = a % b;
		a = b;
		b = t;
	}
	a
}

fn rat_from_parts(num: i128, den: i128) -> Result<Rat, ToolError> {
	if den == 0 {
		return Err(ToolError::timecode("rational denominator must not be zero"));
	}
	let (mut num, mut den) = (num, den);
	if den < 0 {
		num = -num;
		den = -den;
	}
	let g = gcd(num, den);
	let (num, den) = if g > 1 { (num / g, den / g) } else { (num, den) };
	let num = i64::try_from(num)
		.map_err(|_| ToolError::timecode("rational numerator out of range"))?;
	let den = i64::try_from(den)
		.map_err(|_| ToolError::timecode("rational denominator out of range"))?;
	Ok(Rat { num, den })
}

/// A rational number in lowest terms with a strictly positive
/// denominator. Serialises as `{"num": .., "den": ..}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Rat {
	/// Numerator (may be negative).
	pub num: i64,
	/// Denominator, always `> 0`.
	pub den: i64,
}

impl Rat {
	/// Builds a reduced rational; `den == 0` is rejected.
	pub fn new(num: i64, den: i64) -> Result<Rat, ToolError> {
		rat_from_parts(num as i128, den as i128)
	}

	/// An integer as a rational.
	pub fn from_int(value: i64) -> Rat {
		Rat { num: value, den: 1 }
	}

	/// `self + other`, checked for overflow.
	pub fn checked_add(self, other: Rat) -> Result<Rat, ToolError> {
		rat_from_parts(
			self.num as i128 * other.den as i128 + other.num as i128 * self.den as i128,
			self.den as i128 * other.den as i128,
		)
	}

	/// `self - other`, checked for overflow.
	pub fn checked_sub(self, other: Rat) -> Result<Rat, ToolError> {
		rat_from_parts(
			self.num as i128 * other.den as i128 - other.num as i128 * self.den as i128,
			self.den as i128 * other.den as i128,
		)
	}

	/// `self * factor` (integer factor), checked for overflow.
	pub fn checked_mul_int(self, factor: i64) -> Result<Rat, ToolError> {
		rat_from_parts(self.num as i128 * factor as i128, self.den as i128)
	}

	/// `self / factor` (integer divisor), checked; `factor == 0` is a
	/// hard rejection (`TIMECODE_INVALID`).
	pub fn checked_div_int(self, factor: i64) -> Result<Rat, ToolError> {
		if factor == 0 {
			return Err(ToolError::timecode("rational division by zero"));
		}
		rat_from_parts(self.num as i128, self.den as i128 * factor as i128)
	}

	/// Whether the value is exactly zero.
	pub fn is_zero(self) -> bool {
		self.num == 0
	}

	/// Floating-point view (only for frame maths and rendering; never for
	/// the wire format).
	pub fn to_f64(self) -> f64 {
		self.num as f64 / self.den as f64
	}

	/// The wire form.
	pub fn to_json(self) -> Value {
		serde_json::to_value(self).expect("rat serialisation")
	}
}

impl PartialOrd for Rat {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		let lhs = self.num as i128 * other.den as i128;
		let rhs = other.num as i128 * self.den as i128;
		Some(lhs.cmp(&rhs))
	}
}

impl Ord for Rat {
	fn cmp(&self, other: &Self) -> Ordering {
		self.partial_cmp(other).expect("rational ordering")
	}
}

impl fmt::Display for Rat {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}/{}", self.num, self.den)
	}
}

fn read_rat_map<'de, A: MapAccess<'de>>(map: A) -> Result<Rat, A::Error> {
	struct RatVisitor;
	impl<'de> Visitor<'de> for RatVisitor {
		type Value = Rat;

		fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			f.write_str("rational seconds {num, den} with den != 0")
		}

		fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Rat, A::Error> {
			let mut num: Option<i64> = None;
			let mut den: Option<i64> = None;
			while let Some(key) = map.next_key::<String>()? {
				match key.as_str() {
					"num" => {
						if num.is_some() {
							return Err(de::Error::duplicate_field("num"));
						}
						num = Some(map.next_value()?);
					}
					"den" => {
						if den.is_some() {
							return Err(de::Error::duplicate_field("den"));
						}
						den = Some(map.next_value()?);
					}
					other => {
						return Err(de::Error::unknown_field(other, &["num", "den"]));
					}
				}
			}
			let num = num.ok_or_else(|| de::Error::missing_field("num"))?;
			let den = den.ok_or_else(|| de::Error::missing_field("den"))?;
			rat_from_parts(num as i128, den as i128).map_err(de::Error::custom)
		}
	}
	RatVisitor.visit_map(map)
}

impl<'de> Deserialize<'de> for Rat {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Rat, D::Error> {
		struct PlainRat;
		impl<'de> Visitor<'de> for PlainRat {
			type Value = Rat;

			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("rational seconds {num, den} with den != 0")
			}

			fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Rat, A::Error> {
				read_rat_map(map)
			}
		}
		deserializer.deserialize_map(PlainRat)
	}
}

/// The two accepted time shapes at the tool face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeValue {
	/// Rational seconds.
	Rational(Rat),
	/// Frame index on the target's frame grid.
	Frame(u64),
}

impl TimeValue {
	/// Resolves to rational seconds on the frame grid defined by `rate`
	/// (frames per second as `{num, den}`).
	pub fn to_rational(self, rate: Rat) -> Result<Rat, ToolError> {
		match self {
			TimeValue::Rational(r) => Ok(r),
			TimeValue::Frame(frame) => {
				if rate.num <= 0 {
					return Err(ToolError::timecode(format!(
						"cannot resolve a frame index against the frame rate {rate}"
					)));
				}
				// seconds = frame / fps = frame * den / num
				rat_from_parts(frame as i128 * rate.den as i128, rate.num as i128)
			}
		}
	}

	/// The wire form (always rational seconds; frame indices are sugar).
	pub fn to_json(self) -> Value {
		match self {
			TimeValue::Rational(r) => r.to_json(),
			// Never emitted unresolved: frame indices only exist on input.
			TimeValue::Frame(f) => serde_json::json!(f),
		}
	}
}

impl fmt::Display for TimeValue {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			TimeValue::Rational(r) => write!(f, "{r}s"),
			TimeValue::Frame(n) => write!(f, "frame {n}"),
		}
	}
}

const REJECT: &str =
	"bare floating-point seconds are rejected; use {num, den} rational seconds or an integer frame index";

impl<'de> Deserialize<'de> for TimeValue {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<TimeValue, D::Error> {
		struct TimeVisitor;
		impl<'de> Visitor<'de> for TimeVisitor {
			type Value = TimeValue;

			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("rational seconds {num, den} or a non-negative integer frame index")
			}

			fn visit_u64<E: de::Error>(self, value: u64) -> Result<TimeValue, E> {
				Ok(TimeValue::Frame(value))
			}

			fn visit_i64<E: de::Error>(self, value: i64) -> Result<TimeValue, E> {
				u64::try_from(value)
					.map(TimeValue::Frame)
					.map_err(|_| E::custom("negative frame index"))
			}

			fn visit_f64<E: de::Error>(self, _value: f64) -> Result<TimeValue, E> {
				Err(E::custom(REJECT))
			}

			fn visit_str<E: de::Error>(self, value: &str) -> Result<TimeValue, E> {
				Err(E::custom(format!(
					"timecode strings are rejected ({value:?}); {REJECT}"
				)))
			}

			fn visit_bool<E: de::Error>(self, _value: bool) -> Result<TimeValue, E> {
				Err(E::custom(REJECT))
			}

			fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TimeValue, A::Error> {
				read_rat_map(map).map(TimeValue::Rational)
			}
		}
		deserializer.deserialize_any(TimeVisitor)
	}
}

/// Which entity supplies the frame grid for a frame index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeContext {
	/// A clip: its owning sequence's frame rate.
	Clip(String),
	/// A track: its owning sequence's frame rate.
	Track(String),
	/// A sequence (or the active sequence when the id is empty).
	Sequence(String),
	/// Raw footage: the media's own frame rate.
	Footage(String),
}

impl fmt::Display for TimeContext {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			TimeContext::Clip(id) => write!(f, "clip {id}"),
			TimeContext::Track(id) => write!(f, "track {id}"),
			TimeContext::Sequence(id) => write!(f, "sequence {id}"),
			TimeContext::Footage(id) => write!(f, "footage {id}"),
		}
	}
}

/// Reads a required time field (`{num, den}` or frame index).
pub fn time_arg(args: &Value, key: &str) -> Result<TimeValue, ToolError> {
	let raw = args
		.get(key)
		.ok_or_else(|| ToolError::invalid_params(format!("missing field '{key}'")))?;
	serde_json::from_value::<TimeValue>(raw.clone())
		.map_err(|err| ToolError::timecode(format!("field '{key}': {err}")))
}

/// Reads an optional time field.
pub fn opt_time_arg(args: &Value, key: &str) -> Result<Option<TimeValue>, ToolError> {
	match args.get(key) {
		None | Some(Value::Null) => Ok(None),
		Some(_) => time_arg(args, key).map(Some),
	}
}

/// Reads a required frame-rate field: a positive integer or `{num, den}`.
pub fn rate_arg(args: &Value, key: &str) -> Result<Rat, ToolError> {
	let raw = args
		.get(key)
		.ok_or_else(|| ToolError::invalid_params(format!("missing field '{key}'")))?;
	rate_value(raw)
		.ok_or_else(|| ToolError::timecode(format!("field '{key}': {REJECT}")))
}

/// Parses a frame-rate value (integer fps or `{num, den}`); `None` for
/// anything else, including bare floats.
pub fn rate_value(raw: &Value) -> Option<Rat> {
	match raw {
		Value::Number(n) if n.is_u64() => Rat::new(n.as_u64()? as i64, 1).ok(),
		Value::Number(n) if n.is_i64() => Rat::new(n.as_i64()?, 1).ok(),
		Value::Object(map) => {
			let num = map.get("num")?.as_i64()?;
			let den = map.get("den")?.as_i64()?;
			if map.len() != 2 {
				return None;
			}
			Rat::new(num, den).ok()
		}
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn reduction_and_sign_normalisation() {
		assert_eq!(Rat::new(4, 8).unwrap(), Rat { num: 1, den: 2 });
		assert_eq!(Rat::new(-6, 8).unwrap(), Rat { num: -3, den: 4 });
		assert_eq!(Rat::new(6, -8).unwrap(), Rat { num: -3, den: 4 });
		assert_eq!(Rat::new(0, 5).unwrap(), Rat { num: 0, den: 1 });
		assert_eq!(Rat::new(30000, 1001).unwrap(), Rat { num: 30000, den: 1001 });
	}

	#[test]
	fn zero_denominator_is_rejected() {
		let err = Rat::new(1, 0).unwrap_err();
		assert_eq!(err.code, crate::error::CODE_TIMECODE);
		assert!(serde_json::from_value::<Rat>(json!({"num": 1, "den": 0})).is_err());
	}

	#[test]
	fn arithmetic_stays_reduced() {
		let a = Rat::new(1, 2).unwrap();
		let b = Rat::new(1, 3).unwrap();
		assert_eq!(a.checked_add(b).unwrap(), Rat { num: 5, den: 6 });
		assert_eq!(a.checked_sub(b).unwrap(), Rat { num: 1, den: 6 });
		assert_eq!(a.checked_mul_int(4).unwrap(), Rat { num: 2, den: 1 });
	}

	#[test]
	fn ordering_is_exact_not_float() {
		// 1/3 vs 0.3333333333333333 (16 threes): both round to the same
		// f64, but the rationals are distinct and ordered correctly.
		let a = Rat::new(1, 3).unwrap();
		let b = Rat::new(3333333333333333, 10000000000000000).unwrap();
		assert_eq!(a.to_f64(), b.to_f64(), "f64 cannot tell them apart");
		assert!(a > b, "exact ordering must separate them");
		assert!(b < a);
		assert_eq!(a.cmp(&b), std::cmp::Ordering::Greater);
	}

	#[test]
	fn bare_float_is_rejected_by_serde() {
		assert!(serde_json::from_value::<TimeValue>(json!(1.5)).is_err());
		assert!(serde_json::from_value::<TimeValue>(json!(29.97)).is_err());
		let err = serde_json::from_value::<TimeValue>(json!(1.0)).unwrap_err();
		assert!(err.to_string().contains("floating-point"), "{err}");
	}

	#[test]
	fn strings_and_negatives_are_rejected() {
		assert!(serde_json::from_value::<TimeValue>(json!("00:00:01.000")).is_err());
		assert!(serde_json::from_value::<TimeValue>(json!(-3)).is_err());
		assert!(serde_json::from_value::<TimeValue>(json!(true)).is_err());
	}

	#[test]
	fn accepted_shapes() {
		assert_eq!(
			serde_json::from_value::<TimeValue>(json!({"num": 3, "den": 25})).unwrap(),
			TimeValue::Rational(Rat { num: 3, den: 25 })
		);
		// Reduced on the way in.
		assert_eq!(
			serde_json::from_value::<TimeValue>(json!({"num": 6, "den": 50})).unwrap(),
			TimeValue::Rational(Rat { num: 3, den: 25 })
		);
		assert_eq!(
			serde_json::from_value::<TimeValue>(json!(42)).unwrap(),
			TimeValue::Frame(42)
		);
	}

	#[test]
	fn frame_index_resolves_on_the_target_grid() {
		// 30000/1001 fps: frame 30 == 30 * 1001 / 30000 s = 1001/1000 s.
		let rate = Rat::new(30000, 1001).unwrap();
		assert_eq!(
			TimeValue::Frame(30).to_rational(rate).unwrap(),
			Rat { num: 1001, den: 1000 }
		);
		// Integer frame rate: frame N == N seconds / 30.
		let rate = Rat::new(30, 1).unwrap();
		assert_eq!(
			TimeValue::Frame(30).to_rational(rate).unwrap(),
			Rat { num: 1, den: 1 }
		);
		// A rational passes through untouched.
		let t = Rat::new(3, 25).unwrap();
		assert_eq!(
			TimeValue::Rational(t).to_rational(rate).unwrap(),
			t
		);
		// A null frame rate cannot host a frame index.
		let bad = Rat::new(0, 1).unwrap();
		assert!(TimeValue::Frame(1).to_rational(bad).is_err());
	}

	#[test]
	fn rate_field_parsing() {
		assert_eq!(rate_value(&json!(30)).unwrap(), Rat::new(30, 1).unwrap());
		assert_eq!(
			rate_value(&json!({"num": 30000, "den": 1001})).unwrap(),
			Rat::new(30000, 1001).unwrap()
		);
		assert!(rate_value(&json!(29.97)).is_none());
		assert!(rate_value(&json!("30")).is_none());
	}
}

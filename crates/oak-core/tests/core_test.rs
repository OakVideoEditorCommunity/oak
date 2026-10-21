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

//! oakcore-rs contract tests. The oracle is the C++ oakcore behavior:
/// every case below names the C++ semantic it pins down.
use oak_core::{PixelFormat, Rational, SampleFormat, TimeRange, TimeRangeList};

/// Construction reduces (2/4 -> 1/2), normalizes sign (1/-2 -> -1/2),
/// and 0/0 is the null sentinel. C++: Rational ctor + reduced().
#[test]
fn rational_reduction_and_sentinel() {
	assert_eq!(Rational::new(2, 4), Rational::new(1, 2));
	assert_eq!(Rational::new(2, 4).numerator(), 1);
	assert_eq!(Rational::new(2, 4).denominator(), 2);
	assert_eq!(Rational::new(1, -2), Rational::new(-1, 2));
	assert_eq!(Rational::new(1, -2).numerator(), -1);
	assert_eq!(Rational::new(1, -2).denominator(), 2);

	// 0/0 is the null/invalid sentinel.
	let n = Rational::new(0, 0);
	assert!(n.is_null());
	assert!(n.is_nan());
	assert_eq!(n, Rational::NULL);

	// A zero numerator normalizes the denominator to 1 (0/5 -> 0/1).
	let z = Rational::new(0, 5);
	assert!(z.is_null());
	assert!(!z.is_nan());
	assert_eq!(z, Rational::new(0, 1));

	assert_eq!(Rational::NULL, Rational::new(0, 0));
}

/// Arithmetic matches C++ exactly, including 30000/1001-style video
/// rates: (1001/30000 * 30000/1001 == 1), addition across denominators,
/// division by zero yields the C++ result (null propagation).
#[test]
fn rational_arithmetic_video_rates() {
	assert_eq!(
		Rational::new(1001, 30000) * Rational::new(30000, 1001),
		Rational::new(1, 1)
	);
	assert_eq!(
		Rational::new(1, 3) + Rational::new(1, 6),
		Rational::new(1, 2)
	);
	assert_eq!(
		Rational::new(1, 2) - Rational::new(1, 3),
		Rational::new(1, 6)
	);
	assert_eq!(
		Rational::new(2, 3) * Rational::new(3, 4),
		Rational::new(1, 2)
	);

	// Division by a zero-value rational yields 0/0 (NaN): the denominator
	// becomes 0 and reduce_fraction forces the numerator to 0.
	let d = Rational::new(1, 1) / Rational::new(0, 1);
	assert!(d.is_nan());
	assert!(d.is_null());
	assert_eq!(d, Rational::NULL);

	// 0/0 on the left propagates the (unchanged) NaN self.
	assert_eq!(Rational::NULL + Rational::new(1, 2), Rational::NULL);
	// 0/0 on the right yields NULL for every operator.
	assert_eq!(Rational::new(1, 2) + Rational::NULL, Rational::NULL);
	assert_eq!(Rational::new(1, 2) - Rational::NULL, Rational::NULL);
	assert_eq!(Rational::new(1, 2) * Rational::NULL, Rational::NULL);
	assert_eq!(Rational::new(1, 2) / Rational::NULL, Rational::NULL);
}

/// from_string/to_string round-trip incl. sentinel spellings used by
/// project XML ("0/0", RATIONAL_MIN/MAX); garbage input -> null.
#[test]
fn rational_string_roundtrip() {
	assert_eq!(Rational::from_string("0/0"), Rational::NULL);
	assert_eq!(
		Rational::from_string("2147483647"),
		Rational::new(2147483647, 1)
	);
	assert_eq!(
		Rational::from_string("-2147483647/1"),
		Rational::new(-2147483647, 1)
	);

	// Garbage single token parses as 0 -> 0/1 (null but not NaN).
	let g = Rational::from_string("abc");
	assert!(g.is_null());
	assert!(!g.is_nan());
	assert_eq!(g, Rational::new(0, 1));
	assert_eq!(g.to_display_string(), "0/1");

	assert_eq!(Rational::new(30000, 1001).to_display_string(), "30000/1001");

	// More than two '/' elements -> null sentinel.
	assert_eq!(Rational::from_string("a/b/c"), Rational::NULL);
}

/// Ordering across denominators (1/3 vs 1001/3000) and equality of
/// differently-reduced equal values.
#[test]
fn rational_ordering() {
	assert!(Rational::new(1, 3) < Rational::new(1001, 3000));
	assert!(Rational::new(1, 3) > Rational::new(1, 4));
	assert_eq!(Rational::new(1, 3), Rational::new(2, 6));
	assert!(Rational::new(1, 2) > Rational::new(1, 3));

	// NaN orders before everything and equals itself.
	assert!(Rational::NULL < Rational::new(1, 1));
	assert!(Rational::new(1, 1) > Rational::NULL);
	assert_eq!(Rational::NULL, Rational::NULL);

	// Total order sorts a mixed list.
	let mut v = vec![
		Rational::new(1, 2),
		Rational::new(1, 4),
		Rational::new(1, 3),
	];
	v.sort();
	assert_eq!(
		v,
		vec![
			Rational::new(1, 4),
			Rational::new(1, 3),
			Rational::new(1, 2)
		]
	);
}

/// time_to_timestamp/timestamp_to_time match C++ Timecode rounding
/// (half-away-from-zero at frame boundaries), incl. negative times.
#[test]
fn timecode_rounding() {
	// 29.97 fps: the timebase is seconds-per-frame = 1001/30000.
	let tb = Rational::new(1001, 30000);

	// 0 seconds -> 0 frames.
	assert_eq!(tb.time_to_timestamp(Rational::new(0, 1)), 0);

	// 1 full frame.
	assert_eq!(tb.time_to_timestamp(Rational::new(1001, 30000)), 1);

	// Half a frame (0.5 * 1001/30000 s) rounds half-away-from-zero -> 1.
	let half = Rational::new(1001, 60000);
	assert_eq!(tb.time_to_timestamp(half), 1);

	// Negative half frame rounds to -1 (llround half away from zero).
	assert_eq!(tb.time_to_timestamp(Rational::new(-1001, 60000)), -1);

	// 30 frames round-trip to exactly 1001/1000 s.
	assert_eq!(tb.timestamp_to_time(30), Rational::new(1001, 1000));

	// timestamp -> time -> timestamp round trip.
	assert_eq!(tb.time_to_timestamp(tb.timestamp_to_time(29)), 29);
	assert_eq!(tb.time_to_timestamp(tb.timestamp_to_time(300)), 300);

	// Video-rate time: 30 frames at 1001/1000 s -> 30.
	assert_eq!(tb.time_to_timestamp(Rational::new(1001, 1000)), 30);
}

/// TimeRange: contains/intersected/combined, touching ranges,
/// zero-length ranges. C++: TimeRange methods.
#[test]
fn timerange_ops() {
	let r = TimeRange::new(Rational::new(0, 1), Rational::new(10, 1));
	assert!(r.contains(Rational::new(5, 1)));
	assert!(r.contains(Rational::new(0, 1)));
	assert!(!r.contains(Rational::new(10, 1)));
	assert!(!r.contains(Rational::new(-1, 1)));
	assert_eq!(r.length(), Rational::new(10, 1));

	let a = TimeRange::new(Rational::new(0, 1), Rational::new(10, 1));
	let b = TimeRange::new(Rational::new(5, 1), Rational::new(15, 1));
	assert_eq!(
		a.intersected(&b),
		TimeRange::new(Rational::new(5, 1), Rational::new(10, 1))
	);
	assert_eq!(
		a.combined(&b),
		TimeRange::new(Rational::new(0, 1), Rational::new(15, 1))
	);

	// Touching ranges combined into one.
	let c = TimeRange::new(Rational::new(10, 1), Rational::new(20, 1));
	assert_eq!(
		a.combined(&c),
		TimeRange::new(Rational::new(0, 1), Rational::new(20, 1))
	);

	// Zero-length range.
	let z = TimeRange::new(Rational::new(5, 1), Rational::new(5, 1));
	assert_eq!(z.length(), Rational::new(0, 1));
	assert!(!z.contains(Rational::new(5, 1)));

	// out < in is normalized by swapping.
	let swapped = TimeRange::new(Rational::new(10, 1), Rational::new(5, 1));
	assert_eq!(
		swapped,
		TimeRange::new(Rational::new(5, 1), Rational::new(10, 1))
	);
}

/// TimeRangeList insert merges overlapping AND touching ranges;
/// remove splits. C++: TimeRangeList.
#[test]
fn timerangelist_normalization() {
	let mut list = TimeRangeList::new();
	assert!(list.is_empty());
	assert_eq!(list.first(), Rational::NULL);

	list.insert(TimeRange::new(Rational::new(0, 1), Rational::new(10, 1)));
	list.insert(TimeRange::new(Rational::new(20, 1), Rational::new(30, 1)));
	// Overlaps (0,10): merges into (0,15), leaving (0,15) + (20,30).
	list.insert(TimeRange::new(Rational::new(5, 1), Rational::new(15, 1)));

	assert!(!list.is_empty());
	assert_eq!(list.total_length(), Rational::new(25, 1));
	assert_eq!(list.ranges().len(), 2);
	let mut ins: Vec<_> = list.ranges().iter().map(|r| r.in_()).collect();
	ins.sort();
	assert_eq!(ins, vec![Rational::new(0, 1), Rational::new(20, 1)]);

	// Touching inserts merge into a single range.
	let mut t = TimeRangeList::new();
	t.insert(TimeRange::new(Rational::new(0, 1), Rational::new(10, 1)));
	t.insert(TimeRange::new(Rational::new(10, 1), Rational::new(20, 1)));
	assert_eq!(t.ranges().len(), 1);
	assert_eq!(
		t.ranges()[0],
		TimeRange::new(Rational::new(0, 1), Rational::new(20, 1))
	);

	// remove splits (0,20) into (0,5) + (15,20) around the removed (5,15).
	let mut s = TimeRangeList::new();
	s.insert(TimeRange::new(Rational::new(0, 1), Rational::new(20, 1)));
	s.remove(TimeRange::new(Rational::new(5, 1), Rational::new(15, 1)));
	assert_eq!(s.ranges().len(), 2);
	let mut outs: Vec<_> = s.ranges().iter().map(|r| r.out()).collect();
	outs.sort();
	assert_eq!(outs, vec![Rational::new(5, 1), Rational::new(20, 1)]);

	// first() returns the first range's in().
	let mut f = TimeRangeList::new();
	f.insert(TimeRange::new(Rational::new(3, 1), Rational::new(7, 1)));
	assert_eq!(f.first(), Rational::new(3, 1));
}

/// Frame-grid snap semantics (C++ TimeRangeListFrameIterator::Snap).
#[test]
fn timerangelist_snap() {
	let tb = Rational::new(1001, 30000); // 29.97 fps, seconds per frame.

	// 0.5 frames floors down to frame 0.
	let half = Rational::new(1001, 60000);
	assert_eq!(TimeRangeList::snap(half, tb), Rational::new(0, 1));

	// Exactly 1 frame stays put.
	let one = Rational::new(1001, 30000);
	assert_eq!(TimeRangeList::snap(one, tb), Rational::new(1001, 30000));

	// 1.5 frames (1001/20000 s) floors down to 1 frame.
	let one_and_half = Rational::new(1001, 20000);
	assert_eq!(
		TimeRangeList::snap(one_and_half, tb),
		Rational::new(1001, 30000)
	);

	// 30 frames round-trip exactly.
	assert_eq!(
		TimeRangeList::snap(Rational::new(1001, 1000), tb),
		Rational::new(1001, 1000)
	);
}

/// Enum discriminants identical to the C++ enums (C ABI contract).
#[test]
fn format_enum_values_match_cpp() {
	assert_eq!(PixelFormat::Invalid as i32, -1);
	assert_eq!(PixelFormat::U8 as i32, 0);
	assert_eq!(PixelFormat::U10 as i32, 1);
	assert_eq!(PixelFormat::U16 as i32, 2);
	assert_eq!(PixelFormat::F16 as i32, 3);
	assert_eq!(PixelFormat::F32 as i32, 4);

	assert_eq!(PixelFormat::Invalid.bytes_per_channel(), 0);
	assert_eq!(PixelFormat::U8.bytes_per_channel(), 1);
	assert_eq!(PixelFormat::U10.bytes_per_channel(), 4); // packed RGBA10A2
	assert_eq!(PixelFormat::U16.bytes_per_channel(), 2);
	assert_eq!(PixelFormat::F16.bytes_per_channel(), 2);
	assert_eq!(PixelFormat::F32.bytes_per_channel(), 4);
	assert_eq!(PixelFormat::F32.bytes_per_pixel(4), 16);

	assert_eq!(SampleFormat::Invalid as i32, -1);
	assert_eq!(SampleFormat::U8Planar as i32, 0);
	assert_eq!(SampleFormat::S16Planar as i32, 1);
	assert_eq!(SampleFormat::S32Planar as i32, 2);
	assert_eq!(SampleFormat::S64Planar as i32, 3);
	assert_eq!(SampleFormat::F32Planar as i32, 4);
	assert_eq!(SampleFormat::F64Planar as i32, 5);
	assert_eq!(SampleFormat::U8 as i32, 6);
	assert_eq!(SampleFormat::S16 as i32, 7);
	assert_eq!(SampleFormat::S32 as i32, 8);
	assert_eq!(SampleFormat::S64 as i32, 9);
	assert_eq!(SampleFormat::F32 as i32, 10);
	assert_eq!(SampleFormat::F64 as i32, 11);

	assert_eq!(SampleFormat::Invalid.bytes_per_sample(), 0);
	assert_eq!(SampleFormat::U8.bytes_per_sample(), 1);
	assert_eq!(SampleFormat::S16.bytes_per_sample(), 2);
	assert_eq!(SampleFormat::S32.bytes_per_sample(), 4);
	assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
	assert_eq!(SampleFormat::F64.bytes_per_sample(), 8);
	assert_eq!(SampleFormat::F64Planar.bytes_per_sample(), 8);

	assert!(SampleFormat::U8Planar.is_planar());
	assert!(SampleFormat::F64Planar.is_planar());
	assert!(!SampleFormat::U8.is_planar());
	assert!(!SampleFormat::Invalid.is_planar());

	assert_eq!(SampleFormat::U8Planar.to_packed(), SampleFormat::U8);
	assert_eq!(SampleFormat::F64Planar.to_packed(), SampleFormat::F64);
	assert_eq!(SampleFormat::U8.to_packed(), SampleFormat::U8);
	assert_eq!(SampleFormat::U8.to_planar(), SampleFormat::U8Planar);
	assert_eq!(SampleFormat::S16.to_planar(), SampleFormat::S16Planar);
	assert_eq!(SampleFormat::F64.to_planar(), SampleFormat::F64Planar);
}

/// Extreme i64 inputs (which C++ `int` could never receive) must reduce
/// without panicking, capped to the C++ `i32::MAX` reduce cap; the
/// RATIONAL_MIN/MAX sentinels propagate NaN through arithmetic.
#[test]
fn rational_large_inputs_and_minmax() {
	let mn = Rational::new(i64::MIN, 1);
	assert_eq!(mn.numerator(), -2147483647);
	assert_eq!(mn.denominator(), 1);

	let mx = Rational::new(i64::MAX, 1);
	assert_eq!(mx.numerator(), 2147483647);
	assert_eq!(mx.denominator(), 1);

	let mx = Rational::new(2147483647, 1);
	assert!(!mx.is_null()); // 2147483647/1 is a valid value, not null
	let r = mx + Rational::new(1, 1);
	assert!(r.is_nan());
	assert_eq!(r, Rational::NULL);

	let mn = Rational::new(-2147483647, 1);
	let r = Rational::new(1, 1) * mn;
	assert!(r.is_nan());
}

/// Additional edge cases: full-encompassing remove erases, partial
/// removes trim the correct endpoint, NaN timebase yields 0 frames, and
/// invalid format conversions are identities.
#[test]
fn additional_edge_cases() {
	// remove fully encompassing an element erases it.
	let mut l = TimeRangeList::new();
	l.insert(TimeRange::new(Rational::new(0, 1), Rational::new(20, 1)));
	l.remove(TimeRange::new(Rational::new(-5, 1), Rational::new(25, 1)));
	assert!(l.is_empty());

	// Trim the element's out down to the removal's in.
	let mut l2 = TimeRangeList::new();
	l2.insert(TimeRange::new(Rational::new(0, 1), Rational::new(20, 1)));
	l2.remove(TimeRange::new(Rational::new(5, 1), Rational::new(25, 1)));
	assert_eq!(l2.ranges().len(), 1);
	assert_eq!(
		l2.ranges()[0],
		TimeRange::new(Rational::new(0, 1), Rational::new(5, 1))
	);

	// Trim the element's in up to the removal's out.
	let mut l3 = TimeRangeList::new();
	l3.insert(TimeRange::new(Rational::new(0, 1), Rational::new(20, 1)));
	l3.remove(TimeRange::new(Rational::new(-5, 1), Rational::new(5, 1)));
	assert_eq!(l3.ranges().len(), 1);
	assert_eq!(
		l3.ranges()[0],
		TimeRange::new(Rational::new(5, 1), Rational::new(20, 1))
	);

	// A NaN timebase yields 0 frames.
	assert_eq!(Rational::NULL.time_to_timestamp(Rational::new(1, 1)), 0);
	// to_f64 of the null sentinel is NaN.
	assert!(Rational::NULL.to_f64().is_nan());

	// Invalid format conversions are identities.
	assert_eq!(SampleFormat::Invalid.to_packed(), SampleFormat::Invalid);
	assert_eq!(SampleFormat::Invalid.to_planar(), SampleFormat::Invalid);
}

/// from_double: C++ Rational::from_double parity — NaN and huge values
/// yield 0/0; common values round-trip; tiny values use the
/// high-precision retry path.
#[test]
fn rational_from_double() {
	use oak_core::Rational;
	assert!(Rational::from_double(f64::NAN).is_nan());
	assert!(Rational::from_double(1e300).is_nan());
	assert!(Rational::from_double(-1e300).is_nan());
	let r = Rational::from_double(0.5);
	assert_eq!((r.numerator(), r.denominator()), (1, 2));
	let fps = Rational::from_double(29.97002997002997);
	assert!((fps.to_f64() - 29.97002997002997).abs() < 1e-9);
	let third = Rational::from_double(1.0 / 3.0);
	assert!((third.to_f64() - 1.0 / 3.0).abs() < 1e-9);
	assert_eq!(Rational::from_double(0.0).numerator(), 0);
}

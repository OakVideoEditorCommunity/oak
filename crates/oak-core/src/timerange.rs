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

//! Time ranges and normalized range lists (oakcore `TimeRange` /
//! `TimeRangeList` equivalents).

use crate::rational::{self, Rational};

/// Half-open time range [in, out) — mirrors `olive::core::TimeRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TimeRange {
	in_: Rational,
	out: Rational,
}

impl TimeRange {
	/// Construct and normalize (C++ ctor calls `normalize()`: if `out <
	/// in` the two are swapped). The doc comment on the skeleton claimed
	/// normalization is not performed; matching C++ takes precedence.
	pub fn new(in_: Rational, out: Rational) -> TimeRange {
		let mut r = TimeRange { in_, out };
		r.normalize();
		r
	}

	/// Inclusive start.
	pub fn in_(&self) -> Rational {
		self.in_
	}

	/// Exclusive end.
	pub fn out(&self) -> Rational {
		self.out
	}

	/// `out - in`. When either endpoint is a `RATIONAL_MIN/MAX`
	/// sentinel the subtraction propagates NaN, matching C++ which
	/// stores the same sentinel value for `length_`.
	pub fn length(&self) -> Rational {
		self.out - self.in_
	}

	/// True when `t` lies in [in, out).
	pub fn contains(&self, t: Rational) -> bool {
		t >= self.in_ && t < self.out
	}

	/// True when `self` contains `compare`, honoring inclusivity of the
	/// in/out edges (C++ `TimeRange::contains(TimeRange)`).
	fn contains_range(&self, compare: &TimeRange, in_inclusive: bool, out_inclusive: bool) -> bool {
		let contains_in = if in_inclusive {
			compare.in_ >= self.in_
		} else {
			compare.in_ > self.in_
		};
		let contains_out = if out_inclusive {
			compare.out <= self.out
		} else {
			compare.out < self.out
		};
		contains_in && contains_out
	}

	/// True when `self` and `a` overlap, honoring edge inclusivity
	/// (C++ `TimeRange::overlaps_with`).
	fn overlaps_with(&self, a: &TimeRange, in_inclusive: bool, out_inclusive: bool) -> bool {
		let does_not_overlap_in = if in_inclusive {
			a.out < self.in_
		} else {
			a.out <= self.in_
		};
		let does_not_overlap_out = if out_inclusive {
			a.in_ > self.out
		} else {
			a.in_ >= self.out
		};
		!does_not_overlap_in && !does_not_overlap_out
	}

	/// Intersection; empty when disjoint (C++ `intersected`).
	///
	/// Note: C++ normalizes the result, so disjoint inputs produce a
	/// swapped (in > out) range rather than an "empty" marker; we match
	/// that bit-for-bit.
	pub fn intersected(&self, other: &TimeRange) -> TimeRange {
		TimeRange::new(
			std::cmp::max(self.in_, other.in_),
			std::cmp::min(self.out, other.out),
		)
	}

	/// Union that also merges touching ranges (C++ `combined`).
	pub fn combined(&self, other: &TimeRange) -> TimeRange {
		TimeRange::new(
			std::cmp::min(self.in_, other.in_),
			std::cmp::max(self.out, other.out),
		)
	}

	/// C++ `set_in` + `normalize`.
	fn set_in(&mut self, in_: Rational) {
		self.in_ = in_;
		self.normalize();
	}

	/// C++ `set_out` + `normalize`.
	fn set_out(&mut self, out: Rational) {
		self.out = out;
		self.normalize();
	}

	/// C++ `normalize`: swap if `out < in`.
	fn normalize(&mut self) {
		if self.out < self.in_ {
			std::mem::swap(&mut self.out, &mut self.in_);
		}
	}
}

/// Normalized (sorted, non-overlapping) list of ranges — mirrors
/// `olive::core::TimeRangeList` including its merge-on-insert and
/// subtraction semantics.
///
/// Deliberately a newtype over `Vec<TimeRange>` rather than a plain
/// `Vec`: [`insert`](TimeRangeList::insert) and
/// [`remove`](TimeRangeList::remove) maintain the normalized invariant
/// (merged, non-overlapping ranges), which a raw `Vec` cannot enforce.
/// The wrapped `Vec` stays private for the same reason.
#[derive(Clone, Debug, Default)]
pub struct TimeRangeList(Vec<TimeRange>);

impl TimeRangeList {
	/// Empty list.
	pub fn new() -> Self {
		TimeRangeList(Vec::new())
	}

	/// True when any element fully contains `range` (C++
	/// `TimeRangeList::contains`, inclusive edges).
	fn contains_range(&self, range: &TimeRange) -> bool {
		self.0
			.iter()
			.any(|r| r.contains_range(range, true, true))
	}

	/// Insert a range, merging overlaps and touching neighbors
	/// (C++ `insert(TimeRange)`).
	pub fn insert(&mut self, range: TimeRange) {
		// If the list already fully contains this range, nothing to do.
		if self.contains_range(&range) {
			return;
		}

		let mut range = range;
		let mut i = 0;
		while i < self.0.len() {
			let compare = self.0[i];
			if compare.overlaps_with(&range, true, true) {
				range = compare.combined(&range);
				self.0.remove(i);
			} else {
				i += 1;
			}
		}

		self.0.push(range);
	}

	/// Subtract a range (C++ `remove`, via `util_remove`).
	pub fn remove(&mut self, range: TimeRange) {
		let mut additions: Vec<TimeRange> = Vec::new();

		let mut i = 0;
		while i < self.0.len() {
			let compare = self.0[i];

			if range.contains_range(&compare, true, true) {
				// The removal range entirely encompasses this element.
				self.0.remove(i);
			} else if compare.contains_range(&range, false, false) {
				// The removal range is strictly inside this element:
				// split it into two.
				let mut new_range = compare;
				new_range.set_in(range.out);
				let mut trimmed = compare;
				trimmed.set_out(range.in_);
				self.0[i] = trimmed;
				additions.push(new_range);
				break;
			} else {
				if compare.in_ < range.in_ && compare.out > range.in_ {
					// This element's out overlaps the range's in: trim it.
					self.0[i].set_out(range.in_);
				} else if compare.in_ < range.out && compare.out > range.out {
					// This element's in overlaps the range's out: trim it.
					self.0[i].set_in(range.out);
				}
				i += 1;
			}
		}

		self.0.extend(additions);
	}

	/// Sorted ranges view.
	pub fn ranges(&self) -> &[TimeRange] {
		&self.0
	}

	/// True when the list has no ranges.
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}

	/// Total covered duration (sum of each range's length).
	pub fn total_length(&self) -> Rational {
		let mut total = Rational::new(0, 1);
		for r in &self.0 {
			total = total + r.length();
		}
		total
	}

	/// First time covered by any range (C++ `in()` on the first range);
	/// null rational when empty.
	pub fn first(&self) -> Rational {
		match self.0.first() {
			Some(r) => r.in_(),
			None => Rational::NULL,
		}
	}

	/// Frame-accurate iteration helper: snap a time to the containing
	/// frame grid of `timebase` (C++ TimeRangeListFrameIterator snap,
	/// `k_floor` rounding).
	///
	/// Associated function (no `self`): a single snap needs no list
	/// state.
	pub fn snap(time: Rational, timebase: Rational) -> Rational {
		rational::snap_time_to_timebase(time, timebase)
	}
}

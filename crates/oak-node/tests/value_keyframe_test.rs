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

//! Value system and keyframe contract tests.

use std::sync::atomic::Ordering;

use oak_core::Rational;
use oak_node::handle::{self, CHandle, RefBox};
use oak_node::keyframe::{Interpolation, Keyframe, KeyframeTrack};
use oak_node::value::{NodeValue, NodeValueTable, ValueType};

/// NodeValueTable: last-push-wins per type, tag preserved, `get` of an
/// absent type returns None (C++ NodeValueTable semantics).
#[test]
fn value_table_last_push_wins() {
	let mut t = NodeValueTable::default();
	assert!(t.is_empty());
	assert!(t.get(ValueType::Float).is_none());

	t.push(
		ValueType::Float,
		NodeValue::Float(1.0),
		Some("a".to_string()),
	);
	t.push(
		ValueType::Float,
		NodeValue::Float(2.0),
		Some("b".to_string()),
	);
	t.push(ValueType::Int, NodeValue::Int(7), None);

	assert_eq!(t.count(), 3);
	assert_eq!(t.get(ValueType::Float), Some(&NodeValue::Float(2.0)));
	assert_eq!(t.get(ValueType::Int), Some(&NodeValue::Int(7)));

	t.clear();
	assert!(t.is_empty());
	assert!(t.get(ValueType::Int).is_none());
}

/// Texture values release their handle reference on drop (refcount
/// discipline: no C++ Variant shared_ptr aliasing exists here).
#[test]
fn texture_value_drop_releases() {
	let h = handle::make_owned(7u32);
	// Add one reference for the texture payload (CHandle copies are
	// bitwise; addref is the caller's contract).
	unsafe { (h.addref.unwrap())(h.ctx) };
	let refs = |h: &CHandle| -> u32 {
		unsafe {
			(*(h.ctx as *const RefBox<u32>))
				.refs
				.load(Ordering::Relaxed)
		}
	};
	assert_eq!(refs(&h), 2);

	{
		let v = NodeValue::Texture(h);
		assert_eq!(refs(&h), 2, "no extra reference taken on clone");
		drop(v); // must release the payload's reference
		assert_eq!(refs(&h), 1);
	}

	// Dropping the payload twice would underflow the counter — the
	// single release above is the whole contract.
	unsafe { (h.release.unwrap())(h.ctx) }; // back to 0, box freed
}

/// NodeValue::clone addrefs texture handles (C++ shared_ptr-in-Variant
/// semantics): each clone owns one reference, so clone + double drop is
/// balanced. A bitwise clone would double-release the box.
#[test]
fn texture_value_clone_addrefs() {
	let h = handle::make_owned(7u32);
	let refs = |h: &CHandle| -> u32 {
		unsafe {
			(*(h.ctx as *const RefBox<u32>))
				.refs
				.load(Ordering::Relaxed)
		}
	};
	assert_eq!(refs(&h), 1);

	{
		let a = NodeValue::Texture(h);
		assert_eq!(refs(&h), 1, "construction takes the caller's reference");
		{
			let b = a.clone();
			assert_eq!(refs(&h), 2, "clone addrefs");
			drop(b);
			assert_eq!(refs(&h), 1);
		}
		// Dropping `a` must release the last reference and free the box
		// exactly once — a bitwise clone would have made refs hit zero
		// too early and double-released here (use-after-free).
		drop(a);
	}
}

/// KeyframeTrack: insert keeps order; replace at same time overwrites;
/// remove missing key returns false.
#[test]
fn keyframe_track_ordering() {
	let mut track = KeyframeTrack::default();
	assert!(track.keys().is_empty());

	track.set_key(key(10, 1.0));
	track.set_key(key(0, 0.0));
	track.set_key(key(5, 0.5));
	assert_eq!(
		times(&track),
		vec![
			Rational::new(0, 1),
			Rational::new(5, 1),
			Rational::new(10, 1)
		]
	);

	// Replace at an existing time overwrites in place (still sorted).
	track.set_key(key(5, 9.0));
	assert_eq!(track.keys().len(), 3);
	assert_eq!(
		track.value_at(Rational::new(5, 1)),
		Some(NodeValue::Float(9.0))
	);

	// Remove: existing key true, missing false.
	assert!(track.remove_key(Rational::new(10, 1)));
	assert!(!track.remove_key(Rational::new(10, 1)));
	assert_eq!(track.keys().len(), 2);
}

/// Interpolation parity: linear/bezier/hold values at sampled times
/// match the C++ lerp/bezier math within 1e-9 (control points and the
/// bisection solver included — `// CPP-PARITY: node.cpp:465`,
/// `// CPP-PARITY: core/src/util/bezier.cpp`).
#[test]
fn interpolation_matches_cpp() {
	let eps = 1e-9;

	// Linear between (0, 0) and (10, 1): t=5 -> 0.5 exactly
	// (lerp(a,b,t) = a*(1-t)+b*t).
	let mut track = KeyframeTrack::default();
	track.set_key(key(0, 0.0));
	track.set_key(key(10, 1.0));
	assert!(track.value_at(Rational::new(5, 1)).is_some());
	let v = track.value_at(Rational::new(5, 1)).unwrap();
	assert!((v.to_double() - 0.5).abs() < eps);
	assert!((track.value_at(Rational::new(3, 1)).unwrap().to_double() - 0.3).abs() < eps);

	// Hold: the first key's value holds until the next key.
	let mut hold = KeyframeTrack::default();
	hold.set_key(Keyframe {
		time: Rational::new(0, 1),
		value: NodeValue::Float(2.0),
		interpolation: Interpolation::Hold,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	hold.set_key(key(10, 9.0));
	assert_eq!(
		hold.value_at(Rational::new(7, 1)),
		Some(NodeValue::Float(2.0)),
		"hold keeps the before value"
	);

	// Cubic bezier with symmetric handles: at the curve's midpoint the
	// value equals the exact cubic evaluation (5.0 by symmetry).
	let mut cubic = KeyframeTrack::default();
	cubic.set_key(Keyframe {
		time: Rational::new(0, 1),
		value: NodeValue::Float(0.0),
		interpolation: Interpolation::Bezier,
		bezier_in: (0.0, 0.0),
		bezier_out: (1.0, 1.0),
	});
	cubic.set_key(Keyframe {
		time: Rational::new(10, 1),
		value: NodeValue::Float(10.0),
		interpolation: Interpolation::Bezier,
		bezier_in: (-1.0, -1.0),
		bezier_out: (0.0, 0.0),
	});
	let v = cubic.value_at(Rational::new(5, 1)).unwrap();
	assert!(
		(v.to_double() - 5.0).abs() < eps,
		"cubic midpoint: {}",
		v.to_double()
	);

	// Quadratic bezier with a linear x map: before=(0,0) bezier with
	// out=(2,2), after=(4,4) linear. x(t)=4t so x=2 -> t=0.5 and
	// y(0.5)=2.0 exactly.
	let mut quad = KeyframeTrack::default();
	quad.set_key(Keyframe {
		time: Rational::new(0, 1),
		value: NodeValue::Float(0.0),
		interpolation: Interpolation::Bezier,
		bezier_in: (0.0, 0.0),
		bezier_out: (2.0, 2.0),
	});
	quad.set_key(key(4, 4.0));
	let v = quad.value_at(Rational::new(2, 1)).unwrap();
	assert!(
		(v.to_double() - 2.0).abs() < eps,
		"quadratic midpoint: {}",
		v.to_double()
	);

	// Rational type re-quantizes through Rational::from_double.
	let mut rt = KeyframeTrack::default();
	rt.set_key(Keyframe {
		time: Rational::new(0, 1),
		value: NodeValue::Rational(Rational::new(0, 1)),
		interpolation: Interpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	rt.set_key(Keyframe {
		time: Rational::new(10, 1),
		value: NodeValue::Rational(Rational::new(1, 1)),
		interpolation: Interpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	match rt.value_at(Rational::new(5, 1)) {
		Some(NodeValue::Rational(r)) => assert!((r.to_f64() - 0.5).abs() < eps),
		other => panic!("expected rational, got {:?}", other),
	}

	// Vec2 interpolates component-wise.
	let mut vec = KeyframeTrack::default();
	vec.set_key(Keyframe {
		time: Rational::new(0, 1),
		value: NodeValue::Vec2([0.0, 0.0]),
		interpolation: Interpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	vec.set_key(Keyframe {
		time: Rational::new(10, 1),
		value: NodeValue::Vec2([10.0, 20.0]),
		interpolation: Interpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	match vec.value_at(Rational::new(5, 1)) {
		Some(NodeValue::Vec2(v)) => {
			assert!((v[0] - 5.0).abs() < eps);
			assert!((v[1] - 10.0).abs() < eps);
		}
		other => panic!("expected vec2, got {:?}", other),
	}
}

/// Empty track value_at returns None; single-key track holds
/// constant before and after the key.
#[test]
fn keyframe_edge_cases() {
	let empty = KeyframeTrack::default();
	assert_eq!(empty.value_at(Rational::new(0, 1)), None);

	let mut one = KeyframeTrack::default();
	one.set_key(key(5, 3.0));
	assert_eq!(
		one.value_at(Rational::new(0, 1)),
		Some(NodeValue::Float(3.0))
	);
	assert_eq!(
		one.value_at(Rational::new(5, 1)),
		Some(NodeValue::Float(3.0))
	);
	assert_eq!(
		one.value_at(Rational::new(99, 1)),
		Some(NodeValue::Float(3.0))
	);

	// Exact key time returns the exact key value (before-holds branch).
	let mut two = KeyframeTrack::default();
	two.set_key(key(0, 1.0));
	two.set_key(key(10, 2.0));
	assert_eq!(
		two.value_at(Rational::new(0, 1)),
		Some(NodeValue::Float(1.0))
	);
	assert_eq!(
		two.value_at(Rational::new(10, 1)),
		Some(NodeValue::Float(2.0))
	);
}

fn key(time: i64, value: f64) -> Keyframe {
	Keyframe {
		time: Rational::new(time, 1),
		value: NodeValue::Float(value),
		interpolation: Interpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	}
}

fn times(track: &KeyframeTrack) -> Vec<Rational> {
	track.keys().iter().map(|k| k.time).collect()
}

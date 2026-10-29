//! Parametric curves (`oak_ofx_curveset_*`): the curve model behind
//! `OfxParamTypeParametric` parameters (segment-wise cubic Hermite;
//! see `oak_ofx_plugin::param_curve`), as a refcounted handle.
//!
//! JSON is the interchange format with node values and project files
//! (`curves_to_json`/`curves_from_json`); the handle API is for
//! building/inspecting curves without string munging.

use std::sync::Mutex;

use oak_ofx_plugin::param_curve::{self, ControlPoint, Curve};

use crate::codec::{copy_str_out, str_arg};
use crate::handle::NativeMirror;

/// A curve control point (`repr(C)` mirror of
/// `oak_ofx_plugin::param_curve::ControlPoint` — see
/// [`NativeMirror`](crate::handle::NativeMirror)): `key` is the
/// parametric position, `value` the result, `slope` the first
/// derivative (Hermite tangent; recomputed automatically by the
/// mutating calls).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OakOfxCurvePoint {
	pub key: f64,
	pub value: f64,
	pub slope: f64,
}

impl crate::handle::NativeMirror for OakOfxCurvePoint {
	type Native = ControlPoint;

	fn from_native(p: &ControlPoint) -> Self {
		Self {
			key: p.key,
			value: p.value,
			slope: p.slope,
		}
	}

	fn to_native(&self) -> ControlPoint {
		ControlPoint {
			key: self.key,
			value: self.value,
			slope: self.slope,
		}
	}
}

/// A set of parametric curves (one per dimension of a parametric
/// parameter). Refcounted across the ABI
/// ([`oak_ofx_curveset_add_ref`]/[`oak_ofx_curveset_release`]).
pub struct OfxCurveSet {
	curves: Mutex<Vec<Curve>>,
}

crate::export_handle!(
	OfxCurveSet,
	oak_ofx_curveset_add_ref,
	oak_ofx_curveset_release
);

/// A new, empty curve set (ref count 1).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_curveset_new() -> *mut OfxCurveSet {
	crate::handle::into_ffi(OfxCurveSet {
		curves: Mutex::new(Vec::new()),
	}) as *mut OfxCurveSet
}

/// Parses curve-set JSON (the payload node values and project files
/// carry). Null on invalid input or malformed JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_from_json(
	json: *const u8,
	json_len: usize,
) -> *mut OfxCurveSet {
	let Some(json) = (unsafe { str_arg(json, json_len) }) else {
		return std::ptr::null_mut();
	};
	let Some(curves) = param_curve::curves_from_json(json) else {
		return std::ptr::null_mut();
	};
	crate::handle::into_ffi(OfxCurveSet {
		curves: Mutex::new(curves),
	}) as *mut OfxCurveSet
}

/// Serializes the curve set to JSON, snprintf-style; -1 on a null
/// handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_to_json(
	set: *const OfxCurveSet,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if set.is_null() {
		return -1;
	}
	let curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let json = param_curve::curves_to_json(&curves);
	drop(curves);
	copy_str_out(&json, buf, buf_len)
}

/// Number of curves in the set; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_curve_count(set: *const OfxCurveSet) -> i32 {
	if set.is_null() {
		return -1;
	}
	unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.len() as i32
}

/// Appends an empty curve, returning its index; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_add_curve(set: *const OfxCurveSet) -> i32 {
	if set.is_null() {
		return -1;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	curves.push(Curve::empty());
	(curves.len() - 1) as i32
}

/// Appends an identity curve over `[lo, hi]` (the parametric default),
/// returning its index; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_add_identity_curve(
	set: *const OfxCurveSet,
	lo: f64,
	hi: f64,
) -> i32 {
	if set.is_null() {
		return -1;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	curves.push(Curve::identity(lo, hi));
	(curves.len() - 1) as i32
}

/// Number of control points in curve `curve`; -1 on a null handle or
/// out-of-range curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_point_count(set: *const OfxCurveSet, curve: i32) -> i32 {
	if set.is_null() || curve < 0 {
		return -1;
	}
	unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.get(curve as usize)
		.map(|c| c.len() as i32)
		.unwrap_or(-1)
}

/// Reads control point `index` of curve `curve`. False on a null
/// handle/output or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_point_at(
	set: *const OfxCurveSet,
	curve: i32,
	index: i32,
	out: *mut OakOfxCurvePoint,
) -> bool {
	if set.is_null() || out.is_null() || curve < 0 || index < 0 {
		return false;
	}
	let curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let Some(point) = curves
		.get(curve as usize)
		.and_then(|c| c.nth(index as usize))
	else {
		return false;
	};
	unsafe { *out = OakOfxCurvePoint::from_native(point) };
	true
}

/// Inserts or overwrites the control point at `key` (key order is
/// maintained; slopes are recomputed). False on a null handle or an
/// out-of-range curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_upsert_point(
	set: *const OfxCurveSet,
	curve: i32,
	key: f64,
	value: f64,
) -> bool {
	if set.is_null() || curve < 0 {
		return false;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let Some(c) = curves.get_mut(curve as usize) else {
		return false;
	};
	c.upsert(key, value);
	true
}

/// Inserts a control point with its explicit Hermite slope preserved.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_push_point(
	set: *const OfxCurveSet,
	curve: i32,
	point: OakOfxCurvePoint,
) -> bool {
	if set.is_null() || curve < 0 {
		return false;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let Some(curve) = curves.get_mut(curve as usize) else {
		return false;
	};
	let point = point.to_native();
	let index = curve.points.partition_point(|existing| existing.key < point.key);
	if curve.points.get(index).is_some_and(|existing| existing.key == point.key) {
		curve.points[index] = point;
	} else {
		curve.points.insert(index, point);
	}
	true
}

/// Deletes control point `index` of curve `curve` (slopes are
/// recomputed). False on a null handle or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_delete_point(
	set: *const OfxCurveSet,
	curve: i32,
	index: i32,
) -> bool {
	if set.is_null() || curve < 0 || index < 0 {
		return false;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let Some(c) = curves.get_mut(curve as usize) else {
		return false;
	};
	c.delete_nth(index as usize).is_ok()
}

/// Removes all control points of curve `curve` (the curve then
/// evaluates as identity). False on a null handle or an out-of-range
/// curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_clear_curve(set: *const OfxCurveSet, curve: i32) -> bool {
	if set.is_null() || curve < 0 {
		return false;
	}
	let mut curves = unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let Some(c) = curves.get_mut(curve as usize) else {
		return false;
	};
	c.clear();
	true
}

/// Evaluates curve `curve` at `x` (cubic Hermite; out-of-range keys
/// clamp to the endpoint values, an empty curve is identity). NaN on a
/// null handle or an out-of-range curve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_curveset_evaluate(
	set: *const OfxCurveSet,
	curve: i32,
	x: f64,
) -> f64 {
	if set.is_null() || curve < 0 {
		return f64::NAN;
	}
	unsafe { &*set }
		.curves
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.get(curve as usize)
		.map(|c| c.evaluate(x))
		.unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn build_edit_evaluate_roundtrip() {
		unsafe {
			let set = oak_ofx_curveset_new();
			assert!(!set.is_null());
			assert_eq!(oak_ofx_curveset_curve_count(set), 0);

			assert_eq!(oak_ofx_curveset_add_identity_curve(set, 0.0, 1.0), 0);
			assert_eq!(oak_ofx_curveset_point_count(set, 0), 2);
			assert_eq!(oak_ofx_curveset_evaluate(set, 0, 0.5), 0.5);

			// Upsert a midpoint and read it back (sorted by key).
			assert!(oak_ofx_curveset_upsert_point(set, 0, 0.5, 0.25));
			assert_eq!(oak_ofx_curveset_point_count(set, 0), 3);
			let mut p = OakOfxCurvePoint::default();
			assert!(oak_ofx_curveset_point_at(set, 0, 1, &mut p));
			assert_eq!((p.key, p.value), (0.5, 0.25));
			assert!(oak_ofx_curveset_evaluate(set, 0, 0.5).is_finite());

			// JSON roundtrip preserves the model.
			let n = oak_ofx_curveset_to_json(set, std::ptr::null_mut(), 0);
			assert!(n > 0);
			let mut buf = vec![0u8; n as usize + 1];
			assert_eq!(
				oak_ofx_curveset_to_json(set, buf.as_mut_ptr(), buf.len()),
				n
			);
			buf.truncate(n as usize);
			let set2 = oak_ofx_curveset_from_json(buf.as_ptr(), buf.len());
			assert!(!set2.is_null());
			assert_eq!(oak_ofx_curveset_curve_count(set2), 1);
			assert_eq!(oak_ofx_curveset_point_count(set2, 0), 3);
			assert_eq!(
				oak_ofx_curveset_evaluate(set, 0, 0.5),
				oak_ofx_curveset_evaluate(set2, 0, 0.5)
			);

			// Edit: delete, clear, out-of-range failures.
			assert!(oak_ofx_curveset_delete_point(set2, 0, 1));
			assert_eq!(oak_ofx_curveset_point_count(set2, 0), 2);
			assert!(oak_ofx_curveset_clear_curve(set2, 0));
			assert_eq!(oak_ofx_curveset_point_count(set2, 0), 0);
			assert_eq!(
				oak_ofx_curveset_evaluate(set2, 0, 0.7),
				0.7,
				"empty is identity"
			);
			assert!(oak_ofx_curveset_point_at(set2, 0, 0, &mut p) == false);
			assert!(!oak_ofx_curveset_upsert_point(set2, 9, 0.0, 0.0));
			assert!(oak_ofx_curveset_evaluate(set2, 9, 0.0).is_nan());

			oak_ofx_curveset_release(set);
			oak_ofx_curveset_release(set2);
			oak_ofx_curveset_release(std::ptr::null());
		}
	}

	#[test]
	fn null_and_malformed_input_fail_closed() {
		unsafe {
			assert!(oak_ofx_curveset_from_json(std::ptr::null(), 0).is_null());
			assert!(oak_ofx_curveset_from_json("{".as_ptr(), 1).is_null());
			assert_eq!(oak_ofx_curveset_curve_count(std::ptr::null()), -1);
			assert_eq!(
				oak_ofx_curveset_to_json(std::ptr::null(), std::ptr::null_mut(), 0),
				-1
			);
		}
	}
}

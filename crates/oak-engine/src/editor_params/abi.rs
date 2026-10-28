//! C ABI for immutable parameter and keyframe snapshots.

use oak_node::id::NodeId;
use oak_node::value::OakNodeValue;

use crate::codec::copy_str_out;
use crate::coretypes::OakRational;
use crate::editor::EditorSessionHandle;
use crate::editor_params::snapshot::{Param, ParamKeyframe, ParamProperty, ParamSnapshot, ParamValue, build};
use crate::handle::NativeMirror;

/// Opaque immutable parameter snapshot with independent refcounting.
mod keyframes;
pub use keyframes::*;

pub struct EditorParamHandle {
	snapshot: ParamSnapshot,
}

crate::export_handle!(EditorParamHandle, oak_editor_params_add_ref, oak_editor_params_release);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// POD parameter metadata. Strings and nested collections use indexed queries.
pub struct OakParam {
	pub value_type: i32,
	pub flags: u32,
	pub property_count: i32,
	pub value_count: i32,
	pub track_count: i32,
}

impl NativeMirror for OakParam {
	type Native = Param;

	fn from_native(param: &Param) -> Self {
		Self {
			value_type: param.value_type,
			flags: param.flags,
			property_count: param.properties.len() as i32,
			value_count: param.values.len() as i32,
			track_count: param.tracks.len() as i32,
		}
	}

	fn to_native(&self) -> Param {
		Param {
			input_id: String::new(),
			label: String::new(),
			value_type: self.value_type,
			flags: self.flags,
			properties: Vec::new(),
			values: Vec::new(),
			tracks: Vec::new(),
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// POD/string value carrier. `has_text` selects the matching text query.
pub struct OakParamValue {
	pub has_pod: bool,
	pub pod: OakNodeValue,
	pub has_text: bool,
}

impl Default for OakParamValue {
	fn default() -> Self {
		Self {
			has_pod: false,
			pod: OakNodeValue::none(),
			has_text: false,
		}
	}
}

impl NativeMirror for OakParamValue {
	type Native = ParamValue;

	fn from_native(value: &ParamValue) -> Self {
		Self {
			has_pod: value.pod.is_some(),
			pod: value.pod.unwrap_or_else(OakNodeValue::none),
			has_text: value.text.is_some(),
		}
	}

	fn to_native(&self) -> ParamValue {
		ParamValue {
			pod: self.has_pod.then_some(self.pod),
			text: None,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// One input property. The key is retrieved as a bounded string.
pub struct OakParamProperty {
	pub value: OakParamValue,
}

impl NativeMirror for OakParamProperty {
	type Native = ParamProperty;

	fn from_native(property: &ParamProperty) -> Self {
		Self {
			value: OakParamValue::from_native(&property.value),
		}
	}

	fn to_native(&self) -> ParamProperty {
		ParamProperty {
			key: String::new(),
			value: self.value.to_native(),
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// One keyframe with interpolation mode and relative bezier handles.
pub struct OakParamKeyframe {
	pub time: OakRational,
	pub value: OakParamValue,
	pub interpolation: i32,
	pub bezier_in_x: f64,
	pub bezier_in_y: f64,
	pub bezier_out_x: f64,
	pub bezier_out_y: f64,
}

impl Default for OakParamKeyframe {
	fn default() -> Self {
		Self {
			time: OakRational::default(),
			value: OakParamValue::default(),
			interpolation: 0,
			bezier_in_x: 0.0,
			bezier_in_y: 0.0,
			bezier_out_x: 0.0,
			bezier_out_y: 0.0,
		}
	}
}

impl NativeMirror for OakParamKeyframe {
	type Native = ParamKeyframe;

	fn from_native(key: &ParamKeyframe) -> Self {
		Self {
			time: key.time,
			value: OakParamValue::from_native(&key.value),
			interpolation: key.interpolation,
			bezier_in_x: key.bezier_in.0,
			bezier_in_y: key.bezier_in.1,
			bezier_out_x: key.bezier_out.0,
			bezier_out_y: key.bezier_out.1,
		}
	}

	fn to_native(&self) -> ParamKeyframe {
		ParamKeyframe {
			time: self.time,
			value: self.value.to_native(),
			interpolation: self.interpolation,
			bezier_in: (self.bezier_in_x, self.bezier_in_y),
			bezier_out: (self.bezier_out_x, self.bezier_out_y),
		}
	}
}

/// Captures a node's visible parameters. Returns null for a null session or
/// stale node ID. The returned handle is released with `oak_editor_params_release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_snapshot(
	session: *const EditorSessionHandle,
	node: u64,
) -> *mut EditorParamHandle {
	if session.is_null() {
		return std::ptr::null_mut();
	}
	let Some(node) = NodeId::from_identity(node) else {
		return std::ptr::null_mut();
	};
	let project = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.project
		.clone();
	build(&project, node)
		.map(|snapshot| crate::handle::into_ffi(EditorParamHandle { snapshot }) as *mut EditorParamHandle)
		.unwrap_or(std::ptr::null_mut())
}

/// Returns the undo revision captured by this parameter snapshot, or zero for null.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_params_revision(params: *const EditorParamHandle) -> i64 {
	unsafe { params.as_ref() }.map(|p| p.snapshot.revision).unwrap_or(0)
}

/// Returns the number of visible declared parameters, or zero for a null handle.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_params_count(params: *const EditorParamHandle) -> i32 {
	unsafe { params.as_ref() }.map(|p| p.snapshot.params.len() as i32).unwrap_or(0)
}

/// Copies metadata for one parameter. Returns false without writing when the
/// handle/output is null or the index is out of range.
///
/// # Safety
/// `params` must be live and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_at(
	params: *const EditorParamHandle,
	index: i32,
	out: *mut OakParam,
) -> bool {
	if params.is_null() || out.is_null() || index < 0 {
		return false;
	}
	let Some(param) = unsafe { &*params }.snapshot.params.get(index as usize) else {
		return false;
	};
	unsafe { *out = OakParam::from_native(param) };
	true
}

/// Copies a parameter's stable input ID as bounded UTF-8, or returns `-1` for
/// null/invalid indices.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_input_at(
	params: *const EditorParamHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || index < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(index as usize)
		.map(|param| copy_str_out(&param.input_id, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies a parameter's behavior-provided display label as bounded UTF-8.
/// Returns `-1` for null/invalid indices. App language packs are not applied.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_label_at(
	params: *const EditorParamHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || index < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(index as usize)
		.map(|param| copy_str_out(&param.label, buf, buf_len))
		.unwrap_or(-1)
}

/// Returns the element at `index` in a parameter's static-value list, or
/// `i32::MIN` for null/invalid arguments. This avoids imposing an array-size limit.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_params_value_element_at(
	params: *const EditorParamHandle,
	param: i32,
	index: i32,
) -> i32 {
	if params.is_null() || param < 0 || index < 0 {
		return i32::MIN;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.values.get(index as usize))
		.map(|value| value.0)
		.unwrap_or(i32::MIN)
}

/// Returns the element at `index` in a parameter's keyframe-track list, or
/// `i32::MIN` for null/invalid arguments.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_params_track_element_at(
	params: *const EditorParamHandle,
	param: i32,
	index: i32,
) -> i32 {
	if params.is_null() || param < 0 || index < 0 {
		return i32::MIN;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.tracks.get(index as usize))
		.map(|track| track.0)
		.unwrap_or(i32::MIN)
}

/// Copies one property and its POD/string value. Returns false without writing
/// when arguments are null or either index is out of range.
///
/// # Safety
/// `params` must be live and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_property_at(
	params: *const EditorParamHandle,
	param: i32,
	property: i32,
	out: *mut OakParamProperty,
) -> bool {
	if params.is_null() || out.is_null() || param < 0 || property < 0 {
		return false;
	}
	let Some(value) = unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.properties.get(property as usize))
	else {
		return false;
	};
	unsafe { *out = OakParamProperty::from_native(value) };
	true
}

/// Copies a property key such as `combo_option` or `ui_group` as bounded UTF-8.
/// Returns `-1` for null/invalid indices.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_property_key_at(
	params: *const EditorParamHandle,
	param: i32,
	property: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || param < 0 || property < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.properties.get(property as usize))
		.map(|property| copy_str_out(&property.key, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies a property's string value. Returns `-1` for invalid indices or when
/// the property has no string representation.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_property_text_at(
	params: *const EditorParamHandle,
	param: i32,
	property: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || param < 0 || property < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.properties.get(property as usize))
		.and_then(|property| property.value.text.as_deref())
		.map(|text| copy_str_out(text, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies a static value for one parameter element (`-1` for scalar inputs).
/// Returns false without writing for invalid arguments or an absent element.
///
/// # Safety
/// `params` must be live and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_value_at(
	params: *const EditorParamHandle,
	param: i32,
	element: i32,
	out: *mut OakParamValue,
) -> bool {
	if params.is_null() || out.is_null() || param < 0 {
		return false;
	}
	let Some(value) = unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.values.iter().find(|(index, _)| *index == element))
	else {
		return false;
	};
	unsafe { *out = OakParamValue::from_native(&value.1) };
	true
}

/// Copies a static value's string representation. Returns `-1` for invalid
/// arguments or when that element has no string value.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_value_text_at(
	params: *const EditorParamHandle,
	param: i32,
	element: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || param < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.values.iter().find(|(index, _)| *index == element))
		.and_then(|value| value.1.text.as_deref())
		.map(|text| copy_str_out(text, buf, buf_len))
		.unwrap_or(-1)
}


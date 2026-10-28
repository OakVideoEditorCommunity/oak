//! Native mirrors for the parameter snapshot ABI.

use std::ffi::c_void;

use crate::node::NodeValue;
use crate::types::Rational;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ParamRaw {
	pub value_type: i32,
	pub flags: u32,
	pub property_count: i32,
	pub value_count: i32,
	pub track_count: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ValueRaw {
	pub has_pod: bool,
	pub pod: NodeValue,
	pub has_text: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PropertyRaw {
	pub value: ValueRaw,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KeyframeRaw {
	pub time: Rational,
	pub value: ValueRaw,
	pub interpolation: i32,
	pub bezier_in_x: f64,
	pub bezier_in_y: f64,
	pub bezier_out_x: f64,
	pub bezier_out_y: f64,
}

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	pub(crate) fn oak_editor_params_snapshot(session: *mut c_void, node: u64) -> *mut c_void;
	pub(crate) fn oak_editor_params_release(params: *const c_void);
	pub(crate) fn oak_editor_params_revision(params: *const c_void) -> i64;
	pub(crate) fn oak_editor_params_count(params: *const c_void) -> i32;
	pub(crate) fn oak_editor_params_at(params: *const c_void, index: i32, out: *mut ParamRaw) -> bool;
	pub(crate) fn oak_editor_params_input_at(params: *const c_void, index: i32, buf: *mut u8, len: usize) -> i32;
	pub(crate) fn oak_editor_params_label_at(params: *const c_void, index: i32, buf: *mut u8, len: usize) -> i32;
	pub(crate) fn oak_editor_params_value_element_at(params: *const c_void, param: i32, index: i32) -> i32;
	pub(crate) fn oak_editor_params_track_element_at(params: *const c_void, param: i32, index: i32) -> i32;
	pub(crate) fn oak_editor_params_property_at(
		params: *const c_void,
		param: i32,
		property: i32,
		out: *mut PropertyRaw,
	) -> bool;
	pub(crate) fn oak_editor_params_property_key_at(
		params: *const c_void,
		param: i32,
		property: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
	pub(crate) fn oak_editor_params_property_text_at(
		params: *const c_void,
		param: i32,
		property: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
	pub(crate) fn oak_editor_params_value_at(
		params: *const c_void,
		param: i32,
		element: i32,
		out: *mut ValueRaw,
	) -> bool;
	pub(crate) fn oak_editor_params_value_text_at(
		params: *const c_void,
		param: i32,
		element: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
	pub(crate) fn oak_editor_params_keyframe_count(params: *const c_void, param: i32, element: i32) -> i32;
	pub(crate) fn oak_editor_params_keyframe_at(
		params: *const c_void,
		param: i32,
		element: i32,
		index: i32,
		out: *mut KeyframeRaw,
	) -> bool;
	pub(crate) fn oak_editor_params_keyframe_text_at(
		params: *const c_void,
		param: i32,
		element: i32,
		index: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
}

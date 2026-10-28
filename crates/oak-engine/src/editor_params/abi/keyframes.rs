//! Keyframe count, POD data and string-value queries.

use super::{EditorParamHandle, OakParamKeyframe};
use crate::codec::copy_str_out;
use crate::handle::NativeMirror;

/// Returns the number of keys on one parameter element track. Returns zero for
/// invalid arguments and for an absent or empty track.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_params_keyframe_count(
	params: *const EditorParamHandle,
	param: i32,
	element: i32,
) -> i32 {
	if params.is_null() || param < 0 {
		return 0;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.tracks.iter().find(|(index, _)| *index == element))
		.map(|track| track.1.len() as i32)
		.unwrap_or(0)
}

/// Copies one keyframe, including interpolation and relative Bezier handles.
/// Returns false without writing for invalid arguments or an out-of-range key.
///
/// # Safety
/// `params` must be live and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_keyframe_at(
	params: *const EditorParamHandle,
	param: i32,
	element: i32,
	index: i32,
	out: *mut OakParamKeyframe,
) -> bool {
	if params.is_null() || out.is_null() || param < 0 || index < 0 {
		return false;
	}
	let Some(key) = unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.tracks.iter().find(|(index, _)| *index == element))
		.and_then(|track| track.1.get(index as usize))
	else {
		return false;
	};
	unsafe { *out = OakParamKeyframe::from_native(key) };
	true
}

/// Copies a keyframe's string value. Returns `-1` for invalid arguments or when
/// the key has no string representation.
///
/// # Safety
/// `params` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_params_keyframe_text_at(
	params: *const EditorParamHandle,
	param: i32,
	element: i32,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if params.is_null() || param < 0 || index < 0 {
		return -1;
	}
	unsafe { &*params }
		.snapshot
		.params
		.get(param as usize)
		.and_then(|param| param.tracks.iter().find(|(track, _)| *track == element))
		.and_then(|track| track.1.get(index as usize))
		.and_then(|key| key.value.text.as_deref())
		.map(|text| copy_str_out(text, buf, buf_len))
		.unwrap_or(-1)
}

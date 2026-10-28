//! Sequence-local marker and work-area queries for editor sessions.

use crate::codec::copy_str_out;
use crate::coretypes::OakTimeRange;
use crate::editor::EditorSessionHandle;
use crate::handle::NativeMirror;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// Native marker data; the marker name is fetched separately as UTF-8.
pub struct OakEditorMarker {
	pub time: OakTimeRange,
	pub color: i32,
}

/// Returns the marker count, or zero for a null session.
///
/// # Safety
/// A non-null `session` must point to a live editor session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_count(session: *const EditorSessionHandle) -> i32 {
	if session.is_null() {
		return 0;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	// Clone the Arc before releasing the session lock, avoiding nested lock holds.
	markers.lock().unwrap_or_else(|e| e.into_inner()).size() as i32
}

/// Copies marker timing/color at `index`; returns false for invalid arguments
/// or an out-of-range index.
///
/// # Safety
/// Non-null session/output pointers must be valid, and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_at(
	session: *const EditorSessionHandle,
	index: i32,
	out: *mut OakEditorMarker,
) -> bool {
	if session.is_null() || out.is_null() || index < 0 {
		return false;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	let markers = markers.lock().unwrap_or_else(|e| e.into_inner());
	let Some(marker) = markers.at(index as usize) else {
		return false;
	};
	unsafe {
		*out = OakEditorMarker {
			time: OakTimeRange::from_native(marker.time()),
			color: marker.color(),
		}
	};
	true
}

/// Writes a marker name through the standard bounded-string convention.
/// Returns the required byte count, or `-1` for an invalid session/index.
///
/// # Safety
/// Non-null session/buffer pointers must be valid; `buf_len` bytes at `buf`
/// must be writable when `buf` is non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_name_at(
	session: *const EditorSessionHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if session.is_null() || index < 0 {
		return -1;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	let markers = markers.lock().unwrap_or_else(|e| e.into_inner());
	match markers.at(index as usize) {
		Some(marker) => copy_str_out(marker.name(), buf, buf_len),
		None => -1,
	}
}

/// Copies the work-area range and returns whether the work area is enabled.
/// Disabled work areas still report their retained range when arguments are valid.
///
/// # Safety
/// Non-null session/output pointers must be valid, and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_workarea(
	session: *const EditorSessionHandle,
	out: *mut OakTimeRange,
) -> bool {
	if session.is_null() || out.is_null() {
		return false;
	}
	let workarea = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.workarea
		.clone();
	let workarea = workarea.lock().unwrap_or_else(|e| e.into_inner());
	unsafe { *out = OakTimeRange::from_native(workarea.range()) };
	workarea.enabled()
}

#[cfg(test)]
/// Adds one marker directly to session-owned state for auxiliary-query tests.
/// This bypasses undo and is intentionally unavailable in non-test builds.
pub(crate) fn add_marker_for_test(session: *mut EditorSessionHandle, name: &str) {
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	markers
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.add_marker(oak_timeline::marker::TimelineMarker::with_time(
			3,
			oak_core::TimeRange::new(oak_core::Rational::new(1, 1), oak_core::Rational::new(2, 1)),
			name,
		));
}

#[cfg(test)]
mod tests {
	use crate::node::OAK_NODE_ID_INVALID;

	#[test]
	fn null_handles_fail_closed() {
		unsafe {
			assert!(crate::editor::oak_editor_project(std::ptr::null()).is_null());
			assert!(!crate::editor::oak_editor_bind_project_to_library(std::ptr::null_mut()));
			assert!(!crate::editor::oak_editor_unbind(std::ptr::null_mut()));
			assert!(!crate::editor::oak_editor_storage_bound(std::ptr::null()));
			assert!(!crate::editor::oak_editor_set_current_sequence(std::ptr::null_mut(), 0));
			assert_eq!(
				crate::editor::oak_editor_current_sequence(std::ptr::null()),
				OAK_NODE_ID_INVALID
			);
			assert_eq!(crate::editor::aux::oak_editor_marker_count(std::ptr::null()), 0);
			assert_eq!(
				crate::editor::aux::oak_editor_marker_name_at(std::ptr::null(), 0, std::ptr::null_mut(), 0),
				-1
			);
			assert!(!crate::editor::aux::oak_editor_workarea(std::ptr::null(), std::ptr::null_mut()));
			assert_eq!(
				crate::editor::oak_editor_save(std::ptr::null(), "x".as_ptr(), 1, 0),
				oak_storage::error::OAKSTORAGE_E_INVALID
			);
		}
	}
}

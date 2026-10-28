//! Session-local edit subscriptions with deterministic callback-data cleanup.

use std::ffi::c_void;
use std::sync::Arc;

use crate::editor::EditorSessionHandle;

/// Native edit callback: operation code and the revision after command execution.
pub(crate) type EditCallback = unsafe extern "C" fn(*mut c_void, u32, i64);
/// Optional destructor for callback-owned user data.
pub(crate) type DropCallback = unsafe extern "C" fn(*mut c_void);

/// Shared registration cloned out of the session lock before notification.
pub(crate) struct SessionObserver {
	/// Stable ID used for idempotent unsubscribe and Drop cleanup.
	pub(crate) id: u64,
	callback: EditCallback,
	user_data: *mut c_void,
	drop_callback: Option<DropCallback>,
}

// Callback data is foreign-owned. The ABI requires it to be valid for calls and
// safe to release on any thread after the final Arc reference disappears.
unsafe impl Send for SessionObserver {}
unsafe impl Sync for SessionObserver {}

impl Drop for SessionObserver {
	fn drop(&mut self) {
		if let Some(drop_callback) = self.drop_callback {
			unsafe { drop_callback(self.user_data) };
		}
	}
}

/// Registers one callback and returns its nonzero subscription ID, or zero for
/// invalid arguments. Data is owned by the registration thereafter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_edit_subscribe(
	session: *mut EditorSessionHandle,
	callback: Option<EditCallback>,
	user_data: *mut c_void,
	drop_callback: Option<DropCallback>,
) -> u64 {
	if session.is_null() {
		return 0;
	}
	let Some(callback) = callback else {
		return 0;
	};
	let mut state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let id = state.next_observer_id;
	state.next_observer_id = state.next_observer_id.wrapping_add(1).max(1);
	state.observers.push(Arc::new(SessionObserver {
		id,
		callback,
		user_data,
		drop_callback,
	}));
	id
}

/// Removes one subscription. Returns false for null or an unknown ID. Callback
/// data is destroyed after any in-flight shared notification releases its Arc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_edit_unsubscribe(
	session: *mut EditorSessionHandle,
	id: u64,
) -> bool {
	if session.is_null() || id == 0 {
		return false;
	}
	let mut state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let before = state.observers.len();
	state.observers.retain(|observer| observer.id != id);
	before != state.observers.len()
}

/// Calls a snapshot of current subscribers outside the session mutex. Panics in
/// foreign callbacks are contained so they cannot cross the C ABI or abort edit.
pub(crate) fn notify(session: &EditorSessionHandle, op: u32) {
	let (observers, revision) = {
		let state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
		(state.observers.clone(), oak_undo::global::index().unwrap_or(0))
	};
	for observer in observers {
		let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
			(observer.callback)(observer.user_data, op, revision);
		}));
	}
}

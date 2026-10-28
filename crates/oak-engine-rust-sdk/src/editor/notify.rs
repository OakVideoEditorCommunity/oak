//! RAII subscriptions for successful session command notifications.

use std::ffi::c_void;
use std::sync::Mutex;

use crate::editor::EditorSession;

#[cfg(test)]
mod tests;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_edit_subscribe(
		session: *mut c_void,
		callback: Option<unsafe extern "C" fn(*mut c_void, u32, i64)>,
		user_data: *mut c_void,
		drop_callback: Option<unsafe extern "C" fn(*mut c_void)>,
	) -> u64;
	fn oak_editor_edit_unsubscribe(session: *mut c_void, id: u64) -> bool;
}

/// Serialized Rust callback storage. The mutex also makes `FnMut` safe when
/// notifications from concurrent command threads overlap.
struct ObserverCallback {
	callback: Mutex<Box<dyn FnMut(u32, i64) + Send>>,
}

unsafe extern "C" fn call_observer(data: *mut c_void, op: u32, revision: i64) {
	let observer = unsafe { &*(data as *const ObserverCallback) };
	let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		let mut callback = observer.callback.lock().unwrap_or_else(|e| e.into_inner());
		callback(op, revision);
	}));
}

unsafe extern "C" fn drop_observer(data: *mut c_void) {
	drop(unsafe { Box::from_raw(data as *mut ObserverCallback) });
}

/// A live edit subscription. Dropping it unregisters the callback and releases
/// its Rust closure after any in-flight notification has finished.
pub struct EditSubscription {
	session: EditorSession,
	id: u64,
	active: bool,
}

impl EditSubscription {
	/// Unregisters immediately. Repeated calls and Drop after this are no-ops.
	pub fn unsubscribe(mut self) {
		self.unsubscribe_inner();
	}

	/// Whether this handle has not been explicitly unsubscribed.
	pub fn is_active(&self) -> bool {
		self.active
	}

	fn unsubscribe_inner(&mut self) {
		if self.active {
			unsafe { oak_editor_edit_unsubscribe(self.session.ptr, self.id) };
			self.active = false;
		}
	}
}

impl Drop for EditSubscription {
	fn drop(&mut self) {
		self.unsubscribe_inner();
	}
}

impl EditorSession {
	/// Subscribes to successful semantic commands executed on this session.
	/// Callbacks receive `(operation_code, revision)` and run synchronously after
	/// the session lock is released. Domain failures do not emit notifications.
	/// Output-buffer errors can still emit because the command already executed.
	pub fn subscribe_edit(
		&self,
		callback: impl FnMut(u32, i64) + Send + 'static,
	) -> Option<EditSubscription> {
		let observer = Box::new(ObserverCallback {
			callback: Mutex::new(Box::new(callback)),
		});
		let data = Box::into_raw(observer) as *mut c_void;
		let id = unsafe {
			oak_editor_edit_subscribe(self.ptr, Some(call_observer), data, Some(drop_observer))
		};
		if id == 0 {
			unsafe { drop_observer(data) };
			return None;
		}
		Some(EditSubscription {
			session: self.clone(),
			id,
			active: true,
		})
	}
}

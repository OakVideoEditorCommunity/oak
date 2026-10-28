//! C ABI additions for oak-undo (prefix `oak_undo_`): history
//! navigation/inspection and change observers.
//!
//! The base surface (can_undo/can_redo/count/index,
//! group_begin/end/abort, undo/redo) lives in [`crate::node`] next to
//! the project API; this module completes it with history jumping,
//! clearing, row inspection (the history panel's data source), and the
//! observer hook functional plugins use as the "project modified"
//! signal.

use std::ffi::c_void;
use std::sync::Mutex;

use crate::codec::{copy_str_out, str_arg};
use oak_undo::undocommand::UndoCommand;

/// Jumps to history position `index` (undoing/redoing as needed).
/// False on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_jump(index: i64) -> bool {
	oak_undo::global::jump(index).is_ok()
}

/// Clears the undo stack. False on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_clear() -> bool {
	oak_undo::global::clear().is_ok()
}

/// Takes ownership of one migration command and records it on this engine's
/// process-wide undo stack. The pointer must come from `Box<UndoCommand>`;
/// ownership is transferred exactly once and the command is dropped on errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_undo_push_owned(
	command: *mut c_void,
	name: *const u8,
	name_len: usize,
) -> bool {
	if command.is_null() {
		return false;
	}
	let command = unsafe { Box::from_raw(command as *mut UndoCommand) };
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return false;
	};
	oak_undo::global::push(*command, name).is_ok()
}

/// The display name of history row `row`, snprintf-style; -1 when out
/// of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_undo_command_name(row: i64, buf: *mut u8, buf_len: usize) -> i32 {
	match oak_undo::global::command_name(row) {
		Ok(name) => copy_str_out(&name, buf, buf_len),
		Err(_) => -1,
	}
}

/// Whether history row `row` is currently in the done state: 0/1, -1
/// when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_command_done(row: i64) -> i32 {
	match oak_undo::global::command_done(row) {
		Ok(done) => done as i32,
		Err(_) => -1,
	}
}

/// A change observer: called after any undoable command lands (push,
/// undo, redo, jump, clear).
pub type OakUndoObserverFn = unsafe extern "C" fn(userdata: *mut c_void);

// The registry holds (callback, userdata) pairs; the userdata pointer
// is opaque consumer state kept alive by the caller for the
// registration's lifetime (there is no removal — matching
// oak_undo::global::add_observer).
static OBSERVERS: Mutex<Vec<(OakUndoObserverFn, usize)>> = Mutex::new(Vec::new());

// Registered with oak_undo once, on the first observer registration.
static TRAMPOLINE_ONCE: std::sync::Once = std::sync::Once::new();

fn dispatch_observers() {
	let observers = OBSERVERS.lock().unwrap_or_else(|e| e.into_inner());
	for (cb, userdata) in observers.iter() {
		unsafe { cb(*userdata as *mut c_void) };
	}
}

/// Registers a change observer (registration-only; there is no
/// removal, matching the underlying API). Multiple observers may be
/// registered; a null callback is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_undo_add_observer(
	cb: Option<OakUndoObserverFn>,
	userdata: *mut c_void,
) {
	let Some(cb) = cb else {
		return;
	};
	TRAMPOLINE_ONCE.call_once(|| {
		oak_undo::global::add_observer(dispatch_observers);
	});
	OBSERVERS
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.push((cb, userdata as usize));
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};

	#[test]
	fn history_rows_and_observer() {
		// Serialize: the undo stack is process-global (shared with the
		// node tests).
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());

		static SEEN: AtomicUsize = AtomicUsize::new(0);
		unsafe extern "C" fn observer(userdata: *mut c_void) {
			assert!(userdata.is_null());
			SEEN.fetch_add(1, Ordering::SeqCst);
		}
		unsafe { oak_undo_add_observer(Some(observer), std::ptr::null_mut()) };

		unsafe {
			let project = crate::node::oak_project_new();
			let base = crate::node::oak_undo_count();
			let (n, nl) = ("A Folder".as_ptr(), 8);
			let folder = crate::node::oak_folder_create(
				project,
				crate::node::oak_project_root(project),
				n,
				nl,
			);
			assert!(crate::node::oak_node_id_is_valid(folder));
			assert_eq!(crate::node::oak_undo_count(), base + 1);
			assert!(SEEN.load(Ordering::SeqCst) > 0, "the observer fired");

			// Row inspection: the newest row is the folder creation.
			let mut buf = [0u8; 64];
			let len = oak_undo_command_name(base, buf.as_mut_ptr(), buf.len());
			assert!(len > 0);
			assert_eq!(oak_undo_command_done(base), 1);
			assert_eq!(oak_undo_command_name(-1, buf.as_mut_ptr(), 64), -1);

			// Jump back to base and forward again.
			assert!(oak_undo_jump(base));
			assert_eq!(oak_undo_command_done(base), 0);
			assert!(oak_undo_jump(base + 1));
			assert_eq!(oak_undo_command_done(base), 1);

			crate::node::oak_project_release(project);
			assert!(oak_undo_clear());
			// clear() pushes a fresh empty command (the "New Project" row).
			assert_eq!(crate::node::oak_undo_count(), 1);
		}
	}
}

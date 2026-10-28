//! Migration-only ownership transfer for legacy Rust undo commands.
//!
//! New consumers must use semantic `EditCommand` values instead. This module
//! exists so an existing host can move its remaining `UndoCommand` values onto
//! the same engine stack used by `EditorSession`, without maintaining two histories.

use oak_undo::undocommand::UndoCommand;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_undo_push_owned(command: *mut std::ffi::c_void, name: *const u8, len: usize) -> bool;
}

/// Moves an owned legacy command onto the engine's undo stack. The command is
/// consumed exactly once, including error paths. Returns the engine push status.
pub fn undo_push(command: UndoCommand, name: &str) -> bool {
	let raw = Box::into_raw(Box::new(command)) as *mut std::ffi::c_void;
	unsafe { oak_undo_push_owned(raw, name.as_ptr(), name.len()) }
}

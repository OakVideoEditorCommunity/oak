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

//! Edge-path coverage: error-code mapping, `Default` impls, the
//! prepared flag, and the `push_pre_executed` redo-tail/cap paths.
//! Everything here goes through the crate's public value-typed API
//! (the former handle-level/refcount tests were removed together with
//! the `CHandle` layer they exercised).

use oak_undo::error::{
	Error, OAKUNDO_E_FAILED, OAKUNDO_E_INVALID, OAKUNDO_E_NOMEM, OAKUNDO_E_NOT_FOUND,
	OAKUNDO_E_STATE,
};
use oak_undo::undocommand::{MultiUndoCommand, UndoCommand};
use oak_undo::undostack::{EmptyCommand, UndoStack, K_MAX_UNDO_COMMANDS};

/// Every `Error` variant maps to its documented public code.
#[test]
fn error_code_mapping_is_complete() {
	assert_eq!(Error::Invalid.code(), OAKUNDO_E_INVALID);
	assert_eq!(Error::State.code(), OAKUNDO_E_STATE);
	assert_eq!(Error::Failed("ctx".to_string()).code(), OAKUNDO_E_FAILED);
	assert_eq!(Error::NotFound.code(), OAKUNDO_E_NOT_FOUND);
	assert_eq!(Error::NoMem.code(), OAKUNDO_E_NOMEM);
}

/// `Default` impls mirror `new()`.
#[test]
fn default_impls_match_new() {
	let _empty = EmptyCommand;
	let stack = UndoStack::default();
	assert_eq!(
		stack.done_count(),
		1,
		"fresh stack holds the bottom command"
	);
	assert!(!stack.can_undo());
	let multi = MultiUndoCommand::default();
	assert_eq!(multi.child_count(), 0);
}

/// `set_prepared` is idempotent and `has_prepared` reflects it.
#[test]
fn prepared_flag_roundtrip() {
	let mut cmd = UndoCommand::from_closures(|| {}, || {});
	assert!(cmd.has_prepared());
	cmd.set_prepared();
	assert!(cmd.has_prepared());
}

/// `push_pre_executed` drops the redoable tail and evicts the oldest row
/// past the cap (mirrors `push`).
#[test]
fn push_pre_executed_clears_redo_tail_and_caps() {
	let mut stack = UndoStack::new();
	let name = "row";

	// Push two, undo one, then push_pre_executed: redo tail is dropped.
	for _ in 0..2 {
		stack.push(UndoCommand::from_closures(|| {}, || {}), name);
	}
	stack.undo().unwrap();
	stack.push_pre_executed(UndoCommand::from_closures(|| {}, || {}), name);
	assert!(
		!stack.can_redo(),
		"push_pre_executed drops the redoable tail"
	);

	// Fill past the cap with pre-executed commands: the oldest rows are
	// evicted and the count stays at K_MAX_UNDO_COMMANDS.
	for _ in 0..(K_MAX_UNDO_COMMANDS + 10) {
		stack.push_pre_executed(UndoCommand::from_closures(|| {}, || {}), name);
	}
	assert_eq!(
		stack.done_count(),
		K_MAX_UNDO_COMMANDS as i64,
		"pre-executed rows evict at the cap"
	);
}

/// A command value is moved, not copied: moving it into a stack leaves no
/// usable alias behind (the compiler enforces this; the assertion pins the
/// ownership semantics the old handle shells emulated).
#[test]
fn command_value_moves_into_stack() {
	let mut stack = UndoStack::new();
	stack.push(UndoCommand::from_closures(|| {}, || {}), "A");
	stack.push(UndoCommand::from_closures(|| {}, || {}), "B");
	assert_eq!(stack.command_count(), 3);
	assert!(stack.can_undo());
	stack.undo().unwrap();
	assert!(stack.can_redo());
	assert_eq!(stack.done_count(), 2);
	assert_eq!(stack.command_name(2).unwrap(), "B"); // undone row still labeled
}

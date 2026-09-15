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

//! `olive::UndoStack` — the undo/redo history stack. Mirrors
//! `src/undo/src/undostack.h`.
//!
//! Two deques: `commands` (done commands, oldest at the front) and
//! `undone` (undone commands, most-recently-undone at the front). The
//! fresh stack holds a single "New/Open Project" empty command so that
//! `can_undo` is false at the bottom (see `undostack.cpp::clear`).
//!
//! The former handle-level exports (`undostack_init` … `undostack_free`)
//! are gone with the C ABI: the stack is a plain owned value, shared
//! process-wide through the [`crate::global`] module.

use std::collections::VecDeque;

use crate::error::Result;
use crate::undocommand::UndoCommand;

/// Maximum number of retained history rows (`k_max_undo_commands`).
pub const K_MAX_UNDO_COMMANDS: usize = 200;

/// The invariant bottom-of-stack command ("New/Open Project"); not
/// undoable, so `can_undo` returns false at index 0.
pub struct EmptyCommand;

impl EmptyCommand {
	/// A no-op command (redo/undo do nothing).
	pub fn new() -> UndoCommand {
		UndoCommand::empty()
	}
}

impl Default for EmptyCommand {
	fn default() -> Self {
		EmptyCommand
	}
}

/// One history row: the command plus its user-visible label.
pub struct CommandEntry {
	/// The command.
	pub command: UndoCommand,
	/// User-visible label.
	pub name: String,
}

/// `olive::UndoStack`.
pub struct UndoStack {
	/// Done commands, oldest at the front.
	commands: VecDeque<CommandEntry>,
	/// Undone commands, most-recently-undone at the front.
	undone: VecDeque<CommandEntry>,
}

impl UndoStack {
	/// New stack; contains a single empty "New/Open Project" command.
	pub fn new() -> Self {
		let mut stack = UndoStack {
			commands: VecDeque::new(),
			undone: VecDeque::new(),
		};
		stack.clear();
		stack
	}

	/// Push `command` and execute its redo; drops any redoable tail and
	/// evicts the oldest row beyond the cap.
	pub fn push(&mut self, command: UndoCommand, name: &str) {
		if command.is_multi() && command.multi_child_count() == 0 {
			return;
		}
		if self.can_redo() {
			self.undone.clear();
		}
		let mut command = command;
		command.redo_and_set_modified();
		self.commands.push_back(CommandEntry {
			command,
			name: name.to_string(),
		});
		if self.commands.len() > K_MAX_UNDO_COMMANDS {
			self.commands.pop_front();
		}
	}

	/// Push an already-executed command (redo skipped); `set_done(true)`.
	pub fn push_pre_executed(&mut self, command: UndoCommand, name: &str) {
		if command.is_multi() && command.multi_child_count() == 0 {
			return;
		}
		if self.can_redo() {
			self.undone.clear();
		}
		let mut command = command;
		command.set_done(true);
		self.commands.push_back(CommandEntry {
			command,
			name: name.to_string(),
		});
		if self.commands.len() > K_MAX_UNDO_COMMANDS {
			self.commands.pop_front();
		}
	}

	/// Undo/redo until the done-command count equals `index`.
	pub fn jump(&mut self, index: i64) {
		let index = index.max(0);
		while (self.commands.len() as i64) > index && self.can_undo() {
			let _ = self.undo();
		}
		while (self.commands.len() as i64) < index && self.can_redo() {
			let _ = self.redo();
		}
	}

	/// Delete all commands and push a fresh empty command.
	pub fn clear(&mut self) {
		self.commands.clear();
		self.undone.clear();
		self.commands.push_back(CommandEntry {
			command: EmptyCommand::new(),
			name: "New/Open Project".to_string(),
		});
	}

	/// Whether an undo is possible (more than the bottom empty command).
	pub fn can_undo(&self) -> bool {
		!self.commands.is_empty()
			&& !self
				.commands
				.back()
				.is_none_or(|entry| entry.command.is_empty())
	}

	/// Whether a redo is possible.
	pub fn can_redo(&self) -> bool {
		!self.undone.is_empty()
	}

	/// Undo the most recently done command, if any.
	pub fn undo(&mut self) -> Result<()> {
		if self.can_undo() {
			if let Some(entry) = self.commands.pop_back() {
				let mut command = entry.command;
				command.undo_and_set_modified();
				self.undone.push_front(CommandEntry {
					command,
					name: entry.name,
				});
			}
		}
		Ok(())
	}

	/// Redo the most recently undone command, if any.
	pub fn redo(&mut self) -> Result<()> {
		if self.can_redo() {
			if let Some(entry) = self.undone.pop_front() {
				let mut command = entry.command;
				command.redo_and_set_modified();
				self.commands.push_back(CommandEntry {
					command,
					name: entry.name,
				});
			}
		}
		Ok(())
	}

	/// Total number of history rows (done + undone commands).
	pub fn command_count(&self) -> i64 {
		(self.commands.len() + self.undone.len()) as i64
	}

	/// Number of done commands (the current position).
	pub fn done_count(&self) -> i64 {
		self.commands.len() as i64
	}

	/// Whether the row at `row` is currently done.
	pub fn command_is_done(&self, row: i64) -> Result<bool> {
		if row < 0 || row >= self.command_count() {
			return Err(crate::error::Error::NotFound);
		}
		Ok(row < self.commands.len() as i64)
	}

	/// Label of the history row at `row`.
	pub fn command_name(&self, row: i64) -> Result<&str> {
		if row < 0 || row >= self.command_count() {
			return Err(crate::error::Error::NotFound);
		}
		let row = row as usize;
		if row < self.commands.len() {
			Ok(self.commands[row].name.as_str())
		} else {
			Ok(self.undone[row - self.commands.len()].name.as_str())
		}
	}
}

impl Default for UndoStack {
	fn default() -> Self {
		Self::new()
	}
}

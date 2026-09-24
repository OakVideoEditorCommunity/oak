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

//! Pointer-edit commands (`src/timeline/src/timelineundopointer.h`): trims
//! that compensate the adjacent block (`BlockTrimCommand`), slides
//! (`TrackSlideCommand`) and destructive placement (`TrackPlaceBlockCommand`).
//!
//! Graph mutation routes directly through the oaknode Rust domain (the
//! oaknode C ABI was deleted in the single-lib unification); command
//! wrapping through `oak_undo::undocommand::UndoCommand` values.

use oak_core::{Rational, TimeRange};
use oak_node::graph::NodeEntry;
use oak_undo::undocommand::UndoCommand;

use crate::common::MovementMode;
use crate::undocommon::{
	block_can_be_removed, box_command, create_and_run_block_remove_command,
	create_block_remove_command, Command, MultiUndoCommand,
};
use crate::util::{
	block_add_to_graph, block_gap_create, block_in, block_kind, block_length, block_next,
	block_out, block_previous, block_remove_from_graph, block_set_in,
	block_set_length_and_media_out, block_set_length_keeping_out, block_track,
	track_append_block, track_insert_block_after, track_insert_block_before, track_length,
	track_ripple_remove_block, tracklist_track_at, tracklist_track_count, BlockKind, NodeRef,
};

use super::undogeneral::{TimelineAddTrackCommand, TrackReplaceBlockWithGapCommand};
use super::undoripple::TrackRippleRemoveAreaCommand;

/// `BlockTrimCommand` — trim one block, compensating the block directly
/// adjacent so the rest of the track is unaffected (timelineundopointer.h).
///
/// By default only gaps are shortened; when the adjacent block must grow and
/// is not a gap, a gap is inserted. Set [`Self::set_trim_is_a_roll_edit`] to
/// always trim into the adjacent block instead.
pub struct BlockTrimCommand {
	/// Owning track (`track_`).
	track: NodeRef,
	/// Block to trim (`block_`).
	block: NodeRef,
	/// New length (`new_length_`).
	new_length: Rational,
	/// Trim direction (`mode_`).
	mode: MovementMode,
	/// Whether `prepare` found the length unchanged, so `redo`/`undo` no-op
	/// (`doing_nothing_`).
	doing_nothing_: bool,
	/// Signed difference `old_length - new_length`; positive when trimming
	/// shorter (`trim_diff_`).
	trim_diff_: Rational,
	/// Block length before the trim (`old_length_`).
	old_length_: Rational,
	/// The block to compensate (`adjacent_`).
	adjacent_: Option<NodeRef>,
	/// Whether the adjacent block is required for this trim (`needs_adjacent_`).
	needs_adjacent_: bool,
	/// Whether `adjacent_` was created by this command (`we_created_adjacent_`).
	we_created_adjacent_: bool,
	/// Whether the adjacent block is removed by this trim (`we_removed_adjacent_`).
	we_removed_adjacent_: bool,
	/// Graph-removal command for a removed adjacent (`deleted_adjacent_command_`).
	deleted_adjacent_command_: Option<UndoCommand>,
	/// Always trim into the adjacent block instead of creating a gap
	/// (`trim_is_a_roll_edit_`).
	trim_is_a_roll_edit_: bool,
	/// Remove a zero-length adjacent block from the whole graph (default true)
	/// rather than only from the track (`remove_block_from_graph_`).
	remove_block_from_graph_: bool,
	/// Arena entry of a created adjacent gap while it is detached from the
	/// graph (between `undo` and the next `redo`).
	adjacent_entry_: Option<NodeEntry>,
}

impl BlockTrimCommand {
	/// Construct from track + block + new length + movement mode.
	///
	/// New signature (single-lib): `pub fn new(track: NodeRef, block: NodeRef, new_length: Rational, mode: MovementMode) -> BlockTrimCommand`
	pub fn new(track: NodeRef, block: NodeRef, new_length: Rational, mode: MovementMode) -> Self {
		Self {
			track,
			block,
			new_length,
			mode,
			doing_nothing_: false,
			trim_diff_: Rational::new(0, 1),
			old_length_: Rational::new(0, 1),
			adjacent_: None,
			needs_adjacent_: false,
			we_created_adjacent_: false,
			we_removed_adjacent_: false,
			deleted_adjacent_command_: None,
			trim_is_a_roll_edit_: false,
			remove_block_from_graph_: true,
			adjacent_entry_: None,
		}
	}

	/// Always trim the adjacent block instead of creating a gap
	/// (`set_trim_is_a_roll_edit`).
	pub fn set_trim_is_a_roll_edit(&mut self, e: bool) {
		self.trim_is_a_roll_edit_ = e;
	}

	/// Whether an adjacent block shortened to zero length is removed from the
	/// whole graph (default true) or only from the track
	/// (`set_remove_zero_length_from_graph`).
	pub fn set_remove_zero_length_from_graph(&mut self, e: bool) {
		self.remove_block_from_graph_ = e;
	}

	/// `prepare`: compute the trim delta and whether an adjacent block is needed.
	pub fn prepare(&mut self) {
		// Store old length
		self.old_length_ = block_length(&self.block);

		// If the length isn't changing, set a flag to do nothing
		if self.old_length_ == self.new_length {
			self.doing_nothing_ = true;
			return;
		}
		self.doing_nothing_ = false;

		// Positive when trimming shorter, negative when trimming longer
		self.trim_diff_ = self.old_length_ - self.new_length;

		// Retrieve our adjacent block (or None if there is none)
		self.adjacent_ = if self.mode == MovementMode::TrimIn {
			block_previous(&self.block)
		} else {
			block_next(&self.block)
		};

		// Ignore when trimming the out with no adjacent, because the user must
		// have trimmed the end of the last block in the track
		self.needs_adjacent_ = self.mode == MovementMode::TrimIn || self.adjacent_.is_some();

		if self.needs_adjacent_ {
			// If we're trimming shorter, we need an adjacent, so check if we have a
			// viable one
			let adjacent_kind = self.adjacent_.as_ref().map(block_kind).unwrap_or(BlockKind::Other);
			self.we_created_adjacent_ = self.trim_diff_ > Rational::new(0, 1)
				&& (self.adjacent_.is_none()
					|| (adjacent_kind != BlockKind::Gap && !self.trim_is_a_roll_edit_));

			if self.we_created_adjacent_ {
				// We shortened but don't have a viable adjacent to lengthen, so create
				// one filling exactly the space the trim freed. The module stores
				// positions on the block (Olive derives the inserted gap's position
				// from the track order), so the in point is the freed space's start
				// and the length closes the gap up to the block's new edge:
				// trim-in frees [in, in + diff) (the gap is inserted before the
				// block), trim-out frees [out - diff, out) (inserted after it).
				self.adjacent_ = Some(block_gap_create(&self.track.project));
				if let Some(gap) = &self.adjacent_ {
					if self.mode == MovementMode::TrimIn {
						block_set_in(gap, block_in(&self.block));
					} else {
						block_set_in(gap, block_out(&self.block) - self.trim_diff_);
					}
					block_set_length_and_media_out(gap, self.trim_diff_);
				}
			} else if let Some(adjacent) = &self.adjacent_ {
				// Determine if we're removing the adjacent
				self.we_removed_adjacent_ =
					(block_length(adjacent) + self.trim_diff_).is_null();
			}
		}
	}

	/// `redo`: apply the trim, creating/removing the adjacent block as needed.
	pub fn redo(&mut self) {
		if self.doing_nothing_ {
			return;
		}

		if self.mode == MovementMode::TrimIn {
			// Trim-in keeps the out fixed and moves the in (media follows).
			block_set_length_keeping_out(&self.block, self.new_length);
		} else {
			// Trim-out keeps the in fixed and moves the out (media untouched).
			block_set_length_and_media_out(&self.block, self.new_length);
		}

		if self.needs_adjacent_ {
			if self.we_created_adjacent_ {
				// Add the adjacent and insert it
				if let Some(gap) = &self.adjacent_ {
					block_add_to_graph(gap, self.adjacent_entry_.take());
					if self.mode == MovementMode::TrimIn {
						track_insert_block_before(&self.track, gap, &self.block);
					} else {
						track_insert_block_after(&self.track, gap, Some(&self.block));
					}
				}
			} else if self.we_removed_adjacent_ {
				if let Some(adjacent) = &self.adjacent_ {
					track_ripple_remove_block(&self.track, adjacent);

					// It no longer inputs/outputs anything, remove it
					if self.remove_block_from_graph_ && block_can_be_removed(adjacent) {
						if self.deleted_adjacent_command_.is_none() {
							self.deleted_adjacent_command_ =
								Some(create_and_run_block_remove_command(adjacent));
						} else if let Some(c) = self.deleted_adjacent_command_.as_mut() {
							c.redo_now();
						}
					}
				}
			} else if let Some(adjacent) = &self.adjacent_ {
				let adjacent_length = block_length(adjacent) + self.trim_diff_;

				if self.mode == MovementMode::TrimIn {
					// The previous neighbour grows/shrinks at its out edge
					// to meet the trimmed in point.
					block_set_length_and_media_out(adjacent, adjacent_length);
				} else {
					// The next neighbour grows/shrinks at its in edge to
					// meet the trimmed out point (its content end anchored).
					block_set_length_keeping_out(adjacent, adjacent_length);
				}
			}
		}
	}

	/// `undo`: revert the trim and any adjacent-block changes.
	pub fn undo(&mut self) {
		if self.doing_nothing_ {
			return;
		}

		// `trim_diff_` is positive when trimming shorter, negative when trimming
		// longer
		if self.needs_adjacent_ {
			if self.we_created_adjacent_ {
				// The adjacent is ours, just delete it
				if let Some(gap) = &self.adjacent_ {
					track_ripple_remove_block(&self.track, gap);
					if self.adjacent_entry_.is_none() {
						self.adjacent_entry_ = block_remove_from_graph(gap);
					}
				}
			} else if let Some(adjacent) = &self.adjacent_ {
				if self.we_removed_adjacent_ {
					if let Some(c) = self.deleted_adjacent_command_.as_mut() {
						// We deleted the adjacent, restore it now
						c.undo_now();
					}

					if self.mode == MovementMode::TrimIn {
						track_insert_block_before(&self.track, adjacent, &self.block);
					} else {
						track_insert_block_after(&self.track, adjacent, Some(&self.block));
					}
				} else {
					let adjacent_length = block_length(adjacent) - self.trim_diff_;

					if self.mode == MovementMode::TrimIn {
						block_set_length_and_media_out(adjacent, adjacent_length);
					} else {
						block_set_length_keeping_out(adjacent, adjacent_length);
					}
				}
			}
		}

		if self.mode == MovementMode::TrimIn {
			block_set_length_keeping_out(&self.block, self.old_length_);
		} else {
			block_set_length_and_media_out(&self.block, self.old_length_);
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for BlockTrimCommand {
	/// `Command::redo` — the inherent method takes precedence.
	fn redo(&mut self) {
		self.redo();
	}

	/// `Command::undo` — the inherent method takes precedence.
	fn undo(&mut self) {
		self.undo();
	}
}

/// `TrackSlideCommand` — slide a set of blocks along a track by `movement`,
/// trimming or inserting the in/out adjacent blocks to compensate
/// (timelineundopointer.h).
pub struct TrackSlideCommand {
	/// Owning track (`track_`).
	track: NodeRef,
	/// Blocks to slide (`blocks_`; non-empty for a valid command).
	blocks: Vec<NodeRef>,
	/// Signed movement (`movement_`).
	movement: Rational,
	/// Whether `prepare` created the in adjacent (`we_created_in_adjacent_`).
	we_created_in_adjacent_: bool,
	/// Whether the slide removes the in adjacent (`we_removed_in_adjacent_`).
	we_removed_in_adjacent_: bool,
	/// Block adjacent on the in side (`in_adjacent_`).
	in_adjacent: Option<NodeRef>,
	/// Graph-removal command for a removed in adjacent.
	in_adjacent_remove_command_: Option<UndoCommand>,
	/// Arena entry of a created in adjacent gap while detached from the graph.
	in_adjacent_entry_: Option<NodeEntry>,
	/// Whether `prepare` created the out adjacent (`we_created_out_adjacent_`).
	we_created_out_adjacent_: bool,
	/// Whether the slide removes the out adjacent (`we_removed_out_adjacent_`).
	we_removed_out_adjacent_: bool,
	/// Block adjacent on the out side (`out_adjacent_`).
	out_adjacent: Option<NodeRef>,
	/// Graph-removal command for a removed out adjacent.
	out_adjacent_remove_command_: Option<UndoCommand>,
	/// Arena entry of a created out adjacent gap while detached from the graph.
	out_adjacent_entry_: Option<NodeEntry>,
}

impl TrackSlideCommand {
	/// Construct from track + moving blocks + in/out adjacent blocks + movement.
	///
	/// New signature (single-lib): `pub fn new(track: NodeRef, blocks: Vec<NodeRef>, in_adjacent: Option<NodeRef>, out_adjacent: Option<NodeRef>, movement: Rational) -> TrackSlideCommand`
	pub fn new(
		track: NodeRef,
		blocks: Vec<NodeRef>,
		in_adjacent: Option<NodeRef>,
		out_adjacent: Option<NodeRef>,
		movement: Rational,
	) -> Self {
		Self {
			track,
			blocks,
			movement,
			we_created_in_adjacent_: false,
			we_removed_in_adjacent_: false,
			in_adjacent,
			in_adjacent_remove_command_: None,
			in_adjacent_entry_: None,
			we_created_out_adjacent_: false,
			we_removed_out_adjacent_: false,
			out_adjacent,
			out_adjacent_remove_command_: None,
			out_adjacent_entry_: None,
		}
	}

	/// `prepare`: capture adjacent state.
	pub fn prepare(&mut self) {
		if self.in_adjacent.is_none() {
			self.in_adjacent = Some(block_gap_create(&self.track.project));
			if let Some(gap) = &self.in_adjacent {
				// A created in adjacent only makes sense when sliding left
				// (movement < 0): the gap fills [in0 + movement, in0) ahead
				// of the first block. Positions are stored on the block in
				// the module model, so both ends are written explicitly.
				block_set_in(gap, block_in(&self.blocks[0]) + self.movement);
				block_set_length_and_media_out(gap, Rational::new(0, 1) - self.movement);
			}
			self.we_created_in_adjacent_ = true;
		} else {
			self.we_created_in_adjacent_ = false;
		}

		if self.out_adjacent.is_none()
			&& block_next(self.blocks.last().expect("non-empty blocks")).is_some()
		{
			self.out_adjacent = Some(block_gap_create(&self.track.project));
			if let Some(gap) = &self.out_adjacent {
				// A created out adjacent only makes sense when sliding right
				// (movement > 0): the gap fills [last_out, last_out + movement)
				// after the last block.
				block_set_in(gap, block_out(self.blocks.last().expect("non-empty blocks")));
				block_set_length_and_media_out(gap, self.movement);
			}
			self.we_created_out_adjacent_ = true;
		} else {
			self.we_created_out_adjacent_ = false;
		}
	}

	/// `redo`: apply the slide.
	///
	/// The C++ derives the sliding blocks' new positions from the track
	/// layout (the resized neighbours shift them implicitly); the
	/// stored-range model has no layout, so the CALLER positions the
	/// `blocks` by `movement` before running this command — it only owns
	/// the adjacent resize/insert/remove half of the slide.
	pub fn redo(&mut self) {
		// We will always have an in adjacent if there was a valid slide
		if self.we_created_in_adjacent_ {
			// We created the in adjacent, so all we have to do is insert it
			if let Some(gap) = &self.in_adjacent {
				block_add_to_graph(gap, self.in_adjacent_entry_.take());
				// `blocks` is non-empty when the command was constructed validly
				track_insert_block_before(&self.track, gap, &self.blocks[0]);
			}
		} else if let Some(adjacent) = &self.in_adjacent {
			if Rational::new(0, 1) - self.movement == block_length(adjacent) {
				// Movement will remove the in adjacent
				track_ripple_remove_block(&self.track, adjacent);

				if block_can_be_removed(adjacent) {
					if self.in_adjacent_remove_command_.is_none() {
						self.in_adjacent_remove_command_ =
							Some(create_block_remove_command(adjacent));
					}

					if let Some(c) = self.in_adjacent_remove_command_.as_mut() {
						c.redo_now();
					}
				}

				self.we_removed_in_adjacent_ = true;
			} else {
				// Simply resize the adjacent
				block_set_length_and_media_out(
					adjacent,
					block_length(adjacent) + self.movement,
				);
			}
		}

		// We may not have an out adjacent if the slide was at the end of the track
		if let Some(adjacent) = &self.out_adjacent {
			if self.we_created_out_adjacent_ {
				// We created the out adjacent, so we just have to insert it
				block_add_to_graph(adjacent, self.out_adjacent_entry_.take());
				track_insert_block_after(
					&self.track,
					adjacent,
					Some(self.blocks.last().expect("non-empty blocks")),
				);
			} else if self.movement == block_length(adjacent) {
				// Movement will remove the out adjacent
				track_ripple_remove_block(&self.track, adjacent);

				if block_can_be_removed(adjacent) {
					if self.out_adjacent_remove_command_.is_none() {
						self.out_adjacent_remove_command_ =
							Some(create_block_remove_command(adjacent));
					}

					if let Some(c) = self.out_adjacent_remove_command_.as_mut() {
						c.redo_now();
					}
				}

				self.we_removed_out_adjacent_ = true;
			} else {
				// Simply resize the adjacent: its in edge follows the last
				// sliding block's out, so the content end stays anchored.
				block_set_length_keeping_out(adjacent, block_length(adjacent) - self.movement);
			}
		}
	}

	/// `undo`: revert the slide (the caller restores the moved blocks
	/// alongside this, mirroring how it positioned them for the redo).
	pub fn undo(&mut self) {
		if self.we_created_in_adjacent_ {
			// We created this, so we can remove it now
			if let Some(gap) = &self.in_adjacent {
				track_ripple_remove_block(&self.track, gap);
				if self.in_adjacent_entry_.is_none() {
					self.in_adjacent_entry_ = block_remove_from_graph(gap);
				}
			}
		} else if let Some(adjacent) = &self.in_adjacent {
			if self.we_removed_in_adjacent_ {
				if let Some(c) = self.in_adjacent_remove_command_.as_mut() {
					// We removed this, so we can restore it now
					c.undo_now();
				}

				// `blocks` is non-empty when the command was constructed validly
				track_insert_block_before(&self.track, adjacent, &self.blocks[0]);
			} else {
				// Simply resize the adjacent
				block_set_length_and_media_out(
					adjacent,
					block_length(adjacent) - self.movement,
				);
			}
		}

		if let Some(adjacent) = &self.out_adjacent {
			if self.we_created_out_adjacent_ {
				// We created this, so we can remove it now
				track_ripple_remove_block(&self.track, adjacent);
				if self.out_adjacent_entry_.is_none() {
					self.out_adjacent_entry_ = block_remove_from_graph(adjacent);
				}
			} else if self.we_removed_out_adjacent_ {
				if let Some(c) = self.out_adjacent_remove_command_.as_mut() {
					// We removed this, so we can restore it now
					c.undo_now();
				}

				track_insert_block_after(
					&self.track,
					adjacent,
					Some(self.blocks.last().expect("non-empty blocks")),
				);
			} else {
				// Simply resize the adjacent (mirror of the redo).
				block_set_length_keeping_out(adjacent, block_length(adjacent) + self.movement);
			}
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackSlideCommand {
	/// `Command::redo` — the inherent method takes precedence.
	fn redo(&mut self) {
		self.redo();
	}

	/// `Command::undo` — the inherent method takes precedence.
	fn undo(&mut self) {
		self.undo();
	}
}

/// `TrackPlaceBlockCommand` — destructively place `block` at `in`, trimming or
/// removing blocks that occupy that area and inserting a gap if the block
/// extends past the end of the sequence (timelineundopointer.h).
pub struct TrackPlaceBlockCommand {
	/// Track list (`timeline_`).
	timeline: NodeRef,
	/// Target track index (`track_index_`).
	track_index: i32,
	/// Placement in point (`in_`).
	in_: Rational,
	/// Gap inserted when the block extends past the end of the sequence (`gap_`).
	gap_: Option<NodeRef>,
	/// Arena entry of `gap_` while detached from the graph.
	gap_entry_: Option<NodeEntry>,
	/// Block to place (C++ `insert_`).
	block: NodeRef,
	/// Track-add commands created when the track index is out of range
	/// (`add_track_commands_`).
	add_track_commands_: Vec<TimelineAddTrackCommand>,
	/// Ripple-removal of the area the block occupies (`ripple_remove_command_`).
	ripple_remove_command_: Option<TrackRippleRemoveAreaCommand>,
	/// The block's in point captured on the first redo (restored on undo
	/// so a sync re-place / move round-trips the original position).
	old_in_: Option<Rational>,
}

impl TrackPlaceBlockCommand {
	/// Construct from track list + track index + block + in point.
	///
	/// New signature (single-lib): `pub fn new(timeline: NodeRef, track_index: i32, block: NodeRef, in_: Rational) -> TrackPlaceBlockCommand`
	pub fn new(timeline: NodeRef, track_index: i32, block: NodeRef, in_: Rational) -> Self {
		Self {
			timeline,
			track_index,
			in_,
			gap_: None,
			gap_entry_: None,
			block,
			add_track_commands_: Vec::new(),
			ripple_remove_command_: None,
			old_in_: None,
		}
	}

	/// `redo`: place the block destructively.
	pub fn redo(&mut self) {
		// The module stores the position on the block (no track-order
		// derivation like C++), so the placement contract is that the
		// block's in point becomes `in_` — fresh clips otherwise keep
		// their birth in=0 and every drop lands at the timeline zero. The
		// original position is captured once so undo restores it (the
		// sync re-place path round-trips the block's position).
		if self.old_in_.is_none() {
			self.old_in_ = Some(block_in(&self.block));
		}
		block_set_in(&self.block, self.in_);
		// Determine if we need to add tracks
		let track_count = tracklist_track_count(&self.timeline) as i32;

		if self.track_index >= track_count {
			if self.add_track_commands_.is_empty() {
				// First redo, create the missing tracks now
				for _ in 0..(self.track_index - track_count + 1) {
					self.add_track_commands_
						.push(TimelineAddTrackCommand::new(self.timeline.clone()));
				}
			}

			for cmd in &mut self.add_track_commands_ {
				cmd.redo();
			}
		}

		let Some(track) = tracklist_track_at(&self.timeline, self.track_index as usize) else {
			return;
		};

		let in_ = self.in_;
		let append = in_ >= track_length(&track);

		// Check if the placement location is past the end of the timeline
		if append {
			if in_ > track_length(&track) {
				// If so, insert a gap filling [track end, in_): the module
				// stores the gap's span on the block, so both ends are
				// written explicitly.
				if self.gap_.is_none() {
					self.gap_ = Some(block_gap_create(&self.timeline.project));
					if let Some(gap) = &self.gap_ {
						block_set_in(gap, track_length(&track));
						block_set_length_and_media_out(gap, in_ - track_length(&track));
					}
				}
				if let Some(gap) = &self.gap_ {
					block_add_to_graph(gap, self.gap_entry_.take());
					track_append_block(&track, gap);
				}
			}

			track_append_block(&track, &self.block);
		} else {
			// Place the block at this point
			if self.ripple_remove_command_.is_none() {
				let insert_length = block_length(&self.block);
				let mut cmd = TrackRippleRemoveAreaCommand::new(
					track.clone(),
					TimeRange::new(in_, in_ + insert_length),
				);
				cmd.set_allow_splitting_gaps(true);
				self.ripple_remove_command_ = Some(cmd);
			}

			let cmd = self.ripple_remove_command_.as_mut().unwrap();
			cmd.prepare();
			cmd.redo();

			// Insert after the block that follows the cleared area
			let index = self
				.ripple_remove_command_
				.as_ref()
				.unwrap()
				.get_insertion_index();
			track_insert_block_after(&track, &self.block, index.as_ref());
		}
	}

	/// `undo`: remove the placed block and restore displaced blocks.
	pub fn undo(&mut self) {
		let Some(t) = tracklist_track_at(&self.timeline, self.track_index as usize) else {
			return;
		};

		// Firstly, remove our insert
		track_ripple_remove_block(&t, &self.block);
		// Restore the position the block had before the placement.
		if let Some(old_in) = self.old_in_ {
			block_set_in(&self.block, old_in);
		}

		if self.ripple_remove_command_.is_some() {
			// If we ripple-removed, just undo that
			self.ripple_remove_command_.as_mut().unwrap().undo();
		} else if let Some(gap) = &self.gap_ {
			track_ripple_remove_block(&t, gap);
			if self.gap_entry_.is_none() {
				self.gap_entry_ = block_remove_from_graph(gap);
			}
		}

		// Remove tracks if we added them
		for cmd in self.add_track_commands_.iter_mut().rev() {
			cmd.undo();
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackPlaceBlockCommand {
	/// `Command::redo` — the inherent method takes precedence.
	fn redo(&mut self) {
		self.redo();
	}

	/// `Command::undo` — the inherent method takes precedence.
	fn undo(&mut self) {
		self.undo();
	}
}

/// `TrackMoveBlockCommand` — move `block` within the track list so its in
/// point becomes `in_` (the capi's `oakengine_sequence_move_clip` assembly:
/// the clip's old spot is replaced with a gap, then the clip is placed at the
/// destination; both halves run as ONE undoable entry). The destination is
/// `track_index` in the list, which the caller normally resolves from the
/// block itself — passing a different track index yields a cross-track move.
///
/// Unlike Olive, the module's block in/out points are stored on the block
/// rather than derived from the track order, so the move also rewrites the
/// block's in point (keeping the length; the out follows) on redo and
/// restores it on undo.
pub struct TrackMoveBlockCommand {
	/// Composite of the gap + place halves (`children_`).
	children: MultiUndoCommand,
	/// The block being moved (`block_`).
	block: NodeRef,
	/// In point captured at construction (`old_in_`).
	old_in: Rational,
	/// Destination in point (`in_`).
	in_: Rational,
}

impl TrackMoveBlockCommand {
	/// Construct from track list + destination track index + block + in point.
	///
	/// New signature (single-lib): `pub fn new(timeline: NodeRef, track_index: i32, block: NodeRef, in_: Rational) -> TrackMoveBlockCommand`
	pub fn new(timeline: NodeRef, track_index: i32, block: NodeRef, in_: Rational) -> Self {
		let mut children = MultiUndoCommand::new();
		if let Some(track) = block_track(&block) {
			children.add_child(Box::new(TrackReplaceBlockWithGapCommand::new(
				track,
				block.clone(),
				true,
			)));
		}
		children.add_child(Box::new(TrackPlaceBlockCommand::new(
			timeline,
			track_index,
			block.clone(),
			in_,
		)));
		let old_in = block_in(&block);
		Self {
			children,
			block,
			old_in,
			in_,
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackMoveBlockCommand {
	/// `Command::redo` — the composite runs in child order.
	fn redo(&mut self) {
		// The module stores the block's position on the block itself (no
		// track-order derivation), so re-home it before placing.
		block_set_in(&self.block, self.in_);
		self.children.redo();
	}

	/// `Command::undo` — the composite runs children in reverse.
	fn undo(&mut self) {
		self.children.undo();
		block_set_in(&self.block, self.old_in);
	}
}

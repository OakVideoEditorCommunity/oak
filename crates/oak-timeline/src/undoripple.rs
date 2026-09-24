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

//! Ripple commands (`src/timeline/src/timelineundoripple.h`): clearing a time
//! area on one or all tracks, the ripple tool, and gap deletion at regions.
//!
//! Graph mutation routes directly through the oaknode Rust domain (the
//! oaknode C ABI was deleted in the single-lib unification); command
//! wrapping through `oak_undo::undocommand::UndoCommand` values. The C++
//! `std::map<..., TrackHandleLess>` containers are modelled as
//! order-preserving `Vec<(NodeRef, _)>` pairs; order follows the track
//! `NodeId`, matching `TrackHandleLess`.
//!
//! The oaknode queries a `prepare()` needs — locating blocks at a time,
//! detecting gaps, enumerating track lists / sequence tracks and reading the
//! locked flag — go through the matching `crate::util` helpers, wrapped by
//! the thin private helpers below (each annotated with its `// CPP-PARITY`
//! source line).

use oak_core::{Rational, TimeRange};
use oak_node::graph::NodeEntry;
use oak_undo::undocommand::UndoCommand;

use crate::common::MovementMode;
use crate::undocommon::{
	block_can_be_removed, box_command, create_block_remove_command, Command,
};
use crate::util::{
	block_add_to_graph, block_gap_create, block_in, block_kind, block_length, block_next,
	block_out, block_previous, block_remove_from_graph, block_set_in,
	block_set_length_and_media_in, block_set_length_and_media_out, block_set_length_keeping_out,
	block_track, sequence_all_tracks, sequence_track_list,
	track_insert_block_after, track_locked, track_nearest_block_after_or_at,
	track_nearest_block_before_or_at, track_prepend_block, track_ripple_remove_block,
	tracklist_track_at, tracklist_track_count, BlockKind, NodeRef,
};

use super::undogeneral::BlockResizeCommand;
use super::undosplit::BlockSplitCommand;
use super::undotrack::TrackRippleRemoveBlockCommand;

/// C++ unary `Rational::operator-` (`Rational(num_, -den_)`); the crate's
/// `Rational` has no `Neg`, so negate by subtracting from zero (same idiom as
/// `marker.rs`).
fn rat_neg(r: Rational) -> Rational {
	Rational::new(0, 1) - r
}

// The helpers below mirror the oaknode C ABI queries the C++ `prepare()`
// bodies use. Each wraps a `crate::util` oaknode-domain query (see the
// `// CPP-PARITY` marker on each).

/// C++ `oaknode_track_get_nearest_block_before_or_at`.
// CPP-PARITY timelineundoripple.cpp:64
fn nearest_block_before_or_at(track: &NodeRef, t: Rational) -> Option<NodeRef> {
	track_nearest_block_before_or_at(track, t)
}

/// C++ `oaknode_track_get_nearest_block_after_or_at`.
// CPP-PARITY timelineundoripple.cpp:551
fn nearest_block_after_or_at(track: &NodeRef, t: Rational) -> Option<NodeRef> {
	track_nearest_block_after_or_at(track, t)
}

/// C++ anonymous-namespace `is_gap` (`oaknode_block_get_kind`).
// CPP-PARITY timelineundoripple.cpp:448
fn is_gap(b: &NodeRef) -> bool {
	block_kind(b) == BlockKind::Gap
}

/// C++ `oaknode_tracklist_get_track_count`.
// CPP-PARITY timelineundoripple.cpp:211
fn track_list_count(list: &NodeRef) -> usize {
	tracklist_track_count(list)
}

/// C++ `oaknode_tracklist_get_track_at`.
// CPP-PARITY timelineundoripple.cpp:215
fn track_list_at(list: &NodeRef, index: usize) -> Option<NodeRef> {
	tracklist_track_at(list, index)
}

/// C++ `oaknode_track_get_locked`.
// CPP-PARITY timelineundoripple.cpp:221
fn track_is_locked(track: &NodeRef) -> bool {
	track_locked(track)
}

/// C++ `TimelineRippleRemoveAreaCommand` ctor loop over
/// `oaknode_sequence_get_track_list` (one list per track type).
// CPP-PARITY timelineundoripple.cpp:254
fn sequence_track_lists(timeline: &NodeRef) -> Vec<NodeRef> {
	let mut lists = Vec::new();
	for kind in [
		oak_node::track::TrackType::Video,
		oak_node::track::TrackType::Audio,
		oak_node::track::TrackType::Subtitle,
	] {
		if let Some(list) = sequence_track_list(timeline, kind) {
			lists.push(list);
		}
	}
	lists
}

/// `TrackRippleRemoveAreaCommand` — clear the area between `range.in` and
/// `range.out` on a single track (timelineundoripple.h). Blocks are trimmed and
/// removed; subsequent blocks push backward unless a block is inserted at the
/// in point (see `get_insertion_index`).
///
/// `track_`/`range_` are the fixed inputs; `prepare()` derives the per-block
/// trim/remove/splice operations. `redo()`/`undo()` are self-contained against
/// that derived state.
pub struct TrackRippleRemoveAreaCommand {
	/// Owning track.
	track: NodeRef,
	/// Area to clear.
	range: TimeRange,
	/// Whether `prepare` has run (the oakundo command path never calls
	/// `prepare()` itself, so `redo` derives the operations on first use).
	prepared: bool,
	/// Out-point trim on the first block (`timelineundoripple.h` `trim_out_`).
	trim_out_: Option<TrimOperation>,
	/// Blocks fully inside the range to remove (`removals_`).
	removals_: Vec<RemoveOperation>,
	/// In-point trim on the trailing block (`trim_in_`).
	trim_in_: Option<TrimOperation>,
	/// Block any replacement should be inserted after (`insert_previous_`).
	insert_previous_: Option<NodeRef>,
	/// Whether a gap spanning the range may be split (`allow_splitting_gaps_`).
	allow_splitting_gaps_: bool,
	/// Split command used when the range cuts through a block (`splice_split_command_`).
	splice_split_command_: Option<BlockSplitCommand>,
	/// Node-remove sub-commands for removed blocks (`remove_block_commands_`).
	remove_block_commands_: Vec<UndoCommand>,
}

/// One block trimmed at its out or in edge (C++ `TrimOperation`).
struct TrimOperation {
	/// The trimmed block.
	block: NodeRef,
	/// Length before the trim.
	old_length: Rational,
	/// Length after the trim.
	new_length: Rational,
}

/// One block removed and the predecessor it should be re-inserted after
/// (C++ `RemoveOperation`).
struct RemoveOperation {
	/// The removed block.
	block: NodeRef,
	/// Predecessor to re-insert after on undo (`None` = front of the track).
	before: Option<NodeRef>,
}

impl TrackRippleRemoveAreaCommand {
	/// Construct from track + range.
	///
	/// New signature (single-lib): `pub fn new(track: NodeRef, range: TimeRange) -> TrackRippleRemoveAreaCommand`
	pub fn new(track: NodeRef, range: TimeRange) -> Self {
		Self {
			track,
			range,
			prepared: false,
			trim_out_: None,
			removals_: Vec::new(),
			trim_in_: None,
			insert_previous_: None,
			allow_splitting_gaps_: false,
			splice_split_command_: None,
			remove_block_commands_: Vec::new(),
		}
	}

	/// The block that will follow the cleared area, for inserting a replacement
	/// (`get_insertion_index`); `None` when not applicable.
	///
	/// New signature (single-lib): `pub fn get_insertion_index(&self) -> Option<NodeRef>`
	pub fn get_insertion_index(&self) -> Option<NodeRef> {
		self.insert_previous_.clone()
	}

	/// The block produced by the splice split, if the range split a block
	/// (`get_spliced_block`); `None` when nothing was split.
	///
	/// New signature (single-lib): `pub fn get_spliced_block(&self) -> Option<NodeRef>`
	pub fn get_spliced_block(&self) -> Option<NodeRef> {
		match &self.splice_split_command_ {
			Some(cmd) => cmd.new_block(),
			None => None,
		}
	}

	/// Whether gaps inside the range may be split instead of removed outright
	/// (`set_allow_splitting_gaps`).
	pub fn set_allow_splitting_gaps(&mut self, e: bool) {
		self.allow_splitting_gaps_ = e;
	}

	/// `prepare`: compute the trim/remove operations for the range.
	///
	/// Mirrors the C++ algorithm (`oaknode_track_get_nearest_block_before_or_at`
	/// is a direct oaknode-domain query; `redo` invokes this on first use
	/// because the oakundo command path never calls `prepare` itself).
	pub fn prepare(&mut self) {
		// Idempotent: recompute from the current track state, discarding any
		// previously derived operations.
		self.trim_out_ = None;
		self.removals_.clear();
		self.trim_in_ = None;
		self.insert_previous_ = None;
		self.splice_split_command_ = None;

		let track = self.track.clone();
		let in_ = self.range.in_();
		let out = self.range.out();

		// CPP-PARITY timelineundoripple.cpp:57-64
		let Some(first_block) = nearest_block_before_or_at(&track, in_) else {
			return;
		};
		let fb = first_block;

		// Determine if this first block is getting trimmed or removed
		let first_block_is_out_trimmed = block_in(&fb) < in_;
		let first_block_is_in_trimmed = block_out(&fb) > out;

		// Set's the block that any insert command should insert AFTER.
		self.insert_previous_ = if first_block_is_out_trimmed {
			Some(fb.clone())
		} else {
			block_previous(&fb)
		};

		// If it's getting trimmed, determine if it's actually getting spliced
		if first_block_is_out_trimmed && first_block_is_in_trimmed {
			if !self.allow_splitting_gaps_ && is_gap(&fb) {
				// As a rule, we don't split gaps, so we just treat it as a
				// trim of the range requested
				self.trim_out_ = Some(TrimOperation {
					block: fb.clone(),
					old_length: block_length(&fb),
					new_length: block_length(&fb) - self.range.length(),
				});
			} else {
				// This block is getting spliced, so we'll handle that later
				self.splice_split_command_ = Some(BlockSplitCommand::new(fb.clone(), in_));
			}
		} else {
			// It's just getting trimmed or removed, so we'll append that operation
			if first_block_is_out_trimmed {
				self.trim_out_ = Some(TrimOperation {
					block: fb.clone(),
					old_length: block_length(&fb),
					new_length: block_length(&fb) - (block_out(&fb) - in_),
				});
			} else if first_block_is_in_trimmed {
				self.trim_in_ = Some(TrimOperation {
					block: fb.clone(),
					old_length: block_length(&fb),
					new_length: block_length(&fb) - (out - block_in(&fb)),
				});
			} else {
				// We know for sure this block is within the range so it will be removed
				self.removals_.push(RemoveOperation {
					block: fb.clone(),
					before: block_previous(&fb),
				});
			}

			// If the first block is getting in trimmed, we're already at the
			// end of our range
			if !first_block_is_in_trimmed {
				let mut next = block_next(&fb);
				while let Some(nx) = next {
					// The module world allows gaps BETWEEN blocks (their
					// in/out points are stored, unlike Olive's contiguous
					// track ordering), so a successor starting at/after the
					// region does not overlap it and must not be touched.
					if block_in(&nx) >= out {
						break;
					}
					let trimming = block_out(&nx) > out;

					if trimming {
						self.trim_in_ = Some(TrimOperation {
							block: nx.clone(),
							old_length: block_length(&nx),
							new_length: block_length(&nx) - (out - block_in(&nx)),
						});
						break;
					} else {
						self.removals_.push(RemoveOperation {
							block: nx.clone(),
							before: block_previous(&nx),
						});

						if block_out(&nx) == out {
							break;
						}
					}

					next = block_next(&nx);
				}
			}
		}

		self.prepared = true;
	}

	/// `redo`: apply the ripple removal.
	pub fn redo(&mut self) {
		// The oakundo command path never invokes `prepare()` (the oakundo
		// wrapper only dispatches redo/undo), so derive the operations
		// on first use.
		if !self.prepared {
			self.prepare();
		}
		if self.splice_split_command_.is_some() {
			// We're just splicing (C++ `redo_now` = prepare + redo)
			let cmd = self.splice_split_command_.as_mut().unwrap();
			cmd.redo();

			// Trim the in of the split: the second half produced by the
			// split keeps the original out point, so removing the first
			// `range.length()` of it keeps the timeline in fixed at the
			// range out and advances the media window past the removed
			// content (in-anchored, media in moves with the length).
			if let Some(split) = cmd.new_block() {
				let new_len =
					block_length(&split) - (self.range.out() - block_in(&split));
				block_set_length_and_media_in(&split, new_len);
			}
		} else {
			if let Some(t) = &self.trim_out_ {
				// An out-end trim keeps the in point and the media window
				// (in-anchored).
				block_set_length_and_media_out(&t.block, t.new_length);
			}

			if let Some(t) = &self.trim_in_ {
				// An in-end trim keeps the out point and advances the media
				// window past the removed head.
				block_set_length_keeping_out(&t.block, t.new_length);
			}

			// Perform removals
			if !self.removals_.is_empty() {
				for op in &self.removals_ {
					// Ripple remove them all first
					track_ripple_remove_block(&self.track, &op.block);
				}

				// Create undo commands for node removals where possible
				if self.remove_block_commands_.is_empty() {
					for op in &self.removals_ {
						if block_can_be_removed(&op.block) {
							self.remove_block_commands_
								.push(create_block_remove_command(&op.block));
						}
					}
				}

				for i in 0..self.remove_block_commands_.len() {
					self.remove_block_commands_[i].redo_now();
				}
			}
		}
	}

	/// `undo`: restore the removed/trimmed blocks.
	pub fn undo(&mut self) {
		if self.splice_split_command_.is_some() {
			let cmd = self.splice_split_command_.as_mut().unwrap();
			cmd.undo();
		} else {
			if let Some(t) = &self.trim_out_ {
				// In-anchored, matching the redo (see above).
				block_set_length_and_media_out(&t.block, t.old_length);
			}

			if let Some(t) = &self.trim_in_ {
				// Out-anchored, matching the redo (see above).
				block_set_length_keeping_out(&t.block, t.old_length);
			}

			// Un-remove any blocks
			for i in (0..self.remove_block_commands_.len()).rev() {
				self.remove_block_commands_[i].undo_now();
			}

			for op in &self.removals_ {
				track_insert_block_after(&self.track, &op.block, op.before.as_ref());
			}
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackRippleRemoveAreaCommand {
	fn redo(&mut self) {
		self.redo()
	}

	fn undo(&mut self) {
		self.undo()
	}
}

/// `TrackListRippleRemoveAreaCommand` — clear a time area across every track
/// in a list (timelineundoripple.h). Composes a `TrackRippleRemoveAreaCommand`
/// per track.
pub struct TrackListRippleRemoveAreaCommand {
	/// Track list.
	list: NodeRef,
	/// Area to clear.
	range: TimeRange,
	/// The tracks actually worked on (`working_tracks_`).
	working_tracks_: Vec<NodeRef>,
	/// Per-track child commands (`commands_`).
	commands_: Vec<TrackRippleRemoveAreaCommand>,
}

impl TrackListRippleRemoveAreaCommand {
	/// Construct from track list + in + out points.
	///
	/// New signature (single-lib): `pub fn new(list: NodeRef, in_: Rational, out: Rational) -> TrackListRippleRemoveAreaCommand`
	pub fn new(list: NodeRef, in_: Rational, out: Rational) -> Self {
		Self {
			list,
			range: TimeRange::new(in_, out),
			working_tracks_: Vec::new(),
			commands_: Vec::new(),
		}
	}

	/// `prepare`: build the per-track commands.
	pub fn prepare(&mut self) {
		self.working_tracks_.clear();
		self.commands_.clear();

		let list = self.list.clone();
		let count = track_list_count(&list);
		for i in 0..count {
			let Some(track) = track_list_at(&list, i) else {
				continue;
			};

			if track_is_locked(&track) {
				continue;
			}

			self.commands_
				.push(TrackRippleRemoveAreaCommand::new(track.clone(), self.range));
			self.working_tracks_.push(track);
		}
	}

	/// `redo`: apply every per-track command.
	pub fn redo(&mut self) {
		for cmd in &mut self.commands_ {
			// C++ `redo_now` = prepare + redo.
			cmd.prepare();
			cmd.redo();
		}
	}

	/// `undo`: revert every per-track command.
	pub fn undo(&mut self) {
		for cmd in &mut self.commands_ {
			cmd.undo();
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackListRippleRemoveAreaCommand {
	fn redo(&mut self) {
		self.redo()
	}

	fn undo(&mut self) {
		self.undo()
	}
}

/// `TimelineRippleRemoveAreaCommand` — clear a time area across the whole
/// timeline (timelineundoripple.h). A multi-command that drives a
/// `TrackListRippleRemoveAreaCommand` per track list.
#[allow(dead_code)] // `timeline`/`range` mirror the C++ members; the children carry the work.
pub struct TimelineRippleRemoveAreaCommand {
	/// Owning sequence.
	timeline: NodeRef,
	/// Area to clear.
	range: TimeRange,
	/// Per-track-list children (C++ `MultiUndoCommand` children).
	commands_: Vec<TrackListRippleRemoveAreaCommand>,
}

impl TimelineRippleRemoveAreaCommand {
	/// Construct from sequence + in + out points.
	///
	/// The ctor walks the sequence's track lists (one per track type) and
	/// creates a `TrackListRippleRemoveAreaCommand` for each.
	///
	/// New signature (single-lib): `pub fn new(timeline: NodeRef, in_: Rational, out: Rational) -> TimelineRippleRemoveAreaCommand`
	pub fn new(timeline: NodeRef, in_: Rational, out: Rational) -> Self {
		let range = TimeRange::new(in_, out);

		let mut commands_ = Vec::new();
		for list in sequence_track_lists(&timeline) {
			commands_.push(TrackListRippleRemoveAreaCommand::new(list, in_, out));
		}

		Self {
			timeline,
			range,
			commands_,
		}
	}

	/// `redo`: apply the ripple removal.
	pub fn redo(&mut self) {
		for cmd in &mut self.commands_ {
			cmd.prepare();
			cmd.redo();
		}
	}

	/// `undo`: revert the ripple removal.
	pub fn undo(&mut self) {
		for cmd in &mut self.commands_ {
			cmd.undo();
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TimelineRippleRemoveAreaCommand {
	fn redo(&mut self) {
		self.redo()
	}

	fn undo(&mut self) {
		self.undo()
	}
}

/// `TrackListRippleToolCommand::RippleInfo` — which block to ripple and
/// whether a trailing gap should be appended (timelineundoripple.h).
pub struct RippleInfo {
	/// Block to ripple.
	block: NodeRef,
	/// Whether to append a gap after the ripple.
	append_gap: bool,
}

impl RippleInfo {
	/// Creates a per-block ripple record: `block` is the block whose length
	/// the ripple changes, and `append_gap` requests inserting a gap of the
	/// movement span ahead of it instead (C++
	/// `TrackListRippleToolCommand::RippleInfo{block, append_gap}`).
	///
	/// Added for the M6 test-coverage pass: without a constructor the
	/// integration tests could only build an empty info list, leaving the
	/// whole `ripple()` body unreachable (review §M5).
	pub fn new(block: NodeRef, append_gap: bool) -> Self {
		Self { block, append_gap }
	}

	/// The block this record targets.
	pub fn block(&self) -> &NodeRef {
		&self.block
	}

	/// Whether the ripple inserts a gap instead of resizing the block.
	pub fn append_gap(&self) -> bool {
		self.append_gap
	}
}

/// `TrackListRippleToolCommand` — ripple-tool edit: shift blocks on the listed
/// tracks by `ripple_movement` (timelineundoripple.h).
#[allow(dead_code)] // `track_list` mirrors the C++ member; the per-track info drives the work.
pub struct TrackListRippleToolCommand {
	/// Track list.
	track_list: NodeRef,
	/// Per-track ripple info, ordered by track id (TrackHandleLess parity).
	info: Vec<(NodeRef, RippleInfo)>,
	/// Signed ripple movement.
	ripple_movement: Rational,
	/// Movement mode.
	movement_mode: MovementMode,
	/// Per-track working data persisting across redo/undo (`working_data_`).
	working_data_: Vec<(NodeRef, WorkingData)>,
}

impl TrackListRippleToolCommand {
	/// Construct from track list + per-track info + movement + mode.
	///
	/// New signature (single-lib): `pub fn new(track_list: NodeRef, info: Vec<(NodeRef, RippleInfo)>, ripple_movement: Rational, movement_mode: MovementMode) -> TrackListRippleToolCommand`
	pub fn new(
		track_list: NodeRef,
		info: Vec<(NodeRef, RippleInfo)>,
		ripple_movement: Rational,
		movement_mode: MovementMode,
	) -> Self {
		Self {
			track_list,
			info,
			ripple_movement,
			movement_mode,
			working_data_: Vec::new(),
		}
	}

	/// `redo`: ripple forward.
	pub fn redo(&mut self) {
		self.ripple(true);
	}

	/// `undo`: ripple backward.
	pub fn undo(&mut self) {
		self.ripple(false);
	}

	/// Apply the movement in the given direction (`TrackListRippleToolCommand::ripple`,
	/// timelineundoripple.cpp:292).
	fn ripple(&mut self, redo: bool) {
		if self.info.is_empty() {
			return;
		}

		// C++ accumulates `pre_latest_out`/`post_latest_out` (per block) into
		// `pre_latest_out`/`post_latest_out` for cache invalidation, but never
		// reads them afterwards; they are omitted here as dead code.

		for (track, info) in &self.info {
			let track_copy = track.clone();
			let b = info.block.clone();

			// Generate block length
			let mut operation_movement = self.ripple_movement;

			if self.movement_mode == MovementMode::TrimIn {
				operation_movement = rat_neg(operation_movement);
			}

			if !redo {
				operation_movement = rat_neg(operation_movement);
			}

			let new_block_length = block_length(&b) + operation_movement;

			// Fetch or default-construct this track's working data (C++ map
			// `working_data_[track]`, persisted across redo/undo).
			let wd_index = match self
				.working_data_
				.iter()
				.position(|(t, _)| t.id == track_copy.id)
			{
				Some(i) => i,
				None => {
					self.working_data_
						.push((track_copy.clone(), WorkingData::default()));
					self.working_data_.len() - 1
				}
			};
			let wd = &mut self.working_data_[wd_index].1;

			if info.append_gap {
				// Rather than rippling the referenced block, we'll insert a gap and
				// ripple with that
				if redo {
					if wd.created_gap.is_none() {
						let gap = block_gap_create(&track_copy.project);
						// The gap takes the ripple movement's span at `b`'s
						// in point; positions are stored on the block in the
						// module model, so both ends are written explicitly
						// (approximation: the C++ layout additionally shifts
						// `b` and its successors right by that span — the
						// stored-range command leaves that shift to the
						// caller).
						block_set_in(&gap, block_in(&b));
						block_set_length_and_media_out(
							&gap,
							if self.ripple_movement < Rational::new(0, 1) {
								rat_neg(self.ripple_movement)
							} else {
								self.ripple_movement
							},
						);
						wd.created_gap = Some(gap);
					}
					let gap = wd.created_gap.clone().expect("created above");

					block_add_to_graph(&gap, wd.created_gap_entry.take());

					// C++ `oaknode_track_insert_block_before(track, gap, b)`;
					// insert after `b`'s predecessor (prepend when `b` is the
					// first block).
					let before = block_previous(&b);
					match before {
						Some(prev) => {
							track_insert_block_after(&track_copy, &gap, Some(&prev))
						}
						None => track_prepend_block(&track_copy, &gap),
					}

					// As an insertion, the earliest change is at the gap's in point
					wd.earliest_point_of_change = block_in(&gap);
				} else if let Some(gap) = &wd.created_gap {
					track_ripple_remove_block(&track_copy, gap);
					if wd.created_gap_entry.is_none() {
						wd.created_gap_entry = block_remove_from_graph(gap);
					}
				}
			} else if (redo && new_block_length.is_null())
				|| (!redo && block_track(&b).is_none())
			{
				// The ripple is the length of this block. We assume that for this to
				// happen, it must have been a gap that we will now remove.
				if redo {
					// The earliest point changes will happen is at the start of this block
					wd.earliest_point_of_change = block_in(&b);

					// Remove gap from track and from graph
					wd.removed_gap = Some(b.clone());
					wd.removed_gap_after = block_previous(&b);
					track_ripple_remove_block(&track_copy, &b);
					wd.removed_gap_entry = block_remove_from_graph(&b);
				} else if let Some(gap) = &wd.removed_gap {
					// Restore gap to graph and track
					block_add_to_graph(gap, wd.removed_gap_entry.take());
					track_insert_block_after(
						&track_copy,
						gap,
						wd.removed_gap_after.as_ref(),
					);

					// The earliest point changes will happen is at the start of this block
					wd.earliest_point_of_change = block_in(gap);
				}
			} else {
				// Store old length
				wd.old_length = block_length(&b);

				if self.movement_mode == MovementMode::TrimIn {
					// The earliest point changes will occur is in point of this block
					wd.earliest_point_of_change = block_in(&b);

					// Update length
					block_set_length_and_media_in(&b, new_block_length);
				} else {
					// The earliest point changes will occur is the out point if trimming
					// out or the in point if trimming in
					wd.earliest_point_of_change = block_out(&b);

					// Update length
					block_set_length_and_media_out(&b, new_block_length);
				}
			}
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TrackListRippleToolCommand {
	fn redo(&mut self) {
		self.redo()
	}

	fn undo(&mut self) {
		self.undo()
	}
}

/// `TrackListRippleToolCommand::WorkingData` — per-track state computed during
/// a ripple edit (timelineundoripple.h).
pub struct WorkingData {
	/// Gap created by the ripple.
	created_gap: Option<NodeRef>,
	/// Arena entry of `created_gap` while detached from the graph.
	created_gap_entry: Option<NodeEntry>,
	/// Gap removed by the ripple.
	removed_gap: Option<NodeRef>,
	/// Arena entry of `removed_gap` while detached from the graph.
	removed_gap_entry: Option<NodeEntry>,
	/// Block that follows the removed gap.
	removed_gap_after: Option<NodeRef>,
	/// Track length before the edit.
	old_length: Rational,
	/// Earliest point affected on the track.
	earliest_point_of_change: Rational,
}

impl Default for WorkingData {
	/// C++ default ctor: null block handles, false flags, NULL rationals.
	fn default() -> Self {
		Self {
			created_gap: None,
			created_gap_entry: None,
			removed_gap: None,
			removed_gap_entry: None,
			removed_gap_after: None,
			old_length: Rational::NULL,
			earliest_point_of_change: Rational::NULL,
		}
	}
}

/// `TimelineRippleDeleteGapsAtRegionsCommand` — delete gaps located at the
/// given (track, range) regions (timelineundoripple.h).
pub struct TimelineRippleDeleteGapsAtRegionsCommand {
	/// Owning sequence.
	timeline: NodeRef,
	/// Regions to clear, ordered by track id.
	regions: Vec<(NodeRef, TimeRange)>,
	/// Gap-removal/resize sub-commands (`commands_`).
	commands_: Vec<UndoCommand>,
}

/// One requested (track, range) region resolved to a gap block (C++
/// `RemovalRequest`).
struct RemovalRequest {
	/// The gap block to remove or resize.
	gap: NodeRef,
	/// The region it must clear.
	range: TimeRange,
}

impl TimelineRippleDeleteGapsAtRegionsCommand {
	/// Construct from sequence + regions.
	///
	/// New signature (single-lib): `pub fn new(timeline: NodeRef, regions: Vec<(NodeRef, TimeRange)>) -> TimelineRippleDeleteGapsAtRegionsCommand`
	pub fn new(timeline: NodeRef, regions: Vec<(NodeRef, TimeRange)>) -> Self {
		Self {
			timeline,
			regions,
			commands_: Vec::new(),
		}
	}

	/// Whether `prepare` produced any work (`has_commands`).
	pub fn has_commands(&self) -> bool {
		!self.commands_.is_empty()
	}

	/// `prepare`: build the per-region gap-removal commands.
	///
	/// The gap-kind, nearest-block, sequence-track and locked-flag queries
	/// all go through the oaknode domain via `crate::util`; `redo` invokes
	/// this on first use because the oakundo command wrapper only dispatches
	/// `redo`/`undo`.
	pub fn prepare(&mut self) {
		self.commands_.clear();

		let mut max_gaps = 0usize;
		let mut requested_gaps: Vec<(NodeRef, Vec<RemovalRequest>)> = Vec::new();

		// Convert regions to gaps
		for (track, range) in &self.regions {
			let track_copy = track.clone();
			let block = nearest_block_before_or_at(&track_copy, range.in_());

			if let Some(block) = block {
				if is_gap(&block) {
					let index = requested_gaps
						.iter()
						.position(|(t, _)| t.id == track_copy.id)
						.unwrap_or_else(|| {
							requested_gaps.push((track_copy.clone(), Vec::new()));
							requested_gaps.len() - 1
						});

					let gaps_on_track = &mut requested_gaps[index].1;
					let this_req = RemovalRequest {
						gap: block.clone(),
						range: *range,
					};

					// Insertion sort
					let mut insert_at = None;
					for i in 0..gaps_on_track.len() {
						if gaps_on_track[i].range.in_() < range.in_() {
							insert_at = Some(i);
							break;
						}
					}
					match insert_at {
						Some(i) => gaps_on_track.insert(i, this_req),
						None => gaps_on_track.push(this_req),
					}

					max_gaps = max_gaps.max(gaps_on_track.len());
				}
			}
			// C++ prints "Failed to find corresponding gap to region" to
			// stderr here; we simply skip the region.
		}

		// For each gap on each track, find a corresponding gap on every other
		// track (which may include a requested gap) to ripple in order to keep
		// everything synchronized
		let mut gap_lengths: Vec<(NodeRef, Rational)> = Vec::new();
		for gap_index in 0..max_gaps {
			let mut earliest_point = Rational::new(2147483647, 1); // RATIONAL_MAX
			let mut ripple_length = Rational::new(2147483647, 1); // RATIONAL_MAX
			let mut latest_point = Rational::new(-2147483647, 1); // RATIONAL_MIN

			for (_track, gaps_on_track) in &requested_gaps {
				if gap_index < gaps_on_track.len() {
					let gap = &gaps_on_track[gap_index];
					earliest_point = std::cmp::min(earliest_point, gap.range.in_());
					ripple_length = std::cmp::min(ripple_length, gap.range.length());
					latest_point = std::cmp::max(latest_point, gap.range.out());
				}
			}

			// Determine which gaps will be involved in this operation
			let mut gaps: Vec<NodeRef> = Vec::new();

			for track in sequence_all_tracks(&self.timeline) {
				if track_is_locked(&track) {
					continue;
				}

				let mut gap: Option<NodeRef> = None;
				let requested_here = requested_gaps.iter().find(|(t, _)| t.id == track.id);
				if let Some((_, requested_on_track)) = requested_here {
					if gap_index < requested_on_track.len() {
						gap = Some(requested_on_track[gap_index].gap.clone());
					}
				}

				if gap.is_none() {
					// No requested gap was at this index, find one
					let block = nearest_block_after_or_at(&track, earliest_point);
					if let Some(block) = block {
						// Found a block, test if it's a gap
						if is_gap(&block) {
							gap = Some(block.clone());
						} else {
							if block_in(&block) == earliest_point {
								let next = block_next(&block);
								if let Some(next) = next {
									if is_gap(&next) {
										gap = Some(next.clone());
									} else {
										ripple_length = Rational::new(0, 1);
									}
								} else {
									ripple_length = Rational::new(0, 1);
								}
							} else {
								let prev = block_previous(&block);
								if let Some(prev) = prev {
									if is_gap(&prev) {
										gap = Some(prev.clone());
									} else {
										ripple_length = Rational::new(0, 1);
									}
								} else {
									ripple_length = Rational::new(0, 1);
								}
							}
						}
					}
					// Else: assume track finishes here and track won't be
					// affected by this operation
				}

				if let Some(g) = gap {
					let g = g.clone();
					gaps.push(g.clone());

					if !gap_lengths.iter().any(|(b, _)| b.id == g.id) {
						gap_lengths.push((g.clone(), block_length(&g)));
					}

					if let Some((_, len)) = gap_lengths.iter().find(|(b, _)| b.id == g.id) {
						ripple_length = std::cmp::min(ripple_length, *len);
					}
				}

				if ripple_length == Rational::new(0, 1) {
					break;
				}
			}

			if ripple_length > Rational::new(0, 1) {
				for g in gaps {
					let g_len = gap_lengths
						.iter()
						.find(|(b, _)| b.id == g.id)
						.map(|(_, l)| *l)
						.unwrap_or(Rational::NULL);

					if g_len == ripple_length {
						if let Some(track) = block_track(&g) {
							self.commands_.push(
								TrackRippleRemoveBlockCommand::new(track, g.clone())
									.to_command(),
							);
						}
					} else {
						let mut new_len = g_len;
						for (b, l) in gap_lengths.iter_mut() {
							if b.id == g.id {
								*l = *l - ripple_length;
								new_len = *l;
							}
						}
						self.commands_
							.push(BlockResizeCommand::new(g.clone(), new_len).to_command());
					}
				}
			}
		}
	}

	/// `redo`: apply the gap deletions.
	///
	/// The oakundo command path never invokes `prepare()` (the oakundo
	/// wrapper only dispatches `redo`/`undo`), so the sub-commands are built
	/// on first use; later redos re-apply the stored commands.
	pub fn redo(&mut self) {
		if self.commands_.is_empty() {
			self.prepare();
		}
		for i in 0..self.commands_.len() {
			self.commands_[i].redo_now();
		}
	}

	/// `undo`: restore the deleted gaps.
	pub fn undo(&mut self) {
		for i in (0..self.commands_.len()).rev() {
			self.commands_[i].undo_now();
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		box_command(self)
	}
}

impl Command for TimelineRippleDeleteGapsAtRegionsCommand {
	fn redo(&mut self) {
		self.redo()
	}

	fn undo(&mut self) {
		self.undo()
	}
}

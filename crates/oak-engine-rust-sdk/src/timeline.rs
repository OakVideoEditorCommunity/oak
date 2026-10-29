//! Safe wrapper over the engine's timeline edit C ABI
//! (`oak_timeline_*`): the undoable timeline edit commands.
//!
//! Every call is one undoable edit step on the global undo stack
//! (group several with [`crate::node::undo_group_begin`]/`end`). These
//! are the `edit.*` primitives a functional plugin builds on (split,
//! ripple delete, trim, move, enable/disable, track add/remove).

use std::ffi::c_void;

use crate::node::{NodeId, Project};
use crate::types::Rational;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_timeline_block_split(project: *const c_void, block: u64, point: Rational) -> bool;
	fn oak_timeline_split_preserving_links(
		project: *const c_void,
		blocks: *const u64,
		count: usize,
		point: Rational,
	) -> bool;
	fn oak_timeline_track_split_at(project: *const c_void, track: u64, point: Rational) -> bool;
	fn oak_timeline_ripple_remove_area(
		project: *const c_void,
		timeline: u64,
		in_: Rational,
		out: Rational,
	) -> bool;
	fn oak_timeline_block_resize(project: *const c_void, block: u64, new_length: Rational) -> bool;
	fn oak_timeline_block_resize_media_in(
		project: *const c_void,
		block: u64,
		new_length: Rational,
	) -> bool;
	fn oak_timeline_block_set_media_in(
		project: *const c_void,
		block: u64,
		media_in: Rational,
	) -> bool;
	fn oak_timeline_block_set_enabled(project: *const c_void, block: u64, enabled: bool) -> bool;
	fn oak_timeline_block_trim(
		project: *const c_void,
		track: u64,
		block: u64,
		new_length: Rational,
		mode: i32,
	) -> bool;
	fn oak_timeline_block_place(
		project: *const c_void,
		timeline: u64,
		track_index: i32,
		block: u64,
		in_: Rational,
	) -> bool;
	fn oak_timeline_block_move(
		project: *const c_void,
		timeline: u64,
		track_index: i32,
		block: u64,
		in_: Rational,
	) -> bool;
	fn oak_timeline_add_track(project: *const c_void, track_list: u64) -> u64;
	fn oak_timeline_remove_track(project: *const c_void, track: u64) -> bool;
	fn oak_timeline_transition_set_offsets(
		project: *const c_void,
		block: u64,
		in_offset: Rational,
		out_offset: Rational,
	) -> bool;
}

/// Splits a block at `point` (seconds). The second half becomes a new
/// block (re-query the track for it).
pub fn block_split(project: &Project, block: NodeId, point: Rational) -> bool {
	unsafe { oak_timeline_block_split(project.as_raw(), block.raw(), point) }
}

/// Splits every block at `point` as ONE undoable command (originally
/// linked pairs keep their rear halves linked).
pub fn split_preserving_links(project: &Project, blocks: &[NodeId], point: Rational) -> bool {
	let ids: Vec<u64> = blocks.iter().map(|b| b.raw()).collect();
	unsafe { oak_timeline_split_preserving_links(project.as_raw(), ids.as_ptr(), ids.len(), point) }
}

/// Splits every block of a track at `point`.
pub fn track_split_at(project: &Project, track: NodeId, point: Rational) -> bool {
	unsafe { oak_timeline_track_split_at(project.as_raw(), track.raw(), point) }
}

/// Ripple-removes `[in_, out)` from every track of a timeline (the
/// sequence node), closing the gap.
pub fn ripple_remove_area(
	project: &Project,
	timeline: NodeId,
	in_: Rational,
	out: Rational,
) -> bool {
	unsafe { oak_timeline_ripple_remove_area(project.as_raw(), timeline.raw(), in_, out) }
}

/// Resizes a block (out point moves, in anchored).
pub fn block_resize(project: &Project, block: NodeId, new_length: Rational) -> bool {
	unsafe { oak_timeline_block_resize(project.as_raw(), block.raw(), new_length) }
}

/// Resizes a block keeping the out point (the media in point shifts).
pub fn block_resize_media_in(project: &Project, block: NodeId, new_length: Rational) -> bool {
	unsafe { oak_timeline_block_resize_media_in(project.as_raw(), block.raw(), new_length) }
}

/// Sets a clip's media in point.
pub fn block_set_media_in(project: &Project, block: NodeId, media_in: Rational) -> bool {
	unsafe { oak_timeline_block_set_media_in(project.as_raw(), block.raw(), media_in) }
}

/// Enables/disables a block.
pub fn block_set_enabled(project: &Project, block: NodeId, enabled: bool) -> bool {
	unsafe { oak_timeline_block_set_enabled(project.as_raw(), block.raw(), enabled) }
}

/// How a trim moves the block/neighbours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimMode {
	/// No trim.
	None,
	/// A move edit.
	Move,
	/// Trim the in point.
	TrimIn,
	/// Trim the out point.
	TrimOut,
}

/// Trims a block to `new_length` with trim semantics.
pub fn block_trim(
	project: &Project,
	track: NodeId,
	block: NodeId,
	new_length: Rational,
	mode: TrimMode,
) -> bool {
	let mode = match mode {
		TrimMode::None => 0,
		TrimMode::Move => 1,
		TrimMode::TrimIn => 2,
		TrimMode::TrimOut => 3,
	};
	unsafe { oak_timeline_block_trim(project.as_raw(), track.raw(), block.raw(), new_length, mode) }
}

/// Places a block onto track `track_index` of `timeline` at `in_`
/// (moving across tracks / inserting at a point).
pub fn block_place(
	project: &Project,
	timeline: NodeId,
	track_index: i32,
	block: NodeId,
	in_: Rational,
) -> bool {
	unsafe {
		oak_timeline_block_place(
			project.as_raw(),
			timeline.raw(),
			track_index,
			block.raw(),
			in_,
		)
	}
}

/// Moves a block to track `track_index` of `timeline` at `in_` (the
/// source slot becomes a gap).
pub fn block_move(
	project: &Project,
	timeline: NodeId,
	track_index: i32,
	block: NodeId,
	in_: Rational,
) -> bool {
	unsafe {
		oak_timeline_block_move(
			project.as_raw(),
			timeline.raw(),
			track_index,
			block.raw(),
			in_,
		)
	}
}

/// Adds a track to a track list (a sequence's video/audio/subtitle
/// list node — see [`Project::sequence_track_list_at`]). Returns the
/// new track.
pub fn add_track(project: &Project, track_list: NodeId) -> Option<NodeId> {
	let id = unsafe { oak_timeline_add_track(project.as_raw(), track_list.raw()) };
	if id == 0 {
		None
	} else {
		Some(NodeId::from_raw(id))
	}
}

/// Removes a track (its blocks go with it).
pub fn remove_track(project: &Project, track: NodeId) -> bool {
	unsafe { oak_timeline_remove_track(project.as_raw(), track.raw()) }
}

/// Sets a transition's in/out offsets.
pub fn transition_set_offsets(
	project: &Project,
	block: NodeId,
	in_offset: Rational,
	out_offset: Rational,
) -> bool {
	unsafe {
		oak_timeline_transition_set_offsets(project.as_raw(), block.raw(), in_offset, out_offset)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Split + undo unwind through the C ABI (mirrors the engine-side
	/// test).
	#[test]
	fn split_is_one_undoable_step() {
		// Serialize: the undo stack is process-global.
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());

		let project = Project::new();
		let sequence = project.create_sequence("Seq").expect("sequence");
		let list = project.sequence_track_list_at(sequence, 0);
		let track = project.tracklist_track_at(list, 0);
		let clip = project.create_clip().expect("clip");
		assert!(project.block_set_length(clip, Rational::new(2, 1)));
		assert!(project.track_append_block(track, clip));

		let base = crate::node::undo_count();
		let before = project.track_block_count(track);
		assert!(block_split(&project, clip, Rational::new(1, 2)));
		assert_eq!(crate::node::undo_count(), base + 1);
		assert_eq!(project.track_block_count(track), before + 1);

		assert!(crate::node::undo());
		assert_eq!(project.track_block_count(track), before);
		assert!(crate::node::redo());
		assert_eq!(project.track_block_count(track), before + 1);

		assert!(!block_split(&project, NodeId::INVALID, Rational::new(1, 2)));
		assert!(crate::node::undo_clear());
	}

	/// Track add/remove.
	#[test]
	fn add_and_remove_track() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());

		let project = Project::new();
		let sequence = project.create_sequence("Seq").expect("sequence");
		let list = project.sequence_track_list_at(sequence, 0);
		let before = project.tracklist_track_count(list);

		let track = add_track(&project, list).expect("track");
		assert_eq!(project.tracklist_track_count(list), before + 1);
		assert!(remove_track(&project, track));
		assert_eq!(project.tracklist_track_count(list), before);
		assert!(!remove_track(&project, NodeId::INVALID));
		assert!(crate::node::undo_clear());
	}
}

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

//! Contract tests for the undo commands left untested by `domain_test`:
//! the remaining ripple, pointer, track, split and general commands.
//!
//! Each test builds a project with a sequence/track-list/track, applies a
//! command through `redo`, checks the observable graph state and `undo`s
//! back to the original state. Commands whose `redo` self-prepares are
//! also exercised through the oakundo boxing path (`to_command` +
//! `redo_now`/`undo_now`); commands that need `prepare` are prepared
//! before boxing (the boxed `Command` trait has no prepare callback).

use std::sync::{Arc, Mutex};

use oak_core::{Rational, TimeRange};
use oak_node::block::{ClipBlockBehavior, GapBlockBehavior};
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior};
use oak_timeline::common::MovementMode;
use oak_timeline::undocommon::Command;
use oak_timeline::undogeneral::{
	BlockEnableDisableCommand, BlockResizeWithMediaInCommand, BlockSetMediaInCommand,
	TimelineAddDefaultTransitionCommand, TimelineAddTrackCommand, TrackListInsertGaps,
};
use oak_timeline::undopointer::{BlockTrimCommand, TrackSlideCommand};
use oak_timeline::undoripple::{
	RippleInfo, TimelineRippleDeleteGapsAtRegionsCommand, TimelineRippleRemoveAreaCommand,
	TrackListRippleRemoveAreaCommand, TrackListRippleToolCommand, TrackRippleRemoveAreaCommand,
};
use oak_timeline::undosplit::{
	BlockSplitCommand, BlockSplitPreservingLinksCommand, TrackSplitAtTimeCommand,
};
use oak_timeline::undotrack::{
	TrackInsertBlockAfterCommand, TrackPrependBlockCommand, TrackReplaceBlockCommand,
};
use oak_timeline::util::{
	block_enabled, block_kind, block_remove_from_graph, block_track, clip_media_in,
	track_append_block, track_block_at, track_block_count, track_length, BlockKind, NodeRef,
};

/// A project with one sequence owning one (video) track list.
fn make_project() -> Arc<Mutex<Project>> {
	let project = Project::new();
	let (seq_id, list_id) = {
		let mut p = project.lock().unwrap();
		let (core, behavior) = SequenceBehavior::create();
		let seq_id = p.graph.add_node(core, behavior);
		let (core, behavior) = TrackListBehavior::create();
		let list_id = p.graph.add_node(core, behavior);
		if let Some(e) = p.graph.get_mut(seq_id) {
			if let Some(a) = e.behavior.as_any_mut() {
				if let Some(s) = a.downcast_mut::<SequenceBehavior>() {
					s.track_lists.push(list_id);
				}
			}
		}
		if let Some(e) = p.graph.get_mut(list_id) {
			if let Some(a) = e.behavior.as_any_mut() {
				if let Some(l) = a.downcast_mut::<TrackListBehavior>() {
					l.sequence = Some(seq_id);
				}
			}
		}
		(seq_id, list_id)
	};
	let _ = (seq_id, list_id);
	project
}

/// The sequence and its video track list of `project`.
fn sequence_and_list(project: &Arc<Mutex<Project>>) -> (NodeRef, NodeRef) {
	let p = project.lock().unwrap();
	let mut seq = None;
	let mut list = None;
	for id in p.graph.node_ids() {
		if let Some(e) = p.graph.get(id) {
			if e.behavior
				.as_any()
				.and_then(|a| a.downcast_ref::<SequenceBehavior>())
				.is_some()
			{
				seq = Some(NodeRef::new(project.clone(), id));
			} else if e
				.behavior
				.as_any()
				.and_then(|a| a.downcast_ref::<TrackListBehavior>())
				.is_some()
			{
				list = Some(NodeRef::new(project.clone(), id));
			}
		}
	}
	(seq.unwrap(), list.unwrap())
}

/// Create a clip block spanning `[in, out)` that is not on any track.
fn new_clip(project: &Arc<Mutex<Project>>, in_: Rational, out: Rational) -> NodeRef {
	let clip = oak_timeline::util::block_clip_create(project);
	{
		let mut p = project.lock().unwrap();
		if let Some(e) = p.graph.get_mut(clip.id) {
			if let Some(a) = e.behavior.as_any_mut() {
				if let Some(c) = a.downcast_mut::<ClipBlockBehavior>() {
					c.core.range = TimeRange::new(in_, out);
				}
			}
		}
	}
	clip
}

/// Create a gap block spanning `[in, out)` that is not on any track.
fn new_gap(project: &Arc<Mutex<Project>>, in_: Rational, out: Rational) -> NodeRef {
	let gap = oak_timeline::util::block_gap_create(project);
	{
		let mut p = project.lock().unwrap();
		if let Some(e) = p.graph.get_mut(gap.id) {
			if let Some(a) = e.behavior.as_any_mut() {
				if let Some(g) = a.downcast_mut::<GapBlockBehavior>() {
					g.core.range = TimeRange::new(in_, out);
				}
			}
		}
	}
	gap
}

/// Append a clip block spanning `[in, out)` to `track`.
fn add_clip(track: &NodeRef, in_: Rational, out: Rational) -> NodeRef {
	let clip = new_clip(&track.project, in_, out);
	track_append_block(track, &clip);
	clip
}

/// Append a gap block spanning `[in, out)` to `track`.
fn add_gap(track: &NodeRef, in_: Rational, out: Rational) -> NodeRef {
	let gap = new_gap(&track.project, in_, out);
	track_append_block(track, &gap);
	gap
}

/// The block span of `block` (`None` for a stale node).
fn span_of(block: &NodeRef) -> Option<(Rational, Rational)> {
	let p = block.project.lock().unwrap();
	p.graph.get(block.id).and_then(|e| e.behavior.as_any()).and_then(|a| {
		[
			a.downcast_ref::<ClipBlockBehavior>().map(|b| &b.core),
			a.downcast_ref::<GapBlockBehavior>().map(|b| &b.core),
		]
		.into_iter()
		.flatten()
		.next()
		.map(|core| (core.in_(), core.out()))
	})
}

/// Set a track's locked flag (the flag `prepare` consults).
fn set_track_locked(track: &NodeRef, locked: bool) {
	let mut p = track.project.lock().unwrap();
	if let Some(a) = p.graph.get_mut(track.id).and_then(|e| e.behavior.as_any_mut()) {
		if let Some(t) = a.downcast_mut::<TrackBehavior>() {
			t.locked = locked;
		}
	}
}

/// Whether `node` is currently attached to the project graph.
fn node_in_graph(project: &Arc<Mutex<Project>>, node: &NodeRef) -> bool {
	let p = project.lock().unwrap();
	p.graph.get(node.id).is_some()
}

/// Link two blocks (C++ `Node::link`).
fn link_blocks(project: &Arc<Mutex<Project>>, a: &NodeRef, b: &NodeRef) {
	let mut p = project.lock().unwrap();
	let _ = p.graph.link(a.id, b.id);
}

/// Whether two blocks are linked (C++ `Node::are_linked`).
fn blocks_are_linked(project: &Arc<Mutex<Project>>, a: &NodeRef, b: &NodeRef) -> bool {
	let p = project.lock().unwrap();
	p.graph.are_linked(a.id, b.id)
}

// ---------------------------------------------------------------------------
// undoripple.rs
// ---------------------------------------------------------------------------

/// `TrackRippleRemoveAreaCommand` splices a block that the range cuts
/// through: redo splits it at the range in, trims the remainder to the
/// range out, and undo re-joins the halves.
#[test]
fn ripple_remove_area_splices_cutting_block() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TrackRippleRemoveAreaCommand::new(
		track.clone(),
		TimeRange::new(Rational::new(25, 1), Rational::new(50, 1)),
	);
	cmd.prepare();
	// The range cuts through the block, so the host is the insertion point
	// and no second half exists yet.
	assert_eq!(cmd.get_insertion_index().map(|b| b.id), Some(clip.id));
	assert!(cmd.get_spliced_block().is_none());

	cmd.redo();
	let spliced = cmd.get_spliced_block().expect("range split the block");
	assert_ne!(spliced.id, clip.id);
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 0).unwrap().id, clip.id);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(span_of(&spliced), Some((Rational::new(25, 1), Rational::new(75, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));

	// The next redo re-attaches the same second half identity.
	cmd.redo();
	assert_eq!(cmd.get_spliced_block().map(|b| b.id), Some(spliced.id));
	assert_eq!(track_block_count(&track), 2);
}

/// Gaps are not split by default (`set_allow_splitting_gaps`): the range
/// trims the gap as a whole. With the flag set, the splice path splits it.
#[test]
fn ripple_remove_area_gap_splitting_flag() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let gap = add_gap(&track, Rational::new(0, 1), Rational::new(100, 1));
	let range = TimeRange::new(Rational::new(25, 1), Rational::new(50, 1));

	// Default: the whole range shortens the gap (100 - 25 = 75), no split.
	let mut cmd = TrackRippleRemoveAreaCommand::new(track.clone(), range);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 1);
	assert!(cmd.get_spliced_block().is_none());
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(75, 1))));
	cmd.undo();
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(100, 1))));

	// Allowed: the gap is split and the remainders are trimmed.
	let mut cmd = TrackRippleRemoveAreaCommand::new(track.clone(), range);
	cmd.set_allow_splitting_gaps(true);
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	let spliced = cmd.get_spliced_block().expect("gap split allowed");
	assert_eq!(block_kind(&spliced), BlockKind::Gap);
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(span_of(&spliced), Some((Rational::new(25, 1), Rational::new(75, 1))));
	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TrackRippleRemoveAreaCommand`: undo before any redo leaves the graph
/// untouched (the trim operations are only applied by redo).
#[test]
fn ripple_remove_area_undo_before_redo_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let c1 = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let c2 = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));

	let mut cmd = TrackRippleRemoveAreaCommand::new(
		track.clone(),
		TimeRange::new(Rational::new(25, 1), Rational::new(75, 1)),
	);
	cmd.prepare();
	cmd.undo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(span_of(&c1), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&c2), Some((Rational::new(50, 1), Rational::new(100, 1))));

	cmd.redo();
	assert_eq!(span_of(&c1), Some((Rational::new(0, 1), Rational::new(25, 1))));
	// c2's in-end trim is out-anchored: its out stays at 100.
	assert_eq!(span_of(&c2), Some((Rational::new(75, 1), Rational::new(100, 1))));

	cmd.undo();
	assert_eq!(span_of(&c1), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&c2), Some((Rational::new(50, 1), Rational::new(100, 1))));
}

/// `TrackRippleRemoveAreaCommand` with no block at/before the range (an
/// empty track or a range before the first block) is a no-op.
#[test]
fn ripple_remove_area_no_candidate_block_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());

	let mut cmd = TrackRippleRemoveAreaCommand::new(
		track.clone(),
		TimeRange::new(Rational::new(0, 1), Rational::new(50, 1)),
	);
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 0);

	let clip = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));
	let mut cmd = TrackRippleRemoveAreaCommand::new(
		track.clone(),
		TimeRange::new(Rational::new(0, 1), Rational::new(25, 1)),
	);
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(50, 1), Rational::new(100, 1))));
}

/// `TrackRippleRemoveAreaCommand` against a stale track reference (its
/// node detached from the graph) is a no-op.
#[test]
fn ripple_remove_area_stale_track_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	block_remove_from_graph(&track);
	assert!(!node_in_graph(&project, &track));

	let mut cmd = TrackRippleRemoveAreaCommand::new(
		track.clone(),
		TimeRange::new(Rational::new(25, 1), Rational::new(50, 1)),
	);
	cmd.redo();
	cmd.undo();
	assert!(!node_in_graph(&project, &track));
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TrackListRippleRemoveAreaCommand` applies the range to every unlocked
/// track in the list and skips locked ones.
#[test]
fn tracklist_ripple_remove_area_skips_locked_track() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let unlocked = TimelineAddTrackCommand::run_immediately(list.clone());
	let locked = TimelineAddTrackCommand::run_immediately(list.clone());
	let unlocked_clip = add_clip(&unlocked, Rational::new(0, 1), Rational::new(100, 1));
	let locked_clip = add_clip(&locked, Rational::new(0, 1), Rational::new(100, 1));
	set_track_locked(&locked, true);

	let mut cmd = TrackListRippleRemoveAreaCommand::new(
		list.clone(),
		Rational::new(25, 1),
		Rational::new(75, 1),
	);
	cmd.prepare();
	cmd.redo();
	// Unlocked track spliced into [0,25) + [25,50); locked track untouched.
	assert_eq!(track_block_count(&unlocked), 2);
	assert_eq!(span_of(&unlocked_clip), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(track_block_count(&locked), 1);
	assert_eq!(span_of(&locked_clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert_eq!(track_length(&locked), Rational::new(100, 1));

	cmd.undo();
	assert_eq!(track_block_count(&unlocked), 1);
	assert_eq!(span_of(&unlocked_clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TimelineRippleRemoveAreaCommand` walks the sequence's track lists; the
/// boxing path (`to_command`) dispatches through the trait.
#[test]
fn timeline_ripple_remove_area_round_trip() {
	let project = make_project();
	let (seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TimelineRippleRemoveAreaCommand::new(
		seq.clone(),
		Rational::new(25, 1),
		Rational::new(75, 1),
	)
	.to_command();
	cmd.redo_now();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(25, 1))));
	cmd.undo_now();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));

	// A range covering the whole block removes it (graph node included) and
	// undo re-attaches it.
	let mut cmd = TimelineRippleRemoveAreaCommand::new(
		seq,
		Rational::new(0, 1),
		Rational::new(100, 1),
	)
	.to_command();
	cmd.redo_now();
	assert_eq!(track_block_count(&track), 0);
	assert!(!node_in_graph(&project, &clip));
	cmd.undo_now();
	assert_eq!(track_block_count(&track), 1);
	assert!(node_in_graph(&project, &clip));
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TimelineRippleDeleteGapsAtRegionsCommand` removes a gap covering the
/// requested region; undo restores it at its old slot.
#[test]
fn timeline_ripple_delete_gaps_at_regions_round_trip() {
	let project = make_project();
	let (seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let first = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(100, 1));
	let second = add_clip(&track, Rational::new(100, 1), Rational::new(150, 1));

	let mut cmd = TimelineRippleDeleteGapsAtRegionsCommand::new(
		seq,
		vec![(
			track.clone(),
			TimeRange::new(Rational::new(50, 1), Rational::new(100, 1)),
		)],
	);
	assert!(!cmd.has_commands());
	cmd.prepare();
	assert!(cmd.has_commands());

	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 0).unwrap().id, first.id);
	assert_eq!(track_block_at(&track, 1).unwrap().id, second.id);
	assert!(block_track(&gap).is_none());

	cmd.undo();
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(track_block_at(&track, 1).unwrap().id, gap.id);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(100, 1))));
}

/// A requested region that lies inside a gap longer than the region
/// resizes the gap by the region length rather than removing it.
#[test]
fn timeline_ripple_delete_gaps_resizes_longer_gap() {
	let project = make_project();
	let (seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(150, 1));
	add_clip(&track, Rational::new(150, 1), Rational::new(200, 1));

	let mut cmd = TimelineRippleDeleteGapsAtRegionsCommand::new(
		seq,
		vec![(
			track.clone(),
			TimeRange::new(Rational::new(60, 1), Rational::new(80, 1)),
		)],
	);
	cmd.prepare();
	assert!(cmd.has_commands());

	cmd.redo();
	// The gap keeps its in point and loses the 20-frame region length. In
	// the C++ the following clip also ripples left by the removed length
	// (the layout derives it); the stored-range command trims the gap only,
	// so callers that need the tail shifted do it explicitly.
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(130, 1))));

	cmd.undo();
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(150, 1))));
}

/// A region whose nearest block is not a gap, and an empty region list,
/// both produce no commands and leave the track untouched.
#[test]
fn timeline_ripple_delete_gaps_non_gap_region_is_noop() {
	let project = make_project();
	let (seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
	let gap = add_gap(&track, Rational::new(100, 1), Rational::new(150, 1));

	let mut cmd = TimelineRippleDeleteGapsAtRegionsCommand::new(
		seq.clone(),
		vec![(
			track.clone(),
			TimeRange::new(Rational::new(0, 1), Rational::new(50, 1)),
		)],
	);
	cmd.prepare();
	assert!(!cmd.has_commands());
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 2);

	let mut empty =
		TimelineRippleDeleteGapsAtRegionsCommand::new(seq, Vec::new());
	empty.prepare();
	assert!(!empty.has_commands());
	empty.redo();
	empty.undo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert_eq!(span_of(&gap), Some((Rational::new(100, 1), Rational::new(150, 1))));
}

/// `TrackListRippleToolCommand` with no per-track info is a no-op in both
/// directions (the `ripple()` early return). Real-info behavior is covered
/// by `track_list_ripple_tool_*` below, which `RippleInfo::new` made
/// constructible (review §M5).
#[test]
fn track_list_ripple_tool_empty_info_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TrackListRippleToolCommand::new(
		list.clone(),
		Vec::new(),
		Rational::new(10, 1),
		MovementMode::TrimOut,
	);
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));

	let mut boxed = TrackListRippleToolCommand::new(
		list,
		Vec::new(),
		Rational::new(0, 1) - Rational::new(10, 1),
		MovementMode::TrimIn,
	)
	.to_command();
	boxed.redo_now();
	boxed.undo_now();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TrackListRippleToolCommand` with real per-track info resizes the block
/// and restores it on undo. Trim-out is in-anchored: the clip keeps its in
/// point and its out follows the movement, while the media window (which
/// the ripple does not consume here) stays put.
#[test]
fn track_list_ripple_tool_resizes_with_real_info() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let info = RippleInfo::new(clip.clone(), false);
	assert!(!info.append_gap());
	assert_eq!(info.block().id, clip.id);
	let mut cmd = TrackListRippleToolCommand::new(
		list.clone(),
		vec![(track.clone(), info)],
		Rational::new(10, 1),
		MovementMode::TrimOut,
	);
	cmd.redo();
	assert_eq!(
		span_of(&clip),
		Some((Rational::new(0, 1), Rational::new(110, 1))),
		"the in-point stays and the out-point follows the movement"
	);
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));

	cmd.undo();
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));
}

/// `TrackListRippleToolCommand` with `append_gap = true` inserts a gap of
/// the movement span ahead of the block and removes it on undo.
#[test]
fn track_list_ripple_tool_append_gap_creates_a_gap() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TrackListRippleToolCommand::new(
		list.clone(),
		vec![(track.clone(), RippleInfo::new(clip.clone(), true))],
		Rational::new(10, 1),
		MovementMode::TrimOut,
	);
	cmd.redo();
	assert_eq!(track_block_count(&track), 2, "a gap was inserted");
	// The gap takes the block's in point; the stored-range command leaves
	// the block in place (the C++ layout would push it right by the gap
	// length — the caller owns that shift, see the insert site's note).
	let gap = track_block_at(&track, 0).expect("gap at the front");
	assert_eq!(block_kind(&gap), BlockKind::Gap);
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(10, 1))));
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 1, "the inserted gap is removed");
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

// ---------------------------------------------------------------------------
// undopointer.rs
// ---------------------------------------------------------------------------

/// `BlockTrimCommand` shortens a clip and lengthens its adjacent gap so
/// the rest of the track keeps its position; undo restores both.
///
/// Trim-out is in-anchored (`common.rs` `k_trim_out`: trim the out point):
/// the clip keeps its in and the following gap grows leftward to meet the
/// new out, so everything after the gap stays put.
#[test]
fn block_trim_with_gap_adjacent_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(100, 1));

	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		clip.clone(),
		Rational::new(25, 1),
		MovementMode::TrimOut,
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(span_of(&gap), Some((Rational::new(25, 1), Rational::new(100, 1))));
	assert_eq!(
		clip_media_in(&clip),
		Rational::new(0, 1),
		"a trim-out does not move the media window"
	);

	cmd.undo();
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(100, 1))));
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));
}

/// `BlockTrimCommand` inserts a gap filling the space the trim freed when
/// the adjacent block is a clip and the trim is not a roll edit.
#[test]
fn block_trim_creates_gap_next_to_clip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let first = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let second = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));

	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		first.clone(),
		Rational::new(25, 1),
		MovementMode::TrimOut,
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(track_block_at(&track, 0).unwrap().id, first.id);
	let created = track_block_at(&track, 1).unwrap();
	assert_eq!(block_kind(&created), BlockKind::Gap);
	// The trim keeps `first`'s in fixed and moves its out to 25; the fresh
	// gap fills the freed [25,50) space so `second` does not move.
	assert_eq!(span_of(&first), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(span_of(&created), Some((Rational::new(25, 1), Rational::new(50, 1))));
	assert_eq!(track_block_at(&track, 2).unwrap().id, second.id);

	cmd.undo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(span_of(&first), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&second), Some((Rational::new(50, 1), Rational::new(100, 1))));
}

/// `BlockTrimCommand::set_trim_is_a_roll_edit` trims into the adjacent
/// clip instead of creating a compensating gap: the shared seam moves, the
/// left clip keeps its in, the follower keeps its out.
#[test]
fn block_trim_roll_edit_resizes_adjacent_clip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let first = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let second = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));

	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		first.clone(),
		Rational::new(25, 1),
		MovementMode::TrimOut,
	);
	cmd.set_trim_is_a_roll_edit(true);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(span_of(&first), Some((Rational::new(0, 1), Rational::new(25, 1))));
	assert_eq!(span_of(&second), Some((Rational::new(25, 1), Rational::new(100, 1))));
	assert_eq!(
		span_of(&first).map(|(_, out)| out),
		span_of(&second).map(|(in_, _)| in_),
		"the two clips keep sharing the rolled seam"
	);
	assert_eq!(
		clip_media_in(&first),
		Rational::new(0, 1),
		"the left clip's media window is untouched (trim-out)"
	);
	assert_eq!(
		clip_media_in(&second),
		Rational::new(-25, 1),
		"the follower grows at its head, revealing earlier media"
	);

	cmd.undo();
	assert_eq!(span_of(&first), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&second), Some((Rational::new(50, 1), Rational::new(100, 1))));
	assert_eq!(clip_media_in(&first), Rational::new(0, 1));
	assert_eq!(clip_media_in(&second), Rational::new(0, 1));
}

/// `BlockTrimCommand` (default `set_remove_zero_length_from_graph`) removes
/// a zero-length adjacent from the whole graph; undo re-attaches it.
#[test]
fn block_trim_removes_zero_length_adjacent_from_graph() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(75, 1));

	// Trimming longer by exactly the gap length shrinks the gap to zero.
	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		clip.clone(),
		Rational::new(75, 1),
		MovementMode::TrimOut,
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 1);
	assert!(block_track(&gap).is_none());
	assert!(!node_in_graph(&project, &gap));

	cmd.undo();
	assert_eq!(track_block_count(&track), 2);
	assert!(node_in_graph(&project, &gap));
	assert_eq!(track_block_at(&track, 1).unwrap().id, gap.id);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(75, 1))));
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
}

/// `BlockTrimCommand::set_remove_zero_length_from_graph(false)` leaves a
/// zero-length adjacent in the graph (only off the track).
#[test]
fn block_trim_zero_length_adjacent_kept_in_graph() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(75, 1));

	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		clip.clone(),
		Rational::new(75, 1),
		MovementMode::TrimOut,
	);
	cmd.set_remove_zero_length_from_graph(false);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 1);
	assert!(block_track(&gap).is_none());
	assert!(node_in_graph(&project, &gap));

	cmd.undo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 1).unwrap().id, gap.id);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(75, 1))));
}

/// A trim that does not change the length is a no-op; the boxing path
/// (`to_command` + `redo_now`/`undo_now`) routes through the same body.
#[test]
fn block_trim_same_length_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));

	let mut cmd = BlockTrimCommand::new(
		track.clone(),
		clip.clone(),
		Rational::new(50, 1),
		MovementMode::TrimOut,
	);
	cmd.prepare();
	let mut boxed = cmd.to_command();
	boxed.redo_now();
	boxed.undo_now();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
}

/// `TrackSlideCommand` resizes both adjacent blocks by the movement;
/// undo restores their original lengths. The previous neighbour grows at
/// its out edge (in anchored; no negative in-point) and the next shrinks at
/// its in edge (out anchored, media advancing over the consumed head). The
/// command does not position the sliding blocks itself — the stored-range
/// model has no track layout, so the caller places them by `movement` (see
/// `graphops::slide_clip`).
#[test]
fn track_slide_resizes_adjacent_blocks() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let previous = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let slide = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));
	let next = add_clip(&track, Rational::new(100, 1), Rational::new(150, 1));

	let mut cmd = TrackSlideCommand::new(
		track.clone(),
		vec![slide.clone()],
		Some(previous.clone()),
		Some(next.clone()),
		Rational::new(20, 1),
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(span_of(&previous), Some((Rational::new(0, 1), Rational::new(70, 1))));
	assert_eq!(span_of(&slide), Some((Rational::new(50, 1), Rational::new(100, 1))));
	assert_eq!(span_of(&next), Some((Rational::new(120, 1), Rational::new(150, 1))));
	assert_eq!(
		clip_media_in(&previous),
		Rational::new(0, 1),
		"the in-anchored growth leaves the media window alone"
	);
	assert_eq!(
		clip_media_in(&next),
		Rational::new(20, 1),
		"the next block's head is consumed, advancing its media in"
	);

	cmd.undo();
	assert_eq!(span_of(&previous), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(span_of(&next), Some((Rational::new(100, 1), Rational::new(150, 1))));
	assert_eq!(clip_media_in(&previous), Rational::new(0, 1));
	assert_eq!(clip_media_in(&next), Rational::new(0, 1));
}

/// `TrackSlideCommand` removes the out adjacent when the movement exactly
/// consumes it; undo re-attaches it after the moving block. The in adjacent
/// grows at its out edge (in anchored) while the out adjacent disappears.
#[test]
fn track_slide_removes_out_adjacent_at_boundary() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let in_gap = add_gap(&track, Rational::new(0, 1), Rational::new(20, 1));
	let clip = add_clip(&track, Rational::new(20, 1), Rational::new(70, 1));
	let out_gap = add_gap(&track, Rational::new(70, 1), Rational::new(90, 1));

	let mut cmd = TrackSlideCommand::new(
		track.clone(),
		vec![clip.clone()],
		Some(in_gap.clone()),
		Some(out_gap.clone()),
		Rational::new(20, 1),
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert!(block_track(&out_gap).is_none());
	assert!(!node_in_graph(&project, &out_gap));
	assert_eq!(span_of(&in_gap), Some((Rational::new(0, 1), Rational::new(40, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 3);
	assert!(node_in_graph(&project, &out_gap));
	assert_eq!(track_block_at(&track, 2).unwrap().id, out_gap.id);
	assert_eq!(span_of(&in_gap), Some((Rational::new(0, 1), Rational::new(20, 1))));
	assert_eq!(span_of(&out_gap), Some((Rational::new(70, 1), Rational::new(90, 1))));
}

/// `TrackSlideCommand` removes the in adjacent when a leftward movement
/// exactly consumes it; undo re-attaches it before the moving block. The
/// out adjacent grows at its in edge (out anchored) as the clip slides left.
#[test]
fn track_slide_removes_in_adjacent_at_boundary() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let in_gap = add_gap(&track, Rational::new(0, 1), Rational::new(20, 1));
	let clip = add_clip(&track, Rational::new(20, 1), Rational::new(70, 1));
	let out_gap = add_gap(&track, Rational::new(70, 1), Rational::new(90, 1));

	let mut cmd = TrackSlideCommand::new(
		track.clone(),
		vec![clip.clone()],
		Some(in_gap.clone()),
		Some(out_gap.clone()),
		Rational::new(-20, 1),
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert!(block_track(&in_gap).is_none());
	assert!(!node_in_graph(&project, &in_gap));
	assert_eq!(span_of(&out_gap), Some((Rational::new(50, 1), Rational::new(90, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 3);
	assert!(node_in_graph(&project, &in_gap));
	assert_eq!(track_block_at(&track, 0).unwrap().id, in_gap.id);
	assert_eq!(span_of(&in_gap), Some((Rational::new(0, 1), Rational::new(20, 1))));
	assert_eq!(span_of(&out_gap), Some((Rational::new(70, 1), Rational::new(90, 1))));
}

/// `TrackSlideCommand::prepare` creates the missing in adjacent (a leftward
/// slide into empty space) and undo removes it again.
#[test]
fn track_slide_creates_in_adjacent_gap() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TrackSlideCommand::new(
		track.clone(),
		vec![clip.clone()],
		None,
		None,
		Rational::new(-10, 1),
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	let created = track_block_at(&track, 0).unwrap();
	assert_eq!(block_kind(&created), BlockKind::Gap);
	assert_eq!(span_of(&created), Some((Rational::new(-10, 1), Rational::new(0, 1))));
	assert_eq!(track_block_at(&track, 1).unwrap().id, clip.id);

	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert!(!node_in_graph(&project, &created));
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TrackSlideCommand::prepare` creates the missing out adjacent when the
/// moving block has a successor, and undo removes it again.
#[test]
fn track_slide_creates_out_adjacent_gap() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let in_gap = add_gap(&track, Rational::new(0, 1), Rational::new(10, 1));
	let clip = add_clip(&track, Rational::new(10, 1), Rational::new(110, 1));
	let tail = add_clip(&track, Rational::new(110, 1), Rational::new(160, 1));

	let mut cmd = TrackSlideCommand::new(
		track.clone(),
		vec![clip.clone()],
		Some(in_gap.clone()),
		None,
		Rational::new(10, 1),
	);
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 4);
	let created = track_block_at(&track, 2).unwrap();
	assert_eq!(block_kind(&created), BlockKind::Gap);
	assert_eq!(span_of(&created), Some((Rational::new(110, 1), Rational::new(120, 1))));
	assert_eq!(track_block_at(&track, 3).unwrap().id, tail.id);

	cmd.undo();
	assert_eq!(track_block_count(&track), 3);
	assert!(!node_in_graph(&project, &created));
	assert_eq!(span_of(&in_gap), Some((Rational::new(0, 1), Rational::new(10, 1))));
	assert_eq!(span_of(&clip), Some((Rational::new(10, 1), Rational::new(110, 1))));
}

// ---------------------------------------------------------------------------
// undotrack.rs
// ---------------------------------------------------------------------------

/// `TrackPrependBlockCommand` prepends the block; undo detaches it again.
/// The trait dispatch (the oakundo path) is exercised explicitly.
#[test]
fn track_prepend_block_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let existing = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let fresh = new_clip(&project, Rational::new(50, 1), Rational::new(100, 1));

	let mut cmd = TrackPrependBlockCommand::new(track.clone(), fresh.clone());
	Command::redo(&mut cmd);
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 0).unwrap().id, fresh.id);
	assert_eq!(block_track(&fresh).map(|t| t.id), Some(track.id));

	Command::undo(&mut cmd);
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(track_block_at(&track, 0).unwrap().id, existing.id);
	assert!(block_track(&fresh).is_none());

	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
}

/// `TrackInsertBlockAfterCommand` inserts after a predecessor (`None`
/// prepends) and undo ripples the block back out; the boxing path is
/// covered too.
#[test]
fn track_insert_block_after_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let first = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let second = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));
	let inserted = new_clip(&project, Rational::new(50, 1), Rational::new(75, 1));

	let mut cmd =
		TrackInsertBlockAfterCommand::new(track.clone(), inserted.clone(), Some(first.clone()));
	cmd.redo();
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(track_block_at(&track, 0).unwrap().id, first.id);
	assert_eq!(track_block_at(&track, 1).unwrap().id, inserted.id);
	assert_eq!(track_block_at(&track, 2).unwrap().id, second.id);

	cmd.undo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 0).unwrap().id, first.id);
	assert_eq!(track_block_at(&track, 1).unwrap().id, second.id);

	// `None` predecessor inserts at the front.
	let front = new_clip(&project, Rational::new(200, 1), Rational::new(250, 1));
	let mut boxed =
		TrackInsertBlockAfterCommand::new(track.clone(), front.clone(), None).to_command();
	boxed.redo_now();
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(track_block_at(&track, 0).unwrap().id, front.id);
	boxed.undo_now();
	assert_eq!(track_block_count(&track), 2);
	assert!(block_track(&front).is_none());
}

/// `TrackReplaceBlockCommand` swaps one block for another at the same
/// track slot and swaps back on undo.
#[test]
fn track_replace_block_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let old = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let replacement = new_clip(&project, Rational::new(0, 1), Rational::new(50, 1));

	let mut cmd = TrackReplaceBlockCommand::new(track.clone(), old.clone(), replacement.clone());
	cmd.redo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(track_block_at(&track, 0).unwrap().id, replacement.id);
	assert_eq!(block_track(&replacement).map(|t| t.id), Some(track.id));
	assert!(block_track(&old).is_none());

	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(track_block_at(&track, 0).unwrap().id, old.id);
	assert_eq!(block_track(&old).map(|t| t.id), Some(track.id));
	assert!(block_track(&replacement).is_none());
}

// ---------------------------------------------------------------------------
// undosplit.rs
// ---------------------------------------------------------------------------

/// `TrackSplitAtTimeCommand` splits the block containing the point; undo
/// re-joins the halves, a second redo re-attaches the same second half.
#[test]
fn track_split_at_time_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd = TrackSplitAtTimeCommand::new(track.clone(), Rational::new(40, 1));
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 0).unwrap().id, clip.id);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(40, 1))));
	let second = track_block_at(&track, 1).unwrap();
	assert_eq!(block_kind(&second), BlockKind::Clip);
	assert_eq!(span_of(&second), Some((Rational::new(40, 1), Rational::new(100, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));

	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	assert_eq!(track_block_at(&track, 1).unwrap().id, second.id);
}

/// `TrackSplitAtTimeCommand` with a point outside the block (past the
/// end, on the in point or on the out point) is a no-op.
#[test]
fn track_split_at_time_outside_block_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	for point in [Rational::new(100, 1), Rational::new(0, 1), Rational::new(150, 1)] {
		let mut cmd = TrackSplitAtTimeCommand::new(track.clone(), point);
		cmd.redo();
		cmd.undo();
		assert_eq!(track_block_count(&track), 1);
		assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	}
}

/// `BlockSplitPreservingLinksCommand` splits every block containing the
/// time and links the resulting halves of originally linked blocks; undo
/// restores all originals.
#[test]
fn block_split_preserving_links_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track_a = TimelineAddTrackCommand::run_immediately(list.clone());
	let track_b = TimelineAddTrackCommand::run_immediately(list.clone());
	let a = add_clip(&track_a, Rational::new(0, 1), Rational::new(100, 1));
	let b = add_clip(&track_b, Rational::new(0, 1), Rational::new(100, 1));
	link_blocks(&project, &a, &b);
	assert!(blocks_are_linked(&project, &a, &b));

	let mut cmd = BlockSplitPreservingLinksCommand::new(
		vec![a.clone(), b.clone()],
		vec![Rational::new(40, 1)],
	);
	// `redo` builds and applies the child splits on first use.
	cmd.redo();
	assert_eq!(track_block_count(&track_a), 2);
	assert_eq!(track_block_count(&track_b), 2);
	assert_eq!(span_of(&a), Some((Rational::new(0, 1), Rational::new(40, 1))));
	assert_eq!(span_of(&b), Some((Rational::new(0, 1), Rational::new(40, 1))));

	let split_a = cmd.get_split(&a, 0).expect("a was split");
	let split_b = cmd.get_split(&b, 0).expect("b was split");
	assert_eq!(split_a.id, track_block_at(&track_a, 1).unwrap().id);
	assert_eq!(split_b.id, track_block_at(&track_b, 1).unwrap().id);
	assert_eq!(span_of(&split_a), Some((Rational::new(40, 1), Rational::new(100, 1))));
	assert_eq!(span_of(&split_b), Some((Rational::new(40, 1), Rational::new(100, 1))));
	// The new halves of the linked pair stay linked.
	assert!(blocks_are_linked(&project, &split_a, &split_b));
	assert!(cmd.get_split(&split_a, 0).is_none());
	assert!(cmd.get_split(&a, 1).is_none());

	cmd.undo();
	assert_eq!(track_block_count(&track_a), 1);
	assert_eq!(track_block_count(&track_b), 1);
	assert_eq!(span_of(&a), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert_eq!(span_of(&b), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert!(blocks_are_linked(&project, &a, &b));

	cmd.redo();
	assert_eq!(track_block_count(&track_a), 2);
	assert_eq!(track_block_count(&track_b), 2);
}

/// A time that lies outside every block, and an empty block/time list,
/// produce no child commands and leave the tracks untouched.
#[test]
fn block_split_preserving_links_noop_time() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd =
		BlockSplitPreservingLinksCommand::new(vec![clip.clone()], vec![Rational::new(200, 1)]);
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	assert!(cmd.get_split(&clip, 0).is_none());

	let mut empty = BlockSplitPreservingLinksCommand::new(Vec::new(), Vec::new());
	empty.redo();
	empty.undo();
	assert_eq!(track_block_count(&track), 1);
}

// ---------------------------------------------------------------------------
// undogeneral.rs
// ---------------------------------------------------------------------------

/// `BlockResizeWithMediaInCommand` resizes keeping the timeline in-point
/// fixed while `media_in` follows the length change, so the media content
/// end stays anchored (C++ `ClipBlock::set_length_and_media_in`); undo
/// restores the original length and media window.
#[test]
fn block_resize_with_media_in_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));

	let mut cmd = BlockResizeWithMediaInCommand::new(clip.clone(), Rational::new(30, 1));
	cmd.redo();
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(30, 1))));
	assert_eq!(
		clip_media_in(&clip),
		Rational::new(20, 1),
		"media_in advances by the trimmed 20 frames (media_out stays at 50)"
	);
	cmd.undo();
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));
}

/// `BlockSetMediaInCommand` writes the media-in point and restores it on
/// undo; the boxing path is covered too.
#[test]
fn block_set_media_in_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));

	let mut cmd = BlockSetMediaInCommand::new(clip.clone(), Rational::new(12, 1));
	cmd.redo();
	assert_eq!(clip_media_in(&clip), Rational::new(12, 1));
	cmd.undo();
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));

	let mut boxed = BlockSetMediaInCommand::new(clip.clone(), Rational::new(20, 1)).to_command();
	boxed.redo_now();
	assert_eq!(clip_media_in(&clip), Rational::new(20, 1));
	boxed.undo_now();
	assert_eq!(clip_media_in(&clip), Rational::new(0, 1));
}

/// `BlockEnableDisableCommand` flips the enabled flag (captured at
/// construction) and restores it on undo; the boxing path is covered too.
#[test]
fn block_enable_disable_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	assert!(block_enabled(&clip));

	let mut cmd = BlockEnableDisableCommand::new(clip.clone(), false);
	cmd.redo();
	assert!(!block_enabled(&clip));
	cmd.undo();
	assert!(block_enabled(&clip));

	let mut boxed = BlockEnableDisableCommand::new(clip.clone(), false).to_command();
	boxed.redo_now();
	assert!(!block_enabled(&clip));
	boxed.undo_now();
	assert!(block_enabled(&clip));
}

/// `TrackListInsertGaps` extends an existing gap crossed by the insertion
/// point instead of adding a second one; undo restores its length. The
/// extension is in-anchored (the insertion pushes the following content
/// rightward; the C++ derives that shift from the track layout, while this
/// caller-facing command leaves it to whoever places the followers).
#[test]
fn track_list_insert_gaps_extends_existing_gap() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	let gap = add_gap(&track, Rational::new(50, 1), Rational::new(100, 1));
	add_clip(&track, Rational::new(100, 1), Rational::new(150, 1));

	let mut cmd =
		TrackListInsertGaps::new(list, Rational::new(75, 1), Rational::new(20, 1));
	cmd.prepare();
	cmd.redo();
	// The gap keeps its in point and grows rightward by the gap length.
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(120, 1))));

	cmd.undo();
	assert_eq!(track_block_count(&track), 3);
	assert_eq!(span_of(&gap), Some((Rational::new(50, 1), Rational::new(100, 1))));
}

/// `TrackListInsertGaps` at the in point of the first block prepends the
/// gap; undo removes it again.
#[test]
fn track_list_insert_gaps_at_track_start_round_trip() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));

	let mut cmd =
		TrackListInsertGaps::new(list, Rational::new(0, 1), Rational::new(20, 1));
	cmd.prepare();
	cmd.redo();
	assert_eq!(track_block_count(&track), 2);
	let gap = track_block_at(&track, 0).unwrap();
	assert_eq!(block_kind(&gap), BlockKind::Gap);
	assert_eq!(span_of(&gap), Some((Rational::new(0, 1), Rational::new(20, 1))));
	assert_eq!(track_block_at(&track, 1).unwrap().id, clip.id);

	cmd.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
}

/// `TrackListInsertGaps` skips locked tracks; a point past the track end
/// produces no work at all.
#[test]
fn track_list_insert_gaps_locked_and_past_end_are_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list.clone());
	let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
	set_track_locked(&track, true);

	let mut locked =
		TrackListInsertGaps::new(list.clone(), Rational::new(25, 1), Rational::new(20, 1));
	locked.prepare();
	locked.redo();
	locked.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));

	// The point is past the single 50-long clip: no split, no gap.
	set_track_locked(&track, false);
	let mut past_end = TrackListInsertGaps::new(list, Rational::new(100, 1), Rational::new(20, 1));
	past_end.prepare();
	past_end.redo();
	past_end.undo();
	assert_eq!(track_block_count(&track), 1);
	assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
}

/// `TimelineAddDefaultTransitionCommand` walks the selection and builds no
/// child commands (the default-transition config has no Rust model yet),
/// so redo/undo are no-ops; `prepare` and the boxing path still run.
#[test]
fn timeline_add_default_transition_is_noop() {
	let project = make_project();
	let (_seq, list) = sequence_and_list(&project);
	let track = TimelineAddTrackCommand::run_immediately(list);
	add_gap(&track, Rational::new(0, 1), Rational::new(10, 1));
	let a = add_clip(&track, Rational::new(10, 1), Rational::new(110, 1));
	let b = add_clip(&track, Rational::new(110, 1), Rational::new(210, 1));
	add_gap(&track, Rational::new(210, 1), Rational::new(230, 1));

	let mut cmd = TimelineAddDefaultTransitionCommand::new(
		vec![a.clone(), b.clone()],
		Rational::new(1, 30),
	);
	cmd.prepare();
	cmd.redo();
	cmd.undo();
	assert_eq!(track_block_count(&track), 4);
	assert_eq!(block_kind(&track_block_at(&track, 0).unwrap()), BlockKind::Gap);
	assert_eq!(block_kind(&track_block_at(&track, 1).unwrap()), BlockKind::Clip);
	assert_eq!(block_kind(&track_block_at(&track, 2).unwrap()), BlockKind::Clip);
	assert_eq!(block_kind(&track_block_at(&track, 3).unwrap()), BlockKind::Gap);
	assert_eq!(span_of(&a), Some((Rational::new(10, 1), Rational::new(110, 1))));

	let mut boxed = TimelineAddDefaultTransitionCommand::new(
		vec![a.clone()],
		Rational::new(1, 30),
	)
	.to_command();
	boxed.redo_now();
	boxed.undo_now();
	assert_eq!(track_block_count(&track), 4);
}

// ---------------------------------------------------------------------------
// Boxing sweep: `to_command` for the remaining commands
// ---------------------------------------------------------------------------

/// The oakundo boxing path dispatches through the `Command` trait for the
/// commands that build their derived state on the first `redo`. One fresh
/// project per command keeps the command state independent.
#[test]
fn boxed_self_preparing_commands_round_trip() {
	// TrackRippleRemoveAreaCommand (splice path).
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		let clip = add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
		let mut boxed = TrackRippleRemoveAreaCommand::new(
			track.clone(),
			TimeRange::new(Rational::new(25, 1), Rational::new(75, 1)),
		)
		.to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track), 2);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
		assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(100, 1))));
	}

	// TrackListRippleRemoveAreaCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list.clone());
		add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
		let mut cmd = TrackListRippleRemoveAreaCommand::new(
			list,
			Rational::new(25, 1),
			Rational::new(75, 1),
		);
		// `redo` runs the children but does not build them: prepare first.
		cmd.prepare();
		let mut boxed = cmd.to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track), 2);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
	}

	// TimelineRippleDeleteGapsAtRegionsCommand.
	{
		let project = make_project();
		let (seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
		let gap = add_gap(&track, Rational::new(50, 1), Rational::new(100, 1));
		let mut boxed = TimelineRippleDeleteGapsAtRegionsCommand::new(
			seq,
			vec![(
				track.clone(),
				TimeRange::new(Rational::new(50, 1), Rational::new(100, 1)),
			)],
		)
		.to_command();
		boxed.redo_now();
		assert!(block_track(&gap).is_none());
		boxed.undo_now();
		assert!(block_track(&gap).is_some());
	}

	// TrackSplitAtTimeCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
		let mut boxed =
			TrackSplitAtTimeCommand::new(track.clone(), Rational::new(40, 1)).to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track), 2);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
	}

	// BlockSplitPreservingLinksCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track_a = TimelineAddTrackCommand::run_immediately(list.clone());
		let track_b = TimelineAddTrackCommand::run_immediately(list);
		let a = add_clip(&track_a, Rational::new(0, 1), Rational::new(100, 1));
		let b = add_clip(&track_b, Rational::new(0, 1), Rational::new(100, 1));
		let mut boxed = BlockSplitPreservingLinksCommand::new(
			vec![a, b],
			vec![Rational::new(40, 1)],
		)
		.to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track_a), 2);
		assert_eq!(track_block_count(&track_b), 2);
		boxed.undo_now();
		assert_eq!(track_block_count(&track_a), 1);
		assert_eq!(track_block_count(&track_b), 1);
	}

	// BlockSplitCommand (round-tripped in domain_test; boxing here).
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
		let host = track_block_at(&track, 0).unwrap();
		let mut boxed = BlockSplitCommand::new(host, Rational::new(40, 1)).to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track), 2);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
	}

	// TrackPrependBlockCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
		let fresh = new_clip(&project, Rational::new(50, 1), Rational::new(100, 1));
		let mut boxed = TrackPrependBlockCommand::new(track.clone(), fresh.clone()).to_command();
		boxed.redo_now();
		assert_eq!(track_block_at(&track, 0).unwrap().id, fresh.id);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
	}

	// TrackReplaceBlockCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		let old = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
		let replacement = new_clip(&project, Rational::new(0, 1), Rational::new(50, 1));
		let mut boxed =
			TrackReplaceBlockCommand::new(track.clone(), old.clone(), replacement.clone())
				.to_command();
		boxed.redo_now();
		assert_eq!(track_block_at(&track, 0).unwrap().id, replacement.id);
		boxed.undo_now();
		assert_eq!(track_block_at(&track, 0).unwrap().id, old.id);
	}

	// BlockResizeWithMediaInCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		let clip = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
		let mut boxed =
			BlockResizeWithMediaInCommand::new(clip.clone(), Rational::new(30, 1)).to_command();
		boxed.redo_now();
		assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(30, 1))));
		boxed.undo_now();
		assert_eq!(span_of(&clip), Some((Rational::new(0, 1), Rational::new(50, 1))));
	}
}

/// The boxing path for the commands whose derived state is built by an
/// explicit `prepare` (the boxed `Command` trait has no prepare callback).
#[test]
fn boxed_prepared_commands_round_trip() {
	// TrackSlideCommand.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list);
		let previous = add_clip(&track, Rational::new(0, 1), Rational::new(50, 1));
		let slide = add_clip(&track, Rational::new(50, 1), Rational::new(100, 1));
		let next = add_clip(&track, Rational::new(100, 1), Rational::new(150, 1));
		let mut cmd = TrackSlideCommand::new(
			track.clone(),
			vec![slide],
			Some(previous.clone()),
			Some(next.clone()),
			Rational::new(20, 1),
		);
		cmd.prepare();
		let mut boxed = cmd.to_command();
		boxed.redo_now();
		assert_eq!(span_of(&previous), Some((Rational::new(0, 1), Rational::new(70, 1))));
		assert_eq!(span_of(&next), Some((Rational::new(120, 1), Rational::new(150, 1))));
		boxed.undo_now();
		assert_eq!(span_of(&previous), Some((Rational::new(0, 1), Rational::new(50, 1))));
		assert_eq!(span_of(&next), Some((Rational::new(100, 1), Rational::new(150, 1))));
	}

	// TrackListInsertGaps.
	{
		let project = make_project();
		let (_seq, list) = sequence_and_list(&project);
		let track = TimelineAddTrackCommand::run_immediately(list.clone());
		add_clip(&track, Rational::new(0, 1), Rational::new(100, 1));
		let mut cmd = TrackListInsertGaps::new(list, Rational::new(40, 1), Rational::new(20, 1));
		cmd.prepare();
		let mut boxed = cmd.to_command();
		boxed.redo_now();
		assert_eq!(track_block_count(&track), 3);
		boxed.undo_now();
		assert_eq!(track_block_count(&track), 1);
	}
}

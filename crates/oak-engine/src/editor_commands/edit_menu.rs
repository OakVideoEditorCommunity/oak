//! Composite edit-menu operations built from lower-level timeline commands.
//!
//! Public operations execute and group their commands into undo entries.
//! Helpers ending in `_commands` only assemble commands so a caller can combine
//! them with other edits. Integer positions and durations use the sequence's
//! frame timebase unless a function explicitly accepts rational time.

#![allow(unused_imports)]
use super::prelude::*;
use super::{editing::*, generators::*, media::*, moving::*, project::*, sequence::*, tracks::*, transitions::*};


/// Duplicate `clips` right after the selection's end (undoable
/// "Duplicate"): clipboard copy + paste anchored at the latest out point
/// (the C++ Edit > Duplicate semantics).
pub fn duplicate_clips(
	p: &ProjectRef,
	seq: NodeId,
	clips: &[NodeId],
) -> Result<Vec<NodeId>, String> {
	if clips.is_empty() {
		return Err("nothing selected".to_string());
	}
	let items = copy_clips(p, clips);
	if items.is_empty() {
		return Err("nothing to duplicate".to_string());
	}
	let end = items
		.iter()
		.map(|c| c.start_ts + c.length_ts)
		.max()
		.unwrap_or(0);
	paste_clips(p, seq, &items, end)
}

/// Builds commands that shift every unlocked block starting at or after
/// `from_ts` by `delta_ts` (both are frame timestamps). The boundary is
/// inclusive: a block starting exactly at `from_ts` is also shifted.
///
/// The commands are returned without being executed or pushed onto the undo
/// stack, allowing insert-paste to combine the shifts and placement into one
/// undo entry.
pub(crate) fn shift_blocks_at_or_after_commands(
	p: &ProjectRef,
	seq: NodeId,
	from_ts: i64,
	delta_ts: i64,
) -> Result<Vec<oak_undo::undocommand::UndoCommand>, String> {
	if delta_ts == 0 {
		return Ok(Vec::new());
	}
	let (tb, targets) = {
		let g = lock(p);
		let tb = sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?;
		let from = ts_to_rational(from_ts, tb);
		let mut targets = Vec::new();
		for kind in [TrackType::Video, TrackType::Audio] {
			let Some(list) = track_list_of(&g.graph, seq, kind) else {
				continue;
			};
			let Some(l) = track_list_behavior(&g.graph, list) else {
				continue;
			};
			for &track in &l.tracks {
				if track_behavior(&g.graph, track).map(|t| t.locked).unwrap_or(false) {
					continue;
				}
				let Some(t) = track_behavior(&g.graph, track) else {
					continue;
				};
				for &b in &t.blocks {
					let Some(core) = block_core_of(&g.graph, b) else {
						continue;
					};
					if core.range.in_() >= from {
						targets.push(b);
					}
				}
			}
		}
		(tb, targets)
	};
	let delta = ts_to_rational(delta_ts, tb);
	let mut children = Vec::new();
	for b in targets {
		let (r1, r2) = (node_ref(p, b), node_ref(p, b));
		let old_in = block_in(&r1);
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || block_set_in(&r1, old_in + delta),
			move || block_set_in(&r2, old_in),
		));
	}
	Ok(children)
}

/// Pastes with ripple as one undoable "Paste Insert" entry.
///
/// The pasted span is measured from `items[0].start_ts` to the latest relative
/// end in the clipboard. Every unlocked video/audio block starting at or after
/// the playhead first shifts right by that span; the clipboard is then pasted
/// with its first item anchored at the playhead. An empty clipboard is an
/// error, and no commands are executed if command construction fails.
pub fn paste_clips_insert(
	p: &ProjectRef,
	seq: NodeId,
	items: &[ClipboardClip],
	playhead_ts: i64,
) -> Result<Vec<NodeId>, String> {
	if items.is_empty() {
		return Err("the clipboard is empty".to_string());
	}
	let anchor = items[0].start_ts;
	let span = items
		.iter()
		.map(|c| c.start_ts - anchor + c.length_ts)
		.max()
		.unwrap_or(0);
	let mut children = shift_blocks_at_or_after_commands(p, seq, playhead_ts, span)?;
	// The paste machinery's command builder, minus its push.
	let (mut paste_cmds, new_ids) = paste_clips_commands(p, seq, items, playhead_ts)?;
	children.append(&mut paste_cmds);
	push_multi(children, "Paste Insert")?;
	Ok(new_ids)
}

/// Builds the placement, footage-connection and link commands for a paste.
///
/// The returned commands and node IDs follow `items` order and are not executed
/// here, allowing paste-insert to prepend tail shifts in the same undo entry.
/// Additional clips are linked to the first new clip in a star topology.
/// Clip nodes are allocated while commands are assembled; if a later item
/// fails validation, earlier allocations are not transactionally rolled back.
pub(crate) fn paste_clips_commands(
	p: &ProjectRef,
	seq: NodeId,
	items: &[ClipboardClip],
	playhead_ts: i64,
) -> Result<(Vec<oak_undo::undocommand::UndoCommand>, Vec<NodeId>), String> {
	if items.is_empty() {
		return Err("the clipboard is empty".to_string());
	}
	let anchor = items[0].start_ts;
	let mut clips = Vec::with_capacity(items.len());
	for item in items {
		let start = playhead_ts + (item.start_ts - anchor);
		let out = start + item.length_ts;
		clips.push((item, start, out));
	}

	let tb = {
		let g = lock(p);
		sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?
	};
	let track_count_of = |kind: TrackType| -> usize {
		let g = lock(p);
		track_list_of(&g.graph, seq, kind)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.map(|l| l.tracks.len())
			.unwrap_or(0)
	};

	let mut new_ids = Vec::with_capacity(items.len());
	let mut commands = Vec::new();
	for (item, start, out) in clips {
		let list = {
			let g = lock(p);
			track_list_of(&g.graph, seq, item.kind)
				.ok_or_else(|| "sequence has no track list for this type".to_string())?
		};
		let count = track_count_of(item.kind);
		if count == 0 {
			return Err("no track of the clip's kind to paste onto".to_string());
		}
		let track_index = item.track_index.min(count - 1);
		let in_r = ts_to_rational(start, tb);
		let media_r = ts_to_rational(item.media_in_ts, tb);
		let length_r = ts_to_rational(out, tb) - in_r;

		let clip = {
			let mut g = lock(p);
			let id = create_footage_clip(&mut g.graph, item.footage, media_r, length_r);
			if let Some(c) = g
				.graph
				.get_mut(id)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
			{
				c.core.speed = item.speed;
			}
			id
		};
		new_ids.push(clip);
		commands.push(
			oak_timeline::undopointer::TrackPlaceBlockCommand::new(
				node_ref(p, list),
				track_index as i32,
				node_ref(p, clip),
				in_r,
			)
			.to_command(),
		);
		commands.push(connect_command(
			p,
			item.footage,
			clip,
			oak_node::block::clip_input::TEXTURE_INPUT,
		)?);
	}
	if new_ids.len() > 1 {
		let first = new_ids[0];
		for &other in &new_ids[1..] {
			let (p1, p2) = (p.clone(), p.clone());
			commands.push(oak_undo::undocommand::UndoCommand::from_closures(
				move || {
					lock(&p1).graph.link(first, other);
				},
				move || {
					lock(&p2).graph.unlink(first, other);
				},
			));
		}
	}
	Ok((commands, new_ids))
}

/// Builds one area-deletion undo entry across every unlocked video/audio track.
///
/// Each selected track receives a `TrackRippleRemoveAreaCommand` for the half-
/// open `[in_ts, out_ts)` range. When `ripple` is true, blocks whose original
/// in-point is at or after `out_ts` also receive a command shifting them left
/// by the range duration. When it is false, no additional tail-shift commands
/// are added. Locked tracks are excluded, and `out_ts <= in_ts` is rejected
/// before any command is constructed.
pub fn delete_area(
	p: &ProjectRef,
	seq: NodeId,
	in_ts: i64,
	out_ts: i64,
	ripple: bool,
) -> Result<(), String> {
	if out_ts <= in_ts {
		return Err("invalid area (need in < out)".to_string());
	}
	let (tb, tracks) = {
		let g = lock(p);
		let tb = sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?;
		let mut tracks = Vec::new();
		for kind in [TrackType::Video, TrackType::Audio] {
			let Some(list) = track_list_of(&g.graph, seq, kind) else {
				continue;
			};
			let Some(l) = track_list_behavior(&g.graph, list) else {
				continue;
			};
			for &track in &l.tracks {
				if track_behavior(&g.graph, track).map(|t| t.locked).unwrap_or(false) {
					continue;
				}
				tracks.push(track);
			}
		}
		(tb, tracks)
	};
	let in_r = ts_to_rational(in_ts, tb);
	let out_r = ts_to_rational(out_ts, tb);
	let mut children = Vec::new();
	for track in &tracks {
		children.push(
			oak_timeline::undoripple::TrackRippleRemoveAreaCommand::new(
				node_ref(p, *track),
				TimeRange::new(in_r, out_r),
			)
			.to_command(),
		);
	}
	if ripple {
		// Capture the original tail before executing any removal command. The
		// shifts are appended after the removals; multi-command undo runs in
		// reverse order, restoring these positions before undoing the removals.
		let span = out_r - in_r;
		let targets = {
			let g = lock(p);
			let mut targets = Vec::new();
			for &track in &tracks {
				let Some(t) = track_behavior(&g.graph, track) else {
					continue;
				};
				for &b in &t.blocks {
					let Some(core) = block_core_of(&g.graph, b) else {
						continue;
					};
					if core.range.in_() >= out_r {
						targets.push(b);
					}
				}
			}
			targets
		};
		for b in targets {
			let (r1, r2) = (node_ref(p, b), node_ref(p, b));
			let old_in = block_in(&r1);
			children.push(oak_undo::undocommand::UndoCommand::from_closures(
				move || block_set_in(&r1, old_in - span),
				move || block_set_in(&r2, old_in),
			));
		}
	}
	push_multi(
		children,
		if ripple {
			"Ripple Delete In/Out"
		} else {
			"Delete In/Out"
		},
	)
}

/// Rolls one clip edge and its immediate neighbor to `frame_ts`.
///
/// `at_in` selects the previous block; otherwise the next block is used.
/// A missing neighbor or a non-clip neighbor is a successful no-op. Otherwise
/// this delegates to [`roll_edit`]: the blocks must share an exact boundary,
/// and `frame_ts` must lie strictly inside their combined range. Invalid
/// boundaries or out-of-range positions return an error rather than no-op.
pub fn roll_clip_edge_to(
	p: &ProjectRef,
	clip: NodeId,
	at_in: bool,
	frame_ts: i64,
) -> Result<(), String> {
	let (track, blocks, index) = {
		let g = lock(p);
		let track = clip_track(&g.graph, clip)
			.ok_or_else(|| "the clip is not on a track".to_string())?;
		let blocks = track_behavior(&g.graph, track)
			.map(|t| t.blocks.clone())
			.unwrap_or_default();
		let index = blocks
			.iter()
			.position(|&b| b == clip)
			.ok_or_else(|| "the clip is not on its track".to_string())?;
		(track, blocks, index)
	};
	let neighbor = if at_in {
		index.checked_sub(1).map(|i| blocks[i])
	} else {
		blocks.get(index + 1).copied()
	};
	let Some(neighbor) = neighbor else {
		return Ok(());
	};
	// Only clip blocks roll (a gap neighbor has no media to reveal).
	let is_clip = |b: NodeId| {
		let g = lock(p);
		clip_behavior(&g.graph, b).is_some()
	};
	if !is_clip(neighbor) {
		return Ok(());
	}
	let (a, b) = if at_in { (neighbor, clip) } else { (clip, neighbor) };
	roll_edit(p, track, a, b, frame_ts)
}

/// Nest `clips` into a new sequence and replace them with one nested
/// clip spanning their range ("嵌套"): the new sequence (named after the
/// first clip) lands in the root folder and holds the clips at their
/// relative offsets; the originals are deleted from the host sequence.
///
/// The steps are separate undo entries (the sequence factory and the
/// placement machinery predate command composition).
pub fn nest_clips(p: &ProjectRef, seq: NodeId, clips: &[NodeId]) -> Result<NodeId, String> {
	if clips.is_empty() {
		return Err("nothing selected".to_string());
	}
	let items = copy_clips(p, clips);
	if items.is_empty() {
		return Err("nothing to nest".to_string());
	}
	let start = items.iter().map(|c| c.start_ts).min().unwrap_or(0);
	let end = items
		.iter()
		.map(|c| c.start_ts + c.length_ts)
		.max()
		.unwrap_or(0);
	let track_index = items[0].track_index;
	let label = {
		let g = lock(p);
		g.graph
			.get(clips[0])
			.map(|e| e.core.label.clone())
			.filter(|s| !s.is_empty())
			.unwrap_or_else(|| "Nested Sequence".to_string())
	};
	let nested_seq = create_sequence(p, &label);
	// Repack the clips at their relative offsets starting at 0.
	let mut rel = items.clone();
	for item in &mut rel {
		item.start_ts -= start;
	}
	paste_clips(p, nested_seq, &rel, 0)?;
	let nested = place_nested_sequence_clip(p, seq, nested_seq, track_index, start, end)?;
	// The nested clip's placement overwrites the covered originals
	// (the placement machinery splits/removes what it covers); delete
	// whatever survives, ignoring the already-gone.
	for &clip in clips {
		let _ = delete_clip(p, clip);
	}
	Ok(nested)
}

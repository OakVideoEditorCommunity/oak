//! Clip trimming, rolling, sliding, slipping and track movement.
//!
//! These operations preserve the distinction between timeline in/out points
//! and source-media offsets. Integer arguments are frame timestamps converted
//! through the owning sequence's timebase. Public functions push undo entries;
//! `_command`/`_commands` helpers leave execution and grouping to callers.

#![allow(unused_imports)]
use super::prelude::*;
use super::{edit_menu::*, generators::*, media::*, moving::*, project::*, sequence::*, tracks::*, transitions::*};


/// Trim `clip`'s timeline range to `[new_in_ts, new_out_ts)` (undoable
/// "Trim Clip"; the facade's `oakengine_clip_trim` semantics: one end at
/// a time, trim-in anchors the OUT (its in moves and the media window
/// follows), trim-out anchors the IN (its out moves, media untouched) —
/// the same net anchors as the module's own `BlockTrimCommand`).
pub fn trim_clip(
	p: &ProjectRef,
	clip: NodeId,
	new_in_ts: i64,
	new_out_ts: i64,
) -> Result<(), String> {
	if new_in_ts < 0 || new_out_ts <= new_in_ts {
		return Err("invalid trim range (need 0 <= new_in < new_out)".to_string());
	}
	let (tb, old_in, old_out, old_length) = {
		let g = lock(p);
		let tb = clip_track(&g.graph, clip)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (in_r, out_r, _) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		let length = out_r - in_r;
		(tb, in_r, out_r, length)
	};
	if new_in_ts == rational_to_ts(old_in, tb) && new_out_ts == rational_to_ts(old_out, tb) {
		return Ok(());
	}
	let new_in = ts_to_rational(new_in_ts, tb);
	let new_out = ts_to_rational(new_out_ts, tb);

	let mut children = Vec::new();
	if new_in != old_in {
		// in-trim: length = block out - new in (out anchored).
		children.push(trim_command(p, clip, true, old_length, old_out - new_in));
	}
	if new_out != old_out {
		// out-trim: length = new out - new in (in anchored); the old
		// length is the post-in-trim length (out - new in) when both ends
		// move.
		let post_in_trim = old_out - new_in;
		children.push(trim_command(p, clip, false, post_in_trim, new_out - new_in));
	}
	push_multi(children, "Trim Clip")
}

/// Ripple-trim `clip` so its `start_edge` lands on `new_frame` (undoable
/// "Ripple Trim Clip"): the trimmed clip's media anchor is kept (trim-in
/// anchors the OUT, trim-out anchors the IN, exactly like [`trim_clip`]),
/// and every block after it on the same track shifts rigidly by the same
/// delta — no gap opens behind the trimmed edge, the tail just follows.
pub fn ripple_trim_clip(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	new_frame: i64,
) -> Result<(), String> {
	let (tb, old_in, old_out, tail) = {
		let g = lock(p);
		let tb = clip_track(&g.graph, clip)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (in_r, out_r, _) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		let track =
			clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?;
		let blocks = track_behavior(&g.graph, track)
			.map(|t| t.blocks.clone())
			.unwrap_or_default();
		let index = blocks
			.iter()
			.position(|&b| b == clip)
			.ok_or_else(|| "the clip is not on its track".to_string())?;
		(tb, in_r, out_r, blocks[index + 1..].to_vec())
	};
	let new = ts_to_rational(new_frame, tb);
	let old_length = old_out - old_in;
	// start_edge (trim-in): the out stays anchored, length = out - new;
	// the tail shifts by (new - in). Otherwise (trim-out): the in stays
	// anchored, length = new - in; the tail shifts by (new - out).
	let (trim_len, anchored) = if start_edge {
		(old_out - new, true)
	} else {
		(new - old_in, false)
	};
	let delta = if start_edge {
		new - old_in
	} else {
		new - old_out
	};
	if delta.is_null() {
		return Ok(());
	}
	let mut children = Vec::new();
	children.push(trim_command(p, clip, anchored, old_length, trim_len));
	for b in tail {
		let b_ref = node_ref(p, b);
		let b_old_in = block_in(&b_ref);
		let (r1, r2) = (b_ref.clone(), b_ref);
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || block_set_in(&r1, b_old_in + delta),
			move || block_set_in(&r2, b_old_in),
		));
	}
	push_multi(children, "Ripple Trim Clip")
}

/// Roll-edit the shared boundary of adjacent clips `a`/`b` (both on
/// `track`) to `new_frame` (undoable "Roll Edit"): the boundary moves
/// without disturbing anything else on the track — the module's own
/// `BlockTrimCommand` in roll-edit mode trims `a` out-anchored and
/// compensates `b` in-anchored, so both media anchors are preserved.
pub fn roll_edit(
	p: &ProjectRef,
	track: NodeId,
	a: NodeId,
	b: NodeId,
	new_frame: i64,
) -> Result<(), String> {
	let (tb, a_in, a_out, b_in, b_out) = {
		let g = lock(p);
		let tb = clip_track(&g.graph, a)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (a_in, a_out, _) =
			clip_range(&g.graph, a).ok_or_else(|| "node a is not a clip".to_string())?;
		let (b_in, b_out, _) =
			clip_range(&g.graph, b).ok_or_else(|| "node b is not a clip".to_string())?;
		(tb, a_in, a_out, b_in, b_out)
	};
	if a_out != b_in {
		return Err("roll edit: the clips do not share a boundary".to_string());
	}
	let new = ts_to_rational(new_frame, tb);
	if new <= a_in || new >= b_out {
		return Err("roll edit: boundary would escape the adjacent clips".to_string());
	}
	if new == a_out {
		// Boundary didn't move — nothing to do.
		return Ok(());
	}
	let mut cmd = oak_timeline::undopointer::BlockTrimCommand::new(
		node_ref(p, track),
		node_ref(p, a),
		new - a_in,
		oak_timeline::common::MovementMode::TrimOut,
	);
	cmd.set_trim_is_a_roll_edit(true);
	cmd.prepare();
	push(cmd.to_command(), "Roll Edit")
}

/// Slide `clip` so its in point becomes `new_start` (undoable "Slide
/// Clip"): the clip's own media window and length are untouched — the
/// in point moves and the out follows — while the adjacent blocks are
/// shortened/lengthened to fill the gap or absorb the overlap (the left
/// neighbor's out moves to `new_start`, the right neighbor's in follows
/// the clip's new out). A neighbor that would collapse to a negative
/// length is left alone, leaving the hole open.
pub fn slide_clip(p: &ProjectRef, clip: NodeId, new_start: i64) -> Result<(), String> {
	let (tb, old_in, old_out, left, right) = {
		let g = lock(p);
		let tb = clip_track(&g.graph, clip)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (in_r, out_r, _) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		let track =
			clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?;
		let blocks = track_behavior(&g.graph, track)
			.map(|t| t.blocks.clone())
			.unwrap_or_default();
		let index = blocks
			.iter()
			.position(|&b| b == clip)
			.ok_or_else(|| "the clip is not on its track".to_string())?;
		(
			tb,
			in_r,
			out_r,
			blocks.get(index.wrapping_sub(1)).copied(),
			blocks.get(index + 1).copied(),
		)
	};
	let new = ts_to_rational(new_start.max(0), tb);
	let delta = new - old_in;
	if delta.is_null() {
		return Ok(());
	}
	let clip_len = old_out - old_in;
	let mut children = Vec::new();
	// The clip itself slides: in moves, length and media window stay put
	// (`set_in` keeps the length and never touches the media in-point).
	{
		let c_ref = node_ref(p, clip);
		let (r1, r2) = (c_ref.clone(), c_ref);
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || block_set_in(&r1, new),
			move || block_set_in(&r2, old_in),
		));
	}
	// Left neighbor: its out follows the clip's new in (in anchored,
	// media untouched). A neighbor that would collapse to a negative
	// length is left alone, leaving the hole open.
	if let Some(l) = left {
		let l_ref = node_ref(p, l);
		let l_len = block_length(&l_ref);
		if new > block_in(&l_ref) {
			let (r1, r2) = (l_ref.clone(), l_ref);
			children.push(oak_undo::undocommand::UndoCommand::from_closures(
				move || block_set_length_and_media_out(&r1, l_len + delta),
				move || block_set_length_and_media_out(&r2, l_len),
			));
		}
	}
	// Right neighbor: its in follows the clip's new out while its content
	// end stays anchored (out anchored; the media in advances over the
	// consumed head); skipped when it would collapse to a negative length.
	if let Some(r) = right {
		let r_ref = node_ref(p, r);
		let r_len = block_length(&r_ref);
		if new + clip_len < block_out(&r_ref) {
			let (r1, r2) = (r_ref.clone(), r_ref);
			children.push(oak_undo::undocommand::UndoCommand::from_closures(
				move || block_set_length_keeping_out(&r1, r_len - delta),
				move || block_set_length_keeping_out(&r2, r_len),
			));
		}
	}
	push_multi(children, "Slide Clip")
}

/// Slip `clip` so its media in-point becomes `new_media_in` (undoable
/// "Slip Clip"): the timeline range and length stay put, only the media
/// window slides inside the clip (clamped to frame 0).
pub fn slip_clip(p: &ProjectRef, clip: NodeId, new_media_in: i64) -> Result<(), String> {
	let (tb, old_media_in) = {
		let g = lock(p);
		let tb = clip_track(&g.graph, clip)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (_, _, media_in) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		(tb, media_in)
	};
	let new = ts_to_rational(new_media_in.max(0), tb);
	if new == old_media_in {
		return Ok(());
	}
	let c_ref = node_ref(p, clip);
	let (r1, r2) = (c_ref.clone(), c_ref);
	push(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || clip_set_media_in(&r1, new),
			move || clip_set_media_in(&r2, old_media_in),
		),
		"Slip Clip",
	)
}

/// The undoable same-track move command for one clip (its in point becomes
/// `new_in_ts`; the module's `TrackMoveBlockCommand` — the old spot becomes
/// a gap, length and media-in are preserved).
pub(crate) fn move_clip_command(
	p: &ProjectRef,
	clip: NodeId,
	new_in_ts: i64,
) -> Result<oak_undo::undocommand::UndoCommand, String> {
	let (tb, list, track_index) = {
		let g = lock(p);
		let track =
			clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?;
		let list = track_behavior(&g.graph, track)
			.and_then(|t| t.track_list)
			.ok_or_else(|| "the clip's track has no list".to_string())?;
		let track_index = track_behavior(&g.graph, track)
			.map(|t| t.index)
			.unwrap_or(0);
		let tb = track_list_behavior(&g.graph, list)
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the sequence has no valid frame rate".to_string())?;
		(tb, list, track_index)
	};
	Ok(oak_timeline::undopointer::TrackMoveBlockCommand::new(
		node_ref(p, list),
		track_index,
		node_ref(p, clip),
		ts_to_rational(new_in_ts, tb),
	)
	.to_command())
}

/// Move `clip` within its track so its in point becomes `new_in_ts`
/// (undoable "Move Clip"; the module's `TrackMoveBlockCommand` — the old
/// spot becomes a gap, length and media-in are preserved). A negative
/// target clamps to frame 0 (the NLE drop-past-the-start behavior).
pub fn move_clip(p: &ProjectRef, clip: NodeId, new_in_ts: i64) -> Result<(), String> {
	push(move_clip_command(p, clip, new_in_ts.max(0))?, "Move Clip")
}

/// The undoable cross-track move commands for one clip (gap on the source
/// track, re-homed in point, place on the destination track), WITHOUT
/// pushing — assembled by callers that move several clips in one undoable
/// entry.
pub(crate) fn move_clip_to_track_commands(
	p: &ProjectRef,
	clip: NodeId,
	dest_track: NodeId,
	new_in_ts: i64,
) -> Result<Vec<oak_undo::undocommand::UndoCommand>, String> {
	let (tb, list, dest_index, source_track) = {
		let g = lock(p);
		let source_track =
			clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?;
		let list = track_behavior(&g.graph, dest_track)
			.and_then(|t| t.track_list)
			.ok_or_else(|| "the destination track has no list".to_string())?;
		let dest_index = track_behavior(&g.graph, dest_track)
			.map(|t| t.index)
			.unwrap_or(0);
		let tb = track_list_behavior(&g.graph, list)
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the sequence has no valid frame rate".to_string())?;
		(tb, list, dest_index, source_track)
	};
	let in_r = ts_to_rational(new_in_ts, tb);

	let gap = oak_timeline::undogeneral::TrackReplaceBlockWithGapCommand::new(
		node_ref(p, source_track),
		node_ref(p, clip),
		true,
	)
	.to_command();
	// The module stores a block's position on the block itself, so the
	// place command alone would keep the old in point: re-home it between
	// the gap and the place (the facade's BlockInCmdData step).
	let old_in = {
		let g = lock(p);
		clip_range(&g.graph, clip)
			.map(|(in_r, _, _)| in_r)
			.ok_or_else(|| "the node is not a clip".to_string())?
	};
	let (p1, p2) = (p.clone(), p.clone());
	let rehome = oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut guard = lock(&p1);
			with_block_core_mut(&mut guard.graph, clip, |core| core.set_in(in_r));
		},
		move || {
			let mut guard = lock(&p2);
			with_block_core_mut(&mut guard.graph, clip, |core| core.set_in(old_in));
		},
	);
	let place = oak_timeline::undopointer::TrackPlaceBlockCommand::new(
		node_ref(p, list),
		dest_index,
		node_ref(p, clip),
		in_r,
	)
	.to_command();
	Ok(vec![gap, rehome, place])
}

/// Move `clip` to a different track at `new_in_ts` (undoable "Move Clip
/// to Track", one row): the source spot becomes a gap, the block's in
/// point is re-homed, and the clip is placed on the destination track
/// (the facade's `oakengine_sequence_move_clip_to_track` composition).
pub fn move_clip_to_track(
	p: &ProjectRef,
	clip: NodeId,
	dest_track: NodeId,
	new_in_ts: i64,
) -> Result<(), String> {
	push_multi(
		move_clip_to_track_commands(p, clip, dest_track, new_in_ts.max(0))?,
		"Move Clip to Track",
	)
}

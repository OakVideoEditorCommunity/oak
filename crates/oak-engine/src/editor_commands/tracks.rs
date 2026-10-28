//! Track state, graph-edge commands and footage/adjustment clip placement.
//!
//! Track flag setters capture their previous values for undo, while height and
//! other explicitly documented setters are non-undoable. Clip-placement helpers
//! validate track kind, range and index before execution and return the new
//! block's stable node ID.

#![allow(unused_imports)]
use super::prelude::*;
use super::{editing::*, generators::*, media::*, moving::*, project::*, sequence::*, transitions::*};


/// An undoable edge remove: succeeds even when nothing is connected (the
/// redo is then a no-op, mirroring the C++ command's redo swallowing). The
/// undo re-connects the edge captured at construction — the module's
/// `oaknode_node_disconnect_undoable` did not model this (documented
/// deviation); the app retains the source node, so its effect-chain edits
/// undo faithfully.
pub fn disconnect_command(
	p: &ProjectRef,
	to: NodeId,
	input_id: &str,
) -> Result<oak_undo::undocommand::UndoCommand, String> {
	let source = {
		let g = lock(p);
		let has_input = g
			.graph
			.get(to)
			.map(|e| e.core.has_input(input_id))
			.unwrap_or(false);
		if !has_input {
			return Err(format!("disconnect: no input \"{input_id}\""));
		}
		g.graph.connected_output(to, input_id, -1)
	};
	let (p1, p2) = (p.clone(), p.clone());
	let (id1, id2) = (input_id.to_string(), input_id.to_string());
	Ok(oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut g = lock(&p1);
			g.graph.disconnect_input(to, &id1, -1);
		},
		move || {
			if let Some(from) = source {
				let mut g = lock(&p2);
				let _ = g.graph.connect(from, to, &id2, -1);
			}
		},
	))
}

/// An undoable context-position set (the facade's
/// `oakengine_node_set_context_position`): the first entry is established
/// by the redo itself; the undo restores the previous position or removes
/// the entry it created.
pub fn set_context_position_command(
	p: &ProjectRef,
	node: NodeId,
	context: NodeId,
	x: f64,
	y: f64,
) -> Result<oak_undo::undocommand::UndoCommand, String> {
	let old = {
		let g = lock(p);
		if !g.graph.is_valid(node) || !g.graph.is_valid(context) {
			return Err("set position: node not found".to_string());
		};
		g.graph.get(node).and_then(|e| {
			e.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == context)
				.map(|(_, pos, expanded)| (*pos, *expanded))
		})
	};
	let (p1, p2) = (p.clone(), p.clone());
	Ok(oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut g = lock(&p1);
			if let Some(e) = g.graph.get_mut(node) {
				e.core.set_context_position(context, x, y, false);
			}
		},
		move || {
			let mut g = lock(&p2);
			if let Some(e) = g.graph.get_mut(node) {
				match old {
					Some(((ox, oy), expanded)) => {
						e.core.set_context_position(context, ox, oy, expanded);
					}
					None => {
						e.core.remove_from_context(context);
					}
				}
			}
		},
	))
}

/// Add a track of `kind` to the sequence (undoable "Add Track"; the
/// module's `TimelineAddTrackCommand`), returning the new track's index.
pub fn add_track(p: &ProjectRef, seq: NodeId, kind: TrackType) -> Result<usize, String> {
	let list = find_or_create_track_list(p, seq, kind)
		.ok_or_else(|| "sequence has no track list for this type".to_string())?;
	let before: Vec<NodeId> = {
		let g = lock(p);
		track_list_behavior(&g.graph, list)
			.map(|l| l.tracks.clone())
			.unwrap_or_default()
	};
	push(
		oak_timeline::undogeneral::TimelineAddTrackCommand::new(node_ref(p, list)).to_command(),
		"Add Track",
	)?;
	let g = lock(p);
	let tracks = track_list_behavior(&g.graph, list)
		.map(|l| l.tracks.clone())
		.ok_or_else(|| "add track command produced no track list".to_string())?;
	tracks
		.iter()
		.position(|id| !before.contains(id))
		.ok_or_else(|| "add track command produced no track".to_string())
}

/// Remove `track` from its list (undoable "Remove Track"; the module's
/// `TimelineRemoveTrackCommand` redo performs the full list+graph
/// removal, so no live compensation is needed).
pub fn remove_track(p: &ProjectRef, track: NodeId) -> Result<(), String> {
	push(
		oak_timeline::undogeneral::TimelineRemoveTrackCommand::new(node_ref(p, track)).to_command(),
		"Remove Track",
	)
}

/// A track's height in internal units (`None` when the id is stale).
pub fn track_height(p: &ProjectRef, track: NodeId) -> Option<f64> {
	let g = lock(p);
	track_behavior(&g.graph, track).map(|t| t.height)
}

/// A track's muted flag (`None` when the id is stale). Muted means
/// "silenced" on audio tracks and "hidden" on video/subtitle tracks
/// (Olive parity: one flag drives both).
pub fn track_muted(p: &ProjectRef, track: NodeId) -> Option<bool> {
	let g = lock(p);
	track_behavior(&g.graph, track).map(|t| t.muted)
}

/// A track's locked flag (`None` when the id is stale).
pub fn track_locked(p: &ProjectRef, track: NodeId) -> Option<bool> {
	let g = lock(p);
	track_behavior(&g.graph, track).map(|t| t.locked)
}

/// An undoable track-flag set (the closures capture the previous value, so
/// the undo restores it exactly). Shared by the mute/hide and lock toggles.
pub(crate) fn set_track_flag(
	p: &ProjectRef,
	track: NodeId,
	field: TrackFlag,
	value: bool,
	name: &str,
) -> Result<(), String> {
	let old = {
		let g = lock(p);
		let t = track_behavior(&g.graph, track)
			.ok_or_else(|| "set track flag: the node is not a track".to_string())?;
		match field {
			TrackFlag::Muted => t.muted,
			TrackFlag::Locked => t.locked,
		}
	};
	if old == value {
		return Ok(());
	}
	let (p1, p2) = (p.clone(), p.clone());
	push(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				let mut g = lock(&p1);
				if let Some(t) = g
					.graph
					.get_mut(track)
					.and_then(|e| e.behavior.as_any_mut())
					.and_then(|a| a.downcast_mut::<TrackBehavior>())
				{
					match field {
						TrackFlag::Muted => t.muted = value,
						TrackFlag::Locked => t.locked = value,
					}
				}
			},
			move || {
				let mut g = lock(&p2);
				if let Some(t) = g
					.graph
					.get_mut(track)
					.and_then(|e| e.behavior.as_any_mut())
					.and_then(|a| a.downcast_mut::<TrackBehavior>())
				{
					match field {
						TrackFlag::Muted => t.muted = old,
						TrackFlag::Locked => t.locked = old,
					}
				}
			},
		),
		name,
	)
}

/// The track flags the undoable setters cover.
#[derive(Clone, Copy)]
pub(crate) enum TrackFlag {
	/// Muted (audio) / hidden (video, subtitle).
	Muted,
	/// Locked against clip edits.
	Locked,
}

/// Set a track's muted flag (undoable "Set Track Muted"). On video and
/// subtitle tracks this is the visibility (show/hide) toggle.
pub fn set_track_muted(p: &ProjectRef, track: NodeId, muted: bool) -> Result<(), String> {
	set_track_flag(p, track, TrackFlag::Muted, muted, "Set Track Muted")
}

/// Set a track's locked flag (undoable "Set Track Locked"). Locked tracks
/// reject clip edits (the app layer refuses trim/move/split/delete).
pub fn set_track_locked(p: &ProjectRef, track: NodeId, locked: bool) -> Result<(), String> {
	set_track_flag(p, track, TrackFlag::Locked, locked, "Set Track Locked")
}

/// Set a track's height in internal units (NOT undoable, mirroring the
/// facade's `oakengine_track_set_height`).
pub fn set_track_height(p: &ProjectRef, track: NodeId, height: f64) {
	if height <= 0.0 {
		return;
	}
	let mut g = lock(p);
	if let Some(t) = g
		.graph
		.get_mut(track)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<TrackBehavior>())
	{
		t.height = height;
	}
}

/// The stable per-clip color index, fixed when the clip is created: the
/// node identity is unique per creation and never shifts when clips are
/// added or removed elsewhere, so a clip keeps its color for its whole
/// life. The block core's `override_color` stores the resulting index.
pub(crate) fn clip_color_index(id: NodeId) -> i32 {
	(id.identity() % 8) as i32
}

/// The display name of `footage` for clip labels: its node label (the
/// imported basename), falling back to the file basename.
pub(crate) fn footage_display_name(g: &Graph, footage: NodeId) -> String {
	if let Some(e) = g.get(footage) {
		if !e.core.label.is_empty() {
			return e.core.label.clone();
		}
	}
	footage_behavior(g, footage)
		.and_then(|f| {
			Path::new(&f.filename)
				.file_name()
				.map(|n| n.to_string_lossy().into_owned())
		})
		.unwrap_or_default()
}

/// Create a footage clip block in `g`: the shared span/state setup plus
/// the per-clip color and footage-name label, both fixed at creation
/// time (C++ `oaknode_clip_set_media_in` +
/// `oaknode_block_set_length_and_media_out`; the place command writes the
/// in point).
pub(crate) fn create_footage_clip(
	g: &mut Graph,
	footage: NodeId,
	media_in: Rational,
	length: Rational,
) -> NodeId {
	let label = footage_display_name(g, footage);
	let (core, behavior) = oak_node::block::clip_create();
	let id = g.add_node(core, behavior);
	if let Some(entry) = g.get_mut(id) {
		entry.core.override_color = clip_color_index(id);
		entry.core.label = label;
		if let Some(c) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
		{
			c.core.media_in = media_in;
			c.core.set_length_and_media_out(length);
		}
	}
	id
}

/// Place a clip of `footage` on track `track_index` of the sequence's
/// `kind` list (undoable "Add Clip", one row): the clip block is created
/// in the project, placed by the module's `TrackPlaceBlockCommand`, and
/// the footage is wired to the clip's `tex_in` — the facade's
/// `oakengine_sequence_add_footage_clip_ex` composition, minus the
/// scratch-project dance (the app keeps everything in one project).
///
/// `in_ts`/`out_ts`/`media_in_ts` are frame timestamps in the sequence's
/// frame-rate timebase. On undo the block is detached from the track but
/// left as an orphan node in the project graph (the module command's
/// documented behavior).
pub fn place_footage_clip(
	p: &ProjectRef,
	seq: NodeId,
	footage: NodeId,
	kind: TrackType,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
	media_in_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts || media_in_ts < 0 {
		return Err("invalid clip range (need 0 <= in < out and media_in >= 0)".to_string());
	}
	if kind != TrackType::Video && kind != TrackType::Audio {
		return Err("clips are only supported on video and audio tracks".to_string());
	}
	let (tb, list) = {
		let g = lock(p);
		if footage_behavior(&g.graph, footage).is_none() {
			return Err("the footage node is not in the project".to_string());
		}
		let tb = sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?;
		let list = track_list_of(&g.graph, seq, kind)
			.ok_or_else(|| "sequence has no track list for this type".to_string())?;
		(tb, list)
	};
	let track_count = {
		let g = lock(p);
		track_list_behavior(&g.graph, list)
			.map(|l| l.tracks.len())
			.unwrap_or(0)
	};
	if track_index >= track_count {
		return Err(format!(
			"track index {track_index} out of range ({track_count} tracks)"
		));
	}

	let in_r = ts_to_rational(in_ts, tb);
	let out_r = ts_to_rational(out_ts, tb);
	let media_r = ts_to_rational(media_in_ts, tb);
	let length = out_r - in_r;

	// The clip block, positioned by media-in + length and stamped with
	// its creation-time color + footage-name label.
	let clip = {
		let mut g = lock(p);
		create_footage_clip(&mut g.graph, footage, media_r, length)
	};

	let place = oak_timeline::undopointer::TrackPlaceBlockCommand::new(
		node_ref(p, list),
		track_index as i32,
		node_ref(p, clip),
		in_r,
	)
	.to_command();
	let edge = connect_command(p, footage, clip, oak_node::block::clip_input::TEXTURE_INPUT)?;
	push_multi(vec![place, edge], "Add Clip")?;
	Ok(clip)
}

/// Add an adjustment layer to `track` spanning `[in_ts, out_ts)` (frame
/// timestamps in the sequence's timebase): an [`AdjustmentBlockBehavior`]
/// block with no footage, so it renders its effect chain over the clips
/// underneath it. Only video tracks take adjustment layers.
///
/// Undoable "Add Adjustment Layer" (the block is created in the project
/// and placed by the module's `TrackPlaceBlockCommand`); on undo the
/// block is detached from the track but left as an orphan node in the
/// project graph (the module command's documented behavior, same as
/// [`place_footage_clip`]).
pub fn create_adjustment_layer(
	p: &ProjectRef,
	seq: NodeId,
	track: NodeId,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid adjustment layer range (need 0 <= in < out)".to_string());
	}
	let (list, track_index, in_r, out_r) = {
		let g = lock(p);
		let behavior = track_behavior(&g.graph, track)
			.ok_or_else(|| "the track is not in the project".to_string())?;
		if behavior.kind != TrackType::Video {
			return Err("adjustment layers are only supported on video tracks".to_string());
		}
		let tb = sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?;
		let list = behavior
			.track_list
			.ok_or_else(|| "the track is not in a track list".to_string())?;
		let index = track_list_behavior(&g.graph, list)
			.and_then(|l| l.tracks.iter().position(|&t| t == track))
			.ok_or_else(|| "the track is not in its list".to_string())?;
		(
			list,
			index,
			ts_to_rational(in_ts, tb),
			ts_to_rational(out_ts, tb),
		)
	};

	// The block itself: no label, no color override, no media wiring —
	// just its timeline span. Its placement below is the undoable part.
	let block = {
		let mut g = lock(p);
		let (core, behavior) = oak_node::block::adjustment_create();
		let block = g.graph.add_node(core, behavior);
		with_block_core_mut(&mut g.graph, block, |core| {
			core.range = TimeRange::new(in_r, out_r);
		});
		block
	};

	push(
		oak_timeline::undopointer::TrackPlaceBlockCommand::new(
			node_ref(p, list),
			track_index as i32,
			node_ref(p, block),
			in_r,
		)
		.to_command(),
		"Add Adjustment Layer",
	)?;
	Ok(block)
}

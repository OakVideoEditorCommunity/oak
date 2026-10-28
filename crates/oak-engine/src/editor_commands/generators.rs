//! Clip creation from footage, nested sequences and generator nodes.
//!
//! This module also contains linked A/V placement, split operations and the
//! shared trim-command builder. Placement functions return stable node IDs and
//! document their undo/orphan behavior at the call site; frame timestamps are
//! interpreted in the host sequence's timebase.

#![allow(unused_imports)]
use super::prelude::*;
use super::{edit_menu::*, editing::*, media::*, moving::*, project::*, sequence::*, tracks::*, transitions::*};


/// The transition wedges of every clip on `track`, keyed by clip:
/// `(start, end)` are the widths of the transition before the clip (drawn
/// on its head) and after it (drawn on its tail); `None` when that side has
/// no transition. Gaps and transitions themselves are not keyed.
pub fn clip_transition_widths(
	g: &Graph,
	track: NodeId,
) -> std::collections::HashMap<NodeId, (Option<Rational>, Option<Rational>)> {
	let mut widths: std::collections::HashMap<NodeId, (Option<Rational>, Option<Rational>)> =
		std::collections::HashMap::new();
	let Some(blocks) = track_behavior(g, track).map(|t| t.blocks.clone()) else {
		return widths;
	};
	for &block in &blocks {
		if is_timeline_clip(g, block) {
			widths.insert(block, (None, None));
		}
	}
	for &block in &blocks {
		let Some(transition) = g
			.get(block)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<oak_node::block::TransitionBlockBehavior>())
		else {
			continue;
		};
		// A transition overlaps its neighbours: it reaches `in_offset` into
		// the block before it and `out_offset` into the block after it.
		if let Some(prev) =
			g.connected_output(block, oak_node::block::transition_input::OUT_BLOCK, -1)
		{
			if let Some(pair) = widths.get_mut(&prev) {
				pair.1 = Some(transition.in_offset);
			}
		}
		if let Some(next) =
			g.connected_output(block, oak_node::block::transition_input::IN_BLOCK, -1)
		{
			if let Some(pair) = widths.get_mut(&next) {
				pair.0 = Some(transition.out_offset);
			}
		}
	}
	widths
}

/// Places ONE video clip fed by a sequence node — a nested-sequence clip
/// (drag a sequence entry from the project explorer onto the timeline).
/// The clip reads the sequence's output during playback; the sequence's
/// own tracks stay in the source sequence (a frame, not a folder of
/// tracks, on the host timeline).
pub fn place_nested_sequence_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	source_seq: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid clip range (need 0 <= in < out)".to_string());
	}
	let (tb, list, source_behavior) = {
		let g = lock(p);
		sequence_behavior(&g.graph, source_seq)
			.ok_or_else(|| "the source sequence is not in the project".to_string())?;
		let tb = sequence_time_base(&g.graph, host_seq)
			.ok_or_else(|| "host sequence has no valid frame rate".to_string())?;
		let list = track_list_of(&g.graph, host_seq, TrackType::Video)
			.ok_or_else(|| "host sequence has no video track list".to_string())?;
		let label = format!("{}（序列）", node_label(&g.graph, source_seq));
		(tb, list, label)
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
	let length = out_r - in_r;

	let clip = {
		let mut g = lock(p);
		let (ccore, cbehavior) = oak_node::block::clip_create();
		let mut core = ccore;
		core.label = source_behavior;
		g.graph.add_node(core, cbehavior)
	};
	{
		let mut g = lock(p);
		let Some(clip_behavior) = g
			.graph
			.get_mut(clip)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<oak_node::block::ClipBlockBehavior>())
		else {
			return Err("clip node is not a clip".to_string());
		};
		clip_behavior.core.range = oak_core::TimeRange::new(in_r, length);
	}

	let place = oak_timeline::undopointer::TrackPlaceBlockCommand::new(
		node_ref(p, list),
		track_index as i32,
		node_ref(p, clip),
		in_r,
	)
	.to_command();
	let edge = connect_command(
		p,
		source_seq,
		clip,
		oak_node::block::clip_input::TEXTURE_INPUT,
	)?;
	push_multi(vec![place, edge], "Add Clip")?;
	Ok(clip)
}

/// Places ONE video clip fed by the text generator `text` (drag a text
/// entry from the project explorer onto the timeline) — a generator clip,
/// not a decode of media: the clip reads the generator's output through
/// `tex_in`, so the text re-renders at whatever time the clip spans.
/// Undoable as one "Add Clip". `in_ts`/`out_ts` are frame timestamps in
/// the host sequence's frame-rate timebase; a clip dragged shorter or
/// longer than the drop's default simply re-spans the same generator.
pub fn place_text_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	text: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	{
		let g = lock(p);
		if node_type_id(&g.graph, text) != TEXT_FOOTAGE_TYPE_ID {
			return Err("the text entry is not in the project".to_string());
		}
	}
	place_generator_clip(p, host_seq, text, track_index, in_ts, out_ts)
}

/// Place a clip fed by a GENERATOR node (checkerboard / color bars /
/// text / shape / solid…) at `(track_index, [in_ts, out_ts))` — the
/// effect-library drag-to-timeline counterpart of [`place_text_clip`]:
/// one undo row covers the placement and the generator→`tex_in` edge.
/// The generator must already be in the project; the clip is labelled
/// after it. Returns the clip's node id.
pub fn place_generator_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	generator: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid clip range (need 0 <= in < out)".to_string());
	}
	let (tb, list, label) = {
		let g = lock(p);
		if g.graph.get(generator).is_none() {
			return Err("the generator node is not in the project".to_string());
		}
		let tb = sequence_time_base(&g.graph, host_seq)
			.ok_or_else(|| "host sequence has no valid frame rate".to_string())?;
		let list = track_list_of(&g.graph, host_seq, TrackType::Video)
			.ok_or_else(|| "host sequence has no video track list".to_string())?;
		let label = node_label(&g.graph, generator);
		(tb, list, label)
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
	let length = out_r - in_r;

	// The clip block, labelled after its generator and stamped with its
	// creation-time color (the footage-clip convention). A generator has no
	// media, so only the length is written; `TrackPlaceBlockCommand` sets
	// the in-point from the drop.
	let clip = oak_timeline::util::block_clip_create(p).id;
	{
		let mut g = lock(p);
		let Some(entry) = g.graph.get_mut(clip) else {
			return Err("clip node is not in the project".to_string());
		};
		entry.core.override_color = clip_color_index(clip);
		entry.core.label = label;
		if let Some(c) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
		{
			c.core.set_length_and_media_out(length);
		}
	}

	let place = oak_timeline::undopointer::TrackPlaceBlockCommand::new(
		node_ref(p, list),
		track_index as i32,
		node_ref(p, clip),
		in_r,
	)
	.to_command();
	let edge = connect_command(
		p,
		generator,
		clip,
		oak_node::block::clip_input::TEXTURE_INPUT,
	)?;
	push_multi(vec![place, edge], "Add Clip")?;
	Ok(clip)
}

/// Place linked clips of one footage on several tracks in ONE undoable
/// "Add Clip" entry (the NLE A/V-drop: a video-with-audio file lands as
/// a video clip plus a linked audio clip). `placements` lists the
/// `(track kind, track index)` targets in order; every clip shares the
/// same timeline range and media-in, and all clips are linked both ways
/// (C++ `block_links_` semantics: grouped edits like split/ripple apply
/// to the whole group).
pub fn place_footage_clips_linked(
	p: &ProjectRef,
	seq: NodeId,
	footage: NodeId,
	placements: &[(TrackType, usize)],
	in_ts: i64,
	out_ts: i64,
	media_in_ts: i64,
) -> Result<Vec<NodeId>, String> {
	if placements.len() < 2 {
		return Err("linked placement needs at least two tracks".to_string());
	}
	let tb = {
		let g = lock(p);
		if footage_behavior(&g.graph, footage).is_none() {
			return Err("the footage node is not in the project".to_string());
		}
		sequence_time_base(&g.graph, seq)
			.ok_or_else(|| "sequence has no valid frame rate".to_string())?
	};
	let in_r = ts_to_rational(in_ts, tb);
	let out_r = ts_to_rational(out_ts, tb);
	let media_r = ts_to_rational(media_in_ts, tb);
	let length = out_r - in_r;

	// Create all clips first (graph writes are not undoable; the placement
	// commands below are). Each gets its creation-time color + footage
	// name label.
	let mut clips = Vec::with_capacity(placements.len());
	for _ in placements {
		let mut g = lock(p);
		let id = create_footage_clip(&mut g.graph, footage, media_r, length);
		clips.push(id);
	}

	let mut commands = Vec::new();
	for (&(kind, track_index), &clip) in placements.iter().zip(&clips) {
		let list = {
			let g = lock(p);
			track_list_of(&g.graph, seq, kind)
				.ok_or_else(|| "sequence has no track list for this type".to_string())?
		};
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
			footage,
			clip,
			oak_node::block::clip_input::TEXTURE_INPUT,
		)?);
	}
	// Link the group both ways. Each ordered pair is its own command whose
	// undo removes ONLY the direction it added (incremental undo), so a
	// link created after the drop survives the drop's undo.
	for (i, &a) in clips.iter().enumerate() {
		for (j, &b) in clips.iter().enumerate() {
			if i == j {
				continue;
			}
			let (p1, p2) = (p.clone(), p.clone());
			commands.push(oak_undo::undocommand::UndoCommand::from_closures(
				move || {
					let mut g = lock(&p1);
					if let Some(entry) = g.graph.get_mut(a) {
						let links = &mut entry.core.links;
						if !links.contains(&b) {
							links.push(b);
						}
					}
				},
				move || {
					let mut g = lock(&p2);
					if let Some(entry) = g.graph.get_mut(a) {
						entry.core.links.retain(|&l| l != b);
					}
				},
			));
		}
	}
	push_multi(commands, "Add Clip")?;
	Ok(clips)
}

/// Split `clip` at `time_ts` (a frame timestamp strictly inside the
/// clip's range), undoable "Split Clip" (the module's
/// `BlockSplitCommand`).
pub fn split_clip(p: &ProjectRef, clip: NodeId, time_ts: i64) -> Result<(), String> {
	let (tb, in_r, out_r) = {
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
		(tb, in_r, out_r)
	};
	let point = ts_to_rational(time_ts, tb);
	if point <= in_r || point >= out_r {
		return Err(format!(
			"split time {time_ts} is not strictly inside the clip"
		));
	}
	push(
		oak_timeline::undosplit::BlockSplitCommand::new(node_ref(p, clip), point).to_command(),
		"Split Clip",
	)
}

/// Split every clip of `blocks` at `time_ts` as ONE undoable
/// `BlockSplitPreservingLinksCommand` (the Cmd+K path): originally linked
/// pairs that are both split get their rear halves linked too — the
/// per-block plain split left the rear half of an A/V pair unlinked.
/// Blocks whose range does not strictly contain the time are skipped by
/// the command itself.
pub fn split_clips_preserving_links(
	p: &ProjectRef,
	blocks: &[NodeId],
	time_ts: i64,
) -> Result<(), String> {
	let Some((&first, rest)) = blocks.split_first() else {
		return Ok(());
	};
	let tb = {
		let g = lock(p);
		clip_track(&g.graph, first)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?
	};
	let point = ts_to_rational(time_ts, tb);
	let mut refs = Vec::with_capacity(blocks.len());
	refs.push(node_ref(p, first));
	refs.extend(rest.iter().map(|&b| node_ref(p, b)));
	let times = vec![point; refs.len()];
	push(
		oak_timeline::undosplit::BlockSplitPreservingLinksCommand::new(refs, times).to_command(),
		"Split Clips",
	)
}

/// An undoable length change for `clip` (trim semantics: `out_anchored`
/// keeps the out point — trim-in — and shifts the in; otherwise the in
/// stays — trim-out). Shared by [`trim_clip`] and [`ripple_trim_clip`];
/// the closure applies the new length on redo and restores `old` on undo.
pub(crate) fn trim_command(
	p: &ProjectRef,
	clip: NodeId,
	out_anchored: bool,
	old: Rational,
	new: Rational,
) -> oak_undo::undocommand::UndoCommand {
	let (p1, p2) = (p.clone(), p.clone());
	oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut guard = lock(&p1);
			with_block_core_mut(&mut guard.graph, clip, |core| {
				if out_anchored {
					core.set_length_keeping_out(new);
				} else {
					core.set_length_and_media_out(new);
				}
			});
		},
		move || {
			let mut guard = lock(&p2);
			with_block_core_mut(&mut guard.graph, clip, |core| {
				if out_anchored {
					core.set_length_keeping_out(old);
				} else {
					core.set_length_and_media_out(old);
				}
			});
		},
	)
}

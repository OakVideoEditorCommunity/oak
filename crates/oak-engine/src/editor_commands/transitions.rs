//! Transition creation, default-transition batching and duration adjustment.
//!
//! Command-building helpers validate seam geometry and return commands without
//! executing them; public wrappers push the corresponding undo entries. A
//! transition's `half` duration extends to each side of a seam, and existing
//! transitions are detected to keep batch operations idempotent.

#![allow(unused_imports)]
use super::prelude::*;
use super::{editing::*, generators::*, media::*, moving::*, project::*, sequence::*, tracks::*};


/// The transition block joining `prev` to `next` on `track`, when one is
/// already wired between them.
pub(crate) fn transition_between(g: &Graph, track: NodeId, prev: NodeId, next: NodeId) -> Option<NodeId> {
	track_behavior(g, track)
		.map(|t| t.blocks.clone())
		.unwrap_or_default()
		.into_iter()
		.find(|&b| {
			g.get(b)
				.and_then(|e| e.behavior.as_any())
				.map(|a| a.is::<oak_node::block::TransitionBlockBehavior>())
				.unwrap_or(false)
				&& g.connected_output(b, oak_node::block::transition_input::OUT_BLOCK, -1)
					== Some(prev)
				&& g.connected_output(b, oak_node::block::transition_input::IN_BLOCK, -1)
					== Some(next)
		})
}

/// Build (without pushing) the commands that create a transition block at
/// the seam between `prev` and `next` and wire both sides into it.
///
/// The block covers `[out(prev) - half, out(prev) + half]`: `in_offset`
/// reaches into `prev` and `out_offset` into `next` (the module's
/// transition geometry), both `half` for a symmetric default transition.
/// `prev` and `next` must be clip blocks listed back to back on one track
/// and must touch exactly. Everything is validated before the block node
/// is created, so a rejected seam leaves the graph untouched.
///
/// The block enters the track through the module's non-destructive
/// `TrackInsertBlockAfterCommand`; `TrackPlaceBlockCommand` is unusable
/// here because its redo ripple-removes the span the new block covers,
/// which would trim away both clips the transition straddles.
pub(crate) fn transition_commands(
	p: &ProjectRef,
	prev: NodeId,
	next: NodeId,
	half: Rational,
) -> Result<(NodeId, Vec<oak_undo::undocommand::UndoCommand>), String> {
	if half <= Rational::new(0, 1) {
		return Err("transition: the length must be positive".to_string());
	}
	let (seam, owner) = {
		let g = lock(p);
		if clip_behavior(&g.graph, prev).is_none() {
			return Err("transition: the outgoing block is not a clip".to_string());
		}
		if clip_behavior(&g.graph, next).is_none() {
			return Err("transition: the incoming block is not a clip".to_string());
		}
		let Some(prev_core) = block_core_of(&g.graph, prev) else {
			return Err("transition: the outgoing clip has no block core".to_string());
		};
		let Some(next_core) = block_core_of(&g.graph, next) else {
			return Err("transition: the incoming clip has no block core".to_string());
		};
		if next_core.in_() != prev_core.out() {
			return Err("transition: the clips are not contiguous at the seam".to_string());
		}
		let Some(track) = prev_core.track else {
			return Err("transition: the outgoing clip is not on a track".to_string());
		};
		if next_core.track != Some(track) {
			return Err("transition: the clips are not on the same track".to_string());
		}
		let Some(blocks) = track_behavior(&g.graph, track).map(|t| t.blocks.clone()) else {
			return Err("transition: the track is not in the project".to_string());
		};
		let adjacent = blocks
			.iter()
			.position(|&b| b == prev)
			.and_then(|i| blocks.get(i + 1).copied())
			== Some(next);
		if !adjacent {
			return Err("transition: the clips are not adjacent on the track".to_string());
		}
		if transition_between(&g.graph, track, prev, next).is_some() {
			return Err("transition: the seam already carries a transition".to_string());
		}
		(prev_core.out(), track)
	};

	let block = {
		let mut g = lock(p);
		let (core, behavior) = oak_node::block::transition_create();
		let id = g.graph.add_node(core, behavior);
		let Some(t) = g
			.graph
			.get_mut(id)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<oak_node::block::TransitionBlockBehavior>())
		else {
			return Err("transition: could not create the block".to_string());
		};
		t.core.range = TimeRange::new(seam - half, seam + half);
		t.in_offset = half;
		t.out_offset = half;
		id
	};
	let insert = oak_timeline::undotrack::TrackInsertBlockAfterCommand::new(
		node_ref(p, owner),
		node_ref(p, block),
		Some(node_ref(p, prev)),
	)
	.to_command();
	let from = connect_command(p, prev, block, oak_node::block::transition_input::OUT_BLOCK)?;
	let to = connect_command(p, next, block, oak_node::block::transition_input::IN_BLOCK)?;
	Ok((block, vec![insert, from, to]))
}

/// Create a transition at the seam between two contiguous clips and push
/// it as one "Add Transition" undo row (the timeline's wedge drag path).
pub fn add_transition_at_seam(
	p: &ProjectRef,
	prev: NodeId,
	next: NodeId,
	half: Rational,
) -> Result<NodeId, String> {
	let (block, commands) = transition_commands(p, prev, next, half)?;
	push_multi(commands, "Add Transition")?;
	Ok(block)
}

/// The undo commands of [`add_transition_at_edge`], unpushed (the batch
/// paths build their own combined rows).
pub(crate) fn edge_transition_commands(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	length: Rational,
) -> Result<(NodeId, Vec<oak_undo::undocommand::UndoCommand>), String> {
	if length <= Rational::new(0, 1) {
		return Err("transition: the length must be positive".to_string());
	}
	let (track, anchor, edge_time) = {
		let g = lock(p);
		if clip_behavior(&g.graph, clip).is_none() {
			return Err("transition: the block is not a clip".to_string());
		}
		let Some(core) = block_core_of(&g.graph, clip) else {
			return Err("transition: the clip has no block core".to_string());
		};
		let Some(track) = core.track else {
			return Err("transition: the clip is not on a track".to_string());
		};
		let Some(blocks) = track_behavior(&g.graph, track).map(|t| t.blocks.clone()) else {
			return Err("transition: the track is not in the project".to_string());
		};
		let Some(index) = blocks.iter().position(|&b| b == clip) else {
			return Err("transition: the clip is not on the track".to_string());
		};
		if transition_of_clip(&g.graph, clip, start_edge).is_some() {
			return Err("transition: the edge already carries a transition".to_string());
		}
		// The head transition inserts before the clip (after whatever block
		// precedes it); the tail inserts after the clip.
		let anchor = if start_edge {
			index.checked_sub(1).and_then(|i| blocks.get(i)).copied()
		} else {
			Some(clip)
		};
		let edge_time = if start_edge { core.in_() } else { core.out() };
		(track, anchor, edge_time)
	};
	let block = {
		let mut g = lock(p);
		let (core, behavior) = oak_node::block::transition_create();
		let id = g.graph.add_node(core, behavior);
		let Some(t) = g
			.graph
			.get_mut(id)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<oak_node::block::TransitionBlockBehavior>())
		else {
			return Err("transition: could not create the block".to_string());
		};
		if start_edge {
			// Head: no outgoing side — the whole span eats into the clip.
			t.core.range = TimeRange::new(edge_time, edge_time + length);
			t.in_offset = Rational::new(0, 1);
			t.out_offset = length;
		} else {
			t.core.range = TimeRange::new(edge_time - length, edge_time);
			t.in_offset = length;
			t.out_offset = Rational::new(0, 1);
		}
		id
	};
	let input = if start_edge {
		oak_node::block::transition_input::IN_BLOCK
	} else {
		oak_node::block::transition_input::OUT_BLOCK
	};
	let insert = oak_timeline::undotrack::TrackInsertBlockAfterCommand::new(
		node_ref(p, track),
		node_ref(p, block),
		anchor.map(|b| node_ref(p, b)),
	)
	.to_command();
	let edge = connect_command(p, clip, block, input)?;
	Ok((block, vec![insert, edge]))
}

/// Create a SINGLE-SIDED transition on one edge of `clip` (PR-style: a
/// head fade-in when nothing precedes the clip, a tail fade-out when
/// nothing follows it) and push it as one "Add Transition" undo row.
/// Only the clip itself is wired (`in_block_in` for the head,
/// `out_block_in` for the tail); the renderer blends the open side
/// against black. An edge that already carries a transition is an error.
pub fn add_transition_at_edge(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	length: Rational,
) -> Result<NodeId, String> {
	let (block, commands) = edge_transition_commands(p, clip, start_edge, length)?;
	push_multi(commands, "Add Transition")?;
	Ok(block)
}

/// Add a default transition at every contiguous seam around the selected
/// clips, plus BOTH ends of any clip with no junction at all (the
/// Ctrl+Shift+D action / the timeline clip-menu item — PR's
/// apply-to-edit-points semantics, extended to lone clips).
///
/// `half` is half of the default transition length: a junction transition
/// spans `half` on either side of its seam, a head/tail (single-sided)
/// transition spans `2*half` into its own clip. Every selected clip
/// contributes the seam before it and the seam after it (a neighbor
/// counts only when it is a clip block that touches the clip exactly,
/// shared seams built once), plus a head transition when nothing
/// precedes it and a tail transition when nothing follows it. Seams and
/// edges that cannot take a transition (an existing one) are skipped.
/// The whole batch is one "Add Transition" undo row; a batch that builds
/// nothing is an error, so the action never leaves an empty row. Returns
/// the number of transitions created.
///
/// `seq` scopes the action: a selected block whose track list belongs to
/// another sequence is ignored.
pub fn add_default_transition(
	p: &ProjectRef,
	seq: NodeId,
	clip_blocks: &[NodeId],
	half: Rational,
) -> Result<usize, String> {
	let lists = {
		let g = lock(p);
		let Some(s) = sequence_behavior(&g.graph, seq) else {
			return Err("transition: the sequence is not in the project".to_string());
		};
		s.track_lists.clone()
	};

	let mut seams: Vec<(NodeId, NodeId)> = Vec::new();
	// Standalone ends, applied ONLY when the clip has no junction at all
	// (PR's both-ends-on-a-lone-clip default): `(clip, start_edge)`.
	let mut edges: Vec<(NodeId, bool)> = Vec::new();
	for &clip in clip_blocks {
		let (candidates, touches_prev, touches_next) = {
			let g = lock(p);
			let Some(track) = clip_track(&g.graph, clip) else {
				continue;
			};
			let Some(list) = track_behavior(&g.graph, track).and_then(|t| t.track_list) else {
				continue;
			};
			if !lists.contains(&list) {
				continue;
			}
			let Some(blocks) = track_behavior(&g.graph, track).map(|t| t.blocks.clone()) else {
				continue;
			};
			let Some(index) = blocks.iter().position(|&b| b == clip) else {
				continue;
			};
			if clip_behavior(&g.graph, clip).is_none() {
				continue;
			}
			let range = block_core_of(&g.graph, clip).map(|c| (c.in_(), c.out()));
			let mut sides = Vec::new();
			let prev = index.checked_sub(1).and_then(|i| blocks.get(i)).copied();
			if let Some(prev) = prev {
				if clip_behavior(&g.graph, prev).is_some()
					&& block_core_of(&g.graph, prev).map(|c| c.out()) == range.map(|r| r.0)
				{
					sides.push((prev, clip));
				}
			}
			let next = blocks.get(index + 1).copied();
			if let Some(next) = next {
				if clip_behavior(&g.graph, next).is_some()
					&& block_core_of(&g.graph, next).map(|c| c.in_()) == range.map(|r| r.1)
				{
					sides.push((clip, next));
				}
			}
			let (touches_prev, touches_next) = (
				sides.iter().any(|(_, next)| *next == clip),
				sides.iter().any(|(prev, _)| *prev == clip),
			);
			(sides, touches_prev, touches_next)
		};
		let seam_count = candidates.len();
		for seam in candidates {
			if !seams.contains(&seam) {
				seams.push(seam);
			}
		}
		// A clip with no junction at all and no transition wired to it
		// takes the free ends (a leading gap is "nothing" — a head
		// fade-in still applies); a clip with any junction or an existing
		// transition keeps just what it has (the action is idempotent: a
		// second run builds nothing).
		if seam_count == 0
			&& transition_of_clip(&lock(p).graph, clip, true).is_none()
			&& transition_of_clip(&lock(p).graph, clip, false).is_none()
		{
			if !touches_prev {
				edges.push((clip, true));
			}
			if !touches_next {
				edges.push((clip, false));
			}
		}
	}

	// One undo row covers the junctions AND the standalone ends; a batch
	// that builds nothing is an error, so the action never leaves an empty
	// row.
	let mut commands = Vec::new();
	let mut created = 0usize;
	for (prev, next) in seams {
		if let Ok((_, children)) = transition_commands(p, prev, next, half) {
			commands.extend(children);
			created += 1;
		}
	}
	let edge_length = half + half;
	for (clip, start_edge) in edges {
		if let Ok((_, children)) = edge_transition_commands(p, clip, start_edge, edge_length) {
			commands.extend(children);
			created += 1;
		}
	}
	if created == 0 {
		return Err("transition: no seam or clip edge accepts a transition".to_string());
	}
	push_multi(commands, "Add Transition")?;
	Ok(created)
}

/// The transition block drawn on one edge of `clip`, when its neighbour on
/// that side is a transition wired into the clip.
///
/// The timeline widget addresses a wedge through the clip it is drawn on:
/// the start wedge belongs to the transition *before* the clip (the clip
/// feeds its `in_block_in`) and the end wedge to the one *after* it
/// (`out_block_in`).
pub fn transition_of_clip(g: &Graph, clip: NodeId, start_edge: bool) -> Option<NodeId> {
	let track = clip_track(g, clip)?;
	let blocks = track_behavior(g, track)?.blocks.clone();
	let index = blocks.iter().position(|&b| b == clip)?;
	let candidate = if start_edge {
		blocks.get(index.checked_sub(1)?).copied()?
	} else {
		blocks.get(index + 1).copied()?
	};
	let input = if start_edge {
		oak_node::block::transition_input::IN_BLOCK
	} else {
		oak_node::block::transition_input::OUT_BLOCK
	};
	if g.connected_output(candidate, input, -1) == Some(clip) {
		Some(candidate)
	} else {
		None
	}
}

/// Resize one wedge of the transition attached to `clip` (the timeline
/// widget's wedge drag, its `TransitionChanged` event).
///
/// `start_edge` names the wedge by the clip it is drawn on: the start wedge
/// is the incoming transition's `out_offset`, the end wedge the outgoing
/// one's `in_offset`. The width is clamped to `frame..=clip length` — one
/// frame at the smallest, and never wider than the clip it eats into — and
/// written through [`oak_timeline::undogeneral::TransitionSetOffsetsCommand`],
/// so one "Transition Length" undo row covers the drag. The seam does not
/// move. Returns the width actually written.
pub fn set_transition_length(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	frame: Rational,
	new_length: Rational,
) -> Result<Rational, String> {
	let (transition, clip_length) = {
		let g = lock(p);
		let Some(transition) = transition_of_clip(&g.graph, clip, start_edge) else {
			return Err("transition: the clip has no transition on that edge".to_string());
		};
		let Some(core) = block_core_of(&g.graph, clip) else {
			return Err("transition: the clip has no block core".to_string());
		};
		(transition, core.length())
	};
	let clamped = new_length.max(frame).min(clip_length);
	push_command(
		oak_timeline::undogeneral::TransitionSetOffsetsCommand::new(
			node_ref(p, transition),
			if start_edge { None } else { Some(clamped) },
			if start_edge { Some(clamped) } else { None },
		)
		.to_command(),
		"Transition Length",
	)?;
	Ok(clamped)
}

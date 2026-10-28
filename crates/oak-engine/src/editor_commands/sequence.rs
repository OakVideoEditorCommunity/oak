//! Sequence, track and block queries plus timeline timebase conversion.
//!
//! Rational values represent timeline/media time, while integer timestamps are
//! frame units derived with `sequence_time_base`, `rational_to_ts` and
//! `ts_to_rational`. Fallible single-node lookups use `Option` for stale or
//! incompatible nodes; mutation and legacy-footage reprobing document their
//! individual side effects.

#![allow(unused_imports)]
use super::prelude::*;
use super::{edit_menu::*, editing::*, generators::*, media::*, moving::*, project::*, tracks::*, transitions::*};


/// The project's multicam nodes (the wizard/sequence drop rewire uses the
/// one whose source sequence matches).
pub fn multicam_nodes(g: &Graph) -> Vec<NodeId> {
	g.node_ids()
		.into_iter()
		.filter(|&id| {
			g.get(id)
				.map(|e| e.behavior.type_id() == "org.olivevideoeditor.Olive.multicam")
				.unwrap_or(false)
		})
		.collect()
}

/// The sequence's video format `(width, height, rate)` from its first
/// video stream.
pub fn sequence_video_params(g: &Graph, seq: NodeId) -> Option<(i32, i32, Rational)> {
	let v = sequence_behavior(g, seq)?.video_params.first()?;
	Some((v.width, v.height, v.frame_rate))
}

/// The sequence's frame duration as a `(num, den)` timebase pair (the
/// frame rate flipped; the facade's `seq_time_base`). `None` when the
/// sequence has no valid frame rate.
pub fn sequence_time_base(g: &Graph, seq: NodeId) -> Option<(i64, i64)> {
	let (_, _, rate) = sequence_video_params(g, seq)?;
	let num = rate.numerator();
	let den = rate.denominator();
	if num <= 0 || den <= 0 {
		return None;
	}
	Some((den, num))
}

/// The sequence content length (rational seconds): the longest track out
/// point across every track list (the module's `verify_length` overall).
pub fn sequence_length(g: &Graph, seq: NodeId) -> Rational {
	let mut best = Rational::new(0, 1);
	let Some(s) = sequence_behavior(g, seq) else {
		return best;
	};
	for &list_id in &s.track_lists {
		let Some(list) = track_list_behavior(g, list_id) else {
			continue;
		};
		for &track_id in &list.tracks {
			let Some(track) = track_behavior(g, track_id) else {
				continue;
			};
			for &block_id in &track.blocks {
				let Some(entry) = g.get(block_id) else {
					continue;
				};
				let Some(core) = block_core(entry) else {
					continue;
				};
				let out = core.out();
				if out > best {
					best = out;
				}
			}
		}
	}
	best
}

/// Borrow a block's core (any block kind).
pub(crate) fn block_core(entry: &oak_node::graph::NodeEntry) -> Option<&BlockCore> {
	let any = entry.behavior.as_any()?;
	if let Some(c) = any.downcast_ref::<ClipBlockBehavior>() {
		return Some(&c.core);
	}
	if let Some(g) = any.downcast_ref::<oak_node::block::GapBlockBehavior>() {
		return Some(&g.core);
	}
	if let Some(t) = any.downcast_ref::<oak_node::block::TransitionBlockBehavior>() {
		return Some(&t.core);
	}
	any.downcast_ref::<AdjustmentBlockBehavior>()
		.map(|a| &a.core)
}

/// Borrow a block's core mutably (any block kind).
pub(crate) fn block_core_mut(entry: &mut oak_node::graph::NodeEntry) -> Option<&mut BlockCore> {
	let any = entry.behavior.as_any_mut()?;
	if any.is::<ClipBlockBehavior>() {
		Some(
			&mut any
				.downcast_mut::<ClipBlockBehavior>()
				.expect("checked above")
				.core,
		)
	} else if any.is::<oak_node::block::GapBlockBehavior>() {
		Some(
			&mut any
				.downcast_mut::<oak_node::block::GapBlockBehavior>()
				.expect("checked above")
				.core,
		)
	} else if any.is::<oak_node::block::TransitionBlockBehavior>() {
		Some(
			&mut any
				.downcast_mut::<oak_node::block::TransitionBlockBehavior>()
				.expect("checked above")
				.core,
		)
	} else if any.is::<AdjustmentBlockBehavior>() {
		Some(
			&mut any
				.downcast_mut::<AdjustmentBlockBehavior>()
				.expect("checked above")
				.core,
		)
	} else {
		None
	}
}

/// Borrow the block core at `node` (any block kind).
pub fn block_core_of(g: &Graph, node: NodeId) -> Option<&BlockCore> {
	block_core(g.get(node)?)
}

/// Mutate the block core at `node` (any block kind). Returns `false` and
/// leaves the graph untouched when `node` is not a block.
pub(crate) fn with_block_core_mut(g: &mut Graph, node: NodeId, f: impl FnOnce(&mut BlockCore)) -> bool {
	let Some(entry) = g.get_mut(node) else {
		return false;
	};
	let Some(core) = block_core_mut(entry) else {
		return false;
	};
	f(core);
	true
}

/// Whether `node` is a clip-class block: a clip or an adjustment layer.
/// Gaps and transitions are blocks too, but not clips.
pub(crate) fn is_timeline_clip(g: &Graph, node: NodeId) -> bool {
	let Some(entry) = g.get(node) else {
		return false;
	};
	let Some(any) = entry.behavior.as_any() else {
		return false;
	};
	any.is::<ClipBlockBehavior>() || any.is::<AdjustmentBlockBehavior>()
}

/// The sequence playhead (rational seconds).
pub fn sequence_playhead(g: &Graph, seq: NodeId) -> Rational {
	sequence_behavior(g, seq)
		.map(|s| s.playhead)
		.unwrap_or(Rational::new(0, 1))
}

/// Move the sequence playhead (rational seconds).
pub fn sequence_set_playhead(p: &ProjectRef, seq: NodeId, time: Rational) {
	let mut guard = lock(p);
	if let Some(s) = guard
		.graph
		.get_mut(seq)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<SequenceBehavior>())
	{
		s.playhead = time;
	}
}

// ---------------------------------------------------------------------------
// Frame timestamps <-> rational seconds
// ---------------------------------------------------------------------------

/// Greatest common divisor (1 when both are zero).
pub(crate) fn gcd(a: i64, b: i64) -> i64 {
	let (mut a, mut b) = (a.abs(), b.abs());
	while b != 0 {
		let t = b;
		b = a % b;
		a = t;
	}
	if a == 0 {
		1
	} else {
		a
	}
}

/// Rational seconds -> timestamp in timebase units, rounding half away
/// from zero (the facade's `rational_to_ts`, `Timecode::k_round`).
pub fn rational_to_ts(r: Rational, tb: (i64, i64)) -> i64 {
	let (num, den) = (r.numerator(), r.denominator());
	if den == 0 || tb.0 == 0 || tb.1 == 0 {
		return 0;
	}
	let n = num as i128 * tb.1 as i128;
	let d = den as i128 * tb.0 as i128;
	let q = n / d;
	let r = n % d;
	let rr = if r < 0 { -r } else { r };
	let dd = if d < 0 { -d } else { d };
	if rr * 2 >= dd {
		(q + if n < 0 { -1 } else { 1 }) as i64
	} else {
		q as i64
	}
}

/// Timestamp -> reduced rational seconds (`time = ts * tb`).
pub fn ts_to_rational(ts: i64, tb: (i64, i64)) -> Rational {
	let num = ts as i128 * tb.0 as i128;
	let den = tb.1 as i128;
	let g = gcd((num % den) as i64, den as i64) as i128;
	Rational::new((num / g) as i64, (den / g) as i64)
}

// ---------------------------------------------------------------------------
// Tracks and clips
// ---------------------------------------------------------------------------

/// The sequence's track list of `kind`, when it exists.
pub fn track_list_of(g: &Graph, seq: NodeId, kind: TrackType) -> Option<NodeId> {
	sequence_behavior(g, seq)?
		.track_lists
		.iter()
		.find(|&&list_id| track_list_behavior(g, list_id).map(|l| l.kind) == Some(kind))
		.copied()
}

/// Find (or create) the sequence's track list of `kind` (the facade's
/// `oaknode_sequence_get_track_list` find-or-create semantics; mirrors the
/// CLI's `find_or_create_track_list`).
pub fn find_or_create_track_list(p: &ProjectRef, seq: NodeId, kind: TrackType) -> Option<NodeId> {
	let mut guard = lock(p);
	if let Some(list) = track_list_of(&guard.graph, seq, kind) {
		return Some(list);
	}
	// Create it: a graph node owned by the sequence.
	let (core, behavior) = TrackListBehavior::create();
	let mut behavior = behavior;
	if let Some(a) = behavior.as_any_mut() {
		if let Some(list) = a.downcast_mut::<TrackListBehavior>() {
			list.kind = kind;
			list.array_base = sequence_behavior(&guard.graph, seq)?.track_lists.len() as i32;
		}
	}
	let list_id = guard.graph.add_node(core, behavior);
	if let Some(s) = guard
		.graph
		.get_mut(seq)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<SequenceBehavior>())
	{
		s.track_lists.push(list_id);
	}
	if let Some(l) = guard
		.graph
		.get_mut(list_id)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<TrackListBehavior>())
	{
		l.sequence = Some(seq);
	}
	Some(list_id)
}

/// The tracks of the sequence's `kind` list, in stack order.
pub fn track_ids(g: &Graph, seq: NodeId, kind: TrackType) -> Vec<NodeId> {
	track_list_of(g, seq, kind)
		.and_then(|list| track_list_behavior(g, list).map(|l| l.tracks.clone()))
		.unwrap_or_default()
}

/// The clip blocks of a track, in timeline order. Gaps and transitions are
/// not clips and are skipped; adjustment layers are.
pub fn clip_ids(g: &Graph, track: NodeId) -> Vec<NodeId> {
	track_behavior(g, track)
		.map(|t| {
			t.blocks
				.iter()
				.copied()
				.filter(|&b| is_timeline_clip(g, b))
				.collect()
		})
		.unwrap_or_default()
}

/// A block's timeline range and media in-point (rational seconds).
pub fn clip_range(g: &Graph, clip: NodeId) -> Option<(Rational, Rational, Rational)> {
	let c = block_core_of(g, clip)?;
	Some((c.in_(), c.out(), c.media_in))
}

/// The block's owning track.
pub fn clip_track(g: &Graph, clip: NodeId) -> Option<NodeId> {
	block_core_of(g, clip)?.track
}

/// The first footage node feeding `id` (upstream BFS over input edges),
/// mirroring the facade's `oaknode_node_find_input_footage`.
pub fn find_input_footage(g: &Graph, id: NodeId) -> Option<NodeId> {
	let mut frontier = vec![id];
	let mut visited: Vec<NodeId> = Vec::new();
	while !frontier.is_empty() {
		let mut next = Vec::new();
		for cur in frontier {
			if visited.contains(&cur) {
				continue;
			}
			visited.push(cur);
			let entry = g.get(cur)?;
			if entry.behavior.type_id() == "org.olivevideoeditor.Olive.footage" && cur != id {
				return Some(cur);
			}
			for (src, _, _) in g.input_connections(cur) {
				next.push(src);
			}
		}
		frontier = next;
	}
	None
}

/// The media filename feeding a clip (upstream BFS + footage behavior).
pub fn clip_media_filename(g: &Graph, clip: NodeId) -> Option<String> {
	let footage = find_input_footage(g, clip)?;
	Some(footage_behavior(g, footage)?.filename.clone())
}

// ---------------------------------------------------------------------------
// Footage
// ---------------------------------------------------------------------------

/// The footage's probed duration in seconds (`None` when unprobed).
pub fn footage_duration_seconds(g: &Graph, id: NodeId) -> Option<f64> {
	let d = footage_behavior(g, id)?.duration();
	let den = d.denominator();
	if den == 0 {
		return None;
	}
	let seconds = d.numerator() as f64 / den as f64;
	(seconds > 0.0).then_some(seconds)
}

/// Reprobe footage whose stream metadata is missing: projects saved
/// before the probe recorded streams (C++ files have no `<streams>`
/// segment at all) load with an empty inventory, which leaves the
/// footage duration unknown. Relative filenames resolve against the
/// project file's directory (the C++ project-dir convention) and are
/// made absolute on the node. Best effort per node: an unreadable or
/// missing file stays unprobed.
pub fn reprobe_unprobed_footage(project: &ProjectRef) {
	let dir = {
		let guard = lock(project);
		std::path::Path::new(guard.filename())
			.parent()
			.map(|p| p.to_path_buf())
	};
	let targets: Vec<(NodeId, std::path::PathBuf)> = {
		let guard = lock(project);
		footage_ids(&guard)
			.into_iter()
			.filter_map(|id| {
				let f = footage_behavior(&guard.graph, id)?;
				if !f.streams.is_empty() || f.filename.is_empty() {
					return None;
				}
				let path = std::path::Path::new(&f.filename);
				let resolved = if path.is_absolute() {
					path.to_path_buf()
				} else {
					dir.as_ref()?.join(path)
				};
				resolved.is_file().then_some((id, resolved))
			})
			.collect()
	};
	for (id, resolved) in targets {
		let mut guard = lock(project);
		if let Some(f) = guard
			.graph
			.get_mut(id)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<FootageBehavior>())
		{
			f.filename = resolved.to_string_lossy().into_owned();
			// Best effort: a failed probe leaves the footage unprobed
			// (valid stays false), exactly like a failed import probe.
			let _ = f.probe();
		}
	}
}

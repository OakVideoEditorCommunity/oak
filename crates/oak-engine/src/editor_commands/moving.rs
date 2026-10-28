//! Linked clip movement, deletion, markers, clipboard and block-property edits.
//!
//! Public mutation functions push their own undo entries unless their document
//! comment says otherwise. Clipboard capture is read-only; command builders in
//! other modules are used when several operations must share one undo entry.
//! Integer timeline/media fields are frame timestamps in the owning sequence's
//! timebase, while marker and work-area APIs use rational time directly.

#![allow(unused_imports)]
use super::prelude::*;
use super::{edit_menu::*, editing::*, generators::*, media::*, project::*, sequence::*, tracks::*, transitions::*};


/// Moves `clip` to `new_in_ts` and moves every linked follower by the same
/// frame delta as one undoable "Move Clip" entry.
///
/// `dest_track` changes only the dragged clip's track; each follower remains
/// on its own track. The dragged clip supplies the reference delta, and that
/// delta is clamped against the earliest in-point in the whole group. Thus no
/// participant moves before frame 0 and their relative offsets are preserved.
/// The main clip must be a tracked clip with a valid sequence timebase.
/// Followers that have become detached from their tracks are skipped, while
/// other invalid followers return an error before any command is executed.
pub fn move_clip_with_links(
	p: &ProjectRef,
	clip: NodeId,
	dest_track: Option<NodeId>,
	new_in_ts: i64,
	linked: &[NodeId],
) -> Result<(), String> {
	// The dragged clip's old in point frames the shared frame delta.
	let old_in_ts = {
		let g = lock(p);
		let tb = clip_track(&g.graph, clip)
			.and_then(|t| track_behavior(&g.graph, t))
			.and_then(|t| t.track_list)
			.and_then(|l| track_list_behavior(&g.graph, l))
			.and_then(|l| l.sequence)
			.and_then(|s| sequence_time_base(&g.graph, s))
			.ok_or_else(|| "the clip's sequence has no valid frame rate".to_string())?;
		let (in_r, _, _) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		rational_to_ts(in_r, tb)
	};
	// The linked clips' current in points (each stays on its own track;
	// only its in point follows the shared delta). A linked clip that is
	// off any track (e.g. ripple-removed) is left in place instead of
	// failing the whole move.
	let mut linked_ins: Vec<(NodeId, i64)> = Vec::new();
	for &other in linked {
		if other == clip {
			continue;
		}
		let other_in_ts = {
			let g = lock(p);
			let Some(track) = clip_track(&g.graph, other) else {
				continue;
			};
			let tb = track_behavior(&g.graph, track)
				.and_then(|t| t.track_list)
				.and_then(|l| track_list_behavior(&g.graph, l))
				.and_then(|l| l.sequence)
				.and_then(|s| sequence_time_base(&g.graph, s))
				.ok_or_else(|| "a linked clip's sequence has no valid frame rate".to_string())?;
			let (in_r, _, _) = clip_range(&g.graph, other)
				.ok_or_else(|| "a linked node is not a clip".to_string())?;
			rational_to_ts(in_r, tb)
		};
		linked_ins.push((other, other_in_ts));
	}
	// Group-aware clamp: the shared delta may not push ANY clip of the
	// group below frame 0 (per-clip clamping would silently de-sync the
	// group).
	let min_in = linked_ins
		.iter()
		.map(|(_, ts)| *ts)
		.fold(old_in_ts, i64::min);
	let delta = (new_in_ts - old_in_ts).max(-min_in);
	let new_in_ts = old_in_ts + delta;

	let mut commands = Vec::new();
	match dest_track {
		Some(track) => commands.extend(move_clip_to_track_commands(p, clip, track, new_in_ts)?),
		None => commands.push(move_clip_command(p, clip, new_in_ts)?),
	}
	for (other, other_in_ts) in linked_ins {
		commands.push(move_clip_command(p, other, other_in_ts + delta)?);
	}
	push_multi(commands, "Move Clip")
}

/// Links or unlinks the supplied blocks as one undoable entry.
///
/// Linking creates every unordered pair inside the set, not merely a chain or
/// a link to the first block. Unlinking removes all internal pairs. In both
/// cases, undo restores the exact internal links captured before the edit;
/// links to nodes outside `blocks` are never changed. Fewer than two blocks
/// are a successful no-op and do not create an undo entry.
pub fn set_clips_linked(p: &ProjectRef, blocks: &[NodeId], linked: bool) -> Result<(), String> {
	if blocks.len() < 2 {
		return Ok(());
	}
	let blocks: Vec<NodeId> = blocks.to_vec();
	// Snapshot the prior links among the set (the undo's target state).
	let prior: Vec<(NodeId, NodeId)> = {
		let g = lock(p);
		let mut v = Vec::new();
		for (i, &a) in blocks.iter().enumerate() {
			for &b in &blocks[i + 1..] {
				if g.graph.links_of(a).contains(&b) {
					v.push((a, b));
				}
			}
		}
		v
	};
	let all_pairs: Vec<(NodeId, NodeId)> = {
		let mut v = Vec::new();
		for (i, &a) in blocks.iter().enumerate() {
			for &b in &blocks[i + 1..] {
				v.push((a, b));
			}
		}
		v
	};
	// The shared mutation: clear the set's internal links, then restore the
	// target topology (redo: all pairs when linking, none when unlinking;
	// undo: the snapshot).
	fn apply(p: &ProjectRef, blocks: &[NodeId], target: &[(NodeId, NodeId)]) {
		let mut g = lock(p);
		for (i, &a) in blocks.iter().enumerate() {
			for &b in &blocks[i + 1..] {
				g.graph.unlink(a, b);
			}
		}
		for &(a, b) in target {
			g.graph.link(a, b);
		}
	}
	let redo_target = if linked { all_pairs } else { Vec::new() };
	let (p1, p2) = (p.clone(), p.clone());
	let (b1, b2) = (blocks.clone(), blocks);
	push_command(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || apply(&p1, &b1, &redo_target),
			move || apply(&p2, &b2, &prior),
		),
		if linked { "Link Clips" } else { "Unlink Clips" },
	)
}

/// Deletes one tracked clip without moving later blocks.
///
/// The first command replaces the clip on its track with a gap of the same
/// duration; the second removes the clip node from the project graph. Both are
/// grouped into one undoable "Delete Clips" entry, so undo restores both the
/// node and its track placement. A clip that is not on a track is rejected.
pub fn delete_clip(p: &ProjectRef, clip: NodeId) -> Result<(), String> {
	let source_track = {
		let g = lock(p);
		clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?
	};
	let gap = oak_timeline::undogeneral::TrackReplaceBlockWithGapCommand::new(
		node_ref(p, source_track),
		node_ref(p, clip),
		true,
	)
	.to_command();
	let remove = oak_task::nodeops::remove_node_command(p.clone(), clip);
	push_multi(vec![gap, remove], "Delete Clips")
}

/// Removes one tracked clip and ripples its owning track over the clip range.
///
/// This delegates trimming, removal and tail movement to
/// `TrackRippleRemoveAreaCommand`, wrapped as one "Ripple Delete Clip" undo
/// entry. Other tracks are not visited. The clip must have both an owning
/// track and a readable clip range; otherwise an error is returned.
pub fn ripple_delete_clip(p: &ProjectRef, clip: NodeId) -> Result<(), String> {
	let (track, range) = {
		let g = lock(p);
		let track =
			clip_track(&g.graph, clip).ok_or_else(|| "the clip is not on a track".to_string())?;
		let (in_r, out_r, _) =
			clip_range(&g.graph, clip).ok_or_else(|| "the node is not a clip".to_string())?;
		(track, TimeRange::new(in_r, out_r))
	};
	push(
		oak_timeline::undoripple::TrackRippleRemoveAreaCommand::new(node_ref(p, track), range)
			.to_command(),
		"Ripple Delete Clip",
	)
}

/// Adds a zero-length marker at `time` as one undoable "Add Marker" entry.
///
/// `time` is rational seconds, and `name` and `color` are stored unchanged.
/// An existing marker whose in-point compares equal to `time` is rejected
/// before command construction, avoiding the marker list's duplicate-time
/// assertion. This is the legacy `CHandle` variant; session dispatch uses its
/// Arc-backed marker list directly.
pub fn marker_add(markers: &CHandle, time: Rational, name: &str, color: i32) -> Result<(), String> {
	if marker_index_at(markers, time).is_some() {
		return Err("a marker already exists at that time".to_string());
	}
	push(
		oak_timeline::marker::MarkerAddCommand::new(
			*markers,
			TimeRange::new(time, time),
			name,
			color,
		)
		.to_command(),
		"Add Marker",
	)
}

/// Removes the first marker whose in-point equals `time`.
///
/// The removal is one undoable "Remove Marker" entry. If no marker has that
/// exact rational time, this function returns `Err("no marker at that time")`;
/// it does not silently treat a missing marker as a successful no-op.
pub fn marker_remove(markers: &CHandle, time: Rational) -> Result<(), String> {
	let Some(index) = marker_index_at(markers, time) else {
		return Err("no marker at that time".to_string());
	};
	push(
		oak_timeline::marker::MarkerRemoveCommand::new(*markers, index).to_command(),
		"Remove Marker",
	)
}

/// Undoable workarea set ("Set Workarea"): the enabled flag plus the
/// in/out range as ONE entry. `old_range` is supplied by the caller (the
/// range read before the change — e.g. the drag-start range of a ruler
/// work-area drag); the enabled flag's previous value is captured by the
/// module command itself.
pub fn workarea_set_undoable(
	wa: &CHandle,
	enabled: bool,
	range: TimeRange,
	old_range: TimeRange,
) -> Result<(), String> {
	push_multi(
		vec![
			oak_timeline::workarea::WorkareaSetEnabledCommand::new(*wa, enabled).to_command(),
			oak_timeline::workarea::WorkareaSetRangeCommand::new_with_old(*wa, range, old_range)
				.to_command(),
		],
		"Set Workarea",
	)
}

/// Undoable removal of `node` from the project graph ("Remove Node"; the
/// module's remove command drops incident edges and restores the entry on
/// undo). The sequence node itself (the graph's output) cannot be
/// removed.
pub fn remove_node(p: &ProjectRef, node: NodeId) -> Result<(), String> {
	push(
		oak_timeline::undocommon::create_remove_command(&node_ref(p, node)),
		"Remove Node",
	)
}

// ---------------------------------------------------------------------------
// Test serialization
// ---------------------------------------------------------------------------

/// A process-wide test lock for state shared by engine and app tests.
///
/// Tests that touch the global undo stack, oak_core configuration or codec
/// decode sessions hold this guard to prevent cross-test interference.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Clipboard (Cut / Copy / Paste)
// ---------------------------------------------------------------------------

/// One clip captured in the clipboard (Cut/Copy). Everything needed to
/// re-place it on a timeline is here; effects/multicam contexts are not
/// modeled yet (footage clips carry their block core only).
#[derive(Clone, Debug)]
pub struct ClipboardClip {
	/// The footage node the clip decodes.
	pub footage: NodeId,
	/// Track type the clip was on.
	pub kind: TrackType,
	/// Per-type index of the track it was on (re-used on paste when the
	/// track still exists).
	pub track_index: usize,
	/// Media in-point in frame timestamps.
	pub media_in_ts: i64,
	/// Timeline in-point in frame timestamps.
	pub start_ts: i64,
	/// Duration in frame timestamps.
	pub length_ts: i64,
	/// Playback speed.
	pub speed: f64,
}

/// Captures footage-backed clips into owned, time-ordered clipboard entries.
///
/// Nodes without upstream footage, a readable clip range, an owning track or
/// a track list are skipped. Frame timestamps are derived from each source
/// sequence's timebase, falling back to 1/25 when that timebase is unavailable.
/// The returned entries are sorted by `start_ts`; no graph mutation or undo
/// entry is produced. Cut is implemented by callers as copy followed by delete.
pub fn copy_clips(p: &ProjectRef, clips: &[NodeId]) -> Vec<ClipboardClip> {
	let g = lock(p);
	let mut out = Vec::new();
	for &clip in clips {
		let Some(footage) = find_input_footage(&g.graph, clip) else {
			continue;
		};
		let Some((in_r, out_r, media_in)) = clip_range(&g.graph, clip) else {
			continue;
		};
		let Some(track) = clip_track(&g.graph, clip) else {
			continue;
		};
		let Some(t) = track_behavior(&g.graph, track) else {
			continue;
		};
		let Some(list) = t.track_list.and_then(|l| track_list_behavior(&g.graph, l)) else {
			continue;
		};
		let (tb_num, tb_den) = {
			// Frame timestamps are stored rationally; the clipboard keeps
			// the sequence timebase for exactness.
			let seq = list.sequence.and_then(|s| sequence_time_base(&g.graph, s));
			seq.unwrap_or((1, 25))
		};
		let to_ts = |r: Rational| {
			(r.numerator() * tb_den)
				.checked_div(r.denominator() * tb_num)
				.unwrap_or(0)
		};
		let speed = clip_behavior(&g.graph, clip)
			.map(|c| c.core.speed)
			.unwrap_or(1.0);
		out.push(ClipboardClip {
			footage,
			kind: list.kind,
			track_index: t.index.max(0) as usize,
			media_in_ts: to_ts(media_in),
			start_ts: to_ts(in_r),
			length_ts: to_ts(out_r - in_r),
			speed,
		});
	}
	out.sort_by_key(|c| c.start_ts);
	out
}

/// Pastes clipboard entries at `playhead_ts` as one undoable "Paste" entry.
///
/// `items[0]` anchors the paste: its start becomes the playhead and subsequent
/// entries retain their offsets from that first entry. Each clip reuses its
/// recorded per-kind track index, clamped to the last available track of that
/// kind. Every additional pasted clip is linked to the first pasted clip;
/// original link topology is not stored in `ClipboardClip` and is not restored.
/// Returned node IDs follow `items` order.
pub fn paste_clips(
	p: &ProjectRef,
	seq: NodeId,
	items: &[ClipboardClip],
	playhead_ts: i64,
) -> Result<Vec<NodeId>, String> {
	let (commands, new_ids) = paste_clips_commands(p, seq, items, playhead_ts)?;
	push_multi(commands, "Paste")?;
	Ok(new_ids)
}

// ---------------------------------------------------------------------------
// Block property edits (启用/禁用、速度、重命名)
// ---------------------------------------------------------------------------

/// Enables or disables every supplied block as one undoable batch.
///
/// Missing nodes, non-block nodes and blocks already in the requested state
/// are skipped. Each remaining block captures its own previous flag for undo;
/// an empty or fully unchanged input therefore produces no undo entry.
pub fn set_blocks_enabled(p: &ProjectRef, blocks: &[NodeId], enabled: bool) -> Result<(), String> {
	let mut children = Vec::new();
	for &b in blocks {
		let old = {
			let g = lock(p);
			match block_core_of(&g.graph, b) {
				Some(core) if core.enabled != enabled => core.enabled,
				_ => continue,
			}
		};
		let (r1, r2) = (node_ref(p, b), node_ref(p, b));
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || oak_timeline::util::block_set_enabled(&r1, enabled),
			move || oak_timeline::util::block_set_enabled(&r2, old),
		));
	}
	push_multi(
		children,
		if enabled {
			"Enable Clips"
		} else {
			"Disable Clips"
		},
	)
}

/// Sets `BlockCore::speed` on the supplied clips without changing their
/// timeline ranges or media-in points.
///
/// Speed must be finite and non-zero; negative values are accepted. Nodes that
/// are not blocks, and clips already within `1e-12` of the requested speed,
/// are skipped. All remaining changes form one "Speed/Duration" undo entry,
/// with each block's previous speed restored independently on undo.
pub fn set_clips_speed(p: &ProjectRef, clips: &[NodeId], speed: f64) -> Result<(), String> {
	if !(speed.is_finite() && speed.abs() > 1e-9) {
		return Err("speed must be a non-zero finite number".to_string());
	}
	let mut children = Vec::new();
	for &c in clips {
		let old = {
			let g = lock(p);
			match block_core_of(&g.graph, c) {
				Some(core) if (core.speed - speed).abs() > 1e-12 => core.speed,
				_ => continue,
			}
		};
		let (r1, r2) = (node_ref(p, c), node_ref(p, c));
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				let mut g = lock(&r1.project);
				with_block_core_mut(&mut g.graph, r1.id, |core| core.speed = speed);
			},
			move || {
				let mut g = lock(&r2.project);
				with_block_core_mut(&mut g.graph, r2.id, |core| core.speed = old);
			},
		));
	}
	push_multi(children, "Speed/Duration")
}

/// Sets the node label of every supplied block as one undoable "Rename" entry.
///
/// Whitespace-only names are rejected; otherwise `name` is stored exactly as
/// supplied, including surrounding whitespace. Missing nodes and nodes already
/// carrying the label are skipped. Each changed node restores its own previous
/// label on undo.
pub fn rename_blocks(p: &ProjectRef, blocks: &[NodeId], name: &str) -> Result<(), String> {
	if name.trim().is_empty() {
		return Err("empty name".to_string());
	}
	let mut children = Vec::new();
	for &b in blocks {
		let Some(old) = ({
			let g = lock(p);
			g.graph.get(b).map(|e| e.core.label.clone())
		}) else {
			continue;
		};
		if old == name {
			continue;
		}
		let (p1, p2) = (p.clone(), p.clone());
		let (new_name, old_name) = (name.to_string(), old);
		children.push(oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				if let Some(e) = lock(&p1).graph.get_mut(b) {
					e.core.label = new_name.clone();
				}
			},
			move || {
				if let Some(e) = lock(&p2).graph.get_mut(b) {
					e.core.label = old_name.clone();
				}
			},
		));
	}
	push_multi(children, "Rename")
}

//! Execute one strongly typed semantic command against a live editor session.
//!
//! This module is deliberately the only command dispatcher. The C ABI converts
//! its native parameters into [`EditCommand`], while the SDK performs the same
//! mapping in Rust. Both paths arrive here, so undo grouping, current-sequence
//! selection, validation and result construction cannot drift apart.
//!
//! # Conventions
//!
//! * `project` is the session's only live project, never a reloaded copy.
//! * `sequence` is captured from `current_sequence` once per command. Clip and
//!   timeline operations that do not take an explicit sequence use this value.
//! * IDs cross the boundary as generational `u64` tokens and are validated by
//!   [`id`]/[`ids`]. Invalid IDs return an error before the edit is attempted.
//! * `primary`, `many`, `value` and `empty` keep result shapes consistent; they
//!   do not execute commands or alter the undo stack.
//! * Domain helpers in `editor_commands` own the actual undo semantics. This
//!   dispatcher must not wrap every operation in another undo group.

use oak_node::id::NodeId;
use oak_core::TimeRange;

use crate::editor::SessionState;
use crate::editor_commands as ops;

use super::{EditCommand, EditResult, WireClipboardClip, WireRational, id, track_kind};

fn ids(raw: &[u64]) -> Result<Vec<NodeId>, String> {
	raw.iter().map(|&raw| id(raw)).collect()
}

/// Converts an operation that creates one node into the common result shape.
/// The `?` is intentional: failed creation returns no ID and no result object.
fn primary(value: Result<NodeId, String>) -> Result<EditResult, String> {
	Ok(EditResult {
		primary: Some(value?.identity()),
		..EditResult::default()
	})
}

/// Converts multi-node results (linked placement, paste, duplicate, nesting).
/// IDs are returned in domain order so callers can map them back to requests.
fn many(value: Result<Vec<NodeId>, String>) -> Result<EditResult, String> {
	Ok(EditResult {
		ids: value?.into_iter().map(|id| id.identity()).collect(),
		..EditResult::default()
	})
}

/// Converts count/index results such as AddTrack and AddDefaultTransition.
fn value(result: Result<usize, String>) -> Result<EditResult, String> {
	Ok(EditResult {
		value: result? as i64,
		..EditResult::default()
	})
}

/// Converts a mutation with no direct return value. `revision` is filled in
/// by the ABI wrapper after execution, not guessed here.
fn empty(result: Result<(), String>) -> Result<EditResult, String> {
	result.map(|()| EditResult::default())
}

/// Executes `command` once and propagates domain errors unchanged.
///
/// Atomicity and undo grouping are the domain command's contract; this layer
/// never reports success after a failed operation and never replays a command
/// to make output buffers fit.
pub(crate) fn execute(session: &mut SessionState, command: EditCommand) -> Result<EditResult, String> {
	let project = &session.project;
	let sequence = session.current_sequence;
	match command {
		// ---- tracks and project/sequence structure ------------------------
		EditCommand::AddTrack { kind } => value(ops::add_track(project, sequence, track_kind(kind)?)),
		EditCommand::RemoveTrack { track } => empty(ops::remove_track(project, id(track)?)),
		EditCommand::SetTrackHeight { track, height } => {
			ops::set_track_height(project, id(track)?, height);
			Ok(EditResult::default())
		}
		EditCommand::SetTrackMuted { track, muted } => empty(ops::set_track_muted(project, id(track)?, muted)),
		EditCommand::SetTrackLocked { track, locked } => empty(ops::set_track_locked(project, id(track)?, locked)),
		EditCommand::CreateSequence {
			// All four format fields are either present or absent. Mixing a
			// custom width with default rate/interlacing would be ambiguous,
			// so reject it rather than silently inventing parameters.
			name,
			width,
			height,
			rate,
			interlaced,
		} => {
			let params = match (width, height, rate, interlaced) {
				(Some(width), Some(height), Some(rate), Some(interlaced)) => {
					Some((width, height, rate.native(), interlaced))
				}
				(None, None, None, None) => None,
				_ => return Err("sequence params must be all present or all absent".into()),
			};
			let id = ops::create_sequence_with_params(project, &name, params);
			session.current_sequence = id;
			Ok(EditResult {
				primary: Some(id.identity()),
				..EditResult::default()
			})
		}
		EditCommand::SetSequenceParameters {
			sequence,
			name,
			width,
			height,
			rate,
			interlaced,
		} => empty(ops::set_sequence_parameters(
			project,
			id(sequence)?,
			&name,
			width,
			height,
			rate.native(),
			interlaced,
		)),
		EditCommand::SetSequencePlayhead { sequence, time } => {
			// Playhead writes are not undoable edits, matching viewer seek.
			ops::sequence_set_playhead(project, id(sequence)?, time.native());
			Ok(EditResult::default())
		}
		EditCommand::CreateFolder { name } => primary(ops::create_folder(project, &name)),
		EditCommand::CreateTextFootage => primary(ops::create_text_footage_node(project)),
		EditCommand::ImportFootage { path } => primary(ops::import_footage(project, std::path::Path::new(&path))),
		EditCommand::EnsureSequencesMounted => {
			ops::ensure_sequences_mounted(project);
			Ok(EditResult::default())
		}
		EditCommand::ReprobeUnprobedFootage => {
			ops::reprobe_unprobed_footage(project);
			Ok(EditResult::default())
		}
		EditCommand::CreateAdjustmentLayer { track, in_ts, out_ts } => {
			primary(ops::create_adjustment_layer(project, sequence, id(track)?, in_ts, out_ts))
		}
		EditCommand::PlaceFootageClip {
			footage,
			kind,
			track_index,
			in_ts,
			out_ts,
			media_in_ts,
		} => primary(ops::place_footage_clip(
			project,
			sequence,
			id(footage)?,
			track_kind(kind)?,
			track_index,
			in_ts,
			out_ts,
			media_in_ts,
		)),
		EditCommand::PlaceNestedSequenceClip {
			source,
			track_index,
			in_ts,
			out_ts,
		} => primary(ops::place_nested_sequence_clip(
			project,
			sequence,
			id(source)?,
			track_index,
			in_ts,
			out_ts,
		)),
		EditCommand::PlaceTextClip {
			text,
			track_index,
			in_ts,
			out_ts,
		} => primary(ops::place_text_clip(project, sequence, id(text)?, track_index, in_ts, out_ts)),
		EditCommand::PlaceGeneratorClip {
			generator,
			track_index,
			in_ts,
			out_ts,
		} => primary(ops::place_generator_clip(
			project,
			sequence,
			id(generator)?,
			track_index,
			in_ts,
			out_ts,
		)),
		EditCommand::PlaceFootageClipsLinked {
			// A/V drops are one undo entry. Placement tuples retain their
			// caller-provided order; `many` preserves the created clip order.
			footage,
			placements,
			in_ts,
			out_ts,
			media_in_ts,
		} => {
			let placements = placements
				.into_iter()
				.map(|(kind, index)| Ok((track_kind(kind)?, index)))
				.collect::<Result<Vec<_>, String>>()?;
			many(ops::place_footage_clips_linked(
				project,
				sequence,
				id(footage)?,
				&placements,
				in_ts,
				out_ts,
				media_in_ts,
			))
		}
		EditCommand::AddTransitionAtSeam { previous, next, half } => primary(ops::add_transition_at_seam(
			project,
			id(previous)?,
			id(next)?,
			half.native(),
		)),
		EditCommand::AddTransitionAtEdge {
			clip,
			start_edge,
			length,
		} => primary(ops::add_transition_at_edge(
			project,
			id(clip)?,
			start_edge,
			length.native(),
		)),
		EditCommand::AddDefaultTransition { clips, half } => value(ops::add_default_transition(
			project,
			sequence,
			&ids(&clips)?,
			half.native(),
		)),
		EditCommand::SetTransitionLength {
			clip,
			start_edge,
			// `frame` is the minimum wedge width, `length` is the requested
			// width. The domain clamps to the clip length and returns the
			// actual width, which must reach the caller unchanged.
			frame,
			length,
		} => {
			let clamped = ops::set_transition_length(project, id(clip)?, start_edge, frame.native(), length.native())?;
			Ok(EditResult {
				rational: Some(WireRational {
					num: clamped.numerator(),
					den: clamped.denominator(),
				}),
				..EditResult::default()
			})
		}
		EditCommand::SplitClip { clip, time_ts } => empty(ops::split_clip(project, id(clip)?, time_ts)),
		EditCommand::SplitClipsPreservingLinks { clips, time_ts } => {
			empty(ops::split_clips_preserving_links(project, &ids(&clips)?, time_ts))
		}
		EditCommand::TrimClip {
			clip,
			new_in_ts,
			new_out_ts,
		} => empty(ops::trim_clip(project, id(clip)?, new_in_ts, new_out_ts)),
		EditCommand::RippleTrimClip {
			clip,
			start_edge,
			new_frame,
		} => empty(ops::ripple_trim_clip(project, id(clip)?, start_edge, new_frame)),
		EditCommand::RollEdit {
			track,
			a,
			b,
			new_frame,
		} => empty(ops::roll_edit(project, id(track)?, id(a)?, id(b)?, new_frame)),
		EditCommand::SlideClip { clip, new_start } => empty(ops::slide_clip(project, id(clip)?, new_start)),
		EditCommand::SlipClip { clip, new_media_in } => empty(ops::slip_clip(project, id(clip)?, new_media_in)),
		EditCommand::MoveClip { clip, new_in_ts } => empty(ops::move_clip(project, id(clip)?, new_in_ts)),
		EditCommand::MoveClipToTrack {
			clip,
			track,
			new_in_ts,
		} => empty(ops::move_clip_to_track(project, id(clip)?, id(track)?, new_in_ts)),
		EditCommand::MoveClipWithLinks {
			// `None` means move on the current track; `Some(id)` is a real
			// destination track, not the invalid-ID sentinel.
			clip,
			track,
			new_in_ts,
			linked,
		} => {
			let track = match track {
				Some(track) => Some(id(track)?),
				None => None,
			};
			empty(ops::move_clip_with_links(
				project,
				id(clip)?,
				track,
				new_in_ts,
				&ids(&linked)?,
			))
		}
		EditCommand::SetClipsLinked { clips, linked } => empty(ops::set_clips_linked(project, &ids(&clips)?, linked)),
		EditCommand::DeleteClip { clip } => empty(ops::delete_clip(project, id(clip)?)),
		EditCommand::RippleDeleteClip { clip } => empty(ops::ripple_delete_clip(project, id(clip)?)),
		EditCommand::DeleteArea {
			in_ts,
			out_ts,
			ripple,
		} => empty(ops::delete_area(project, sequence, in_ts, out_ts, ripple)),
		EditCommand::RollClipEdgeTo {
			clip,
			at_in,
			frame_ts,
		} => empty(ops::roll_clip_edge_to(project, id(clip)?, at_in, frame_ts)),
		EditCommand::MarkerAdd { time, name, color } => {
			// Session auxiliaries are already native Arc values. Constructing
			// a temporary CHandle here previously caused a segfault, so the
			// undo command is created directly with `new_arc`.
			let time = time.native();
			if session
				.markers
				.lock()
				.unwrap_or_else(|e| e.into_inner())
				.get_marker_at_time(time)
				.is_some()
			{
				return Err("a marker already exists at that time".into());
			}
			empty(ops::push_command(
				oak_timeline::marker::MarkerAddCommand::new_arc(
					session.markers.clone(),
					TimeRange::new(time, time),
					&name,
					color,
				)
				.to_command(),
				"Add Marker",
			))
		}
		EditCommand::MarkerRemove { time } => {
			// Removing a missing marker is an error. As with add, the marker
			// list is read under its own lock and the lock is released before
			// the undo command executes.
			let time = time.native();
			let list = session.markers.lock().unwrap_or_else(|e| e.into_inner());
			let Some(index) = (0..list.size())
				.find(|&index| list.at(index).is_some_and(|marker| marker.time().in_() == time))
			else {
				return Err("no marker at that time".into());
			};
			drop(list);
			empty(ops::push_command(
				oak_timeline::marker::MarkerRemoveCommand::new_arc(session.markers.clone(), index)
					.to_command(),
				"Remove Marker",
			))
		}
		EditCommand::WorkareaSet { enabled, range } => {
			// Preview writes during a ruler drag are deliberately not pushed
			// to undo history. The drag commit uses WorkareaSetUndoable below.
			let mut workarea = session.workarea.lock().unwrap_or_else(|e| e.into_inner());
			workarea.set_enabled(enabled);
			workarea.set_range(range.native());
			drop(workarea);
			Ok(EditResult::default())
		}
		EditCommand::WorkareaSetUndoable {
			// Enabled and range form one undo entry. `old_range` belongs to
			// the drag start, not necessarily to the currently previewed range.
			enabled,
			range,
			old_range,
		} => {
			empty(ops::push_multi_command(
				vec![
					oak_timeline::workarea::WorkareaSetEnabledCommand::new_arc(
						session.workarea.clone(),
						enabled,
					)
					.to_command(),
					oak_timeline::workarea::WorkareaSetRangeCommand::new_with_old_arc(
						session.workarea.clone(),
						range.native(),
						old_range.native(),
					)
					.to_command(),
				],
				"Set Workarea",
			))
		}
		EditCommand::RemoveNode { node } => empty(ops::remove_node(project, id(node)?)),
		EditCommand::Connect { from, to, input_id } => empty(ops::push_command(
			ops::connect_command(project, id(from)?, id(to)?, &input_id)?,
			"Connect Nodes",
		)),
		EditCommand::Disconnect { to, input_id } => empty(ops::push_command(
			ops::disconnect_command(project, id(to)?, &input_id)?,
			"Disconnect Nodes",
		)),
		EditCommand::SetContextPosition { node, context, x, y } => empty(ops::push_command(
			ops::set_context_position_command(project, id(node)?, id(context)?, x, y)?,
			"Move Node",
		)),
		EditCommand::CopyClips { clips } => {
			// Clipboard ownership belongs to the session, not the project or
			// an AppEngine widget. A copy also returns an owned DTO snapshot;
			// a later paste uses the same session clipboard.
			session.clipboard = ops::copy_clips(project, &ids(&clips)?);
			Ok(EditResult {
				clipboard: session.clipboard.iter().cloned().map(WireClipboardClip::wire).collect(),
				..EditResult::default()
			})
		}
		EditCommand::PasteClips { playhead_ts } => {
			many(ops::paste_clips(project, sequence, &session.clipboard, playhead_ts))
		}
		EditCommand::SetBlocksEnabled { clips, enabled } => {
			empty(ops::set_blocks_enabled(project, &ids(&clips)?, enabled))
		}
		EditCommand::SetClipsSpeed { clips, speed } => empty(ops::set_clips_speed(project, &ids(&clips)?, speed)),
		EditCommand::RenameBlocks { clips, name } => empty(ops::rename_blocks(project, &ids(&clips)?, &name)),
		EditCommand::DuplicateClips { clips } => many(ops::duplicate_clips(project, sequence, &ids(&clips)?)),
		EditCommand::PasteClipsInsert { playhead_ts } => {
			many(ops::paste_clips_insert(project, sequence, &session.clipboard, playhead_ts))
		}
		EditCommand::NestClips { clips } => primary(ops::nest_clips(project, sequence, &ids(&clips)?)),
		EditCommand::EffectInsert {
			host,
			index,
			type_id,
		} => primary(crate::effect::insert_impl(project, id(host)?, index, &type_id)),
		EditCommand::EffectRemove { host, effect } => {
			empty(crate::effect::remove_impl(project, id(host)?, id(effect)?))
		}
		EditCommand::EffectMove {
			host,
			effect,
			new_index,
		} => empty(crate::effect::move_impl(project, id(host)?, id(effect)?, new_index)),
		EditCommand::EffectSetEnabled { effect, enabled } => {
			empty(crate::effect::set_enabled_impl(project, id(effect)?, enabled))
		}
		EditCommand::SetNodeStandardValue {
			node,
			input_id,
			element,
			value,
		} => empty(crate::node::set_standard_value_impl(
			project,
			id(node)?,
			&input_id,
			element,
			value,
		)),
	}
}

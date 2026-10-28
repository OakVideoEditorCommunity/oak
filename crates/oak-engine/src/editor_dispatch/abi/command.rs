//! Conversion from borrowed native ABI slots to owned semantic commands.

use crate::codec::str_arg;
use crate::node::OAK_NODE_ID_INVALID;

use super::super::EditCommand;
use super::{OakEditOp, OakEditParams};

/// Copies an optional UTF-8 byte slice into an owned string for command dispatch.
pub(super) fn text(ptr: *const u8, len: usize) -> String {
	unsafe { str_arg(ptr, len) }.unwrap_or_default().to_string()
}

/// Decodes borrowed ABI storage into an owned command before executing it.
/// This keeps no caller pointers in the domain layer and validates list lengths
/// indirectly through the resulting command's domain checks.
pub(super) fn command(op: OakEditOp, p: &OakEditParams) -> Result<EditCommand, String> {
	let ids = if p.ids.is_null() {
		Vec::new()
	} else {
		unsafe { std::slice::from_raw_parts(p.ids, p.ids_len) }.to_vec()
	};
	let placements = if p.placements.is_null() {
		Vec::new()
	} else {
		unsafe { std::slice::from_raw_parts(p.placements, p.placements_len) }
			.iter()
			.map(|placement| (placement.kind, placement.index))
			.collect()
	};
	Ok(match op {
		OakEditOp::AddTrack => EditCommand::AddTrack { kind: p.kind },
		OakEditOp::RemoveTrack => EditCommand::RemoveTrack { track: p.a },
		OakEditOp::SetTrackHeight => EditCommand::SetTrackHeight {
			track: p.a,
			height: p.height,
		},
		OakEditOp::SetTrackMuted => EditCommand::SetTrackMuted {
			track: p.a,
			muted: p.enabled,
		},
		OakEditOp::SetTrackLocked => EditCommand::SetTrackLocked {
			track: p.a,
			locked: p.enabled,
		},
		OakEditOp::CreateSequence => EditCommand::CreateSequence {
			name: text(p.name, p.name_len),
			width: (p.kind != 0).then_some(p.a as i32),
			height: (p.kind != 0).then_some(p.b as i32),
			rate: (p.kind != 0).then_some(p.rate),
			interlaced: (p.kind != 0).then_some(p.enabled),
		},
		OakEditOp::SetSequenceParameters => EditCommand::SetSequenceParameters {
			sequence: p.a,
			name: text(p.name, p.name_len),
			width: p.b as i32,
			height: p.c as i32,
			rate: p.rate,
			interlaced: p.enabled,
		},
		OakEditOp::SetSequencePlayhead => EditCommand::SetSequencePlayhead {
			sequence: p.a,
			time: p.rate,
		},
		OakEditOp::CreateFolder => EditCommand::CreateFolder {
			name: text(p.name, p.name_len),
		},
		OakEditOp::CreateTextFootage => EditCommand::CreateTextFootage,
		OakEditOp::ImportFootage => EditCommand::ImportFootage {
			path: text(p.path, p.path_len),
		},
		OakEditOp::EnsureSequencesMounted => EditCommand::EnsureSequencesMounted,
		OakEditOp::ReprobeUnprobedFootage => EditCommand::ReprobeUnprobedFootage,
		OakEditOp::CreateAdjustmentLayer => EditCommand::CreateAdjustmentLayer {
			track: p.a,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
		},
		OakEditOp::PlaceFootageClip => EditCommand::PlaceFootageClip {
			footage: p.a,
			kind: p.kind,
			track_index: p.track_index,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
			media_in_ts: p.media_in_ts,
		},
		OakEditOp::PlaceNestedSequenceClip => EditCommand::PlaceNestedSequenceClip {
			source: p.a,
			track_index: p.track_index,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
		},
		OakEditOp::PlaceTextClip => EditCommand::PlaceTextClip {
			text: p.a,
			track_index: p.track_index,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
		},
		OakEditOp::PlaceGeneratorClip => EditCommand::PlaceGeneratorClip {
			generator: p.a,
			track_index: p.track_index,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
		},
		OakEditOp::PlaceFootageClipsLinked => EditCommand::PlaceFootageClipsLinked {
			footage: p.a,
			placements,
			in_ts: p.in_ts,
			out_ts: p.out_ts,
			media_in_ts: p.media_in_ts,
		},
		OakEditOp::AddTransitionAtSeam => EditCommand::AddTransitionAtSeam {
			previous: p.a,
			next: p.b,
			half: p.rate,
		},
		OakEditOp::AddTransitionAtEdge => EditCommand::AddTransitionAtEdge {
			clip: p.a,
			start_edge: p.start_edge,
			length: p.rate,
		},
		OakEditOp::AddDefaultTransition => EditCommand::AddDefaultTransition {
			clips: ids,
			half: p.rate,
		},
		OakEditOp::SetTransitionLength => EditCommand::SetTransitionLength {
			clip: p.a,
			start_edge: p.start_edge,
			frame: p.range.in_,
			length: p.rate,
		},
		OakEditOp::SplitClip => EditCommand::SplitClip {
			clip: p.a,
			time_ts: p.in_ts,
		},
		OakEditOp::SplitClipsPreservingLinks => EditCommand::SplitClipsPreservingLinks {
			clips: ids,
			time_ts: p.in_ts,
		},
		OakEditOp::TrimClip => EditCommand::TrimClip {
			clip: p.a,
			new_in_ts: p.in_ts,
			new_out_ts: p.out_ts,
		},
		OakEditOp::RippleTrimClip => EditCommand::RippleTrimClip {
			clip: p.a,
			start_edge: p.start_edge,
			new_frame: p.new_frame,
		},
		OakEditOp::RollEdit => EditCommand::RollEdit {
			track: p.a,
			a: p.b,
			b: p.c,
			new_frame: p.new_frame,
		},
		OakEditOp::SlideClip => EditCommand::SlideClip {
			clip: p.a,
			new_start: p.new_frame,
		},
		OakEditOp::SlipClip => EditCommand::SlipClip {
			clip: p.a,
			new_media_in: p.new_frame,
		},
		OakEditOp::MoveClip => EditCommand::MoveClip {
			clip: p.a,
			new_in_ts: p.new_frame,
		},
		OakEditOp::MoveClipToTrack => EditCommand::MoveClipToTrack {
			clip: p.a,
			track: p.b,
			new_in_ts: p.new_frame,
		},
		OakEditOp::MoveClipWithLinks => EditCommand::MoveClipWithLinks {
			clip: p.a,
			track: (p.b != OAK_NODE_ID_INVALID).then_some(p.b),
			new_in_ts: p.new_frame,
			linked: ids,
		},
		OakEditOp::SetClipsLinked => EditCommand::SetClipsLinked {
			clips: ids,
			linked: p.linked,
		},
		OakEditOp::DeleteClip => EditCommand::DeleteClip { clip: p.a },
		OakEditOp::RippleDeleteClip => EditCommand::RippleDeleteClip { clip: p.a },
		OakEditOp::DeleteArea => EditCommand::DeleteArea {
			in_ts: p.in_ts,
			out_ts: p.out_ts,
			ripple: p.ripple,
		},
		OakEditOp::RollClipEdgeTo => EditCommand::RollClipEdgeTo {
			clip: p.a,
			at_in: p.start_edge,
			frame_ts: p.new_frame,
		},
		OakEditOp::MarkerAdd => EditCommand::MarkerAdd {
			time: p.rate,
			name: text(p.name, p.name_len),
			color: p.kind,
		},
		OakEditOp::MarkerRemove => EditCommand::MarkerRemove { time: p.rate },
		OakEditOp::WorkareaSet => EditCommand::WorkareaSet {
			enabled: p.enabled,
			range: p.range,
		},
		OakEditOp::WorkareaSetUndoable => EditCommand::WorkareaSetUndoable {
			enabled: p.enabled,
			range: p.range,
			old_range: p.old_range,
		},
		OakEditOp::RemoveNode => EditCommand::RemoveNode { node: p.a },
		OakEditOp::Connect => EditCommand::Connect {
			from: p.a,
			to: p.b,
			input_id: text(p.input_id, p.input_id_len),
		},
		OakEditOp::Disconnect => EditCommand::Disconnect {
			to: p.a,
			input_id: text(p.input_id, p.input_id_len),
		},
		OakEditOp::SetContextPosition => EditCommand::SetContextPosition {
			node: p.a,
			context: p.b,
			x: p.x,
			y: p.y,
		},
		OakEditOp::CopyClips => EditCommand::CopyClips { clips: ids },
		OakEditOp::PasteClips => EditCommand::PasteClips {
			playhead_ts: p.in_ts,
		},
		OakEditOp::SetBlocksEnabled => EditCommand::SetBlocksEnabled {
			clips: ids,
			enabled: p.enabled,
		},
		OakEditOp::SetClipsSpeed => EditCommand::SetClipsSpeed {
			clips: ids,
			speed: p.speed,
		},
		OakEditOp::RenameBlocks => EditCommand::RenameBlocks {
			clips: ids,
			name: text(p.name, p.name_len),
		},
		OakEditOp::DuplicateClips => EditCommand::DuplicateClips { clips: ids },
		OakEditOp::PasteClipsInsert => EditCommand::PasteClipsInsert {
			playhead_ts: p.in_ts,
		},
		OakEditOp::NestClips => EditCommand::NestClips { clips: ids },
		OakEditOp::EffectInsert => EditCommand::EffectInsert {
			host: p.a,
			index: p.track_index,
			type_id: text(p.path, p.path_len),
		},
		OakEditOp::EffectRemove => EditCommand::EffectRemove {
			host: p.a,
			effect: p.b,
		},
		OakEditOp::EffectMove => EditCommand::EffectMove {
			host: p.a,
			effect: p.b,
			new_index: p.track_index,
		},
		OakEditOp::EffectSetEnabled => EditCommand::EffectSetEnabled {
			effect: p.a,
			enabled: p.enabled,
		},
		OakEditOp::SetNodeStandardValue => EditCommand::SetNodeStandardValue {
			node: p.a,
			input_id: text(p.input_id, p.input_id_len),
			element: p.element,
			value: p.node_value,
		},
		OakEditOp::UpsertNodeKeyframe => EditCommand::UpsertNodeKeyframe {
			node: p.a,
			input_id: text(p.input_id, p.input_id_len),
			element: p.element,
			time: p.rate,
			value: p.node_value,
			interpolation: p.kind,
			bezier_in: (p.bezier_in_x, p.bezier_in_y),
			bezier_out: (p.bezier_out_x, p.bezier_out_y),
		},
		OakEditOp::RemoveNodeKeyframe => EditCommand::RemoveNodeKeyframe {
			node: p.a,
			input_id: text(p.input_id, p.input_id_len),
			element: p.element,
			time: p.rate,
		},
		OakEditOp::SetNodeValueAtTime => EditCommand::SetNodeValueAtTime {
			node: p.a,
			input_id: text(p.input_id, p.input_id_len),
			element: p.element,
			time: p.rate,
			value: p.node_value,
		},
		OakEditOp::PushNodeButton => EditCommand::PushNodeButton {
			node: p.a,
			input_id: text(p.input_id, p.input_id_len),
		},
	})
}

//! Mapping from safe command variants to operation-specific ABI slots.

use crate::node::NodeId;
use crate::editor::KeyframeInterpolation;
use crate::types::Rational;

use super::super::EditCommand;
use super::super::abi::*;
use super::{CommandArgs, kind_code};

impl CommandArgs {
	pub(super) fn map(&mut self, command: EditCommand) {
		match command {
			EditCommand::AddTrack { kind } => {
				self.op = OakEditOp::AddTrack;
				self.params.kind = kind_code(kind);
			}
			EditCommand::RemoveTrack { track } => self.simple(OakEditOp::RemoveTrack, track),
			EditCommand::SetTrackHeight { track, height } => {
				self.simple(OakEditOp::SetTrackHeight, track);
				self.params.height = height;
			}
			EditCommand::SetTrackMuted { track, muted } => {
				self.simple(OakEditOp::SetTrackMuted, track);
				self.params.enabled = muted;
			}
			EditCommand::SetTrackLocked { track, locked } => {
				self.simple(OakEditOp::SetTrackLocked, track);
				self.params.enabled = locked;
			}
			EditCommand::CreateSequence {
				name,
				width,
				height,
				rate,
				interlaced,
			} => {
				self.op = OakEditOp::CreateSequence;
				self.name(name);
				self.params.kind = width.is_some() as i32;
				self.params.a = width.unwrap_or(0) as u64;
				self.params.b = height.unwrap_or(0) as u64;
				self.params.rate = rate.unwrap_or(Rational::NULL);
				self.params.enabled = interlaced.unwrap_or(false);
			}
			EditCommand::SetSequenceParameters {
				sequence,
				name,
				width,
				height,
				rate,
				interlaced,
			} => {
				self.simple(OakEditOp::SetSequenceParameters, sequence);
				self.name(name);
				self.params.b = width as u64;
				self.params.c = height as u64;
				self.params.rate = rate;
				self.params.enabled = interlaced;
			}
			EditCommand::SetSequencePlayhead { sequence, time } => {
				self.simple(OakEditOp::SetSequencePlayhead, sequence);
				self.params.rate = time;
			}
			EditCommand::CreateFolder { name } => {
				self.op = OakEditOp::CreateFolder;
				self.name(name);
			}
			EditCommand::CreateTextFootage => self.op = OakEditOp::CreateTextFootage,
			EditCommand::ImportFootage { path } => {
				self.op = OakEditOp::ImportFootage;
				self.path(path);
			}
			EditCommand::EnsureSequencesMounted => self.op = OakEditOp::EnsureSequencesMounted,
			EditCommand::ReprobeUnprobedFootage => self.op = OakEditOp::ReprobeUnprobedFootage,
			EditCommand::CreateAdjustmentLayer {
				track,
				in_ts,
				out_ts,
			} => {
				self.simple(OakEditOp::CreateAdjustmentLayer, track);
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
			}
			EditCommand::PlaceFootageClip {
				footage,
				kind,
				track_index,
				in_ts,
				out_ts,
				media_in_ts,
			} => {
				self.simple(OakEditOp::PlaceFootageClip, footage);
				self.params.kind = kind_code(kind);
				self.params.track_index = track_index;
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
				self.params.media_in_ts = media_in_ts;
			}
			EditCommand::PlaceNestedSequenceClip {
				source,
				track_index,
				in_ts,
				out_ts,
			} => {
				self.simple(OakEditOp::PlaceNestedSequenceClip, source);
				self.params.track_index = track_index;
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
			}
			EditCommand::PlaceTextClip {
				text,
				track_index,
				in_ts,
				out_ts,
			} => {
				self.simple(OakEditOp::PlaceTextClip, text);
				self.params.track_index = track_index;
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
			}
			EditCommand::PlaceGeneratorClip {
				generator,
				track_index,
				in_ts,
				out_ts,
			} => {
				self.simple(OakEditOp::PlaceGeneratorClip, generator);
				self.params.track_index = track_index;
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
			}
			EditCommand::PlaceFootageClipsLinked {
				footage,
				placements,
				in_ts,
				out_ts,
				media_in_ts,
			} => {
				self.simple(OakEditOp::PlaceFootageClipsLinked, footage);
				self.placements(placements);
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
				self.params.media_in_ts = media_in_ts;
			}
			EditCommand::AddTransitionAtSeam {
				previous,
				next,
				half,
			} => {
				self.simple(OakEditOp::AddTransitionAtSeam, previous);
				self.params.b = next.raw();
				self.params.rate = half;
			}
			EditCommand::AddTransitionAtEdge {
				clip,
				start_edge,
				length,
			} => {
				self.simple(OakEditOp::AddTransitionAtEdge, clip);
				self.params.start_edge = start_edge;
				self.params.rate = length;
			}
			EditCommand::AddDefaultTransition { clips, half } => {
				self.op = OakEditOp::AddDefaultTransition;
				self.ids(clips);
				self.params.rate = half;
			}
			EditCommand::SetTransitionLength {
				clip,
				start_edge,
				frame,
				length,
			} => {
				self.simple(OakEditOp::SetTransitionLength, clip);
				self.params.start_edge = start_edge;
				self.params.range.in_ = frame;
				self.params.rate = length;
			}
			EditCommand::SplitClip { clip, time_ts } => {
				self.simple(OakEditOp::SplitClip, clip);
				self.params.in_ts = time_ts;
			}
			EditCommand::SplitClipsPreservingLinks { clips, time_ts } => {
				self.op = OakEditOp::SplitClipsPreservingLinks;
				self.ids(clips);
				self.params.in_ts = time_ts;
			}
			EditCommand::TrimClip {
				clip,
				new_in_ts,
				new_out_ts,
			} => {
				self.simple(OakEditOp::TrimClip, clip);
				self.params.in_ts = new_in_ts;
				self.params.out_ts = new_out_ts;
			}
			EditCommand::RippleTrimClip {
				clip,
				start_edge,
				new_frame,
			} => {
				self.simple(OakEditOp::RippleTrimClip, clip);
				self.params.start_edge = start_edge;
				self.params.new_frame = new_frame;
			}
			EditCommand::RollEdit {
				track,
				a,
				b,
				new_frame,
			} => {
				self.simple(OakEditOp::RollEdit, track);
				self.params.b = a.raw();
				self.params.c = b.raw();
				self.params.new_frame = new_frame;
			}
			EditCommand::SlideClip { clip, new_start } => {
				self.simple(OakEditOp::SlideClip, clip);
				self.params.new_frame = new_start;
			}
			EditCommand::SlipClip { clip, new_media_in } => {
				self.simple(OakEditOp::SlipClip, clip);
				self.params.new_frame = new_media_in;
			}
			EditCommand::MoveClip { clip, new_in_ts } => {
				self.simple(OakEditOp::MoveClip, clip);
				self.params.new_frame = new_in_ts;
			}
			EditCommand::MoveClipToTrack {
				clip,
				track,
				new_in_ts,
			} => {
				self.simple(OakEditOp::MoveClipToTrack, clip);
				self.params.b = track.raw();
				self.params.new_frame = new_in_ts;
			}
			EditCommand::MoveClipWithLinks {
				clip,
				track,
				new_in_ts,
				linked,
			} => {
				self.simple(OakEditOp::MoveClipWithLinks, clip);
				self.params.b = track.map_or(invalid_id(), NodeId::raw);
				self.params.new_frame = new_in_ts;
				self.ids(linked);
			}
			EditCommand::SetClipsLinked { clips, linked } => {
				self.op = OakEditOp::SetClipsLinked;
				self.ids(clips);
				self.params.linked = linked;
			}
			EditCommand::DeleteClip { clip } => self.simple(OakEditOp::DeleteClip, clip),
			EditCommand::RippleDeleteClip { clip } => {
				self.simple(OakEditOp::RippleDeleteClip, clip)
			}
			EditCommand::DeleteArea {
				in_ts,
				out_ts,
				ripple,
			} => {
				self.op = OakEditOp::DeleteArea;
				self.params.in_ts = in_ts;
				self.params.out_ts = out_ts;
				self.params.ripple = ripple;
			}
			EditCommand::RollClipEdgeTo {
				clip,
				at_in,
				frame_ts,
			} => {
				self.simple(OakEditOp::RollClipEdgeTo, clip);
				self.params.start_edge = at_in;
				self.params.new_frame = frame_ts;
			}
			EditCommand::MarkerAdd { time, name, color } => {
				self.op = OakEditOp::MarkerAdd;
				self.params.rate = time;
				self.name(name);
				self.params.kind = color;
			}
			EditCommand::MarkerRemove { time } => {
				self.op = OakEditOp::MarkerRemove;
				self.params.rate = time;
			}
			EditCommand::WorkareaSet { enabled, range } => {
				self.op = OakEditOp::WorkareaSet;
				self.params.enabled = enabled;
				self.params.range = range;
			}
			EditCommand::WorkareaSetUndoable {
				enabled,
				range,
				old_range,
			} => {
				self.op = OakEditOp::WorkareaSetUndoable;
				self.params.enabled = enabled;
				self.params.range = range;
				self.params.old_range = old_range;
			}
			EditCommand::RemoveNode { node } => self.simple(OakEditOp::RemoveNode, node),
			EditCommand::Connect { from, to, input_id } => {
				self.simple(OakEditOp::Connect, from);
				self.params.b = to.raw();
				self.input_id(input_id);
			}
			EditCommand::Disconnect { to, input_id } => {
				self.simple(OakEditOp::Disconnect, to);
				self.input_id(input_id);
			}
			EditCommand::SetContextPosition {
				node,
				context,
				x,
				y,
			} => {
				self.simple(OakEditOp::SetContextPosition, node);
				self.params.b = context.raw();
				self.params.x = x;
				self.params.y = y;
			}
			EditCommand::CopyClips { clips } => {
				self.op = OakEditOp::CopyClips;
				self.ids(clips);
			}
			EditCommand::PasteClips { playhead_ts } => {
				self.op = OakEditOp::PasteClips;
				self.params.in_ts = playhead_ts;
			}
			EditCommand::SetBlocksEnabled { clips, enabled } => {
				self.op = OakEditOp::SetBlocksEnabled;
				self.ids(clips);
				self.params.enabled = enabled;
			}
			EditCommand::SetClipsSpeed { clips, speed } => {
				self.op = OakEditOp::SetClipsSpeed;
				self.ids(clips);
				self.params.speed = speed;
			}
			EditCommand::RenameBlocks { clips, name } => {
				self.op = OakEditOp::RenameBlocks;
				self.ids(clips);
				self.name(name);
			}
			EditCommand::DuplicateClips { clips } => {
				self.op = OakEditOp::DuplicateClips;
				self.ids(clips);
			}
			EditCommand::PasteClipsInsert { playhead_ts } => {
				self.op = OakEditOp::PasteClipsInsert;
				self.params.in_ts = playhead_ts;
			}
			EditCommand::NestClips { clips } => {
				self.op = OakEditOp::NestClips;
				self.ids(clips);
			}
			EditCommand::EffectInsert {
				host,
				index,
				type_id,
			} => {
				self.simple(OakEditOp::EffectInsert, host);
				self.params.track_index = index;
				self.path(type_id);
			}
			EditCommand::EffectRemove { host, effect } => {
				self.simple(OakEditOp::EffectRemove, host);
				self.params.b = effect.raw();
			}
			EditCommand::EffectMove {
				host,
				effect,
				new_index,
			} => {
				self.simple(OakEditOp::EffectMove, host);
				self.params.b = effect.raw();
				self.params.track_index = new_index;
			}
			EditCommand::EffectSetEnabled { effect, enabled } => {
				self.simple(OakEditOp::EffectSetEnabled, effect);
				self.params.enabled = enabled;
			}
			EditCommand::SetNodeStandardValue {
				node,
				input_id,
				element,
				value,
			} => {
				self.simple(OakEditOp::SetNodeStandardValue, node);
				self.input_id(input_id);
				self.params.element = element;
				self.params.node_value = value;
			}
			EditCommand::UpsertNodeKeyframe {
				node,
				input_id,
				element,
				time,
				value,
				interpolation,
				bezier_in,
				bezier_out,
			} => {
				self.simple(OakEditOp::UpsertNodeKeyframe, node);
				self.input_id(input_id);
				self.params.element = element;
				self.params.rate = time;
				self.params.node_value = value;
				self.params.kind = match interpolation {
					KeyframeInterpolation::Hold => 0,
					KeyframeInterpolation::Linear => 1,
					KeyframeInterpolation::Bezier => 2,
				};
				(self.params.bezier_in_x, self.params.bezier_in_y) = bezier_in;
				(self.params.bezier_out_x, self.params.bezier_out_y) = bezier_out;
			}
			EditCommand::RemoveNodeKeyframe {
				node,
				input_id,
				element,
				time,
			} => {
				self.simple(OakEditOp::RemoveNodeKeyframe, node);
				self.input_id(input_id);
				self.params.element = element;
				self.params.rate = time;
			}
			EditCommand::SetNodeValueAtTime {
				node,
				input_id,
				element,
				time,
				value,
			} => {
				self.simple(OakEditOp::SetNodeValueAtTime, node);
				self.input_id(input_id);
				self.params.element = element;
				self.params.rate = time;
				self.params.node_value = value;
			}
		}
	}
}

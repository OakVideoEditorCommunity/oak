//! Conversion between strongly typed commands and native ABI buffers.

use crate::editor::{EditorSession, TrackKind};
use crate::node::NodeId;
use crate::types::Rational;

use super::abi::*;
use super::{ClipboardClip, EditCommand, EditResult};

/// Owns every buffer referenced by `params`, keeping FFI pointers valid until
/// the call finishes and retaining output storage for owned-result conversion.
struct CommandArgs {
	op: OakEditOp,
	params: OakEditParams,
	_name: Vec<u8>,
	_path: Vec<u8>,
	_input_id: Vec<u8>,
	_ids: Vec<u64>,
	_placements: Vec<OakTrackPlacement>,
	result_ids: Vec<u64>,
	result_clipboard: Vec<OakClipboardClip>,
}

impl CommandArgs {
	fn new(command: EditCommand) -> Self {
		let mut result = Self {
			op: OakEditOp::AddTrack,
			params: OakEditParams::default(),
			_name: Vec::new(),
			_path: Vec::new(),
			_input_id: Vec::new(),
			_ids: Vec::new(),
			_placements: Vec::new(),
			result_ids: vec![0; 1024],
			result_clipboard: vec![OakClipboardClip::default(); 1024],
		};
		result.params.result_ids = result.result_ids.as_mut_ptr();
		result.params.result_ids_capacity = result.result_ids.len();
		result.params.result_clipboard = result.result_clipboard.as_mut_ptr();
		result.params.result_clipboard_capacity = result.result_clipboard.len();
		result.map(command);
		result
	}

	fn name(&mut self, value: String) {
		self._name = value.into_bytes();
		self.params.name = self._name.as_ptr();
		self.params.name_len = self._name.len();
	}

	fn path(&mut self, value: String) {
		self._path = value.into_bytes();
		self.params.path = self._path.as_ptr();
		self.params.path_len = self._path.len();
	}

	fn input_id(&mut self, value: String) {
		self._input_id = value.into_bytes();
		self.params.input_id = self._input_id.as_ptr();
		self.params.input_id_len = self._input_id.len();
	}

	fn ids(&mut self, value: Vec<NodeId>) {
		self._ids = value.into_iter().map(NodeId::raw).collect();
		self.params.ids = self._ids.as_ptr();
		self.params.ids_len = self._ids.len();
	}

	fn placements(&mut self, value: Vec<(TrackKind, usize)>) {
		self._placements = value
			.into_iter()
			.map(|(kind, index)| OakTrackPlacement {
				kind: kind_code(kind),
				index,
			})
			.collect();
		self.params.placements = self._placements.as_ptr();
		self.params.placements_len = self._placements.len();
	}

	/// Packs variant fields into the opcode-specific slots documented by the ABI.
	fn map(&mut self, command: EditCommand) {
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
			EditCommand::CreateAdjustmentLayer { track, in_ts, out_ts } => {
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
			EditCommand::AddTransitionAtSeam { previous, next, half } => {
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
			EditCommand::RippleDeleteClip { clip } => self.simple(OakEditOp::RippleDeleteClip, clip),
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
			EditCommand::SetContextPosition { node, context, x, y } => {
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
		}
	}

	fn simple(&mut self, op: OakEditOp, a: NodeId) {
		self.op = op;
		self.params.a = a.raw();
	}

	/// Copies bounded engine output into safe, owned IDs and clipboard entries.
	fn into_result(self, raw: OakEditResult) -> EditResult {
		let primary = NodeId::from_raw(raw.primary);
		let primary = primary.is_valid().then_some(primary);
		let rational = (!raw.rational.is_null()).then_some(raw.rational);
		let ids = self.result_ids[..raw.ids_len]
			.iter()
			.map(|&id| NodeId::from_raw(id))
			.collect();
		let clipboard = self.result_clipboard[..raw.clipboard_len]
			.iter()
			.map(|clip| ClipboardClip {
				footage: NodeId::from_raw(clip.footage),
				kind: track_kind(clip.kind),
				track_index: clip.track_index,
				media_in_ts: clip.media_in_ts,
				start_ts: clip.start_ts,
				length_ts: clip.length_ts,
				speed: clip.speed,
			})
			.collect();
		EditResult {
			primary,
			ids,
			value: raw.value,
			rational,
			revision: raw.revision,
			clipboard,
		}
	}
}

fn kind_code(kind: TrackKind) -> i32 {
	match kind {
		TrackKind::Video => 0,
		TrackKind::Audio => 1,
		TrackKind::Subtitle => 2,
	}
}

fn track_kind(raw: i32) -> TrackKind {
	match raw {
		1 => TrackKind::Audio,
		2 => TrackKind::Subtitle,
		_ => TrackKind::Video,
	}
}

impl EditorSession {
	/// Executes exactly one semantic edit and returns owned results.
	/// The engine writes bounded error text; no retry is attempted because a
	/// result-buffer error can occur after the mutation has already run.
	pub fn execute(&self, command: EditCommand) -> Result<EditResult, String> {
		let args = CommandArgs::new(command);
		let mut raw = OakEditResult::default();
		let mut error = [0u8; 4096];
		let code = unsafe {
			oak_editor_edit(
				self.ptr,
				args.op.code(),
				&args.params,
				&mut raw,
				error.as_mut_ptr(),
				error.len(),
			)
		};
		if code != 0 {
			let len = error.iter().position(|&b| b == 0).unwrap_or(error.len());
			let message = String::from_utf8_lossy(&error[..len]);
			return Err(if message.is_empty() {
				format!("editor command failed with code {code}")
			} else {
				message.into_owned()
			});
		}
		Ok(args.into_result(raw))
	}
}

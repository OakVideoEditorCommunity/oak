//! Conversion between strongly typed commands and native ABI buffers.

use crate::editor::{EditorSession, TrackKind};
use crate::node::NodeId;

use super::abi::*;
use super::{ClipboardClip, EditCommand, EditResult};

mod map;

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

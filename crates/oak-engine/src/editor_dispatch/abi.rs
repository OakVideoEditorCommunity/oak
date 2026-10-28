//! Native C ABI for semantic editor commands (no serialization).
//!
//! The SDK has a mirror of every `repr(C)` type in this file. Their field order,
//! integer widths and operation numbers must change in lockstep. New operations
//! should be appended rather than inserted so existing numeric opcodes stay stable.

use crate::codec::{copy_str_out, str_arg};
use crate::editor::EditorSessionHandle;
use crate::node::OAK_NODE_ID_INVALID;

use super::{EditCommand, EditResult, WireClipboardClip, execute};

#[repr(u32)]
#[derive(Clone, Copy, Debug)]
/// Numeric operation selector accepted by [`oak_editor_edit`].
pub enum OakEditOp {
	AddTrack,
	RemoveTrack,
	SetTrackHeight,
	SetTrackMuted,
	SetTrackLocked,
	CreateSequence,
	SetSequenceParameters,
	SetSequencePlayhead,
	CreateFolder,
	CreateTextFootage,
	ImportFootage,
	EnsureSequencesMounted,
	ReprobeUnprobedFootage,
	CreateAdjustmentLayer,
	PlaceFootageClip,
	PlaceNestedSequenceClip,
	PlaceTextClip,
	PlaceGeneratorClip,
	PlaceFootageClipsLinked,
	AddTransitionAtSeam,
	AddTransitionAtEdge,
	AddDefaultTransition,
	SetTransitionLength,
	SplitClip,
	SplitClipsPreservingLinks,
	TrimClip,
	RippleTrimClip,
	RollEdit,
	SlideClip,
	SlipClip,
	MoveClip,
	MoveClipToTrack,
	MoveClipWithLinks,
	SetClipsLinked,
	DeleteClip,
	RippleDeleteClip,
	DeleteArea,
	RollClipEdgeTo,
	MarkerAdd,
	MarkerRemove,
	WorkareaSet,
	WorkareaSetUndoable,
	RemoveNode,
	Connect,
	Disconnect,
	SetContextPosition,
	CopyClips,
	PasteClips,
	SetBlocksEnabled,
	SetClipsSpeed,
	RenameBlocks,
	DuplicateClips,
	PasteClipsInsert,
	NestClips,
}

impl OakEditOp {
	/// Validates an opcode received from outside Rust; unknown values are rejected.
	fn from_u32(value: u32) -> Option<Self> {
		Some(match value {
			0 => Self::AddTrack,
			1 => Self::RemoveTrack,
			2 => Self::SetTrackHeight,
			3 => Self::SetTrackMuted,
			4 => Self::SetTrackLocked,
			5 => Self::CreateSequence,
			6 => Self::SetSequenceParameters,
			7 => Self::SetSequencePlayhead,
			8 => Self::CreateFolder,
			9 => Self::CreateTextFootage,
			10 => Self::ImportFootage,
			11 => Self::EnsureSequencesMounted,
			12 => Self::ReprobeUnprobedFootage,
			13 => Self::CreateAdjustmentLayer,
			14 => Self::PlaceFootageClip,
			15 => Self::PlaceNestedSequenceClip,
			16 => Self::PlaceTextClip,
			17 => Self::PlaceGeneratorClip,
			18 => Self::PlaceFootageClipsLinked,
			19 => Self::AddTransitionAtSeam,
			20 => Self::AddTransitionAtEdge,
			21 => Self::AddDefaultTransition,
			22 => Self::SetTransitionLength,
			23 => Self::SplitClip,
			24 => Self::SplitClipsPreservingLinks,
			25 => Self::TrimClip,
			26 => Self::RippleTrimClip,
			27 => Self::RollEdit,
			28 => Self::SlideClip,
			29 => Self::SlipClip,
			30 => Self::MoveClip,
			31 => Self::MoveClipToTrack,
			32 => Self::MoveClipWithLinks,
			33 => Self::SetClipsLinked,
			34 => Self::DeleteClip,
			35 => Self::RippleDeleteClip,
			36 => Self::DeleteArea,
			37 => Self::RollClipEdgeTo,
			38 => Self::MarkerAdd,
			39 => Self::MarkerRemove,
			40 => Self::WorkareaSet,
			41 => Self::WorkareaSetUndoable,
			42 => Self::RemoveNode,
			43 => Self::Connect,
			44 => Self::Disconnect,
			45 => Self::SetContextPosition,
			46 => Self::CopyClips,
			47 => Self::PasteClips,
			48 => Self::SetBlocksEnabled,
			49 => Self::SetClipsSpeed,
			50 => Self::RenameBlocks,
			51 => Self::DuplicateClips,
			52 => Self::PasteClipsInsert,
			53 => Self::NestClips,
			_ => return None,
		})
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// One requested track in a linked audio/video placement.
pub struct OakTrackPlacement {
	pub kind: i32,
	pub index: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// Operation-specific parameter slots shared by every edit opcode.
///
/// `a`–`d` usually carry raw node identities; `command` below is the canonical
/// slot-to-field mapping. Input strings are UTF-8 pointer/length pairs, not
/// NUL-terminated strings. All input and output buffers are borrowed only for
/// the duration of the FFI call. Timestamp slots use sequence frame units.
pub struct OakEditParams {
	// Generic identities or integer parameters.
	pub a: u64,
	pub b: u64,
	pub c: u64,
	pub d: u64,
	// Timeline positions in the current sequence's frame timebase.
	pub in_ts: i64,
	pub out_ts: i64,
	pub media_in_ts: i64,
	pub new_frame: i64,
	// Shared discriminants, flags and scalar values.
	pub kind: i32,
	pub track_index: usize,
	pub enabled: bool,
	pub start_edge: bool,
	pub linked: bool,
	pub ripple: bool,
	pub height: f64,
	pub speed: f64,
	pub x: f64,
	pub y: f64,
	// Rational times and ranges used by markers, transitions and work areas.
	pub rate: super::WireRational,
	pub range: super::WireTimeRange,
	pub old_range: super::WireTimeRange,
	// Borrowed UTF-8 strings and arrays, each with an explicit element/byte count.
	pub name: *const u8,
	pub name_len: usize,
	pub path: *const u8,
	pub path_len: usize,
	pub input_id: *const u8,
	pub input_id_len: usize,
	pub ids: *const u64,
	pub ids_len: usize,
	pub placements: *const OakTrackPlacement,
	pub placements_len: usize,
	// Caller-owned result storage. Capacities are element counts, not bytes.
	pub result_ids: *mut u64,
	pub result_ids_capacity: usize,
	pub result_clipboard: *mut WireClipboardClip,
	pub result_clipboard_capacity: usize,
}

impl Default for OakEditParams {
	fn default() -> Self {
		Self {
			a: 0,
			b: 0,
			c: 0,
			d: 0,
			in_ts: 0,
			out_ts: 0,
			media_in_ts: 0,
			new_frame: 0,
			kind: 0,
			track_index: 0,
			enabled: false,
			start_edge: false,
			linked: false,
			ripple: false,
			height: 0.0,
			speed: 0.0,
			x: 0.0,
			y: 0.0,
			rate: Default::default(),
			range: Default::default(),
			old_range: Default::default(),
			name: std::ptr::null(),
			name_len: 0,
			path: std::ptr::null(),
			path_len: 0,
			input_id: std::ptr::null(),
			input_id_len: 0,
			ids: std::ptr::null(),
			ids_len: 0,
			placements: std::ptr::null(),
			placements_len: 0,
			result_ids: std::ptr::null_mut(),
			result_ids_capacity: 0,
			result_clipboard: std::ptr::null_mut(),
			result_clipboard_capacity: 0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// Fixed-size portion of a command result; variable arrays go into `OakEditParams`.
pub struct OakEditResult {
	pub primary: u64,
	pub value: i64,
	pub rational: super::WireRational,
	pub revision: i64,
	pub ids_len: usize,
	pub clipboard_len: usize,
}

/// Copies an optional UTF-8 byte slice into an owned string for command dispatch.
fn text(ptr: *const u8, len: usize) -> String {
	unsafe { str_arg(ptr, len) }.unwrap_or_default().to_string()
}

/// Decodes borrowed ABI storage into an owned command before executing it.
/// This keeps no caller pointers in the domain layer and validates list lengths
/// indirectly through the resulting command's domain checks.
fn command(op: OakEditOp, p: &OakEditParams) -> Result<EditCommand, String> {
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
		OakEditOp::SetTrackHeight => EditCommand::SetTrackHeight { track: p.a, height: p.height },
		OakEditOp::SetTrackMuted => EditCommand::SetTrackMuted { track: p.a, muted: p.enabled },
		OakEditOp::SetTrackLocked => EditCommand::SetTrackLocked { track: p.a, locked: p.enabled },
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
		OakEditOp::SetSequencePlayhead => EditCommand::SetSequencePlayhead { sequence: p.a, time: p.rate },
		OakEditOp::CreateFolder => EditCommand::CreateFolder { name: text(p.name, p.name_len) },
		OakEditOp::CreateTextFootage => EditCommand::CreateTextFootage,
		OakEditOp::ImportFootage => EditCommand::ImportFootage { path: text(p.path, p.path_len) },
		OakEditOp::EnsureSequencesMounted => EditCommand::EnsureSequencesMounted,
		OakEditOp::ReprobeUnprobedFootage => EditCommand::ReprobeUnprobedFootage,
		OakEditOp::CreateAdjustmentLayer => EditCommand::CreateAdjustmentLayer { track: p.a, in_ts: p.in_ts, out_ts: p.out_ts },
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
		OakEditOp::AddTransitionAtSeam => EditCommand::AddTransitionAtSeam { previous: p.a, next: p.b, half: p.rate },
		OakEditOp::AddTransitionAtEdge => EditCommand::AddTransitionAtEdge { clip: p.a, start_edge: p.start_edge, length: p.rate },
		OakEditOp::AddDefaultTransition => EditCommand::AddDefaultTransition { clips: ids, half: p.rate },
		OakEditOp::SetTransitionLength => EditCommand::SetTransitionLength {
			clip: p.a,
			start_edge: p.start_edge,
			frame: p.range.in_,
			length: p.rate,
		},
		OakEditOp::SplitClip => EditCommand::SplitClip { clip: p.a, time_ts: p.in_ts },
		OakEditOp::SplitClipsPreservingLinks => EditCommand::SplitClipsPreservingLinks { clips: ids, time_ts: p.in_ts },
		OakEditOp::TrimClip => EditCommand::TrimClip { clip: p.a, new_in_ts: p.in_ts, new_out_ts: p.out_ts },
		OakEditOp::RippleTrimClip => EditCommand::RippleTrimClip { clip: p.a, start_edge: p.start_edge, new_frame: p.new_frame },
		OakEditOp::RollEdit => EditCommand::RollEdit { track: p.a, a: p.b, b: p.c, new_frame: p.new_frame },
		OakEditOp::SlideClip => EditCommand::SlideClip { clip: p.a, new_start: p.new_frame },
		OakEditOp::SlipClip => EditCommand::SlipClip { clip: p.a, new_media_in: p.new_frame },
		OakEditOp::MoveClip => EditCommand::MoveClip { clip: p.a, new_in_ts: p.new_frame },
		OakEditOp::MoveClipToTrack => EditCommand::MoveClipToTrack { clip: p.a, track: p.b, new_in_ts: p.new_frame },
		OakEditOp::MoveClipWithLinks => EditCommand::MoveClipWithLinks {
			clip: p.a,
			track: (p.b != OAK_NODE_ID_INVALID).then_some(p.b),
			new_in_ts: p.new_frame,
			linked: ids,
		},
		OakEditOp::SetClipsLinked => EditCommand::SetClipsLinked { clips: ids, linked: p.linked },
		OakEditOp::DeleteClip => EditCommand::DeleteClip { clip: p.a },
		OakEditOp::RippleDeleteClip => EditCommand::RippleDeleteClip { clip: p.a },
		OakEditOp::DeleteArea => EditCommand::DeleteArea { in_ts: p.in_ts, out_ts: p.out_ts, ripple: p.ripple },
		OakEditOp::RollClipEdgeTo => EditCommand::RollClipEdgeTo { clip: p.a, at_in: p.start_edge, frame_ts: p.new_frame },
		OakEditOp::MarkerAdd => EditCommand::MarkerAdd { time: p.rate, name: text(p.name, p.name_len), color: p.kind },
		OakEditOp::MarkerRemove => EditCommand::MarkerRemove { time: p.rate },
		OakEditOp::WorkareaSet => EditCommand::WorkareaSet { enabled: p.enabled, range: p.range },
		OakEditOp::WorkareaSetUndoable => EditCommand::WorkareaSetUndoable { enabled: p.enabled, range: p.range, old_range: p.old_range },
		OakEditOp::RemoveNode => EditCommand::RemoveNode { node: p.a },
		OakEditOp::Connect => EditCommand::Connect { from: p.a, to: p.b, input_id: text(p.input_id, p.input_id_len) },
		OakEditOp::Disconnect => EditCommand::Disconnect { to: p.a, input_id: text(p.input_id, p.input_id_len) },
		OakEditOp::SetContextPosition => EditCommand::SetContextPosition { node: p.a, context: p.b, x: p.x, y: p.y },
		OakEditOp::CopyClips => EditCommand::CopyClips { clips: ids },
		OakEditOp::PasteClips => EditCommand::PasteClips { playhead_ts: p.in_ts },
		OakEditOp::SetBlocksEnabled => EditCommand::SetBlocksEnabled { clips: ids, enabled: p.enabled },
		OakEditOp::SetClipsSpeed => EditCommand::SetClipsSpeed { clips: ids, speed: p.speed },
		OakEditOp::RenameBlocks => EditCommand::RenameBlocks { clips: ids, name: text(p.name, p.name_len) },
		OakEditOp::DuplicateClips => EditCommand::DuplicateClips { clips: ids },
		OakEditOp::PasteClipsInsert => EditCommand::PasteClipsInsert { playhead_ts: p.in_ts },
		OakEditOp::NestClips => EditCommand::NestClips { clips: ids },
	})
}

/// Copies a successful result without ever rerunning the mutation.
/// Returns `-2` when the result pointer or caller capacity is insufficient.
fn write_result(result: EditResult, p: &OakEditParams, out: *mut OakEditResult) -> i32 {
	if out.is_null() {
		return -2;
	}
	if result.ids.len() > p.result_ids_capacity || result.clipboard.len() > p.result_clipboard_capacity {
		return -2;
	}
	// Execution has already happened at this point. In particular, callers must
	// not respond to `-2` by retrying blindly, or they could apply the edit twice.
	if !result.ids.is_empty() {
		unsafe { std::ptr::copy_nonoverlapping(result.ids.as_ptr(), p.result_ids, result.ids.len()) };
	}
	if !result.clipboard.is_empty() {
		unsafe {
			std::ptr::copy_nonoverlapping(result.clipboard.as_ptr(), p.result_clipboard, result.clipboard.len())
		};
	}
	unsafe {
		*out = OakEditResult {
			primary: result.primary.unwrap_or(OAK_NODE_ID_INVALID),
			value: result.value,
			rational: result.rational.unwrap_or_default(),
			revision: oak_undo::global::index().unwrap_or(0),
			ids_len: result.ids.len(),
			clipboard_len: result.clipboard.len(),
		}
	};
	0
}

/// Executes one semantic edit. All data crosses as native values/buffers.
///
/// Returns `0` on success, `-1` for an invalid request or domain error, and `-2`
/// for result-output failure. Error text is written as a bounded NUL-terminated
/// UTF-8 string when an error buffer is supplied.
///
/// # Safety
/// `session` and `params` must be valid for the call. Every non-null pointer in
/// `params` must reference its declared length/capacity, output buffers must be
/// writable and non-overlapping, and `result`/`error` must obey the same rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_edit(
	session: *mut EditorSessionHandle,
	op: u32,
	params: *const OakEditParams,
	result: *mut OakEditResult,
	error: *mut u8,
	error_len: usize,
) -> i32 {
	if session.is_null() || params.is_null() {
		return -1;
	}
	let Some(op) = OakEditOp::from_u32(op) else {
		copy_str_out("unknown edit operation", error, error_len);
		return -1;
	};
	let params = unsafe { &*params };
	let command = match command(op, params) {
		Ok(command) => command,
		Err(error_text) => {
			copy_str_out(&error_text, error, error_len);
			return -1;
		}
	};
	let mut session = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	match execute(&mut session, command) {
		Ok(edit_result) => write_result(edit_result, params, result),
		Err(error_text) => {
			copy_str_out(&error_text, error, error_len);
			-1
		}
	}
}

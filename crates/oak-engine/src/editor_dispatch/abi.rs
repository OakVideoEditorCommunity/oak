//! Native C ABI for semantic editor commands (no serialization).
//!
//! The SDK has a mirror of every `repr(C)` type in this file. Their field order,
//! integer widths and operation numbers must change in lockstep. New operations
//! should be appended rather than inserted so existing numeric opcodes stay stable.

use crate::codec::copy_str_out;
use crate::editor::EditorSessionHandle;
use crate::node::OAK_NODE_ID_INVALID;

use super::{EditResult, WireClipboardClip, execute};

mod command;
use command::command;

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
	EffectInsert,
	EffectRemove,
	EffectMove,
	EffectSetEnabled,
	SetNodeStandardValue,
	UpsertNodeKeyframe,
	RemoveNodeKeyframe,
	SetNodeValueAtTime,
	PushNodeButton,
	SetNodeTextValue,
	ReplaceFootage,
	DeleteEntry,
	SetFootageProxyParams,
	ClearFootageProxyParams,
	CreateSequenceFromFootage,
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
			54 => Self::EffectInsert,
			55 => Self::EffectRemove,
			56 => Self::EffectMove,
			57 => Self::EffectSetEnabled,
			58 => Self::SetNodeStandardValue,
			59 => Self::UpsertNodeKeyframe,
			60 => Self::RemoveNodeKeyframe,
			61 => Self::SetNodeValueAtTime,
			62 => Self::PushNodeButton,
			63 => Self::SetNodeTextValue,
			64 => Self::ReplaceFootage,
			65 => Self::DeleteEntry,
			66 => Self::SetFootageProxyParams,
			67 => Self::ClearFootageProxyParams,
			68 => Self::CreateSequenceFromFootage,
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
#[derive(Clone, Debug)]
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
	// Effect parameter element index and POD value used by SetNodeStandardValue.
	pub element: i32,
	pub node_value: oak_node::value::OakNodeValue,
	// Bezier handle offsets used by UpsertNodeKeyframe.
	pub bezier_in_x: f64,
	pub bezier_in_y: f64,
	pub bezier_out_x: f64,
	pub bezier_out_y: f64,
	// Custom proxy generation parameters used by SetFootageProxyParams.
	pub proxy_params: oak_codec::proxymanager::ProxyParams,
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
			element: 0,
			node_value: oak_node::value::OakNodeValue::none(),
			bezier_in_x: 0.0,
			bezier_in_y: 0.0,
			bezier_out_x: 0.0,
			bezier_out_y: 0.0,
			proxy_params: oak_codec::proxymanager::ProxyParams::default(),
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

/// Copies a successful result without ever rerunning the mutation.
/// Returns `-2` when the result pointer or caller capacity is insufficient.
fn write_result(result: EditResult, p: &OakEditParams, out: *mut OakEditResult) -> i32 {
	if out.is_null() {
		return -2;
	}
	if result.ids.len() > p.result_ids_capacity
		|| result.clipboard.len() > p.result_clipboard_capacity
	{
		return -2;
	}
	// Execution has already happened at this point. In particular, callers must
	// not respond to `-2` by retrying blindly, or they could apply the edit twice.
	if !result.ids.is_empty() {
		unsafe {
			std::ptr::copy_nonoverlapping(result.ids.as_ptr(), p.result_ids, result.ids.len())
		};
	}
	if !result.clipboard.is_empty() {
		unsafe {
			std::ptr::copy_nonoverlapping(
				result.clipboard.as_ptr(),
				p.result_clipboard,
				result.clipboard.len(),
			)
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
	let mut session_guard = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (executed, code) = match execute(&mut session_guard, command) {
		Ok(edit_result) => (true, write_result(edit_result, params, result)),
		Err(error_text) => {
			copy_str_out(&error_text, error, error_len);
			(false, -1)
		}
	};
	drop(session_guard);
	if executed {
		crate::editor::notify::notify(unsafe { &*session }, op as u32);
	}
	code
}

//! Native ABI mirrors used only inside the safe edit wrapper.
//!
//! These layouts mirror `oak_engine::editor_dispatch::abi` exactly. They are
//! crate-private so safe SDK users never handle operation-specific pointer slots.
//! Keep opcode order and every field synchronized with the engine definitions.

use crate::node::{NodeId, NodeValue};
use crate::types::{Rational, TimeRange};

#[repr(u32)]
#[derive(Clone, Copy, Debug)]
/// Numeric operation selector; discriminants are part of the native ABI.
pub(crate) enum OakEditOp {
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
}

impl OakEditOp {
	/// Returns the stable integer expected by the engine dispatcher.
	pub(crate) fn code(self) -> u32 {
		self as u32
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// Native `(track kind, track index)` pair used by linked placement.
pub(crate) struct OakTrackPlacement {
	pub kind: i32,
	pub index: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// Native clipboard entry; timestamps use the source sequence's frame timebase.
pub(crate) struct OakClipboardClip {
	pub footage: u64,
	pub kind: i32,
	pub track_index: usize,
	pub media_in_ts: i64,
	pub start_ts: i64,
	pub length_ts: i64,
	pub speed: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// Generic operation parameters and borrowed input/output buffers.
///
/// Only the slots documented by the selected opcode are read. String lengths
/// are bytes, array lengths/capacities are elements, and pointers remain valid
/// until `oak_editor_edit` returns.
pub(crate) struct OakEditParams {
	pub a: u64,
	pub b: u64,
	pub c: u64,
	pub d: u64,
	pub in_ts: i64,
	pub out_ts: i64,
	pub media_in_ts: i64,
	pub new_frame: i64,
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
	pub rate: Rational,
	pub range: TimeRange,
	pub old_range: TimeRange,
	pub element: i32,
	pub node_value: NodeValue,
	pub bezier_in_x: f64,
	pub bezier_in_y: f64,
	pub bezier_out_x: f64,
	pub bezier_out_y: f64,
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
	pub result_ids: *mut u64,
	pub result_ids_capacity: usize,
	pub result_clipboard: *mut OakClipboardClip,
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
			rate: Rational::NULL,
			range: TimeRange::default(),
			old_range: TimeRange::default(),
			element: 0,
			node_value: NodeValue::default(),
			bezier_in_x: 0.0,
			bezier_in_y: 0.0,
			bezier_out_x: 0.0,
			bezier_out_y: 0.0,
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
/// Fixed-size result; variable arrays are copied into `OakEditParams` buffers.
pub(crate) struct OakEditResult {
	pub primary: u64,
	pub value: i64,
	pub rational: Rational,
	pub revision: i64,
	pub ids_len: usize,
	pub clipboard_len: usize,
}

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	/// Executes the operation once. Returns 0, or a negative engine error code.
	pub(crate) fn oak_editor_edit(
		session: *mut std::ffi::c_void,
		op: u32,
		params: *const OakEditParams,
		result: *mut OakEditResult,
		error: *mut u8,
		error_len: usize,
	) -> i32;
}

/// Sentinel written by the engine when a result has no primary node.
pub(crate) fn invalid_id() -> u64 {
	NodeId::INVALID.raw()
}

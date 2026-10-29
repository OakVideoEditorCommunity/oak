//! Safe wrapper over the engine's document-model C ABI (`oak_node_*`,
//! `oak_project_*`, `oak_undo_*`).
//!
//! Nodes are addressed by [`NodeId`] tokens (u64); their lifetime is the
//! project's, no handles to manage. The project itself is a refcounted
//! [`Project`] handle; plugins read the host's open project through
//! [`active_project`].

use std::ffi::c_void;

use crate::codec::string_out;
use crate::types::Rational;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_node_id_invalid() -> u64;
	fn oak_node_id_is_valid(id: u64) -> bool;

	fn oak_project_new() -> *mut c_void;
	fn oak_project_set_active(project: *const c_void);
	fn oak_project_active() -> *mut c_void;
	fn oak_project_into_storage_handle(project: *mut c_void)
	-> crate::editor::StorageProjectHandle;
	fn oak_project_name(project: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_project_filename(project: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_project_pretty_filename(project: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_project_modified(project: *const c_void) -> bool;
	fn oak_project_root(project: *const c_void) -> u64;
	fn oak_project_add_ref(project: *const c_void);
	fn oak_project_release(project: *const c_void);
	fn oak_project_uuid(project: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_project_setting(
		project: *const c_void,
		key: *const u8,
		key_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_project_set_setting(
		project: *const c_void,
		key: *const u8,
		key_len: usize,
		value: *const u8,
		value_len: usize,
	) -> bool;
	fn oak_project_cache_location(
		project: *const c_void,
		setting: *mut i32,
		path: *mut u8,
		path_len: usize,
	) -> i32;
	fn oak_project_set_cache_location(
		project: *const c_void,
		setting: i32,
		path: *const u8,
		path_len: usize,
	) -> bool;

	fn oak_node_type_id(project: *const c_void, id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_node_kind(project: *const c_void, id: u64) -> i32;

	fn oak_folder_child_count(project: *const c_void, id: u64) -> i32;
	fn oak_folder_child_at(project: *const c_void, id: u64, index: i32) -> u64;

	fn oak_footage_filename(project: *const c_void, id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_footage_total_stream_count(project: *const c_void, id: u64) -> i32;
	fn oak_footage_video_stream_count(project: *const c_void, id: u64) -> i32;
	fn oak_footage_audio_stream_count(project: *const c_void, id: u64) -> i32;
	fn oak_footage_subtitle_stream_count(project: *const c_void, id: u64) -> i32;
	fn oak_footage_duration(project: *const c_void, id: u64) -> Rational;
	fn oak_footage_video_length(project: *const c_void, id: u64) -> Rational;
	fn oak_footage_video_params(
		project: *const c_void,
		id: u64,
		ordinal: i32,
		out: *mut StreamVideoParams,
	) -> bool;
	fn oak_footage_audio_params(
		project: *const c_void,
		id: u64,
		ordinal: i32,
		out: *mut StreamAudioParams,
	) -> bool;
	fn oak_footage_custom_proxy_params(
		project: *mut c_void,
		id: u64,
		out: *mut crate::codec::ProxyParams,
	) -> bool;
	fn oak_footage_effective_proxy_params(
		project: *mut c_void,
		id: u64,
		out: *mut crate::codec::ProxyParams,
	) -> bool;
	fn oak_footage_source_start_time(
		project: *const c_void,
		footage: u64,
		out: *mut Rational,
	) -> bool;

	fn oak_sequence_track_list_count(project: *const c_void, id: u64) -> i32;
	fn oak_sequence_track_list_at(project: *const c_void, id: u64, index: i32) -> u64;
	fn oak_sequence_playhead(project: *const c_void, id: u64) -> Rational;
	fn oak_sequence_length(project: *const c_void, id: u64) -> Rational;
	fn oak_sequence_frame_rate(project: *const c_void, id: u64) -> Rational;
	fn oak_sequence_geometry(project: *const c_void, id: u64, width: *mut i32, height: *mut i32) -> bool;
	fn oak_sequence_interlaced(project: *const c_void, id: u64) -> i32;
	fn oak_sequence_set_video_params(
		project: *const c_void,
		id: u64,
		width: i32,
		height: i32,
		frame_rate_num: i32,
		frame_rate_den: i32,
	) -> bool;
	fn oak_node_label(project: *const c_void, id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_node_set_label(project: *const c_void, id: u64, label: *const u8, label_len: usize) -> bool;
	fn oak_tracklist_kind(project: *const c_void, id: u64) -> i32;
	fn oak_tracklist_track_count(project: *const c_void, id: u64) -> i32;
	fn oak_tracklist_track_at(project: *const c_void, id: u64, index: i32) -> u64;
	fn oak_track_kind(project: *const c_void, id: u64) -> i32;
	fn oak_track_height(project: *const c_void, id: u64) -> f64;
	fn oak_track_muted(project: *const c_void, id: u64) -> bool;
	fn oak_track_locked(project: *const c_void, id: u64) -> bool;
	fn oak_track_block_count(project: *const c_void, id: u64) -> i32;
	fn oak_track_block_at(project: *const c_void, id: u64, index: i32) -> u64;
	fn oak_track_block_containing_time(project: *const c_void, id: u64, time: Rational) -> u64;
	fn oak_track_length(project: *const c_void, id: u64) -> Rational;

	fn oak_block_in(project: *const c_void, id: u64) -> Rational;
	fn oak_block_out(project: *const c_void, id: u64) -> Rational;
	fn oak_block_length(project: *const c_void, id: u64) -> Rational;
	fn oak_block_media_in(project: *const c_void, id: u64) -> Rational;
	fn oak_block_media_out(project: *const c_void, id: u64) -> Rational;
	fn oak_block_enabled(project: *const c_void, id: u64) -> bool;

	fn oak_node_standard_value(
		project: *const c_void,
		id: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
		out: *mut NodeValue,
	) -> bool;
	fn oak_node_value_at_time(
		project: *const c_void,
		id: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
		time: Rational,
		out: *mut NodeValue,
	) -> bool;

	fn oak_undo_group_begin(name: *const u8, name_len: usize) -> bool;
	fn oak_undo_group_end() -> bool;
	fn oak_undo_group_abort() -> bool;
	fn oak_undo_undo() -> bool;
	fn oak_undo_redo() -> bool;

	fn oak_footage_import(
		project: *const c_void,
		filename: *const u8,
		filename_len: usize,
	) -> u64;
	fn oak_folder_create(
		project: *const c_void,
		parent: u64,
		name: *const u8,
		name_len: usize,
	) -> u64;
	fn oak_node_remove(project: *const c_void, id: u64) -> bool;
	fn oak_sequence_create(project: *const c_void, name: *const u8, name_len: usize) -> u64;
	fn oak_track_add(project: *const c_void, sequence: u64, kind: i32) -> u64;
	fn oak_block_create_clip(project: *const c_void) -> u64;
	fn oak_block_create_gap(project: *const c_void) -> u64;
	fn oak_block_create_transition(project: *const c_void) -> u64;
	fn oak_block_create_adjustment(project: *const c_void) -> u64;
	fn oak_track_append_block(project: *const c_void, track: u64, block: u64) -> bool;
	fn oak_track_insert_block_after(
		project: *const c_void,
		track: u64,
		block: u64,
		before: u64,
	) -> bool;
	fn oak_track_ripple_remove_block(project: *const c_void, track: u64, block: u64) -> bool;
	fn oak_track_replace_block(project: *const c_void, track: u64, old: u64, new: u64) -> bool;
	fn oak_block_set_length(project: *const c_void, block: u64, value: Rational) -> bool;
	fn oak_block_set_length_and_media_in(
		project: *const c_void,
		block: u64,
		value: Rational,
	) -> bool;
	fn oak_block_set_media_in(project: *const c_void, block: u64, value: Rational) -> bool;
	fn oak_node_set_standard_value(
		project: *const c_void,
		id: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
		value: NodeValue,
	) -> bool;
	fn oak_node_connect(
		project: *const c_void,
		from: u64,
		to: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
	) -> bool;
	fn oak_node_disconnect(
		project: *const c_void,
		to: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
	) -> bool;

	fn oak_project_save(project: *const c_void, path: *const u8, path_len: usize) -> bool;
	fn oak_project_load(path: *const u8, path_len: usize) -> *mut c_void;
	fn oak_project_set_filename(project: *const c_void, path: *const u8, path_len: usize) -> bool;
	fn oak_project_set_working_color_space(
		project: *const c_void,
		setting: *const u8,
		setting_len: usize,
	) -> bool;
	fn oak_project_set_output_color_spec(
		project: *const c_void,
		gamut: *const u8,
		gamut_len: usize,
		transfer: *const u8,
		transfer_len: usize,
	) -> bool;
	fn oak_project_deep_copy(project: *const c_void) -> *mut c_void;

	fn oak_undo_can_undo() -> bool;
	fn oak_undo_can_redo() -> bool;
	fn oak_undo_count() -> i64;
	fn oak_undo_index() -> i64;
	fn oak_undo_jump(index: i64) -> bool;
	fn oak_undo_clear() -> bool;
	fn oak_undo_command_name(row: i64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_undo_command_done(row: i64) -> i32;
	fn oak_undo_add_observer(cb: Option<UndoObserverFn>, userdata: *mut c_void);
}

/// A node identity token (packed generational id). `INVALID` names no
/// node; ids are stable within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

impl NodeId {
	/// The null token.
	pub const INVALID: NodeId = NodeId(u32::MAX as u64);

	/// Whether this token names a real node slot.
	pub fn is_valid(self) -> bool {
		unsafe { oak_node_id_is_valid(self.0) }
	}

	/// Constructs a packed node token from its raw representation.
	/// This performs no graph lookup; call `is_valid` for untrusted IDs.
	pub fn from_raw(raw: u64) -> NodeId {
		NodeId(raw)
	}

	/// The raw token (crate-internal consumers: the render ABI).
	/// Returns the packed raw representation used by SDK DTOs and commands.
	pub fn raw(self) -> u64 {
		self.0
	}
}

/// A node's coarse kind.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
	Folder = 1,
	Footage = 2,
	Sequence = 3,
	Track = 4,
	Clip = 5,
	Gap = 6,
	Transition = 7,
	Adjustment = 8,
	TrackList = 9,
}

impl NodeKind {
	fn from_i32(v: i32) -> Option<NodeKind> {
		match v {
			1 => Some(NodeKind::Folder),
			2 => Some(NodeKind::Footage),
			3 => Some(NodeKind::Sequence),
			4 => Some(NodeKind::Track),
			5 => Some(NodeKind::Clip),
			6 => Some(NodeKind::Gap),
			7 => Some(NodeKind::Transition),
			8 => Some(NodeKind::Adjustment),
			9 => Some(NodeKind::TrackList),
			_ => None,
		}
	}
}

/// A video stream's parameters as carried by a footage node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamVideoParams {
	pub width: i32,
	pub height: i32,
	pub frame_rate: Rational,
	pub pixel_format: i32,
	pub channels: i32,
	pub interlaced: i32,
}

/// An audio stream's parameters as carried by a footage node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamAudioParams {
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub format: i32,
}

/// A node parameter value (POD; layout matches the engine's
/// `OakNodeValue`). String-typed values are not bridged.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeValue {
	/// `NodeValueKind` discriminant (0 = none … 9 = string).
	pub kind: i32,
	/// INT/COMBO value, BOOL 0/1, RATIONAL numerator.
	pub num: i64,
	/// RATIONAL denominator.
	pub den: i64,
	/// FLOAT f[0]; VEC2/3/4 f[0..n-1]; COLOR r,g,b,a.
	pub f: [f64; 4],
}

/// An open project. `Clone` adds a reference, `Drop` releases.
pub struct Project {
	ptr: *mut c_void,
}

unsafe impl Send for Project {}
unsafe impl Sync for Project {}

impl Project {
	/// Converts this owned handle into the storage migration representation used
	/// by legacy hosts. The same live Arc is retained; this handle is consumed.
	pub fn into_storage_project_handle(self) -> crate::editor::StorageProjectHandle {
		let handle = unsafe { oak_project_into_storage_handle(self.ptr) };
		std::mem::forget(self);
		handle
	}
	/// A new, initialized project (empty graph with a root folder).
	pub fn new() -> Self {
		Self {
			ptr: unsafe { oak_project_new() },
		}
	}

	/// Wraps a raw engine handle (crate-internal: the storage ABI hands
	/// projects back this way). The pointer takes over one reference.
	pub(crate) fn from_raw(ptr: *mut c_void) -> Self {
		Self { ptr }
	}

	/// Borrows the raw engine handle (crate-internal: storage/render
	/// calls take it as an argument).
	pub(crate) fn as_raw(&self) -> *mut c_void {
		self.ptr
	}

	/// Transfers the raw reference to another SDK handle without dropping it.
	pub(crate) fn into_raw(self) -> *mut c_void {
		let ptr = self.ptr;
		std::mem::forget(self);
		ptr
	}

	/// The project display name.
	pub fn name(&self) -> String {
		string_out(|buf, len| unsafe { oak_project_name(self.ptr, buf, len) }).unwrap_or_default()
	}

	/// The project file path ("" when unsaved).
	pub fn filename(&self) -> String {
		string_out(|buf, len| unsafe { oak_project_filename(self.ptr, buf, len) })
			.unwrap_or_default()
	}

	/// The display-friendly file path.
	pub fn pretty_filename(&self) -> String {
		string_out(|buf, len| unsafe { oak_project_pretty_filename(self.ptr, buf, len) })
			.unwrap_or_default()
	}

	/// Whether the project has unsaved changes.
	pub fn modified(&self) -> bool {
		unsafe { oak_project_modified(self.ptr) }
	}

	/// The stable uuid used by the project library.
	pub fn uuid(&self) -> String {
		string_out(|buf, len| unsafe { oak_project_uuid(self.ptr, buf, len) }).unwrap_or_default()
	}

	/// Reads a persisted project setting.
	pub fn setting(&self, key: &str) -> Option<String> {
		string_out(|buf, len| unsafe {
			oak_project_setting(self.ptr, key.as_ptr(), key.len(), buf, len)
		})
	}

	/// Writes a persisted project setting and marks the project modified.
	pub fn set_setting(&self, key: &str, value: &str) -> bool {
		unsafe {
			oak_project_set_setting(
				self.ptr,
				key.as_ptr(),
				key.len(),
				value.as_ptr(),
				value.len(),
			)
		}
	}

	/// `(cache mode, custom path)`; mode 2 uses the custom path.
	pub fn cache_location(&self) -> (i32, String) {
		let mode = std::cell::Cell::new(0);
		let path = string_out(|buf, len| unsafe {
			oak_project_cache_location(self.ptr, mode.as_ptr(), buf, len)
		})
		.unwrap_or_default();
		(mode.get(), path)
	}

	/// Writes the cache policy and marks the project modified.
	pub fn set_cache_location(&self, mode: i32, path: &str) -> bool {
		unsafe {
			oak_project_set_cache_location(self.ptr, mode, path.as_ptr(), path.len())
		}
	}

	/// The root folder of the bin tree.
	pub fn root(&self) -> NodeId {
		NodeId::from_raw(unsafe { oak_project_root(self.ptr) })
	}

	// -- generic node queries ------------------------------------------

	/// The node's coarse kind (`None` for unknown/invalid).
	pub fn node_kind(&self, id: NodeId) -> Option<NodeKind> {
		NodeKind::from_i32(unsafe { oak_node_kind(self.ptr, id.0) })
	}

	/// The node's full type id string.
	pub fn node_type_id(&self, id: NodeId) -> Option<String> {
		string_out(|buf, len| unsafe { oak_node_type_id(self.ptr, id.0, buf, len) })
	}

	// -- folder --------------------------------------------------------

	/// Number of children in a folder.
	pub fn folder_child_count(&self, folder: NodeId) -> usize {
		unsafe { oak_folder_child_count(self.ptr, folder.0) as usize }
	}

	/// The `index`-th child of a folder (`NodeId::INVALID` when out of
	/// range).
	pub fn folder_child_at(&self, folder: NodeId, index: usize) -> NodeId {
		NodeId::from_raw(unsafe { oak_folder_child_at(self.ptr, folder.0, index as i32) })
	}

	// -- footage -------------------------------------------------------

	/// The footage's absolute file path.
	pub fn footage_filename(&self, footage: NodeId) -> Option<String> {
		string_out(|buf, len| unsafe { oak_footage_filename(self.ptr, footage.0, buf, len) })
	}

	/// Total stream count.
	pub fn footage_total_stream_count(&self, footage: NodeId) -> usize {
		unsafe { oak_footage_total_stream_count(self.ptr, footage.0) as usize }
	}

	pub fn footage_video_stream_count(&self, footage: NodeId) -> usize {
		unsafe { oak_footage_video_stream_count(self.ptr, footage.0) as usize }
	}

	pub fn footage_audio_stream_count(&self, footage: NodeId) -> usize {
		unsafe { oak_footage_audio_stream_count(self.ptr, footage.0) as usize }
	}

	pub fn footage_subtitle_stream_count(&self, footage: NodeId) -> usize {
		unsafe { oak_footage_subtitle_stream_count(self.ptr, footage.0) as usize }
	}

	/// Total duration (rational seconds).
	pub fn footage_duration(&self, footage: NodeId) -> Rational {
		unsafe { oak_footage_duration(self.ptr, footage.0) }
	}

	/// Video stream length (rational seconds).
	pub fn footage_video_length(&self, footage: NodeId) -> Rational {
		unsafe { oak_footage_video_length(self.ptr, footage.0) }
	}

	/// The `ordinal`-th video stream's params.
	pub fn footage_video_params(&self, footage: NodeId, ordinal: usize) -> Option<StreamVideoParams> {
		let mut out = StreamVideoParams::default();
		if unsafe { oak_footage_video_params(self.ptr, footage.0, ordinal as i32, &mut out) } {
			Some(out)
		} else {
			None
		}
	}

	/// The `ordinal`-th audio stream's params.
	pub fn footage_audio_params(&self, footage: NodeId, ordinal: usize) -> Option<StreamAudioParams> {
		let mut out = StreamAudioParams::default();
		if unsafe { oak_footage_audio_params(self.ptr, footage.0, ordinal as i32, &mut out) } {
			Some(out)
		} else {
			None
		}
	}

	/// Returns the per-footage proxy override, or `None` when global defaults apply.
	pub fn footage_custom_proxy_params(&self, footage: NodeId) -> Option<crate::codec::ProxyParams> {
		let mut out = crate::codec::ProxyParams::default();
		unsafe { oak_footage_custom_proxy_params(self.ptr, footage.0, &mut out) }.then_some(out)
	}

	/// Returns custom proxy parameters when present, otherwise global defaults.
	pub fn footage_effective_proxy_params(&self, footage: NodeId) -> Option<crate::codec::ProxyParams> {
		let mut out = crate::codec::ProxyParams::default();
		unsafe { oak_footage_effective_proxy_params(self.ptr, footage.0, &mut out) }.then_some(out)
	}

	/// Returns source-start timecode only when the footage carries it.
	pub fn footage_source_start_time(&self, footage: NodeId) -> Option<Rational> {
		let mut out = Rational::NULL;
		unsafe { oak_footage_source_start_time(self.ptr, footage.0, &mut out) }.then_some(out)
	}

	// -- sequence / track list / track ---------------------------------

	/// Number of track lists in a sequence (video lists first).
	pub fn sequence_track_list_count(&self, sequence: NodeId) -> usize {
		unsafe { oak_sequence_track_list_count(self.ptr, sequence.0) as usize }
	}

	/// The `index`-th track list of a sequence.
	pub fn sequence_track_list_at(&self, sequence: NodeId, index: usize) -> NodeId {
		NodeId::from_raw(unsafe { oak_sequence_track_list_at(self.ptr, sequence.0, index as i32) })
	}

	/// The sequence playhead (rational seconds).
	pub fn sequence_playhead(&self, sequence: NodeId) -> Rational {
		unsafe { oak_sequence_playhead(self.ptr, sequence.0) }
	}

	/// The sequence's length.
	pub fn sequence_length(&self, sequence: NodeId) -> Rational {
		unsafe { oak_sequence_length(self.ptr, sequence.0) }
	}

	/// The sequence's frame rate (the null rational when it has no
	/// video params).
	pub fn sequence_frame_rate(&self, sequence: NodeId) -> Rational {
		unsafe { oak_sequence_frame_rate(self.ptr, sequence.0) }
	}

	/// The sequence's output geometry (width, height); None when it has
	/// no video params.
	pub fn sequence_geometry(&self, sequence: NodeId) -> Option<(i32, i32)> {
		let (mut w, mut h) = (0, 0);
		unsafe { oak_sequence_geometry(self.ptr, sequence.0, &mut w, &mut h) }.then_some((w, h))
	}

	/// Whether the sequence uses interlaced video (`None` for a stale id).
	pub fn sequence_interlaced(&self, sequence: NodeId) -> Option<bool> {
		match unsafe { oak_sequence_interlaced(self.ptr, sequence.0) } {
			0 => Some(false),
			1 => Some(true),
			_ => None,
		}
	}

	/// Sets the sequence's output video params (width/height/frame
	/// rate; pixel format stays F32 RGBA).
	pub fn sequence_set_video_params(
		&self,
		sequence: NodeId,
		width: i32,
		height: i32,
		frame_rate_num: i32,
		frame_rate_den: i32,
	) -> bool {
		unsafe {
			oak_sequence_set_video_params(
				self.ptr,
				sequence.0,
				width,
				height,
				frame_rate_num,
				frame_rate_den,
			)
		}
	}

	/// The node's user label ("" when unset).
	pub fn node_label(&self, id: NodeId) -> String {
		string_out(|buf, len| unsafe { oak_node_label(self.ptr, id.0, buf, len) })
			.unwrap_or_default()
	}

	/// Sets the node's user label (a direct write, not an undoable
	/// edit).
	pub fn node_set_label(&self, id: NodeId, label: &str) -> bool {
		unsafe { oak_node_set_label(self.ptr, id.0, label.as_ptr(), label.len()) }
	}

	/// The track list's media type code (`TrackType`).
	pub fn tracklist_kind(&self, tracklist: NodeId) -> i32 {
		unsafe { oak_tracklist_kind(self.ptr, tracklist.0) }
	}

	/// Number of tracks in a track list.
	pub fn tracklist_track_count(&self, tracklist: NodeId) -> usize {
		unsafe { oak_tracklist_track_count(self.ptr, tracklist.0) as usize }
	}

	/// The `index`-th track of a track list (stack order).
	pub fn tracklist_track_at(&self, tracklist: NodeId, index: usize) -> NodeId {
		NodeId::from_raw(unsafe { oak_tracklist_track_at(self.ptr, tracklist.0, index as i32) })
	}

	/// The track's media type code (`TrackType`).
	pub fn track_kind(&self, track: NodeId) -> i32 {
		unsafe { oak_track_kind(self.ptr, track.0) }
	}

	pub fn track_height(&self, track: NodeId) -> f64 {
		unsafe { oak_track_height(self.ptr, track.0) }
	}

	pub fn track_muted(&self, track: NodeId) -> bool {
		unsafe { oak_track_muted(self.ptr, track.0) }
	}

	pub fn track_locked(&self, track: NodeId) -> bool {
		unsafe { oak_track_locked(self.ptr, track.0) }
	}

	/// Number of blocks on a track.
	pub fn track_block_count(&self, track: NodeId) -> usize {
		unsafe { oak_track_block_count(self.ptr, track.0) as usize }
	}

	/// The `index`-th block on a track (timeline order).
	pub fn track_block_at(&self, track: NodeId, index: usize) -> NodeId {
		NodeId::from_raw(unsafe { oak_track_block_at(self.ptr, track.0, index as i32) })
	}

	/// The block strictly containing `time` on this track.
	pub fn track_block_containing_time(&self, track: NodeId, time: Rational) -> NodeId {
		NodeId::from_raw(unsafe { oak_track_block_containing_time(self.ptr, track.0, time) })
	}

	/// The track's total length (end of the last block).
	pub fn track_length(&self, track: NodeId) -> Rational {
		unsafe { oak_track_length(self.ptr, track.0) }
	}

	// -- block ----------------------------------------------------------

	/// The block's timeline in-point.
	pub fn block_in(&self, block: NodeId) -> Rational {
		unsafe { oak_block_in(self.ptr, block.0) }
	}

	/// The block's timeline out-point.
	pub fn block_out(&self, block: NodeId) -> Rational {
		unsafe { oak_block_out(self.ptr, block.0) }
	}

	/// The block's length.
	pub fn block_length(&self, block: NodeId) -> Rational {
		unsafe { oak_block_length(self.ptr, block.0) }
	}

	/// The block's media in-point.
	pub fn block_media_in(&self, block: NodeId) -> Rational {
		unsafe { oak_block_media_in(self.ptr, block.0) }
	}

	/// The block's media out-point.
	pub fn block_media_out(&self, block: NodeId) -> Rational {
		unsafe { oak_block_media_out(self.ptr, block.0) }
	}

	/// The block's standard enabled flag.
	pub fn block_enabled(&self, block: NodeId) -> bool {
		unsafe { oak_block_enabled(self.ptr, block.0) }
	}

	// -- node parameters -------------------------------------------------

	/// The input's static value. `None` for unknown inputs or values
	/// without a POD representation (strings are not bridged yet).
	pub fn standard_value(&self, id: NodeId, input_id: &str, element: i32) -> Option<NodeValue> {
		let mut out = NodeValue::default();
		if unsafe {
			oak_node_standard_value(self.ptr, id.0, input_id.as_ptr(), input_id.len(), element, &mut out)
		} {
			Some(out)
		} else {
			None
		}
	}

	/// The input's value at `time` (keyframe-interpolated when
	/// animated).
	pub fn value_at_time(
		&self,
		id: NodeId,
		input_id: &str,
		element: i32,
		time: Rational,
	) -> Option<NodeValue> {
		let mut out = NodeValue::default();
		if unsafe {
			oak_node_value_at_time(
				self.ptr,
				id.0,
				input_id.as_ptr(),
				input_id.len(),
				element,
				time,
				&mut out,
			)
		} {
			Some(out)
		} else {
			None
		}
	}
	// ---------------------------------------------------------------------
	// Editing (undo-integrated: every mutation pushes a command; batch
	// several calls into one undo step with `undo_group_begin`/`end`)
	// ---------------------------------------------------------------------

	/// Imports a media file into the bin (probes first; a failed probe
	/// leaves nothing behind). Undoable.
	pub fn import_footage(&self, filename: &str) -> Option<NodeId> {
		let id = unsafe { oak_footage_import(self.ptr, filename.as_ptr(), filename.len()) };
		let id = NodeId::from_raw(id);
		id.is_valid().then_some(id)
	}

	/// Creates a folder under `parent` (the root works). Undoable.
	pub fn create_folder(&self, parent: NodeId, name: &str) -> Option<NodeId> {
		let id = unsafe { oak_folder_create(self.ptr, parent.0, name.as_ptr(), name.len()) };
		let id = NodeId::from_raw(id);
		id.is_valid().then_some(id)
	}

	/// Removes a node from the project entirely. Undoable.
	pub fn remove_node(&self, id: NodeId) -> bool {
		unsafe { oak_node_remove(self.ptr, id.0) }
	}

	/// Creates a sequence with the default 2V+2A track layout, mounted in
	/// the bin root. (Creation itself is not undoable, like the app.)
	pub fn create_sequence(&self, name: &str) -> Option<NodeId> {
		let id = unsafe { oak_sequence_create(self.ptr, name.as_ptr(), name.len()) };
		let id = NodeId::from_raw(id);
		id.is_valid().then_some(id)
	}

	/// Adds a track to the sequence's list of the given media kind
	/// (`TrackType` code; 0 = video, 1 = audio, 2 = subtitle). Undoable.
	pub fn add_track(&self, sequence: NodeId, kind: i32) -> Option<NodeId> {
		let id = unsafe { oak_track_add(self.ptr, sequence.0, kind) };
		let id = NodeId::from_raw(id);
		id.is_valid().then_some(id)
	}

	/// Creates a clip block (not on any track yet — placement is the
	/// undoable step).
	pub fn create_clip(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_block_create_clip(self.ptr) });
		id.is_valid().then_some(id)
	}

	/// Creates a gap block.
	pub fn create_gap(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_block_create_gap(self.ptr) });
		id.is_valid().then_some(id)
	}

	/// Creates a transition block.
	pub fn create_transition(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_block_create_transition(self.ptr) });
		id.is_valid().then_some(id)
	}

	/// Creates an adjustment block.
	pub fn create_adjustment(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_block_create_adjustment(self.ptr) });
		id.is_valid().then_some(id)
	}

	/// Appends `block` to the end of `track`. Undoable.
	pub fn track_append_block(&self, track: NodeId, block: NodeId) -> bool {
		unsafe { oak_track_append_block(self.ptr, track.0, block.0) }
	}

	/// Inserts `block` after `before` (`None` = append). Undoable.
	pub fn track_insert_block_after(
		&self,
		track: NodeId,
		block: NodeId,
		before: Option<NodeId>,
	) -> bool {
		let before = before.unwrap_or(NodeId::INVALID);
		unsafe { oak_track_insert_block_after(self.ptr, track.0, block.0, before.0) }
	}

	/// Removes `block` from `track`, closing the gap (ripple). Undoable.
	pub fn track_ripple_remove_block(&self, track: NodeId, block: NodeId) -> bool {
		unsafe { oak_track_ripple_remove_block(self.ptr, track.0, block.0) }
	}

	/// Replaces `old` with `new` on `track`. Undoable.
	pub fn track_replace_block(&self, track: NodeId, old: NodeId, new: NodeId) -> bool {
		unsafe { oak_track_replace_block(self.ptr, track.0, old.0, new.0) }
	}

	/// Sets the block's length (right-edge resize). Undoable.
	pub fn block_set_length(&self, block: NodeId, length: Rational) -> bool {
		unsafe { oak_block_set_length(self.ptr, block.0, length) }
	}

	/// Sets the block's length, keeping the out point anchored in the
	/// media (left-edge resize). Undoable.
	pub fn block_set_length_and_media_in(&self, block: NodeId, length: Rational) -> bool {
		unsafe { oak_block_set_length_and_media_in(self.ptr, block.0, length) }
	}

	/// Sets the block's media in-point. Undoable.
	pub fn block_set_media_in(&self, block: NodeId, media_in: Rational) -> bool {
		unsafe { oak_block_set_media_in(self.ptr, block.0, media_in) }
	}

	/// Sets the input's static value (POD types only). Undoable.
	pub fn set_standard_value(
		&self,
		id: NodeId,
		input_id: &str,
		element: i32,
		value: NodeValue,
	) -> bool {
		unsafe {
			oak_node_set_standard_value(
				self.ptr,
				id.0,
				input_id.as_ptr(),
				input_id.len(),
				element,
				value,
			)
		}
	}

	/// Connects `from`'s output to `to`'s `input_id` at `element` (-1 =
	/// the scalar input). These edges are document structure (persisted
	/// in the project file).
	pub fn connect(&self, from: NodeId, to: NodeId, input_id: &str, element: i32) -> bool {
		unsafe {
			oak_node_connect(self.ptr, from.0, to.0, input_id.as_ptr(), input_id.len(), element)
		}
	}

	/// Disconnects `to`'s `input_id` at `element`.
	pub fn disconnect(&self, to: NodeId, input_id: &str, element: i32) -> bool {
		unsafe { oak_node_disconnect(self.ptr, to.0, input_id.as_ptr(), input_id.len(), element) }
	}

}

impl Default for Project {
	fn default() -> Self {
		Self::new()
	}
}

impl Clone for Project {
	fn clone(&self) -> Self {
		unsafe { oak_project_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for Project {
	fn drop(&mut self) {
		unsafe { oak_project_release(self.ptr) };
	}
}

/// Sets (or with `None` clears) the process-wide active project — the
/// one plugins see through [`active_project`].
pub fn set_active_project(project: Option<&Project>) {
	unsafe {
		oak_project_set_active(match project {
			Some(p) => p.ptr,
			None => std::ptr::null(),
		})
	};
}

/// The process-wide active project (`None` when the host has none open).
pub fn active_project() -> Option<Project> {
	let ptr = unsafe { oak_project_active() };
	if ptr.is_null() {
		None
	} else {
		Some(Project { ptr })
	}
}


impl Project {
	/// Saves the project as XML to `path` (`None` = the project's own
	/// filename; save-as semantics: the path becomes the filename).
	/// Plugins are trusted in-process code — no path restrictions.
	pub fn save(&self, path: Option<&str>) -> bool {
		let (p, pl) = match path {
			Some(p) => (p.as_ptr(), p.len()),
			None => (std::ptr::null(), 0),
		};
		unsafe { oak_project_save(self.ptr, p, pl) }
	}

	/// Loads a project from an XML file. Not made active automatically.
	pub fn load(path: &str) -> Option<Project> {
		let ptr = unsafe { oak_project_load(path.as_ptr(), path.len()) };
		if ptr.is_null() {
			None
		} else {
			Some(Project { ptr })
		}
	}

	/// Sets the project file path (used by save when no path is given).
	pub fn set_filename(&self, path: &str) -> bool {
		unsafe { oak_project_set_filename(self.ptr, path.as_ptr(), path.len()) }
	}

	/// Sets the working color space: persisted into the project settings
	/// AND applied to the process-wide color pipeline.
	pub fn set_working_color_space(&self, setting: &str) -> bool {
		unsafe { oak_project_set_working_color_space(self.ptr, setting.as_ptr(), setting.len()) }
	}

	/// Sets the output gamut/transfer (persisted + applied to the
	/// pipeline).
	pub fn set_output_color_spec(&self, gamut: &str, transfer: &str) -> bool {
		unsafe {
			oak_project_set_output_color_spec(
				self.ptr,
				gamut.as_ptr(),
				gamut.len(),
				transfer.as_ptr(),
				transfer.len(),
			)
		}
	}

	/// A deep copy of the project (background-render isolation).
	pub fn deep_copy(&self) -> Option<Project> {
		let ptr = unsafe { oak_project_deep_copy(self.ptr) };
		if ptr.is_null() {
			None
		} else {
			Some(Project { ptr })
		}
	}
}

/// Begins an undo group (calls inside form one undo step).
pub fn undo_group_begin(name: &str) -> bool {
	unsafe { oak_undo_group_begin(name.as_ptr(), name.len()) }
}

/// Closes the current undo group. False when none is open.
pub fn undo_group_end() -> bool {
	unsafe { oak_undo_group_end() }
}

/// Aborts the current undo group, rolling it back. False when none is
/// open.
pub fn undo_group_abort() -> bool {
	unsafe { oak_undo_group_abort() }
}

/// Undoes one step. False when the stack is empty.
pub fn undo() -> bool {
	unsafe { oak_undo_undo() }
}

/// Redoes one step. False when nothing to redo.
pub fn redo() -> bool {
	unsafe { oak_undo_redo() }
}

// ---------------------------------------------------------------------------
// Undo stack (read-only)
// ---------------------------------------------------------------------------

/// Whether the global undo stack can undo.
pub fn undo_can_undo() -> bool {
	unsafe { oak_undo_can_undo() }
}

/// Whether the global undo stack can redo.
pub fn undo_can_redo() -> bool {
	unsafe { oak_undo_can_redo() }
}

/// Number of commands on the undo stack.
pub fn undo_count() -> i64 {
	unsafe { oak_undo_count() }
}

/// The current position on the undo stack.
pub fn undo_index() -> i64 {
	unsafe { oak_undo_index() }
}

/// Jumps to history position `index` (undoing/redoing as needed).
pub fn undo_jump(index: i64) -> bool {
	unsafe { oak_undo_jump(index) }
}

/// Clears the undo stack (leaving the fresh "new project" row).
pub fn undo_clear() -> bool {
	unsafe { oak_undo_clear() }
}

/// The display name of history row `row`; None when out of range.
pub fn undo_command_name(row: i64) -> Option<String> {
	string_out(|buf, len| unsafe { oak_undo_command_name(row, buf, len) })
}

/// Whether history row `row` is currently done; None when out of range.
pub fn undo_command_done(row: i64) -> Option<bool> {
	match unsafe { oak_undo_command_done(row) } {
		0 => Some(false),
		1 => Some(true),
		_ => None,
	}
}

type UndoObserverFn = unsafe extern "C" fn(userdata: *mut c_void);

// Registered observers (double-boxed closures; the engine hands the
// pointer back as userdata). Registration-only, matching the engine.
static UNDO_OBSERVERS: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// Registers a change observer: called after any undoable command lands
/// (push, undo, redo, jump, clear) — the "project modified" signal.
/// Registration-only (no removal).
pub fn add_undo_observer(f: impl Fn() + Send + 'static) {
	unsafe extern "C" fn trampoline(userdata: *mut c_void) {
		if userdata.is_null() {
			return;
		}
		let f = unsafe { &**(userdata as *const Box<dyn Fn() + Send>) };
		f();
	}
	let boxed = Box::into_raw(Box::new(Box::new(f) as Box<dyn Fn() + Send>));
	UNDO_OBSERVERS
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.push(boxed as usize);
	unsafe { oak_undo_add_observer(Some(trampoline), boxed as *mut c_void) };
}

/// Serializes every test that touches the process-global undo stack
/// (all test modules in this crate share it).
#[cfg(test)]
pub(crate) static UNDO_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
	use super::*;

	/// History navigation/inspection and the observer hook (the undo
	/// stack is process-global; serialize).

	#[test]
	fn undo_history_rows_and_observer() {
		let _guard = UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
		add_undo_observer(|| {
			SEEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		});

		let project = Project::new();
		let base = undo_count();
		project.create_folder(project.root(), "Observed").expect("folder");
		assert_eq!(undo_count(), base + 1);
		assert!(SEEN.load(std::sync::atomic::Ordering::SeqCst) > 0);

		assert!(undo_command_name(base).is_some());
		assert_eq!(undo_command_done(base), Some(true));
		assert!(undo_jump(base));
		assert_eq!(undo_command_done(base), Some(false));
		assert!(undo_jump(base + 1));
		assert_eq!(undo_command_done(base), Some(true));

		assert!(undo_clear());
		assert_eq!(undo_count(), 1, "clear leaves the fresh empty row");
	}

	#[test]
	fn node_label_and_sequence_params() {
		let _guard = UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		let project = Project::new();
		let folder = project.create_folder(project.root(), "Labeled").expect("folder");
		assert_eq!(project.node_label(folder), "Labeled");
		assert!(project.node_set_label(folder, "Renamed"));
		assert_eq!(project.node_label(folder), "Renamed");
		assert!(!project.node_set_label(NodeId::INVALID, "x"));

		let seq = project.create_sequence("Seq").expect("sequence");
		// A fresh sequence carries the default 29.97 fps video params.
		assert_eq!(project.sequence_frame_rate(seq), Rational::new(30000, 1001));
		assert!(project.sequence_geometry(seq).is_some());
		assert!(project.sequence_set_video_params(seq, 1920, 1080, 24, 1));
		assert_eq!(project.sequence_frame_rate(seq), Rational::new(24, 1));
		assert_eq!(project.sequence_geometry(seq), Some((1920, 1080)));
		assert!(!project.sequence_set_video_params(NodeId::INVALID, 1, 1, 1, 1));
		assert!(undo_clear());
	}

	#[test]
	fn project_basics_and_node_ids() {
		let project = Project::new();
		assert!(!project.modified());
		assert!(project.filename().is_empty());

		let root = project.root();
		assert!(root.is_valid());
		assert!(!NodeId::INVALID.is_valid());
		assert_eq!(project.node_kind(root), Some(NodeKind::Folder));
		assert_eq!(project.node_kind(NodeId::INVALID), None);
		assert!(project
			.node_type_id(root)
			.is_some_and(|id| id.ends_with("folder")));
		assert_eq!(project.folder_child_count(root), 0);

		// Clone shares the project: same root.
		let clone = project.clone();
		assert_eq!(clone.root(), root);
	}

	#[test]
	fn active_project_slot() {
		set_active_project(None);
		assert!(active_project().is_none());

		let project = Project::new();
		set_active_project(Some(&project));
		let active = active_project().expect("active");
		assert_eq!(active.root(), project.root());

		set_active_project(None);
		assert!(active_project().is_none());
	}

	#[test]
	fn undo_read_only() {
		// Fresh process-wide stack state is whatever the suite left;
		// just verify the calls work.
		let _ = undo_can_undo();
		let _ = undo_can_redo();
		assert!(undo_count() >= 0);
	}

	#[test]
	fn unreadable_nodes_fail_cleanly() {
		let project = Project::new();
		let root = project.root();
		// The root folder has no "file_in" input; and no node reads work
		// on an invalid id.
		assert!(project.standard_value(root, "file_in", -1).is_none());
		assert!(project.standard_value(NodeId::INVALID, "file_in", -1).is_none());
		assert!(project
			.value_at_time(root, "file_in", -1, Rational::new(0, 1))
			.is_none());
		assert_eq!(project.footage_filename(root), None, "root is a folder");
		assert_eq!(project.track_block_count(root), 0);
		assert_eq!(project.sequence_track_list_at(root, 0), NodeId::INVALID);
	}
}

#[cfg(test)]
mod edit_tests {
	use super::*;
	use crate::codec::{Codec, EncoderSession, EncodingParams, Format};
	use crate::codec::Frame;

	// The undo stack is process-global; serialize the edit tests.

	/// A real 64x64 clip encoded through the SDK (the encode session
	/// doubles as test-media generation).
	fn make_test_clip(name: &str) -> String {
		let out = std::env::temp_dir().join(format!(
			"oaksdk_nodeedit_{name}_{}.mp4",
			std::process::id()
		));
		let mut params = EncodingParams::new();
		params.set_filename(out.to_str().unwrap()).unwrap();
		params.format = Format::MPEG4Video as i32;
		params.video_enabled = 1;
		params.video_codec = Codec::H264 as i32;
		params.video_width = 64;
		params.video_height = 64;
		params.video_time_base_num = 1;
		params.video_time_base_den = 10;
		params.video_pixel_format = crate::types::PixelFormat::F32 as i32;
		params.video_pixel_aspect_num = 1;
		params.video_pixel_aspect_den = 1;

		let session = EncoderSession::create(&params).expect("create encoder");
		session.open().expect("open");
		let mut vp = crate::types::VideoParams::new();
		vp.width = 64;
		vp.height = 64;
		vp.format = crate::types::PixelFormat::F32 as i32;
		vp.channel_count = 4;
		for i in 0..10 {
			let mut frame = Frame::with_params(&vp);
			frame.allocate().expect("allocate");
			frame.set_timestamp(Rational::new(i, 10));
			session.write_video(&frame).expect("write");
		}
		session.flush().expect("flush");
		session.close().expect("close");
		out.to_string_lossy().into_owned()
	}

	/// The full chain through the SDK: import → sequence → clip →
	/// place → trim → undo back → redo forward.
	#[test]
	fn sdk_edit_chain_and_undo() {
		let _guard = UNDO_SERIAL.lock().unwrap();
		let project = Project::new();

		// Import (encode a clip first: the SDK has no testmedia helper).
		let clip_path = make_test_clip("chain");
		let footage = project.import_footage(&clip_path).expect("import");
		assert_eq!(project.folder_child_count(project.root()), 1);
		assert!(project.footage_video_stream_count(footage) >= 1);
		let params = project.footage_video_params(footage, 0).expect("video params");
		assert_eq!((params.width, params.height), (64, 64));

		// Sequence + clip + placement.
		let seq = project.create_sequence("Seq 1").expect("sequence");
		assert_eq!(project.sequence_track_list_count(seq), 2);
		let tracklist = project.sequence_track_list_at(seq, 0);
		let track = project.tracklist_track_at(tracklist, 0);
		let clip = project.create_clip().expect("clip");
		assert_eq!(project.node_kind(clip), Some(NodeKind::Clip));
		assert!(project.track_append_block(track, clip));
		assert_eq!(project.track_block_count(track), 1);

		// Trim.
		assert!(project.block_set_length(clip, Rational::new(5, 1)));
		assert_eq!(project.block_length(clip), Rational::new(5, 1));

		// Undo: trim, placement, import (the sequence mount is creation,
		// not an edit).
		assert!(undo());
		assert_ne!(project.block_length(clip), Rational::new(5, 1));
		assert!(undo());
		assert_eq!(project.track_block_count(track), 0);
		assert!(undo());
		assert_eq!(project.folder_child_count(project.root()), 1, "only the sequence remains");
		assert_eq!(project.folder_child_at(project.root(), 0), seq);

		// Redo the whole chain.
		assert!(redo());
		assert!(redo());
		assert!(redo());
		assert_eq!(project.folder_child_count(project.root()), 2);
		assert_eq!(project.track_block_count(track), 1);
		assert_eq!(project.block_length(clip), Rational::new(5, 1));
	}

	#[test]
	fn sdk_undo_groups_batch_imports() {
		let _guard = UNDO_SERIAL.lock().unwrap();
		let project = Project::new();
		let clip_path = make_test_clip("group");
		let base = undo_count();

		assert!(undo_group_begin("import twice"));
		assert!(project.import_footage(&clip_path).is_some());
		assert!(undo_group_end());
		assert_eq!(undo_count(), base + 1, "the group is one undo step");
		assert!(undo());
		assert_eq!(project.folder_child_count(project.root()), 0);

		assert!(undo_group_begin("aborted"));
		assert!(project.import_footage(&clip_path).is_some());
		assert!(undo_group_abort());
		assert_eq!(project.folder_child_count(project.root()), 0);
		assert_eq!(undo_count(), base + 1, "abort records nothing");
	}
}

#[cfg(test)]
mod persist_tests {
	use super::*;


	#[test]
	fn save_load_roundtrip() {
		let _guard = UNDO_SERIAL.lock().unwrap();
		let project = Project::new();
		let folder = project
			.create_folder(project.root(), "Persisted")
			.expect("folder");

		let path = std::env::temp_dir()
			.join(format!("oaksdk_persist_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		assert!(project.save(Some(&path)));
		assert!(!project.modified());

		let loaded = Project::load(&path).expect("load");
		assert_eq!(loaded.filename(), path);
		assert_eq!(loaded.folder_child_count(loaded.root()), 1);
		assert_eq!(loaded.folder_child_at(loaded.root(), 0), folder);

		assert!(Project::load("/nonexistent/oak-sdk.ove").is_none());
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn settings_sync_pipeline() {
		let project = Project::new();
		let saved = crate::state::color_working_space();
		assert!(project.set_working_color_space("srgb_legacy"));
		assert_eq!(crate::state::color_working_space(), "srgb_legacy");
		assert!(project.set_working_color_space(&saved));

		assert!(project.set_output_color_spec("displayp3", "srgb"));
		assert_eq!(crate::state::color_output_gamut(), "displayp3");
		project.set_output_color_spec("srgb", "srgb");
	}

	#[test]
	fn deep_copy_is_independent() {
		let _guard = UNDO_SERIAL.lock().unwrap();
		let project = Project::new();
		let folder = project.create_folder(project.root(), "CopyTest").expect("folder");

		let copy = project.deep_copy().expect("deep copy");
		assert_eq!(copy.folder_child_count(copy.root()), 1);

		assert!(copy.remove_node(folder));
		assert_eq!(copy.folder_child_count(copy.root()), 0);
		assert_eq!(project.folder_child_count(project.root()), 1);
	}
}

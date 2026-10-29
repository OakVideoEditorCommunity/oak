//! C ABI for oak-node's document model: the active project, node
//! identity, and read-only structure traversal.
//!
//! Nodes are addressed by `NodeId::identity()` tokens (u64);
//! `OAK_NODE_ID_INVALID` (u32::MAX) is the null token. Everything takes
//! the project handle explicitly — plugins read the active project via
//! [`oak_project_active`]. All entry points are null-safe and never
//! panic across the boundary.

use std::sync::{Arc, Mutex, RwLock};

use oak_node::block::{AdjustmentBlockBehavior, ClipBlockBehavior, GapBlockBehavior, TransitionBlockBehavior};
use oak_node::footage::FootageBehavior;
use oak_node::folder::FolderBehavior;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior};
use oak_timeline::util::GraphBlockRange;

use crate::codec::{copy_str_out, str_arg};
use crate::coretypes::OakRational;
use crate::handle::NativeMirror;

/// The null node token (`NodeId::INVALID`'s identity).
pub const OAK_NODE_ID_INVALID: u64 = u32::MAX as u64;

/// The null node token.
#[unsafe(no_mangle)]
pub extern "C" fn oak_node_id_invalid() -> u64 {
	OAK_NODE_ID_INVALID
}

/// Whether `id` is a valid node token.
#[unsafe(no_mangle)]
pub extern "C" fn oak_node_id_is_valid(id: u64) -> bool {
	NodeId::from_identity(id).is_some()
}

// ---------------------------------------------------------------------------
// Project handle + the active-project slot
// ---------------------------------------------------------------------------

/// An open project (refcounted handle).
pub struct ProjectHandle {
	inner: Arc<Mutex<Project>>,
}

/// Converts an owned engine project handle into the storage migration handle
/// used by legacy hosts. The input reference is consumed exactly once; the
/// returned handle owns the same project Arc, not a reloaded copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_into_storage_handle(
	project: *mut ProjectHandle,
) -> oak_core::handle::CHandle {
	let Some(project) = (unsafe { project.as_ref() }) else {
		return oak_core::handle::CHandle::null();
	};
	let inner = project.project().clone();
	unsafe { oak_project_release(project) };
	oak_storage::nodeutil::make_project_owned(inner)
}

impl ProjectHandle {
	/// Wraps an existing shared project as a handle with ref count 1
	/// (the storage backend boundary hands projects over as
	/// `Arc<Mutex<Project>>`).
	pub(crate) fn from_arc(inner: Arc<Mutex<Project>>) -> *mut ProjectHandle {
		crate::handle::into_ffi(ProjectHandle { inner }) as *mut ProjectHandle
	}

	/// The shared project (crate-internal consumers: storage save,
	/// frame rendering).
	pub(crate) fn project(&self) -> &Arc<Mutex<Project>> {
		&self.inner
	}
}

crate::export_handle!(
	ProjectHandle,
	oak_project_add_ref,
	oak_project_release
);

/// The process-wide active project slot: the host sets it when a project
/// opens, plugins read it via [`oak_project_active`].
static ACTIVE_PROJECT: RwLock<Option<Arc<Mutex<Project>>>> = RwLock::new(None);

/// A new, initialized project (empty graph with a root folder), ref
/// count 1.
#[unsafe(no_mangle)]
pub extern "C" fn oak_project_new() -> *mut ProjectHandle {
	let project = Project::new();
	project
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.initialize()
		.ok();
	crate::handle::into_ffi(ProjectHandle { inner: project }) as *mut ProjectHandle
}

/// Sets (or with null clears) the active project. The slot clones the
/// handle's Arc; the caller keeps its reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_active(project: *const ProjectHandle) {
	let slot = if project.is_null() {
		None
	} else {
		Some(unsafe { &*project }.inner.clone())
	};
	*ACTIVE_PROJECT.write().unwrap_or_else(|e| e.into_inner()) = slot;
}

/// A new reference on the active project (null when none is set).
#[unsafe(no_mangle)]
pub extern "C" fn oak_project_active() -> *mut ProjectHandle {
	let slot = ACTIVE_PROJECT.read().unwrap_or_else(|e| e.into_inner()).clone();
	match slot {
		Some(inner) => crate::handle::into_ffi(ProjectHandle { inner }) as *mut ProjectHandle,
		None => std::ptr::null_mut(),
	}
}

pub(crate) fn with_project<R>(
	project: *const ProjectHandle,
	f: impl FnOnce(&Project) -> R,
) -> Option<R> {
	if project.is_null() {
		return None;
	}
	let guard = unsafe { &*project }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	Some(f(&guard))
}

fn node_id(token: u64) -> Option<NodeId> {
	NodeId::from_identity(token)
}

/// Clones the project's shared `Arc` out of its handle (for manager APIs
/// that take the same `Arc<Mutex<Project>>` the app and workers share).
pub(crate) fn project_arc(project: *const ProjectHandle) -> Option<Arc<Mutex<Project>>> {
	if project.is_null() {
		return None;
	}
	Some(unsafe { &*project }.inner.clone())
}

fn behavior_as<T: 'static>(project: &Project, id: NodeId) -> Option<&T> {
	project
		.graph
		.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<T>()
}

fn write_str<R: Default>(value: Option<R>, f: impl FnOnce(&R, *mut u8, usize) -> i32, buf: *mut u8, buf_len: usize) -> i32 {
	match value {
		Some(v) => f(&v, buf, buf_len),
		None => -1,
	}
}

macro_rules! project_string_getter {
	($( $name:ident => |$p:ident| $body:expr ),* $(,)?) => {
		$(
			#[doc = "Snprintf-style project string getter; -1 on null/missing."]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(
				project: *const ProjectHandle,
				buf: *mut u8,
				buf_len: usize,
			) -> i32 {
				write_str(
					with_project(project, |$p| $body).flatten(),
					|s, buf, len| copy_str_out(s, buf, len),
					buf,
					buf_len,
				)
			}
		)*
	};
}

project_string_getter! {
	oak_project_name => |p| Some(p.name()),
	oak_project_filename => |p| Some(p.filename.clone()),
	oak_project_pretty_filename => |p| Some(p.pretty_filename().to_string()),
}

/// Whether the project has unsaved changes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_modified(project: *const ProjectHandle) -> bool {
	with_project(project, |p| p.modified).unwrap_or(false)
}

/// The root folder node token (the invalid token when the project is
/// uninitialized).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_root(project: *const ProjectHandle) -> u64 {
	with_project(project, |p| p.root.identity()).unwrap_or(OAK_NODE_ID_INVALID)
}

// ---------------------------------------------------------------------------
// Generic node queries
// ---------------------------------------------------------------------------

/// The node's type id string (e.g. "org.olivevideoeditor.Olive.clipblock"),
/// snprintf-style; -1 on null/unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_type_id(
	project: *const ProjectHandle,
	id: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let value = node_id(id).and_then(|id| {
		with_project(project, |p| {
			p.graph.get(id).map(|e| e.behavior.type_id().to_string())
		})
		.flatten()
	});
	write_str(value, |s, b, l| copy_str_out(s, b, l), buf, buf_len)
}

/// The node's coarse kind: 1 folder, 2 footage, 3 sequence, 4 track,
/// 5 clip, 6 gap, 7 transition, 8 adjustment, 9 track list; 0 unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_kind(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		p.graph.get(id).map(|e| {
			match e.behavior.type_id() {
				"org.olivevideoeditor.Olive.folder" => 1,
				"org.olivevideoeditor.Olive.footage" => 2,
				"org.olivevideoeditor.Olive.sequence" => 3,
				"org.olivevideoeditor.Olive.track" => 4,
				"org.olivevideoeditor.Olive.clipblock" => 5,
				"org.olivevideoeditor.Olive.gapblock" => 6,
				"org.olivevideoeditor.Olive.transitionblock" => 7,
				"org.olivevideoeditor.Olive.adjustment" => 8,
				"org.olivevideoeditor.Olive.tracklist" => 9,
				_ => 0,
			}
		})
	})
	.flatten()
	.unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Folder
// ---------------------------------------------------------------------------

/// Number of children in a folder (0 on null/wrong kind).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_folder_child_count(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		behavior_as::<FolderBehavior>(p, id).map(|f| f.children.len() as i32)
	})
	.flatten()
	.unwrap_or(0)
}

/// The `index`-th child of a folder (invalid token when out of range).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_folder_child_at(
	project: *const ProjectHandle,
	id: u64,
	index: i32,
) -> u64 {
	let Some(id) = node_id(id) else { return OAK_NODE_ID_INVALID };
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		behavior_as::<FolderBehavior>(p, id)
			.and_then(|f| f.children.get(index as usize).copied())
			.map(|c| c.identity())
	})
	.flatten()
	.unwrap_or(OAK_NODE_ID_INVALID)
}

// ---------------------------------------------------------------------------
// Footage
// ---------------------------------------------------------------------------

/// A video stream's parameters as carried by a footage node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakStreamVideoParams {
	pub width: i32,
	pub height: i32,
	pub frame_rate: OakRational,
	pub pixel_format: i32,
	pub channels: i32,
	pub interlaced: i32,
}

impl NativeMirror for OakStreamVideoParams {
	type Native = oak_node::value::VideoParams;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			width: native.width,
			height: native.height,
			frame_rate: OakRational::from_native(&native.frame_rate),
			pixel_format: native.pixel_format,
			channels: native.channels,
			interlaced: i32::from(native.interlaced),
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			width: self.width,
			height: self.height,
			frame_rate: self.frame_rate.to_native(),
			pixel_format: self.pixel_format,
			channels: self.channels,
			interlaced: self.interlaced != 0,
		}
	}
}

/// An audio stream's parameters as carried by a footage node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakStreamAudioParams {
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub format: i32,
}

impl NativeMirror for OakStreamAudioParams {
	type Native = oak_node::value::AudioParams;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			sample_rate: native.sample_rate,
			channel_layout: native.channel_layout,
			format: native.format,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			sample_rate: self.sample_rate,
			channel_layout: self.channel_layout,
			format: self.format,
		}
	}
}

/// The footage's absolute file path, snprintf-style; -1 on null/wrong
/// kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_filename(
	project: *const ProjectHandle,
	id: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let value = node_id(id).and_then(|id| {
		with_project(project, |p| {
			behavior_as::<FootageBehavior>(p, id).map(|f| f.filename.clone())
		})
		.flatten()
	});
	write_str(value, |s, b, l| copy_str_out(s, b, l), buf, buf_len)
}

macro_rules! footage_count {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("`FootageBehavior::", stringify!($native), "` (0 on null/wrong kind).")]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(project: *const ProjectHandle, id: u64) -> i32 {
				let Some(id) = node_id(id) else { return 0 };
				with_project(project, |p| {
					behavior_as::<FootageBehavior>(p, id).map(|f| f.$native() as i32)
				})
				.flatten()
				.unwrap_or(0)
			}
		)*
	};
}

footage_count! {
	oak_footage_total_stream_count => total_stream_count,
	oak_footage_video_stream_count => video_stream_count,
	oak_footage_audio_stream_count => audio_stream_count,
	oak_footage_subtitle_stream_count => subtitle_stream_count,
}

macro_rules! footage_rational {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("`FootageBehavior::", stringify!($native), "` (null rational on null/wrong kind).")]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(project: *const ProjectHandle, id: u64) -> OakRational {
				let null = OakRational { num: 0, den: 0 };
				let Some(id) = node_id(id) else { return null };
				with_project(project, |p| {
					behavior_as::<FootageBehavior>(p, id)
						.map(|f| OakRational::from_native(&f.$native()))
				})
				.flatten()
				.unwrap_or(null)
			}
		)*
	};
}

footage_rational! {
	oak_footage_duration => duration,
	oak_footage_video_length => video_length,
}

/// Source-start timecode when present; false when unset or not footage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_source_start_time(
	project: *const ProjectHandle,
	id: u64,
	out: *mut OakRational,
) -> bool {
	let (Some(id), Some(out)) = (node_id(id), unsafe { out.as_mut() }) else {
		return false;
	};
	let value = with_project(project, |project| {
		behavior_as::<FootageBehavior>(project, id).and_then(|footage| {
			footage
				.has_source_start_time
				.then_some(OakRational::from_native(&footage.source_start_time))
		})
	})
	.flatten();
	let Some(value) = value else {
		return false;
	};
	*out = value;
	true
}

/// The `ordinal`-th video stream's params. False when out of range or
/// wrong kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_video_params(
	project: *const ProjectHandle,
	id: u64,
	ordinal: i32,
	out: *mut OakStreamVideoParams,
) -> bool {
	if out.is_null() || ordinal < 0 {
		return false;
	}
	let Some(id) = node_id(id) else { return false };
	let params = with_project(project, |p| {
		behavior_as::<FootageBehavior>(p, id)
			.and_then(|f| f.video_params(ordinal as usize))
			.map(|v| OakStreamVideoParams::from_native(&v))
	})
	.flatten();
	match params {
		Some(v) => {
			unsafe { *out = v };
			true
		}
		None => false,
	}
}

/// The `ordinal`-th audio stream's params. False when out of range or
/// wrong kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_audio_params(
	project: *const ProjectHandle,
	id: u64,
	ordinal: i32,
	out: *mut OakStreamAudioParams,
) -> bool {
	if out.is_null() || ordinal < 0 {
		return false;
	}
	let Some(id) = node_id(id) else { return false };
	let params = with_project(project, |p| {
		behavior_as::<FootageBehavior>(p, id)
			.and_then(|f| f.audio_params(ordinal as usize))
			.map(|a| OakStreamAudioParams::from_native(&a))
	})
	.flatten();
	match params {
		Some(v) => {
			unsafe { *out = v };
			true
		}
		None => false,
	}
}

/// Copies a footage node's custom proxy parameters. False for null/unknown
/// footage or when no custom override exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_custom_proxy_params(
	project: *const ProjectHandle,
	id: u64,
	out: *mut oak_codec::proxymanager::ProxyParams,
) -> bool {
	if out.is_null() {
		return false;
	}
	let Some(id) = node_id(id) else { return false };
	with_project(project, |p| {
		behavior_as::<FootageBehavior>(p, id).and_then(|f| f.custom_proxy_params.clone())
	})
	.flatten()
	.map(|params| {
		unsafe { *out = params };
		true
	})
	.unwrap_or(false)
}

/// Copies a footage node's effective proxy parameters (custom override or
/// global defaults). False for null/unknown footage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_effective_proxy_params(
	project: *const ProjectHandle,
	id: u64,
	out: *mut oak_codec::proxymanager::ProxyParams,
) -> bool {
	if out.is_null() {
		return false;
	}
	let Some(id) = node_id(id) else { return false };
	with_project(project, |p| {
		behavior_as::<FootageBehavior>(p, id).map(|f| f.effective_proxy_params())
	})
	.flatten()
	.map(|params| {
		unsafe { *out = params };
		true
	})
	.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Sequence / track list / track
// ---------------------------------------------------------------------------

/// Number of track lists in a sequence (0 on null/wrong kind).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_track_list_count(
	project: *const ProjectHandle,
	id: u64,
) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		behavior_as::<SequenceBehavior>(p, id).map(|s| s.track_lists.len() as i32)
	})
	.flatten()
	.unwrap_or(0)
}

/// The `index`-th track list of a sequence (video lists first, C++
/// order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_track_list_at(
	project: *const ProjectHandle,
	id: u64,
	index: i32,
) -> u64 {
	let Some(id) = node_id(id) else { return OAK_NODE_ID_INVALID };
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		behavior_as::<SequenceBehavior>(p, id)
			.and_then(|s| s.track_lists.get(index as usize).copied())
			.map(|t| t.identity())
	})
	.flatten()
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The sequence playhead (rational seconds).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_playhead(
	project: *const ProjectHandle,
	id: u64,
) -> OakRational {
	let null = OakRational { num: 0, den: 0 };
	let Some(id) = node_id(id) else { return null };
	with_project(project, |p| {
		behavior_as::<SequenceBehavior>(p, id)
			.map(|s| OakRational::from_native(&s.playhead))
	})
	.flatten()
	.unwrap_or(null)
}

/// The track list's media type (`TrackType` code; -1 on null/wrong
/// kind).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_tracklist_kind(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return -1 };
	with_project(project, |p| {
		behavior_as::<TrackListBehavior>(p, id).map(|t| t.kind.to_c())
	})
	.flatten()
	.unwrap_or(-1)
}

/// Number of tracks in a track list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_tracklist_track_count(
	project: *const ProjectHandle,
	id: u64,
) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		behavior_as::<TrackListBehavior>(p, id).map(|t| t.tracks.len() as i32)
	})
	.flatten()
	.unwrap_or(0)
}

/// The `index`-th track of a track list (stack order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_tracklist_track_at(
	project: *const ProjectHandle,
	id: u64,
	index: i32,
) -> u64 {
	let Some(id) = node_id(id) else { return OAK_NODE_ID_INVALID };
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		behavior_as::<TrackListBehavior>(p, id)
			.and_then(|t| t.tracks.get(index as usize).copied())
			.map(|t| t.identity())
	})
	.flatten()
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The track's media type (`TrackType` code; -1 on null/wrong kind).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_kind(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return -1 };
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id).map(|t| t.kind.to_c())
	})
	.flatten()
	.unwrap_or(-1)
}

/// Number of blocks on a track.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_block_count(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id).map(|t| t.blocks.len() as i32)
	})
	.flatten()
	.unwrap_or(0)
}

/// The `index`-th block on a track (timeline order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_block_at(
	project: *const ProjectHandle,
	id: u64,
	index: i32,
) -> u64 {
	let Some(id) = node_id(id) else { return OAK_NODE_ID_INVALID };
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id)
			.and_then(|t| t.block_at(index as usize))
			.map(|b| b.identity())
	})
	.flatten()
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The block strictly containing `time` on this track (invalid token
/// when none).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_block_containing_time(
	project: *const ProjectHandle,
	id: u64,
	time: OakRational,
) -> u64 {
	let Some(id) = node_id(id) else { return OAK_NODE_ID_INVALID };
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id).and_then(|t| {
			t.block_containing_time(time.to_native(), &GraphBlockRange { graph: &p.graph })
				.map(|b| b.identity())
		})
	})
	.flatten()
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The track's total length (end of the last block; 0 on empty/null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_length(project: *const ProjectHandle, id: u64) -> OakRational {
	let null = OakRational { num: 0, den: 1 };
	let Some(id) = node_id(id) else { return null };
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id).map(|t| {
			OakRational::from_native(&t.length(&GraphBlockRange { graph: &p.graph }))
		})
	})
	.flatten()
	.unwrap_or(null)
}

// ---------------------------------------------------------------------------
// Block (clip/gap/transition/adjustment — anything with a BlockCore)
// ---------------------------------------------------------------------------

fn block_core<'a>(
	project: &'a Project,
	id: NodeId,
) -> Option<&'a oak_node::block::BlockCore> {
	let any = project.graph.get(id)?.behavior.as_any()?;
	[
		any.downcast_ref::<ClipBlockBehavior>().map(|b| &b.core),
		any.downcast_ref::<GapBlockBehavior>().map(|b| &b.core),
		any.downcast_ref::<TransitionBlockBehavior>().map(|b| &b.core),
		any.downcast_ref::<AdjustmentBlockBehavior>().map(|b| &b.core),
	]
	.into_iter()
	.flatten()
	.next()
}

macro_rules! block_rational {
	($( $name:ident => |$c:ident| $body:expr ),* $(,)?) => {
		$(
			#[doc = "BlockCore read (null rational on null/non-block)."]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(
				project: *const ProjectHandle,
				id: u64,
			) -> OakRational {
				let null = OakRational { num: 0, den: 0 };
				let Some(id) = node_id(id) else { return null };
				with_project(project, |p| {
					block_core(p, id).map(|$c| OakRational::from_native(&$body))
				})
				.flatten()
				.unwrap_or(null)
			}
		)*
	};
}

block_rational! {
	oak_block_in => |c| c.in_(),
	oak_block_out => |c| c.out(),
	oak_block_length => |c| c.length(),
	oak_block_media_in => |c| c.media_in,
	oak_block_media_out => |c| c.media_out(),
}

/// The track's display height in pixels (24px floor, 64px default).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_height_pixels(
	project: *const ProjectHandle,
	id: u64,
) -> f64 {
	let Some(id) = node_id(id) else { return 64.0 };
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id)
			.map(|t| {
				f64::from(
					oak_node::track::internal_height_to_pixel_height(t.height).max(24),
				)
			})
			.unwrap_or(64.0)
	})
	.unwrap_or(64.0)
}

/// The track's owning track-list token (invalid when not a track).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_list(project: *const ProjectHandle, id: u64) -> u64 {
	let Some(id) = node_id(id) else {
		return OAK_NODE_ID_INVALID;
	};
	with_project(project, |p| {
		behavior_as::<TrackBehavior>(p, id)
			.and_then(|t| t.track_list)
			.map(|l| l.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The track's index inside its list (-1 when not a track).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_index(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return -1 };
	with_project(project, |p| behavior_as::<TrackBehavior>(p, id).map(|t| t.index))
		.flatten()
		.unwrap_or(-1)
}

/// Creates a generator node from the factory table (invalid when the type
/// id is unknown or not a generator).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_generator_create(
	project: *const ProjectHandle,
	type_id: *const u8,
	type_id_len: usize,
) -> u64 {
	let Some(type_id) = (unsafe { str_arg(type_id, type_id_len) }) else {
		return OAK_NODE_ID_INVALID;
	};
	with_project_mut(project, |g| {
		let Some((core, behavior)) = oak_node::factory::Factory::global().create_any(type_id)
		else {
			return OAK_NODE_ID_INVALID;
		};
		if !behavior.categories().contains(&oak_node::node::Category::Generator) {
			return OAK_NODE_ID_INVALID;
		}
		g.graph.add_node(core, behavior).identity()
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Removes a project setting key (false on null/missing key or project).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_unset_setting(
	project: *const ProjectHandle,
	key: *const u8,
	key_len: usize,
) -> bool {
	let Some(key) = (unsafe { str_arg(key, key_len) }) else {
		return false;
	};
	if key.is_empty() {
		return false;
	}
	with_project_mut(project, |g| {
		let removed = g.settings.remove(key).is_some();
		if removed {
			g.set_modified(true);
		}
		removed
	})
	.unwrap_or(false)
}

/// Converts a pixel track height to the graph's internal height units.
#[unsafe(no_mangle)]
pub extern "C" fn oak_track_internal_height_from_pixels(pixels: i32) -> f64 {
	oak_node::track::pixel_height_to_internal_height(pixels)
}

/// The graph's virtual input/output endpoint tokens (`false` when the
/// project has no endpoints).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_graph_endpoints(
	project: *const ProjectHandle,
	input: *mut u64,
	output: *mut u64,
) -> bool {
	if input.is_null() || output.is_null() {
		return false;
	}
	with_project(project, |p| p.graph.endpoints())
		.flatten()
		.map(|(i, o)| {
			unsafe {
				*input = i.identity();
				*output = o.identity();
			}
			true
		})
		.unwrap_or(false)
}

/// The block's owning track token (invalid when trackless).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_track(project: *const ProjectHandle, id: u64) -> u64 {
	let Some(id) = node_id(id) else {
		return OAK_NODE_ID_INVALID;
	};
	with_project(project, |p| {
		block_core(p, id)
			.and_then(|c| c.track)
			.map(|t| t.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The block's playback speed (`1.0` for non-blocks).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_speed(project: *const ProjectHandle, id: u64) -> f64 {
	let Some(id) = node_id(id) else { return 1.0 };
	with_project(project, |p| block_core(p, id).map(|c| c.speed))
		.flatten()
		.unwrap_or(1.0)
}

/// Number of block links (the linked A/V group).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_link_count(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return 0 };
	with_project(project, |p| {
		block_core(p, id).map(|c| c.links.len() as i32)
	})
	.flatten()
	.unwrap_or(0)
}

/// One linked block token (invalid when out of range).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_link_at(
	project: *const ProjectHandle,
	id: u64,
	index: i32,
) -> u64 {
	let Some(id) = node_id(id) else {
		return OAK_NODE_ID_INVALID;
	};
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		block_core(p, id)
			.and_then(|c| c.links.get(index as usize).copied())
			.map(|l| l.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The node's creation-time color override (-1 when unset).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_color(project: *const ProjectHandle, id: u64) -> i32 {
	let Some(id) = node_id(id) else { return -1 };
	with_project(project, |p| p.graph.get(id).map(|e| e.core.override_color))
		.flatten()
		.unwrap_or(-1)
}

/// The clip's connected footage token (invalid when not a clip).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_footage(project: *const ProjectHandle, id: u64) -> u64 {
	let Some(id) = node_id(id) else {
		return OAK_NODE_ID_INVALID;
	};
	with_project(project, |p| {
		behavior_as::<ClipBlockBehavior>(p, id)
			.and_then(|c| c.footage)
			.map(|f| f.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// The multicam source the block reads from, when its texture input is a
/// multicam node (invalid otherwise).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_multicam_source(
	project: *const ProjectHandle,
	id: u64,
) -> u64 {
	let Some(id) = node_id(id) else {
		return OAK_NODE_ID_INVALID;
	};
	with_project(project, |p| {
		p.graph
			.connected_output(id, oak_node::block::clip_input::TEXTURE_INPUT, -1)
			.filter(|&source| {
				p.graph
					.get(source)
					.is_some_and(|e| e.behavior.type_id() == "org.olivevideoeditor.Olive.multicam")
			})
			.map(|s| s.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Width (seconds) of the transition drawn on one edge of `clip`: the
/// transition before it for `start_edge`, after it otherwise. `0/0` when
/// the edge carries no transition.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_transition_width(
	project: *const ProjectHandle,
	id: u64,
	start_edge: bool,
) -> OakRational {
	let null = OakRational { num: 0, den: 0 };
	let Some(id) = node_id(id) else { return null };
	with_project(project, |p| {
		let core = block_core(p, id)?;
		let track = core.track?;
		let blocks = &behavior_as::<TrackBehavior>(p, track)?.blocks;
		let index = blocks.iter().position(|&b| b == id)?;
		let candidate = if start_edge {
			blocks.get(index.checked_sub(1)?).copied()?
		} else {
			blocks.get(index + 1).copied()?
		};
		let input = if start_edge {
			oak_node::block::transition_input::IN_BLOCK
		} else {
			oak_node::block::transition_input::OUT_BLOCK
		};
		if p.graph.connected_output(candidate, input, -1) != Some(id) {
			return None;
		}
		let transition = behavior_as::<TransitionBlockBehavior>(p, candidate)?;
		Some(OakRational::from_native(if start_edge {
			&transition.out_offset
		} else {
			&transition.in_offset
		}))
	})
	.flatten()
	.unwrap_or(null)
}

/// Number of live nodes in the project graph (arena order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_node_count(project: *const ProjectHandle) -> i32 {
	with_project(project, |p| p.graph.node_ids().len() as i32).unwrap_or(0)
}

/// The `index`-th live node token in arena order (invalid when out of
/// range).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_node_at(
	project: *const ProjectHandle,
	index: i32,
) -> u64 {
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	with_project(project, |p| {
		p.graph
			.node_ids()
			.get(index as usize)
			.map(|id| id.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

// ---------------------------------------------------------------------------
// Node parameter reads (OakNodeValue POD mirror)
// ---------------------------------------------------------------------------

/// The input's static value. False on null input, unknown node/input, or
/// a value without a POD representation (e.g. strings — string reads are
/// not bridged yet).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_standard_value(
	project: *const ProjectHandle,
	id: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
	out: *mut oak_node::value::OakNodeValue,
) -> bool {
	read_value(project, id, input_id, input_id_len, element, None, out)
}

/// The input's value at `time` (keyframe-interpolated when animated).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_value_at_time(
	project: *const ProjectHandle,
	id: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
	time: OakRational,
	out: *mut oak_node::value::OakNodeValue,
) -> bool {
	read_value(
		project,
		id,
		input_id,
		input_id_len,
		element,
		Some(time.to_native()),
		out,
	)
}

fn read_value(
	project: *const ProjectHandle,
	token: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
	time: Option<oak_core::Rational>,
	out: *mut oak_node::value::OakNodeValue,
) -> bool {
	if out.is_null() {
		return false;
	}
	let Some(id) = node_id(token) else { return false };
	let Some(input) = (unsafe { str_arg(input_id, input_id_len) }) else {
		return false;
	};
	let value = with_project(project, |p| {
		let entry = p.graph.get(id)?;
		let declared = entry.core.input_data_type(input)?;
		let v = match time {
			Some(t) => entry.core.value_at_time(input, element, t),
			None => entry.core.standard_value(input, element),
		};
		oak_node::value::OakNodeValue::from_node_value(declared, &v).ok()
	})
	.flatten();
	match value {
		Some(v) => {
			unsafe { *out = v };
			true
		}
		None => false,
	}
}

// ---------------------------------------------------------------------------
// Undo stack (read-only)
// ---------------------------------------------------------------------------

/// Whether the global undo stack can undo.
/// The node's user label, snprintf-style; -1 on a null handle or
/// unknown id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_label(
	project: *const ProjectHandle,
	id: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(id) = NodeId::from_identity(id) else {
		return -1;
	};
	match with_project(project, |g| g.graph.get(id).map(|e| e.core.label.clone())).flatten() {
		Some(label) => copy_str_out(&label, buf, buf_len),
		None => -1,
	}
}

/// Sets the node's user label (a direct graph write, not an undoable
/// edit — matching `oak_task::nodeops::set_node_label`). False on a null
/// handle, unknown id, or invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_set_label(
	project: *const ProjectHandle,
	id: u64,
	label: *const u8,
	label_len: usize,
) -> bool {
	let Some(id) = NodeId::from_identity(id) else {
		return false;
	};
	let Some(label) = (unsafe { str_arg(label, label_len) }) else {
		return false;
	};
	with_project_mut(project, |g| {
		let Some(entry) = g.graph.get_mut(id) else {
			return false;
		};
		entry.core.label = label.to_string();
		true
	})
	.unwrap_or(false)
}

/// The sequence's length (its `last_length`); the null rational (0/0)
/// on a null handle, a non-sequence id, or an unset length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_length(
	project: *const ProjectHandle,
	id: u64,
) -> OakRational {
	let Some(id) = NodeId::from_identity(id) else {
		return OakRational::default();
	};
	with_project(project, |g| {
		g.graph
			.get(id)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<SequenceBehavior>())
			.map(|s| OakRational::from_native(&s.last_length))
	})
	.flatten()
	.unwrap_or_default()
}

/// The sequence's frame rate (its first video stream); the null
/// rational (0/0) when it has no video params.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_frame_rate(
	project: *const ProjectHandle,
	id: u64,
) -> OakRational {
	let Some(id) = NodeId::from_identity(id) else {
		return OakRational::default();
	};
	with_project(project, |g| {
		g.graph
			.get(id)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<SequenceBehavior>())
			.and_then(|s| s.video_params.first())
			.map(|v| OakRational::from_native(&v.frame_rate))
	})
	.flatten()
	.unwrap_or_default()
}

/// The sequence's output geometry (its first video stream). False on a
/// null handle, a non-sequence id, or no video params.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_geometry(
	project: *const ProjectHandle,
	id: u64,
	width: *mut i32,
	height: *mut i32,
) -> bool {
	let Some(id) = NodeId::from_identity(id) else {
		return false;
	};
	let params = with_project(project, |g| {
		g.graph
			.get(id)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<SequenceBehavior>())
			.and_then(|s| s.video_params.first())
			.map(|v| (v.width, v.height))
	})
	.flatten();
	let Some((w, h)) = params else {
		return false;
	};
	if !width.is_null() {
		unsafe { *width = w };
	}
	if !height.is_null() {
		unsafe { *height = h };
	}
	true
}

/// Sets the sequence's output video params (width/height/frame rate;
/// pixel format stays F32 RGBA). False on a null handle or a
/// non-sequence id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_set_video_params(
	project: *const ProjectHandle,
	id: u64,
	width: i32,
	height: i32,
	frame_rate_num: i32,
	frame_rate_den: i32,
) -> bool {
	let Some(id) = NodeId::from_identity(id) else {
		return false;
	};
	with_project_mut(project, |g| {
		let Some(s) = g
			.graph
			.get_mut(id)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<SequenceBehavior>())
		else {
			return false;
		};
		if s.video_params.is_empty() {
			s.video_params.push(oak_node::value::VideoParams {
				width,
				height,
				frame_rate: oak_core::Rational::new(
					i64::from(frame_rate_num),
					i64::from(frame_rate_den),
				),
				pixel_format: oak_core::PixelFormat::F32 as i32,
				channels: 4,
				interlaced: false,
			});
		} else {
			let v = &mut s.video_params[0];
			v.width = width;
			v.height = height;
			v.frame_rate = oak_core::Rational::new(
				i64::from(frame_rate_num),
				i64::from(frame_rate_den),
			);
		}
		true
	})
	.unwrap_or(false)
}

/// Sets the sequence's interlaced flag (no-op on null/non-sequence).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_set_interlaced(
	project: *const ProjectHandle,
	id: u64,
	interlaced: bool,
) -> bool {
	let Some(id) = NodeId::from_identity(id) else {
		return false;
	};
	with_project_mut(project, |g| {
		let Some(s) = g
			.graph
			.get_mut(id)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<SequenceBehavior>())
		else {
			return false;
		};
		if let Some(v) = s.video_params.first_mut() {
			v.interlaced = interlaced;
		}
		true
	})
	.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_can_undo() -> bool {
	oak_undo::global::can_undo()
}

/// Whether the global undo stack can redo.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_can_redo() -> bool {
	oak_undo::global::can_redo()
}

/// Number of commands on the undo stack (-1 on error).
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_count() -> i64 {
	oak_undo::global::count().unwrap_or(-1)
}

/// The current position on the undo stack (-1 on error).
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_index() -> i64 {
	oak_undo::global::index().unwrap_or(-1)
}

/// Serializes every test that touches the process-global undo stack
/// (all three test modules in this file share it).
#[cfg(test)]
pub(crate) static UNDO_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
	use super::*;

	fn cstr_arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	/// A project with one footage node in the root folder.
	fn project_with_footage() -> *mut ProjectHandle {
		let project = oak_project_new();
		assert!(!project.is_null());
		// Build a footage node through oak-node directly (the engine's
		// read API is what's under test).
		{
			let inner = unsafe { &*project }.inner.clone();
			let mut g = inner.lock().unwrap();
			let (core, mut behavior) = oak_node::footage::FootageBehavior::create();
			{
				let b = behavior
					.as_any_mut()
					.unwrap()
					.downcast_mut::<FootageBehavior>()
					.unwrap();
				b.filename = "/tmp/oak-node-test.mp4".to_string();
			}
			let id = g.graph.add_node(core, behavior);
			let root = g.root;
			g.graph
				.get_mut(root)
				.unwrap()
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<FolderBehavior>()
				.unwrap()
				.add_child(id);
		}
		project
	}

	#[test]
	fn id_tokens_and_project_basics() {
		assert_eq!(oak_node_id_invalid(), u32::MAX as u64);
		assert!(!oak_node_id_is_valid(oak_node_id_invalid()));

		let project = oak_project_new();
		unsafe {
			let root = oak_project_root(project);
			assert!(oak_node_id_is_valid(root), "initialized project has a root");
			assert_eq!(oak_node_kind(project, root), 1, "root is a folder");
			assert_eq!(oak_folder_child_count(project, root), 0);

			let mut buf = [0u8; 128];
			let n = oak_node_type_id(project, root, buf.as_mut_ptr(), buf.len());
			assert!(std::str::from_utf8(&buf[..n as usize]).unwrap().ends_with("folder"));
			assert!(oak_project_filename(project, buf.as_mut_ptr(), buf.len()) >= 0);
			assert!(!oak_project_modified(project));

			oak_project_release(project);
		}
	}

	/// Node labels and sequence introspection through the C ABI.
	#[test]
	fn node_label_and_sequence_params() {
		// Folder creation is undoable; serialize against the other
		// undo-stack tests.
		let _guard = UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let root = oak_project_root(project);
			let (n, nl) = ("Labeled".as_ptr(), 7);
			let folder = oak_folder_create(project, root, n, nl);
			assert!(oak_node_id_is_valid(folder));

			let mut buf = [0u8; 64];
			let len = oak_node_label(project, folder, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..len as usize]).unwrap(), "Labeled");
			assert!(oak_node_set_label(project, folder, "Renamed".as_ptr(), 7));
			let len = oak_node_label(project, folder, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..len as usize]).unwrap(), "Renamed");
			assert_eq!(oak_node_label(project, u64::MAX, buf.as_mut_ptr(), 64), -1);
			assert!(!oak_node_set_label(project, u64::MAX, "x".as_ptr(), 1));

			let (s, sl) = ("Seq".as_ptr(), 3);
			let seq = oak_sequence_create(project, s, sl);
			// A fresh sequence carries the default 29.97 fps video params.
			assert_eq!(oak_sequence_frame_rate(project, seq), OakRational { num: 30000, den: 1001 });
			assert!(oak_sequence_geometry(project, seq, std::ptr::null_mut(), std::ptr::null_mut()));
			assert!(oak_sequence_set_video_params(project, seq, 1920, 1080, 24, 1));
			assert_eq!(oak_sequence_frame_rate(project, seq), OakRational { num: 24, den: 1 });
			let (mut w, mut h) = (0, 0);
			assert!(oak_sequence_geometry(project, seq, &mut w, &mut h));
			assert_eq!((w, h), (1920, 1080));
			assert_eq!(oak_sequence_length(project, seq), OakRational { num: 0, den: 1 });
			assert!(!oak_sequence_set_video_params(project, u64::MAX, 1, 1, 1, 1));

			oak_project_release(project);
			assert!(oak_undo::global::clear().is_ok());
		}
	}

	#[test]
	fn active_project_slot() {
		unsafe {
			oak_project_set_active(std::ptr::null());
			assert!(oak_project_active().is_null());

			let project = oak_project_new();
			oak_project_set_active(project);
			let active = oak_project_active();
			assert!(!active.is_null());
			// Same underlying project: the root token matches.
			assert_eq!(oak_project_root(active), oak_project_root(project));

			oak_project_set_active(std::ptr::null());
			assert!(oak_project_active().is_null());

			oak_project_release(active);
			oak_project_release(project);
		}
	}

	#[test]
	fn footage_traversal() {
		let project = project_with_footage();
		unsafe {
			let root = oak_project_root(project);
			assert_eq!(oak_folder_child_count(project, root), 1);
			let child = oak_folder_child_at(project, root, 0);
			assert!(oak_node_id_is_valid(child));
			assert_eq!(oak_node_kind(project, child), 2, "footage");

			let mut buf = [0u8; 128];
			let n = oak_footage_filename(project, child, buf.as_mut_ptr(), buf.len());
			assert_eq!(
				std::str::from_utf8(&buf[..n as usize]).unwrap(),
				"/tmp/oak-node-test.mp4"
			);

			// Unprobed: zero streams.
			assert_eq!(oak_footage_total_stream_count(project, child), 0);
			assert!(!oak_footage_video_params(project, child, 0, std::ptr::null_mut()));

			oak_project_release(project);
		}
	}

	#[test]
	fn sequence_track_tracklist_traversal() {
		let project = oak_project_new();
		{
			let inner = unsafe { &*project }.inner.clone();
			let mut g = inner.lock().unwrap();
			// sequence → tracklist(video) → track(video)
			let (seq_core, seq_b) = oak_node::sequence::SequenceBehavior::create();
			let seq = g.graph.add_node(seq_core, seq_b);
			let (tl_core, tl_b) = oak_node::track::TrackListBehavior::create();
			let tl = g.graph.add_node(tl_core, tl_b);
			let (tr_core, tr_b) = oak_node::track::TrackBehavior::create();
			let tr = g.graph.add_node(tr_core, tr_b);

			let tl_b = g
				.graph
				.get_mut(tl)
				.unwrap()
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<TrackListBehavior>()
				.unwrap();
			tl_b.tracks.push(tr);
			let seq_b = g
				.graph
				.get_mut(seq)
				.unwrap()
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<SequenceBehavior>()
				.unwrap();
			seq_b.track_lists.push(tl);
		}
		unsafe {
			let root = oak_project_root(project);
			let _ = root;
			// find the sequence via graph scan (it is not in the bin)
			// — simpler: the traversal entry points are exercised directly.
			// (sequence id is not discoverable via the folder tree)
			let mut found_seq = 0u64;
			{
				let inner = &*project;
				let g = inner.inner.lock().unwrap();
				for id in g.graph.node_ids() {
					if g.graph.get(id).unwrap().behavior.type_id()
						== "org.olivevideoeditor.Olive.sequence"
					{
						found_seq = id.identity();
					}
				}
			}
			assert_eq!(oak_sequence_track_list_count(project, found_seq), 1);
			let tl = oak_sequence_track_list_at(project, found_seq, 0);
			assert_eq!(oak_tracklist_track_count(project, tl), 1);
			let tr = oak_tracklist_track_at(project, tl, 0);
			assert_eq!(oak_track_block_count(project, tr), 0);
			oak_project_release(project);
		}
	}

	#[test]
	fn param_reads_and_undo_readonly() {
		let project = project_with_footage();
		unsafe {
			let root = oak_project_root(project);
			let child = oak_folder_child_at(project, root, 0);

			// footage has a "file_in" text input — not POD-representable,
			// so the read must fail cleanly (false), not crash.
			let mut out = oak_node::value::OakNodeValue::none();
			let (i, il) = cstr_arg("file_in");
			assert!(!oak_node_standard_value(project, child, i, il, -1, &mut out));
			// Unknown input id: false.
			let (i, il) = cstr_arg("no_such_input");
			assert!(!oak_node_standard_value(project, child, i, il, -1, &mut out));
			// value_at_time on a static text input: also false (string).
			let (i, il) = cstr_arg("file_in");
			assert!(!oak_node_value_at_time(
				project,
				child,
				i,
				il,
				-1,
				OakRational { num: 0, den: 1 },
				&mut out,
			));

			// Fresh process-wide stack state is whatever the suite left;
			// just verify the calls work.
			let _ = oak_undo_can_undo();
			let _ = oak_undo_can_redo();
			assert!(oak_undo_count() >= 0);
			assert!(oak_undo_index() >= 0 || oak_undo_count() == 0);

			oak_project_release(project);
		}
	}

	#[test]
	fn null_safety_sweep() {
		unsafe {
			let mut buf = [0u8; 64];
			assert_eq!(oak_node_kind(std::ptr::null(), 1), 0);
			assert_eq!(oak_node_type_id(std::ptr::null(), 1, buf.as_mut_ptr(), 64), -1);
			assert_eq!(oak_folder_child_count(std::ptr::null(), 1), 0);
			assert_eq!(oak_folder_child_at(std::ptr::null(), 1, 0), OAK_NODE_ID_INVALID);
			assert_eq!(oak_footage_filename(std::ptr::null(), 1, buf.as_mut_ptr(), 64), -1);
			assert_eq!(oak_footage_duration(std::ptr::null(), 1).den, 0);
			assert_eq!(oak_sequence_track_list_count(std::ptr::null(), 1), 0);
			assert_eq!(oak_tracklist_kind(std::ptr::null(), 1), -1);
			assert_eq!(oak_track_length(std::ptr::null(), 1).num, 0);
			assert_eq!(oak_block_in(std::ptr::null(), 1).den, 0);
			assert!(!oak_project_modified(std::ptr::null()));
			assert_eq!(oak_project_root(std::ptr::null()), OAK_NODE_ID_INVALID);
			oak_project_release(std::ptr::null());
		}
	}
}

// ---------------------------------------------------------------------------
// Batch 2: editing (undo-integrated)
// ---------------------------------------------------------------------------
//
// Every mutation pushes an oak-timeline/oak-task undo command through the
// global undo stack, exactly like the app's own edits. Plugins batch
// several calls into one undo step with oak_undo_group_begin/end.

use oak_node::value::NodeValue;
use oak_timeline::util::NodeRef;
use oak_undo::undocommand::UndoCommand;

fn with_project_mut<R>(project: *const ProjectHandle, f: impl FnOnce(&mut Project) -> R) -> Option<R> {
	if project.is_null() {
		return None;
	}
	let mut guard = unsafe { &*project }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	Some(f(&mut guard))
}

fn push_undo(cmd: UndoCommand, name: &str) -> bool {
	oak_undo::global::push(cmd, name).is_ok()
}

/// Begins an undo group (nested calls inside form one undo step). False
/// on invalid name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_undo_group_begin(name: *const u8, name_len: usize) -> bool {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return false;
	};
	oak_undo::global::group_begin(name).is_ok()
}

/// Closes the current undo group. False when none is open.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_group_end() -> bool {
	oak_undo::global::group_end().is_ok()
}

/// Aborts the current undo group, rolling it back. False when none is
/// open.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_group_abort() -> bool {
	oak_undo::global::group_abort().is_ok()
}

/// Undoes one step. False when the stack is empty.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_undo() -> bool {
	oak_undo::global::undo().is_ok()
}

/// Redoes one step. False when nothing to redo.
#[unsafe(no_mangle)]
pub extern "C" fn oak_undo_redo() -> bool {
	oak_undo::global::redo().is_ok()
}

/// Imports a media file into the project's bin (probes first; a failed
/// probe leaves nothing behind). Undoable. Returns the footage node
/// token, or the invalid token on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_footage_import(
	project: *const ProjectHandle,
	filename: *const u8,
	filename_len: usize,
) -> u64 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return OAK_NODE_ID_INVALID;
	};
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	if !std::path::Path::new(filename).exists() {
		return OAK_NODE_ID_INVALID;
	}
	let id = {
		let Some(id) = with_project_mut(project, |guard| {
			if !guard.root.valid() {
				return None;
			}
			let (mut core, mut behavior) = FootageBehavior::create();
			core.set_standard_value(
				"file_in",
				-1,
				NodeValue::Text(filename.to_string()),
			);
			let f = behavior
				.as_any_mut()
				.and_then(|a| a.downcast_mut::<FootageBehavior>())?;
			f.filename = filename.to_string();
			// Probe before the node enters the graph so a failed probe
			// leaves no orphan behind (and nothing on the undo stack).
			f.probe().ok()?;
			let id = guard.graph.add_node(core, behavior);
			if let Some(e) = guard.graph.get_mut(id) {
				e.core.label = std::path::Path::new(filename)
					.file_name()
					.map(|f| f.to_string_lossy().into_owned())
					.unwrap_or_else(|| filename.to_string());
			}
			Some(id)
		})
		.flatten() else {
			return OAK_NODE_ID_INVALID;
		};
		id
	};
	let root = unsafe { &*project }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.root;
	let project_ref = unsafe { &*project }.inner.clone();
	if !push_undo(
		oak_task::nodeops::folder_add_child_command(
			(project_ref.clone(), root),
			(project_ref, id),
		),
		"Import Footage",
	) {
		return OAK_NODE_ID_INVALID;
	}
	id.identity()
}

/// Creates a folder under `parent` (the root token works). Undoable.
/// Returns the folder token, invalid on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_folder_create(
	project: *const ProjectHandle,
	parent: u64,
	name: *const u8,
	name_len: usize,
) -> u64 {
	let Some(parent) = node_id(parent) else {
		return OAK_NODE_ID_INVALID;
	};
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return OAK_NODE_ID_INVALID;
	};
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	let id = with_project_mut(project, |guard| {
		let (mut core, behavior) = oak_node::folder::create(name);
		core.label = name.to_string();
		guard.graph.add_node(core, behavior)
	});
	let Some(id) = id else {
		return OAK_NODE_ID_INVALID;
	};
	let project_ref = unsafe { &*project }.inner.clone();
	if !push_undo(
		oak_task::nodeops::folder_add_child_command(
			(project_ref.clone(), parent),
			(project_ref, id),
		),
		"Create Folder",
	) {
		return OAK_NODE_ID_INVALID;
	}
	id.identity()
}

/// Removes a node from the project entirely. Undoable. False on
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_remove(project: *const ProjectHandle, id: u64) -> bool {
	let Some(id) = node_id(id) else { return false };
	if project.is_null() {
		return false;
	}
	let project_ref = unsafe { &*project }.inner.clone();
	push_undo(oak_task::nodeops::remove_node_command(project_ref, id), "Remove Node")
}

/// Creates a sequence with the default 2 video + 2 audio track layout,
/// mounted in the bin root. Returns the sequence token, invalid on
/// failure. (Like the app, creation itself is not undoable.)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_create(
	project: *const ProjectHandle,
	name: *const u8,
	name_len: usize,
) -> u64 {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return OAK_NODE_ID_INVALID;
	};
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	let Some(seq) = with_project_mut(project, |guard| {
		let (mut core, behavior) = SequenceBehavior::create();
		core.label = name.to_string();
		guard.graph.add_node(core, behavior)
	}) else {
		return OAK_NODE_ID_INVALID;
	};
	let project_ref = unsafe { &*project }.inner.clone();
	let root = {
		let guard = project_ref.lock().unwrap_or_else(|e| e.into_inner());
		guard.root
	};
	// Mount under the bin root. Not pushed: part of creation, like the app.
	oak_task::nodeops::folder_add_child_command(
		(project_ref.clone(), root),
		(project_ref.clone(), seq),
	)
	.redo_now();
	// The default 2V+2A layout, also part of creation.
	for kind in [
		oak_node::track::TrackType::Video,
		oak_node::track::TrackType::Video,
		oak_node::track::TrackType::Audio,
		oak_node::track::TrackType::Audio,
	] {
		let Some(list) = find_or_create_track_list(&project_ref, seq, kind) else {
			continue;
		};
		oak_timeline::undogeneral::TimelineAddTrackCommand::new(NodeRef::new(
			project_ref.clone(),
			list,
		))
		.redo();
	}
	seq.identity()
}

/// The sequence's track list for `kind` (creating it when absent).
fn find_or_create_track_list(
	project: &Arc<Mutex<Project>>,
	seq: NodeId,
	kind: oak_node::track::TrackType,
) -> Option<NodeId> {
	let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
	// Existing list of this kind?
	let existing = behavior_as::<SequenceBehavior>(&guard, seq)?.track_lists.iter().find_map(
		|&list| {
			let matches = behavior_as::<TrackListBehavior>(&guard, list)
				.is_some_and(|l| l.kind == kind);
			matches.then_some(list)
		},
	);
	if existing.is_some() {
		return existing;
	}
	let (core, mut behavior) = TrackListBehavior::create();
	if let Some(list) = behavior
		.as_any_mut()
		.and_then(|a| a.downcast_mut::<TrackListBehavior>())
	{
		list.kind = kind;
		list.array_base =
			behavior_as::<SequenceBehavior>(&guard, seq)?.track_lists.len() as i32;
	}
	let list_id = guard.graph.add_node(core, behavior);
	if let Some(s) = guard
		.graph
		.get_mut(seq)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<SequenceBehavior>())
	{
		s.track_lists.push(list_id);
	}
	if let Some(l) = guard
		.graph
		.get_mut(list_id)
		.and_then(|e| e.behavior.as_any_mut())
		.and_then(|a| a.downcast_mut::<TrackListBehavior>())
	{
		l.sequence = Some(seq);
	}
	Some(list_id)
}

/// Adds a track to the sequence's `kind` list (creating the list when
/// absent). Undoable. Returns the new track's token, invalid on failure.
/// `kind`: the `TrackType` code (see `oak_track_kind`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_add(
	project: *const ProjectHandle,
	sequence: u64,
	kind: i32,
) -> u64 {
	let Some(kind) = oak_node::track::TrackType::from_c(kind) else {
		return OAK_NODE_ID_INVALID;
	};
	let Some(seq) = node_id(sequence) else {
		return OAK_NODE_ID_INVALID;
	};
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	let project_ref = unsafe { &*project }.inner.clone();
	let Some(list) = find_or_create_track_list(&project_ref, seq, kind) else {
		return OAK_NODE_ID_INVALID;
	};
	let track_cmd = oak_timeline::undogeneral::TimelineAddTrackCommand::new(NodeRef::new(
		project_ref,
		list,
	));
	// The command creates the track in its constructor; `track()` reports
	// it. Push (which runs redo) makes it live and undoable.
	let track = track_cmd.track().id;
	if !push_undo(UndoCommand::new(track_cmd), "Add Track") {
		return OAK_NODE_ID_INVALID;
	}
	track.identity()
}

// ---------------------------------------------------------------------------
// Block creation and track placement
// ---------------------------------------------------------------------------

macro_rules! block_create_fn {
	($( $name:ident => $create:expr, $doc:literal );* $(;)?) => {
		$(
			#[doc = $doc]
			/// Undo-free creation (placement on a track is the undoable
			/// step). Returns the block token, invalid on failure.
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(project: *const ProjectHandle) -> u64 {
				with_project_mut(project, |guard| {
					let (core, behavior) = $create();
					guard.graph.add_node(core, behavior)
				})
				.map(|id| id.identity())
				.unwrap_or(OAK_NODE_ID_INVALID)
			}
		)*
	};
}

block_create_fn! {
	oak_block_create_clip => oak_node::block::clip_create, "Creates a clip block (not on any track yet).";
	oak_block_create_gap => oak_node::block::gap_create, "Creates a gap block.";
	oak_block_create_transition => oak_node::block::transition_create, "Creates a transition block.";
	oak_block_create_adjustment => oak_node::block::adjustment_create, "Creates an adjustment block.";
}

/// Appends `block` to the end of `track`. Undoable. False on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_append_block(
	project: *const ProjectHandle,
	track: u64,
	block: u64,
) -> bool {
	oak_track_insert_block_after(project, track, block, OAK_NODE_ID_INVALID)
}

/// Inserts `block` after `before` (invalid token = append). Undoable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_insert_block_after(
	project: *const ProjectHandle,
	track: u64,
	block: u64,
	before: u64,
) -> bool {
	let (Some(track), Some(block)) = (node_id(track), node_id(block)) else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let project_ref = unsafe { &*project }.inner.clone();
	let before = node_id(before).map(|b| NodeRef::new(project_ref.clone(), b));
	push_undo(
		UndoCommand::new(oak_timeline::undotrack::TrackInsertBlockAfterCommand::new(
			NodeRef::new(project_ref.clone(), track),
			NodeRef::new(project_ref, block),
			before,
		)),
		"Insert Block",
	)
}

/// Removes `block` from `track`, closing the gap (ripple). Undoable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_ripple_remove_block(
	project: *const ProjectHandle,
	track: u64,
	block: u64,
) -> bool {
	let (Some(track), Some(block)) = (node_id(track), node_id(block)) else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let project_ref = unsafe { &*project }.inner.clone();
	push_undo(
		UndoCommand::new(oak_timeline::undotrack::TrackRippleRemoveBlockCommand::new(
			NodeRef::new(project_ref.clone(), track),
			NodeRef::new(project_ref, block),
		)),
		"Ripple Remove Block",
	)
}

/// Replaces `old` with `new` on `track`. Undoable. False on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_replace_block(
	project: *const ProjectHandle,
	track: u64,
	old: u64,
	new: u64,
) -> bool {
	let (Some(track), Some(old), Some(new)) = (node_id(track), node_id(old), node_id(new))
	else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let project_ref = unsafe { &*project }.inner.clone();
	push_undo(
		UndoCommand::new(oak_timeline::undotrack::TrackReplaceBlockCommand::new(
			NodeRef::new(project_ref.clone(), track),
			NodeRef::new(project_ref.clone(), old),
			NodeRef::new(project_ref, new),
		)),
		"Replace Block",
	)
}

// ---------------------------------------------------------------------------
// Block trimming (undoable)
// ---------------------------------------------------------------------------

macro_rules! block_trim_fn {
	($( $name:ident => $cmd:ident, $label:literal );* $(;)?) => {
		$(
			#[doc = $label]
			/// Undoable. False on failure.
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(
				project: *const ProjectHandle,
				block: u64,
				value: OakRational,
			) -> bool {
				use oak_timeline::util::NodeRef;
				let Some(block) = node_id(block) else { return false };
				if project.is_null() {
					return false;
				}
				let project_ref = unsafe { &*project }.inner.clone();
				push_undo(
					UndoCommand::new(oak_timeline::undogeneral::$cmd::new(
						NodeRef::new(project_ref, block),
						value.to_native(),
					)),
					$label,
				)
			}
		)*
	};
}

block_trim_fn! {
	oak_block_set_length => BlockResizeCommand, "Set Block Length";
	oak_block_set_length_and_media_in => BlockResizeWithMediaInCommand, "Set Block Length";
	oak_block_set_media_in => BlockSetMediaInCommand, "Set Media In";
}

// ---------------------------------------------------------------------------
// Parameter writes (undoable)
// ---------------------------------------------------------------------------

/// Sets the input's static value (POD types only; strings are not
/// bridged). Undoable. False on null input, unknown node/input, or a
/// non-POD value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_set_standard_value(
	project: *const ProjectHandle,
	id: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
	value: oak_node::value::OakNodeValue,
) -> bool {
	let Some(id) = node_id(id) else { return false };
	let Some(input) = (unsafe { str_arg(input_id, input_id_len) }) else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	set_standard_value_impl(unsafe { &*project }.project(), id, input, element, value).is_ok()
}

/// Sets one declared input's static POD value as a single undoable parameter edit.
///
/// The input's declared type drives conversion from the ABI value. Unknown
/// nodes/inputs and incompatible values return an error before an undo command
/// is constructed. The previous value is captured per input/element so undo
/// restores exactly that value rather than resetting the whole node.
pub(crate) fn set_standard_value_impl(
	project_ref: &Arc<Mutex<Project>>,
	id: NodeId,
	input: &str,
	element: i32,
	value: oak_node::value::OakNodeValue,
) -> Result<(), String> {
	let (new_value, old_value) = {
		let guard = project_ref.lock().unwrap_or_else(|e| e.into_inner());
		let Some(entry) = guard.graph.get(id) else {
			return Err("set parameter: node not found".into());
		};
		let Some(declared) = entry.core.input_data_type(&input) else {
			return Err(format!("set parameter: no input \"{input}\""));
		};
		let Ok(new_value) = value.to_node_value(declared) else {
			return Err(format!("set parameter: incompatible value for \"{input}\""));
		};
		let old_value = entry.core.standard_value(&input, element);
		(new_value, old_value)
	};

	let project_redo = project_ref.clone();
	let project_undo = project_ref.clone();
	let input_redo = input.to_string();
	let input_undo = input.to_string();
	oak_undo::global::push(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				let mut g = project_redo.lock().unwrap_or_else(|e| e.into_inner());
				if let Some(entry) = g.graph.get_mut(id) {
					entry.core.set_standard_value(&input_redo, element, new_value.clone());
				}
			},
			move || {
				let mut g = project_undo.lock().unwrap_or_else(|e| e.into_inner());
				if let Some(entry) = g.graph.get_mut(id) {
					entry.core.set_standard_value(&input_undo, element, old_value.clone());
				}
			},
		),
		"Set Parameter",
	)
	.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Graph edges (document structure: persisted in the project file)
// ---------------------------------------------------------------------------

/// Connects `from`'s output to `to`'s `input_id` at `element` (-1 = the
/// scalar input). False on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_connect(
	project: *const ProjectHandle,
	from: u64,
	to: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
) -> bool {
	let (Some(from), Some(to)) = (node_id(from), node_id(to)) else {
		return false;
	};
	let Some(input) = (unsafe { str_arg(input_id, input_id_len) }) else {
		return false;
	};
	with_project_mut(project, |guard| guard.graph.connect(from, to, input, element).is_ok())
		.unwrap_or(false)
}

/// Disconnects `to`'s `input_id` at `element`. False on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_node_disconnect(
	project: *const ProjectHandle,
	to: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
) -> bool {
	let Some(to) = node_id(to) else { return false };
	let Some(input) = (unsafe { str_arg(input_id, input_id_len) }) else {
		return false;
	};
	with_project_mut(project, |guard| {
		guard.graph.disconnect_input(to, input, element);
		true
	})
	.unwrap_or(false)
}

#[cfg(test)]
mod edit_tests {
	use super::*;


	fn temp_clip(name: &str) -> String {
		let dir = std::env::temp_dir().join(format!(
			"oakengine_nodeedit_{}_{}",
			name,
			std::process::id()
		));
		let _ = std::fs::create_dir_all(&dir);
		let path = dir.join("clip.mp4");
		oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("generate test media");
		path.to_string_lossy().into_owned()
	}

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	/// The money test: import footage → create sequence → create clip →
	/// place on track → trim → undo the whole chain back to the empty
	/// project.
	#[test]
	fn edit_chain_and_undo_rollback() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let undo_base = oak_undo_count();

			// Import footage (undoable).
			let clip_path = temp_clip("chain");
			let (f, fl) = arg(&clip_path);
			let footage = oak_footage_import(project, f, fl);
			assert!(oak_node_id_is_valid(footage), "import the test clip");
			let root = oak_project_root(project);
			assert_eq!(oak_folder_child_count(project, root), 1);
			assert_eq!(oak_folder_child_at(project, root, 0), footage);
			assert!(oak_footage_video_stream_count(project, footage) >= 1);

			// Create a sequence (creation itself is not undoable, like the
			// app's flow).
			let (n, nl) = arg("Seq 1");
			let seq = oak_sequence_create(project, n, nl);
			assert!(oak_node_id_is_valid(seq));
			assert_eq!(oak_sequence_track_list_count(project, seq), 2);

			// Create a clip and place it on the first video track.
			let clip = oak_block_create_clip(project);
			assert!(oak_node_id_is_valid(clip));
			assert_eq!(oak_node_kind(project, clip), 5);

			let tracklist = oak_sequence_track_list_at(project, seq, 0);
			let track = oak_tracklist_track_at(project, tracklist, 0);
			assert!(oak_track_append_block(project, track, clip));
			assert_eq!(oak_track_block_count(project, track), 1);
			assert_eq!(oak_track_block_at(project, track, 0), clip);

			// Trim it.
			assert!(oak_block_set_length(
				project,
				clip,
				OakRational { num: 5, den: 1 }
			));
			assert_eq!(
				oak_block_length(project, clip),
				OakRational { num: 5, den: 1 }
			);

			// Undo: trim, placement, import — in reverse order.
			assert!(oak_undo_undo(), "undo the trim");
			assert_ne!(
				oak_block_length(project, clip),
				OakRational { num: 5, den: 1 },
				"the trim was rolled back"
			);
			assert!(oak_undo_undo(), "undo the placement");
			assert_eq!(oak_track_block_count(project, track), 0);
			assert!(oak_undo_undo(), "undo the import");
			// The sequence stays: its mount is part of its creation, not
			// an undoable edit (same as the app).
			assert_eq!(oak_folder_child_count(project, root), 1);
			assert_eq!(oak_folder_child_at(project, root, 0), seq);

			// Redo everything.
			assert!(oak_undo_redo());
			assert!(oak_undo_redo());
			assert!(oak_undo_redo());
			assert_eq!(oak_folder_child_count(project, root), 2);
			assert_eq!(oak_track_block_count(project, track), 1);
			assert_eq!(
				oak_block_length(project, clip),
				OakRational { num: 5, den: 1 }
			);
			assert_eq!(oak_undo_count(), undo_base + 3);

			oak_project_release(project);
		}
	}

	#[test]
	fn undo_groups_batch_edits() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let base = oak_undo_count();

			let (n, nl) = arg("Import Two Clips");
			assert!(oak_undo_group_begin(n, nl));
			let clip_path = temp_clip("group");
			let (f, fl) = arg(&clip_path);
			let f1 = oak_footage_import(project, f, fl);
			assert!(oak_node_id_is_valid(f1));
			assert!(oak_undo_group_end());
			assert_eq!(oak_undo_count(), base + 1, "the group is one undo step");

			assert!(oak_undo_undo());
			assert!(oak_undo_redo());

			// group_abort rolls back and records nothing.
			let (n, nl) = arg("aborted");
			assert!(oak_undo_group_begin(n, nl));
			let f2 = oak_footage_import(project, f, fl);
			assert!(oak_node_id_is_valid(f2));
			assert!(oak_undo_group_abort());
			assert_eq!(oak_undo_count(), base + 1, "abort records nothing");
			let root = oak_project_root(project);
			assert_eq!(oak_folder_child_count(project, root), 1, "aborted import rolled back");

			oak_project_release(project);
		}
	}

	#[test]
	fn param_write_is_undoable() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let clip = oak_block_create_clip(project);
			let (i, il) = arg("media_in_in");

			let mut value = oak_node::value::OakNodeValue::none();
			value.kind = oak_node::value::oak::RATIONAL;
			value.num = 7;
			value.den = 1;
			assert!(oak_node_set_standard_value(project, clip, i, il, -1, value));

			let mut out = oak_node::value::OakNodeValue::none();
			assert!(oak_node_standard_value(project, clip, i, il, -1, &mut out));
			assert_eq!((out.num, out.den), (7, 1));

			assert!(oak_undo_undo());
			assert!(oak_node_standard_value(project, clip, i, il, -1, &mut out));
			assert_eq!((out.num, out.den), (0, 1), "undo restored the old value");
			assert!(oak_undo_redo());
			assert!(oak_node_standard_value(project, clip, i, il, -1, &mut out));
			assert_eq!((out.num, out.den), (7, 1));

			// Unknown input id fails cleanly.
			let (i, il) = arg("no_such_input");
			assert!(!oak_node_set_standard_value(project, clip, i, il, -1, value));

			oak_project_release(project);
		}
	}

	#[test]
	fn edit_null_safety() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			assert_eq!(oak_footage_import(project, std::ptr::null(), 0), OAK_NODE_ID_INVALID);
			assert_eq!(
				oak_footage_import(project, "/nonexistent/x.mp4".as_ptr(), 17),
				OAK_NODE_ID_INVALID
			);
			assert_eq!(oak_folder_create(std::ptr::null(), 1, "f".as_ptr(), 1), OAK_NODE_ID_INVALID);
			assert_eq!(oak_sequence_create(std::ptr::null(), "s".as_ptr(), 1), OAK_NODE_ID_INVALID);
			assert!(!oak_node_remove(project, OAK_NODE_ID_INVALID));
			// A well-formed token naming no live node is a no-op command:
			// either outcome is acceptable, it just must not crash.
			let _ = oak_node_remove(project, 1);
			assert!(!oak_track_append_block(project, OAK_NODE_ID_INVALID, 2));
			assert!(!oak_track_ripple_remove_block(project, OAK_NODE_ID_INVALID, 2));
			assert!(!oak_block_set_length(project, OAK_NODE_ID_INVALID, OakRational { num: 1, den: 1 }));
			assert!(!oak_node_connect(project, 1, OAK_NODE_ID_INVALID, "x".as_ptr(), 1, -1));
			// On a non-empty stack this undoes a real entry; redo it
			// immediately — a redoable tail would be dropped by the next
			// push, shifting the global count out from under the other
			// tests (UNDO_SERIAL serializes the stack, not the tail).
			if oak_undo_undo() {
				assert!(oak_undo_redo());
			}
			oak_project_release(project);
		}
	}
}

// ---------------------------------------------------------------------------
// Batch 3: project persistence and settings
// ---------------------------------------------------------------------------

/// Saves the project as XML to `path` (null/empty = the project's own
/// filename; the path becomes the project's filename, save-as
/// semantics). False when no path is available or on IO/serialize
/// error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_save(
	project: *const ProjectHandle,
	path: *const u8,
	path_len: usize,
) -> bool {
	if project.is_null() {
		return false;
	}
	let path = if path.is_null() || path_len == 0 {
		None
	} else {
		match unsafe { str_arg(path, path_len) } {
			Some(p) => Some(p.to_string()),
			None => return false,
		}
	};
	let mut guard = unsafe { &*project }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let target = match path {
		Some(p) => p,
		None => {
			if guard.filename.is_empty() {
				return false;
			}
			guard.filename.clone()
		}
	};
	let xml = match oak_node::serializer::save(&guard) {
		Ok(xml) => xml,
		Err(_) => return false,
	};
	if std::fs::write(&target, xml).is_err() {
		return false;
	}
	guard.filename = target;
	guard.modified = false;
	true
}

/// Loads a project from an XML file. Null on failure. The project is
/// NOT made active automatically.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_load(
	path: *const u8,
	path_len: usize,
) -> *mut ProjectHandle {
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return std::ptr::null_mut();
	};
	let xml = match std::fs::read_to_string(path) {
		Ok(xml) => xml,
		Err(_) => return std::ptr::null_mut(),
	};
	let project = match oak_node::serializer::load(&xml) {
		Ok(p) => p,
		Err(_) => return std::ptr::null_mut(),
	};
	project
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.filename = path.to_string();
	crate::handle::into_ffi(ProjectHandle { inner: project }) as *mut ProjectHandle
}

/// Sets the project file path (used by save when no path is given).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_filename(
	project: *const ProjectHandle,
	path: *const u8,
	path_len: usize,
) -> bool {
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return false;
	};
	with_project_mut(project, |guard| {
		guard.filename = path.to_string();
	})
	.is_some()
}

/// Sets the working color space: persisted into the project settings
/// AND applied to the process-wide color pipeline.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_working_color_space(
	project: *const ProjectHandle,
	setting: *const u8,
	setting_len: usize,
) -> bool {
	let Some(setting) = (unsafe { str_arg(setting, setting_len) }) else {
		return false;
	};
	let space = oak_core::colormath::WorkingColorSpace::from_setting(setting);
	if with_project_mut(project, |guard| guard.set_working_color_space(space)).is_none() {
		return false;
	}
	let spec = oak_core::color::pipeline_output_spec();
	oak_core::color::set_pipeline_color_settings(space, spec);
	true
}

/// Sets the output gamut/transfer (persisted into the project settings
/// AND applied to the process-wide color pipeline).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_output_color_spec(
	project: *const ProjectHandle,
	gamut: *const u8,
	gamut_len: usize,
	transfer: *const u8,
	transfer_len: usize,
) -> bool {
	let (Some(gamut), Some(transfer)) = (
		unsafe { str_arg(gamut, gamut_len) },
		unsafe { str_arg(transfer, transfer_len) },
	) else {
		return false;
	};
	let spec = oak_core::colormath::OutputColorSpec::from_settings(gamut, transfer);
	if with_project_mut(project, |guard| guard.set_output_color_spec(spec)).is_none() {
		return false;
	}
	let working = oak_core::color::pipeline_working_space();
	oak_core::color::set_pipeline_color_settings(working, spec);
	true
}

/// A deep copy of the project (background-render isolation). Null on
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_deep_copy(
	project: *const ProjectHandle,
) -> *mut ProjectHandle {
	if project.is_null() {
		return std::ptr::null_mut();
	}
	let copy = {
		let guard = unsafe { &*project }
			.inner
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		guard.deep_copy()
	};
	match copy {
		Ok(inner) => crate::handle::into_ffi(ProjectHandle { inner }) as *mut ProjectHandle,
		Err(_) => std::ptr::null_mut(),
	}
}

/// The project's stable library uuid, snprintf-style.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_uuid(
	project: *const ProjectHandle,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	write_str(
		with_project(project, |p| p.uuid.clone()),
		|value, buf, len| copy_str_out(&value, buf, len),
		buf,
		buf_len,
	)
}

/// Reads an arbitrary project setting (including cache/OCIO metadata).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_setting(
	project: *const ProjectHandle,
	key: *const u8,
	key_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(key) = (unsafe { str_arg(key, key_len) }) else {
		return -1;
	};
	write_str(
		with_project(project, |p| p.settings.get(key).cloned()).flatten(),
		|value, buf, len| copy_str_out(value, buf, len),
		buf,
		buf_len,
	)
}

/// Writes an arbitrary project setting and marks the project modified.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_setting(
	project: *const ProjectHandle,
	key: *const u8,
	key_len: usize,
	value: *const u8,
	value_len: usize,
) -> bool {
	let (Some(key), Some(value)) = (unsafe { str_arg(key, key_len) }, unsafe {
		str_arg(value, value_len)
	}) else {
		return false;
	};
	if key.is_empty() {
		return false;
	}
	with_project_mut(project, |guard| {
		guard.settings.insert(key.to_string(), value.to_string());
		guard.set_modified(true);
	})
	.is_some()
}

/// Reads `(cache_location_setting, custom_cache_path)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_cache_location(
	project: *const ProjectHandle,
	setting: *mut i32,
	path: *mut u8,
	path_len: usize,
) -> i32 {
	let Some((mode, custom)) = with_project(project, |p| {
		(p.cache_location_setting, p.custom_cache_path.clone())
	}) else {
		return -1;
	};
	if !setting.is_null() {
		unsafe { *setting = mode };
	}
	copy_str_out(&custom, path, path_len)
}

/// Writes the cache policy and marks the project modified.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_project_set_cache_location(
	project: *const ProjectHandle,
	setting: i32,
	path: *const u8,
	path_len: usize,
) -> bool {
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return false;
	};
	with_project_mut(project, |guard| {
		guard.cache_location_setting = setting.clamp(0, 2);
		guard.custom_cache_path = if guard.cache_location_setting == 2 {
			path.trim().to_string()
		} else {
			String::new()
		};
		guard.set_modified(true);
	})
	.is_some()
}

/// The sequence's interlaced flag (-1 for a stale id).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_sequence_interlaced(project: *const ProjectHandle, id: u64) -> i32 {
	node_id(id)
		.and_then(|id| {
			with_project(project, |p| {
				behavior_as::<SequenceBehavior>(p, id)
					.and_then(|seq| seq.video_params.first().map(|params| params.interlaced))
			})
			.flatten()
		})
		.map_or(-1, |value| value as i32)
}

/// Track presentation/behavior flags used by timeline snapshots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_height(project: *const ProjectHandle, id: u64) -> f64 {
	node_id(id)
		.and_then(|id| with_project(project, |p| behavior_as::<TrackBehavior>(p, id).map(|t| t.height)).flatten())
		.unwrap_or(0.0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_muted(project: *const ProjectHandle, id: u64) -> bool {
	node_id(id)
		.and_then(|id| with_project(project, |p| behavior_as::<TrackBehavior>(p, id).map(|t| t.muted)).flatten())
		.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_track_locked(project: *const ProjectHandle, id: u64) -> bool {
	node_id(id)
		.and_then(|id| with_project(project, |p| behavior_as::<TrackBehavior>(p, id).map(|t| t.locked)).flatten())
		.unwrap_or(false)
}

/// A block's standard enabled flag (false for stale/non-boolean values).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_block_enabled(project: *const ProjectHandle, id: u64) -> bool {
	node_id(id)
		.and_then(|id| {
			with_project(project, |p| {
				p.graph.get(id).and_then(|entry| {
					entry.behavior.as_any().and_then(|behavior| {
						if let Some(block) = behavior.downcast_ref::<ClipBlockBehavior>() {
							Some(block.core.enabled)
						} else if let Some(block) = behavior.downcast_ref::<GapBlockBehavior>() {
							Some(block.core.enabled)
						} else if let Some(block) = behavior.downcast_ref::<TransitionBlockBehavior>() {
							Some(block.core.enabled)
						} else if let Some(block) = behavior.downcast_ref::<AdjustmentBlockBehavior>() {
							Some(block.core.enabled)
						} else {
							None
						}
					})
				})
			})
			.flatten()
		})
		.unwrap_or(false)
}

#[cfg(test)]
mod persist_tests {
	use super::*;

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	#[test]
	fn save_load_roundtrip() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let (n, nl) = arg("Persisted Folder");
			let folder = oak_folder_create(project, oak_project_root(project), n, nl);
			assert!(oak_node_id_is_valid(folder));

			let path = std::env::temp_dir()
				.join(format!("oakengine_persist_{}.ove", std::process::id()))
				.to_string_lossy()
				.into_owned();
			let (p, pl) = arg(&path);
			assert!(oak_project_save(project, p, pl));
			assert!(!oak_project_modified(project), "saved projects are clean");

			let loaded = oak_project_load(p, pl);
			assert!(!loaded.is_null(), "reload the saved project");
			let root = oak_project_root(loaded);
			assert_eq!(oak_folder_child_count(loaded, root), 1);
			assert_eq!(oak_folder_child_at(loaded, root, 0), folder,
				"NodeIds survive the round trip");

			oak_project_release(loaded);
			oak_project_release(project);
			let _ = std::fs::remove_file(&path);

			// Negative paths.
			assert!(!oak_project_save(std::ptr::null(), p, pl));
			assert!(oak_project_load("nope".as_ptr(), 4).is_null());
		}
	}

	#[test]
	fn settings_write_syncs_the_pipeline() {
		let project = oak_project_new();
		unsafe {
			let saved_working = crate::corestate::oak_core_color_get_working_space;
			let mut buf = [0u8; 64];
			let n = saved_working(buf.as_mut_ptr(), buf.len());
			let saved = std::str::from_utf8(&buf[..n as usize]).unwrap().to_string();

			let (w, wl) = arg("srgb_legacy");
			assert!(oak_project_set_working_color_space(project, w, wl));
			let n = crate::corestate::oak_core_color_get_working_space(buf.as_mut_ptr(), buf.len());
			assert_eq!(
				std::str::from_utf8(&buf[..n as usize]).unwrap(),
				"srgb_legacy",
				"the pipeline followed the project setting"
			);

			// Restore the pipeline.
			assert!(oak_project_set_working_color_space(project, saved.as_ptr(), saved.len()));
			oak_project_release(project);
		}
	}

	#[test]
	fn deep_copy_is_independent() {
		let _guard = super::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		unsafe {
			let project = oak_project_new();
			let (n, nl) = arg("CopyTest");
			let folder = oak_folder_create(project, oak_project_root(project), n, nl);
			assert!(oak_node_id_is_valid(folder));

			let copy = oak_project_deep_copy(project);
			assert!(!copy.is_null());
			assert_eq!(oak_folder_child_count(copy, oak_project_root(copy)), 1);

			// Mutating the copy leaves the original alone.
			assert!(oak_node_remove(copy, folder));
			assert_eq!(oak_folder_child_count(copy, oak_project_root(copy)), 0);
			assert_eq!(oak_folder_child_count(project, oak_project_root(project)), 1);

			oak_project_release(copy);
			oak_project_release(project);
		}
	}

	#[test]
	fn project_settings_and_snapshot_field_queries() {
		unsafe {
			let project = oak_project_new();
			let sequence = oak_sequence_create(project, "Query".as_ptr(), 5);
			let list = oak_sequence_track_list_at(project, sequence, 0);
			let track = oak_tracklist_track_at(project, list, 0);
			let sequence_id = NodeId::from_identity(sequence).unwrap();
			let track_id = NodeId::from_identity(track).unwrap();
			with_project_mut(project, |guard| {
				let sequence_behavior = guard
					.graph
					.get_mut(sequence_id)
					.and_then(|entry| entry.behavior.as_any_mut())
					.and_then(|behavior| behavior.downcast_mut::<SequenceBehavior>())
					.expect("sequence behavior");
				sequence_behavior.video_params[0].interlaced = true;
				let track_behavior = guard
					.graph
					.get_mut(track_id)
					.and_then(|entry| entry.behavior.as_any_mut())
					.and_then(|behavior| behavior.downcast_mut::<TrackBehavior>())
					.expect("track behavior");
				track_behavior.height = 77.0;
				track_behavior.muted = true;
				track_behavior.locked = true;
			});
			assert_eq!(oak_sequence_interlaced(project, sequence), 1);
			assert_eq!(oak_track_height(project, track), 77.0);
			assert!(oak_track_muted(project, track));
			assert!(oak_track_locked(project, track));

			assert!(oak_project_set_setting(project, "key".as_ptr(), 3, "value".as_ptr(), 5));
			let mut buf = [0u8; 64];
			let len = oak_project_setting(project, "key".as_ptr(), 3, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..len as usize]).unwrap(), "value");
			assert!(!oak_project_set_setting(project, "".as_ptr(), 0, "bad".as_ptr(), 3));
			assert!(oak_project_set_cache_location(project, 2, "/tmp/cache".as_ptr(), 10));
			let mut mode = -1;
			let len = oak_project_cache_location(project, &mut mode, buf.as_mut_ptr(), buf.len());
			assert_eq!(mode, 2);
			assert_eq!(std::str::from_utf8(&buf[..len as usize]).unwrap(), "/tmp/cache");

			let expected = with_project(project, |guard| guard.uuid.clone()).unwrap();
			let len = oak_project_uuid(project, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..len as usize]).unwrap(), expected);
			oak_project_release(project);
		}
	}
}

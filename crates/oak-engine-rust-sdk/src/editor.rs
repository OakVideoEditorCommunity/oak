//! Safe, high-level editor sessions and owned read snapshots.

use std::ffi::c_void;

use crate::codec::string_out;
use crate::node::{NodeId, NodeKind, Project, StreamAudioParams, StreamVideoParams};
use crate::storage::StorageError;
use crate::types::{Rational, TimeRange};

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_add_ref(session: *const c_void);
	fn oak_editor_release(session: *const c_void);
	fn oak_editor_new() -> *mut c_void;
	fn oak_editor_open(uri: *const u8, uri_len: usize, version_info: *mut i32) -> *mut c_void;
	fn oak_editor_open_library(uuid: *const u8, uuid_len: usize, error: *mut i32) -> *mut c_void;
	fn oak_editor_create_library(name: *const u8, name_len: usize, error: *mut i32) -> *mut c_void;
	fn oak_editor_project(session: *const c_void) -> *mut c_void;
	fn oak_editor_save(
		session: *const c_void,
		path: *const u8,
		path_len: usize,
		options: u32,
	) -> i32;
	fn oak_editor_bind(session: *mut c_void) -> bool;
	fn oak_editor_unbind(session: *mut c_void) -> bool;
	fn oak_editor_storage_bound(session: *const c_void) -> bool;
	fn oak_editor_storage_last_error(session: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_editor_set_current_sequence(session: *mut c_void, id: u64) -> bool;
	fn oak_editor_current_sequence(session: *const c_void) -> u64;
	fn oak_editor_create_sequence(session: *mut c_void, name: *const u8, name_len: usize) -> u64;
	fn oak_editor_marker_count(session: *const c_void) -> i32;
	fn oak_editor_marker_at(session: *const c_void, index: i32, out: *mut EditorMarkerRaw) -> bool;
	fn oak_editor_marker_name_at(
		session: *const c_void,
		index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_editor_workarea(session: *const c_void, out: *mut TimeRange) -> bool;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct EditorMarkerRaw {
	time: TimeRange,
	color: i32,
}

/// One live editor session. Drop releases the session and unbinds its
/// project from write-through storage. `project()` returns an
/// independently refcounted handle to the same live project.
pub struct EditorSession {
	ptr: *mut c_void,
}

unsafe impl Send for EditorSession {}
unsafe impl Sync for EditorSession {}

impl EditorSession {
	fn from_raw(ptr: *mut c_void) -> Result<Self, StorageError> {
		if ptr.is_null() {
			Err(StorageError(-1))
		} else {
			Ok(Self { ptr })
		}
	}

	/// Creates a new project and binds it when a default library is configured.
	pub fn new() -> Result<Self, StorageError> {
		let session = Self::from_raw(unsafe { oak_editor_new() })?;
		let _ = session.bind();
		Ok(session)
	}

	/// Opens any storage URI and binds the loaded project.
	pub fn open(uri: &str) -> Result<Self, StorageError> {
		let mut info = 0;
		let ptr = unsafe { oak_editor_open(uri.as_ptr(), uri.len(), &mut info) };
		if ptr.is_null() {
			return Err(StorageError(info));
		}
		let session = Self { ptr };
		let _ = session.bind();
		Ok(session)
	}

	/// Opens a row in the configured default library.
	pub fn open_library(uuid: &str) -> Result<Self, StorageError> {
		let mut error = 0;
		let ptr = unsafe { oak_editor_open_library(uuid.as_ptr(), uuid.len(), &mut error) };
		if ptr.is_null() {
			return Err(StorageError(error));
		}
		let session = Self { ptr };
		let _ = session.bind();
		Ok(session)
	}

	/// Creates a blank row in the default library without reloading it.
	pub fn create_in_library(name: &str) -> Result<Self, StorageError> {
		let mut error = 0;
		let ptr = unsafe { oak_editor_create_library(name.as_ptr(), name.len(), &mut error) };
		if ptr.is_null() {
			return Err(StorageError(error));
		}
		let session = Self { ptr };
		let _ = session.bind();
		Ok(session)
	}

	/// A new safe handle to the same live project used by storage binding.
	pub fn project(&self) -> Project {
		Project::from_raw(unsafe { oak_editor_project(self.ptr) })
	}

	/// Saves through the storage backend selected by the path extension.
	pub fn save(&self, path: &str) -> Result<(), StorageError> {
		let code = unsafe { oak_editor_save(self.ptr, path.as_ptr(), path.len(), 0) };
		if code == 0 {
			Ok(())
		} else {
			Err(StorageError(code))
		}
	}

	/// Attempts the default-library binding. False means unconfigured,
	/// which is a supported headless mode rather than a fatal error.
	pub fn bind(&self) -> bool {
		unsafe { oak_editor_bind(self.ptr) }
	}

	/// Explicitly flushes and unbinds before the session is dropped.
	/// This is required before deleting its library row. Drop is the
	/// same operation as a fallback, and repeated closes are harmless.
	pub fn close(&self) {
		unsafe { oak_editor_unbind(self.ptr) };
	}

	pub fn storage_bound(&self) -> bool {
		unsafe { oak_editor_storage_bound(self.ptr) }
	}

	pub fn storage_last_error(&self) -> Option<String> {
		string_out(|buf, len| unsafe { oak_editor_storage_last_error(self.ptr, buf, len) })
	}

	pub fn current_sequence(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_editor_current_sequence(self.ptr) });
		id.is_valid().then_some(id)
	}

	pub fn set_current_sequence(&self, sequence: NodeId) -> bool {
		unsafe { oak_editor_set_current_sequence(self.ptr, sequence.raw()) }
	}

	/// Creates a sequence with the engine's default track lists and selects it.
	pub fn create_sequence(&self, name: &str) -> Result<NodeId, StorageError> {
		let id = NodeId::from_raw(unsafe {
			oak_editor_create_sequence(self.ptr, name.as_ptr(), name.len())
		});
		if id.is_valid() {
			Ok(id)
		} else {
			Err(StorageError(-1))
		}
	}

	/// Builds an owned snapshot. The revision is the global undo row count
	/// at snapshot start and lets callers detect later edits.
	pub fn snapshot(&self) -> ProjectSnapshot {
		let revision = crate::node::undo_count();
		let project = self.project();
		let root = entry_snapshot(&project, project.root());
		let mut sequences = Vec::new();
		let mut footages = Vec::new();
		collect_media(&project, project.root(), &mut sequences, &mut footages);
		let current_sequence = self.current_sequence();
		let markers = self.markers();
		let workarea = self.workarea();
		ProjectSnapshot {
			revision,
			name: project.name(),
			filename: project.filename(),
			uuid: project.uuid(),
			modified: project.modified(),
			root,
			sequences,
			footages,
			current_sequence,
			markers,
			workarea,
		}
	}

	fn markers(&self) -> Vec<MarkerSnapshot> {
		let count = unsafe { oak_editor_marker_count(self.ptr) }.max(0);
		(0..count)
			.filter_map(|index| {
				let mut raw = EditorMarkerRaw::default();
				if !unsafe { oak_editor_marker_at(self.ptr, index, &mut raw) } {
					return None;
				}
				Some(MarkerSnapshot {
					name: string_out(|buf, len| unsafe {
						oak_editor_marker_name_at(self.ptr, index, buf, len)
					})
					.unwrap_or_default(),
					time: raw.time,
					color: raw.color,
				})
			})
			.collect()
	}

	fn workarea(&self) -> Option<WorkAreaSnapshot> {
		let mut range = TimeRange::default();
		unsafe { oak_editor_workarea(self.ptr, &mut range) }.then_some(WorkAreaSnapshot { range })
	}
}

impl Clone for EditorSession {
	fn clone(&self) -> Self {
		unsafe { oak_editor_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for EditorSession {
	fn drop(&mut self) {
		unsafe { oak_editor_release(self.ptr) };
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
	Video,
	Audio,
	Subtitle,
}

fn track_kind(raw: i32) -> Option<TrackKind> {
	match raw {
		0 => Some(TrackKind::Video),
		1 => Some(TrackKind::Audio),
		2 => Some(TrackKind::Subtitle),
		_ => None,
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntrySnapshot {
	pub id: NodeId,
	pub kind: Option<NodeKind>,
	pub name: String,
	pub children: Vec<EntrySnapshot>,
}

fn entry_snapshot(project: &Project, id: NodeId) -> EntrySnapshot {
	let children = if project.node_kind(id) == Some(NodeKind::Folder) {
		(0..project.folder_child_count(id))
			.map(|index| entry_snapshot(project, project.folder_child_at(id, index)))
			.collect()
	} else {
		Vec::new()
	};
	EntrySnapshot {
		id,
		kind: project.node_kind(id),
		name: project.node_label(id),
		children,
	}
}

fn collect_media(
	project: &Project,
	folder: NodeId,
	sequences: &mut Vec<SequenceSnapshot>,
	footages: &mut Vec<FootageSnapshot>,
) {
	for index in 0..project.folder_child_count(folder) {
		let id = project.folder_child_at(folder, index);
		match project.node_kind(id) {
			Some(NodeKind::Folder) => collect_media(project, id, sequences, footages),
			Some(NodeKind::Sequence) => sequences.push(sequence_snapshot(project, id)),
			Some(NodeKind::Footage) => footages.push(FootageSnapshot {
				id,
				name: project.node_label(id),
				filename: project.footage_filename(id),
				duration: project.footage_duration(id),
				video_streams: project.footage_video_stream_count(id),
				audio_streams: project.footage_audio_stream_count(id),
				subtitle_streams: project.footage_subtitle_stream_count(id),
				video: project.footage_video_params(id, 0),
				audio: project.footage_audio_params(id, 0),
			}),
			_ => {}
		}
	}
}

fn sequence_snapshot(project: &Project, id: NodeId) -> SequenceSnapshot {
	let (width, height) = project.sequence_geometry(id).unwrap_or((0, 0));
	let mut tracks = Vec::new();
	for list_index in 0..project.sequence_track_list_count(id) {
		let list = project.sequence_track_list_at(id, list_index);
		for track_index in 0..project.tracklist_track_count(list) {
			let track = project.tracklist_track_at(list, track_index);
			let clips = (0..project.track_block_count(track))
				.map(|block_index| {
					let block = project.track_block_at(track, block_index);
					ClipSnapshot {
						id: block,
						kind: project.node_kind(block),
						name: project.node_label(block),
						in_time: project.block_in(block),
						out_time: project.block_out(block),
						media_in: project.block_media_in(block),
						media_out: project.block_media_out(block),
						enabled: project.block_enabled(block),
					}
				})
				.collect();
			tracks.push(TrackSnapshot {
				id: track,
				kind: track_kind(project.track_kind(track)),
				height: project.track_height(track),
				muted: project.track_muted(track),
				locked: project.track_locked(track),
				clips,
			});
		}
	}
	SequenceSnapshot {
		id,
		name: project.node_label(id),
		width,
		height,
		frame_rate: project.sequence_frame_rate(id),
		interlaced: project.sequence_interlaced(id).unwrap_or(false),
		length: project.sequence_length(id),
		playhead: project.sequence_playhead(id),
		tracks,
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct SequenceSnapshot {
	pub id: NodeId,
	pub name: String,
	pub width: i32,
	pub height: i32,
	pub frame_rate: Rational,
	pub interlaced: bool,
	pub length: Rational,
	pub playhead: Rational,
	pub tracks: Vec<TrackSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackSnapshot {
	pub id: NodeId,
	pub kind: Option<TrackKind>,
	pub height: f64,
	pub muted: bool,
	pub locked: bool,
	pub clips: Vec<ClipSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipSnapshot {
	pub id: NodeId,
	pub kind: Option<NodeKind>,
	pub name: String,
	pub in_time: Rational,
	pub out_time: Rational,
	pub media_in: Rational,
	pub media_out: Rational,
	pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FootageSnapshot {
	pub id: NodeId,
	pub name: String,
	pub filename: Option<String>,
	pub duration: Rational,
	pub video_streams: usize,
	pub audio_streams: usize,
	pub subtitle_streams: usize,
	pub video: Option<StreamVideoParams>,
	pub audio: Option<StreamAudioParams>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarkerSnapshot {
	pub name: String,
	pub time: TimeRange,
	pub color: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkAreaSnapshot {
	pub range: TimeRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectSnapshot {
	pub revision: i64,
	pub name: String,
	pub filename: String,
	pub uuid: String,
	pub modified: bool,
	pub root: EntrySnapshot,
	pub sequences: Vec<SequenceSnapshot>,
	pub footages: Vec<FootageSnapshot>,
	pub current_sequence: Option<NodeId>,
	pub markers: Vec<MarkerSnapshot>,
	pub workarea: Option<WorkAreaSnapshot>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::state::Config;

	#[test]
	fn file_round_trip_preserves_the_owned_snapshot() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("new session");
		let folder = session
			.project()
			.create_folder(session.project().root(), "Nested Bin")
			.expect("nested folder");
		let media = crate::test_support::make_test_clip("snapshot");
		let footage = session.project().import_footage(&media).expect("footage");
		let sequence = session.create_sequence("SDK Sequence").expect("sequence");
		let project = session.project();
		let list = project.sequence_track_list_at(sequence, 0);
		let track = project.tracklist_track_at(list, 0);
		let clip = project.create_clip().expect("clip");
		assert!(project.track_append_block(track, clip));
		assert!(project.block_set_length(clip, Rational::new(2, 1)));

		let enabled = session.snapshot();
		assert!(enabled.sequences[0].tracks[0].clips[0].enabled);
		assert!(crate::timeline::block_set_enabled(&project, clip, false));
		let disabled = session.snapshot();
		assert!(!disabled.sequences[0].tracks[0].clips[0].enabled);
		assert!(disabled.revision > enabled.revision);
		assert!(crate::node::undo());
		assert!(session.snapshot().sequences[0].tracks[0].clips[0].enabled);
		assert!(crate::node::redo());
		assert!(!session.snapshot().sequences[0].tracks[0].clips[0].enabled);

		let before = session.snapshot();
		assert_eq!(before.current_sequence, Some(sequence));
		assert_eq!(before.sequences.len(), 1);
		assert_eq!(before.sequences[0].tracks[0].clips.len(), 1);
		assert_eq!(before.footages.len(), 1);
		assert_eq!(before.footages[0].id, footage);
		assert_eq!(before.footages[0].filename.as_deref(), Some(media.as_str()));
		assert!(before.footages[0].video_streams >= 1);
		assert_eq!(before.footages[0].video.map(|p| p.width), Some(64));
		assert!(before.footages[0].duration > Rational::new(0, 1));
		assert!(before.root.children.iter().any(|entry| entry.id == folder));
		assert!(before.markers.is_empty());
		assert!(before.workarea.is_none());

		let path = std::env::temp_dir()
			.join(format!("oaksdk_editor_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		session.save(&path).expect("save");
		let opened = EditorSession::open(&path).expect("open");
		let after = opened.snapshot();
		assert_eq!(after.filename, path);
		assert!(!after.modified);
		assert_eq!(after.current_sequence, Some(sequence));
		assert_eq!(after.sequences, before.sequences);
		assert_eq!(after.footages, before.footages);
		assert_eq!(after.root.children, before.root.children);
		assert_eq!(after.root.kind, Some(NodeKind::Folder));
		assert!(after.revision >= before.revision);
		let _ = std::fs::remove_file(path);
		let _ = std::fs::remove_file(media);
	}

	#[test]
	fn project_settings_and_cache_round_trip() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("new session");
		let project = session.project();
		assert_eq!(project.setting("missing"), None);
		assert!(!project.set_setting("", "rejected"));
		assert!(project.set_setting("ocioconfig", "/tmp/test.ocio"));
		assert_eq!(
			project.setting("ocioconfig").as_deref(),
			Some("/tmp/test.ocio")
		);
		assert!(project.set_cache_location(2, "/tmp/oak-cache"));
		assert_eq!(project.cache_location(), (2, "/tmp/oak-cache".into()));
		assert!(project.set_cache_location(0, "/ignored"));
		assert_eq!(project.cache_location(), (0, String::new()));
		assert!(project.set_cache_location(-10, "/ignored"));
		assert_eq!(project.cache_location().0, 0);
		assert!(project.set_cache_location(99, "  /tmp/custom  "));
		assert_eq!(project.cache_location(), (2, "/tmp/custom".into()));
	}

	#[test]
	fn invalid_inputs_and_shared_close_semantics() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		Config::global()
			.set("Storage/Backend", "off")
			.expect("disable library");
		assert!(EditorSession::open("not-a-storage-uri").is_err());
		assert!(EditorSession::open_library("missing").is_err());

		let session = EditorSession::new().expect("new session");
		let first = session.create_sequence("First").expect("first");
		let second = session.create_sequence("Second").expect("second");
		assert_eq!(session.current_sequence(), Some(second));
		assert!(session.set_current_sequence(first));
		assert!(!session.set_current_sequence(NodeId::INVALID));
		assert_eq!(session.current_sequence(), Some(first));
		assert_eq!(session.snapshot().sequences.len(), 2);

		let cloned = session.clone();
		cloned.close();
		assert!(!cloned.storage_bound());
		assert!(!session.storage_bound(), "clones share one session");
		let project = session.project();
		drop(cloned);
		drop(session);
		assert!(
			project.root().is_valid(),
			"the project handle owns its own reference"
		);
	}
}

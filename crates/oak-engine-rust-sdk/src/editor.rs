//! Safe, high-level editor sessions and owned read snapshots.

use std::ffi::c_void;

use crate::codec::string_out;
use crate::node::{NodeId, Project};
use crate::storage::StorageError;
use crate::types::TimeRange;

/// Owned C-compatible storage project handle used only for same-instance
/// migration. Layout matches `oak_core::handle::CHandle` exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StorageProjectHandle {
	/// Opaque refcounted project box.
	pub ctx: *mut c_void,
	/// Atomic increment supplied by the storage backend.
	pub addref: Option<unsafe extern "C" fn(*mut c_void)>,
	/// Atomic decrement supplied by the storage backend.
	pub release: Option<unsafe extern "C" fn(*mut c_void)>,
	/// Storage handle ABI version.
	pub abi_version: u32,
}

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
	fn oak_editor_open_project_from_library(uuid: *const u8, uuid_len: usize, error: *mut i32) -> *mut c_void;
	fn oak_editor_adopt_storage_project_handle(project: StorageProjectHandle) -> *mut c_void;
	fn oak_editor_from_project_handle(project: *mut c_void) -> *mut c_void;
	fn oak_editor_create_library(name: *const u8, name_len: usize, error: *mut i32) -> *mut c_void;
	fn oak_editor_project(session: *const c_void) -> *mut c_void;
	fn oak_editor_save(
		session: *const c_void,
		path: *const u8,
		path_len: usize,
		options: u32,
	) -> i32;
	fn oak_editor_bind_project_to_library(session: *mut c_void) -> bool;
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
	pub(crate) ptr: *mut c_void,
}

// The engine serializes session/project mutation behind mutexes. The opaque
// handle itself is refcounted and may be shared; raw pointer dereferences stay
// inside the engine ABI rather than in SDK client threads.
unsafe impl Send for EditorSession {}
unsafe impl Sync for EditorSession {}

impl EditorSession {
	/// Creates a fresh session around an existing project handle without
	/// reopening or copying the document. The handle is consumed exactly once.
	pub fn from_project(project: Project) -> Result<Self, StorageError> {
		Self::from_raw(unsafe { oak_editor_from_project_handle(project.into_raw()) })
	}

	/// Adopts an owned storage handle into a session without reopening the project.
	///
	/// This is intended for migration from an existing shared project owner. The
	/// handle is consumed, and the session and original `ProjectRef` point to the
	/// same Arc; it is not a second document or a raw-Arc reconstruction.
	pub fn adopt_storage_project_handle(
		handle: StorageProjectHandle,
	) -> Result<Self, StorageError> {
		Self::from_raw(unsafe { oak_editor_adopt_storage_project_handle(handle) })
	}

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
		let _ = session.bind_project_to_library();
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
		let _ = session.bind_project_to_library();
		Ok(session)
	}

	/// Opens the project identified by `uuid` in the configured default library.
	pub fn open_project_from_library(uuid: &str) -> Result<Self, StorageError> {
		let mut error = 0;
		let ptr = unsafe {
			oak_editor_open_project_from_library(uuid.as_ptr(), uuid.len(), &mut error)
		};
		if ptr.is_null() {
			return Err(StorageError(error));
		}
		let session = Self { ptr };
		let _ = session.bind_project_to_library();
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
		let _ = session.bind_project_to_library();
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

	/// Registers this exact project for write-through persistence in the default
	/// library. Later successful undo-path edits can be written back automatically.
	/// Binding does not itself save or create a library row.
	///
	/// False commonly means that no library is configured, which is a supported
	/// headless/non-persistent mode rather than a fatal error.
	pub fn bind_project_to_library(&self) -> bool {
		unsafe { oak_editor_bind_project_to_library(self.ptr) }
	}

	/// Explicitly flushes pending writes and removes the library binding.
	/// Automatic persistence stops, but the session and in-memory project remain
	/// usable. This is required before deleting its library row. Drop performs
	/// the same operation as a fallback, and repeated closes are harmless.
	pub fn close(&self) {
		unsafe { oak_editor_unbind(self.ptr) };
	}

	/// Queries whether the project currently has an active write-through binding.
	/// This neither binds nor saves the project and does not report write errors.
	pub fn storage_bound(&self) -> bool {
		unsafe { oak_editor_storage_bound(self.ptr) }
	}

	/// Returns the most recent write-through error, if the backend supplied one.
	pub fn storage_last_error(&self) -> Option<String> {
		string_out(|buf, len| unsafe { oak_editor_storage_last_error(self.ptr, buf, len) })
	}

	/// Returns the sequence targeted by timeline commands without an explicit ID.
	pub fn current_sequence(&self) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe { oak_editor_current_sequence(self.ptr) });
		id.is_valid().then_some(id)
	}

	/// Selects the target sequence; false means the ID is not a session sequence.
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

	/// The session's markers (owned DTOs).
	pub fn markers(&self) -> Vec<MarkerSnapshot> {
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
		let (enabled, range) = self.workarea_state();
		enabled.then_some(WorkAreaSnapshot { range })
	}

	/// The session's work area: the enabled flag and its range (the range
	/// is meaningful even while disabled).
	pub fn workarea_state(&self) -> (bool, TimeRange) {
		let mut range = TimeRange::default();
		let enabled = unsafe { oak_editor_workarea(self.ptr, &mut range) };
		(enabled, range)
	}
}

impl Clone for EditorSession {
	/// Clones the handle, not the project; both values refer to the same session.
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

mod snapshot;
mod graph;
mod params;
mod notify;
mod workflow;
mod media;
pub mod proxy;
pub mod sync;
pub mod multicam;
pub use snapshot::*;
pub use graph::*;
pub use params::*;
pub use notify::*;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::node::NodeKind;
	use crate::state::Config;
	use crate::types::Rational;

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
		assert!(EditorSession::open_project_from_library("missing").is_err());

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

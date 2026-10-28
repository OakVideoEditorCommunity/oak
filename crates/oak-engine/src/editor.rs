//! High-level editor sessions: one live project, its write-through
//! binding, current sequence and sequence-local auxiliary state.
//!
//! The C ABI exposes only opaque session/project handles and POD data.
//! In particular, consumers never recover the `Arc<Mutex<Project>>` that
//! the engine and write-through backend share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
pub mod aux;
pub mod notify;
pub mod proxy;
pub mod sync;
pub mod multicam;

use oak_core::handle::CHandle;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_storage::nodeutil;
use oak_timeline::marker::TimelineMarkerList;
use oak_timeline::workarea::TimelineWorkArea;

use crate::codec::{copy_str_out, str_arg};
use crate::node::{OAK_NODE_ID_INVALID, ProjectHandle};

/// Mutable runtime context owned by one [`EditorSessionHandle`].
///
/// A session wraps one live project together with the editing state that should
/// not be stored inside the project graph: the selected sequence, sequence-local
/// auxiliary objects, clipboard and write-through binding. Commands receive this
/// structure so they all operate on the same project and session context.
///
/// This type is engine-internal, not a C ABI value and not a serializable project
/// format. The handle's outer mutex serializes session access; fields that can
/// also be shared independently have their own `Arc<Mutex<_>>` ownership.
pub(crate) struct SessionState {
	/// The sole live project shared with project handles and write-through storage.
	pub(crate) project: Arc<Mutex<Project>>,
	/// Sequence used by commands that do not take an explicit sequence ID.
	pub(crate) current_sequence: NodeId,
	// Arc-backed marker/work-area objects can be shared with CHandle-compatible
	// commands without creating short-lived native handles.
	pub(crate) markers: Arc<Mutex<TimelineMarkerList>>,
	pub(crate) workarea: Arc<Mutex<TimelineWorkArea>>,
	/// Owned clips captured by the most recent copy operation.
	pub(crate) clipboard: Vec<crate::editor_commands::ClipboardClip>,
	/// Handle used to flush and detach write-through storage on unbind/session drop.
	pub(crate) storage: Option<CHandle>,
	/// Session-local edit callbacks cloned before notification.
	pub(crate) observers: Vec<Arc<notify::SessionObserver>>,
	/// Next nonzero ID returned by the edit subscription API.
	pub(crate) next_observer_id: u64,
	/// Active proxy tasks keyed by footage identity. Values are engine task IDs;
	/// keeping them in session state prevents duplicate generation and lets
	/// status queries distinguish a live worker from a stale working file.
	pub(crate) proxy_tasks: HashMap<u64, u64>,
}

impl SessionState {
	/// Creates auxiliary state for a project without rebinding or copying it.
	fn new(project: Arc<Mutex<Project>>) -> Self {
		Self {
			project,
			current_sequence: NodeId::INVALID,
			markers: Arc::new(Mutex::new(TimelineMarkerList::new())),
			workarea: Arc::new(Mutex::new(TimelineWorkArea::new())),
			clipboard: Vec::new(),
			storage: None,
			observers: Vec::new(),
			next_observer_id: 1,
			proxy_tasks: HashMap::new(),
		}
	}

	/// Takes the handle first so cleanup is idempotent even if called explicitly
	/// before `EditorSessionHandle::drop`.
	fn unbind(&mut self) {
		if let Some(handle) = self.storage.take() {
			oak_storage::writethrough::unbind_project(handle);
			nodeutil::release_project(handle);
		}
	}
}

/// An opaque editor session. The mutex serializes session mutations;
/// project access additionally uses the project's own lock.
pub struct EditorSessionHandle {
	pub(crate) inner: Mutex<SessionState>,
}

impl EditorSessionHandle {
	fn project(&self) -> Arc<Mutex<Project>> {
		self.inner
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.project
			.clone()
	}
}

impl Drop for EditorSessionHandle {
	fn drop(&mut self) {
		self.inner
			.get_mut()
			.unwrap_or_else(|e| e.into_inner())
			.unbind();
	}
}

crate::export_handle!(EditorSessionHandle, oak_editor_add_ref, oak_editor_release);

/// Builds a new opaque session directly from a shared project Arc.
///
/// This is the low-level constructor: it creates fresh `SessionState` and turns
/// it into an FFI handle without copying the project or selecting a sequence.
fn session_from_project_arc(project: Arc<Mutex<Project>>) -> *mut EditorSessionHandle {
	crate::handle::into_ffi(EditorSessionHandle {
		inner: Mutex::new(SessionState::new(project)),
	}) as *mut EditorSessionHandle
}

/// Adopts a project handle into a new editor session.
///
/// The session retains the handle's underlying project Arc, then releases the
/// supplied handle reference. After construction through
/// [`session_from_project_arc`], the first sequence is selected as the default
/// editing target. A null project handle produces a null session handle.
fn session_from_project_handle(project: *mut ProjectHandle) -> *mut EditorSessionHandle {
	if project.is_null() {
		return std::ptr::null_mut();
	}
	let inner = unsafe { &*project }.project().clone();
	unsafe { crate::node::oak_project_release(project) };
	let session = session_from_project_arc(inner);
	if session.is_null() {
		return session;
	}
	// The first sequence is a useful default, matching the app adopt path.
	let first = {
		let guard = unsafe { &*session }
			.inner
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let project = guard.project.lock().unwrap_or_else(|e| e.into_inner());
		project.graph.node_ids().into_iter().find(|&id| {
			project
				.graph
				.get(id)
				.and_then(|entry| entry.behavior.as_any())
				.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
				.is_some()
		})
	};
	if let Some(id) = first {
		unsafe { oak_editor_set_current_sequence(session, id.identity()) };
	}
	session
}

/// Creates an uninitialized-session around a new, initialized project.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_new() -> *mut EditorSessionHandle {
	let project = Project::new();
	if project
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.initialize()
		.is_err()
	{
		return std::ptr::null_mut();
	}
	session_from_project_arc(project)
}

/// Opens any URI supported by the storage registry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_open(
	uri: *const u8,
	uri_len: usize,
	version_info: *mut i32,
) -> *mut EditorSessionHandle {
	let project = unsafe { crate::storage::oak_storage_open(uri, uri_len, version_info) };
	let session = session_from_project_handle(project);
	if !session.is_null() {
		if let Some(path) = unsafe { str_arg(uri, uri_len) } {
			let project = unsafe { &*session }.project();
			let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
			guard.set_filename(path);
			guard.set_modified(false);
		}
	}
	session
}

/// Opens the project identified by `uuid` from the configured default library.
///
/// The library is the project's catalog and storage backend; this function does
/// not open a library as an asset inside another project. On success it returns
/// an editor session for the loaded project and reports `OAKSTORAGE_OK` through
/// `error` when that pointer is non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_open_project_from_library(
	uuid: *const u8,
	uuid_len: usize,
	error: *mut i32,
) -> *mut EditorSessionHandle {
	if !error.is_null() {
		unsafe { *error = oak_storage::error::OAKSTORAGE_E_INVALID };
	}
	let Some(uuid) = (unsafe { str_arg(uuid, uuid_len) }) else {
		return std::ptr::null_mut();
	};
	if uuid.is_empty() {
		return std::ptr::null_mut();
	}
	let uri = match crate::storage::project_uri(uuid) {
		Ok(uri) => uri,
		Err(code) => {
			if !error.is_null() {
				unsafe { *error = code };
			}
			return std::ptr::null_mut();
		}
	};
	let result = match oak_storage::backend::StorageBackend::load(
		oak_storage::writethrough::backend(),
		&uri,
	) {
		Ok(result) => result,
		Err(e) => {
			if !error.is_null() {
				unsafe { *error = e.code() };
			}
			return std::ptr::null_mut();
		}
	};
	if result.project.is_null() {
		return std::ptr::null_mut();
	}
	let project = unsafe { nodeutil::project_arc(&result.project) };
	nodeutil::release_project(result.project);
	let project = match project {
		Ok(project) => project,
		Err(e) => {
			if !error.is_null() {
				unsafe { *error = e.code() };
			}
			return std::ptr::null_mut();
		}
	};
	project
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.set_modified(false);
	if !error.is_null() {
		unsafe { *error = oak_storage::error::OAKSTORAGE_OK };
	}
	session_from_project_handle(ProjectHandle::from_arc(project))
}

/// Adopts one owned storage project handle without reopening or copying it.
///
/// This is the migration path for a host that already owns a `ProjectRef`: the
/// handle's Arc is retained by the new session and the supplied handle reference
/// is released. Both sides therefore observe the same live project. Null or
/// invalid handles return null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_adopt_storage_project_handle(
	project: oak_core::handle::CHandle,
) -> *mut EditorSessionHandle {
	if project.is_null() {
		return std::ptr::null_mut();
	}
	let project = match unsafe { nodeutil::project_arc(&project) } {
		Ok(project) => project,
		Err(_) => {
			nodeutil::release_project(project);
			return std::ptr::null_mut();
		}
	};
	session_from_project_handle(ProjectHandle::from_arc(project))
}

/// Creates a blank row in the configured default library. The returned
/// session owns the same project that was saved, not a reloaded copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_create_library(
	name: *const u8,
	name_len: usize,
	error: *mut i32,
) -> *mut EditorSessionHandle {
	if !error.is_null() {
		unsafe { *error = oak_storage::error::OAKSTORAGE_E_INVALID };
	}
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return std::ptr::null_mut();
	};
	if name.trim().is_empty() {
		return std::ptr::null_mut();
	}
	let uri = match crate::storage::default_library() {
		Ok(uri) => uri,
		Err(code) => {
			if !error.is_null() {
				unsafe { *error = code };
			}
			return std::ptr::null_mut();
		}
	};
	let project = Project::new();
	{
		let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
		if guard.initialize().is_err() {
			return std::ptr::null_mut();
		}
		guard
			.settings
			.insert("projectname".into(), name.trim().into());
	}
	if let Err(e) = oak_storage::writethrough::backend().save_project(&project, &uri, 0) {
		if !error.is_null() {
			unsafe { *error = e.code() };
		}
		return std::ptr::null_mut();
	}
	if !error.is_null() {
		unsafe { *error = oak_storage::error::OAKSTORAGE_OK };
	}
	session_from_project_arc(project)
}

/// Returns a new reference on the session's live project.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_project(
	session: *const EditorSessionHandle,
) -> *mut ProjectHandle {
	if session.is_null() {
		return std::ptr::null_mut();
	}
	ProjectHandle::from_arc(unsafe { &*session }.project())
}

/// Saves through the storage registry and records the file path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_save(
	session: *const EditorSessionHandle,
	path: *const u8,
	path_len: usize,
	options: u32,
) -> i32 {
	if session.is_null() {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	}
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let project = unsafe { oak_editor_project(session) };
	let code =
		unsafe { crate::storage::oak_storage_save(project, path.as_ptr(), path.len(), options) };
	unsafe { crate::node::oak_project_release(project) };
	if code == oak_storage::error::OAKSTORAGE_OK {
		let project = unsafe { &*session }.project();
		let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
		guard.set_filename(path);
		guard.set_modified(false);
	}
	code
}

/// Binds this exact project instance to the configured default library for
/// write-through persistence.
///
/// After binding, each successful undo-path command can write the project's
/// changes back to its library row. Binding itself does not save immediately;
/// the first undo-path operation creates the row when necessary. The returned
/// storage handle is retained in `SessionState::storage` so explicit unbind and
/// session destruction can flush and release the same binding.
///
/// A false result commonly means that no library is configured or storage is
/// disabled; this is a supported non-persistent mode, not a project-open error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_bind_project_to_library(
	session: *mut EditorSessionHandle,
) -> bool {
	if session.is_null() {
		return false;
	}
	let (project, existing) = {
		let state = unsafe { &*session }
			.inner
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		(state.project.clone(), state.storage.is_some())
	};
	if existing {
		return true;
	}
	let handle = nodeutil::make_project_owned(project);
	oak_storage::writethrough::bind_project(handle);
	let bound = oak_storage::writethrough::is_bound(handle);
	let mut state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	state.storage = Some(handle);
	bound
}

/// Returns whether this session's project currently has an active
/// write-through library binding.
///
/// This is a status query only: it does not bind, save or flush the project.
/// False means a null session, no binding, or a binding disabled by storage
/// configuration. It does not report whether the last write succeeded; use
/// `oak_editor_storage_last_error` for that separate diagnostic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_storage_bound(session: *const EditorSessionHandle) -> bool {
	if session.is_null() {
		return false;
	}
	let state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	state
		.storage
		.is_some_and(|handle| oak_storage::writethrough::is_bound(handle))
}

/// Flushes pending writes and closes the write-through library binding.
///
/// Explicit unbind provides deterministic cleanup before operations such as
/// deleting the project's library row. The session and in-memory project remain
/// alive; only automatic persistence stops. Session destruction performs the
/// same cleanup as a fallback, and repeated unbind calls are no-ops.
/// Returns false only for a null session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_unbind(session: *mut EditorSessionHandle) -> bool {
	if session.is_null() {
		return false;
	}
	unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.unbind();
	true
}

/// Copies the project's most recent write-through or snapshot error as UTF-8.
///
/// Returns the bounded-string byte count when an error exists. Returns `-1`
/// for a null session, no binding, or no recorded error; these cases do not
/// distinguish success from an unavailable diagnostic by themselves.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_storage_last_error(
	session: *const EditorSessionHandle,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if session.is_null() {
		return -1;
	}
	let state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	match state
		.storage
		.and_then(oak_storage::writethrough::last_error)
	{
		Some(error) => copy_str_out(&error, buf, buf_len),
		None => -1,
	}
}

/// Selects a sequence and resets its sequence-local auxiliary state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_set_current_sequence(
	session: *mut EditorSessionHandle,
	id: u64,
) -> bool {
	if session.is_null() {
		return false;
	}
	let Some(id) = NodeId::from_identity(id) else {
		return false;
	};
	let project = unsafe { &*session }.project();
	let valid = {
		let guard = project.lock().unwrap_or_else(|e| e.into_inner());
		guard
			.graph
			.get(id)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
			.is_some()
	};
	if !valid {
		return false;
	}
	let mut state = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	if state.current_sequence != id {
		state.current_sequence = id;
		state.markers = Arc::new(Mutex::new(TimelineMarkerList::new()));
		state.workarea = Arc::new(Mutex::new(TimelineWorkArea::new()));
	}
	true
}

/// Returns the raw identity of the sequence used by context-dependent edits.
///
/// Returns `OAK_NODE_ID_INVALID` for a null session or when no sequence has
/// been selected. The returned ID is borrowed only as a value; no project
/// handle or graph reference crosses the ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_current_sequence(session: *const EditorSessionHandle) -> u64 {
	if session.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.current_sequence
		.identity()
}

/// Creates and selects a sequence through the existing node ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_create_sequence(
	session: *mut EditorSessionHandle,
	name: *const u8,
	name_len: usize,
) -> u64 {
	let project = unsafe { oak_editor_project(session) };
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	let id = unsafe { crate::node::oak_sequence_create(project, name, name_len) };
	unsafe { crate::node::oak_project_release(project) };
	if !unsafe { oak_editor_set_current_sequence(session, id) } {
		return OAK_NODE_ID_INVALID;
	}
	id
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::coretypes::OakTimeRange;

	#[test]
	fn session_owns_current_sequence_and_auxiliary_queries() {
		unsafe {
			let session = oak_editor_new();
			assert!(!session.is_null());
			let name = "Sequence";
			let sequence = oak_editor_create_sequence(session, name.as_ptr(), name.len());
			assert!(crate::node::oak_node_id_is_valid(sequence));
			assert_eq!(oak_editor_current_sequence(session), sequence);
			assert_eq!(aux::oak_editor_marker_count(session), 0);
			aux::add_marker_for_test(session, "Marker");
			assert_eq!(aux::oak_editor_marker_count(session), 1);
			let mut marker = aux::OakEditorMarker::default();
			assert!(aux::oak_editor_marker_at(session, 0, &mut marker));
			assert_eq!(marker.color, 3);
			assert_eq!(marker.time.in_.num, 1);
			assert_eq!(marker.time.out.num, 2);
			let mut name_buf = [0u8; 16];
			let name_len =
				aux::oak_editor_marker_name_at(session, 0, name_buf.as_mut_ptr(), name_buf.len());
			assert_eq!(
				std::str::from_utf8(&name_buf[..name_len as usize]).unwrap(),
				"Marker"
			);
			assert!(!aux::oak_editor_marker_at(session, 99, &mut marker));
			assert_eq!(
				aux::oak_editor_marker_name_at(session, 99, name_buf.as_mut_ptr(), 16),
				-1
			);
			let mut range = OakTimeRange::default();
			assert!(!aux::oak_editor_workarea(session, &mut range));

			let second = oak_editor_create_sequence(session, name.as_ptr(), name.len());
			assert!(crate::node::oak_node_id_is_valid(second));
			assert_eq!(oak_editor_current_sequence(session), second);
			assert_eq!(
				aux::oak_editor_marker_count(session),
				0,
				"sequence switch resets aux state"
			);
			assert!(!oak_editor_set_current_sequence(session, u64::MAX));
			assert_eq!(oak_editor_current_sequence(session), second);
			oak_editor_release(session);
		}
	}

}

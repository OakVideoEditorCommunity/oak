//! High-level editor sessions: one live project, its write-through
//! binding, current sequence and sequence-local auxiliary state.
//!
//! The C ABI exposes only opaque session/project handles and POD data.
//! In particular, consumers never recover the `Arc<Mutex<Project>>` that
//! the engine and write-through backend share.

use std::sync::{Arc, Mutex};

use oak_core::handle::CHandle;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_storage::nodeutil;
use oak_timeline::marker::TimelineMarkerList;
use oak_timeline::workarea::TimelineWorkArea;

use crate::codec::{copy_str_out, str_arg};
use crate::coretypes::OakTimeRange;
use crate::handle::NativeMirror;
use crate::node::{OAK_NODE_ID_INVALID, ProjectHandle};

struct SessionState {
	project: Arc<Mutex<Project>>,
	current_sequence: NodeId,
	markers: Arc<Mutex<TimelineMarkerList>>,
	workarea: Arc<Mutex<TimelineWorkArea>>,
	storage: Option<CHandle>,
}

impl SessionState {
	fn new(project: Arc<Mutex<Project>>) -> Self {
		Self {
			project,
			current_sequence: NodeId::INVALID,
			markers: Arc::new(Mutex::new(TimelineMarkerList::new())),
			workarea: Arc::new(Mutex::new(TimelineWorkArea::new())),
			storage: None,
		}
	}

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
	inner: Mutex<SessionState>,
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

fn into_session(project: Arc<Mutex<Project>>) -> *mut EditorSessionHandle {
	crate::handle::into_ffi(EditorSessionHandle {
		inner: Mutex::new(SessionState::new(project)),
	}) as *mut EditorSessionHandle
}

fn from_project(project: *mut ProjectHandle) -> *mut EditorSessionHandle {
	if project.is_null() {
		return std::ptr::null_mut();
	}
	let inner = unsafe { &*project }.project().clone();
	unsafe { crate::node::oak_project_release(project) };
	let session = into_session(inner);
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
	into_session(project)
}

/// Opens any URI supported by the storage registry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_open(
	uri: *const u8,
	uri_len: usize,
	version_info: *mut i32,
) -> *mut EditorSessionHandle {
	let project = unsafe { crate::storage::oak_storage_open(uri, uri_len, version_info) };
	let session = from_project(project);
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

/// Opens a row from the configured default library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_open_library(
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
	from_project(ProjectHandle::from_arc(project))
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
	into_session(project)
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

/// Binds this exact project instance to the configured default library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_bind(session: *mut EditorSessionHandle) -> bool {
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

/// Explicitly closes the write-through binding. Repeated calls are no-ops.
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

/// Marker row data; names use `oak_editor_marker_name_at`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakEditorMarker {
	pub time: OakTimeRange,
	pub color: i32,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_count(session: *const EditorSessionHandle) -> i32 {
	if session.is_null() {
		return 0;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	markers.lock().unwrap_or_else(|e| e.into_inner()).size() as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_at(
	session: *const EditorSessionHandle,
	index: i32,
	out: *mut OakEditorMarker,
) -> bool {
	if session.is_null() || out.is_null() || index < 0 {
		return false;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	let markers = markers.lock().unwrap_or_else(|e| e.into_inner());
	let Some(marker) = markers.at(index as usize) else {
		return false;
	};
	unsafe {
		*out = OakEditorMarker {
			time: OakTimeRange::from_native(marker.time()),
			color: marker.color(),
		}
	};
	true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_marker_name_at(
	session: *const EditorSessionHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if session.is_null() || index < 0 {
		return -1;
	}
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	let markers = markers.lock().unwrap_or_else(|e| e.into_inner());
	match markers.at(index as usize) {
		Some(marker) => copy_str_out(marker.name(), buf, buf_len),
		None => -1,
	}
}

/// Returns whether the work area is enabled and copies its range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_workarea(
	session: *const EditorSessionHandle,
	out: *mut OakTimeRange,
) -> bool {
	if session.is_null() || out.is_null() {
		return false;
	}
	let workarea = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.workarea
		.clone();
	let workarea = workarea.lock().unwrap_or_else(|e| e.into_inner());
	unsafe { *out = OakTimeRange::from_native(workarea.range()) };
	workarea.enabled()
}

/// Creates the auxiliary state tests need without exposing its types.
#[cfg(test)]
pub(crate) fn add_marker_for_test(session: *mut EditorSessionHandle, name: &str) {
	let markers = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.markers
		.clone();
	markers
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.add_marker(oak_timeline::marker::TimelineMarker::with_time(
			3,
			oak_core::TimeRange::new(oak_core::Rational::new(1, 1), oak_core::Rational::new(2, 1)),
			name,
		));
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn session_owns_current_sequence_and_auxiliary_queries() {
		unsafe {
			let session = oak_editor_new();
			assert!(!session.is_null());
			let name = "Sequence";
			let sequence = oak_editor_create_sequence(session, name.as_ptr(), name.len());
			assert!(crate::node::oak_node_id_is_valid(sequence));
			assert_eq!(oak_editor_current_sequence(session), sequence);
			assert_eq!(oak_editor_marker_count(session), 0);
			add_marker_for_test(session, "Marker");
			assert_eq!(oak_editor_marker_count(session), 1);
			let mut marker = OakEditorMarker::default();
			assert!(oak_editor_marker_at(session, 0, &mut marker));
			assert_eq!(marker.color, 3);
			assert_eq!(marker.time.in_.num, 1);
			assert_eq!(marker.time.out.num, 2);
			let mut name_buf = [0u8; 16];
			let name_len =
				oak_editor_marker_name_at(session, 0, name_buf.as_mut_ptr(), name_buf.len());
			assert_eq!(
				std::str::from_utf8(&name_buf[..name_len as usize]).unwrap(),
				"Marker"
			);
			assert!(!oak_editor_marker_at(session, 99, &mut marker));
			assert_eq!(
				oak_editor_marker_name_at(session, 99, name_buf.as_mut_ptr(), 16),
				-1
			);
			let mut range = OakTimeRange::default();
			assert!(!oak_editor_workarea(session, &mut range));

			let second = oak_editor_create_sequence(session, name.as_ptr(), name.len());
			assert!(crate::node::oak_node_id_is_valid(second));
			assert_eq!(oak_editor_current_sequence(session), second);
			assert_eq!(
				oak_editor_marker_count(session),
				0,
				"sequence switch resets aux state"
			);
			assert!(!oak_editor_set_current_sequence(session, u64::MAX));
			assert_eq!(oak_editor_current_sequence(session), second);
			oak_editor_release(session);
		}
	}

	#[test]
	fn null_handles_fail_closed() {
		unsafe {
			assert!(oak_editor_project(std::ptr::null()).is_null());
			assert!(!oak_editor_bind(std::ptr::null_mut()));
			assert!(!oak_editor_unbind(std::ptr::null_mut()));
			assert!(!oak_editor_storage_bound(std::ptr::null()));
			assert!(!oak_editor_set_current_sequence(std::ptr::null_mut(), 0));
			assert_eq!(
				oak_editor_current_sequence(std::ptr::null()),
				OAK_NODE_ID_INVALID
			);
			assert_eq!(oak_editor_marker_count(std::ptr::null()), 0);
			assert_eq!(
				oak_editor_marker_name_at(std::ptr::null(), 0, std::ptr::null_mut(), 0),
				-1
			);
			assert!(!oak_editor_workarea(std::ptr::null(), std::ptr::null_mut()));
			assert_eq!(
				oak_editor_save(std::ptr::null(), "x".as_ptr(), 1, 0),
				oak_storage::error::OAKSTORAGE_E_INVALID
			);
		}
	}
}

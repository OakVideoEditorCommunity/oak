//! C ABI for oak-storage (prefix `oak_storage_`): project open/save
//! through the backend registry, backend enumeration, URI validation,
//! and the project library (the database backend).
//!
//! Open/save bridge to the engine's project model: a loaded project
//! arrives as a [`ProjectHandle`](crate::node) (usable with every
//! `oak_project_*`/`oak_node_*` entry point); saving takes one back.
//!
//! Return codes: mutations return `OAKSTORAGE_OK` (0) on success or a
//! negative `OAKSTORAGE_E_*` code (oak-storage's public error codes).
//! Open returns a handle (null on failure) and reports the version
//! info code (0 = OK, positive = TOO_OLD/TOO_NEW/UNKNOWN_VERSION) via
//! an out parameter.

use oak_storage::error::Error;
use oak_storage::nodeutil;
use oak_storage::registry::Registry;
use oak_storage::uri::StorageUri;

use crate::codec::{copy_str_out, str_arg};
use crate::node::ProjectHandle;

/// Maps an oak-storage error to its public code.
fn code(e: Error) -> i32 {
	e.code()
}

/// Parses and resolves `uri` to its claiming backend.
fn resolve(
	uri: *const u8,
	uri_len: usize,
) -> std::result::Result<
	(
		StorageUri,
		std::sync::Arc<dyn oak_storage::backend::StorageBackend>,
	),
	i32,
> {
	let Some(text) = (unsafe { str_arg(uri, uri_len) }) else {
		return Err(oak_storage::error::OAKSTORAGE_E_INVALID);
	};
	let parsed = StorageUri::parse(text).map_err(|_| oak_storage::error::OAKSTORAGE_E_INVALID)?;
	let backend = Registry::global().resolve(&parsed).map_err(code)?;
	Ok((parsed, backend))
}

/// The configured default write-through library.
pub(crate) fn default_library() -> Result<StorageUri, i32> {
	if !oak_storage::writethrough::storage_enabled() {
		return Err(oak_storage::error::OAKSTORAGE_E_INVALID);
	}
	let uri = oak_storage::writethrough::library_uri()
		.ok_or(oak_storage::error::OAKSTORAGE_E_INVALID)?;
	StorageUri::parse(&uri).map_err(|_| oak_storage::error::OAKSTORAGE_E_INVALID)
}

/// A default-library URI selecting one project row.
pub(crate) fn project_uri(uuid: &str) -> Result<StorageUri, i32> {
	let base = default_library()?;
	StorageUri::parse(&format!("{}?project={uuid}", base.to_uri_string()))
		.map_err(|_| oak_storage::error::OAKSTORAGE_E_INVALID)
}

/// The configured default library URI, snprintf-style; negative when
/// write-through storage is disabled or cannot be resolved.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_default_library_uri(buf: *mut u8, buf_len: usize) -> i32 {
	match default_library() {
		Ok(uri) => copy_str_out(&uri.to_uri_string(), buf, buf_len),
		Err(code) => code,
	}
}

/// Opens the project at `uri` through the claiming backend. Returns
/// the project handle (ref count 1), or null on failure. When the
/// backend only reports version info (the project was written by an
/// older/newer build), the handle is null and `version_info` carries
/// the positive info code; `version_info` is 0 on a normal load.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_open(
	uri: *const u8,
	uri_len: usize,
	version_info: *mut i32,
) -> *mut ProjectHandle {
	if !version_info.is_null() {
		unsafe { *version_info = 0 };
	}
	let Ok((_uri, backend)) = resolve(uri, uri_len) else {
		return std::ptr::null_mut();
	};
	let result = match backend.load(&_uri) {
		Ok(r) => r,
		Err(e) => {
			if !version_info.is_null() {
				unsafe { *version_info = code(e) };
			}
			return std::ptr::null_mut();
		}
	};
	if !version_info.is_null() {
		unsafe { *version_info = result.version_info };
	}
	if result.project.is_null() {
		return std::ptr::null_mut();
	}
	// Bridge the oak-storage handle (a box around Arc<Mutex<Project>>)
	// into the engine's project handle: clone the Arc, then release the
	// box.
	let arc = unsafe { nodeutil::project_arc(&result.project) }.ok();
	nodeutil::release_project(result.project);
	match arc {
		Some(inner) => ProjectHandle::from_arc(inner),
		None => std::ptr::null_mut(),
	}
}

/// Saves the project to `uri` through the claiming backend. `options`
/// is the oak-storage save bitmask (unknown bits are ignored). Returns
/// `OAKSTORAGE_OK` or a negative error code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_save(
	project: *const ProjectHandle,
	uri: *const u8,
	uri_len: usize,
	options: u32,
) -> i32 {
	if project.is_null() {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	}
	let Ok((_uri, backend)) = resolve(uri, uri_len) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let inner = unsafe { &*project }.project().clone();
	let handle = nodeutil::make_project_owned(inner);
	let result = backend.save(handle, &_uri, options);
	nodeutil::release_project(handle);
	match result {
		Ok(()) => oak_storage::error::OAKSTORAGE_OK,
		Err(e) => code(e),
	}
}

/// Whether `uri` parses as a storage URI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_uri_is_valid(uri: *const u8, uri_len: usize) -> bool {
	match unsafe { str_arg(uri, uri_len) } {
		Some(text) => StorageUri::parse(text).is_ok(),
		None => false,
	}
}

/// Number of registered backends (built-ins are pre-registered).
#[unsafe(no_mangle)]
pub extern "C" fn oak_storage_backend_count() -> i32 {
	Registry::global().list().len() as i32
}

fn backend_string_at(index: i32, buf: *mut u8, buf_len: usize, field: usize) -> i32 {
	if index < 0 {
		return -1;
	}
	let list = Registry::global().list();
	let Some(entry) = list.get(index as usize) else {
		return -1;
	};
	let s = if field == 0 { &entry.0 } else { &entry.1 };
	copy_str_out(s, buf, buf_len)
}

/// The name of the backend at `index` (registration order),
/// snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_backend_name_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	backend_string_at(index, buf, buf_len, 0)
}

/// The URI scheme of the backend at `index`, snprintf-style; -1 when
/// out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_backend_scheme_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	backend_string_at(index, buf, buf_len, 1)
}

/// The name of the backend that claims `uri`, snprintf-style; -1 when
/// no backend claims it (or the URI is invalid).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_backend_for_uri(
	uri: *const u8,
	uri_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(text) = (unsafe { str_arg(uri, uri_len) }) else {
		return -1;
	};
	let Ok(parsed) = StorageUri::parse(text) else {
		return -1;
	};
	match Registry::global().resolve(&parsed) {
		Ok(backend) => copy_str_out(backend.name(), buf, buf_len),
		Err(_) => -1,
	}
}

// ---------------------------------------------------------------------------
// Project library (the database backend)
// ---------------------------------------------------------------------------

fn library_projects(
	uri: *const u8,
	uri_len: usize,
) -> std::result::Result<Vec<oak_storage::backends::database::ProjectInfo>, i32> {
	let Some(text) = (unsafe { str_arg(uri, uri_len) }) else {
		return Err(oak_storage::error::OAKSTORAGE_E_INVALID);
	};
	let parsed = StorageUri::parse(text).map_err(|_| oak_storage::error::OAKSTORAGE_E_INVALID)?;
	oak_storage::writethrough::backend()
		.list_projects(&parsed)
		.map_err(code)
}

/// Number of projects in the library at `uri`; negative error code on
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_count(uri: *const u8, uri_len: usize) -> i32 {
	match library_projects(uri, uri_len) {
		Ok(projects) => projects.len() as i32,
		Err(c) => c,
	}
}

fn library_string_at(
	uri: *const u8,
	uri_len: usize,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
	name: bool,
) -> i32 {
	if index < 0 {
		return -1;
	}
	let Ok(projects) = library_projects(uri, uri_len) else {
		return -1;
	};
	let Some(info) = projects.get(index as usize) else {
		return -1;
	};
	let s = if name { &info.name } else { &info.uuid };
	copy_str_out(s, buf, buf_len)
}

/// The uuid of the library project at `index`, snprintf-style; -1 on
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_uuid_at(
	uri: *const u8,
	uri_len: usize,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	library_string_at(uri, uri_len, index, buf, buf_len, false)
}

/// The display name of the library project at `index`, snprintf-style;
/// -1 on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_name_at(
	uri: *const u8,
	uri_len: usize,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	library_string_at(uri, uri_len, index, buf, buf_len, true)
}

/// Deletes the project `uuid` from the library at `uri`. Returns
/// `OAKSTORAGE_OK` or a negative error code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_delete(
	uri: *const u8,
	uri_len: usize,
	uuid: *const u8,
	uuid_len: usize,
) -> i32 {
	let (Some(uri), Some(uuid)) = (unsafe { str_arg(uri, uri_len) }, unsafe {
		str_arg(uuid, uuid_len)
	}) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let Ok(parsed) = StorageUri::parse(uri) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	match oak_storage::writethrough::backend().delete_project(&parsed, uuid) {
		Ok(()) => oak_storage::error::OAKSTORAGE_OK,
		Err(e) => code(e),
	}
}

/// Renames the project `uuid` in the library at `uri`. Returns
/// `OAKSTORAGE_OK` or a negative error code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_rename(
	uri: *const u8,
	uri_len: usize,
	uuid: *const u8,
	uuid_len: usize,
	new_name: *const u8,
	new_name_len: usize,
) -> i32 {
	let (Some(uri), Some(uuid), Some(new_name)) = (
		unsafe { str_arg(uri, uri_len) },
		unsafe { str_arg(uuid, uuid_len) },
		unsafe { str_arg(new_name, new_name_len) },
	) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let Ok(parsed) = StorageUri::parse(uri) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	match oak_storage::writethrough::backend().rename_project(&parsed, uuid, new_name) {
		Ok(()) => oak_storage::error::OAKSTORAGE_OK,
		Err(e) => code(e),
	}
}

/// Unix timestamps and derived graph statistics for one library row.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_created_at(uri: *const u8, uri_len: usize, index: i32) -> i64 {
	library_info(uri, uri_len, index).map_or(-1, |info| info.created_at.and_utc().timestamp())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_modified_at(uri: *const u8, uri_len: usize, index: i32) -> i64 {
	library_info(uri, uri_len, index).map_or(-1, |info| info.modified_at.and_utc().timestamp())
}

fn library_info(
	uri: *const u8,
	uri_len: usize,
	index: i32,
) -> Option<oak_storage::backends::database::ProjectInfo> {
	if index < 0 {
		return None;
	}
	let text = unsafe { str_arg(uri, uri_len) }?;
	let parsed = StorageUri::parse(text).ok()?;
	oak_storage::writethrough::backend()
		.list_projects(&parsed)
		.ok()?
		.into_iter()
		.nth(index as usize)
}

fn library_stats(
	uri: *const u8,
	uri_len: usize,
	index: i32,
) -> Option<oak_storage::backends::database::ProjectStats> {
	let info = library_info(uri, uri_len, index)?;
	let text = unsafe { str_arg(uri, uri_len) }?;
	let parsed = StorageUri::parse(text).ok()?;
	oak_storage::writethrough::backend()
		.project_stats(&parsed, &info.uuid)
		.ok()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_duration_ms(uri: *const u8, uri_len: usize, index: i32) -> i64 {
	library_stats(uri, uri_len, index).map_or(0, |stats| stats.duration_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_track_count(uri: *const u8, uri_len: usize, index: i32) -> i32 {
	library_stats(uri, uri_len, index).map_or(0, |stats| stats.track_count)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_clip_count(uri: *const u8, uri_len: usize, index: i32) -> i32 {
	library_stats(uri, uri_len, index).map_or(0, |stats| stats.clip_count)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_footage_count(uri: *const u8, uri_len: usize, index: i32) -> i32 {
	library_stats(uri, uri_len, index).map_or(0, |stats| stats.footage_count)
}

/// Creates a blank row in the selected library and returns its uuid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_create(
	uri: *const u8,
	uri_len: usize,
	name: *const u8,
	name_len: usize,
	uuid_buf: *mut u8,
	uuid_buf_len: usize,
) -> i32 {
	let (Some(uri), Some(name)) = (unsafe { str_arg(uri, uri_len) }, unsafe {
		str_arg(name, name_len)
	}) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	if name.trim().is_empty() {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	}
	let Ok(uri) = StorageUri::parse(uri) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let project = oak_node::project::Project::new();
	let uuid = {
		let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
		if guard.initialize().is_err() {
			return oak_storage::error::OAKSTORAGE_E_INVALID;
		}
		guard.settings.insert("projectname".into(), name.trim().into());
		guard.uuid.clone()
	};
	match oak_storage::writethrough::backend().save_project(&project, &uri, 0) {
		Ok(()) => copy_str_out(&uuid, uuid_buf, uuid_buf_len),
		Err(e) => code(e),
	}
}

/// Duplicates a library row (including its history) and returns the new uuid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_duplicate(
	uri: *const u8,
	uri_len: usize,
	uuid: *const u8,
	uuid_len: usize,
	new_uuid_buf: *mut u8,
	new_uuid_buf_len: usize,
) -> i32 {
	let (Some(uri), Some(uuid)) = (unsafe { str_arg(uri, uri_len) }, unsafe {
		str_arg(uuid, uuid_len)
	}) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let Ok(uri) = StorageUri::parse(uri) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	match oak_storage::writethrough::backend().duplicate_project(&uri, uuid, None) {
		Ok(info) => copy_str_out(&info.uuid, new_uuid_buf, new_uuid_buf_len),
		Err(e) => code(e),
	}
}

/// Imports a file-backed project as a new row and returns its uuid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_import(
	uri: *const u8,
	uri_len: usize,
	file_uri: *const u8,
	file_uri_len: usize,
	new_uuid_buf: *mut u8,
	new_uuid_buf_len: usize,
) -> i32 {
	let (Some(uri), Some(file_uri)) = (unsafe { str_arg(uri, uri_len) }, unsafe {
		str_arg(file_uri, file_uri_len)
	}) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let (Ok(uri), Ok(file_uri)) = (StorageUri::parse(uri), StorageUri::parse(file_uri)) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	match oak_storage::writethrough::backend().import_from_file(&uri, &file_uri) {
		Ok(uuid) => copy_str_out(&uuid, new_uuid_buf, new_uuid_buf_len),
		Err(e) => code(e),
	}
}

/// Exports a library row to a file URI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_storage_library_export(
	uri: *const u8,
	uri_len: usize,
	uuid: *const u8,
	uuid_len: usize,
	file_uri: *const u8,
	file_uri_len: usize,
) -> i32 {
	let (Some(uri), Some(uuid), Some(file_uri)) = (
		unsafe { str_arg(uri, uri_len) },
		unsafe { str_arg(uuid, uuid_len) },
		unsafe { str_arg(file_uri, file_uri_len) },
	) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	let (Ok(uri), Ok(file_uri)) = (StorageUri::parse(uri), StorageUri::parse(file_uri)) else {
		return oak_storage::error::OAKSTORAGE_E_INVALID;
	};
	match oak_storage::writethrough::backend().export_to_file(&uri, uuid, &file_uri) {
		Ok(()) => oak_storage::error::OAKSTORAGE_OK,
		Err(e) => code(e),
	}
}

/// Flushes all write-through bindings and stops the snapshot worker.
#[unsafe(no_mangle)]
pub extern "C" fn oak_storage_flush() {
	oak_storage::writethrough::flush_all();
}

#[cfg(test)]
mod tests {
	use super::*;

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	/// Built-in backends are pre-registered and a file URI resolves to
	/// one of them; an unknown scheme resolves to nothing.
	#[test]
	fn backend_enumeration_and_resolution() {
		unsafe {
			let count = oak_storage_backend_count();
			assert!(count >= 2, "built-ins pre-registered: {count}");
			let mut names = Vec::new();
			let mut buf = [0u8; 64];
			for i in 0..count {
				let n = oak_storage_backend_name_at(i, buf.as_mut_ptr(), buf.len());
				assert!(n > 0);
				names.push(std::str::from_utf8(&buf[..n as usize]).unwrap().to_string());
				assert!(oak_storage_backend_scheme_at(i, buf.as_mut_ptr(), buf.len()) > 0);
			}
			assert!(names.iter().any(|n| n == "ove-xml"), "{names:?}");
			assert!(names.iter().any(|n| n == "otio"), "{names:?}");
			assert_eq!(oak_storage_backend_name_at(-1, buf.as_mut_ptr(), 64), -1);
			assert_eq!(oak_storage_backend_name_at(count, buf.as_mut_ptr(), 64), -1);

			let (u, ul) = arg("file:///tmp/x.ove");
			let n = oak_storage_backend_for_uri(u, ul, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..n as usize]).unwrap(), "ove-xml");
			let (u, ul) = arg("strange://nothing");
			assert_eq!(
				oak_storage_backend_for_uri(u, ul, buf.as_mut_ptr(), buf.len()),
				-1
			);

			assert!(oak_storage_uri_is_valid(u, ul));
			assert!(oak_storage_uri_is_valid("file:///a.ove".as_ptr(), 13));
			assert!(!oak_storage_uri_is_valid(std::ptr::null(), 0));
		}
	}

	/// Save + open round-trip through the file backend: the project
	/// crosses the C ABI as an engine handle on both directions.
	#[test]
	fn file_save_open_roundtrip() {
		unsafe {
			let path = std::env::temp_dir()
				.join(format!("oakengine_storage_{}.ove", std::process::id()))
				.to_string_lossy()
				.into_owned();
			let uri = format!("file://{path}");
			let (u, ul) = arg(&uri);

			let project = crate::node::oak_project_new();
			assert!(!project.is_null());
			assert_eq!(
				oak_storage_save(project, u, ul, 0),
				oak_storage::error::OAKSTORAGE_OK
			);

			let mut info = -1i32;
			let loaded = oak_storage_open(u, ul, &mut info);
			assert!(!loaded.is_null());
			assert_eq!(info, oak_storage::error::OAKSTORAGE_OK);
			// The loaded project is a live engine handle.
			assert!(crate::node::oak_node_id_is_valid(
				crate::node::oak_project_root(loaded)
			));

			crate::node::oak_project_release(loaded);
			crate::node::oak_project_release(project);

			// A missing file fails closed; a non-project URI too.
			let (u, ul) = arg("file:///nonexistent/oak.ove");
			assert!(oak_storage_open(u, ul, std::ptr::null_mut()).is_null());
			let _ = std::fs::remove_file(&path);
		}
	}

	/// The library on a fresh database is empty; the mutations report
	/// the not-found error code.
	#[test]
	fn library_on_empty_database() {
		unsafe {
			let path = std::env::temp_dir()
				.join(format!("oakengine_lib_{}.sqlite", std::process::id()))
				.to_string_lossy()
				.into_owned();
			let _ = std::fs::remove_file(&path);
			// Two slashes, not three: `parse_target` requires an absolute
			// path and the platform path supplies its own root
			// (`/tmp/…` on unix, `C:\…` on Windows — a third slash would
			// make the Windows form root-relative, failing is_absolute).
			let uri = format!("oakdb+sqlite://{path}");
			let (u, ul) = arg(&uri);
			assert_eq!(oak_storage_library_count(u, ul), 0);
			let mut buf = [0u8; 64];
			assert_eq!(
				oak_storage_library_uuid_at(u, ul, 0, buf.as_mut_ptr(), 64),
				-1
			);
			let (id, idl) = arg("no-such-uuid");
			assert!(oak_storage_library_delete(u, ul, id, idl) < 0);
			let (n, nl) = arg("New Name");
			assert!(oak_storage_library_rename(u, ul, id, idl, n, nl) < 0);
			let _ = std::fs::remove_file(&path);
		}
	}
}

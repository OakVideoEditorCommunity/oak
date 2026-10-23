//! Safe wrapper over the engine's storage C ABI (`oak_storage_*`):
//! project open/save through the backend registry, backend
//! enumeration, URI validation, and the project library (the database
//! backend).

use std::ffi::c_void;

use thiserror::Error;

use crate::codec::string_out;
use crate::node::Project;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_storage_open(uri: *const u8, uri_len: usize, version_info: *mut i32) -> *mut c_void;
	fn oak_storage_save(
		project: *const c_void,
		uri: *const u8,
		uri_len: usize,
		options: u32,
	) -> i32;
	fn oak_storage_uri_is_valid(uri: *const u8, uri_len: usize) -> bool;
	fn oak_storage_backend_count() -> i32;
	fn oak_storage_backend_name_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_storage_backend_scheme_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_storage_backend_for_uri(
		uri: *const u8,
		uri_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_storage_library_count(uri: *const u8, uri_len: usize) -> i32;
	fn oak_storage_library_uuid_at(
		uri: *const u8,
		uri_len: usize,
		index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_storage_library_name_at(
		uri: *const u8,
		uri_len: usize,
		index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_storage_library_delete(
		uri: *const u8,
		uri_len: usize,
		uuid: *const u8,
		uuid_len: usize,
	) -> i32;
	fn oak_storage_library_rename(
		uri: *const u8,
		uri_len: usize,
		uuid: *const u8,
		uuid_len: usize,
		new_name: *const u8,
		new_name_len: usize,
	) -> i32;
}

/// The save succeeded / load carried no version note.
pub const OK: i32 = 0;
/// The project was written by an older build (upgraded on load).
pub const TOO_OLD: i32 = 1;
/// The project was written by a newer build (not loaded).
pub const TOO_NEW: i32 = 2;
/// The payload version is unknown (not loaded).
pub const UNKNOWN_VERSION: i32 = 3;

/// A storage operation failure: the engine's `OAKSTORAGE_E_*` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("storage operation failed with code {0}")]
pub struct StorageError(pub i32);

fn check(code: i32) -> Result<(), StorageError> {
	if code == 0 {
		Ok(())
	} else {
		Err(StorageError(code))
	}
}

/// The outcome of [`open`]: the project plus the backend's version
/// note (0 = OK, or [`TOO_OLD`]/[`TOO_NEW`]/[`UNKNOWN_VERSION`]).
pub struct OpenedProject {
	/// The loaded project (None when the backend only reported version
	/// info — a too-new/unknown payload carries no project).
	pub project: Option<Project>,
	/// The version info code.
	pub version_info: i32,
}

/// Opens the project at `uri` through the claiming backend. Err on a
/// load failure (the code is the storage error).
pub fn open(uri: &str) -> Result<OpenedProject, StorageError> {
	let mut version_info = 0i32;
	let ptr = unsafe { oak_storage_open(uri.as_ptr(), uri.len(), &mut version_info) };
	if ptr.is_null() {
		if version_info > 0 {
			// Info-only outcome (TOO_NEW / UNKNOWN_VERSION): not a hard
			// error, but no project either.
			return Ok(OpenedProject {
				project: None,
				version_info,
			});
		}
		return Err(StorageError(version_info));
	}
	Ok(OpenedProject {
		project: Some(Project::from_raw(ptr)),
		version_info,
	})
}

/// Saves the project to `uri` through the claiming backend.
/// `options` is the oak-storage save bitmask (0 for defaults).
pub fn save(project: &Project, uri: &str, options: u32) -> Result<(), StorageError> {
	check(unsafe { oak_storage_save(project.as_raw(), uri.as_ptr(), uri.len(), options) })
}

/// Whether `uri` parses as a storage URI.
pub fn uri_is_valid(uri: &str) -> bool {
	unsafe { oak_storage_uri_is_valid(uri.as_ptr(), uri.len()) }
}

/// A registered backend (name + URI scheme).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendInfo {
	pub name: String,
	pub scheme: String,
}

/// The registered backends, in registration order (the arbitration
/// order within a scheme).
pub fn backends() -> Vec<BackendInfo> {
	let count = unsafe { oak_storage_backend_count() }.max(0);
	(0..count)
		.filter_map(|i| {
			Some(BackendInfo {
				name: string_out(|buf, len| unsafe { oak_storage_backend_name_at(i, buf, len) })?,
				scheme: string_out(|buf, len| unsafe {
					oak_storage_backend_scheme_at(i, buf, len)
				})?,
			})
		})
		.collect()
}

/// The name of the backend claiming `uri` (None when none does).
pub fn backend_for_uri(uri: &str) -> Option<String> {
	string_out(|buf, len| unsafe { oak_storage_backend_for_uri(uri.as_ptr(), uri.len(), buf, len) })
}

/// A project in the library (uuid + display name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryProject {
	pub uuid: String,
	pub name: String,
}

/// The projects in the library at `uri` (an `oakdb+sqlite:///…` URI).
pub fn library_projects(uri: &str) -> Result<Vec<LibraryProject>, StorageError> {
	let count = unsafe { oak_storage_library_count(uri.as_ptr(), uri.len()) };
	if count < 0 {
		return Err(StorageError(count));
	}
	Ok((0..count)
		.filter_map(|i| {
			Some(LibraryProject {
				uuid: string_out(|buf, len| unsafe {
					oak_storage_library_uuid_at(uri.as_ptr(), uri.len(), i, buf, len)
				})?,
				name: string_out(|buf, len| unsafe {
					oak_storage_library_name_at(uri.as_ptr(), uri.len(), i, buf, len)
				})?,
			})
		})
		.collect())
}

/// Deletes a project from the library.
pub fn library_delete(uri: &str, uuid: &str) -> Result<(), StorageError> {
	check(unsafe { oak_storage_library_delete(uri.as_ptr(), uri.len(), uuid.as_ptr(), uuid.len()) })
}

/// Renames a project in the library.
pub fn library_rename(uri: &str, uuid: &str, new_name: &str) -> Result<(), StorageError> {
	check(unsafe {
		oak_storage_library_rename(
			uri.as_ptr(),
			uri.len(),
			uuid.as_ptr(),
			uuid.len(),
			new_name.as_ptr(),
			new_name.len(),
		)
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn file_roundtrip_and_enumeration() {
		let names: Vec<String> = backends().iter().map(|b| b.name.clone()).collect();
		assert!(names.iter().any(|n| n == "ove-xml"), "{names:?}");
		assert!(names.iter().any(|n| n == "otio"), "{names:?}");
		assert_eq!(
			backend_for_uri("file:///tmp/x.ove").as_deref(),
			Some("ove-xml")
		);
		assert!(backend_for_uri("strange://nothing").is_none());
		assert!(uri_is_valid("file:///a.ove"));

		let path = std::env::temp_dir()
			.join(format!("oaksdk_storage_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let uri = format!("file://{path}");
		let project = Project::new();
		save(&project, &uri, 0).expect("save");
		let opened = open(&uri).expect("open");
		assert_eq!(opened.version_info, OK);
		let loaded = opened.project.expect("project");
		assert!(loaded.root().is_valid());
		assert!(open("file:///nonexistent/oak.ove").is_err());
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn library_on_empty_database() {
		let path = std::env::temp_dir()
			.join(format!("oaksdk_lib_{}.sqlite", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let _ = std::fs::remove_file(&path);
		// Two slashes, not three: the platform path supplies its own root
		// (`/tmp/…` on unix, `C:\…` on Windows — a third slash would make
		// the Windows form root-relative, failing is_absolute).
		let uri = format!("oakdb+sqlite://{path}");
		assert_eq!(library_projects(&uri).expect("list"), vec![]);
		assert!(library_delete(&uri, "no-such-uuid").is_err());
		assert!(library_rename(&uri, "no-such-uuid", "x").is_err());
		let _ = std::fs::remove_file(&path);
	}
}

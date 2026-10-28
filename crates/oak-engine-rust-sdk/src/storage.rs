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
	fn oak_storage_default_library_uri(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_storage_library_created_at(uri: *const u8, uri_len: usize, index: i32) -> i64;
	fn oak_storage_library_modified_at(uri: *const u8, uri_len: usize, index: i32) -> i64;
	fn oak_storage_library_duration_ms(uri: *const u8, uri_len: usize, index: i32) -> i64;
	fn oak_storage_library_track_count(uri: *const u8, uri_len: usize, index: i32) -> i32;
	fn oak_storage_library_clip_count(uri: *const u8, uri_len: usize, index: i32) -> i32;
	fn oak_storage_library_footage_count(uri: *const u8, uri_len: usize, index: i32) -> i32;
	fn oak_storage_library_create(
		uri: *const u8,
		uri_len: usize,
		name: *const u8,
		name_len: usize,
		uuid_buf: *mut u8,
		uuid_buf_len: usize,
	) -> i32;
	fn oak_storage_library_duplicate(
		uri: *const u8,
		uri_len: usize,
		uuid: *const u8,
		uuid_len: usize,
		new_uuid_buf: *mut u8,
		new_uuid_buf_len: usize,
	) -> i32;
	fn oak_storage_library_import(
		uri: *const u8,
		uri_len: usize,
		file_uri: *const u8,
		file_uri_len: usize,
		new_uuid_buf: *mut u8,
		new_uuid_buf_len: usize,
	) -> i32;
	fn oak_storage_library_export(
		uri: *const u8,
		uri_len: usize,
		uuid: *const u8,
		uuid_len: usize,
		file_uri: *const u8,
		file_uri_len: usize,
	) -> i32;
	fn oak_storage_flush();
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
	pub created_at: i64,
	pub modified_at: i64,
	pub duration_ms: i64,
	pub track_count: i32,
	pub clip_count: i32,
	pub footage_count: i32,
}

/// The projects in the library at `uri` (an `oakdb+sqlite:///…` URI).
pub fn library_projects(uri: &str) -> Result<Vec<LibraryProject>, StorageError> {
	let count = unsafe { oak_storage_library_count(uri.as_ptr(), uri.len()) };
	if count < 0 {
		return Err(StorageError(count));
	}
	Ok((0..count)
		.map(|i| LibraryProject {
				uuid: string_out(|buf, len| unsafe {
					oak_storage_library_uuid_at(uri.as_ptr(), uri.len(), i, buf, len)
				})
				.unwrap_or_default(),
				name: string_out(|buf, len| unsafe {
					oak_storage_library_name_at(uri.as_ptr(), uri.len(), i, buf, len)
				})
				.unwrap_or_default(),
				created_at: unsafe { oak_storage_library_created_at(uri.as_ptr(), uri.len(), i) },
				modified_at: unsafe { oak_storage_library_modified_at(uri.as_ptr(), uri.len(), i) },
				duration_ms: unsafe { oak_storage_library_duration_ms(uri.as_ptr(), uri.len(), i) },
				track_count: unsafe { oak_storage_library_track_count(uri.as_ptr(), uri.len(), i) },
				clip_count: unsafe { oak_storage_library_clip_count(uri.as_ptr(), uri.len(), i) },
				footage_count: unsafe { oak_storage_library_footage_count(uri.as_ptr(), uri.len(), i) },
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

/// The configured default library URI (`None` when storage is disabled).
pub fn default_library_uri() -> Option<String> {
	string_out(|buf, len| unsafe { oak_storage_default_library_uri(buf, len) })
}

/// Flushes every binding and stops the snapshot worker.
pub fn flush() {
	unsafe { oak_storage_flush() }
}

fn string_result(f: impl Fn(*mut u8, usize) -> i32) -> Result<String, StorageError> {
	// Unlike read-only getters, create/duplicate/import are side-effecting:
	// never use the two-pass "query size, then call again" convention here.
	let mut buf = vec![0u8; 4096];
	let written = f(buf.as_mut_ptr(), buf.len());
	if written < 0 {
		return Err(StorageError(written));
	}
	if written as usize >= buf.len() {
		return Err(StorageError(-1));
	}
	buf.truncate(written as usize);
	String::from_utf8(buf).map_err(|_| StorageError(-1))
}

/// A typed project-library client. Session opening uses the same live
/// project instance that the library and write-through binding share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLibrary {
	uri: String,
}

impl ProjectLibrary {
	pub fn new(uri: impl Into<String>) -> Result<Self, StorageError> {
		let uri = uri.into();
		if !uri_is_valid(&uri) || backend_for_uri(&uri).as_deref() != Some("oakdb") {
			return Err(StorageError(-1));
		}
		Ok(Self { uri })
	}

	pub fn configured() -> Result<Option<Self>, StorageError> {
		Ok(default_library_uri().map(|uri| Self { uri }))
	}

	pub fn uri(&self) -> &str {
		&self.uri
	}

	pub fn projects(&self) -> Result<Vec<LibraryProject>, StorageError> {
		library_projects(&self.uri)
	}

	pub fn create(&self, name: &str) -> Result<String, StorageError> {
		string_result(|buf, len| unsafe {
			oak_storage_library_create(
				self.uri.as_ptr(),
				self.uri.len(),
				name.as_ptr(),
				name.len(),
				buf,
				len,
			)
		})
	}

	pub fn delete(&self, uuid: &str) -> Result<(), StorageError> {
		library_delete(&self.uri, uuid)
	}

	pub fn rename(&self, uuid: &str, name: &str) -> Result<(), StorageError> {
		library_rename(&self.uri, uuid, name)
	}

	pub fn duplicate(&self, uuid: &str) -> Result<String, StorageError> {
		string_result(|buf, len| unsafe {
			oak_storage_library_duplicate(
				self.uri.as_ptr(),
				self.uri.len(),
				uuid.as_ptr(),
				uuid.len(),
				buf,
				len,
			)
		})
	}

	pub fn import(&self, file_uri: &str) -> Result<String, StorageError> {
		string_result(|buf, len| unsafe {
			oak_storage_library_import(
				self.uri.as_ptr(),
				self.uri.len(),
				file_uri.as_ptr(),
				file_uri.len(),
				buf,
				len,
			)
		})
	}

	pub fn export(&self, uuid: &str, file_uri: &str) -> Result<(), StorageError> {
		check(unsafe {
			oak_storage_library_export(
				self.uri.as_ptr(),
				self.uri.len(),
				uuid.as_ptr(),
				uuid.len(),
				file_uri.as_ptr(),
				file_uri.len(),
			)
		})
	}

	/// Opens a row from the configured default library. Like the app,
	/// sessions bind only to the default library, not an arbitrary client.
	pub fn open(&self, uuid: &str) -> Result<crate::editor::EditorSession, StorageError> {
		if default_library_uri().as_deref() != Some(self.uri.as_str()) {
			return Err(StorageError(-1));
		}
		crate::editor::EditorSession::open_project_from_library(uuid)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::state::Config;

	fn storage_config(backend: Option<&str>, path: Option<&str>) {
		Config::global()
			.set("Storage/Backend", backend.unwrap_or("off"))
			.expect("backend config");
		if let Some(path) = path {
			Config::global()
				.set("Storage/SqlitePath", path)
				.expect("sqlite path config");
		}
	}

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

	#[test]
	fn library_lifecycle_and_live_session() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		flush();
		let path = std::env::temp_dir()
			.join(format!("oaksdk_library_{}.sqlite", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let exported = std::env::temp_dir()
			.join(format!("oaksdk_library_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let _ = std::fs::remove_file(&path);
		let _ = std::fs::remove_file(&exported);
		storage_config(Some("sqlite"), Some(&path));

		let library = ProjectLibrary::configured()
			.expect("configured library")
			.expect("sqlite enabled");
		assert!(ProjectLibrary::new("not a uri").is_err());
		assert!(library.create("   ").is_err());
		assert!(library.duplicate("missing").is_err());
		assert!(library.import("/definitely/missing/project.ove").is_err());
		assert!(library.export("missing", "/tmp/never.ove").is_err());
		assert!(library.projects().expect("initial rows").is_empty());
		let dropped = crate::editor::EditorSession::create_in_library("Drop Binding")
			.expect("drop-bound row");
		let dropped_uuid = dropped.project().uuid();
		assert!(dropped.storage_bound());
		drop(dropped);
		library.delete(&dropped_uuid).expect("delete after Drop unbinds");
		assert!(library.projects().expect("drop leaves no row").is_empty());

		let session = crate::editor::EditorSession::create_in_library("Library Test")
			.expect("create live row");
		let uuid = session.project().uuid();
		assert_eq!(library.projects().expect("row after create").len(), 1);
		assert!(!uuid.is_empty());
		assert!(session.storage_bound());
		let sequence = session.create_sequence("Sequence").expect("sequence");
		let project = session.project();
		let list = project.sequence_track_list_at(sequence, 0);
		let track = project.tracklist_track_at(list, 0);
		let clip = project.create_clip().expect("clip");
		assert!(project.track_append_block(track, clip));
		assert!(project.block_set_length(clip, crate::types::Rational::new(2, 1)));
		let media = crate::test_support::make_test_clip("library");
		project.import_footage(&media).expect("import footage");

		let rows = library.projects().expect("rows after edit");
		let row = rows.iter().find(|row| row.uuid == uuid).expect("created row");
		assert_eq!(row.name, "Library Test");
		assert_eq!(row.track_count, 4);
		assert_eq!(row.clip_count, 1);
		assert_eq!(row.footage_count, 1);
		assert!(row.duration_ms >= 2_000);
		assert!(row.created_at > 0 && row.modified_at > 0);

		library.rename(&uuid, "Renamed").expect("rename");
		let duplicate = library.duplicate(&uuid).expect("duplicate");
		assert_eq!(library.projects().expect("rows after duplicate").len(), 2);
		assert_ne!(duplicate, uuid);
		library.export(&uuid, &exported).expect("export");
		let imported = library.import(&exported).expect("import");
		assert_eq!(library.projects().expect("rows after import").len(), 3);
		assert_ne!(imported, uuid);

		let expected = session.snapshot();
		let opened = library.open(&duplicate).expect("open duplicate");
		let snapshot = opened.snapshot();
		assert_eq!(snapshot.sequences, expected.sequences);
		assert_eq!(snapshot.footages, expected.footages);
		opened.create_sequence("Copy Only").expect("edit duplicate");
		assert_eq!(opened.snapshot().sequences.len(), 2);
		assert_eq!(session.snapshot().sequences.len(), 1, "duplicate edits stay independent");

		let imported_session = library.open(&imported).expect("open import");
		assert_eq!(imported_session.snapshot().sequences, expected.sequences);
		assert_eq!(imported_session.snapshot().footages, expected.footages);
		let custom = ProjectLibrary::new(format!(
			"oakdb+sqlite://{}/other.sqlite",
			std::env::temp_dir().display()
		))
		.expect("custom library URI");
		assert!(custom.open(&duplicate).is_err(), "custom client cannot open the default library");

		imported_session.close();
		opened.close();
		session.close();
		assert!(!imported_session.storage_bound());
		assert!(!opened.storage_bound());
		assert!(!session.storage_bound());
		for id in [uuid, duplicate, imported] {
			library.delete(&id).expect("delete row");
		}
		let remaining = library.projects().expect("rows after delete");
		assert!(remaining.is_empty(), "remaining rows: {remaining:?}");
		storage_config(Some("off"), None);
		assert!(ProjectLibrary::configured().expect("disabled config").is_none());
		assert!(crate::editor::EditorSession::create_in_library("Disabled").is_err());
		flush();
		let _ = std::fs::remove_file(path);
		let _ = std::fs::remove_file(exported);
		let _ = std::fs::remove_file(media);
	}
}

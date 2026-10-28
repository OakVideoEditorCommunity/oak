//! Storage-backed project-library DTOs and CRUD helpers.
//!
//! This module supplies the domain facade in `media.rs`; it does not represent
//! an open editor session or own a live `ProjectRef`.

/// Owned project-library row enriched with aggregate timeline statistics.
/// Timestamps are whole Unix seconds; duration is milliseconds.
#[derive(Debug, Clone, PartialEq)]
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

/// Storage-backed project collection located by a parsed library URI.
pub(crate) struct ProjectLibrary {
	uri: oak_storage::uri::StorageUri,
}

impl ProjectLibrary {
	/// Parses and validates the library URI without opening a project.
	pub(crate) fn new(uri: String) -> Result<Self, String> {
		let uri = oak_storage::uri::StorageUri::parse(&uri).map_err(|e| e.to_string())?;
		Ok(Self { uri })
	}

	/// Lists projects and attaches best-effort statistics to each row.
	/// A failed statistics lookup yields zeroes rather than hiding the project.
	pub(crate) fn projects(&self) -> Result<Vec<LibraryProject>, String> {
		let backend = oak_storage::writethrough::backend();
		let infos = backend.list_projects(&self.uri).map_err(|e| e.to_string())?;
		Ok(infos
			.into_iter()
			.map(|info| {
				let stats = backend.project_stats(&self.uri, &info.uuid).unwrap_or_default();
				LibraryProject {
					uuid: info.uuid,
					name: info.name,
					created_at: info.created_at.and_utc().timestamp(),
					modified_at: info.modified_at.and_utc().timestamp(),
					duration_ms: stats.duration_ms,
					track_count: stats.track_count,
					clip_count: stats.clip_count,
					footage_count: stats.footage_count,
				}
			})
			.collect())
	}

	/// Creates and persists an initialized project, returning its stable UUID.
	pub(crate) fn create(&self, name: &str) -> Result<String, String> {
		if name.trim().is_empty() {
			return Err("invalid name".into());
		}
		let project = super::create_project();
		let uuid = {
			let mut guard = super::lock(&project);
			guard.settings.insert("projectname".into(), name.trim().into());
			guard.uuid.clone()
		};
		oak_storage::writethrough::backend()
			.save_project(&project, &self.uri, 0)
			.map_err(|e| e.to_string())?;
		Ok(uuid)
	}

	/// Permanently deletes one project from this library.
	pub(crate) fn delete(&self, uuid: &str) -> Result<(), String> {
		oak_storage::writethrough::backend()
			.delete_project(&self.uri, uuid)
			.map_err(|e| e.to_string())
	}

	/// Changes the library display name without changing the project UUID.
	pub(crate) fn rename(&self, uuid: &str, name: &str) -> Result<(), String> {
		oak_storage::writethrough::backend()
			.rename_project(&self.uri, uuid, name)
			.map_err(|e| e.to_string())
	}

	/// Duplicates a project server-side and returns the new project's UUID.
	pub(crate) fn duplicate(&self, uuid: &str) -> Result<String, String> {
		let info = oak_storage::writethrough::backend()
			.duplicate_project(&self.uri, uuid, None)
			.map_err(|e| e.to_string())?;
		Ok(info.uuid)
	}

	/// Imports a storage URI into this library and returns the imported UUID.
	pub(crate) fn import(&self, file_uri: &str) -> Result<String, String> {
		let file_uri = oak_storage::uri::StorageUri::parse(file_uri).map_err(|e| e.to_string())?;
		oak_storage::writethrough::backend()
			.import_from_file(&self.uri, &file_uri)
			.map_err(|e| e.to_string())
	}

	/// Exports one library project to a file-backed storage URI.
	pub(crate) fn export(&self, uuid: &str, file_uri: &str) -> Result<(), String> {
		let file_uri = oak_storage::uri::StorageUri::parse(file_uri).map_err(|e| e.to_string())?;
		oak_storage::writethrough::backend()
			.export_to_file(&self.uri, uuid, &file_uri)
			.map_err(|e| e.to_string())
	}
}

// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Boundary tests for the oakstorage write-through façade
//! (`oak_storage::writethrough`): configuration gating, binding lifecycle,
//! write-through / flush / unbind, error recording, and the null-handle
//! contract.
//!
//! The config store is process-global, so all tests serialize on a local
//! mutex and write `OAK_CONFIG_DIR` to an isolated temp directory before
//! the first settings access (the tests never call `ConfigStore::save`, so
//! nothing persists).

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use oak_node::project::Project;
use oak_storage::handle::CHandle;
use oak_storage::nodeutil::make_project_owned;
use oak_storage::uri::StorageUri;
use oak_storage::writethrough;

/// Serializes the tests (global config + global bindings + the snapshot
/// thread).
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Take the lock and isolate the config store under a per-process temp
/// directory (once) before any settings access.
fn isolated_config() -> MutexGuard<'static, ()> {
	static INIT: OnceLock<()> = OnceLock::new();
	let guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	INIT.get_or_init(|| {
		let dir = std::env::temp_dir().join(format!(
			"oakstorage_writethrough_config_{}",
			std::process::id()
		));
		let _ = std::fs::remove_dir_all(&dir);
		let _ = std::fs::create_dir_all(&dir);
		std::env::set_var("OAK_CONFIG_DIR", &dir);
	});
	guard
}

fn set_storage_config(backend: &str, sqlite_path: Option<&std::path::Path>, pg_url: Option<&str>) {
	let store = oak_core::configstore::ConfigStore::instance();
	store.set(Some("Storage"), "Backend", backend);
	store.set(
		Some("Storage"),
		"SqlitePath",
		sqlite_path
			.map(|p| p.to_string_lossy().into_owned())
			.as_deref()
			.unwrap_or(""),
	);
	store.set(Some("Storage"), "PgUrl", pg_url.unwrap_or(""));
}

fn work_dir(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!(
		"oakstorage_writethrough_{tag}_{}",
		std::process::id()
	));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).expect("work dir");
	dir
}

fn project_handle() -> (CHandle, Arc<Mutex<Project>>) {
	let project = Project::new();
	let handle = make_project_owned(project.clone());
	(handle, project)
}

fn release(handle: CHandle) {
	if let Some(release) = handle.release {
		// SAFETY: the handle is an owned box (refcount 1) that this test
		// created via `make_project_owned`.
		unsafe { release(handle.ctx) };
	}
}

// ---- null handles and status ------------------------------------------

#[test]
fn null_handles_are_safe_no_ops() {
	let _guard = isolated_config();
	assert!(!writethrough::is_bound(CHandle::null()));
	assert_eq!(writethrough::last_error(CHandle::null()), None);
	writethrough::bind_project(CHandle::null());
	writethrough::unbind_project(CHandle::null());
	// Nothing to flush is a no-op (and idempotent).
	writethrough::flush_all();
	writethrough::flush_all();
}

// ---- configuration gating ---------------------------------------------

#[test]
fn backend_disabled_by_config_leaves_projects_unbound() {
	let _guard = isolated_config();
	set_storage_config("off", None, None);
	assert!(!writethrough::storage_enabled());

	let (handle, _project) = project_handle();
	writethrough::bind_project(handle);
	assert!(
		!writethrough::is_bound(handle),
		"disabled backend must not bind"
	);
	writethrough::unbind_project(handle);
	release(handle);
}

#[test]
fn backend_enabled_by_config_binds_and_round_trips() {
	let _guard = isolated_config();
	let dir = work_dir("sq");
	let db = dir.join("library.db");
	set_storage_config("sqlite", Some(&db), None);
	assert!(writethrough::storage_enabled());

	let (handle, project) = project_handle();
	assert!(!writethrough::is_bound(handle));
	writethrough::bind_project(handle);
	assert!(writethrough::is_bound(handle));
	assert_eq!(writethrough::last_error(handle), None);

	// Re-binding is idempotent (the existing binding is kept).
	writethrough::bind_project(handle);
	assert!(writethrough::is_bound(handle));

	// Add real state so the round trip carries more than an empty row.
	let original_uuid = {
		let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
		let (core, behavior) = oak_node::folder::create("Written Through");
		guard.graph.add_node(core, behavior);
		guard.uuid.clone()
	};

	// A command-success notification writes the project through.
	writethrough::note_command();
	assert_eq!(
		writethrough::last_error(handle),
		None,
		"a write-through to a writable temp path succeeds"
	);

	// The write-through is real: the sqlite library file exists...
	assert!(
		db.is_file(),
		"the sqlite library file was created at {}",
		db.display()
	);
	assert!(
		std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0) > 0,
		"the sqlite library file is non-empty"
	);

	// Flush drains and snapshots; it is idempotent and safe afterwards.
	writethrough::flush_all();
	writethrough::flush_all();

	// ...and a fresh backend load reads the bound project's state back.
	let uri = StorageUri::parse(&writethrough::library_uri().expect("library uri"))
		.expect("parse library uri");
	let loaded = writethrough::backend()
		.load_project(&uri)
		.expect("load the written-through project");
	{
		let guard = loaded.lock().unwrap_or_else(|e| e.into_inner());
		assert_eq!(
			guard.uuid, original_uuid,
			"the loaded library row is the bound project"
		);
		let labels: Vec<String> = guard
			.graph
			.node_ids()
			.into_iter()
			.filter_map(|id| guard.graph.get(id))
			.map(|entry| entry.core.label.clone())
			.collect();
		assert!(
			labels.iter().any(|label| label == "Written Through"),
			"the loaded project carries the state added before note_command: {labels:?}"
		);
	}

	writethrough::unbind_project(handle);
	assert!(!writethrough::is_bound(handle), "unbind drops the binding");
	assert_eq!(writethrough::last_error(handle), None);

	release(handle);
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_through_failure_is_recorded_in_last_error() {
	let _guard = isolated_config();
	// A directory cannot be opened as a SQLite database: the write-through
	// fails and the binding records the error instead of panicking.
	let dir = work_dir("bad");
	set_storage_config("sqlite", Some(&dir), None);

	let (handle, _project) = project_handle();
	writethrough::bind_project(handle);
	assert!(writethrough::is_bound(handle));
	writethrough::note_command();
	assert!(
		writethrough::last_error(handle).is_some(),
		"a failing write-through must record last_error"
	);

	writethrough::flush_all();
	writethrough::unbind_project(handle);
	release(handle);
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_uri_resolves_sqlite_and_pg_configurations() {
	let _guard = isolated_config();
	let dir = work_dir("uri");
	let db = dir.join("lib.db");
	set_storage_config("sqlite", Some(&db), None);
	let uri = writethrough::library_uri().expect("sqlite uri");
	assert!(uri.starts_with("oakdb+sqlite://"), "{uri}");
	assert!(uri.ends_with("lib.db"), "{uri}");

	// PostgreSQL: the postgres:// scheme is stripped into the oakdb body.
	set_storage_config(
		"pg",
		None,
		Some("postgres://user:pass@host:5432/db"),
	);
	assert_eq!(
		writethrough::library_uri().as_deref(),
		Some("oakdb+pg://user:pass@host:5432/db")
	);
	// `postgresql://` is accepted too.
	set_storage_config("pg", None, Some("postgresql://user@host/db"));
	assert_eq!(
		writethrough::library_uri().as_deref(),
		Some("oakdb+pg://user@host/db")
	);

	// pg selected without a URL: no library configured.
	set_storage_config("pg", None, None);
	assert_eq!(writethrough::library_uri(), None);

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn default_library_path_honors_the_config_directory() {
	let _guard = isolated_config();
	let path = writethrough::default_library_path();
	assert!(path.ends_with("library.db"), "{path}");
	let config_dir = std::env::var("OAK_CONFIG_DIR").expect("test isolation set it");
	assert!(
		path.starts_with(&config_dir),
		"{path} must live under {config_dir}"
	);
}

#[test]
fn note_command_without_bindings_is_a_no_op() {
	let _guard = isolated_config();
	set_storage_config(
		"sqlite",
		Some(&work_dir("none").join("lib.db")),
		None,
	);
	// No project is bound in this process (the other tests' bindings are
	// dropped before they return): the notification must not panic.
	writethrough::note_command();
	writethrough::flush_all();
}

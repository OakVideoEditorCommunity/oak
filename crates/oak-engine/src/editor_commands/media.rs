//! Media import, timeline auxiliary handles and project-library storage helpers.
//!
//! The module bridges domain objects to probing, marker/work-area and storage
//! APIs. Functions returning project nodes or library data own the higher-level
//! validation; low-level handle helpers explicitly document their lifetime and
//! no-op behavior. Live editor sessions may instead use Arc-backed auxiliaries.

#![allow(unused_imports)]
use super::prelude::*;
use super::{editing::*, generators::*, library::*, moving::*, project::*, sequence::*, tracks::*, transitions::*};


/// Import a media file into the project's root folder (the facade's
/// `oakengine_project_import_footage`: the node is created in the graph
/// and one undoable "Import Footage" entry adds it to the root folder).
/// Media that fails the probe (missing, corrupt, or undecodable) is
/// rejected before it ever enters the graph — the facade's validity
/// rejection, which the C API skips.
pub fn import_footage(project: &ProjectRef, path: &Path) -> Result<NodeId, String> {
	if !path.exists() {
		return Err(format!("file does not exist: {}", path.display()));
	}
	let filename = path.to_string_lossy().into_owned();
	let (root, id) = {
		let mut guard = lock(project);
		if !guard.root.valid() {
			return Err("the project has no root folder".to_string());
		}
		let (mut core, mut behavior) = FootageBehavior::create();
		core.set_standard_value(
			"file_in",
			-1,
			oak_node::value::NodeValue::Text(filename.clone()),
		);
		let Some(f) = behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<FootageBehavior>())
		else {
			return Err(
				"internal error: footage node created without footage behavior".to_string(),
			);
		};
		f.filename = filename.clone();
		// Probe before the node enters the graph so a failed probe leaves
		// no orphan behind (and nothing lands on the undo stack).
		f.probe()
			.map_err(|e| format!("failed to probe \"{}\": {e}", filename))?;
		let id = guard.graph.add_node(core, behavior);
		let label = path
			.file_name()
			.map(|f| f.to_string_lossy().into_owned())
			.unwrap_or_else(|| filename.clone());
		if let Some(e) = guard.graph.get_mut(id) {
			e.core.label = label;
		}
		(guard.root, id)
	};
	let cmd =
		oak_task::nodeops::folder_add_child_command((project.clone(), root), (project.clone(), id));
	oak_undo::global::push(cmd, "Import Footage").map_err(|e| e.to_string())?;
	Ok(id)
}

/// Replaces a footage node's media path and reprobes it as one undoable edit.
///
/// The new path is probed before the graph is changed. On success, all proxy,
/// custom-generation and source-time settings are retained while streams,
/// decoder and timestamp follow the new file. Undo restores the complete prior
/// behavior, including the old path and probe state. Failure leaves the graph
/// and undo stack untouched.
pub fn replace_footage(project: &ProjectRef, footage: NodeId, path: &Path) -> Result<(), String> {
	if !path.is_file() {
		return Err(format!("file does not exist: {}", path.display()));
	}
	let (old, mut new) = {
		let guard = lock(project);
		let old = footage_behavior(&guard.graph, footage)
			.ok_or_else(|| "entry is not footage".to_string())?
			.clone();
		let mut new = old.clone();
		new.filename = path.to_string_lossy().into_owned();
		(old, new)
	};
	new.probe()
		.map_err(|e| format!("failed to probe \"{}\": {e}", path.display()))?;
	let redo_project = project.clone();
	let undo_project = project.clone();
	push_command(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				if let Some(entry) = lock(&redo_project).graph.get_mut(footage) {
					entry.behavior = Box::new(new.clone());
				}
			},
			move || {
				if let Some(entry) = lock(&undo_project).graph.get_mut(footage) {
					entry.behavior = Box::new(old.clone());
				}
			},
		),
		"Replace Footage",
	)
}

/// Deletes a project-bin entry, including its folder mount, as one undoable
/// "Delete Entry" command. Undo restores the graph node and original folder slot.
pub fn delete_entry(project: &ProjectRef, node: NodeId) -> Result<(), String> {
	push_command(
		oak_task::nodeops::remove_node_command(project.clone(), node),
		"Delete Entry",
	)
}

// ---------------------------------------------------------------------------
// Marker list / workarea auxiliaries
//
// The module's sequences never initialize their own marker/workarea state
// (`SequenceBehavior` defaults both to empty handles), so the app — like
// the facade before it — materializes one of each per open sequence. The
// engine owns the handles and releases them before the project drops.
// ---------------------------------------------------------------------------

/// Create a marker-list handle (owned, refcount 1).
pub fn marker_list_create() -> CHandle {
	oak_timeline::handle::make_owned(oak_timeline::marker::TimelineMarkerList::new())
}

/// Create a workarea handle (owned, refcount 1).
pub fn workarea_create() -> CHandle {
	oak_timeline::handle::make_owned(oak_timeline::workarea::TimelineWorkArea::new())
}

/// Release an owned marker-list / workarea handle (NULL-safe; the handle
/// is dead afterwards).
pub fn release_handle(h: &mut CHandle) {
	if let Some(release) = h.release {
		// SAFETY: `h` is an owned handle from `marker_list_create` /
		// `workarea_create`; the release runs the box's own destructor once
		// per owned reference, and the caller drops the handle afterwards.
		unsafe { release(h.ctx) };
	}
	h.ctx = std::ptr::null_mut();
}

/// One marker as the timeline shows it: in-point, name, color index.
pub fn markers_of(list: &CHandle) -> Vec<(Rational, String, i32)> {
	if list.is_null() {
		return Vec::new();
	}
	// SAFETY: `list` boxes a `TimelineMarkerList` (created by
	// `marker_list_create`); the read is shared and brief.
	let Some(l) = (unsafe {
		oak_timeline::handle::get::<
			std::sync::Arc<std::sync::Mutex<oak_timeline::marker::TimelineMarkerList>>,
		>(list)
	}) else {
		return Vec::new();
	};
	let l = l.lock().unwrap_or_else(|e| e.into_inner());
	(0..l.size())
		.filter_map(|i| l.at(i))
		.map(|m| (m.time().in_(), m.name().to_string(), m.color()))
		.collect()
}

/// The index of the first marker whose in-point equals `time`.
pub fn marker_index_at(list: &CHandle, time: Rational) -> Option<usize> {
	if list.is_null() {
		return None;
	}
	// SAFETY: as `markers_of`.
	let l = unsafe {
		oak_timeline::handle::get::<
			std::sync::Arc<std::sync::Mutex<oak_timeline::marker::TimelineMarkerList>>,
		>(list)
	}?;
	let l = l.lock().unwrap_or_else(|e| e.into_inner());
	(0..l.size()).find(|&i| l.at(i).map(|m| m.time().in_()) == Some(time))
}

/// The workarea's `(enabled, range)`.
pub fn workarea_state(wa: &CHandle) -> Option<(bool, TimeRange)> {
	if wa.is_null() {
		return None;
	}
	// SAFETY: `wa` boxes a `TimelineWorkArea` (created by
	// `workarea_create`); the read is shared and brief.
	let w = unsafe {
		oak_timeline::handle::get::<
			std::sync::Arc<std::sync::Mutex<oak_timeline::workarea::TimelineWorkArea>>,
		>(wa)
	}?;
	let w = w.lock().unwrap_or_else(|e| e.into_inner());
	Some((w.enabled(), *w.range()))
}

/// Live (non-undoable) workarea write: enable flag plus range.
pub fn workarea_set(wa: &CHandle, enabled: bool, range: TimeRange) {
	if wa.is_null() {
		return;
	}
	// SAFETY: `wa` boxes a `TimelineWorkArea`; the engine writes it only
	// from the UI thread.
	if let Some(w) = unsafe {
		oak_timeline::handle::get_mut::<
			std::sync::Arc<std::sync::Mutex<oak_timeline::workarea::TimelineWorkArea>>,
		>(wa)
	} {
		let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
		w.set_enabled(enabled);
		w.set_range(range);
	}
}

// ---------------------------------------------------------------------------
// Project library (M13 D4): the write-through database the manager browses
//
// The facade's library_* exports serialized rows to JSON only because they
// crossed the C ABI; the direct calls below return plain values.
// ---------------------------------------------------------------------------

/// The configured default library as a parsed URI; an error when the
/// write-through backend is disabled or the path does not resolve.
pub(crate) fn library() -> Result<oak_storage::uri::StorageUri, String> {
	if !oak_storage::writethrough::storage_enabled() {
		return Err("the project library is not configured".to_string());
	}
	let uri = oak_storage::writethrough::library_uri()
		.ok_or_else(|| "the project library path does not resolve".to_string())?;
	oak_storage::uri::StorageUri::parse(&uri).map_err(|e| e.to_string())
}

/// The library URI selecting one row (`…?project=<uuid>`).
pub(crate) fn project_uri(uuid: &str) -> Result<oak_storage::uri::StorageUri, String> {
	let uri = library()?;
	oak_storage::uri::StorageUri::parse(&format!("{}?project={uuid}", uri.to_uri_string()))
		.map_err(|e| e.to_string())
}

/// Creates the legacy project-library client used by the domain facade.
///
/// The manager-style CRUD operations do not open or bind a live project.
/// `library_open` and the storage functions below retain their explicit
/// `ProjectRef`/`CHandle` contracts; new editor-session callers use the
/// corresponding session APIs instead.
pub(crate) fn library_client() -> Result<ProjectLibrary, String> {
	let uri = library()?.to_uri_string();
	ProjectLibrary::new(uri).map_err(|e| e.to_string())
}

/// The library rows, most recently modified first (the project manager's
/// data source). With storage disabled the result is the empty list, not
/// an error (the facade contract).
pub fn library_list() -> Result<Vec<LibraryProject>, String> {
	if !oak_storage::writethrough::storage_enabled() {
		return Ok(Vec::new());
	}
	let client = library_client()?;
	let infos = client.projects().map_err(|e| e.to_string())?;
	Ok(infos
		.into_iter()
		.map(|info| {
			LibraryProject {
				uuid: info.uuid,
				name: info.name,
				created_at: info.created_at,
				modified_at: info.modified_at,
				duration_ms: info.duration_ms,
				track_count: info.track_count,
				clip_count: info.clip_count,
				footage_count: info.footage_count,
			}
		})
		.collect())
}

/// Create a blank project row named `name`; returns its uuid. The row
/// lands immediately (one `kind='import'` command), so the manager list
/// shows it before the first edit.
pub fn library_create(name: &str) -> Result<String, String> {
	library_client()?.create(name).map_err(|e| e.to_string())
}

/// Delete the library row `uuid` (cascades settings / snapshots /
/// journal).
pub fn library_delete(uuid: &str) -> Result<(), String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_client()?.delete(uuid).map_err(|e| e.to_string())
}

/// Rename the library row `uuid` (the manager's list name).
pub fn library_rename(uuid: &str, name: &str) -> Result<(), String> {
	if uuid.is_empty() || name.trim().is_empty() {
		return Err("invalid uuid or name".to_string());
	}
	library_client()?.rename(uuid, name.trim()).map_err(|e| e.to_string())
}

/// Duplicate the library row `uuid` (history included) under a fresh
/// uuid; returns the new row's uuid. `None` name defaults to
/// `<name> (copy)` backend-side.
pub fn library_duplicate(uuid: &str) -> Result<String, String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_client()?.duplicate(uuid).map_err(|e| e.to_string())
}

/// Import a `.ove` / `.otio` / `.fcpxml` project file as a new library
/// row; returns the new row's uuid.
pub fn library_import(path: &Path) -> Result<String, String> {
	library_client()?
		.import(&path.to_string_lossy())
		.map_err(|e| e.to_string())
}

/// Export the library row `uuid` to `path`; the format is dispatched by
/// extension through the oakstorage registry.
pub fn library_export(uuid: &str, path: &Path) -> Result<(), String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_client()?
		.export(uuid, &path.to_string_lossy())
		.map_err(|e| e.to_string())
}

/// Load the library row `uuid` as a fresh project (the modified flag is
/// cleared, mirroring the facade's `oakengine_project_load_library`; the
/// undo-stack clear and the storage bind are the engine adopt path's job).
pub fn library_open(uuid: &str) -> Result<ProjectRef, String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	let result = oak_storage::writethrough::backend()
		.load(&project_uri(uuid)?)
		.map_err(|e| e.to_string())?;
	if result.project.is_null() {
		return Err(format!(
			"library load of {uuid} returned no project (info code {})",
			result.version_info
		));
	}
	let handle = result.project;
	let project =
		unsafe { oak_storage::nodeutil::project_arc(&handle) }.map_err(|e| e.to_string())?;
	lock(&project).set_modified(false);
	Ok(project)
}

// ---------------------------------------------------------------------------
// Write-through binding (the engine's per-project session state)
// ---------------------------------------------------------------------------

/// Bind `project` to the configured default library and return the handle
/// the binding was registered under (the caller keeps it for
/// [`storage_bound`] / [`storage_last_error`] queries and releases it with
/// [`storage_unbind`]). No-op (None still returned) semantics mirror the
/// facade: an unconfigured library leaves the project unbound but the
/// handle is still usable for queries.
pub fn storage_bind(project: &ProjectRef) -> CHandle {
	let handle = oak_storage::nodeutil::make_project_owned(project.clone());
	oak_storage::writethrough::bind_project(handle);
	handle
}

/// Whether the project behind `handle` is bound to the write-through
/// session (the status bar's write state).
pub fn storage_bound(handle: &CHandle) -> bool {
	oak_storage::writethrough::is_bound(*handle)
}

/// The last write-through / snapshot error of the project behind
/// `handle`, if any.
pub fn storage_last_error(handle: &CHandle) -> Option<String> {
	oak_storage::writethrough::last_error(*handle)
}

/// Flush the project's pending writes, drop its binding and release the
/// query handle (the engine's project-close path).
pub fn storage_unbind(handle: CHandle) {
	oak_storage::writethrough::unbind_project(handle);
	oak_storage::nodeutil::release_project(handle);
}

/// Flush every bound project and stop the snapshot thread (app exit).
pub fn storage_flush() {
	oak_storage::writethrough::flush_all();
}

// ---------------------------------------------------------------------------
// Undoable edit primitives
//
// The timeline edits the facade's `oakengine_sequence_*` / `oakengine_clip_*`
// exports performed, rebuilt over the oaktimeline command structs and the
// oakundo global stack's safe [`oak_undo::global::push`]. Every entry point
// pushes exactly one undo row (composite edits assemble a multi command).
// ---------------------------------------------------------------------------

/// Push one command onto the global undo stack (redo then record).
pub fn push_command(cmd: oak_undo::undocommand::UndoCommand, name: &str) -> Result<(), String> {
	oak_undo::global::push(cmd, name).map_err(|e| e.to_string())
}

/// Assemble `children` into ONE multi command and push it (an empty set
/// is a no-op, mirroring the stack's empty-multi discard).
pub fn push_multi_command(
	children: Vec<oak_undo::undocommand::UndoCommand>,
	name: &str,
) -> Result<(), String> {
	if children.is_empty() {
		return Ok(());
	}
	let mut multi = oak_undo::undocommand::UndoCommand::multi();
	for child in children {
		multi.multi_add_child(child);
	}
	push_command(multi, name)
}

/// Push one command (private alias).
pub(crate) fn push(cmd: oak_undo::undocommand::UndoCommand, name: &str) -> Result<(), String> {
	push_command(cmd, name)
}

/// Assemble and push a multi command (private alias).
pub(crate) fn push_multi(children: Vec<oak_undo::undocommand::UndoCommand>, name: &str) -> Result<(), String> {
	push_multi_command(children, name)
}

/// An undoable edge add (the module's `oaknode_node_connect_undoable`
/// semantics): validated at creation (existence, connectability, not
/// already connected); the redo connects, the undo disconnects the input.
pub fn connect_command(
	p: &ProjectRef,
	from: NodeId,
	to: NodeId,
	input_id: &str,
) -> Result<oak_undo::undocommand::UndoCommand, String> {
	{
		let g = lock(p);
		if !g.graph.is_valid(from) || !g.graph.is_valid(to) {
			return Err("connect: node not found".to_string());
		}
		let entry = g.graph.get(to).ok_or("connect: node not found")?;
		let input = entry
			.core
			.get_input(input_id)
			.ok_or_else(|| format!("connect: no input \"{input_id}\""))?;
		if !input.is_connectable() {
			return Err(format!("connect: input \"{input_id}\" is not connectable"));
		}
		if g.graph.connected_output(to, input_id, -1).is_some() {
			return Err(format!(
				"connect: input \"{input_id}\" is already connected"
			));
		}
	}
	let (p1, p2) = (p.clone(), p.clone());
	let (id1, id2) = (input_id.to_string(), input_id.to_string());
	Ok(oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut g = lock(&p1);
			let _ = g.graph.connect(from, to, &id1, -1);
		},
		move || {
			let mut g = lock(&p2);
			g.graph.disconnect_input(to, &id2, -1);
		},
	))
}

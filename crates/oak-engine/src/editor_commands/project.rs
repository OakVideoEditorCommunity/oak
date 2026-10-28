//! Project lifecycle, graph behavior access and project-bin construction.
//!
//! `ProjectRef` identifies the one live project shared by editing, serialization
//! and storage. Lock helpers recover poisoned mutexes so a failed command does
//! not permanently block later edits. Sequence/folder creation and OVE loading
//! keep their distinct undo and filename/modified-flag contracts documented below.

#![allow(unused_imports)]
use super::prelude::*;
use super::{editing::*, generators::*, media::*, moving::*, sequence::*, tracks::*, transitions::*};


/// Lock a project, recovering from a poisoned lock (a panicking command
/// body must not wedge every later edit).
pub fn lock(p: &ProjectRef) -> MutexGuard<'_, Project> {
	p.lock().unwrap_or_else(|e| e.into_inner())
}

/// A timeline node reference (the oaktimeline command addressing).
pub fn node_ref(p: &ProjectRef, id: NodeId) -> NodeRef {
	NodeRef::new(p.clone(), id)
}

/// The node id whose stable identity is `identity` (`None` for the
/// sentinel). The app's widget-facing ids (clip ids, explorer entry ids)
/// ARE the node identities.
pub fn id_of(identity: u64) -> Option<NodeId> {
	NodeId::from_identity(identity)
}

// ---------------------------------------------------------------------------
// Behavior borrows
// ---------------------------------------------------------------------------

/// Borrow the sequence behavior at `id`.
pub fn sequence_behavior(g: &Graph, id: NodeId) -> Option<&SequenceBehavior> {
	g.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<SequenceBehavior>()
}

/// Borrow the track-list behavior at `id`.
pub fn track_list_behavior(g: &Graph, id: NodeId) -> Option<&TrackListBehavior> {
	g.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<TrackListBehavior>()
}

/// Borrow the track behavior at `id`.
pub fn track_behavior(g: &Graph, id: NodeId) -> Option<&TrackBehavior> {
	g.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<TrackBehavior>()
}

/// Borrow the clip behavior at `id`.
pub fn clip_behavior(g: &Graph, id: NodeId) -> Option<&ClipBlockBehavior> {
	g.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<ClipBlockBehavior>()
}

/// Borrow the footage behavior at `id`.
pub fn footage_behavior(g: &Graph, id: NodeId) -> Option<&FootageBehavior> {
	g.get(id)?
		.behavior
		.as_any()?
		.downcast_ref::<FootageBehavior>()
}

/// Whether `id` is a folder node.
pub fn is_folder(g: &Graph, id: NodeId) -> bool {
	g.get(id)
		.and_then(|e| e.behavior.as_any())
		.and_then(|a| a.downcast_ref::<oak_node::folder::FolderBehavior>())
		.is_some()
}

// ---------------------------------------------------------------------------
// Project lifecycle
// ---------------------------------------------------------------------------

/// Create a blank, initialized project (the facade's `project_create` +
/// `project_new`, minus the undo-stack clear and the storage bind — the
/// engine's adopt path owns those).
pub fn create_project() -> ProjectRef {
	let project = Project::new();
	lock(&project).initialize().ok();
	project
}

/// Load a `.ove` project file (plain XML; the module serializer ignores
/// the legacy compression flag). The filename is normalized to an absolute
/// path and the modified flag cleared, mirroring the facade's
/// `oakengine_project_load`.
pub fn load_ove(path: &Path) -> Result<ProjectRef, String> {
	let xml = std::fs::read_to_string(path)
		.map_err(|e| format!("failed to read \"{}\": {e}", path.display()))?;
	let project = oak_node::serializer::load(&xml).map_err(|e| format!("failed to parse: {e}"))?;
	let abs = if path.is_absolute() {
		path.to_path_buf()
	} else {
		std::env::current_dir()
			.map(|d| d.join(path))
			.unwrap_or_else(|_| path.to_path_buf())
	};
	let mut guard = lock(&project);
	guard.set_filename(&abs.to_string_lossy());
	guard.set_modified(false);
	drop(guard);
	Ok(project)
}

/// Write the project to `path` through the OVE serializer (the facade's
/// `oakengine_project_save` semantics: the target filename is recorded and
/// the modified flag cleared on success).
pub fn save_ove(project: &ProjectRef, path: &Path) -> Result<(), String> {
	let xml = {
		let guard = lock(project);
		oak_node::serializer::save(&guard).map_err(|e| format!("failed to serialize: {e}"))?
	};
	std::fs::write(path, &xml)
		.map_err(|e| format!("failed to write \"{}\": {e}", path.display()))?;
	let mut guard = lock(project);
	guard.set_filename(&path.to_string_lossy());
	guard.set_modified(false);
	Ok(())
}

/// The display name (`Project::name`: filename base or "(untitled)").
pub fn project_name(p: &Project) -> String {
	p.name()
}

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------

/// Every sequence node in the graph, in arena order.
pub fn sequence_ids(p: &Project) -> Vec<NodeId> {
	p.graph
		.node_ids()
		.into_iter()
		.filter(|&id| sequence_behavior(&p.graph, id).is_some())
		.collect()
}

/// Every footage node in the graph, in arena order.
pub fn footage_ids(p: &Project) -> Vec<NodeId> {
	p.graph
		.node_ids()
		.into_iter()
		.filter(|&id| footage_behavior(&p.graph, id).is_some())
		.collect()
}

/// Create a sequence node named `name` directly in the project's graph
/// (unlike the facade's `oakengine_sequence_new`, which kept the sequence
/// in a scratch project, the direct-rlib app keeps it in the project so
/// saves and the write-through library cover it). The sequence is attached
/// to the project's root folder so the project explorer shows it.
pub fn create_sequence(project: &ProjectRef, name: &str) -> NodeId {
	create_sequence_with_params(project, name, None)
}

/// Create a sequence with an explicit first-video-stream format. `params`
/// `(width, height, rate, interlaced)` overrides the sequence's default
/// video parameters; `None` keeps the defaults. The sequence is attached
/// to the root folder.
pub fn create_sequence_with_params(
	project: &ProjectRef,
	name: &str,
	params: Option<(i32, i32, Rational, bool)>,
) -> NodeId {
	let mut guard = lock(project);
	let (mut core, mut behavior) = SequenceBehavior::create();
	core.label = name.to_string();
	if let Some((width, height, rate, interlaced)) = params {
		if let Some(seq) = behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<SequenceBehavior>())
		{
			if seq.video_params.is_empty() {
				seq.video_params.push(VideoParams {
					width,
					height,
					frame_rate: rate,
					pixel_format: 4, // f32
					channels: 4,
					interlaced,
				});
			} else {
				let v = &mut seq.video_params[0];
				v.width = width;
				v.height = height;
				v.frame_rate = rate;
				v.interlaced = interlaced;
			}
		}
	}
	let root = guard.root;
	let seq = guard.graph.add_node(core, behavior);
	drop(guard);
	// Mount the sequence under the root folder. Not pushed to the undo
	// stack: like the default track layout below, it is part of the
	// sequence's creation, not an undoable edit.
	oak_task::nodeops::folder_add_child_command((project.clone(), root), (project.clone(), seq))
		.redo_now();
	// A new sequence starts with the default 2 video + 2 audio track
	// layout (user-mandated NLE default: V1, V2 on top, A1, A2 below).
	// Driven directly through the add-track commands' redo — NOT pushed
	// to the undo stack, a sequence's default layout is part of its
	// creation, not an undoable edit.
	for kind in [
		TrackType::Video,
		TrackType::Video,
		TrackType::Audio,
		TrackType::Audio,
	] {
		let Some(list) = find_or_create_track_list(project, seq, kind) else {
			continue;
		};
		oak_timeline::undogeneral::TimelineAddTrackCommand::new(node_ref(project, list)).redo();
	}
	seq
}

/// Create a folder under the project's root (the C++ File > New Folder
/// action). The "New Folder" command is pushed to the undo stack — an
/// explicit user action, unlike sequence creation.
pub fn create_folder(project: &ProjectRef, name: &str) -> Result<NodeId, String> {
	let root = {
		let guard = lock(project);
		if !guard.root.valid() {
			return Err("the project has no root folder".to_string());
		}
		guard.root
	};
	let (core, behavior) = oak_node::folder::create(name);
	let id = {
		let mut guard = lock(project);
		guard.graph.add_node(core, behavior)
	};
	let cmd =
		oak_task::nodeops::folder_add_child_command((project.clone(), root), (project.clone(), id));
	oak_undo::global::push(cmd, "New Folder").map_err(|e| e.to_string())?;
	Ok(id)
}

/// The stable type id of the text generator node (the redesigned `textv3`,
/// "org.olivevideoeditor.Olive.text3"): the bin entry the project panel's
/// "添加文本素材" action creates, and the type the timeline drop routes to
/// the generator-clip path instead of the footage path.
pub const TEXT_FOOTAGE_TYPE_ID: &str = "org.olivevideoeditor.Olive.text3";

/// The bin label a created text generator starts with (the plan's "文本"
/// entry name). Fixed, not localized at creation time: it is the node's
/// label, which the user renames like any other bin entry.
pub const TEXT_FOOTAGE_LABEL: &str = "文本";

/// Create a text generator in the project's root folder (the project
/// panel's "添加文本素材" action). ONE undo row covers the node AND its
/// mount: the undo detaches the node from the root folder and removes it
/// from the graph (its entry stays inside the command), so no orphan node
/// outlives the undo — unlike [`create_folder`], which leaves its node in
/// the graph and only undoes the mount. The redo re-inserts the entry,
/// which keeps its stable id while its slot is free (the
/// [`Graph::add_entry`] contract).
pub fn create_text_footage_node(project: &ProjectRef) -> Result<NodeId, String> {
	let root = {
		let guard = lock(project);
		if !guard.root.valid() {
			return Err("the project has no root folder".to_string());
		}
		guard.root
	};
	let Some((mut core, behavior)) =
		oak_node::factory::Factory::global().create_any(TEXT_FOOTAGE_TYPE_ID)
	else {
		return Err(format!(
			"the text node \"{TEXT_FOOTAGE_TYPE_ID}\" is not registered"
		));
	};
	core.label = TEXT_FOOTAGE_LABEL.to_string();
	// The eager insert gives the caller an id for the bin entry before the
	// command is built; the undo parks the entry here between runs.
	let id = {
		let mut guard = lock(project);
		guard.graph.add_node(core, behavior)
	};
	let detached: Arc<Mutex<Option<NodeEntry>>> = Arc::new(Mutex::new(None));
	let (redo_detached, undo_detached) = (detached.clone(), detached.clone());
	let (redo_project, undo_project) = (project.clone(), project.clone());
	let redo = move || {
		let mut guard = lock(&redo_project);
		if !guard.graph.is_valid(id) {
			let entry = redo_detached
				.lock()
				.unwrap_or_else(|e| e.into_inner())
				.take();
			if let Some(entry) = entry {
				guard.graph.add_entry(entry, id);
			}
		}
		if let Some(entry) = guard.graph.get_mut(id) {
			entry.core.bin_folder = Some(root);
		}
		if let Some(entry) = guard.graph.get_mut(root) {
			if let Some(folder) = entry
				.behavior
				.as_any_mut()
				.and_then(|a| a.downcast_mut::<FolderBehavior>())
			{
				folder.add_child(id);
			}
		}
	};
	let undo = move || {
		let mut guard = lock(&undo_project);
		if let Some(entry) = guard.graph.get_mut(root) {
			if let Some(folder) = entry
				.behavior
				.as_any_mut()
				.and_then(|a| a.downcast_mut::<FolderBehavior>())
			{
				folder.remove_child(id);
			}
		}
		if let Some(entry) = guard.graph.get_mut(id) {
			entry.core.bin_folder = None;
		}
		let taken = guard.graph.take_node(id);
		*undo_detached.lock().unwrap_or_else(|e| e.into_inner()) = taken;
	};
	let cmd = oak_undo::undocommand::UndoCommand::from_closures(redo, undo);
	push_command(cmd, "Add Text Footage")?;
	Ok(id)
}

/// Every folder node in the graph, in arena order.
pub fn folder_ids(p: &Project) -> Vec<NodeId> {
	p.graph
		.node_ids()
		.into_iter()
		.filter(|&id| is_folder(&p.graph, id))
		.collect()
}

/// Attach every orphaned sequence to the root folder with a non-undoable
/// command. Projects saved before sequences mounted under the root load
/// with their sequences free-floating; the open path runs this once to
/// migrate them.
pub fn ensure_sequences_mounted(project: &ProjectRef) {
	let (root, orphans) = {
		let guard = lock(project);
		let root = guard.root;
		let children = guard
			.graph
			.get(root)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<FolderBehavior>())
			.map(|f| f.children.clone())
			.unwrap_or_default();
		let orphans = sequence_ids(&guard)
			.into_iter()
			.filter(|&id| !children.contains(&id))
			.collect::<Vec<_>>();
		(root, orphans)
	};
	for id in orphans {
		oak_task::nodeops::folder_add_child_command((project.clone(), root), (project.clone(), id))
			.redo_now();
	}
}

/// Apply new display name and video parameters to a sequence (the
/// sequence-properties dialog's commit; mirrors the CLI's
/// `set_sequence_video_params`, plus the label). Not undoable, like the
/// CLI setter.
pub fn set_sequence_parameters(
	project: &ProjectRef,
	seq: NodeId,
	name: &str,
	width: i32,
	height: i32,
	rate: Rational,
	interlaced: bool,
) -> Result<(), String> {
	let mut guard = lock(project);
	let entry = guard
		.graph
		.get_mut(seq)
		.ok_or_else(|| "sequence no longer exists".to_string())?;
	entry.core.label = name.to_string();
	let Some(s) = entry
		.behavior
		.as_any_mut()
		.and_then(|a| a.downcast_mut::<SequenceBehavior>())
	else {
		return Err("entry is not a sequence".to_string());
	};
	if s.video_params.is_empty() {
		s.video_params.push(VideoParams {
			width,
			height,
			frame_rate: rate,
			pixel_format: 4, // f32
			channels: 4,
			interlaced,
		});
	} else {
		let v = &mut s.video_params[0];
		v.width = width;
		v.height = height;
		v.frame_rate = rate;
		v.interlaced = interlaced;
	}
	Ok(())
}

/// The label of a node (`NodeCore::label`).
pub fn node_label(g: &Graph, id: NodeId) -> String {
	g.get(id).map(|e| e.core.label.clone()).unwrap_or_default()
}

/// The type id of a node (empty when the id is stale).
pub fn node_type_id(g: &Graph, id: NodeId) -> String {
	g.get(id)
		.map(|e| e.behavior.type_id().to_string())
		.unwrap_or_default()
}

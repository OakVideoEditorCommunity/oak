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

//! M12 P3: the real project browser.
//!
//! Builds the widget's [`ProjectEntry`] tree from the project graph's
//! folder hierarchy (M14 R3: the direct oaknode walk — no facade): the
//! project root folder's children are the top-level entries, folders
//! expand through their `FolderBehavior` child lists. Entry ids are the
//! nodes' stable identities, so selection round-trips through
//! [`find_by_identity`].

use crate::widgets::project_explorer::ProjectEntry;

use crate::oakui::graphops::ProjectRef;

/// Converts one graph child, `snapshot`, into a project browser row.
///
/// The row id is the node's stable identity (`snapshot.id.raw()`), its label
/// is `snapshot.name`, and it is marked as a folder only when
/// `snapshot.kind` is [`quercus_engine_rs::node::NodeKind::Folder`]; a node
/// with no kind therefore renders as a leaf.
fn child_entry(snapshot: &quercus_engine_rs::editor::EntrySnapshot) -> ProjectEntry {
	ProjectEntry::new(
		snapshot.id.raw(),
		snapshot.name.clone(),
		snapshot.kind == Some(quercus_engine_rs::node::NodeKind::Folder),
	)
}

/// Maps each child of the folder node `snapshot` to a [`ProjectEntry`].
///
/// `snapshot` must be a folder from the project graph; its `children` are the
/// entries directly beneath it, in graph order. The folder's own identity and
/// name are not included in the result.
fn folder_children(snapshot: &quercus_engine_rs::editor::EntrySnapshot) -> Vec<ProjectEntry> {
	snapshot
		.children
		.iter()
		.map(child_entry)
		.collect()
}

fn find_snapshot<'a>(
	entry: &'a quercus_engine_rs::editor::EntrySnapshot,
	identity: u64,
) -> Option<&'a quercus_engine_rs::editor::EntrySnapshot> {
	if entry.id.raw() == identity {
		return Some(entry);
	}
	entry
		.children
		.iter()
		.find_map(|child| find_snapshot(child, identity))
}

/// Lists the project browser's top-level entries for `project`.
///
/// The entries are the children of the project graph's root folder, in graph
/// order; each [`ProjectEntry`] carries the node's stable identity as its id,
/// the display name, and whether it is a folder. `project` is resolved to an
/// editor session; when that fails (for example the project is not open) an
/// empty vector is returned rather than an error.
pub fn roots(project: &ProjectRef) -> Vec<ProjectEntry> {
	let Ok(session) = super::real::editor_session_for_project(project) else {
		return Vec::new();
	};
	folder_children(&session.snapshot().root)
}

/// The children of the folder whose identity is `parent_id` (empty when
/// not found or not a folder).
pub fn children(project: &ProjectRef, parent_id: u64) -> Vec<ProjectEntry> {
	let Ok(session) = super::real::editor_session_for_project(project) else {
		return Vec::new();
	};
	let snapshot = session.snapshot();
	find_snapshot(&snapshot.root, parent_id)
		.filter(|entry| entry.kind == Some(quercus_engine_rs::node::NodeKind::Folder))
		.map(folder_children)
		.unwrap_or_default()
}

/// The project node with `identity` (validated against the graph), or
/// `None`.
pub fn find_by_identity(
	project: &ProjectRef,
	identity: u64,
) -> Option<quercus_engine_rs::node::NodeId> {
	let session = super::real::editor_session_for_project(project).ok()?;
	find_snapshot(&session.snapshot().root, identity).map(|entry| entry.id)
}

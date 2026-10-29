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

/// One child entry of a folder with its identity.
fn child_entry(snapshot: &oak_engine_rust_sdk::editor::EntrySnapshot) -> ProjectEntry {
	ProjectEntry::new(
		snapshot.id.raw(),
		snapshot.name.clone(),
		snapshot.kind == Some(oak_engine_rust_sdk::node::NodeKind::Folder),
	)
}

/// The children of a folder node.
fn folder_children(snapshot: &oak_engine_rust_sdk::editor::EntrySnapshot) -> Vec<ProjectEntry> {
	snapshot
		.children
		.iter()
		.map(child_entry)
		.collect()
}

fn find_snapshot<'a>(
	entry: &'a oak_engine_rust_sdk::editor::EntrySnapshot,
	identity: u64,
) -> Option<&'a oak_engine_rust_sdk::editor::EntrySnapshot> {
	if entry.id.raw() == identity {
		return Some(entry);
	}
	entry
		.children
		.iter()
		.find_map(|child| find_snapshot(child, identity))
}

/// The top-level entries: the project root folder's children.
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
		.filter(|entry| entry.kind == Some(oak_engine_rust_sdk::node::NodeKind::Folder))
		.map(folder_children)
		.unwrap_or_default()
}

/// The project node with `identity` (validated against the graph), or
/// `None`.
pub fn find_by_identity(
	project: &ProjectRef,
	identity: u64,
) -> Option<oak_engine_rust_sdk::node::NodeId> {
	let session = super::real::editor_session_for_project(project).ok()?;
	find_snapshot(&session.snapshot().root, identity).map(|entry| entry.id)
}

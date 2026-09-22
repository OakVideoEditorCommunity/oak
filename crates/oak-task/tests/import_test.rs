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

//! Boundary tests for `ProjectImportTask` (media import): valid and
//! invalid files, directories (recursive folder creation), duplicate
//! paths, the produced undo command, and cancellation.
//!
//! Media comes from `oak_codec::testmedia`, so the tests run headless.

use oak_node::folder::FolderBehavior;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_task::nodeops::{self, ProjectRef};
use oak_task::project::import::ProjectImportTask;
use oak_task::task::{Task, TaskBehavior};

fn work_dir(tag: &str) -> std::path::PathBuf {
	let path = std::env::temp_dir().join(format!(
		"oaktask_import_{tag}_{}",
		std::process::id()
	));
	let _ = std::fs::remove_dir_all(&path);
	std::fs::create_dir_all(&path).expect("work dir");
	path
}

fn write_clip(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
	let path = dir.join(name);
	oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("test clip generation");
	path
}

/// A fresh project with its initialized root folder.
fn project() -> (ProjectRef, NodeId) {
	let project = Project::new();
	let root = {
		let mut p = project.lock().unwrap();
		p.initialize().expect("root folder");
		p.root
	};
	(project, root)
}

fn run_import(
	task: &mut ProjectImportTask,
	driver: &mut Task,
) -> oak_task::error::Result<()> {
	task.run(driver)
}

fn folder_children(project: &ProjectRef, folder: NodeId) -> Vec<NodeId> {
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	guard
		.graph
		.get(folder)
		.and_then(|e| e.behavior.as_any())
		.and_then(|a| a.downcast_ref::<FolderBehavior>())
		.map(|f| f.children.clone())
		.unwrap_or_default()
}

#[test]
fn import_mixes_valid_and_invalid_files() {
	let dir = work_dir("mixed");
	let media = write_clip(&dir, "clip.mp4");
	let missing = dir.join("missing.mp4");

	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		vec![
			media.to_string_lossy().into_owned(),
			missing.to_string_lossy().into_owned(),
		],
		None,
		2,
	);
	let mut driver = Task::new("Import", None);
	assert!(run_import(&mut task, &mut driver).is_ok());

	assert_eq!(task.file_count(), 2);
	assert_eq!(task.get_file_count(), 1, "one footage imported");
	assert_eq!(task.get_invalid_file_count(), 1, "one file failed");
	assert!(task.has_invalid_files());
	assert!(!task.get_imported_footage(0).is_err());
	assert!(task.get_imported_footage(1).is_err(), "out of range");

	let (project_ref, footage) = task.get_imported_footage(0).expect("footage");
	assert_eq!(nodeops::footage_filename(&project_ref, footage), media.to_string_lossy());
	assert!(nodeops::footage_is_valid(&project_ref, footage));

	// The task's command adds the footage to the destination folder; undo
	// removes it again.
	let mut cmd = task.take_command().expect("command produced");
	assert!(folder_children(&project, root).is_empty());
	cmd.redo_now();
	assert_eq!(folder_children(&project, root), vec![footage]);
	cmd.undo_now();
	assert!(folder_children(&project, root).is_empty());

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_duplicate_paths_create_one_footage_each() {
	let dir = work_dir("dupes");
	let media = write_clip(&dir, "clip.mp4");
	let path = media.to_string_lossy().into_owned();

	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		vec![path.clone(), path],
		None,
		2,
	);
	let mut driver = Task::new("Import", None);
	assert!(run_import(&mut task, &mut driver).is_ok());
	assert_eq!(task.get_file_count(), 2, "each entry imports");
	assert_eq!(task.get_invalid_file_count(), 0);

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_directory_creates_a_labeled_folder_recursively() {
	let dir = work_dir("directory");
	let sub = dir.join("Shots");
	std::fs::create_dir_all(&sub).expect("subdir");
	write_clip(&sub, "a.mp4");
	write_clip(&sub, "b.mp4");

	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		vec![sub.to_string_lossy().into_owned()],
		None,
		2,
	);
	let mut driver = Task::new("Import", None);
	assert!(run_import(&mut task, &mut driver).is_ok());
	assert_eq!(task.get_file_count(), 2, "directory children imported");
	assert_eq!(task.get_invalid_file_count(), 0);

	// The directory became a labeled sub-folder holding both footages.
	let mut cmd = task.take_command().expect("command produced");
	cmd.redo_now();
	let children = folder_children(&project, root);
	assert_eq!(children.len(), 1, "one folder added");
	let sub_folder = children[0];
	assert_eq!(nodeops::node_label(&project, sub_folder), "Shots");
	assert_eq!(folder_children(&project, sub_folder).len(), 2);

	cmd.undo_now();
	assert!(folder_children(&project, root).is_empty());

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_empty_directory_imports_nothing() {
	let dir = work_dir("empty");
	let empty = dir.join("Nothing");
	std::fs::create_dir_all(&empty).expect("empty dir");

	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		vec![empty.to_string_lossy().into_owned()],
		None,
		0,
	);
	let mut driver = Task::new("Import", None);
	assert!(run_import(&mut task, &mut driver).is_ok());
	assert_eq!(task.get_file_count(), 0);
	assert_eq!(task.get_invalid_file_count(), 0);
	assert!(task.take_command().is_ok(), "an (empty) command is still produced");

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_cancel_reports_cancelled_and_yields_no_command() {
	let dir = work_dir("cancel");
	let media = write_clip(&dir, "clip.mp4");

	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		vec![media.to_string_lossy().into_owned()],
		None,
		1,
	);
	let mut driver = Task::new("Import", None);
	driver.cancel();
	let result = run_import(&mut task, &mut driver);
	assert!(
		matches!(result, Err(oak_task::error::Error::Cancelled)),
		"a cancelled import must fail with Cancelled: {result:?}"
	);
	assert!(task.take_command().is_err(), "no command after cancellation");

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn take_command_before_a_run_reports_state() {
	let (project, root) = project();
	let mut task = ProjectImportTask::new(
		Task::new("Import", None),
		(project.clone(), root),
		project.clone(),
		Vec::new(),
		None,
		0,
	);
	assert!(task.take_command().is_err());
}

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

//! Boundary tests for the project load path:
//!
//! - `LoadOTIOTask` (`.otio` / `.fcpxml`) round-trips a saved project
//!   through the real interchange files.
//! - `ProjectLoadTask` (the XML `.oakproj` path) handles valid, missing
//!   and corrupt documents.
//! - Error contract: missing file, corrupt document, unsupported
//!   extension, `take_project` before a run, and the import-confirmation
//!   callback (accept / reject / global reset).
//!
//! The interchange save path is the production `SaveOTIOTask`; the media
//! is a generated test clip, so the tests run headless with no fixtures.

use std::sync::{Arc, Mutex};

use oak_core::{Rational, TimeRange};
use oak_node::block::ClipBlockBehavior;
use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;
use oak_node::node::NodeCore;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior, TrackType};
use oak_task::nodeops;
use oak_task::project::load::{ProjectLoadBaseTask, ProjectLoadTask};
use oak_task::project::loadotio::{set_import_confirm_callback, LoadOTIOTask};
use oak_task::project::saveotio::SaveOTIOTask;
use oak_task::task::{Task, TaskBehavior};

/// Tests that install the process-wide import-confirmation callback must
/// not race each other.
static CALLBACK_LOCK: Mutex<()> = Mutex::new(());

fn work_path(tag: &str, ext: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!("oaktask_load_{tag}_{}.{ext}", std::process::id()))
}

fn write_clip(tag: &str) -> std::path::PathBuf {
	let path = work_path(tag, "mp4");
	oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("test clip generation");
	path
}

/// One video track carrying one clip of `footage` covering `[in, out)`.
fn add_track_with_clip(
	p: &mut Project,
	footage: NodeId,
	kind: TrackType,
	in_: Rational,
	out: Rational,
) -> NodeId {
	let (tcore, tbehavior) = TrackBehavior::create();
	let track = p.graph.add_node(tcore, tbehavior);
	p.graph
		.get_mut(track)
		.unwrap()
		.behavior
		.as_any_mut()
		.unwrap()
		.downcast_mut::<TrackBehavior>()
		.unwrap()
		.kind = kind;

	let (ccore, cbehavior) = oak_node::block::clip_create();
	let clip = p.graph.add_node(ccore, cbehavior);
	p.graph
		.connect(footage, clip, oak_node::block::clip_input::TEXTURE_INPUT, -1)
		.expect("connect footage to clip");
	p.graph
		.get_mut(clip)
		.unwrap()
		.behavior
		.as_any_mut()
		.unwrap()
		.downcast_mut::<ClipBlockBehavior>()
		.expect("clip block")
		.core
		.range = TimeRange::new(in_, out);
	p.graph
		.get_mut(track)
		.unwrap()
		.behavior
		.as_any_mut()
		.unwrap()
		.downcast_mut::<TrackBehavior>()
		.unwrap()
		.append_block(clip);
	track
}

/// A project with one "Edited" sequence (one video track with one 1s clip,
/// one audio track with the same clip) rooted in the bin folder.
fn build_project(media: &str) -> Arc<Mutex<Project>> {
	let project = Project::new();
	{
		let mut p = project.lock().unwrap();
		p.initialize().expect("root folder");

		let footage = {
			let mut f = FootageBehavior::new(media);
			f.probe().expect("probe the generated clip");
			p.graph.add_node(NodeCore::new(), Box::new(f))
		};

		let (score, sbehavior) = SequenceBehavior::create();
		let edited = p.graph.add_node(score, sbehavior);
		{
			let video_track = add_track_with_clip(
				&mut p,
				footage,
				TrackType::Video,
				Rational::new(0, 1),
				Rational::new(1, 1),
			);
			let audio_track = add_track_with_clip(
				&mut p,
				footage,
				TrackType::Audio,
				Rational::new(0, 1),
				Rational::new(1, 1),
			);
			for (kind, tracks) in [
				(TrackType::Video, vec![video_track]),
				(TrackType::Audio, vec![audio_track]),
			] {
				let (lcore, lbehavior) = TrackListBehavior::create();
				let list = p.graph.add_node(lcore, lbehavior);
				{
					let list_b = p
						.graph
						.get_mut(list)
						.unwrap()
						.behavior
						.as_any_mut()
						.unwrap()
						.downcast_mut::<TrackListBehavior>()
						.unwrap();
					list_b.kind = kind;
					list_b.tracks = tracks;
				}
				p.graph
					.get_mut(edited)
					.unwrap()
					.behavior
					.as_any_mut()
					.unwrap()
					.downcast_mut::<SequenceBehavior>()
					.unwrap()
					.track_lists
					.push(list);
			}
			p.graph.get_mut(edited).unwrap().core.label = "Edited".to_string();
		}

		let root = p.root;
		p.graph
			.get_mut(root)
			.unwrap()
			.behavior
			.as_any_mut()
			.unwrap()
			.downcast_mut::<oak_node::folder::FolderBehavior>()
			.expect("root folder")
			.children
			.push(edited);
	}
	project
}

/// Save `project` through the production interchange task.
fn save(project: &Arc<Mutex<Project>>, filename: &std::path::Path) {
	let mut driver = Task::new("Saving project...", None);
	driver.set_behavior(Box::new(SaveOTIOTask {
		base: Task::new("Saving project...", None),
		project: project.clone(),
		filename: filename.to_string_lossy().into_owned(),
	}));
	if let Err(e) = driver.start() {
		panic!(
			"save to {} failed: {e:?} / {}",
			filename.display(),
			driver.error().unwrap_or("unknown error")
		);
	}
}

/// Run the OTIO loader on `filename`; `(task error, stored project)`.
fn load_otio(filename: &std::path::Path) -> (Option<String>, Option<Arc<Mutex<Project>>>) {
	let mut task = Task::new("Loading project...", None);
	let mut loader = LoadOTIOTask::new(ProjectLoadBaseTask::new(
		Task::new("Loading project...", None),
		filename.to_string_lossy().into_owned(),
	));
	let result = loader.run(&mut task);
	let error = if result.is_err() {
		Some(task.error().unwrap_or("unknown error").to_string())
	} else {
		None
	};
	(error, loader.base.take_project().ok())
}

/// The labels of every sequence in `project`.
fn sequence_labels(project: &Arc<Mutex<Project>>) -> Vec<String> {
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	let mut labels = Vec::new();
	for id in guard.graph.node_ids() {
		if let Some(entry) = guard.graph.get(id) {
			if entry
				.behavior
				.as_any()
				.and_then(|a| a.downcast_ref::<SequenceBehavior>())
				.is_some()
			{
				labels.push(entry.core.label.clone());
			}
		}
	}
	labels
}

// ---- OTIO / FCPXML round trip -----------------------------------------

#[test]
fn otio_round_trip_preserves_the_sequence() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let media = write_clip("otio");
	let project = build_project(&media.to_string_lossy());
	let out = work_path("roundtrip", "otio");
	let _ = std::fs::remove_file(&out);
	save(&project, &out);

	let (error, loaded) = load_otio(&out);
	assert_eq!(error, None, "the saved document must load");
	let loaded = loaded.expect("loaded project");
	assert_eq!(sequence_labels(&loaded), vec!["Edited".to_string()]);

	// The sequence carries the saved tracks and clips.
	let (sequence, _) = {
		let guard = loaded.lock().unwrap();
		let mut found = None;
		for id in guard.graph.node_ids() {
			if guard
				.graph
				.get(id)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<SequenceBehavior>())
				.is_some()
			{
				found = Some(id);
				break;
			}
		}
		(found.expect("sequence"), ())
	};
	let video_list =
		nodeops::sequence_track_list(&loaded, sequence, TrackType::Video).expect("video list");
	let audio_list =
		nodeops::sequence_track_list(&loaded, sequence, TrackType::Audio).expect("audio list");
	assert_eq!(nodeops::tracklist_track_count(&loaded, video_list), 1);
	assert_eq!(nodeops::tracklist_track_count(&loaded, audio_list), 1);

	let track = nodeops::tracklist_track_at(&loaded, video_list, 0).expect("video track");
	assert_eq!(nodeops::track_block_count(&loaded, track), 1);
	let clip = nodeops::track_block_at(&loaded, track, 0).expect("clip");
	assert_eq!(nodeops::block_in(&loaded, clip), Rational::new(0, 1));
	assert_eq!(nodeops::block_length(&loaded, clip), Rational::new(1, 1));
	let footage = nodeops::clip_footage(&loaded, clip).expect("clip footage link");
	assert_eq!(
		nodeops::footage_filename(&loaded, footage),
		media.to_string_lossy()
	);

	let _ = std::fs::remove_file(&out);
	let _ = std::fs::remove_file(&media);
}

#[test]
fn fcpxml_round_trip_preserves_the_sequence() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let media = write_clip("fcpxml");
	let project = build_project(&media.to_string_lossy());
	let out = work_path("roundtrip", "fcpxml");
	let _ = std::fs::remove_file(&out);
	save(&project, &out);

	let (error, loaded) = load_otio(&out);
	assert_eq!(error, None, "the saved FCPXML must load");
	let loaded = loaded.expect("loaded project");
	assert_eq!(sequence_labels(&loaded), vec!["Edited".to_string()]);

	// The sequence carries the saved tracks and clips (the same structure
	// the OTIO round trip asserts, through the FCPXML mapping).
	let sequence = {
		let guard = loaded.lock().unwrap();
		let mut found = None;
		for id in guard.graph.node_ids() {
			if guard
				.graph
				.get(id)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<SequenceBehavior>())
				.is_some()
			{
				found = Some(id);
				break;
			}
		}
		found.expect("sequence")
	};
	let video_list =
		nodeops::sequence_track_list(&loaded, sequence, TrackType::Video).expect("video list");
	let audio_list =
		nodeops::sequence_track_list(&loaded, sequence, TrackType::Audio).expect("audio list");
	assert_eq!(nodeops::tracklist_track_count(&loaded, video_list), 1);
	assert_eq!(nodeops::tracklist_track_count(&loaded, audio_list), 1);

	let track = nodeops::tracklist_track_at(&loaded, video_list, 0).expect("video track");
	assert_eq!(nodeops::track_block_count(&loaded, track), 1);
	let clip = nodeops::track_block_at(&loaded, track, 0).expect("clip");
	assert_eq!(nodeops::block_in(&loaded, clip), Rational::new(0, 1));
	assert_eq!(nodeops::block_length(&loaded, clip), Rational::new(1, 1));
	let footage = nodeops::clip_footage(&loaded, clip).expect("clip footage link");
	assert_eq!(
		nodeops::footage_filename(&loaded, footage),
		media.to_string_lossy()
	);

	let _ = std::fs::remove_file(&out);
	let _ = std::fs::remove_file(&media);
}

// ---- error contract ---------------------------------------------------

#[test]
fn load_otio_rejects_a_missing_file() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let missing = work_path("missing", "otio");
	let (error, loaded) = load_otio(&missing);
	assert!(error.is_some(), "a missing document must fail");
	assert!(loaded.is_none(), "no project is stored on failure");
}

#[test]
fn load_otio_rejects_a_corrupt_document() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let path = work_path("corrupt", "otio");
	std::fs::write(&path, b"{ this is not json").expect("write corrupt file");
	let (error, loaded) = load_otio(&path);
	assert!(error.is_some(), "a corrupt document must fail");
	assert!(loaded.is_none());
	let _ = std::fs::remove_file(&path);
}

#[test]
fn load_otio_rejects_an_unknown_extension() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let path = work_path("unknown", "txt");
	std::fs::write(&path, b"{}").expect("write file");
	let (error, loaded) = load_otio(&path);
	let error = error.expect("unknown extension must fail");
	assert!(
		error.contains("Unknown project file format"),
		"error names the supported formats: {error}"
	);
	assert!(loaded.is_none());
	let _ = std::fs::remove_file(&path);
}

#[test]
fn take_project_before_a_run_reports_state() {
	let mut base = ProjectLoadBaseTask::new(Task::new("load", None), "x.oakproj".into());
	assert!(base.take_project().is_err());
}

#[test]
fn xml_loader_rejects_missing_and_corrupt_documents() {
	let mut task = Task::new("load", None);
	let mut load = ProjectLoadTask {
		base: ProjectLoadBaseTask::new(Task::new("load", None), "/definitely/not/here.oakproj".into()),
	};
	assert!(load.run(&mut task).is_err());
	assert!(task
		.error()
		.is_some_and(|e| e.contains("Failed to read file")));

	let path = work_path("corrupt", "oakproj");
	std::fs::write(&path, b"<not-oak>").expect("write corrupt xml");
	let mut task = Task::new("load", None);
	let mut load = ProjectLoadTask {
		base: ProjectLoadBaseTask::new(
			Task::new("load", None),
			path.to_string_lossy().into_owned(),
		),
	};
	assert!(load.run(&mut task).is_err());
	assert!(task
		.error()
		.is_some_and(|e| e.contains("Failed to read XML document")));
	let _ = std::fs::remove_file(&path);
}

#[test]
fn xml_loader_round_trips_a_saved_project() {
	let media = write_clip("oakproj");
	let project = build_project(&media.to_string_lossy());
	let xml = {
		let guard = project.lock().unwrap();
		oak_node::serializer::save(&guard).expect("serialize project")
	};
	let path = work_path("roundtrip", "oakproj");
	std::fs::write(&path, xml).expect("write project");

	let mut task = Task::new("load", None);
	let mut load = ProjectLoadTask {
		base: ProjectLoadBaseTask::new(
			Task::new("load", None),
			path.to_string_lossy().into_owned(),
		),
	};
	assert!(load.run(&mut task).is_ok(), "error: {:?}", task.error());
	let loaded = load.base.take_project().expect("loaded project");
	assert_eq!(sequence_labels(&loaded), vec!["Edited".to_string()]);
	{
		let guard = loaded.lock().unwrap();
		assert_eq!(guard.filename(), path.to_string_lossy());
	}

	let _ = std::fs::remove_file(&path);
	let _ = std::fs::remove_file(&media);
}

// ---- import-confirmation callback -------------------------------------

#[test]
fn import_confirm_callback_can_reject_and_accept() {
	let _guard = CALLBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let media = write_clip("confirm");
	let project = build_project(&media.to_string_lossy());
	let out = work_path("confirm", "otio");
	let _ = std::fs::remove_file(&out);
	save(&project, &out);

	// Rejecting: the task completes but hands no project to the caller.
	let rejected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
	let seen = rejected.clone();
	set_import_confirm_callback(Some(Box::new(move |names| {
		*seen.lock().unwrap() = names.to_vec();
		false
	})));
	let (error, loaded) = load_otio(&out);
	assert_eq!(error, None, "a user rejection is not a task error");
	assert!(loaded.is_none(), "the rejected project is disposed of");
	assert_eq!(*rejected.lock().unwrap(), vec!["Edited".to_string()]);

	// Accepting: the same document loads normally.
	set_import_confirm_callback(Some(Box::new(|_| true)));
	let (error, loaded) = load_otio(&out);
	assert_eq!(error, None);
	assert!(loaded.is_some());

	// Reset the global callback so other tests see the headless default.
	set_import_confirm_callback(None);

	let _ = std::fs::remove_file(&out);
	let _ = std::fs::remove_file(&media);
}

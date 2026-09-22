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

//! Boundary tests for the `.otio` / `.fcpxml` storage backend
//! (`oak_storage::backends::otio`): metadata, URI/extension validation,
//! a rich multi-sequence round trip (collection root, gap, transition,
//! missing media reference), the empty-project shape, and load errors.

use std::sync::Arc;
use std::sync::Mutex;

use oak_core::{Rational, TimeRange};
use oak_node::block::{ClipBlockBehavior, TransitionBlockBehavior};
use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;
use oak_node::node::NodeCore;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior, TrackType};
use oak_storage::backend::{LoadResult, StorageBackend};
use oak_storage::backends::otio::OtioBackend;
use oak_storage::nodeutil::{make_project_owned, project_arc};
use oak_storage::uri::StorageUri;

fn work_path(tag: &str, ext: &str) -> std::path::PathBuf {
	let path = std::env::temp_dir().join(format!(
		"oakstorage_otio_{tag}_{}.{ext}",
		std::process::id()
	));
	let _ = std::fs::remove_file(&path);
	path
}

fn file_uri(path: &std::path::Path) -> StorageUri {
	StorageUri::parse(&format!("file://{}", path.display())).expect("file uri")
}

/// A synthetic media path: the OTIO backend only carries the filename, it
/// never opens/probes the media, so no real file is needed.
fn media_path(tag: &str) -> std::path::PathBuf {
	work_path(tag, "mp4")
}

/// One clip covering `[in, out)` (media seconds), optionally linked to
/// `footage` both as the domain field and as a graph edge.
fn add_clip(
	p: &mut Project,
	track: NodeId,
	footage: Option<NodeId>,
	in_: Rational,
	out: Rational,
) -> NodeId {
	let (core, mut behavior) = oak_node::block::clip_create();
	let block = {
		let clip = behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
			.expect("clip behavior");
		clip.core.range = TimeRange::new(in_, out);
		clip.core.media_in = Rational::new(0, 1);
		clip.core.track = Some(track);
		clip.footage = footage;
		let id = p.graph.add_node(core, behavior);
		if let Some(footage) = footage {
			p.graph
				.connect(
					footage,
					id,
					oak_node::block::clip_input::TEXTURE_INPUT,
					-1,
				)
				.expect("connect footage");
		}
		id
	};
	if let Some(entry) = p.graph.get_mut(track) {
		if let Some(t) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TrackBehavior>())
		{
			t.blocks.push(block);
		}
	}
	block
}

fn add_gap(p: &mut Project, track: NodeId, in_: Rational, out: Rational) -> NodeId {
	let (core, mut behavior) = oak_node::block::gap_create();
	let id = {
		let gap = behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<oak_node::block::GapBlockBehavior>())
			.expect("gap behavior");
		gap.core.range = TimeRange::new(in_, out);
		gap.core.track = Some(track);
		p.graph.add_node(core, behavior)
	};
	if let Some(entry) = p.graph.get_mut(track) {
		if let Some(t) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TrackBehavior>())
		{
			t.blocks.push(id);
		}
	}
	id
}

fn add_transition(
	p: &mut Project,
	track: NodeId,
	in_offset: Rational,
	out_offset: Rational,
) -> NodeId {
	let (core, mut behavior) = oak_node::block::transition_create();
	let id = {
		let t = behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TransitionBlockBehavior>())
			.expect("transition behavior");
		t.core.range = TimeRange::new(Rational::new(3, 1), Rational::new(4, 1));
		t.core.track = Some(track);
		t.in_offset = in_offset;
		t.out_offset = out_offset;
		p.graph.add_node(core, behavior)
	};
	if let Some(entry) = p.graph.get_mut(track) {
		if let Some(t) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TrackBehavior>())
		{
			t.blocks.push(id);
		}
	}
	id
}

fn add_track(p: &mut Project, kind: TrackType) -> NodeId {
	let (core, behavior) = TrackBehavior::create();
	let track = p.graph.add_node(core, behavior);
	if let Some(entry) = p.graph.get_mut(track) {
		if let Some(t) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TrackBehavior>())
		{
			t.kind = kind;
		}
	}
	track
}

fn attach_list(p: &mut Project, sequence: NodeId, kind: TrackType, tracks: Vec<NodeId>) -> NodeId {
	let (core, behavior) = TrackListBehavior::create();
	let list = p.graph.add_node(core, behavior);
	if let Some(entry) = p.graph.get_mut(list) {
		if let Some(l) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<TrackListBehavior>())
		{
			l.kind = kind;
			l.tracks = tracks;
		}
	}
	if let Some(entry) = p.graph.get_mut(sequence) {
		if let Some(s) = entry
			.behavior
			.as_any_mut()
			.and_then(|a| a.downcast_mut::<SequenceBehavior>())
		{
			s.track_lists.push(list);
		}
	}
	list
}

/// A project with `count` sequences named "Seq0".."SeqN"; the first has a
/// video track (clip, explicit gap, transition, footage-less clip) and an
/// audio track.
fn build_project(media: Option<&str>, count: usize) -> Arc<Mutex<Project>> {
	let project = Project::new();
	{
		let mut p = project.lock().unwrap();
		p.initialize().expect("root folder");

		let footage =
			media.map(|media| p.graph.add_node(NodeCore::new(), Box::new(FootageBehavior::new(media))));

		for index in 0..count {
			let (core, behavior) = SequenceBehavior::create();
			let seq = p.graph.add_node(core, behavior);
			if let Some(entry) = p.graph.get_mut(seq) {
				entry.core.label = format!("Seq{index}");
			}
			if index == 0 {
				let video = add_track(&mut p, TrackType::Video);
				add_clip(&mut p, video, footage, Rational::new(0, 1), Rational::new(1, 1));
				// A discontinuity before this block exercises the implicit-gap
				// export path (block in 2 > position 1).
				add_gap(&mut p, video, Rational::new(2, 1), Rational::new(3, 1));
				add_transition(
					&mut p,
					video,
					Rational::new(1, 2),
					Rational::new(1, 4),
				);
				// No footage and no edge: exports a MissingReference.
				add_clip(
					&mut p,
					video,
					None,
					Rational::new(4, 1),
					Rational::new(5, 1),
				);
				attach_list(&mut p, seq, TrackType::Video, vec![video]);

				let audio = add_track(&mut p, TrackType::Audio);
				add_clip(&mut p, audio, footage, Rational::new(0, 1), Rational::new(1, 1));
				attach_list(&mut p, seq, TrackType::Audio, vec![audio]);
			}
		}
	}
	project
}

struct Loaded {
	project: Arc<Mutex<Project>>,
	result: LoadResult,
}

impl Drop for Loaded {
	fn drop(&mut self) {
		if let Some(release) = self.result.project.release {
			// SAFETY: the load result is an owned handle (refcount 1).
			unsafe { release(self.result.project.ctx) };
		}
	}
}

fn load(uri: &StorageUri) -> oak_storage::error::Result<Loaded> {
	let backend = OtioBackend::new();
	let result = backend.load(uri)?;
	let arc = unsafe { project_arc(&result.project)? };
	Ok(Loaded { project: arc, result })
}

/// Sequence labels present in `project`.
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
	labels.sort();
	labels
}

#[test]
fn backend_metadata_and_can_handle() {
	let backend = OtioBackend::new();
	assert_eq!(backend.name(), "otio");
	assert_eq!(backend.uri_scheme(), "file");
	assert!(backend.can_handle(&file_uri(std::path::Path::new("/tmp/a.otio"))));
	assert!(backend.can_handle(&file_uri(std::path::Path::new("/tmp/a.fcpxml"))));
	assert!(!backend.can_handle(&file_uri(std::path::Path::new("/tmp/a.ove"))));
	assert!(!backend.can_handle(&file_uri(std::path::Path::new("/tmp/a"))));
}

#[test]
fn save_rejects_bad_uri_and_extension() {
	let project = build_project(None, 0);
	let backend = OtioBackend::new();

	// Unsupported extension.
	let uri = file_uri(&work_path("bad", "ove"));
	assert!(backend.save_project(&project, &uri, 0).is_err());
	// No extension at all.
	let uri = StorageUri::parse("file:///tmp/noext").expect("uri");
	assert!(backend.save_project(&project, &uri, 0).is_err());
	// Non-local URI (the otio backend only writes files).
	let uri = StorageUri::parse("oakdb+sqlite:///tmp/x.otio").expect("uri");
	assert!(backend.save_project(&project, &uri, 0).is_err());

	// The facade `save` (handle in, path validated the same way).
	let handle = make_project_owned(project);
	let uri = file_uri(&work_path("bad", "ove"));
	assert!(StorageBackend::save(&backend, handle, &uri, 0).is_err());
	if let Some(release) = handle.release {
		unsafe { release(handle.ctx) };
	}
}

#[test]
fn otio_collection_round_trip_preserves_sequences_tracks_and_blocks() {
	let media = media_path("rich");
	let project = build_project(Some(&media.to_string_lossy()), 2);
	let out = work_path("rich", "otio");

	let backend = OtioBackend::new();
	let handle = make_project_owned(project.clone());
	backend
		.save(handle, &file_uri(&out), 0)
		.expect("save through the facade");
	if let Some(release) = handle.release {
		unsafe { release(handle.ctx) };
	}

	let loaded = load(&file_uri(&out)).expect("load");
	assert_eq!(loaded.result.version_info, oak_storage::error::OAKSTORAGE_OK);
	assert_eq!(
		sequence_labels(&loaded.project),
		vec!["Seq0".to_string(), "Seq1".to_string()]
	);

	// The first sequence carries both track lists, and its video track has
	// the four exported composables (clip, gap, transition, clip).
	let guard = loaded.project.lock().unwrap();
	let mut tracks = 0;
	let mut clips = 0;
	let mut transitions = 0;
	let mut footage_links = 0;
	let mut missing_links = 0;
	for id in guard.graph.node_ids() {
		let Some(entry) = guard.graph.get(id) else {
			continue;
		};
		let Some(any) = entry.behavior.as_any() else {
			continue;
		};
		if let Some(t) = any.downcast_ref::<TrackBehavior>() {
			// clip, an implicit gap for the discontinuity, the explicit gap,
			// the transition and the footage-less clip.
			if t.blocks.len() == 5 {
				tracks += 1;
			}
		}
		if let Some(c) = any.downcast_ref::<ClipBlockBehavior>() {
			clips += 1;
			if c.footage.is_some() {
				footage_links += 1;
			} else {
				missing_links += 1;
			}
		}
		if let Some(t) = any.downcast_ref::<TransitionBlockBehavior>() {
			transitions += 1;
			assert_eq!(t.in_offset, Rational::new(1, 2));
			assert_eq!(t.out_offset, Rational::new(1, 4));
		}
	}
	assert_eq!(tracks, 1, "the rich video track");
	assert_eq!(clips, 3, "two linked clips + one missing-reference clip");
	assert_eq!(footage_links, 2);
	assert_eq!(missing_links, 1);
	assert_eq!(transitions, 1);

	drop(guard);
	let _ = std::fs::remove_file(&out);
}

#[test]
fn fcpxml_round_trip_preserves_sequences_tracks_and_blocks() {
	let media = media_path("fcpxml");
	let project = build_project(Some(&media.to_string_lossy()), 1);
	let out = work_path("fcpxml", "fcpxml");

	let backend = OtioBackend::new();
	backend
		.save_project(&project, &file_uri(&out), 0)
		.expect("save fcpxml");
	let loaded = load(&file_uri(&out)).expect("load fcpxml");
	assert_eq!(sequence_labels(&loaded.project), vec!["Seq0".to_string()]);

	// The sequence carries both track lists, and its video track has the
	// exported composables (clip, implicit gap, explicit gap, transition,
	// footage-less clip) — the FCPXML writer places blocks sequentially.
	let guard = loaded.project.lock().unwrap();
	let mut video_tracks = 0;
	let mut audio_tracks = 0;
	let mut clips = 0;
	let mut transitions = 0;
	let mut footage_links = 0;
	let mut missing_links = 0;
	for id in guard.graph.node_ids() {
		let Some(entry) = guard.graph.get(id) else {
			continue;
		};
		let Some(any) = entry.behavior.as_any() else {
			continue;
		};
		if let Some(t) = any.downcast_ref::<TrackBehavior>() {
			if t.kind == TrackType::Video && t.blocks.len() == 5 {
				video_tracks += 1;
			}
			if t.kind == TrackType::Audio && t.blocks.len() == 1 {
				audio_tracks += 1;
			}
		}
		if let Some(c) = any.downcast_ref::<ClipBlockBehavior>() {
			clips += 1;
			if c.footage.is_some() {
				footage_links += 1;
			} else {
				missing_links += 1;
			}
		}
		if let Some(t) = any.downcast_ref::<TransitionBlockBehavior>() {
			transitions += 1;
			// FCP X centers transitions on import: each offset is half the
			// exported duration (1/2 + 1/4 seconds).
			assert_eq!(t.in_offset, Rational::new(3, 8));
			assert_eq!(t.out_offset, Rational::new(3, 8));
		}
	}
	assert_eq!(video_tracks, 1, "the rich video track");
	assert_eq!(audio_tracks, 1, "the audio track");
	assert_eq!(clips, 3, "two linked clips + one missing-reference clip");
	assert_eq!(footage_links, 2);
	assert_eq!(missing_links, 1);
	assert_eq!(transitions, 1);

	drop(guard);
	let _ = std::fs::remove_file(&out);
}

#[test]
fn empty_project_exports_one_empty_timeline() {
	let project = build_project(None, 0);
	let out = work_path("empty", "otio");
	let backend = OtioBackend::new();
	backend
		.save_project(&project, &file_uri(&out), 0)
		.expect("save empty project");

	let loaded = load(&file_uri(&out)).expect("load empty project");
	// The export names the placeholder "Timeline"; the import carries it.
	assert_eq!(sequence_labels(&loaded.project), vec!["Timeline".to_string()]);
	let _ = std::fs::remove_file(&out);
}

#[test]
fn load_reports_missing_corrupt_and_unrooted_documents() {
	let backend = OtioBackend::new();

	// Missing file.
	let missing = work_path("missing", "otio");
	assert!(backend.load(&file_uri(&missing)).is_err());

	// Malformed JSON.
	let corrupt = work_path("corrupt", "otio");
	std::fs::write(&corrupt, b"{ not json").expect("write");
	assert!(backend.load(&file_uri(&corrupt)).is_err());
	let _ = std::fs::remove_file(&corrupt);

	// Valid JSON whose root is neither a timeline nor a collection.
	let raw = work_path("raw", "otio");
	std::fs::write(&raw, br#"{"OTIO_SCHEMA":"RationalTime.1","value":1,"rate":24}"#)
		.expect("write");
	let err = backend.load(&file_uri(&raw)).expect_err("raw root must fail");
	assert!(
		format!("{err}").contains("no timeline or collection root"),
		"{err}"
	);
	let _ = std::fs::remove_file(&raw);

	// Missing FCPXML and no-extension URIs.
	let missing_xml = work_path("missing", "fcpxml");
	assert!(backend.load(&file_uri(&missing_xml)).is_err());
	let no_ext = StorageUri::parse("file:///tmp/noext").expect("uri");
	assert!(backend.load(&no_ext).is_err());
}

//! End-to-end SDK coverage for semantic edits, native ABI conversion and history.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::editor::{EditorSession, TrackKind};
use crate::node::{NodeId, NodeValue};
use crate::state::Config;
use crate::types::{Rational, TimeRange};

use super::{EditCommand, EditResult};

static NEXT_MEDIA: AtomicUsize = AtomicUsize::new(0);

/// A host sequence with two contiguous clips and a separate nestable source.
struct Fixture {
	session: EditorSession,
	footage: NodeId,
	first: NodeId,
	second: NodeId,
	source: NodeId,
	media: String,
}

/// Creates isolated media and session state shared by one test scenario.
fn fixture() -> Fixture {
	Config::global().set("Storage/Backend", "off").expect("disable library");
	let session = EditorSession::new().expect("session");
	let source = session.create_sequence("Source").expect("source");
	session.create_sequence("Host").expect("host");
	let media = crate::test_support::make_test_clip(&format!(
		"semantic_{}",
		NEXT_MEDIA.fetch_add(1, Ordering::Relaxed)
	));
	let footage = session.project().import_footage(&media).expect("footage");
	let first = session
		.execute(EditCommand::PlaceFootageClip {
			footage,
			kind: TrackKind::Video,
			track_index: 0,
			in_ts: 0,
			out_ts: 10,
			media_in_ts: 0,
		})
		.expect("place first")
		.primary
		.expect("first id");
	let second = session
		.execute(EditCommand::PlaceFootageClip {
			footage,
			kind: TrackKind::Video,
			track_index: 0,
			in_ts: 10,
			out_ts: 20,
			media_in_ts: 0,
		})
		.expect("place second")
		.primary
		.expect("second id");
	Fixture {
		session,
		footage,
		first,
		second,
		source,
		media,
	}
}

fn ok(session: &EditorSession, command: EditCommand) -> EditResult {
	session.execute(command).expect("semantic operation")
}

fn current_clips(session: &EditorSession) -> usize {
	let snapshot = session.snapshot();
	let current = snapshot.current_sequence.expect("current sequence");
	let sequence = snapshot
		.sequences
		.iter()
		.find(|sequence| sequence.id == current)
		.expect("current sequence snapshot");
	sequence.tracks[0].clips.len()
}

#[test]
fn marker_only_minimal_repro() {
	// Regression: markers use session-owned Arcs, not temporary CHandles.
	let session = EditorSession::new().expect("session");
	ok(&session, EditCommand::MarkerAdd {
		time: Rational::new(1, 25),
		name: "Marker".into(),
		color: 3,
	});
}

#[test]
fn semantic_edit_undo_redo_and_reload() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let fixture = fixture();
	let before = fixture.session.snapshot();
	ok(&fixture.session, EditCommand::SplitClip {
		clip: fixture.first,
		time_ts: 5,
	});
	let after = fixture.session.snapshot();
	assert!(after.revision > before.revision);
	assert_eq!(current_clips(&fixture.session), 3);
	assert!(crate::node::undo());
	assert_eq!(current_clips(&fixture.session), 2);
	assert!(crate::node::redo());
	assert_eq!(current_clips(&fixture.session), 3);

	// Additional edits prove that save/reload preserves the edited timeline,
	// not merely the initial fixture.
	ok(&fixture.session, EditCommand::TrimClip {
		clip: fixture.first,
		new_in_ts: 2,
		new_out_ts: 5,
	});
	ok(&fixture.session, EditCommand::MoveClip {
		clip: fixture.second,
		new_in_ts: 30,
	});
	let edited = fixture.session.snapshot();
	let path = std::env::temp_dir()
		.join(format!("oaksdk_semantic_{}.ove", std::process::id()))
		.to_string_lossy()
		.into_owned();
	fixture.session.save(&path).expect("save edited project");
	let opened = EditorSession::open(&path).expect("reload edited project");
	assert_eq!(opened.snapshot().sequences, edited.sequences);
	let _ = std::fs::remove_file(path);
	let _ = std::fs::remove_file(fixture.media);
}

#[test]
fn all_semantic_operations_use_native_abi() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let f = fixture();
	let sequence = f.session.current_sequence().expect("current");
	let project = f.session.project();
	let list = project.sequence_track_list_at(sequence, 0);
	let track = project.tracklist_track_at(list, 0);

	// This is deliberately a coverage sweep: every EditCommand variant must
	// survive SDK packing, engine decoding and execution through the native ABI.
	assert_eq!(ok(&f.session, EditCommand::AddTrack { kind: TrackKind::Video }).value, 2);
	let added = project.tracklist_track_at(list, 2);
	ok(&f.session, EditCommand::RemoveTrack { track: added });
	ok(&f.session, EditCommand::SetTrackHeight { track, height: 80.0 });
	ok(&f.session, EditCommand::SetTrackMuted { track, muted: true });
	assert!(project.track_muted(track));
	ok(&f.session, EditCommand::SetTrackLocked { track, locked: true });
	assert!(project.track_locked(track));
	ok(&f.session, EditCommand::SetTrackMuted { track, muted: false });
	ok(&f.session, EditCommand::SetTrackLocked { track, locked: false });

	let custom = ok(&f.session, EditCommand::CreateSequence {
		name: "Custom".into(),
		width: Some(64),
		height: Some(32),
		rate: Some(Rational::new(25, 1)),
		interlaced: Some(false),
	});
	let custom_id = custom.primary.expect("custom sequence");
	assert!(f.session.set_current_sequence(custom_id));
	ok(&f.session, EditCommand::SetSequenceParameters {
		sequence: custom_id,
		name: "Custom 2".into(),
		width: 128,
		height: 64,
		rate: Rational::new(50, 1),
		interlaced: true,
	});
	assert_eq!(project.sequence_interlaced(custom_id), Some(true));
	ok(&f.session, EditCommand::SetSequencePlayhead {
		sequence: custom_id,
		time: Rational::new(1, 25),
	});
	assert!(f.session.set_current_sequence(sequence));
	let folder = ok(&f.session, EditCommand::CreateFolder { name: "Bin".into() }).primary.unwrap();
	let text = ok(&f.session, EditCommand::CreateTextFootage).primary.unwrap();
	ok(&f.session, EditCommand::EnsureSequencesMounted);
	ok(&f.session, EditCommand::ReprobeUnprobedFootage);
	ok(&f.session, EditCommand::CreateAdjustmentLayer {
		track: project.tracklist_track_at(list, 1),
		in_ts: 0,
		out_ts: 20,
	});
	ok(&f.session, EditCommand::PlaceNestedSequenceClip {
		source: f.source,
		track_index: 0,
		in_ts: 20,
		out_ts: 25,
	});
	ok(&f.session, EditCommand::PlaceTextClip {
		text,
		track_index: 0,
		in_ts: 30,
		out_ts: 35,
	});
	ok(&f.session, EditCommand::PlaceGeneratorClip {
		generator: text,
		track_index: 0,
		in_ts: 40,
		out_ts: 45,
	});
	let linked = ok(&f.session, EditCommand::PlaceFootageClipsLinked {
		footage: f.footage,
		placements: vec![(TrackKind::Video, 0), (TrackKind::Audio, 0)],
		in_ts: 50,
		out_ts: 60,
		media_in_ts: 0,
	});
	assert_eq!(linked.ids.len(), 2);
	let transition = ok(&f.session, EditCommand::AddTransitionAtSeam {
		previous: f.first,
		next: f.second,
		half: Rational::new(1, 25),
	});
	assert!(transition.primary.is_some());
	ok(&f.session, EditCommand::AddTransitionAtEdge {
		clip: f.first,
		start_edge: true,
		length: Rational::new(1, 25),
	});
	ok(&f.session, EditCommand::AddDefaultTransition {
		clips: vec![f.first, f.second],
		half: Rational::new(1, 25),
	});
	let clamped = ok(&f.session, EditCommand::SetTransitionLength {
		clip: f.first,
		start_edge: false,
		frame: Rational::new(1, 25),
		length: Rational::new(100, 1),
	});
	assert!(clamped.rational.is_some());

	ok(&f.session, EditCommand::SplitClipsPreservingLinks {
		clips: linked.ids.clone(),
		time_ts: 55,
	});
	ok(&f.session, EditCommand::RippleTrimClip {
		clip: f.second,
		start_edge: true,
		new_frame: 12,
	});
	// Independent fixtures keep destructive edits from invalidating IDs needed
	// by later variants in the sweep.
	let roll_fixture = fixture();
	let roll_track = roll_fixture
		.session
		.project()
		.sequence_track_list_at(roll_fixture.session.current_sequence().unwrap(), 0);
	let roll_track = roll_fixture.session.project().tracklist_track_at(roll_track, 0);
	ok(&roll_fixture.session, EditCommand::RollEdit {
		track: roll_track,
		a: roll_fixture.first,
		b: roll_fixture.second,
		new_frame: 10,
	});
	ok(&f.session, EditCommand::SlideClip { clip: f.second, new_start: 14 });
	ok(&f.session, EditCommand::SlipClip { clip: f.second, new_media_in: 1 });
	ok(&f.session, EditCommand::MoveClipToTrack {
		clip: f.second,
		track: project.tracklist_track_at(list, 1),
		new_in_ts: 15,
	});
	ok(&f.session, EditCommand::MoveClipWithLinks {
		clip: f.first,
		track: None,
		new_in_ts: 1,
		linked: vec![f.second],
	});
	ok(&f.session, EditCommand::SetClipsLinked {
		clips: vec![f.first, f.second],
		linked: true,
	});
	ok(&f.session, EditCommand::SetClipsLinked {
		clips: vec![f.first, f.second],
		linked: false,
	});
	ok(&f.session, EditCommand::RollClipEdgeTo {
		clip: f.first,
		at_in: false,
		frame_ts: 12,
	});

	ok(&f.session, EditCommand::MarkerAdd {
		time: Rational::new(1, 25),
		name: "Marker".into(),
		color: 3,
	});
	assert_eq!(f.session.snapshot().markers.len(), 1);
	ok(&f.session, EditCommand::MarkerRemove { time: Rational::new(1, 25) });
	ok(&f.session, EditCommand::WorkareaSet {
		enabled: true,
		range: TimeRange::new(Rational::new(0, 1), Rational::new(2, 1)),
	});
	ok(&f.session, EditCommand::WorkareaSetUndoable {
		enabled: false,
		range: TimeRange::new(Rational::new(1, 1), Rational::new(2, 1)),
		old_range: TimeRange::new(Rational::new(0, 1), Rational::new(2, 1)),
	});
	ok(&f.session, EditCommand::Disconnect {
		to: f.first,
		input_id: "tex_in".into(),
	});
	ok(&f.session, EditCommand::Connect {
		from: f.footage,
		to: f.first,
		input_id: "tex_in".into(),
	});
	ok(&f.session, EditCommand::SetContextPosition {
		node: f.first,
		context: sequence,
		x: 10.0,
		y: 20.0,
	});

	let copied = ok(&f.session, EditCommand::CopyClips { clips: vec![f.first] });
	assert_eq!(copied.clipboard.len(), 1);
	assert_eq!(ok(&f.session, EditCommand::PasteClips { playhead_ts: 70 }).ids.len(), 1);
	ok(&f.session, EditCommand::SetBlocksEnabled {
		clips: vec![f.first],
		enabled: false,
	});
	ok(&f.session, EditCommand::SetBlocksEnabled {
		clips: vec![f.first],
		enabled: true,
	});
	ok(&f.session, EditCommand::SetClipsSpeed { clips: vec![f.first], speed: 2.0 });
	ok(&f.session, EditCommand::RenameBlocks { clips: vec![f.first], name: "Renamed".into() });
	assert_eq!(ok(&f.session, EditCommand::DuplicateClips { clips: vec![f.first] }).ids.len(), 1);
	ok(&f.session, EditCommand::CopyClips { clips: vec![f.first] });
	assert_eq!(ok(&f.session, EditCommand::PasteClipsInsert { playhead_ts: 80 }).ids.len(), 1);
	assert!(ok(&f.session, EditCommand::NestClips { clips: vec![f.first] }).primary.is_some());

	let effect_fixture = fixture();
	let effect_project = effect_fixture.session.project();
	let effect_host = effect_project.create_clip().expect("effect host");
	let effect_type = crate::effect::factory_entries()
		.into_iter()
		.find(|entry| {
			entry.flags & crate::effect::NODE_FLAG_VIDEO_EFFECT != 0
				&& entry.flags & crate::effect::NODE_FLAG_DONT_SHOW_IN_CREATE_MENU == 0
		})
		.expect("video effect")
		.type_id;
	let effect = ok(&effect_fixture.session, EditCommand::EffectInsert {
		host: effect_host,
		index: 0,
		type_id: effect_type.clone(),
	})
	.primary
	.expect("inserted effect");
	let second_effect = ok(&effect_fixture.session, EditCommand::EffectInsert {
		host: effect_host,
		index: 0,
		type_id: effect_type,
	})
	.primary
	.expect("second effect");
	assert_eq!(effect_project.effect_chain(effect_host), vec![second_effect, effect]);
	ok(&effect_fixture.session, EditCommand::EffectMove {
		host: effect_host,
		effect: second_effect,
		new_index: 1,
	});
	assert_eq!(effect_project.effect_chain(effect_host), vec![effect, second_effect]);
	ok(&effect_fixture.session, EditCommand::SetNodeStandardValue {
		node: effect,
		input_id: "enabled_in".into(),
		element: -1,
		value: NodeValue {
			kind: 3,
			num: 0,
			..NodeValue::default()
		},
	});
	assert!(!effect_project.effect_is_enabled(effect));
	ok(&effect_fixture.session, EditCommand::EffectSetEnabled {
		effect,
		enabled: true,
	});
	assert!(effect_project.effect_is_enabled(effect));
	ok(&effect_fixture.session, EditCommand::UpsertNodeKeyframe {
		node: effect,
		input_id: "enabled_in".into(),
		element: -1,
		time: Rational::new(0, 1),
		value: NodeValue {
			kind: 3,
			num: 1,
			..NodeValue::default()
		},
		interpolation: crate::editor::KeyframeInterpolation::Linear,
		bezier_in: (0.0, 0.0),
		bezier_out: (0.0, 0.0),
	});
	ok(&effect_fixture.session, EditCommand::SetNodeValueAtTime {
		node: effect,
		input_id: "enabled_in".into(),
		element: -1,
		time: Rational::new(0, 1),
		value: NodeValue {
			kind: 3,
			num: 1,
			..NodeValue::default()
		},
	});
	ok(&effect_fixture.session, EditCommand::RemoveNodeKeyframe {
		node: effect,
		input_id: "enabled_in".into(),
		element: -1,
		time: Rational::new(0, 1),
	});
	assert!(effect_fixture
		.session
		.execute(EditCommand::PushNodeButton {
			node: effect,
			input_id: "not_a_plugin_button".into(),
		})
		.is_err());
	ok(&effect_fixture.session, EditCommand::EffectRemove {
		host: effect_host,
		effect: second_effect,
	});
	assert_eq!(effect_project.effect_chain(effect_host), vec![effect]);

	ok(&f.session, EditCommand::DeleteArea { in_ts: 90, out_ts: 95, ripple: true });
	ok(&f.session, EditCommand::RemoveNode { node: folder });
	let delete_fixture = fixture();
	ok(&delete_fixture.session, EditCommand::DeleteClip { clip: delete_fixture.first });
	let ripple_fixture = fixture();
	ok(&ripple_fixture.session, EditCommand::RippleDeleteClip { clip: ripple_fixture.first });
	let split_fixture = fixture();
	ok(&split_fixture.session, EditCommand::SplitClip { clip: split_fixture.first, time_ts: 5 });

	for f in [f, roll_fixture, delete_fixture, ripple_fixture, split_fixture, effect_fixture] {
		let _ = std::fs::remove_file(f.media);
	}
}

mod effects;

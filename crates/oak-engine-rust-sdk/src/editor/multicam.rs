//! Safe multicam state and graph-resolution queries.

use std::ffi::c_void;

use crate::node::NodeId;
use crate::types::Rational;

use super::EditorSession;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_multicam_state(
		session: *const c_void,
		clip: u64,
		out: *mut RawMulticamState,
	) -> bool;
	fn oak_editor_multicam_connected_sequence(session: *const c_void, clip: u64) -> u64;
	fn oak_editor_multicam_is_clip(session: *const c_void, clip: u64) -> bool;
	fn oak_editor_multicam_clip_at(session: *const c_void, sequence: u64, time: Rational) -> u64;
	fn oak_editor_multicam_sequence(session: *const c_void, multicam: u64) -> u64;
	fn oak_editor_multicam_source_track(session: *const c_void, multicam: u64, source: i32) -> u64;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct RawMulticamState {
	sequence: u64,
	multicam: u64,
	clip: u64,
	source_count: i32,
	current_source: i32,
}

/// Resolved multicam node, source sequence and current angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MulticamState {
	pub sequence: NodeId,
	pub multicam: NodeId,
	pub clip: NodeId,
	pub source_count: i32,
	pub current_source: i32,
}

impl EditorSession {
	/// Resolves a clip's multicam state, or `None` when multicam is not enabled.
	pub fn multicam_state(&self, clip: NodeId) -> Option<MulticamState> {
		let mut raw = RawMulticamState::default();
		unsafe { oak_editor_multicam_state(self.ptr, clip.raw(), &mut raw) }.then(|| {
			MulticamState {
				sequence: NodeId::from_raw(raw.sequence),
				multicam: NodeId::from_raw(raw.multicam),
				clip: NodeId::from_raw(raw.clip),
				source_count: raw.source_count,
				current_source: raw.current_source,
			}
		})
	}

	/// Returns the first sequence feeding the clip's texture chain.
	pub fn multicam_connected_sequence(&self, clip: NodeId) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe {
			oak_editor_multicam_connected_sequence(self.ptr, clip.raw())
		});
		id.is_valid().then_some(id)
	}

	/// Whether a clip's direct texture source is a multicam node.
	pub fn multicam_is_clip(&self, clip: NodeId) -> bool {
		unsafe { oak_editor_multicam_is_clip(self.ptr, clip.raw()) }
	}

	/// Finds the multicam clip covering `time` on a sequence's video tracks.
	pub fn multicam_clip_at(&self, sequence: NodeId, time: Rational) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe {
			oak_editor_multicam_clip_at(self.ptr, sequence.raw(), time)
		});
		id.is_valid().then_some(id)
	}

	/// Returns the source sequence connected to a multicam node.
	pub fn multicam_sequence(&self, multicam: NodeId) -> Option<NodeId> {
		let id =
			NodeId::from_raw(unsafe { oak_editor_multicam_sequence(self.ptr, multicam.raw()) });
		id.is_valid().then_some(id)
	}

	/// Returns one angle's track inside the connected source sequence.
	pub fn multicam_source_track(&self, multicam: NodeId, source: i32) -> Option<NodeId> {
		let id = NodeId::from_raw(unsafe {
			oak_editor_multicam_source_track(self.ptr, multicam.raw(), source)
		});
		id.is_valid().then_some(id)
	}
}

#[cfg(test)]
mod tests {
	use crate::edit::EditCommand;
	use crate::editor::{EditorSession, TrackKind};
	use crate::types::Rational;

	#[test]
	fn multicam_enable_switch_split_afv_and_undo_roundtrip() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let angles = session.create_sequence("Angles").expect("angles");
		for kind in [TrackKind::Video, TrackKind::Audio] {
			session
				.execute(EditCommand::AddTrack { kind })
				.expect("second angle track");
		}
		let media_a = crate::test_support::make_test_clip_with_audio("multicam_a");
		let media_b = crate::test_support::make_test_clip_with_audio("multicam_b");
		let footage_a = session.project().import_footage(&media_a).expect("angle A");
		let footage_b = session.project().import_footage(&media_b).expect("angle B");
		for (track_index, footage) in [footage_a, footage_b].into_iter().enumerate() {
			for kind in [TrackKind::Video, TrackKind::Audio] {
				session
					.execute(EditCommand::PlaceFootageClip {
						footage,
						kind,
						track_index,
						in_ts: 0,
						out_ts: 25,
						media_in_ts: 0,
					})
					.expect("place angle");
			}
		}

		let host = session.create_sequence("Host").expect("host");
		let host_video = session
			.execute(EditCommand::PlaceNestedSequenceClip {
				source: angles,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
			})
			.expect("host video")
			.primary
			.expect("host video id");
		let host_audio = session
			.execute(EditCommand::PlaceFootageClip {
				footage: footage_a,
				kind: TrackKind::Audio,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
				media_in_ts: 0,
			})
			.expect("host audio")
			.primary
			.expect("host audio id");
		session
			.execute(EditCommand::SetClipsLinked {
				clips: vec![host_video, host_audio],
				linked: true,
			})
			.expect("link host audio");
		session
			.execute(EditCommand::SetMulticamEnabled {
				clips: vec![host_video],
				enabled: true,
			})
			.expect("enable multicam");
		let state = session.multicam_state(host_video).expect("multicam state");
		assert_eq!(state.sequence, angles);
		assert_eq!(session.multicam_sequence(state.multicam), Some(angles));
		assert!(session.multicam_source_track(state.multicam, 1).is_some());
		assert!(session.multicam_source_track(state.multicam, 99).is_none());
		assert!(state.source_count >= 2);
		assert_eq!(state.current_source, 0);
		assert_eq!(
			session.multicam_connected_sequence(host_video),
			Some(angles)
		);
		assert!(session.multicam_is_clip(host_video));
		assert_eq!(
			session.multicam_clip_at(host, Rational::new(0, 1)),
			Some(host_video)
		);
		assert_eq!(
			session.clip_media_filename(host_audio).as_deref(),
			Some(media_a.as_str())
		);

		session
			.execute(EditCommand::SwitchMulticam {
				clip: host_video,
				source: 1,
				split_clip: false,
				playhead: Rational::new(0, 1),
			})
			.expect("switch to angle B");
		assert_eq!(
			session.multicam_state(host_video).unwrap().current_source,
			1
		);
		assert_eq!(
			session.clip_media_filename(host_audio).as_deref(),
			Some(media_b.as_str())
		);
		assert!(crate::node::undo());
		assert_eq!(
			session.multicam_state(host_video).unwrap().current_source,
			0
		);
		assert_eq!(
			session.clip_media_filename(host_audio).as_deref(),
			Some(media_a.as_str())
		);
		assert!(crate::node::redo());
		assert_eq!(
			session.multicam_state(host_video).unwrap().current_source,
			1
		);
		assert_eq!(
			session.clip_media_filename(host_audio).as_deref(),
			Some(media_b.as_str())
		);

		session
			.execute(EditCommand::SwitchMulticam {
				clip: host_video,
				source: 0,
				split_clip: true,
				playhead: Rational::new(1, 2),
			})
			.expect("split switch to angle A");
		let host_snapshot = session.snapshot();
		let host_clips: Vec<_> = host_snapshot
			.sequences
			.iter()
			.find(|sequence| sequence.id == host)
			.expect("host snapshot")
			.tracks
			.iter()
			.flat_map(|track| track.clips.iter())
			.collect();
		assert!(host_clips.len() >= 3, "split creates a new video block");
		assert!(host_clips.iter().any(|clip| {
			session
				.multicam_state(clip.id)
				.is_some_and(|state| state.current_source == 0)
		}));
		assert!(crate::node::undo());
		assert_eq!(
			session.multicam_state(host_video).unwrap().current_source,
			1
		);

		session
			.execute(EditCommand::SetMulticamEnabled {
				clips: vec![host_video],
				enabled: false,
			})
			.expect("disable multicam");
		assert!(session.multicam_state(host_video).is_none());
		assert!(crate::node::undo());
		assert!(session.multicam_state(host_video).is_some());

		let _ = std::fs::remove_file(media_a);
		let _ = std::fs::remove_file(media_b);
	}

	#[test]
	fn multicam_wizard_builds_offsets_afv_and_survives_reload() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let media_a = crate::test_support::make_test_clip_with_audio("wizard_a");
		let media_b = crate::test_support::make_test_clip_with_audio("wizard_b");
		let angle_a = session.project().import_footage(&media_a).expect("angle A");
		let angle_b = session.project().import_footage(&media_b).expect("angle B");
		let host = session.create_sequence("Host").expect("host");
		assert!(
			session
				.execute(EditCommand::CreateMulticamSequence {
					host_sequence: host,
					angles: vec![angle_a],
					offsets: vec![0.0, 1.0],
					name: "Bad".into(),
				})
				.is_err()
		);
		let source = session
			.execute(EditCommand::CreateMulticamSequence {
				host_sequence: host,
				angles: vec![angle_a, angle_b],
				offsets: vec![0.0, 0.5],
				name: "Wizard".into(),
			})
			.expect("create wizard")
			.primary
			.expect("source sequence");
		assert_eq!(session.current_sequence(), Some(host));
		let host_clip = session
			.multicam_clip_at(host, Rational::new(0, 1))
			.expect("host multicam clip");
		let state = session.multicam_state(host_clip).expect("wizard state");
		assert_eq!(state.sequence, source);
		assert_eq!(state.source_count, 2);
		let snapshot = session.snapshot();
		let source_snapshot = snapshot
			.sequences
			.iter()
			.find(|sequence| sequence.id == source)
			.expect("source snapshot");
		let offset_clip = source_snapshot
			.tracks
			.iter()
			.filter(|track| track.kind == Some(TrackKind::Video))
			.flat_map(|track| track.clips.iter())
			.find(|clip| clip.in_time == Rational::new(13, 25))
			.expect("angle B offset");
		assert_eq!(offset_clip.out_time, Rational::new(38, 25));
		let host_audio = snapshot
			.sequences
			.iter()
			.find(|sequence| sequence.id == host)
			.expect("host snapshot")
			.tracks
			.iter()
			.find(|track| track.kind == Some(TrackKind::Audio))
			.and_then(|track| track.clips.first())
			.expect("host AFV audio")
			.id;
		assert_eq!(
			session.clip_media_filename(host_audio).as_deref(),
			Some(media_a.as_str())
		);

		let path = std::env::temp_dir()
			.join(format!("oak_sdk_wizard_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		session.save(&path).expect("save wizard");
		let opened = EditorSession::open(&path).expect("reload wizard");
		let reloaded_clip = opened
			.multicam_clip_at(host, Rational::new(0, 1))
			.expect("reloaded host clip");
		let reloaded = opened
			.multicam_state(reloaded_clip)
			.expect("reloaded state");
		assert_eq!(reloaded.sequence, source);
		assert_eq!(reloaded.source_count, 2);
		assert_eq!(
			opened.clip_media_filename(host_audio).as_deref(),
			Some(media_a.as_str())
		);

		let _ = std::fs::remove_file(path);
		let _ = std::fs::remove_file(media_a);
		let _ = std::fs::remove_file(media_b);
	}
}

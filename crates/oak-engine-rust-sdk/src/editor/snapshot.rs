//! Owned project and timeline snapshot DTOs.
//!
//! Snapshots detach UI-facing state from live engine handles. They contain no
//! borrowed graph references, so callers may cache or compare them across edits.
//! Use `ProjectSnapshot::revision` to detect that a snapshot has become stale.

use crate::node::{NodeId, NodeKind, Project, StreamAudioParams, StreamVideoParams};
use crate::types::{Rational, TimeRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Track-list kind exposed without depending on engine-internal track types.
pub enum TrackKind {
	/// A video track.
	Video,
	/// An audio track.
	Audio,
	/// A subtitle track.
	Subtitle,
}

/// Converts the native track discriminant, preserving unknown values as `None`.
pub(crate) fn track_kind(raw: i32) -> Option<TrackKind> {
	match raw {
		0 => Some(TrackKind::Video),
		1 => Some(TrackKind::Audio),
		2 => Some(TrackKind::Subtitle),
		_ => None,
	}
}

#[derive(Clone, Debug, PartialEq)]
/// One project-explorer entry. Only folders have recursive `children`.
pub struct EntrySnapshot {
	/// The `id` field.
	pub id: NodeId,
	/// The `kind` field.
	pub kind: Option<NodeKind>,
	/// The `name` field.
	pub name: String,
	/// The `children` field.
	pub children: Vec<EntrySnapshot>,
}

/// Recursively copies a folder subtree while preserving child order.
pub(crate) fn entry_snapshot(project: &Project, id: NodeId) -> EntrySnapshot {
	let children = if project.node_kind(id) == Some(NodeKind::Folder) {
		(0..project.folder_child_count(id))
			.map(|index| entry_snapshot(project, project.folder_child_at(id, index)))
			.collect()
	} else {
		Vec::new()
	};
	EntrySnapshot {
		id,
		kind: project.node_kind(id),
		name: project.node_label(id),
		children,
	}
}

/// Flattens mounted sequences and footage into stable, explorer-order vectors.
/// Footage stream parameters describe the first video/audio stream, if present.
pub(crate) fn collect_media(
	project: &Project,
	folder: NodeId,
	sequences: &mut Vec<SequenceSnapshot>,
	footages: &mut Vec<FootageSnapshot>,
) {
	for index in 0..project.folder_child_count(folder) {
		let id = project.folder_child_at(folder, index);
		match project.node_kind(id) {
			Some(NodeKind::Folder) => collect_media(project, id, sequences, footages),
			Some(NodeKind::Sequence) => sequences.push(sequence_snapshot(project, id)),
			Some(NodeKind::Footage) => footages.push(FootageSnapshot {
				id,
				name: project.node_label(id),
				filename: project.footage_filename(id),
				duration: project.footage_duration(id),
				video_streams: project.footage_video_stream_count(id),
				audio_streams: project.footage_audio_stream_count(id),
				subtitle_streams: project.footage_subtitle_stream_count(id),
				video: project.footage_video_params(id, 0),
				audio: project.footage_audio_params(id, 0),
			}),
			_ => {}
		}
	}
}

/// Copies a sequence and all of its tracks and blocks.
/// Timeline positions remain rational seconds, independent of UI frame rounding.
pub(crate) fn sequence_snapshot(project: &Project, id: NodeId) -> SequenceSnapshot {
	let (width, height) = project.sequence_geometry(id).unwrap_or((0, 0));
	let mut tracks = Vec::new();
	for list_index in 0..project.sequence_track_list_count(id) {
		let list = project.sequence_track_list_at(id, list_index);
		for track_index in 0..project.tracklist_track_count(list) {
			let track = project.tracklist_track_at(list, track_index);
			let clips = (0..project.track_block_count(track))
				.map(|block_index| {
					let block = project.track_block_at(track, block_index);
					ClipSnapshot {
						id: block,
						kind: project.node_kind(block),
						name: project.node_label(block),
						in_time: project.block_in(block),
						out_time: project.block_out(block),
						media_in: project.block_media_in(block),
						media_out: project.block_media_out(block),
						enabled: project.block_enabled(block),
					}
				})
				.collect();
			tracks.push(TrackSnapshot {
				id: track,
				kind: track_kind(project.track_kind(track)),
				height: project.track_height(track),
				muted: project.track_muted(track),
				locked: project.track_locked(track),
				clips,
			});
		}
	}
	SequenceSnapshot {
		id,
		name: project.node_label(id),
		width,
		height,
		frame_rate: project.sequence_frame_rate(id),
		interlaced: project.sequence_interlaced(id).unwrap_or(false),
		length: project.sequence_length(id),
		playhead: project.sequence_playhead(id),
		tracks,
	}
}

#[derive(Clone, Debug, PartialEq)]
/// Sequence format, playback state and flattened track contents.
pub struct SequenceSnapshot {
	/// The `id` field.
	pub id: NodeId,
	/// The `name` field.
	pub name: String,
	/// The `width` field.
	pub width: i32,
	/// The `height` field.
	pub height: i32,
	/// The `frame_rate` field.
	pub frame_rate: Rational,
	/// The `interlaced` field.
	pub interlaced: bool,
	/// The `length` field.
	pub length: Rational,
	/// The `playhead` field.
	pub playhead: Rational,
	/// The `tracks` field.
	pub tracks: Vec<TrackSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
/// One timeline track and its blocks in timeline order.
pub struct TrackSnapshot {
	/// The `id` field.
	pub id: NodeId,
	/// The `kind` field.
	pub kind: Option<TrackKind>,
	/// The `height` field.
	pub height: f64,
	/// The `muted` field.
	pub muted: bool,
	/// The `locked` field.
	pub locked: bool,
	/// The `clips` field.
	pub clips: Vec<ClipSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
/// Block timing on the sequence (`in_time`/`out_time`) and in its source media
/// (`media_in`/`media_out`). Ranges are half-open.
pub struct ClipSnapshot {
	/// The `id` field.
	pub id: NodeId,
	/// The `kind` field.
	pub kind: Option<NodeKind>,
	/// The `name` field.
	pub name: String,
	/// The `in_time` field.
	pub in_time: Rational,
	/// The `out_time` field.
	pub out_time: Rational,
	/// The `media_in` field.
	pub media_in: Rational,
	/// The `media_out` field.
	pub media_out: Rational,
	/// The `enabled` field.
	pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// Project-bin footage metadata and first-stream decode parameters.
pub struct FootageSnapshot {
	/// The `id` field.
	pub id: NodeId,
	/// The `name` field.
	pub name: String,
	/// The `filename` field.
	pub filename: Option<String>,
	/// The `duration` field.
	pub duration: Rational,
	/// The `video_streams` field.
	pub video_streams: usize,
	/// The `audio_streams` field.
	pub audio_streams: usize,
	/// The `subtitle_streams` field.
	pub subtitle_streams: usize,
	/// The `video` field.
	pub video: Option<StreamVideoParams>,
	/// The `audio` field.
	pub audio: Option<StreamAudioParams>,
}

#[derive(Clone, Debug, PartialEq)]
/// One sequence-local marker.
pub struct MarkerSnapshot {
	/// The `name` field.
	pub name: String,
	/// The `time` field.
	pub time: TimeRange,
	/// The `color` field.
	pub color: i32,
}

#[derive(Clone, Debug, PartialEq)]
/// Enabled sequence work area; absence is represented by `None` in the project.
pub struct WorkAreaSnapshot {
	/// The `range` field.
	pub range: TimeRange,
}

#[derive(Clone, Debug, PartialEq)]
/// A detached view of the project at `revision`.
///
/// `root` preserves the explorer hierarchy; `sequences` and `footages` provide
/// flattened access without walking that hierarchy.
pub struct ProjectSnapshot {
	/// The `revision` field.
	pub revision: i64,
	/// The `name` field.
	pub name: String,
	/// The `filename` field.
	pub filename: String,
	/// The `uuid` field.
	pub uuid: String,
	/// The `modified` field.
	pub modified: bool,
	/// The `root` field.
	pub root: EntrySnapshot,
	/// The `sequences` field.
	pub sequences: Vec<SequenceSnapshot>,
	/// The `footages` field.
	pub footages: Vec<FootageSnapshot>,
	/// The `current_sequence` field.
	pub current_sequence: Option<NodeId>,
	/// The `markers` field.
	pub markers: Vec<MarkerSnapshot>,
	/// The `workarea` field.
	pub workarea: Option<WorkAreaSnapshot>,
}


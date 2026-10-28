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
	Video,
	Audio,
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
	pub id: NodeId,
	pub kind: Option<NodeKind>,
	pub name: String,
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
	pub id: NodeId,
	pub name: String,
	pub width: i32,
	pub height: i32,
	pub frame_rate: Rational,
	pub interlaced: bool,
	pub length: Rational,
	pub playhead: Rational,
	pub tracks: Vec<TrackSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
/// One timeline track and its blocks in timeline order.
pub struct TrackSnapshot {
	pub id: NodeId,
	pub kind: Option<TrackKind>,
	pub height: f64,
	pub muted: bool,
	pub locked: bool,
	pub clips: Vec<ClipSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
/// Block timing on the sequence (`in_time`/`out_time`) and in its source media
/// (`media_in`/`media_out`). Ranges are half-open.
pub struct ClipSnapshot {
	pub id: NodeId,
	pub kind: Option<NodeKind>,
	pub name: String,
	pub in_time: Rational,
	pub out_time: Rational,
	pub media_in: Rational,
	pub media_out: Rational,
	pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// Project-bin footage metadata and first-stream decode parameters.
pub struct FootageSnapshot {
	pub id: NodeId,
	pub name: String,
	pub filename: Option<String>,
	pub duration: Rational,
	pub video_streams: usize,
	pub audio_streams: usize,
	pub subtitle_streams: usize,
	pub video: Option<StreamVideoParams>,
	pub audio: Option<StreamAudioParams>,
}

#[derive(Clone, Debug, PartialEq)]
/// One sequence-local marker.
pub struct MarkerSnapshot {
	pub name: String,
	pub time: TimeRange,
	pub color: i32,
}

#[derive(Clone, Debug, PartialEq)]
/// Enabled sequence work area; absence is represented by `None` in the project.
pub struct WorkAreaSnapshot {
	pub range: TimeRange,
}

#[derive(Clone, Debug, PartialEq)]
/// A detached view of the project at `revision`.
///
/// `root` preserves the explorer hierarchy; `sequences` and `footages` provide
/// flattened access without walking that hierarchy.
pub struct ProjectSnapshot {
	pub revision: i64,
	pub name: String,
	pub filename: String,
	pub uuid: String,
	pub modified: bool,
	pub root: EntrySnapshot,
	pub sequences: Vec<SequenceSnapshot>,
	pub footages: Vec<FootageSnapshot>,
	pub current_sequence: Option<NodeId>,
	pub markers: Vec<MarkerSnapshot>,
	pub workarea: Option<WorkAreaSnapshot>,
}


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

//! App-side SDK adapter for the project / timeline / storage domain.
//!
//! This module is a pure `quercus-engine-rs` consumer: the app links no
//! Oak module crate. Project lifecycle goes through `EditorSession`,
//! read-only state through owned `Project` queries and snapshots, and every
//! edit is an SDK `EditCommand` executed on the live session (one undo row
//! per user action, with `undo_group_begin/end` for composite gestures).
//! Marker and work-area state live in the session; the project library is
//! the SDK `ProjectLibrary`.

use std::path::Path;

use quercus_engine_rs::editor::{EditorSession, TrackKind};
use quercus_engine_rs::node::{NodeId, NodeKind, Project};
use quercus_engine_rs::storage::ProjectLibrary;
use quercus_engine_rs::types::{Rational, TimeRange};

use super::engine::LibraryProject;

/// The shared project handle (the SDK's refcounted project; `Clone` adds a
/// reference, it is not an `Arc<Mutex<_>>` of an internal type).
pub type ProjectRef = Project;

/// The node id whose stable identity is `identity` (`None` for the
/// sentinel). The app's widget-facing ids (clip ids, explorer entry ids)
/// ARE the node identities.
pub fn id_of(identity: u64) -> Option<NodeId> {
	let id = NodeId::from_raw(identity);
	id.is_valid().then_some(id)
}

fn ids_of_kind(p: &ProjectRef, kind: NodeKind) -> Vec<NodeId> {
	p.node_ids()
		.into_iter()
		.filter(|&id| p.node_kind(id) == Some(kind))
		.collect()
}

// ---------------------------------------------------------------------------
// Project lifecycle
// ---------------------------------------------------------------------------

/// Create a blank, initialized project through a fresh SDK session.
pub fn create_project() -> ProjectRef {
	EditorSession::new()
		.expect("create a new SDK editor session")
		.project()
}

/// Load a `.ove` project file (plain XML; the module serializer ignores
/// the legacy compression flag).
pub fn load_ove(path: &Path) -> Result<ProjectRef, String> {
	let session = EditorSession::open(&path.to_string_lossy())
		.map_err(|error| format!("failed to open \"{}\": {error}", path.display()))?;
	Ok(session.project())
}

/// Write the project to `path` through the OVE serializer.
pub fn save_ove(project: &ProjectRef, path: &Path) -> Result<(), String> {
	let session = super::real::editor_session_for_project(project)?;
	session
		.save(&path.to_string_lossy())
		.map_err(|error| format!("failed to save \"{}\": {error}", path.display()))
}

/// The display name (`Project::name`: filename base or "(untitled)").
pub fn project_name(p: &ProjectRef) -> String {
	p.name()
}

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------

/// Every sequence node in the graph, in arena order.
pub fn sequence_ids(p: &ProjectRef) -> Vec<NodeId> {
	ids_of_kind(p, NodeKind::Sequence)
}

/// Every footage node in the graph, in arena order.
pub fn footage_ids(p: &ProjectRef) -> Vec<NodeId> {
	ids_of_kind(p, NodeKind::Footage)
}

/// Every folder node in the graph, in arena order.
pub fn folder_ids(p: &ProjectRef) -> Vec<NodeId> {
	ids_of_kind(p, NodeKind::Folder)
}

/// Create a sequence node named `name` and mount it under the root folder.
pub fn create_sequence(project: &ProjectRef, name: &str) -> NodeId {
	create_sequence_with_params(project, name, None)
}

/// Create a sequence with an explicit first-video-stream format.
pub fn create_sequence_with_params(
	project: &ProjectRef,
	name: &str,
	params: Option<(i32, i32, Rational, bool)>,
) -> NodeId {
	let session = super::real::editor_session_for_project(project)
		.expect("create a sequence through the SDK session");
	let (width, height, rate, interlaced) = params
		.map(|(w, h, r, i)| (Some(w), Some(h), Some(sdk_rational(r)), Some(i)))
		.unwrap_or((None, None, None, None));
	let result = session
		.execute(quercus_engine_rs::edit::EditCommand::CreateSequence {
			name: name.to_string(),
			width,
			height,
			rate,
			interlaced,
		})
		.expect("create the sequence");
	primary(result).expect("created sequence id")
}

/// Create a folder under the project's root (undoable "New Folder").
pub fn create_folder(project: &ProjectRef, name: &str) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(project)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::CreateFolder {
		name: name.to_string(),
	})?;
	primary(result)
}

/// The stable type id of the text generator node.
pub const TEXT_FOOTAGE_TYPE_ID: &str = "org.olivevideoeditor.Olive.text3";

/// The bin label a created text generator starts with.
pub const TEXT_FOOTAGE_LABEL: &str = "文本";

/// Create a text generator in the project's root folder (ONE undo row
/// covers the node and its mount).
pub fn create_text_footage_node(project: &ProjectRef) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(project)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::CreateTextFootage)?;
	primary(result)
}

/// Attach every orphaned sequence to the root folder.
pub fn ensure_sequences_mounted(project: &ProjectRef) {
	if let Ok(session) = super::real::editor_session_for_project(project) {
		let _ = session.execute(quercus_engine_rs::edit::EditCommand::EnsureSequencesMounted);
	}
}

/// Apply new display name and video parameters to a sequence (the
/// sequence-properties dialog's commit; not undoable).
pub fn set_sequence_parameters(
	project: &ProjectRef,
	seq: NodeId,
	name: &str,
	width: i32,
	height: i32,
	rate: Rational,
	interlaced: bool,
) -> Result<(), String> {
	if project.node_kind(seq) != Some(NodeKind::Sequence) {
		return Err("entry is not a sequence".to_string());
	}
	if !project.node_set_label(seq, name) {
		return Err("could not rename the sequence".to_string());
	}
	if !project.sequence_set_video_params(
		seq,
		width,
		height,
		rate.num as i32,
		rate.den as i32,
	) {
		return Err("could not set the sequence video parameters".to_string());
	}
	if !project.sequence_set_interlaced(seq, interlaced) {
		return Err("could not set the sequence interlacing".to_string());
	}
	Ok(())
}

/// The label of a node (empty when the id is stale).
pub fn node_label(p: &ProjectRef, id: NodeId) -> String {
	p.node_label(id)
}

/// The type id of a node (empty when the id is stale).
pub fn node_type_id(p: &ProjectRef, id: NodeId) -> String {
	p.node_type_id(id).unwrap_or_default()
}

/// The project's multicam nodes (the wizard/sequence drop rewire uses the
/// one whose source sequence matches).
pub fn multicam_nodes(p: &ProjectRef) -> Vec<NodeId> {
	p.node_ids()
		.into_iter()
		.filter(|&id| {
			p.node_type_id(id).as_deref() == Some("org.olivevideoeditor.Olive.multicam")
		})
		.collect()
}

/// The sequence's video format `(width, height, rate)`.
pub fn sequence_video_params(p: &ProjectRef, seq: NodeId) -> Option<(i32, i32, Rational)> {
	let (width, height) = p.sequence_geometry(seq)?;
	let rate = p.sequence_frame_rate(seq);
	(rate.num > 0 && rate.den > 0).then_some((width, height, rate))
}

/// The sequence's frame duration as a `(num, den)` timebase pair.
pub fn sequence_time_base(p: &ProjectRef, seq: NodeId) -> Option<(i64, i64)> {
	let (_, _, rate) = sequence_video_params(p, seq)?;
	let num = i64::from(rate.num);
	let den = i64::from(rate.den);
	if num <= 0 || den <= 0 {
		return None;
	}
	Some((den, num))
}

/// The sequence content length (rational seconds).
pub fn sequence_length(p: &ProjectRef, seq: NodeId) -> Rational {
	p.sequence_length(seq)
}

/// The sequence's playhead position (rational seconds).
pub fn sequence_playhead(p: &ProjectRef, seq: NodeId) -> Rational {
	p.sequence_playhead(seq)
}

/// Move the sequence's playhead (undoable "Set Playhead").
pub fn sequence_set_playhead(p: &ProjectRef, seq: NodeId, time: Rational) {
	let Ok(session) = super::real::editor_session_for_project(p) else {
		return;
	};
	let _ = session.execute(quercus_engine_rs::edit::EditCommand::SetSequencePlayhead {
		sequence: sdk_node(seq),
		time: sdk_rational(time),
	});
}

// ---------------------------------------------------------------------------
// Timebase helpers
// ---------------------------------------------------------------------------

fn gcd(a: i64, b: i64) -> i64 {
	let (mut a, mut b) = (a.abs(), b.abs());
	while b != 0 {
		let t = b;
		b = a % b;
		a = t;
	}
	a.max(1)
}

/// Rational seconds → integer frame timestamp on timebase `(num, den)`.
pub fn rational_to_ts(r: Rational, tb: (i64, i64)) -> i64 {
	let num = i128::from(r.num) * i128::from(tb.1);
	let den = i128::from(r.den) * i128::from(tb.0);
	if den == 0 {
		return 0;
	}
	(num / den) as i64
}

/// Integer frame timestamp → rational seconds on timebase `(num, den)`.
pub fn ts_to_rational(ts: i64, tb: (i64, i64)) -> Rational {
	if tb.0 <= 0 || tb.1 <= 0 {
		return Rational::new(0, 1);
	}
	let num = ts.saturating_mul(tb.0);
	let den = tb.1;
	let g = gcd(num, den);
	Rational::new(num / g, den / g)
}

// ---------------------------------------------------------------------------
// Tracks and clips
// ---------------------------------------------------------------------------

fn track_kind_of(raw: i32) -> Option<TrackKind> {
	match raw {
		0 => Some(TrackKind::Video),
		1 => Some(TrackKind::Audio),
		2 => Some(TrackKind::Subtitle),
		_ => None,
	}
}

/// The sequence's track list of `kind`, if present.
pub fn track_list_of(p: &ProjectRef, seq: NodeId, kind: TrackKind) -> Option<NodeId> {
	for index in 0..p.sequence_track_list_count(seq) {
		let list = p.sequence_track_list_at(seq, index);
		if track_kind_of(p.tracklist_kind(list)) == Some(kind) {
			return Some(list);
		}
	}
	None
}

/// The tracks of the sequence's list of `kind`, in list order.
pub fn track_ids(p: &ProjectRef, seq: NodeId, kind: TrackKind) -> Vec<NodeId> {
	let Some(list) = track_list_of(p, seq, kind) else {
		return Vec::new();
	};
	(0..p.tracklist_track_count(list))
		.map(|index| p.tracklist_track_at(list, index))
		.collect()
}

fn is_timeline_clip(p: &ProjectRef, node: NodeId) -> bool {
	matches!(
		p.node_kind(node),
		Some(NodeKind::Clip | NodeKind::Adjustment)
	)
}

/// The clip blocks of a track, in timeline order. Gaps and transitions are
/// not clips and are skipped; adjustment layers are.
pub fn clip_ids(p: &ProjectRef, track: NodeId) -> Vec<NodeId> {
	(0..p.track_block_count(track))
		.map(|index| p.track_block_at(track, index))
		.filter(|&block| is_timeline_clip(p, block))
		.collect()
}

/// A block's timeline range and media in-point (rational seconds).
pub fn clip_range(p: &ProjectRef, clip: NodeId) -> Option<(Rational, Rational, Rational)> {
	if p.node_kind(clip).is_none() {
		return None;
	}
	Some((p.block_in(clip), p.block_out(clip), p.block_media_in(clip)))
}

/// The block's owning track.
pub fn clip_track(p: &ProjectRef, clip: NodeId) -> Option<NodeId> {
	p.block_track(clip)
}

/// The first footage node feeding `id`.
pub fn find_input_footage(p: &ProjectRef, id: NodeId) -> Option<NodeId> {
	p.block_footage(id)
}

/// The transition wedges of every clip on `track`, keyed by clip:
/// `(start, end)` are the widths of the transition before the clip (drawn
/// on its head) and after it (drawn on its tail); `None` when that side has
/// no transition.
pub fn clip_transition_widths(
	p: &ProjectRef,
	track: NodeId,
) -> std::collections::HashMap<NodeId, (Option<Rational>, Option<Rational>)> {
	let mut widths = std::collections::HashMap::new();
	for block in 0..p.track_block_count(track) {
		let clip = p.track_block_at(track, block);
		if !is_timeline_clip(p, clip) {
			continue;
		}
		widths.insert(
			clip,
			(
				p.block_transition_width(clip, true),
				p.block_transition_width(clip, false),
			),
		);
	}
	widths
}

/// The footage file feeding `clip` (through its effect chain when present).
pub fn clip_media_filename(p: &ProjectRef, clip: NodeId) -> Option<String> {
	let footage = find_input_footage(p, clip)?;
	p.footage_filename(footage)
}

// ---------------------------------------------------------------------------
// Footage
// ---------------------------------------------------------------------------

/// The footage's probed duration in seconds (`None` when unprobed).
pub fn footage_duration_seconds(p: &ProjectRef, id: NodeId) -> Option<f64> {
	let duration = p.footage_duration(id);
	if duration.den == 0 || duration.num <= 0 {
		return None;
	}
	Some(duration.num as f64 / duration.den as f64)
}

/// Reprobe footage whose stream metadata is missing.
pub fn reprobe_unprobed_footage(project: &ProjectRef) {
	if let Ok(session) = super::real::editor_session_for_project(project) {
		let _ = session.execute(quercus_engine_rs::edit::EditCommand::ReprobeUnprobedFootage);
	}
}

/// Import a media file into the project's root folder (one undoable
/// "Import Footage" entry).
pub fn import_footage(project: &ProjectRef, path: &Path) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(project)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::ImportFootage {
		path: path.to_string_lossy().into_owned(),
	})?;
	primary(result)
}

// ---------------------------------------------------------------------------
// Markers and work area (session-owned auxiliary state)
// ---------------------------------------------------------------------------

/// The session's markers as the timeline shows them: in-point, name,
/// color index.
pub fn markers(session: &EditorSession) -> Vec<(Rational, String, i32)> {
	session
		.markers()
		.into_iter()
		.map(|marker| (marker.time.in_, marker.name, marker.color))
		.collect()
}

fn marker_at(session: &EditorSession, time: Rational) -> bool {
	session
		.markers()
		.iter()
		.any(|marker| marker.time.in_ == time)
}

/// Undoable marker add ("Add Marker"). A marker at the same time is
/// rejected.
pub fn marker_add(
	session: &EditorSession,
	time: Rational,
	name: &str,
	color: i32,
) -> Result<(), String> {
	if marker_at(session, time) {
		return Err("a marker already exists at that time".to_string());
	}
	session
		.execute(quercus_engine_rs::edit::EditCommand::MarkerAdd {
			time: sdk_rational(time),
			name: name.to_string(),
			color,
		})
		.map(|_| ())
}

/// Undoable marker remove ("Remove Marker").
pub fn marker_remove(session: &EditorSession, time: Rational) -> Result<(), String> {
	if !marker_at(session, time) {
		return Err("no marker at that time".to_string());
	}
	session
		.execute(quercus_engine_rs::edit::EditCommand::MarkerRemove {
			time: sdk_rational(time),
		})
		.map(|_| ())
}

/// The session's work area as `(enabled, range)`.
pub fn workarea_state(session: &EditorSession) -> (bool, TimeRange) {
	session.workarea_state()
}

/// Live (non-undoable) workarea write: enable flag plus range.
pub fn workarea_set(session: &EditorSession, enabled: bool, range: TimeRange) {
	let _ = session.execute(quercus_engine_rs::edit::EditCommand::WorkareaSet {
		enabled,
		range: sdk_time_range(range),
	});
}

/// Undoable workarea set ("Set Workarea"): the enabled flag plus the
/// in/out range as ONE entry.
pub fn workarea_set_undoable(
	session: &EditorSession,
	enabled: bool,
	range: TimeRange,
	old_range: TimeRange,
) -> Result<(), String> {
	session
		.execute(quercus_engine_rs::edit::EditCommand::WorkareaSetUndoable {
			enabled,
			range: sdk_time_range(range),
			old_range: sdk_time_range(old_range),
		})
		.map(|_| ())
}

// ---------------------------------------------------------------------------
// Project library (the write-through database the manager browses)
// ---------------------------------------------------------------------------

/// The configured default library as an SDK client; `None` when storage is
/// disabled (the manager shows an empty list, not an error).
fn library_client() -> Option<ProjectLibrary> {
	ProjectLibrary::configured().ok().flatten()
}

/// The library rows, most recently modified first (the project manager's
/// data source). With storage disabled the result is the empty list, not
/// an error (the facade contract).
pub fn library_list() -> Result<Vec<LibraryProject>, String> {
	let Some(client) = library_client() else {
		return Ok(Vec::new());
	};
	let infos = client.projects().map_err(|e| e.to_string())?;
	Ok(infos
		.into_iter()
		.map(|info| LibraryProject {
			uuid: info.uuid,
			name: info.name,
			created_at: info.created_at,
			modified_at: info.modified_at,
			duration_ms: info.duration_ms,
			track_count: info.track_count,
			clip_count: info.clip_count,
			footage_count: info.footage_count,
		})
		.collect())
}

fn library_or_error() -> Result<ProjectLibrary, String> {
	library_client().ok_or_else(|| "the project library is not configured".to_string())
}

/// Create a blank project row named `name`; returns its uuid.
pub fn library_create(name: &str) -> Result<String, String> {
	library_or_error()?.create(name).map_err(|e| e.to_string())
}

/// Delete the library row `uuid` (cascades settings / snapshots /
/// journal).
pub fn library_delete(uuid: &str) -> Result<(), String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_or_error()?.delete(uuid).map_err(|e| e.to_string())
}

/// Rename the library row `uuid` (the manager's list name).
pub fn library_rename(uuid: &str, name: &str) -> Result<(), String> {
	if uuid.is_empty() || name.trim().is_empty() {
		return Err("invalid uuid or name".to_string());
	}
	library_or_error()?
		.rename(uuid, name.trim())
		.map_err(|e| e.to_string())
}

/// Duplicate the library row `uuid` (history included) under a fresh
/// uuid; returns the new row's uuid.
pub fn library_duplicate(uuid: &str) -> Result<String, String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_or_error()?
		.duplicate(uuid)
		.map_err(|e| e.to_string())
}

/// Import a `.ove` / `.otio` / `.fcpxml` project file as a new library
/// row; returns the new row's uuid.
pub fn library_import(path: &Path) -> Result<String, String> {
	library_or_error()?
		.import(&path.to_string_lossy())
		.map_err(|e| e.to_string())
}

/// Export the library row `uuid` to `path`; the format is dispatched by
/// extension through the storage registry.
pub fn library_export(uuid: &str, path: &Path) -> Result<(), String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	library_or_error()?
		.export(uuid, &path.to_string_lossy())
		.map_err(|e| e.to_string())
}

/// Load the library row `uuid` as a fresh project.
pub fn library_open(uuid: &str) -> Result<ProjectRef, String> {
	if uuid.is_empty() {
		return Err("invalid uuid".to_string());
	}
	let session = library_or_error()?
		.open(uuid)
		.map_err(|e| e.to_string())?;
	Ok(session.project())
}

// ---------------------------------------------------------------------------
// Write-through binding (the engine's per-project session state)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Undoable edit primitives
//
// The timeline edits the facade's `oakengine_sequence_*` / `oakengine_clip_*`
// exports performed, rebuilt over the oaktimeline command structs and the
// the engine's global undo stack. Every entry point
// pushes exactly one undo row (composite edits assemble a multi command).
// ---------------------------------------------------------------------------

fn execute(p: &ProjectRef, command: quercus_engine_rs::edit::EditCommand) -> Result<(), String> {
	super::real::editor_session_for_project(p)?
		.execute(command)
		.map(|_| ())
}

fn sdk_node(id: NodeId) -> quercus_engine_rs::node::NodeId {
	quercus_engine_rs::node::NodeId::from_raw(id.raw())
}

fn sdk_track_kind(kind: TrackKind) -> quercus_engine_rs::editor::TrackKind {
	kind
}

fn sdk_rational(value: Rational) -> quercus_engine_rs::types::Rational {
	quercus_engine_rs::types::Rational::new(value.num, value.den)
}

fn from_sdk_rational(value: quercus_engine_rs::types::Rational) -> Rational {
	Rational::new(value.num, value.den)
}

fn sdk_time_range(value: TimeRange) -> quercus_engine_rs::types::TimeRange {
	quercus_engine_rs::types::TimeRange::new(
		value.in_,
		value.out,
	)
}

fn track_kind_from_sdk(kind: quercus_engine_rs::editor::TrackKind) -> TrackKind {
	kind
}

fn primary(result: quercus_engine_rs::edit::EditResult) -> Result<NodeId, String> {
	result
		.primary
		.and_then(|node| id_of(node.raw()))
		.ok_or_else(|| "the edit produced no node".to_string())
}

/// Add a track of `kind` to the sequence (undoable "Add Track"; the
/// module's `TimelineAddTrackCommand`), returning the new track's index.
pub fn add_track(p: &ProjectRef, seq: NodeId, kind: TrackKind) -> Result<usize, String> {
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	session
		.execute(quercus_engine_rs::edit::EditCommand::AddTrack {
			kind: sdk_track_kind(kind),
		})
		.map(|result| result.value.max(0) as usize)
}

/// Remove `track` from its list (undoable "Remove Track"; the module's
/// `TimelineRemoveTrackCommand` redo performs the full list+graph
/// removal, so no live compensation is needed).
pub fn remove_track(p: &ProjectRef, track: NodeId) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RemoveTrack {
			track: sdk_node(track),
		},
	)
}

/// A track's height in internal units (`None` when the id is stale).
pub fn track_height(p: &ProjectRef, track: NodeId) -> Option<f64> {
	(p.node_kind(track) == Some(NodeKind::Track)).then(|| p.track_height(track))
}

/// A track's muted flag (`None` when the id is stale). Muted means
/// "silenced" on audio tracks and "hidden" on video/subtitle tracks
/// (Olive parity: one flag drives both).
pub fn track_muted(p: &ProjectRef, track: NodeId) -> Option<bool> {
	(p.node_kind(track) == Some(NodeKind::Track)).then(|| p.track_muted(track))
}

/// A track's locked flag (`None` when the id is stale).
pub fn track_locked(p: &ProjectRef, track: NodeId) -> Option<bool> {
	(p.node_kind(track) == Some(NodeKind::Track)).then(|| p.track_locked(track))
}

/// Set a track's muted flag (undoable "Set Track Muted"). On video and
/// subtitle tracks this is the visibility (show/hide) toggle.
pub fn set_track_muted(p: &ProjectRef, track: NodeId, muted: bool) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetTrackMuted {
			track: sdk_node(track),
			muted,
		},
	)
}

/// Set a track's locked flag (undoable "Set Track Locked"). Locked tracks
/// reject clip edits (the app layer refuses trim/move/split/delete).
pub fn set_track_locked(p: &ProjectRef, track: NodeId, locked: bool) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetTrackLocked {
			track: sdk_node(track),
			locked,
		},
	)
}

/// Set a track's height in internal units (NOT undoable, mirroring the
/// facade's `oakengine_track_set_height`).
pub fn set_track_height(p: &ProjectRef, track: NodeId, height: f64) {
	if height <= 0.0 {
		return;
	}
	let _ = execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetTrackHeight {
			track: sdk_node(track),
			height,
		},
	);
}

/// The stable per-clip color index, fixed when the clip is created: the
/// node identity is unique per creation and never shifts when clips are
/// added or removed elsewhere, so a clip keeps its color for its whole
/// life (persisted through [`crate::oakui::real`]'s `override_color`).
fn clip_color_index(id: NodeId) -> i32 {
	(id.raw() % 8) as i32
}

/// Place a clip of `footage` on track `track_index` of the sequence's
/// `kind` list (undoable "Add Clip", one row): the clip block is created
/// in the project, placed by the module's `TrackPlaceBlockCommand`, and
/// the footage is wired to the clip's `tex_in` — the facade's
/// `oakengine_sequence_add_footage_clip_ex` composition, minus the
/// scratch-project dance (the app keeps everything in one project).
///
/// `in_ts`/`out_ts`/`media_in_ts` are frame timestamps in the sequence's
/// frame-rate timebase. On undo the block is detached from the track but
/// left as an orphan node in the project graph (the module command's
/// documented behavior).
pub fn place_footage_clip(
	p: &ProjectRef,
	seq: NodeId,
	footage: NodeId,
	kind: TrackKind,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
	media_in_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts || media_in_ts < 0 {
		return Err("invalid clip range (need 0 <= in < out and media_in >= 0)".to_string());
	}
	if kind != TrackKind::Video && kind != TrackKind::Audio {
		return Err("clips are only supported on video and audio tracks".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PlaceFootageClip {
		footage: sdk_node(footage),
		kind: sdk_track_kind(kind),
		track_index,
		in_ts,
		out_ts,
		media_in_ts,
	})?;
	primary(result)
}

/// Add an adjustment layer to `track` spanning `[in_ts, out_ts)` (frame
/// timestamps in the sequence's timebase): an [`AdjustmentBlockBehavior`]
/// block with no footage, so it renders its effect chain over the clips
/// underneath it. Only video tracks take adjustment layers.
///
/// Undoable "Add Adjustment Layer" (the block is created in the project
/// and placed by the module's `TrackPlaceBlockCommand`); on undo the
/// block is detached from the track but left as an orphan node in the
/// project graph (the module command's documented behavior, same as
/// [`place_footage_clip`]).
pub fn create_adjustment_layer(
	p: &ProjectRef,
	seq: NodeId,
	track: NodeId,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid adjustment layer range (need 0 <= in < out)".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::CreateAdjustmentLayer {
		track: sdk_node(track),
		in_ts,
		out_ts,
	})?;
	primary(result)
}

/// Create a transition at the seam between two contiguous clips and push
/// it as one "Add Transition" undo row (the timeline's wedge drag path).
pub fn add_transition_at_seam(
	p: &ProjectRef,
	prev: NodeId,
	next: NodeId,
	half: Rational,
) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::AddTransitionAtSeam {
		previous: sdk_node(prev),
		next: sdk_node(next),
		half: sdk_rational(half),
	})?;
	primary(result)
}

/// Create a SINGLE-SIDED transition on one edge of `clip` (PR-style: a
/// head fade-in when nothing precedes the clip, a tail fade-out when
/// nothing follows it) and push it as one "Add Transition" undo row.
/// Only the clip itself is wired (`in_block_in` for the head,
/// `out_block_in` for the tail); the renderer blends the open side
/// against black. An edge that already carries a transition is an error.
pub fn add_transition_at_edge(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	length: Rational,
) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::AddTransitionAtEdge {
		clip: sdk_node(clip),
		start_edge,
		length: sdk_rational(length),
	})?;
	primary(result)
}

/// Add a default transition at every contiguous seam around the selected
/// clips, plus BOTH ends of any clip with no junction at all (the
/// Ctrl+Shift+D action / the timeline clip-menu item — PR's
/// apply-to-edit-points semantics, extended to lone clips).
///
/// `half` is half of the default transition length: a junction transition
/// spans `half` on either side of its seam, a head/tail (single-sided)
/// transition spans `2*half` into its own clip. Every selected clip
/// contributes the seam before it and the seam after it (a neighbor
/// counts only when it is a clip block that touches the clip exactly,
/// shared seams built once), plus a head transition when nothing
/// precedes it and a tail transition when nothing follows it. Seams and
/// edges that cannot take a transition (an existing one) are skipped.
/// The whole batch is one "Add Transition" undo row; a batch that builds
/// nothing is an error, so the action never leaves an empty row. Returns
/// the number of transitions created.
///
/// `seq` scopes the action: a selected block whose track list belongs to
/// another sequence is ignored.
pub fn add_default_transition(
	p: &ProjectRef,
	seq: NodeId,
	clip_blocks: &[NodeId],
	half: Rational,
) -> Result<usize, String> {
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::AddDefaultTransition {
		clips: clip_blocks.iter().copied().map(sdk_node).collect(),
		half: sdk_rational(half),
	})?;
	Ok(result.value.max(0) as usize)
}

/// Resize one wedge of the transition attached to `clip` (the timeline
/// widget's wedge drag, its `TransitionChanged` event).
///
/// `start_edge` names the wedge by the clip it is drawn on: the start wedge
/// is the incoming transition's `out_offset`, the end wedge the outgoing
/// one's `in_offset`. The width is clamped to `frame..=clip length` — one
/// frame at the smallest, and never wider than the clip it eats into — and
/// written through the engine's transition-offset command,
/// so one "Transition Length" undo row covers the drag. The seam does not
/// move. Returns the width actually written.
pub fn set_transition_length(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	frame: Rational,
	new_length: Rational,
) -> Result<Rational, String> {
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::SetTransitionLength {
		clip: sdk_node(clip),
		start_edge,
		frame: sdk_rational(frame),
		length: sdk_rational(new_length),
	})?;
	result
		.rational
		.map(from_sdk_rational)
		.ok_or_else(|| "the transition length edit returned no length".to_string())
}

/// Places ONE video clip fed by a sequence node — a nested-sequence clip
/// (drag a sequence entry from the project explorer onto the timeline).
/// The clip reads the sequence's output during playback; the sequence's
/// own tracks stay in the source sequence (a frame, not a folder of
/// tracks, on the host timeline).
pub fn place_nested_sequence_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	source_seq: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid clip range (need 0 <= in < out)".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(host_seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PlaceNestedSequenceClip {
		source: sdk_node(source_seq),
		track_index,
		in_ts,
		out_ts,
	})?;
	primary(result)
}

/// Places ONE video clip fed by the text generator `text` (drag a text
/// entry from the project explorer onto the timeline) — a generator clip,
/// not a decode of media: the clip reads the generator's output through
/// `tex_in`, so the text re-renders at whatever time the clip spans.
/// Undoable as one "Add Clip". `in_ts`/`out_ts` are frame timestamps in
/// the host sequence's frame-rate timebase; a clip dragged shorter or
/// longer than the drop's default simply re-spans the same generator.
pub fn place_text_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	text: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(host_seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PlaceTextClip {
		text: sdk_node(text),
		track_index,
		in_ts,
		out_ts,
	})?;
	primary(result)
}

/// Place a clip fed by a GENERATOR node (checkerboard / color bars /
/// text / shape / solid…) at `(track_index, [in_ts, out_ts))` — the
/// effect-library drag-to-timeline counterpart of [`place_text_clip`]:
/// one undo row covers the placement and the generator→`tex_in` edge.
/// The generator must already be in the project; the clip is labelled
/// after it. Returns the clip's node id.
pub fn place_generator_clip(
	p: &ProjectRef,
	host_seq: NodeId,
	generator: NodeId,
	track_index: usize,
	in_ts: i64,
	out_ts: i64,
) -> Result<NodeId, String> {
	if in_ts < 0 || out_ts <= in_ts {
		return Err("invalid clip range (need 0 <= in < out)".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(host_seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PlaceGeneratorClip {
		generator: sdk_node(generator),
		track_index,
		in_ts,
		out_ts,
	})?;
	primary(result)
}

/// Place linked clips of one footage on several tracks in ONE undoable
/// "Add Clip" entry (the NLE A/V-drop: a video-with-audio file lands as
/// a video clip plus a linked audio clip). `placements` lists the
/// `(track kind, track index)` targets in order; every clip shares the
/// same timeline range and media-in, and all clips are linked both ways
/// (C++ `block_links_` semantics: grouped edits like split/ripple apply
/// to the whole group).
pub fn place_footage_clips_linked(
	p: &ProjectRef,
	seq: NodeId,
	footage: NodeId,
	placements: &[(TrackKind, usize)],
	in_ts: i64,
	out_ts: i64,
	media_in_ts: i64,
) -> Result<Vec<NodeId>, String> {
	if placements.len() < 2 {
		return Err("linked placement needs at least two tracks".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PlaceFootageClipsLinked {
		footage: sdk_node(footage),
		placements: placements
			.iter()
			.map(|(kind, index)| (sdk_track_kind(*kind), *index))
			.collect(),
		in_ts,
		out_ts,
		media_in_ts,
	})?;
	Ok(result
		.ids
		.into_iter()
		.filter_map(|node| id_of(node.raw()))
		.collect())
}

/// Split `clip` at `time_ts` (a frame timestamp strictly inside the
/// clip's range), undoable "Split Clip" (the module's
/// `BlockSplitCommand`).
pub fn split_clip(p: &ProjectRef, clip: NodeId, time_ts: i64) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SplitClip {
			clip: sdk_node(clip),
			time_ts,
		},
	)
}

/// Split every clip of `blocks` at `time_ts` as ONE undoable
/// `BlockSplitPreservingLinksCommand` (the Cmd+K path): originally linked
/// pairs that are both split get their rear halves linked too — the
/// per-block plain split left the rear half of an A/V pair unlinked.
/// Blocks whose range does not strictly contain the time are skipped by
/// the command itself.
pub fn split_clips_preserving_links(
	p: &ProjectRef,
	blocks: &[NodeId],
	time_ts: i64,
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SplitClipsPreservingLinks {
			clips: blocks.iter().copied().map(sdk_node).collect(),
			time_ts,
		},
	)
}

/// Trim `clip`'s timeline range to `[new_in_ts, new_out_ts)` (undoable
/// "Trim Clip"; the facade's `oakengine_clip_trim` semantics: one end at
/// a time, trim-in anchors the OUT (its in moves and the media window
/// follows), trim-out anchors the IN (its out moves, media untouched) —
/// the same net anchors as the module's own `BlockTrimCommand`).
pub fn trim_clip(
	p: &ProjectRef,
	clip: NodeId,
	new_in_ts: i64,
	new_out_ts: i64,
) -> Result<(), String> {
	if new_in_ts < 0 || new_out_ts <= new_in_ts {
		return Err("invalid trim range (need 0 <= new_in < new_out)".to_string());
	}
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::TrimClip {
			clip: sdk_node(clip),
			new_in_ts,
			new_out_ts,
		},
	)
}

/// Ripple-trim `clip` so its `start_edge` lands on `new_frame` (undoable
/// "Ripple Trim Clip"): the trimmed clip's media anchor is kept (trim-in
/// anchors the OUT, trim-out anchors the IN, exactly like [`trim_clip`]),
/// and every block after it on the same track shifts rigidly by the same
/// delta — no gap opens behind the trimmed edge, the tail just follows.
pub fn ripple_trim_clip(
	p: &ProjectRef,
	clip: NodeId,
	start_edge: bool,
	new_frame: i64,
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RippleTrimClip {
			clip: sdk_node(clip),
			start_edge,
			new_frame,
		},
	)
}

/// Roll-edit the shared boundary of adjacent clips `a`/`b` (both on
/// `track`) to `new_frame` (undoable "Roll Edit"): the boundary moves
/// without disturbing anything else on the track — the module's own
/// `BlockTrimCommand` in roll-edit mode trims `a` out-anchored and
/// compensates `b` in-anchored, so both media anchors are preserved.
pub fn roll_edit(
	p: &ProjectRef,
	track: NodeId,
	a: NodeId,
	b: NodeId,
	new_frame: i64,
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RollEdit {
			track: sdk_node(track),
			a: sdk_node(a),
			b: sdk_node(b),
			new_frame,
		},
	)
}

/// Slide `clip` so its in point becomes `new_start` (undoable "Slide
/// Clip"): the clip's own media window and length are untouched — the
/// in point moves and the out follows — while the adjacent blocks are
/// shortened/lengthened to fill the gap or absorb the overlap (the left
/// neighbor's out moves to `new_start`, the right neighbor's in follows
/// the clip's new out). A neighbor that would collapse to a negative
/// length is left alone, leaving the hole open.
pub fn slide_clip(p: &ProjectRef, clip: NodeId, new_start: i64) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SlideClip {
			clip: sdk_node(clip),
			new_start,
		},
	)
}

/// Slip `clip` so its media in-point becomes `new_media_in` (undoable
/// "Slip Clip"): the timeline range and length stay put, only the media
/// window slides inside the clip (clamped to frame 0).
pub fn slip_clip(p: &ProjectRef, clip: NodeId, new_media_in: i64) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SlipClip {
			clip: sdk_node(clip),
			new_media_in,
		},
	)
}

/// Move `clip` within its track so its in point becomes `new_in_ts`
/// (undoable "Move Clip"; the module's `TrackMoveBlockCommand` — the old
/// spot becomes a gap, length and media-in are preserved). A negative
/// target clamps to frame 0 (the NLE drop-past-the-start behavior).
pub fn move_clip(p: &ProjectRef, clip: NodeId, new_in_ts: i64) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::MoveClip {
			clip: sdk_node(clip),
			new_in_ts,
		},
	)
}

/// Move `clip` to a different track at `new_in_ts` (undoable "Move Clip
/// to Track", one row): the source spot becomes a gap, the block's in
/// point is re-homed, and the clip is placed on the destination track
/// (the facade's `oakengine_sequence_move_clip_to_track` composition).
pub fn move_clip_to_track(
	p: &ProjectRef,
	clip: NodeId,
	dest_track: NodeId,
	new_in_ts: i64,
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::MoveClipToTrack {
			clip: sdk_node(clip),
			track: sdk_node(dest_track),
			new_in_ts,
		},
	)
}

/// Move `clip` to `new_in_ts` (`dest_track` when the gesture crosses
/// tracks) while every clip in `linked` follows in lockstep: each linked
/// clip keeps its own track and moves by the same frame offset. The whole
/// group lands as ONE undoable "Move Clip" entry (C++ `block_links_`
/// semantics — grouped edits apply to the whole group). The shared delta
/// is clamped so no clip of the group lands before frame 0 (a drag past
/// the timeline start pins the group at 0 instead of failing).
pub fn move_clip_with_links(
	p: &ProjectRef,
	clip: NodeId,
	dest_track: Option<NodeId>,
	new_in_ts: i64,
	linked: &[NodeId],
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::MoveClipWithLinks {
			clip: sdk_node(clip),
			track: dest_track.map(sdk_node),
			new_in_ts,
			linked: linked.iter().copied().map(sdk_node).collect(),
		},
	)
}

/// Link or unlink a set of clips as ONE undoable entry (the C++
/// `TimelineWidget::toggle_links_on_selected` → `oakengine_clip_set_linked`
/// composition): linking connects every pair of the set, unlinking clears
/// every link among them. The undo restores the exact prior topology among
/// the set (links to nodes OUTSIDE the set are untouched, like the C++
/// command's).
pub fn set_clips_linked(p: &ProjectRef, blocks: &[NodeId], linked: bool) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetClipsLinked {
			clips: blocks.iter().copied().map(sdk_node).collect(),
			linked,
		},
	)
}

/// Delete `clip` leaving a gap (undoable "Delete Clips"; the facade's
/// single-clip `oakengine_sequence_delete_clips` composition).
pub fn delete_clip(p: &ProjectRef, clip: NodeId) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::DeleteClip {
			clip: sdk_node(clip),
		},
	)
}

/// Delete `clip` and ripple the following content left (undoable
/// "Ripple Delete Clip"; the module's `TrackRippleRemoveAreaCommand`
/// over the clip's range).
pub fn ripple_delete_clip(p: &ProjectRef, clip: NodeId) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RippleDeleteClip {
			clip: sdk_node(clip),
		},
	)
}

/// Undoable removal of `node` from the project graph ("Remove Node"; the
/// module's remove command drops incident edges and restores the entry on
/// undo). The sequence node itself (the graph's output) cannot be
/// removed.
pub fn remove_node(p: &ProjectRef, node: NodeId) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RemoveNode {
			node: sdk_node(node),
		},
	)
}

// ---------------------------------------------------------------------------
// Test serialization
// ---------------------------------------------------------------------------

/// A process-wide test lock: the app's tests share the oakundo global
/// stack, the oak_core config store and the codec decode sessions, so any
/// test touching them serializes on this lock.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Clipboard (Cut / Copy / Paste)
// ---------------------------------------------------------------------------

/// One clip captured in the clipboard (Cut/Copy). Everything needed to
/// re-place it on a timeline is here; effects/multicam contexts are not
/// modeled yet (footage clips carry their block core only).
#[derive(Clone, Debug)]
pub struct ClipboardClip {
	/// The footage node the clip decodes.
	pub footage: NodeId,
	/// Track type the clip was on.
	pub kind: TrackKind,
	/// Per-type index of the track it was on (re-used on paste when the
	/// track still exists).
	pub track_index: usize,
	/// Media in-point in frame timestamps.
	pub media_in_ts: i64,
	/// Timeline in-point in frame timestamps.
	pub start_ts: i64,
	/// Duration in frame timestamps.
	pub length_ts: i64,
	/// Playback speed.
	pub speed: f64,
}

/// Capture the selected clips into clipboard form (`Copy`; `Cut` copies
/// then deletes). Clips without a footage upstream are skipped.
pub fn copy_clips(p: &ProjectRef, clips: &[NodeId]) -> Vec<ClipboardClip> {
	let Ok(session) = super::real::editor_session_for_project(p) else {
		return Vec::new();
	};
	let Ok(result) = session.execute(quercus_engine_rs::edit::EditCommand::CopyClips {
		clips: clips.iter().copied().map(sdk_node).collect(),
	}) else {
		return Vec::new();
	};
	result
		.clipboard
		.into_iter()
		.filter_map(|clip| {
			Some(ClipboardClip {
				footage: id_of(clip.footage.raw())?,
				kind: track_kind_from_sdk(clip.kind),
				track_index: clip.track_index,
				media_in_ts: clip.media_in_ts,
				start_ts: clip.start_ts,
				length_ts: clip.length_ts,
				speed: clip.speed,
			})
		})
		.collect()
}

/// Paste clipboard clips at `playhead_ts` (frame timestamps), undoable
/// "Paste". The first clip's in-point moves to the playhead; the others
/// keep their relative offsets. Tracks are reused by kind+index (falling
/// back to the last track of the kind). Clips that were linked stay
/// linked inside the pasted group.
pub fn paste_clips(
	p: &ProjectRef,
	seq: NodeId,
	items: &[ClipboardClip],
	playhead_ts: i64,
) -> Result<Vec<NodeId>, String> {
	let _ = (seq, items);
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PasteClips {
		playhead_ts,
	})?;
	Ok(result
		.ids
		.into_iter()
		.filter_map(|node| id_of(node.raw()))
		.collect())
}

// ---------------------------------------------------------------------------
// Edit-menu operations (启用/禁用、速度、重命名、重复、插入粘贴、区域删除、
// 滚动编辑到播放头、嵌套)
// ---------------------------------------------------------------------------

/// Enable or disable blocks (undoable "Enable/Disable Clips"): blocks
/// already in the target state are skipped; the batch is one undo entry.
pub fn set_blocks_enabled(p: &ProjectRef, blocks: &[NodeId], enabled: bool) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetBlocksEnabled {
			clips: blocks.iter().copied().map(sdk_node).collect(),
			enabled,
		},
	)
}

/// Set the speed of clips, span unchanged (undoable "Speed/Duration"):
/// the media mapping `media = (time - in) * speed` follows immediately.
pub fn set_clips_speed(p: &ProjectRef, clips: &[NodeId], speed: f64) -> Result<(), String> {
	if !(speed.is_finite() && speed.abs() > 1e-9) {
		return Err("speed must be a non-zero finite number".to_string());
	}
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::SetClipsSpeed {
			clips: clips.iter().copied().map(sdk_node).collect(),
			speed,
		},
	)
}

/// Rename blocks (set the node label; undoable "Rename").
pub fn rename_blocks(p: &ProjectRef, blocks: &[NodeId], name: &str) -> Result<(), String> {
	if name.trim().is_empty() {
		return Err("empty name".to_string());
	}
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RenameBlocks {
			clips: blocks.iter().copied().map(sdk_node).collect(),
			name: name.to_string(),
		},
	)
}

/// Duplicate `clips` right after the selection's end (undoable
/// "Duplicate"): clipboard copy + paste anchored at the latest out point
/// (the C++ Edit > Duplicate semantics).
pub fn duplicate_clips(
	p: &ProjectRef,
	seq: NodeId,
	clips: &[NodeId],
) -> Result<Vec<NodeId>, String> {
	let _ = seq;
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::DuplicateClips {
		clips: clips.iter().copied().map(sdk_node).collect(),
	})?;
	Ok(result
		.ids
		.into_iter()
		.filter_map(|node| id_of(node.raw()))
		.collect())
}

/// Paste with ripple (undoable "Paste Insert"): every unlocked block
/// starting at or after the playhead shifts right by the pasted span,
/// then the clipboard lands at the playhead — all in ONE undo entry.
pub fn paste_clips_insert(
	p: &ProjectRef,
	seq: NodeId,
	items: &[ClipboardClip],
	playhead_ts: i64,
) -> Result<Vec<NodeId>, String> {
	let _ = (seq, items);
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::PasteClipsInsert {
		playhead_ts,
	})?;
	Ok(result
		.ids
		.into_iter()
		.filter_map(|node| id_of(node.raw()))
		.collect())
}

/// Delete the `[in_ts, out_ts)` area on every unlocked track of the
/// sequence (undoable). Ripple closes the gap (the module's
/// `TrackRippleRemoveAreaCommand` per track); non-ripple reopens a gap of
/// the same size (ripple-remove + shift the tail back right by the span).
pub fn delete_area(
	p: &ProjectRef,
	seq: NodeId,
	in_ts: i64,
	out_ts: i64,
	ripple: bool,
) -> Result<(), String> {
	if out_ts <= in_ts {
		return Err("invalid area (need in < out)".to_string());
	}
	let session = super::real::editor_session_for_project(p)?;
	let _ = session.set_current_sequence(seq);
	session
		.execute(quercus_engine_rs::edit::EditCommand::DeleteArea {
			in_ts,
			out_ts,
			ripple,
		})
		.map(|_| ())
}

/// Roll the boundary at `clip`'s edge to the playhead (undoable
/// "Roll Edit"): `at_in` rolls the boundary with the previous block,
/// otherwise the one with the next block. No-op when the playhead is
/// outside the two blocks' combined span or there is no neighbor.
pub fn roll_clip_edge_to(
	p: &ProjectRef,
	clip: NodeId,
	at_in: bool,
	frame_ts: i64,
) -> Result<(), String> {
	execute(
		p,
		quercus_engine_rs::edit::EditCommand::RollClipEdgeTo {
			clip: sdk_node(clip),
			at_in,
			frame_ts,
		},
	)
}

/// Nest `clips` into a new sequence and replace them with one nested
/// clip spanning their range ("嵌套"): the new sequence (named after the
/// first clip) lands in the root folder and holds the clips at their
/// relative offsets; the originals are deleted from the host sequence.
///
/// The steps are separate undo entries (the sequence factory and the
/// placement machinery predate command composition).
pub fn nest_clips(p: &ProjectRef, seq: NodeId, clips: &[NodeId]) -> Result<NodeId, String> {
	let _ = seq;
	let session = super::real::editor_session_for_project(p)?;
	let result = session.execute(quercus_engine_rs::edit::EditCommand::NestClips {
		clips: clips.iter().copied().map(sdk_node).collect(),
	})?;
	primary(result)
}

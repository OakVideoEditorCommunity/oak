// Oak - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The CLI's engine adapter: a pure `quercus-engine-rs` consumer.
//!
//! Projects are opened as an [`EditorSession`]; queries use the SDK's
//! owned `Project` DTO methods, edits are SDK `EditCommand`s, frames and
//! audio are SDK tasks/buffers, and export is the unified task API. The CLI
//! links no other Oak crate.

use std::path::Path;

use quercus_engine_rs::editor::{EditorSession, TrackKind};
use quercus_engine_rs::node::{NodeId, NodeKind, Project};
use quercus_engine_rs::render::{self, RenderBackend};
use quercus_engine_rs::task::{
	self, EncodingParams, TaskState,
};
use quercus_engine_rs::types::{Rational, TimeRange};

/// The CLI's shared project handle (SDK, not an internal graph).
pub type ProjectRef = Project;

/// A loaded project plus the live session used for edits.
pub struct LoadedProject {
	/// The live editor session.
	pub session: EditorSession,
	/// The session's project handle (same document).
	pub project: Project,
}

/// Opens `path` through the SDK storage registry (`.ove` in practice).
pub fn load_project(path: &str) -> Result<LoadedProject, String> {
	let session = EditorSession::open(path).map_err(|error| error.to_string())?;
	let project = session.project();
	Ok(LoadedProject { session, project })
}

/// The project display name.
///
/// The name stored in the project document (shown in the UI and in CLI
/// output); "" when the name is unset. Distinct from
/// [`project_filename`], which is the on-disk path.
pub fn project_name(project: &Project) -> String {
	project.name()
}

/// The recorded project filename ("" for an unsaved project).
///
/// This is the path the project was opened from or last saved to, not a
/// display label; a project that has never been saved returns "". The CLI
/// falls back to the user-supplied path in that case.
pub fn project_filename(project: &Project) -> String {
	project.filename()
}

/// Whether the project has unsaved changes.
///
/// Reads the underlying project document's modified flag (not a CLI-local
/// dirty bit); true after any edit that has not been saved.
pub fn project_modified(project: &Project) -> bool {
	project.modified()
}

/// Every sequence node, in arena order.
///
/// Arena order is an implementation detail that changes as nodes are added
/// or removed — iterate with it, but do not treat it as stable. The filter
/// is the coarse [`NodeKind::Sequence`]; the CLI's default-sequence choice
/// is simply the first entry.
pub fn sequence_ids(project: &Project) -> Vec<NodeId> {
	project
		.node_ids()
		.into_iter()
		.filter(|id| project.node_kind(*id) == Some(NodeKind::Sequence))
		.collect()
}

/// Every footage node, in arena order.
///
/// Arena order is an implementation detail that changes as nodes are added
/// or removed — iterate with it, but do not treat it as stable. The filter
/// is the coarse [`NodeKind::Footage`]; pairs with [`node_label`] and
/// [`footage_filename`] for listing.
pub fn footage_ids(project: &Project) -> Vec<NodeId> {
	project
		.node_ids()
		.into_iter()
		.filter(|id| project.node_kind(*id) == Some(NodeKind::Footage))
		.collect()
}

/// A node's display label ("" when unset).
///
/// `id` is any node token. The label is user-visible but not unique, and may
/// be empty for generated nodes or unknown tokens; it is the same string the
/// UI shows, not a stable identifier.
pub fn node_label(project: &Project, id: NodeId) -> String {
	project.node_label(id)
}

/// A footage node's media path ("" when unavailable).
///
/// `id` must name a footage node. Returns the absolute source file path; an
/// unset path, a non-footage id or non-UTF-8 bytes produce "", which callers
/// render as an empty path rather than failing the whole listing.
pub fn footage_filename(project: &Project, id: NodeId) -> String {
	project.footage_filename(id).unwrap_or_default()
}

/// The sequence's content length (rational seconds).
///
/// `id` must name the sequence node. The length is the extent of the
/// content on its tracks, falling back to the last requested length when
/// the content is empty; a non-sequence returns the null rational
/// (`den == 0`).
pub fn sequence_length(project: &Project, id: NodeId) -> Rational {
	project.sequence_length(id)
}

/// The sequence's frame rate as a rational.
///
/// `id` must name the sequence node. A non-sequence, or a sequence without
/// video params, returns the null rational (`den == 0`); callers must not
/// divide by its denominator without checking.
pub fn sequence_frame_rate(project: &Project, id: NodeId) -> Rational {
	project.sequence_frame_rate(id)
}

/// The sequence's output geometry `(width, height)` in pixels.
///
/// `id` must name the sequence node. Returns `(0, 0)` when the node is not
/// a sequence or has no video params; [`export_sequence`] treats that as
/// "no video parameters" and refuses to export.
pub fn sequence_geometry(project: &Project, id: NodeId) -> (i32, i32) {
	project.sequence_geometry(id).unwrap_or((0, 0))
}

/// `(video, audio, subtitle)` track counts.
///
/// Sums the track counts of every track list under sequence `id`, bucketing
/// by track-list kind (0 video, 1 audio, 2 subtitle; unknown kinds are
/// ignored). A non-sequence id yields `(0, 0, 0)`, so callers cannot tell
/// "empty sequence" from "not a sequence" here — check the node kind first
/// when that distinction matters.
pub fn sequence_track_counts(project: &Project, id: NodeId) -> (i64, i64, i64) {
	let mut counts = (0i64, 0i64, 0i64);
	for list_index in 0..project.sequence_track_list_count(id) {
		let list = project.sequence_track_list_at(id, list_index);
		let count = project.tracklist_track_count(list) as i64;
		match project.tracklist_kind(list) {
			0 => counts.0 += count,
			1 => counts.1 += count,
			2 => counts.2 += count,
			_ => {}
		}
	}
	counts
}

/// The sequence playhead (rational seconds).
///
/// `id` must name the sequence node; the value is relative to the sequence
/// start. A non-sequence id returns the null rational (`den == 0`), which
/// callers must check before converting. Thin wrapper over
/// `Project::sequence_playhead`.
pub fn sequence_playhead(project: &Project, id: NodeId) -> Rational {
	project.sequence_playhead(id)
}

/// The CLI's rendered frame: tightly packed RGBA F32 bytes.
pub struct RenderedFrame {
	/// Width in pixels.
	pub width: i32,
	/// Height in pixels.
	pub height: i32,
	/// `oak_core` pixel-format code (4 = F32 RGBA; kept for the PPM writer).
	pub format: i32,
	/// Bytes per scanline.
	pub linesize: i32,
	/// Pixel data (`linesize * height` bytes).
	pub data: Vec<u8>,
}

/// Initializes the render manager and the engine backends the CLI uses.
pub fn render_manager_init() -> Result<(), String> {
	let _ = quercus_engine_rs::state::color_set_up_default_config(None);
	render::textengine_install();
	let _ = quercus_engine_rs::audio::create_instance();
	let _ = quercus_engine_rs::ofx::scan();
	quercus_engine_rs::ofx::register_plugin_nodes();
	quercus_engine_rs::ofx::install_render_executor();
	if !render::manager_init(RenderBackend::Threads) && !render::manager_is_initialized() {
		return Err("cannot initialize the render manager".to_string());
	}
	Ok(())
}

/// Shuts the render manager down.
pub fn render_manager_shutdown() {
	render::manager_shutdown();
}

/// Renders one sequence frame at `time` into RGBA F32 bytes.
///
/// `viewer` is the sequence (viewer) node, `time` the rational frame time
/// in sequence seconds, and `width`/`height` the output size in pixels. The
/// returned frame is tightly packed (`linesize == width * 16`, format code
/// 4 = F32 RGBA); the SDK's f32 samples are converted to little-endian
/// bytes here so the PPM writer can consume them.
///
/// # Errors
///
/// "invalid frame size" for a non-positive dimension and "frame render
/// failed" when the SDK renderer returns no frame.
pub fn render_frame(
	project: &Project,
	viewer: NodeId,
	time: Rational,
	width: i32,
	height: i32,
) -> Result<RenderedFrame, String> {
	if width <= 0 || height <= 0 {
		return Err("invalid frame size".to_string());
	}
	let samples = render::render_frame_f32(project, viewer, time, (width as u32, height as u32))
		.ok_or_else(|| "frame render failed".to_string())?;
	let mut data = Vec::with_capacity(samples.len() * 4);
	for sample in samples {
		data.extend_from_slice(&sample.to_le_bytes());
	}
	Ok(RenderedFrame {
		width,
		height,
		format: 4,
		linesize: width * 16,
		data,
	})
}

/// One rendered audio block.
pub struct RenderedAudio {
	/// Interleaved f32 samples.
	pub data: Vec<f32>,
	/// Sample rate in Hz.
	pub sample_rate: i32,
	/// Channel count.
	pub channel_count: i32,
}

/// Renders `range` of the sequence as 48 kHz stereo.
///
/// `viewer` is the sequence (viewer) node and `range` is the rational time
/// range in sequence seconds. The render runs as a blocking SDK task; the
/// returned block carries the sample rate and channel count the engine
/// actually produced, which the CLI uses when writing its WAV output instead
/// of assuming 48 kHz stereo.
///
/// # Errors
///
/// "audio render task rejected", "audio render failed" or "audio render
/// produced no block", depending on the stage that failed.
pub fn render_audio(
	project: &Project,
	viewer: NodeId,
	range: TimeRange,
) -> Result<RenderedAudio, String> {
	let task = task::render_audio(project, viewer, range, 48_000, 0x3)
		.ok_or_else(|| "audio render task rejected".to_string())?;
	if !task.wait() {
		return Err("audio render failed".to_string());
	}
	let block = task
		.take_audio()
		.ok_or_else(|| "audio render produced no block".to_string())?;
	let info = block.info();
	Ok(RenderedAudio {
		data: block.samples(),
		sample_rate: info.sample_rate,
		channel_count: info.channel_count,
	})
}

/// Exports `sequence` to `out` as H.264 (and AAC when the source has audio).
///
/// `out` is the destination media path; `fr_num`/`fr_den` are the export
/// frame rate numerator/denominator (each floored to 1, so a 0 or negative
/// rate becomes 1/1); `frames` is the export length in frames (negative
/// values clamp to 0) and is converted to seconds as
/// `frames * fr_den / fr_num`. The video size comes from
/// [`sequence_geometry`]; audio is forced to 48 kHz with the CLI's stereo
/// layout. The export runs as a blocking SDK task and writes `out`; an
/// audio-less sequence yields a video-only file.
///
/// # Errors
///
/// Returns "the sequence has no video parameters" when the geometry is
/// missing, the task's own error text when it fails (or "export failed"
/// when the task reports no detail), and "export task rejected" when the
/// SDK refuses to start it.
pub fn export_sequence(
	project: &Project,
	sequence: NodeId,
	out: &str,
	fr_num: i32,
	fr_den: i32,
	frames: i64,
) -> Result<(), String> {
	let (width, height) = sequence_geometry(project, sequence);
	if width <= 0 || height <= 0 {
		return Err("the sequence has no video parameters".to_string());
	}
	let params = EncodingParams {
		format: quercus_engine_rs::codec::Format::MPEG4Video as i32,
		video_enabled: true,
		video_codec: quercus_engine_rs::codec::Codec::H264 as i32,
		video_width: width,
		video_height: height,
		video_time_base_num: fr_den.max(1),
		video_time_base_den: fr_num.max(1),
		video_pixel_format: 0,
		audio_enabled: true,
		audio_codec: quercus_engine_rs::codec::Codec::AAC as i32,
		audio_sample_rate: 48_000,
		audio_channel_layout: 0x3,
		export_length_num: frames.max(0) as i32 * fr_den.max(1),
		export_length_den: fr_num.max(1),
		..EncodingParams::default()
	};
	let task = task::export(project, sequence, out, &params)
		.ok_or_else(|| "export task rejected".to_string())?;
	if !task.wait() || task.state() != TaskState::Succeeded {
		let detail = task.error();
		return Err(if detail.is_empty() {
			"export failed".to_string()
		} else {
			detail
		});
	}
	Ok(())
}

/// The default track index for a sequence and track kind.
///
/// Scans the sequence's track lists in order and returns `Some(0)` when the
/// first list of `kind` (video, audio or subtitle) has at least one track;
/// `None` when the sequence has no non-empty list of that kind. The index is
/// always 0 — the helper reports only whether a usable list exists.
pub fn default_track(project: &Project, sequence: NodeId, kind: TrackKind) -> Option<usize> {
	for list_index in 0..project.sequence_track_list_count(sequence) {
		let list = project.sequence_track_list_at(sequence, list_index);
		let matches = match kind {
			TrackKind::Video => 0,
			TrackKind::Audio => 1,
			TrackKind::Subtitle => 2,
		};
		if project.tracklist_kind(list) == matches && project.tracklist_track_count(list) > 0 {
			return Some(0);
		}
	}
	None
}

/// Joins `dir` and `name` into the CLI's output path string.
///
/// `dir` is the output directory and `name` the file (or relative subpath),
/// appended with the platform separator; an absolute `name` replaces `dir`
/// entirely, as [`std::path::Path::join`] does. Non-UTF-8 paths are
/// converted lossily. Does not touch the filesystem.
pub fn join(dir: &str, name: &str) -> String {
	Path::new(dir).join(name).to_string_lossy().into_owned()
}

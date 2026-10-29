// Oak - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The CLI's engine adapter: a pure `oak-engine-rust-sdk` consumer.
//!
//! Projects are opened as an [`EditorSession`]; queries use the SDK's
//! owned `Project` DTO methods, edits are SDK `EditCommand`s, frames and
//! audio are SDK tasks/buffers, and export is the unified task API. The CLI
//! links no other Oak crate.

use std::path::Path;

use oak_engine_rust_sdk::editor::{EditorSession, TrackKind};
use oak_engine_rust_sdk::node::{NodeId, NodeKind, Project};
use oak_engine_rust_sdk::render::{self, RenderBackend};
use oak_engine_rust_sdk::task::{
	self, EncodingParams, TaskState,
};
use oak_engine_rust_sdk::types::{Rational, TimeRange};

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
pub fn project_name(project: &Project) -> String {
	project.name()
}

/// The recorded project filename.
pub fn project_filename(project: &Project) -> String {
	project.filename()
}

/// Whether the project has unsaved changes.
pub fn project_modified(project: &Project) -> bool {
	project.modified()
}

/// Every sequence node, in arena order.
pub fn sequence_ids(project: &Project) -> Vec<NodeId> {
	project
		.node_ids()
		.into_iter()
		.filter(|id| project.node_kind(*id) == Some(NodeKind::Sequence))
		.collect()
}

/// Every footage node, in arena order.
pub fn footage_ids(project: &Project) -> Vec<NodeId> {
	project
		.node_ids()
		.into_iter()
		.filter(|id| project.node_kind(*id) == Some(NodeKind::Footage))
		.collect()
}

/// A node's display label.
pub fn node_label(project: &Project, id: NodeId) -> String {
	project.node_label(id)
}

/// A footage node's media path.
pub fn footage_filename(project: &Project, id: NodeId) -> String {
	project.footage_filename(id).unwrap_or_default()
}

/// The sequence's content length (rational seconds).
pub fn sequence_length(project: &Project, id: NodeId) -> Rational {
	project.sequence_length(id)
}

/// The sequence's frame rate.
pub fn sequence_frame_rate(project: &Project, id: NodeId) -> Rational {
	project.sequence_frame_rate(id)
}

/// The sequence's output geometry.
pub fn sequence_geometry(project: &Project, id: NodeId) -> (i32, i32) {
	project.sequence_geometry(id).unwrap_or((0, 0))
}

/// `(video, audio, subtitle)` track counts.
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
	let _ = oak_engine_rust_sdk::state::color_set_up_default_config(None);
	render::textengine_install();
	let _ = oak_engine_rust_sdk::audio::create_instance();
	let _ = oak_engine_rust_sdk::ofx::scan();
	oak_engine_rust_sdk::ofx::register_plugin_nodes();
	oak_engine_rust_sdk::ofx::install_render_executor();
	if !render::manager_init(RenderBackend::Default) && !render::manager_is_initialized() {
		return Err("cannot initialize the render manager".to_string());
	}
	Ok(())
}

/// Shuts the render manager down.
pub fn render_manager_shutdown() {
	render::manager_shutdown();
}

/// Renders one sequence frame at `time` into RGBA F32 bytes.
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
		format: oak_engine_rust_sdk::codec::Format::MPEG4Video as i32,
		video_enabled: true,
		video_codec: oak_engine_rust_sdk::codec::Codec::H264 as i32,
		video_width: width,
		video_height: height,
		video_time_base_num: fr_den.max(1),
		video_time_base_den: fr_num.max(1),
		video_pixel_format: 0,
		audio_enabled: true,
		audio_codec: oak_engine_rust_sdk::codec::Codec::AAC as i32,
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

/// The default video track index for a sequence (0 when it has one).
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

/// Joins `dir` and `name` (the CLI's output helper).
pub fn join(dir: &str, name: &str) -> String {
	Path::new(dir).join(name).to_string_lossy().into_owned()
}

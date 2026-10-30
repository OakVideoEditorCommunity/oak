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

//! SDK preview/export adapters and the app's pixel/display conversion.
//!
//! Montage construction, ticket scheduling, worker-frame ownership, audio
//! mixing and export execution live in the external `quercus-engine` crate. Synchronous wait helpers
//! are called only by background workers and tests.

use std::sync::mpsc;

use gpui_kit::RenderImage;
use quercus_engine_rs::types::PixelFormat;
use quercus_engine_rs::state::Config;
use quercus_engine_rs::types::Rational;
use quercus_engine_rs::node::NodeId;

use super::engine::{ExportEvent, ExportSession};
use super::frames::{bgra_bytes_to_render_image, f32_rgba_to_bgra_image};
use super::graphops::ProjectRef;
use super::scopes::{analyze_bgra8, analyze_f32_rgba, ScopeData};

/// The pipeline's F32 pixel-format code.
pub const PIXEL_FORMAT_F32: i32 = 4;
/// The packed BGRA8 wire-format code.
pub const SLOT_FORMAT_BGRA8: i32 = 100;

/// Brings up the process-wide render manager through the SDK.
pub fn ensure_render_manager() -> bool {
	if quercus_engine_rs::render::manager_is_initialized() {
		return true;
	}
	if !quercus_engine_rs::render::manager_init(
		quercus_engine_rs::render::RenderBackend::Default,
	) && !quercus_engine_rs::render::manager_is_initialized()
	{
		log::error!("render manager init failed");
	}
	quercus_engine_rs::render::manager_is_initialized()
}

/// Whether preview decoding may substitute proxy media.
pub fn use_proxy_media() -> bool {
	Config::global().get_bool("UseProxyMedia", true)
}

/// An SDK-owned frame and its display conversion.
pub enum RenderedFrame {
	Sdk(quercus_engine_rs::preview_video::VideoFrame),
}

impl RenderedFrame {
	pub fn width(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => frame.info().width,
		}
	}

	pub fn height(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => frame.info().height,
		}
	}

	pub fn format(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => match frame.info().pixel_format {
				quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba => PIXEL_FORMAT_F32,
				quercus_engine_rs::preview_video::VideoPixelFormat::Bgra8 => SLOT_FORMAT_BGRA8,
			},
		}
	}

	pub fn is_shm(&self) -> bool {
		match self {
			RenderedFrame::Sdk(frame) => {
				frame.info().origin == quercus_engine_rs::preview_video::FrameOrigin::SharedMemory
			}
		}
	}

	pub fn is_gpu(&self) -> bool {
		match self {
			RenderedFrame::Sdk(frame) => {
				frame.info().origin == quercus_engine_rs::preview_video::FrameOrigin::Gpu
			}
		}
	}

	/// Builds the viewer image, matching scope data and optional F32 display
	/// samples used by the RGBA16F upload path.
	pub fn to_display(&self) -> Option<(RenderImage, ScopeData, Option<Vec<f32>>)> {
		match self {
			RenderedFrame::Sdk(frame) => {
				let info = frame.info();
				let (w, h) = (info.width.max(0) as u32, info.height.max(0) as u32);
				let data = frame.bytes();
				match info.pixel_format {
					quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba => {
						let mut samples =
							repack_f32_rows(info.width, info.height, info.linesize, &data)?;
						apply_output_node_f32(&mut samples);
						let scope = analyze_f32_rgba(w, h, &samples);
						super::displaycolor::apply_f32_rgba(&mut samples, (w * h) as i64);
						let image = f32_rgba_to_bgra_image(w, h, &samples);
						Some((image, scope, Some(samples)))
					}
					quercus_engine_rs::preview_video::VideoPixelFormat::Bgra8 => {
						let scope = analyze_bgra8(w, h, &data);
						let mut owned = data;
						super::displaycolor::apply_bgra8(&mut owned, (w * h) as i64);
						let image = bgra_bytes_to_render_image(w, h, &owned)?;
						Some((image, scope, None))
					}
				}
			}
		}
	}
}

/// Applies the project's output transform before display-color conversion.
fn apply_output_node_f32(samples: &mut [f32]) {
	quercus_engine_rs::utils::working_to_display_target(
		samples,
		&quercus_engine_rs::state::color_working_space(),
		&quercus_engine_rs::state::color_output_gamut(),
		&quercus_engine_rs::state::color_output_transfer(),
	);
}

/// Repacks padded F32 RGBA rows into tightly packed samples.
fn repack_f32_rows(width: i32, height: i32, linesize: i32, data: &[u8]) -> Option<Vec<f32>> {
	if width <= 0 || height <= 0 {
		return None;
	}
	let row_bytes = (width * 4 * 4) as usize;
	let linesize = (linesize as usize).max(row_bytes);
	if data.len() < linesize * height as usize {
		return None;
	}
	let mut samples = vec![0.0f32; (width * height * 4) as usize];
	for y in 0..height as usize {
		let row = &data[y * linesize..y * linesize + row_bytes];
		for (i, px) in row.as_chunks::<4>().0.iter().enumerate() {
			let value = f32::from_ne_bytes([px[0], px[1], px[2], px[3]]);
			samples[y * (width as usize) * 4 + i] = value;
		}
	}
	Some(samples)
}

fn validate_geometry(width: i32, height: i32, tb: (i64, i64)) -> Result<(), String> {
	if width <= 0 || height <= 0 || tb.0 <= 0 || tb.1 <= 0 {
		return Err("invalid render geometry or frame rate".to_string());
	}
	Ok(())
}

/// Native sequence geometry scaled by the playback resolution divider.
pub fn full_res_render_size(seq_width: u32, seq_height: u32, divider: u32) -> (i32, i32) {
	let w = seq_width.max(1) as f64;
	let h = seq_height.max(1) as f64;
	let scale = 1.0 / divider.max(1) as f64;
	let width = ((w * scale).round() as u32).max(2) as i32;
	let height = ((h * scale).round() as u32).max(2) as i32;
	(width, height)
}

/// Starts one sequence/footage frame render on an SDK engine worker.
pub fn submit_video_frame(
	p: &ProjectRef,
	viewer: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
	format: Option<PixelFormat>,
) -> Result<quercus_engine_rs::task::Task, String> {
	submit_video_frame_scheduled(
		p, viewer, frame_ts, tb, width, height, format, None,
	)
}

/// Starts a forward-window frame at Playback priority.
pub fn submit_playback_video_frame(
	p: &ProjectRef,
	viewer: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
	format: Option<PixelFormat>,
	playhead: i64,
	generation: u64,
) -> Result<quercus_engine_rs::task::Task, String> {
	submit_video_frame_scheduled(
		p,
		viewer,
		frame_ts,
		tb,
		width,
		height,
		format,
		Some((playhead, generation)),
	)
}

fn submit_video_frame_scheduled(
	p: &ProjectRef,
	viewer: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
	format: Option<PixelFormat>,
	playback: Option<(i64, u64)>,
) -> Result<quercus_engine_rs::task::Task, String> {
	validate_geometry(width, height, tb)?;
	let session = super::real::editor_session_for_project(p)?;
	let pixel_format = if format == Some(PixelFormat::F32) {
		quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba
	} else {
		quercus_engine_rs::preview_video::VideoPixelFormat::Bgra8
	};
	let viewer = quercus_engine_rs::node::NodeId::from_raw(viewer.raw());
	let time = rational_to_sdk(Rational::new(frame_ts * tb.0, tb.1));
	let size = (width as u32, height as u32);
	if let Some((playhead, generation)) = playback {
		quercus_engine_rs::task::render_playback_video(
			&session.project(),
			viewer,
			time,
			size,
			pixel_format,
			frame_ts,
			frame_ts.saturating_sub(playhead).abs(),
			generation,
		)
	} else {
		quercus_engine_rs::task::render_video(
			&session.project(),
			viewer,
			time,
			size,
			pixel_format,
		)
	}
	.ok_or_else(|| "failed to start the video render task".to_string())
}

/// Waits on a background caller and transfers an SDK frame exactly once.
fn wait_video_frame(task: quercus_engine_rs::task::Task) -> Result<RenderedFrame, String> {
	if !task.wait() {
		let error = task.error();
		return Err(if error.is_empty() {
			"video render failed".to_string()
		} else {
			error
		});
	}
	task.take_video()
		.map(RenderedFrame::Sdk)
		.ok_or_else(|| "video render produced no frame".to_string())
}

/// Renders one sequence frame through an SDK task.
pub fn render_sequence_frame(
	p: &ProjectRef,
	seq: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
	format: Option<PixelFormat>,
) -> Result<RenderedFrame, String> {
	wait_video_frame(submit_video_frame(
		p, seq, frame_ts, tb, width, height, format,
	)?)
}

/// Renders one source-footage frame through an SDK task.
pub fn render_footage_frame(
	p: &ProjectRef,
	footage: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
	format: Option<PixelFormat>,
) -> Result<RenderedFrame, String> {
	wait_video_frame(submit_video_frame(
		p, footage, frame_ts, tb, width, height, format,
	)?)
}

/// Renders one single-track multicam angle through an SDK task.
pub fn render_multicam_angle_frame(
	p: &ProjectRef,
	seq: NodeId,
	track: NodeId,
	frame_ts: i64,
	tb: (i64, i64),
	width: i32,
	height: i32,
) -> Result<RenderedFrame, String> {
	validate_geometry(width, height, tb)?;
	let session = super::real::editor_session_for_project(p)?;
	let task = quercus_engine_rs::task::render_track_video(
		&session.project(),
		quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
		quercus_engine_rs::node::NodeId::from_raw(track.raw()),
		rational_to_sdk(Rational::new(frame_ts * tb.0, tb.1)),
		(width as u32, height as u32),
		quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba,
	)
	.ok_or_else(|| "failed to start the multicam render task".to_string())?;
	wait_video_frame(task)
}

/// Rendered interleaved F32 audio.
pub struct RenderedAudio {
	pub data: Vec<f32>,
	pub sample_rate: i32,
	pub channel_count: i32,
}

/// Renders an audio range on a background caller.
pub fn render_audio_range(
	p: &ProjectRef,
	seq: NodeId,
	start_ts: i64,
	len_ts: i64,
	tb: (i64, i64),
) -> Result<RenderedAudio, String> {
	let session = super::real::editor_session_for_project(p)?;
	let (tx, rx) = mpsc::channel();
	submit_audio_chunk(&session, seq, start_ts, len_ts, tb, tx)?;
	rx.recv().map(|(_, audio)| audio).map_err(|e| e.to_string())
}

/// Submits one audio chunk without blocking the UI thread. Failure produces
/// aligned stereo silence so prefetch timing cannot drift.
pub fn submit_audio_chunk(
	session: &quercus_engine_rs::editor::EditorSession,
	seq: NodeId,
	start_ts: i64,
	len_ts: i64,
	tb: (i64, i64),
	tx: mpsc::Sender<(i64, RenderedAudio)>,
) -> Result<(), String> {
	if tb.0 <= 0 || tb.1 <= 0 {
		return Err("the sequence has no valid frame rate".into());
	}
	let range = quercus_engine_rs::types::TimeRange::new(
		rational_to_sdk(Rational::new(start_ts * tb.0, tb.1)),
		rational_to_sdk(Rational::new((start_ts + len_ts) * tb.0, tb.1)),
	);
	let expected_frames = ((range.out.to_f64() * 48_000.0).round()
		- (range.in_.to_f64() * 48_000.0).round())
		.max(0.0) as usize;
	let task = quercus_engine_rs::task::render_audio(
		&session.project(),
		quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
		range,
		48_000,
		0x3,
	)
	.ok_or_else(|| "failed to start the audio render task".to_string())?;
	std::thread::spawn(move || {
		if task.wait() {
			if let Some(block) = task.take_audio() {
				let info = block.info();
				let _ = tx.send((
					start_ts,
					RenderedAudio {
						data: block.samples(),
						sample_rate: info.sample_rate,
						channel_count: info.channel_count,
					},
				));
				return;
			}
		}
		let _ = tx.send((
			start_ts,
			RenderedAudio {
				data: vec![0.0; expected_frames * 2],
				sample_rate: 48_000,
				channel_count: 2,
			},
		));
	});
	Ok(())
}

fn rational_to_sdk(value: Rational) -> Rational {
	value
}

const EXPORT_SAMPLE_RATE: i32 = 48_000;
const EXPORT_CHANNEL_LAYOUT: u64 = 0x3;

/// Builds export parameters with the container's first video/audio codecs.
pub fn encoding_params(
	p: &ProjectRef,
	seq: NodeId,
	format: i32,
	_path: &std::path::Path,
	workarea: Option<(i64, i64)>,
	length_frames: i64,
) -> Result<quercus_engine_rs::task::EncodingParams, String> {
	let container = quercus_engine_rs::codec::Format::from_i32(format)
		.ok_or_else(|| format!("unknown export format {format}"))?;
	let video_codec = container
		.video_codecs()
		.iter()
		.next()
		.ok_or_else(|| format!("format {format} has no video codec"))?;
	let audio_codec = container
		.audio_codecs()
		.iter()
		.next()
		.ok_or_else(|| format!("format {format} has no audio codec"))?;
	let (width, height, rate) = super::graphops::sequence_video_params(p, seq)
		.ok_or_else(|| "the sequence has no video parameters".to_string())?;
	let rate_num = rate.num.max(1) as i32;
	let rate_den = rate.den.max(1) as i32;
	let (has_custom_range, range_in, range_out, length) = match workarea.filter(|(s, e)| e > s) {
		Some((in_ts, out_ts)) => (
			true,
			Rational::new(in_ts * i64::from(rate_den), i64::from(rate_num)),
			Rational::new(out_ts * i64::from(rate_den), i64::from(rate_num)),
			Rational::new((out_ts - in_ts) * i64::from(rate_den), i64::from(rate_num)),
		),
		None => (
			false,
			Rational::new(0, 1),
			Rational::new(0, 1),
			Rational::new(
				length_frames.max(0) * i64::from(rate_den),
				i64::from(rate_num),
			),
		),
	};
	Ok(quercus_engine_rs::task::EncodingParams {
		format,
		video_enabled: true,
		video_codec,
		video_width: width.max(1),
		video_height: height.max(1),
		video_time_base_num: rate_den,
		video_time_base_den: rate_num,
		video_pixel_format: 0,
		audio_enabled: true,
		audio_codec,
		audio_sample_rate: EXPORT_SAMPLE_RATE,
		audio_channel_layout: EXPORT_CHANNEL_LAYOUT,
		subtitles_enabled: false,
		export_length_num: length.num as i32,
		export_length_den: length.den as i32,
		has_custom_range,
		custom_range_in_num: range_in.num as i32,
		custom_range_in_den: range_in.den as i32,
		custom_range_out_num: range_out.num as i32,
		custom_range_out_den: range_out.den as i32,
		video_bit_rate: 0,
		audio_bit_rate: 0,
		color_override_enabled: false,
		color_primaries: 0,
		color_trc: 0,
		color_space: 0,
	})
}

/// Builds export parameters with the dialog's full overrides.
pub fn encoding_params_with_settings(
	p: &ProjectRef,
	seq: NodeId,
	settings: &crate::oakui::engine::ExportSettings,
	_path: &std::path::Path,
	workarea: Option<(i64, i64)>,
	length_frames: i64,
) -> Result<quercus_engine_rs::task::EncodingParams, String> {
	let container = quercus_engine_rs::codec::Format::from_i32(settings.format)
		.ok_or_else(|| format!("unknown export format {}", settings.format))?;
	let video_codec = container
		.video_codecs()
		.iter()
		.find(|c| *c == settings.video_codec)
		.ok_or_else(|| {
			format!(
				"codec {} not supported by {container:?}",
				settings.video_codec
			)
		})?;
	let audio_codec = container
		.audio_codecs()
		.iter()
		.find(|c| *c == settings.audio_codec)
		.ok_or_else(|| {
			format!(
				"audio codec {} not supported by {container:?}",
				settings.audio_codec
			)
		})?;
	let (mut width, mut height, rate) = super::graphops::sequence_video_params(p, seq)
		.ok_or_else(|| "the sequence has no video parameters".to_string())?;
	if settings.size.0 > 0 && settings.size.1 > 0 {
		width = settings.size.0;
		height = settings.size.1;
	}
	let mut rate_num = rate.num.max(1) as i32;
	let mut rate_den = rate.den.max(1) as i32;
	if settings.frame_rate > 0.0 {
		rate_num = settings.frame_rate.round().max(1.0) as i32;
		rate_den = 1;
	}
	let (has_custom_range, range_in, range_out, length) = match settings.range {
		Some((in_s, out_s)) if out_s > in_s && in_s >= 0.0 => {
			let fps = i64::from(rate_num.max(1));
			(
				true,
				Rational::new((in_s * fps as f64).round().max(0.0) as i64, fps),
				Rational::new((out_s * fps as f64).round().max(0.0) as i64, fps),
				Rational::new(((out_s - in_s) * fps as f64).round().max(0.0) as i64, fps),
			)
		}
		_ => match workarea.filter(|(s, e)| e > s) {
			Some((in_ts, out_ts)) => (
				true,
				Rational::new(
					in_ts * i64::from(rate_den.max(1)),
					i64::from(rate_num.max(1)),
				),
				Rational::new(
					out_ts * i64::from(rate_den.max(1)),
					i64::from(rate_num.max(1)),
				),
				Rational::new(
					(out_ts - in_ts) * i64::from(rate_den.max(1)),
					i64::from(rate_num.max(1)),
				),
			),
			None => (
				false,
				Rational::new(0, 1),
				Rational::new(0, 1),
				Rational::new(
					length_frames.max(0) * i64::from(rate_den.max(1)),
					i64::from(rate_num.max(1)),
				),
			),
		},
	};
	let pixel_format = if settings.bit_depth == 10 { 1 } else { 0 };
	Ok(quercus_engine_rs::task::EncodingParams {
		format: settings.format,
		video_enabled: true,
		video_codec,
		video_width: width.max(1),
		video_height: height.max(1),
		video_time_base_num: rate_den.max(1),
		video_time_base_den: rate_num.max(1),
		video_pixel_format: pixel_format,
		audio_enabled: true,
		audio_codec,
		audio_sample_rate: EXPORT_SAMPLE_RATE,
		audio_channel_layout: EXPORT_CHANNEL_LAYOUT,
		subtitles_enabled: false,
		export_length_num: length.num as i32,
		export_length_den: length.den as i32,
		has_custom_range,
		custom_range_in_num: range_in.num as i32,
		custom_range_in_den: range_in.den as i32,
		custom_range_out_num: range_out.num as i32,
		custom_range_out_den: range_out.den as i32,
		video_bit_rate: settings.video_bitrate,
		audio_bit_rate: settings.audio_bitrate,
		color_override_enabled: true,
		color_primaries: settings.color_primaries,
		color_trc: settings.color_transfer,
		color_space: settings.color_space,
	})
}

/// Starts an SDK export and adapts polling to the app's event protocol.
pub fn spawn_export(
	session: &quercus_engine_rs::editor::EditorSession,
	seq: quercus_engine_rs::node::NodeId,
	filename: &str,
	params: quercus_engine_rs::task::EncodingParams,
) -> Result<ExportSession, String> {
	let task = quercus_engine_rs::task::export(&session.project(), seq, filename, &params)
		.ok_or_else(|| "failed to start the export task".to_string())?;
	let cancellation = task.cancellation();
	let (tx, rx) = mpsc::channel::<ExportEvent>();
	std::thread::spawn(move || {
		let _ = tx.send(ExportEvent::Started);
		loop {
			let _ = tx.send(ExportEvent::Progress(task.progress().clamp(0.0, 1.0)));
			match task.state() {
				quercus_engine_rs::task::TaskState::Running => {
					std::thread::sleep(std::time::Duration::from_millis(10));
				}
				quercus_engine_rs::task::TaskState::Succeeded => {
					let _ = tx.send(ExportEvent::Finished(true, String::new()));
					break;
				}
				quercus_engine_rs::task::TaskState::Failed
				| quercus_engine_rs::task::TaskState::Cancelled => {
					let error = {
						let error = task.error();
						if error.is_empty() {
							"export failed".into()
						} else {
							error
						}
					};
					let _ = tx.send(ExportEvent::Finished(false, error));
					break;
				}
			}
		}
	});
	Ok(ExportSession {
		events: rx,
		cancel: Box::new(move || cancellation.cancel()),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn full_res_size_follows_the_playback_divider() {
		assert_eq!(full_res_render_size(1920, 1080, 1), (1920, 1080));
		assert_eq!(full_res_render_size(1920, 1080, 2), (960, 540));
		assert_eq!(full_res_render_size(1920, 1080, 4), (480, 270));
		assert_eq!(full_res_render_size(0, 0, 0), (2, 2));
	}

	#[test]
	fn invalid_geometry_is_rejected_before_task_submission() {
		assert!(validate_geometry(0, 1, (1, 25)).is_err());
		assert!(validate_geometry(1, 0, (1, 25)).is_err());
		assert!(validate_geometry(1, 1, (0, 25)).is_err());
		assert!(validate_geometry(1, 1, (1, 25)).is_ok());
	}
}

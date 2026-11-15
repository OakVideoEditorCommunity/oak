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
use super::frames::{bgra_bytes_to_render_image, lazy_gpu_f32_rgba_to_bgra_image};
use super::graphops::ProjectRef;
use super::scopes::ScopeData;

/// The pipeline's F32 pixel-format code.
pub const PIXEL_FORMAT_F32: i32 = 4;
/// The packed BGRA8 wire-format code.
pub const SLOT_FORMAT_BGRA8: i32 = 100;

/// Brings up the in-process render manager through the SDK.
///
/// Backend: the in-process Pipeline backend — Oak removed the separate
/// render worker in `4d6883f55` ("render manager now initializes the
/// in-process Pipeline backend … no ProcessDispatcher, no shared-memory
/// staging, no worker binary requirement"), so frames arrive as
/// `TicketPayload::Video` → `FrameOrigin::Cpu`, never as shm slots.
pub fn ensure_render_manager() -> bool {
	if quercus_engine_rs::render::manager_is_initialized() {
		return true;
	}
	if !quercus_engine_rs::render::manager_init(
		quercus_engine_rs::render::RenderBackend::Pipeline,
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
	/// Width in pixels of the rendered frame.
	///
	/// Taken as-is from the SDK frame info; the display path clamps negative
	/// values with `max(0)` before creating images or buffers.
	pub fn width(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => frame.info().width,
		}
	}

	/// Height in pixels of the rendered frame.
	///
	/// Taken as-is from the SDK frame info; the display path clamps negative
	/// values with `max(0)` before creating images or buffers.
	pub fn height(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => frame.info().height,
		}
	}

	/// The pipeline format code for this frame's pixels.
	///
	/// Returns `PIXEL_FORMAT_F32` (4) for F32 RGBA frames and
	/// `SLOT_FORMAT_BGRA8` (100) for BGRA8 frames — the wire codes the app's
	/// GPU/display slots expect.
	pub fn format(&self) -> i32 {
		match self {
			RenderedFrame::Sdk(frame) => match frame.info().pixel_format {
				quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba => PIXEL_FORMAT_F32,
				quercus_engine_rs::preview_video::VideoPixelFormat::Bgra8 => SLOT_FORMAT_BGRA8,
			},
		}
	}

	/// Whether the frame is backed by shared memory rather than a GPU texture.
	///
	/// True when the SDK frame origin is `SharedMemory` (a CPU plane the app can
	/// read without an extra copy); used to choose the display upload path.
	pub fn is_shm(&self) -> bool {
		match self {
			RenderedFrame::Sdk(frame) => {
				frame.info().origin == quercus_engine_rs::preview_video::FrameOrigin::SharedMemory
			}
		}
	}

	/// Whether the frame is backed by a GPU texture.
	///
	/// True when the SDK frame origin is `Gpu`; such frames can be displayed
	/// zero-copy through `to_display`'s texture registration.
	pub fn is_gpu(&self) -> bool {
		match self {
			RenderedFrame::Sdk(frame) => {
				frame.info().origin == quercus_engine_rs::preview_video::FrameOrigin::Gpu
			}
		}
	}

	/// Converts this frame into the viewer image plus optional display samples.
	///
	/// Three paths: a GPU texture is registered for zero-copy display and the
	/// result is `(lazy image, None, None)`; an F32 RGBA CPU frame is repacked
	/// tightly, passed through the project output transform and display-color
	/// conversion, and returned with its samples (`(image, None, Some(samples))`)
	/// for the RGBA16F upload/scopes; a BGRA8 frame is display-color converted
	/// and wrapped as an image (`(image, None, None)`).
	///
	/// Returns `None` when the frame has invalid dimensions or the repack/image
	/// conversion fails. The middle tuple slot is currently always `None`
	/// (reserved for scope payloads).
	pub fn to_display(&self) -> Option<(RenderImage, Option<ScopeData>, Option<Vec<f32>>)> {
		match self {
			RenderedFrame::Sdk(frame) => {
				let info = frame.info();
				let (w, h) = (info.width.max(0) as u32, info.height.max(0) as u32);
				// Register the display-transformed texture for the viewer. The
				// lazy image keeps geometry and materializes bytes only for CPU consumers.
				if let Some((ptr, tw, th)) = frame.take_gpu_texture() {
					let texture = unsafe {
						std::sync::Arc::from_raw(ptr as *const wgpu::Texture)
					};
					let image = lazy_gpu_f32_rgba_to_bgra_image(
						tw.max(1) as u32,
						th.max(1) as u32,
						frame.clone(),
					);
					super::gpu::register_texture(
						image.id.0,
						tw.max(1) as u32,
						th.max(1) as u32,
						texture,
					);
					return Some((image, None, None));
				}
				let data = frame.bytes();
				match info.pixel_format {
					quercus_engine_rs::preview_video::VideoPixelFormat::F32Rgba => {
						let mut samples =
							repack_f32_rows(info.width, info.height, info.linesize, &data)?;
						apply_output_node_f32(&mut samples);
						super::displaycolor::apply_f32_rgba(&mut samples, (w * h) as i64);
						let image = lazy_gpu_f32_rgba_to_bgra_image(w, h, frame.clone());
						Some((image, None, Some(samples)))
					}
					quercus_engine_rs::preview_video::VideoPixelFormat::Bgra8 => {
						let mut owned = data;
						super::displaycolor::apply_bgra8(&mut owned, (w * h) as i64);
						let image = bgra_bytes_to_render_image(w, h, &owned)?;
						Some((image, None, None))
					}
				}
			}
		}
	}
}

/// Applies the project's output transform before display-color conversion.
pub(crate) fn apply_output_node_f32(samples: &mut [f32]) {
	quercus_engine_rs::utils::working_to_display_target(
		samples,
		&quercus_engine_rs::state::color_working_space(),
		&quercus_engine_rs::state::color_output_gamut(),
		&quercus_engine_rs::state::color_output_transfer(),
	);
}

/// Repacks padded F32 RGBA rows into a tightly packed sample vector.
///
/// # Arguments
///
/// * `width` / `height` - frame size in pixels; must be positive.
/// * `linesize` - row stride in bytes as reported by the frame; values smaller
///   than `width * 16` are treated as tightly packed.
/// * `data` - raw frame bytes, at least `linesize * height` long.
///
/// Returns `None` for non-positive dimensions or a short buffer; otherwise a
/// `width * height * 4` F32 vector in RGBA order (native-endian f32s).
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

/// Checks that a render request has positive dimensions and a positive time base.
///
/// # Arguments
///
/// * `width` / `height` - requested output size in pixels.
/// * `tb` - frame time base `(num, den)`.
///
/// Returns `Err("invalid render geometry or frame rate")` when any value is
/// non-positive; called before submitting a task so bad geometry fails fast.
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
///
/// Thin wrapper over [`submit_video_frame_scheduled`] with no playback
/// metadata; the returned task must be awaited or polled by the caller.
///
/// # Arguments
///
/// * `p` - project the viewer belongs to.
/// * `viewer` - sequence or footage node to render.
/// * `frame_ts` / `tb` - frame timestamp and time base.
/// * `width` / `height` - output size in pixels.
/// * `format` - requested pixel format (`None` → BGRA8).
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing session, or a task
/// that cannot be started.
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

/// Starts one sequence/footage frame render at playback priority.
///
/// Same as [`submit_video_frame`] but tags the request with the current
/// playhead and a generation counter, letting the engine prefer/drop frames by
/// distance from the playhead and discard results from an older generation.
///
/// # Arguments
///
/// * `p` - project the viewer belongs to.
/// * `viewer` - sequence or footage node to render.
/// * `frame_ts` / `tb` - frame timestamp and time base.
/// * `width` / `height` - output size in pixels.
/// * `format` - requested pixel format (`None` → BGRA8).
/// * `playhead` - current playhead timestamp in the same units as `frame_ts`.
/// * `generation` - monotonically increasing playback generation; stale
///   generations are ignored by the scheduler.
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing session, or a task
/// that cannot be started.
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

/// Shared submission path for [`submit_video_frame`] and its playback variant.
///
/// Validates geometry, resolves the editor session for `p`, maps the requested
/// [`PixelFormat`] to the SDK's video format (F32 → F32 RGBA, anything else →
/// BGRA8), converts `frame_ts`/`tb` to rational seconds, then starts either the
/// playback-priority render (`playback = Some((playhead, generation))`, distance
/// `|frame_ts - playhead|`) or an ordinary render.
///
/// # Arguments
///
/// * `p` - project the viewer belongs to.
/// * `viewer` - node id of the sequence or footage being rendered.
/// * `frame_ts` - frame timestamp in `tb` units.
/// * `tb` - frame time base `(num, den)`; both positive.
/// * `width` / `height` - output size in pixels; must be positive.
/// * `format` - requested pixel format (`None` → BGRA8).
/// * `playback` - optional `(playhead_ts, generation)` for playback scheduling;
///   `generation` invalidates stale in-flight frames.
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing session, or a task
/// that cannot be started.
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

/// Waits for an SDK video task and takes its frame exactly once.
///
/// Blocks on the task; on failure returns the task's error message (or
/// `"video render failed"` when empty). On success takes the frame with
/// `take_video`, which consumes it, so the caller must not take again.
///
/// # Arguments
///
/// * `task` - the video render task returned by `submit_*`; ownership moves in.
///
/// # Errors
///
/// Returns an error string when the task fails/cancels or produces no frame.
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

/// Renders one sequence frame through an SDK task and waits for it.
///
/// Submits the frame (see [`submit_video_frame`]) and blocks on
/// [`wait_video_frame`]; the synchronous counterpart is for background
/// workers/tests — the UI path submits and polls instead.
///
/// # Arguments
///
/// * `p` - project the sequence belongs to.
/// * `seq` - sequence node to render.
/// * `frame_ts` / `tb` - frame time and its time base.
/// * `width` / `height` - output size in pixels.
/// * `format` - requested pixel format; `Some(PixelFormat::F32)` selects F32
///   RGBA, anything else selects BGRA8.
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing session, a task that
/// cannot start, or a render failure.
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

/// Renders one source-footage frame through an SDK task and waits for it.
///
/// Same as [`render_sequence_frame`] but `footage` identifies a footage node
/// (the viewer renders the source directly, without the sequence graph).
///
/// # Arguments
///
/// * `p` - project the footage belongs to.
/// * `footage` - footage node to render.
/// * `frame_ts` / `tb` - frame time and its time base.
/// * `width` / `height` - output size in pixels.
/// * `format` - requested pixel format; `Some(PixelFormat::F32)` selects F32
///   RGBA, anything else selects BGRA8.
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing session, a task that
/// cannot start, or a render failure.
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

/// Renders one single-track multicam angle and waits for the frame.
///
/// Always requests F32 RGBA (the multicam compositor's output format) and uses
/// the sequence/track ids directly rather than the viewer path.
///
/// # Arguments
///
/// * `p` - project the sequence belongs to.
/// * `seq` - sequence node owning the tracks.
/// * `track` - track node holding the angle clips.
/// * `frame_ts` / `tb` - frame time and its time base; converted to seconds.
/// * `width` / `height` - output size in pixels; must be positive.
///
/// # Errors
///
/// Returns an error string for invalid geometry, a missing editor session, a
/// task that cannot start, or a render failure.
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

/// Renders an audio range synchronously on a background caller.
///
/// Creates a channel, submits one chunk (which runs on its own thread) and
/// blocks until the chunk arrives; used by non-realtime callers/tests, not the
/// playback prefetch.
///
/// # Arguments
///
/// * `p` - project the sequence belongs to.
/// * `seq` - sequence node to render.
/// * `start_ts` / `len_ts` - range start and length in frames.
/// * `tb` - frame time base `(num, den)`.
///
/// # Errors
///
/// Returns an error string when the session/job cannot be created or the
/// channel closes before delivering a chunk.
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

/// One audio-render job owned by [`AudioRenderWorker`].
struct AudioRenderJob {
	task: quercus_engine_rs::task::Task,
	start_ts: i64,
	expected_frames: usize,
	tx: mpsc::Sender<(i64, RenderedAudio)>,
}

impl AudioRenderJob {
	/// Waits for the job's render task and forwards the result on its channel.
	///
	/// On success sends the decoded block's samples together with its sample
	/// rate and channel count. On failure (task error, cancellation, or no audio
	/// block) sends `expected_frames * 2` zeros at 48 kHz stereo, so a failed
	/// prefetch keeps the playback timeline aligned instead of stalling. The
	/// send is best-effort; a closed receiver is ignored.
	fn run(self) {
		if self.task.wait() {
			if let Some(block) = self.task.take_audio() {
				let info = block.info();
				let _ = self.tx.send((
					self.start_ts,
					RenderedAudio {
						data: block.samples(),
						sample_rate: info.sample_rate,
						channel_count: info.channel_count,
					},
				));
				return;
			}
		}
		// Failure produces aligned stereo silence so prefetch timing
		// cannot drift.
		let _ = self.tx.send((
			self.start_ts,
			RenderedAudio {
				data: vec![0.0; self.expected_frames * 2],
				sample_rate: 48_000,
				channel_count: 2,
			},
		));
	}
}

/// Serializes playback-audio chunk completion onto one worker.
///
/// The playback prefetch submits one chunk per UI tick. Spawning a fresh
/// OS thread for every chunk (the old behavior) creates hundreds of
/// short-lived threads during playback, each taking the engine and audio
/// mutexes; that is both a lock-contention source and a thread-lifetime
/// hazard when a render stalls. One persistent worker keeps the same
/// ordering and non-blocking submit contract with a bounded thread count.
pub struct AudioRenderWorker {
	tx: Option<mpsc::Sender<AudioRenderJob>>,
	_join: std::thread::JoinHandle<()>,
}

impl AudioRenderWorker {
	/// Starts the persistent audio-render worker thread.
	///
	/// Spawns one named thread (`oak-audio-render`) that drains queued
	/// [`AudioRenderJob`]s in submission order, so playback needs no
	/// thread-per-chunk. The worker exits when the sender is dropped (see the
	/// `Drop` impl); the join handle is kept only to detach deliberately.
	///
	/// # Panics
	///
	/// Panics if the OS thread cannot be spawned (`.expect("spawn
	/// oak-audio-render")`); this is a startup-time resource failure.
	pub fn new() -> Self {
		let (tx, rx) = mpsc::channel::<AudioRenderJob>();
		let join = std::thread::Builder::new()
			.name("oak-audio-render".into())
			.spawn(move || {
				while let Ok(job) = rx.recv() {
					job.run();
				}
			})
			.expect("spawn oak-audio-render");
		Self {
			tx: Some(tx),
			_join: join,
		}
	}

	fn submit(&self, job: AudioRenderJob) -> Result<(), String> {
		self.tx
			.as_ref()
			.ok_or_else(|| "the audio render worker is closed".to_string())?
			.send(job)
			.map_err(|_| "the audio render worker is closed".to_string())
	}
}

impl Drop for AudioRenderWorker {
	fn drop(&mut self) {
		// Close the queue; the worker exits after draining queued jobs.
		// The JoinHandle is detached deliberately: shutdown must not block
		// on a render that is waiting for a GPU/worker fence.
		self.tx.take();
	}
}

/// Builds (but does not run) one audio-render job for `[start_ts, start_ts+len_ts)`.
///
/// Converts the frame range to rational seconds using `tb`, computes the number
/// of 48 kHz frames the chunk should contain (used to pad failures with aligned
/// silence), and starts an SDK `render_audio` task at 48 kHz with the stereo
/// layout mask `0x3`.
///
/// # Arguments
///
/// * `session` - editor session that owns the project.
/// * `seq` - sequence node to render.
/// * `start_ts` / `len_ts` - chunk start and length in frames.
/// * `tb` - frame time base `(num, den)`; must be positive.
/// * `tx` - channel the job reports `(start_ts, RenderedAudio)` on.
///
/// # Errors
///
/// Returns an error string for an invalid time base or when the SDK cannot
/// start the render task.
fn audio_render_job(
	session: &quercus_engine_rs::editor::EditorSession,
	seq: NodeId,
	start_ts: i64,
	len_ts: i64,
	tb: (i64, i64),
	tx: mpsc::Sender<(i64, RenderedAudio)>,
) -> Result<AudioRenderJob, String> {
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
	Ok(AudioRenderJob {
		task,
		start_ts,
		expected_frames,
		tx,
	})
}

/// Submits one audio chunk on a one-shot thread without blocking the UI.
///
/// Builds the render job synchronously (so invalid ranges/time base fail here)
/// then spawns an OS thread that waits for it and reports
/// `(start_ts, RenderedAudio)` through `tx`. Used by the synchronous
/// [`render_audio_range`] helper; the playback path uses
/// [`submit_audio_chunk_to_worker`] instead to avoid one thread per chunk.
///
/// # Arguments
///
/// * `session` - editor session that owns the project.
/// * `seq` - sequence node to render.
/// * `start_ts` / `len_ts` - chunk start and length in frames.
/// * `tb` - frame time base `(num, den)`; both must be positive.
/// * `tx` - channel receiving the mixed chunk (or silence on failure).
///
/// # Errors
///
/// Returns an error string when the time base is invalid or the SDK cannot
/// start the audio render task.
pub fn submit_audio_chunk(
	session: &quercus_engine_rs::editor::EditorSession,
	seq: NodeId,
	start_ts: i64,
	len_ts: i64,
	tb: (i64, i64),
	tx: mpsc::Sender<(i64, RenderedAudio)>,
) -> Result<(), String> {
	let job = audio_render_job(session, seq, start_ts, len_ts, tb, tx)?;
	std::thread::spawn(move || job.run());
	Ok(())
}

/// Playback path: queue the chunk on the persistent audio worker instead
/// of creating one OS thread per chunk.
pub fn submit_audio_chunk_to_worker(
	session: &quercus_engine_rs::editor::EditorSession,
	seq: NodeId,
	start_ts: i64,
	len_ts: i64,
	tb: (i64, i64),
	tx: mpsc::Sender<(i64, RenderedAudio)>,
	worker: &AudioRenderWorker,
) -> Result<(), String> {
	let job = audio_render_job(session, seq, start_ts, len_ts, tb, tx)?;
	worker.submit(job)
}

fn rational_to_sdk(value: Rational) -> Rational {
	value
}

const EXPORT_SAMPLE_RATE: i32 = 48_000;
const EXPORT_CHANNEL_LAYOUT: u64 = 0x3;

/// Builds export parameters with the container's first video/audio codecs.
///
/// Used by callers that pick only a container (no per-codec dialog). The
/// sequence's video size and rate are used; `workarea` (frames) becomes the
/// custom range when non-empty, otherwise `length_frames` sets the export
/// length. Video/audio are enabled, subtitles disabled, audio fixed at 48 kHz
/// stereo (layout mask `0x3`), bitrates left at 0 (codec defaults) and color
/// override disabled.
///
/// # Arguments
///
/// * `p` - project the sequence belongs to.
/// * `seq` - sequence node to export.
/// * `format` - export container as a `Format` discriminant.
/// * `_path` - reserved for per-output-path overrides; currently unused.
/// * `workarea` - optional `(in_ts, out_ts)` in frames; used only when
///   `out_ts > in_ts`.
/// * `length_frames` - timeline length in frames when there is no workarea.
///
/// # Errors
///
/// Returns an error string when the format is unknown, the container has no
/// video or audio codec, or the sequence has no video parameters.
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

/// Builds export parameters from the export dialog's full overrides.
///
/// Unlike [`encoding_params`], the container and codecs must match the user's
/// selection, and size/frame rate/range/bit depth/color come from `settings`
/// when set. A positive `settings.frame_rate` replaces the sequence rate with
/// that integer rate (denominator 1); `settings.range` (seconds) wins over
/// `workarea`; a 10-bit setting maps to pixel format code 1, otherwise 0.
///
/// # Arguments
///
/// * `p` - project the sequence belongs to.
/// * `seq` - sequence node to export.
/// * `settings` - dialog overrides: format, video/audio codec, optional size,
///   frame rate, range in seconds, bit depth, bitrates and color metadata.
/// * `_path` - reserved for per-output-path overrides; currently unused.
/// * `workarea` - optional `(in_ts, out_ts)` frame range used when
///   `settings.range` is absent or invalid.
/// * `length_frames` - fallback length when neither range applies.
///
/// # Errors
///
/// Returns an error string when the format is unknown, either selected codec is
/// not offered by the container, or the sequence has no video parameters.
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

/// Starts an SDK export and adapts its polling to the app's event protocol.
///
/// Starts `task::export` on the sequence, then spawns a polling thread that emits
/// `ExportEvent::Started`, a `Progress` value clamped to 0..1 (every 10 ms while
/// running) and finally `Finished(ok, message)` — the message is empty on
/// success and the task error (or `"export failed"`) on failure/cancel. The
/// returned `ExportSession` exposes the event receiver and a cancel closure.
///
/// # Arguments
///
/// * `session` - editor session that owns the project.
/// * `seq` - sequence node to export.
/// * `filename` - output file path passed to the SDK exporter.
/// * `params` - encoding parameters (from `encoding_params` or
///   `encoding_params_with_settings`).
///
/// # Errors
///
/// Returns an error string when the SDK cannot start the export task (no task
/// id). Per-frame failures are reported through `ExportEvent::Finished`.
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

//! Safe wrapper over the engine's task C ABI (`oak_task_*`):
//! background tasks with progress/cancel — render export, project interchange,
//! proxy generation and audio waveform analysis.
//!
//! Tasks run on engine threads; a [`Task`] handle polls or blocks
//! ([`Task::wait`]), and drops release it. Load tasks hand their
//! project over through [`Task::take_project`]; analysis tasks transfer
//! immutable owned DTOs through their corresponding take methods.

use std::ffi::c_void;

use crate::codec::string_out;
use crate::node::{NodeId, Project};

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_task_export(
		project: *const c_void,
		viewer: u64,
		filename: *const u8,
		filename_len: usize,
		params: *const EncodingParams,
	) -> u64;
	fn oak_task_project_save_otio(
		project: *const c_void,
		filename: *const u8,
		filename_len: usize,
	) -> u64;
	fn oak_task_project_load_otio(filename: *const u8, filename_len: usize) -> u64;
	fn oak_task_title(id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_task_progress(id: u64) -> f64;
	fn oak_task_state(id: u64) -> i32;
	fn oak_task_error(id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_task_cancel(id: u64);
	fn oak_task_wait(id: u64) -> bool;
	fn oak_task_release(id: u64);
	fn oak_task_proxy_max_concurrent() -> i32;
	fn oak_task_take_project(id: u64) -> *mut c_void;
	fn oak_task_waveform_extract(
		filename: *const u8,
		filename_len: usize,
		stream_index: i32,
		samples_per_point: i32,
	) -> u64;
	fn oak_task_take_waveform(id: u64) -> *const c_void;
	fn oak_task_thumbnail_render(
		project: *const c_void,
		viewer: u64,
		time: crate::types::Rational,
		width: i32,
		height: i32,
	) -> u64;
	fn oak_task_take_thumbnail(id: u64) -> *const c_void;
	fn oak_task_audio_render(
		project: *const c_void,
		viewer: u64,
		range: crate::types::TimeRange,
		sample_rate: i32,
		channel_layout: u64,
	) -> u64;
	fn oak_task_take_audio(id: u64) -> *const c_void;
	fn oak_task_video_render(
		project: *const c_void,
		viewer: u64,
		time: crate::types::Rational,
		width: i32,
		height: i32,
		pixel_format: i32,
	) -> u64;
	fn oak_task_video_render_playback(
		project: *const c_void,
		viewer: u64,
		time: crate::types::Rational,
		width: i32,
		height: i32,
		pixel_format: i32,
		frame: i64,
		distance: i64,
		generation: u64,
	) -> u64;
	fn oak_task_video_render_track(
		project: *const c_void,
		viewer: u64,
		track: u64,
		time: crate::types::Rational,
		width: i32,
		height: i32,
		pixel_format: i32,
	) -> u64;
	fn oak_task_take_video(id: u64) -> *const c_void;
}

/// `repr(C)` mirror of the engine's encoding parameters for
/// [`export`]. Zeroed fields are disabled/defaults.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodingParams {
	/// Export format id.
	pub format: i32,
	/// The `video_enabled` field.
	pub video_enabled: bool,
	/// The `video_codec` field.
	pub video_codec: i32,
	/// The `video_width` field.
	pub video_width: i32,
	/// The `video_height` field.
	pub video_height: i32,
	/// Frame duration rational (numerator/denominator).
	pub video_time_base_num: i32,
	/// The `video_time_base_den` field.
	pub video_time_base_den: i32,
	/// Delivery pixel format.
	pub video_pixel_format: i32,
	/// The `audio_enabled` field.
	pub audio_enabled: bool,
	/// The `audio_codec` field.
	pub audio_codec: i32,
	/// The `audio_sample_rate` field.
	pub audio_sample_rate: i32,
	/// The `audio_channel_layout` field.
	pub audio_channel_layout: u64,
	/// The `subtitles_enabled` field.
	pub subtitles_enabled: bool,
	/// Export length, rational seconds.
	pub export_length_num: i32,
	/// The `export_length_den` field.
	pub export_length_den: i32,
	/// Whether `[custom_range_in, custom_range_out)` replaces the whole
	/// viewer length.
	pub has_custom_range: bool,
	/// The `custom_range_in_num` field.
	pub custom_range_in_num: i32,
	/// The `custom_range_in_den` field.
	pub custom_range_in_den: i32,
	/// The `custom_range_out_num` field.
	pub custom_range_out_num: i32,
	/// The `custom_range_out_den` field.
	pub custom_range_out_den: i32,
	/// 0 = codec default.
	pub video_bit_rate: i64,
	/// The `audio_bit_rate` field.
	pub audio_bit_rate: i64,
	/// 0 = the project's output spec; 1 = apply the color_* fields.
	pub color_override_enabled: bool,
	/// AVColorPrimaries (SDR 709 = 1, HDR BT.2020 = 9).
	pub color_primaries: i32,
	/// AVColorTransferCharacteristic (SDR 709 = 1, HDR PQ = 16).
	pub color_trc: i32,
	/// AVColorSpace (SDR 709 = 1, HDR BT.2020 = 9).
	pub color_space: i32,
}

/// A task's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
	/// The task is still running.
	Running,
	/// The task completed successfully.
	Succeeded,
	/// The task failed.
	Failed,
	/// The task was cancelled.
	Cancelled,
}

/// A running (or finished) task. Drop releases it (a running task is
/// cancelled and joined first).
pub struct Task {
	id: u64,
}

/// A lightweight cancellation handle for a task owned by another thread.
///
/// It never releases the task registry entry, so it is safe to clone and call
/// after the owning [`Task`] has finished: cancellation then becomes a no-op.
#[derive(Clone)]
pub struct TaskCancellation {
	id: u64,
}

impl TaskCancellation {
	/// Requests cooperative cancellation without joining the worker.
	pub fn cancel(&self) {
		unsafe { oak_task_cancel(self.id) };
	}
}

impl Task {
	pub(crate) fn new(id: u64) -> Option<Task> {
		(id != 0).then_some(Task { id })
	}

	/// Returns the registry identity for session/task correlation APIs.
	pub(crate) fn id(&self) -> u64 {
		self.id
	}

	/// Returns an independent cancellation handle for a UI/session owner.
	pub fn cancellation(&self) -> TaskCancellation {
		TaskCancellation { id: self.id }
	}

	/// The task title.
	pub fn title(&self) -> String {
		string_out(|buf, len| unsafe { oak_task_title(self.id, buf, len) }).unwrap_or_default()
	}

	/// Progress in 0.0..=1.0.
	pub fn progress(&self) -> f64 {
		unsafe { oak_task_progress(self.id) }.max(0.0)
	}

	/// The current state.
	pub fn state(&self) -> TaskState {
		match unsafe { oak_task_state(self.id) } {
			1 => TaskState::Succeeded,
			2 => TaskState::Failed,
			3 => TaskState::Cancelled,
			_ => TaskState::Running,
		}
	}

	/// The error message ("" when none).
	pub fn error(&self) -> String {
		string_out(|buf, len| unsafe { oak_task_error(self.id, buf, len) }).unwrap_or_default()
	}

	/// Requests cancellation (the task aborts at its next cancellation
	/// point).
	pub fn cancel(&self) {
		unsafe { oak_task_cancel(self.id) };
	}

	/// Blocks until the task finishes. True on success.
	pub fn wait(&self) -> bool {
		unsafe { oak_task_wait(self.id) }
	}

	/// Takes the produced project of a succeeded load task (None when
	/// none / already taken).
	pub fn take_project(&self) -> Option<Project> {
		let ptr = unsafe { oak_task_take_project(self.id) };
		(!ptr.is_null()).then_some(Project::from_raw(ptr))
	}

	/// Takes the owned result of a successful waveform task. Subsequent calls
	/// return `None`, and non-waveform tasks never produce a result here.
	pub fn take_waveform(&self) -> Option<crate::audio_analysis::Waveform> {
		crate::audio_analysis::Waveform::from_raw(unsafe { oak_task_take_waveform(self.id) })
	}

	/// Takes the owned result of a successful thumbnail render exactly once.
	pub fn take_thumbnail(&self) -> Option<crate::thumbnail::Thumbnail> {
		crate::thumbnail::Thumbnail::from_raw(unsafe { oak_task_take_thumbnail(self.id) })
	}

	/// Takes the owned result of a successful audio mix exactly once.
	pub fn take_audio(&self) -> Option<crate::preview_audio::AudioBlock> {
		crate::preview_audio::AudioBlock::from_raw(unsafe { oak_task_take_audio(self.id) })
	}

	/// Takes the owned result of a successful video render exactly once.
	pub fn take_video(&self) -> Option<crate::preview_video::VideoFrame> {
		crate::preview_video::VideoFrame::from_raw(unsafe { oak_task_take_video(self.id) })
	}
}

impl Drop for Task {
	fn drop(&mut self) {
		unsafe { oak_task_release(self.id) };
	}
}

/// Maximum number of proxy transcodes that should run concurrently.
///
/// Queueing remains a host policy; this engine value keeps the UI and other SDK
/// consumers aligned with the worker's CPU/thread budget.
pub fn proxy_max_concurrent() -> usize {
	(unsafe { oak_task_proxy_max_concurrent() }).max(1) as usize
}

/// Starts asynchronous min/max peak extraction for one audio-stream ordinal.
///
/// Wait or poll the returned task, then call [`Task::take_waveform`] exactly
/// once. Invalid arguments return `None`; decode failures are reported by the
/// task state and error string.
pub fn extract_waveform(filename: &str, stream_index: i32, samples_per_point: i32) -> Option<Task> {
	Task::new(unsafe {
		oak_task_waveform_extract(
			filename.as_ptr(),
			filename.len(),
			stream_index,
			samples_per_point,
		)
	})
}

/// Starts an asynchronous F32 RGBA thumbnail render of a sequence or video
/// footage node.
///
/// The worker retains the project internally. On success, [`Task::take_thumbnail`]
/// transfers an immutable CPU DTO whose lifetime is independent of the task.
pub fn render_thumbnail(
	project: &Project,
	viewer: NodeId,
	time: crate::types::Rational,
	size: (u32, u32),
) -> Option<Task> {
	if size.0 == 0 || size.1 == 0 {
		return None;
	}
	Task::new(unsafe {
		oak_task_thumbnail_render(
			project.as_raw(),
			viewer.raw(),
			time,
			size.0 as i32,
			size.1 as i32,
		)
	})
}

/// Starts asynchronous audio mixing for a sequence or footage range.
pub fn render_audio(
	project: &Project,
	viewer: NodeId,
	range: crate::types::TimeRange,
	sample_rate: i32,
	channel_layout: u64,
) -> Option<Task> {
	if range.out <= range.in_ || sample_rate <= 0 || channel_layout == 0 {
		return None;
	}
	Task::new(unsafe {
		oak_task_audio_render(
			project.as_raw(),
			viewer.raw(),
			range,
			sample_rate,
			channel_layout,
		)
	})
}

/// Starts asynchronous sequence/footage video rendering into an owned frame.
pub fn render_video(
	project: &Project,
	viewer: NodeId,
	time: crate::types::Rational,
	size: (u32, u32),
	pixel_format: crate::preview_video::VideoPixelFormat,
) -> Option<Task> {
	if size.0 == 0 || size.1 == 0 {
		return None;
	}
	Task::new(unsafe {
		oak_task_video_render(
			project.as_raw(),
			viewer.raw(),
			time,
			size.0 as i32,
			size.1 as i32,
			pixel_format as i32,
		)
	})
}

/// Starts an asynchronous Playback-priority preview frame render.
pub fn render_playback_video(
	project: &Project,
	viewer: NodeId,
	time: crate::types::Rational,
	size: (u32, u32),
	pixel_format: crate::preview_video::VideoPixelFormat,
	frame: i64,
	distance: i64,
	generation: u64,
) -> Option<Task> {
	if size.0 == 0 || size.1 == 0 {
		return None;
	}
	Task::new(unsafe {
		oak_task_video_render_playback(
			project.as_raw(),
			viewer.raw(),
			time,
			size.0 as i32,
			size.1 as i32,
			pixel_format as i32,
			frame,
			distance,
			generation,
		)
	})
}

/// Starts an asynchronous multicam-angle render of one video track.
pub fn render_track_video(
	project: &Project,
	viewer: NodeId,
	track: NodeId,
	time: crate::types::Rational,
	size: (u32, u32),
	pixel_format: crate::preview_video::VideoPixelFormat,
) -> Option<Task> {
	if size.0 == 0 || size.1 == 0 {
		return None;
	}
	Task::new(unsafe {
		oak_task_video_render_track(
			project.as_raw(),
			viewer.raw(),
			track.raw(),
			time,
			size.0 as i32,
			size.1 as i32,
			pixel_format as i32,
		)
	})
}

/// Starts an export of the viewer node (a sequence or footage) to
/// `filename` with `params`. None on invalid input.
pub fn export(
	project: &Project,
	viewer: NodeId,
	filename: &str,
	params: &EncodingParams,
) -> Option<Task> {
	Task::new(unsafe {
		oak_task_export(
			project.as_raw(),
			viewer.raw(),
			filename.as_ptr(),
			filename.len(),
			params,
		)
	})
}

/// Starts a project save to an interchange format (`.otio` /
/// `.fcpxml`, chosen by the filename extension). None on invalid
/// input.
pub fn save_otio(project: &Project, filename: &str) -> Option<Task> {
	Task::new(unsafe {
		oak_task_project_save_otio(project.as_raw(), filename.as_ptr(), filename.len())
	})
}

/// Starts loading an interchange document (`.otio` / `.fcpxml`) as a
/// new project. None on invalid input. Take the project with
/// [`Task::take_project`] once the task succeeds.
pub fn load_otio(filename: &str) -> Option<Task> {
	Task::new(unsafe { oak_task_project_load_otio(filename.as_ptr(), filename.len()) })
}

#[cfg(test)]
mod tests {
	use super::*;

	fn export_params(length_seconds: i32) -> EncodingParams {
		EncodingParams {
			format: crate::codec::Format::MPEG4Video as i32,
			video_enabled: true,
			video_codec: crate::codec::Codec::H264 as i32,
			video_width: 64,
			video_height: 64,
			video_time_base_num: 1,
			video_time_base_den: 25,
			export_length_num: length_seconds,
			export_length_den: 1,
			..EncodingParams::default()
		}
	}

	/// Save → load round-trip through the task path.
	#[test]
	fn save_then_load_otio_roundtrip() {
		// The setup pushes undo entries (clip placement); serialize
		// against the other undo-stack tests.
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let path = std::env::temp_dir()
			.join(format!("oaksdk_task_{}.otio", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let project = Project::new();
		let sequence = project.create_sequence("Seq 1").expect("sequence");
		let list = project.sequence_track_list_at(sequence, 0);
		let track = project.tracklist_track_at(list, 0);
		let clip = project.create_clip().expect("clip");
		assert!(project.block_set_length(clip, crate::types::Rational::new(2, 1)));
		assert!(project.track_append_block(track, clip));

		let save = save_otio(&project, &path).expect("save task");
		assert!(save.wait(), "save failed: {}", save.error());
		assert_eq!(save.state(), TaskState::Succeeded);
		drop(save);

		let load = load_otio(&path).expect("load task");
		assert!(load.wait(), "load failed: {}", load.error());
		let loaded = load.take_project().expect("loaded project");
		assert!(loaded.root().is_valid());
		assert!(load.take_project().is_none(), "a project is taken once");
		drop(load);

		let _ = std::fs::remove_file(&path);
	}

	/// Real export tasks preserve success, cooperative cancellation and errors.
	#[test]
	fn export_success_cancel_and_failure() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		if !crate::render::manager_init(crate::render::RenderBackend::Threads) {
			assert!(crate::render::manager_is_initialized());
		}
		let session = crate::editor::EditorSession::new().expect("session");
		let sequence = session.create_sequence("Export").expect("sequence");
		let media = crate::test_support::make_test_clip("sdk_export");
		let footage = session.project().import_footage(&media).expect("footage");
		session
			.execute(crate::edit::EditCommand::PlaceFootageClip {
				footage,
				kind: crate::editor::TrackKind::Video,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
				media_in_ts: 0,
			})
			.expect("place clip");
		let path = std::env::temp_dir()
			.join(format!("oak_sdk_export_{}.mp4", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let task = export(&session.project(), sequence, &path, &export_params(1)).expect("export");
		assert!(task.wait(), "export failed: {}", task.error());
		assert_eq!(task.state(), TaskState::Succeeded);
		assert!(std::path::Path::new(&path).is_file());

		let cancelled_path = std::env::temp_dir()
			.join(format!("oak_sdk_export_cancel_{}.mp4", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let cancelled = export(
			&session.project(),
			sequence,
			&cancelled_path,
			&export_params(10_000),
		)
		.expect("cancellable export");
		cancelled.cancellation().cancel();
		assert!(!cancelled.wait());
		assert_eq!(cancelled.state(), TaskState::Cancelled);

		let failed = export(
			&session.project(),
			sequence,
			"/nonexistent/oak-sdk-export/output.mp4",
			&export_params(1),
		)
		.expect("invalid output fails asynchronously");
		assert!(!failed.wait());
		assert_eq!(failed.state(), TaskState::Failed);
		assert!(!failed.error().is_empty());

		let _ = std::fs::remove_file(path);
		let _ = std::fs::remove_file(cancelled_path);
		let _ = std::fs::remove_file(media);
	}
}

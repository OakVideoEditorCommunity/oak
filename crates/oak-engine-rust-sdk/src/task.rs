//! Safe wrapper over the engine's task C ABI (`oak_task_*`):
//! background tasks with progress/cancel — the render export and the
//! OTIO/FCPXML project interchange.
//!
//! Tasks run on engine threads; a [`Task`] handle polls or blocks
//! ([`Task::wait`]), and drops release it. Load tasks hand their
//! project over through [`Task::take_project`].

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
}

/// `repr(C)` mirror of the engine's encoding parameters for
/// [`export`]. Zeroed fields are disabled/defaults.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodingParams {
	/// Export format id.
	pub format: i32,
	pub video_enabled: bool,
	pub video_codec: i32,
	pub video_width: i32,
	pub video_height: i32,
	/// Frame duration rational (numerator/denominator).
	pub video_time_base_num: i32,
	pub video_time_base_den: i32,
	/// Delivery pixel format.
	pub video_pixel_format: i32,
	pub audio_enabled: bool,
	pub audio_codec: i32,
	pub audio_sample_rate: i32,
	pub audio_channel_layout: u64,
	pub subtitles_enabled: bool,
	/// Export length, rational seconds.
	pub export_length_num: i32,
	pub export_length_den: i32,
	/// Whether `[custom_range_in, custom_range_out)` replaces the whole
	/// viewer length.
	pub has_custom_range: bool,
	pub custom_range_in_num: i32,
	pub custom_range_in_den: i32,
	pub custom_range_out_num: i32,
	pub custom_range_out_den: i32,
	/// 0 = codec default.
	pub video_bit_rate: i64,
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
	Running,
	Succeeded,
	Failed,
	Cancelled,
}

/// A running (or finished) task. Drop releases it (a running task is
/// cancelled and joined first).
pub struct Task {
	id: u64,
}

impl Task {
	pub(crate) fn new(id: u64) -> Option<Task> {
		(id != 0).then_some(Task { id })
	}

	/// Returns the registry identity for session/task correlation APIs.
	pub(crate) fn id(&self) -> u64 {
		self.id
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
}

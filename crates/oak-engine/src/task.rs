//! C ABI for oak-task (prefix `oak_task_`): background tasks with
//! progress/cancel, covering interchange, render export, proxy generation and
//! audio waveform analysis.
//!
//! Tasks run on engine-spawned threads and live in a process-global
//! registry keyed by a u64 id (0 = invalid). Lifecycle:
//! create (`oak_task_export` / `oak_task_project_save_otio` /
//! `oak_task_project_load_otio`) → poll
//! [`oak_task_progress`]/[`oak_task_state`] or block in
//! [`oak_task_wait`] → load/analysis tasks transfer their owned result through
//! the corresponding take function → [`oak_task_release`].
//!
//! The app-side TaskManager (the task panel's job list) is host
//! infrastructure and intentionally not exported.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use oak_node::id::NodeId;
use oak_task::export::{EncodingParams, ExportTask};
use oak_task::project::load::ProjectLoadBaseTask;
use oak_task::project::loadotio::LoadOTIOTask;
use oak_task::project::saveotio::SaveOTIOTask;
use oak_task::task::{Task, TaskBehavior, TaskEvent};

use crate::codec::{copy_str_out, str_arg};
use crate::coretypes::OakRational;
use crate::handle::NativeMirror;
use crate::node::ProjectHandle;

// ---------------------------------------------------------------------------
// Encoding parameters
// ---------------------------------------------------------------------------

/// `repr(C)` mirror of `oak_task::export::EncodingParams` (the
/// filename travels as a separate `(ptr, len)` argument).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakTaskEncodingParams {
	pub format: i32,
	pub video_enabled: bool,
	pub video_codec: i32,
	pub video_width: i32,
	pub video_height: i32,
	pub video_time_base_num: i32,
	pub video_time_base_den: i32,
	pub video_pixel_format: i32,
	pub audio_enabled: bool,
	pub audio_codec: i32,
	pub audio_sample_rate: i32,
	pub audio_channel_layout: u64,
	pub subtitles_enabled: bool,
	pub export_length_num: i32,
	pub export_length_den: i32,
	pub has_custom_range: bool,
	pub custom_range_in_num: i32,
	pub custom_range_in_den: i32,
	pub custom_range_out_num: i32,
	pub custom_range_out_den: i32,
	pub video_bit_rate: i64,
	pub audio_bit_rate: i64,
	pub color_override_enabled: bool,
	pub color_primaries: i32,
	pub color_trc: i32,
	pub color_space: i32,
}

impl OakTaskEncodingParams {
	/// Adds the out-of-band filename required by the native export request.
	/// `NativeMirror` is not used because the ABI value intentionally does not
	/// contain that required native field.
	fn to_native(&self, filename: String) -> EncodingParams {
		EncodingParams {
			filename,
			format: self.format,
			video_enabled: self.video_enabled,
			video_codec: self.video_codec,
			video_width: self.video_width,
			video_height: self.video_height,
			video_time_base_num: self.video_time_base_num,
			video_time_base_den: self.video_time_base_den,
			video_pixel_format: self.video_pixel_format,
			audio_enabled: self.audio_enabled,
			audio_codec: self.audio_codec,
			audio_sample_rate: self.audio_sample_rate,
			audio_channel_layout: self.audio_channel_layout,
			subtitles_enabled: self.subtitles_enabled,
			export_length_num: self.export_length_num,
			export_length_den: self.export_length_den,
			has_custom_range: self.has_custom_range,
			custom_range_in_num: self.custom_range_in_num,
			custom_range_in_den: self.custom_range_in_den,
			custom_range_out_num: self.custom_range_out_num,
			custom_range_out_den: self.custom_range_out_den,
			video_bit_rate: self.video_bit_rate,
			audio_bit_rate: self.audio_bit_rate,
			color_override_enabled: self.color_override_enabled,
			color_primaries: self.color_primaries,
			color_trc: self.color_trc,
			color_space: self.color_space,
		}
	}
}

// ---------------------------------------------------------------------------
// Task registry
// ---------------------------------------------------------------------------

/// `oak_task_state` result: the task is running.
pub const OAK_TASK_RUNNING: i32 = 0;
/// `oak_task_state` result: finished successfully.
pub const OAK_TASK_SUCCEEDED: i32 = 1;
/// `oak_task_state` result: failed.
pub const OAK_TASK_FAILED: i32 = 2;
/// `oak_task_state` result: cancelled.
pub const OAK_TASK_CANCELLED: i32 = 3;

struct TaskEntry {
	title: String,
	/// Progress in 0.0..=1.0 (f64 bits).
	progress: AtomicU64,
	state: AtomicI32,
	error: Mutex<Option<String>>,
	cancel: Arc<oak_core::cancelatom::CancelAtom>,
	join: Mutex<Option<std::thread::JoinHandle<()>>>,
	/// A load task's produced-project slot (shared with the behavior).
	project_slot: Option<Arc<Mutex<Option<Arc<Mutex<oak_node::project::Project>>>>>>,
	/// Successful waveform extraction result, transferred once to the caller.
	waveform_slot: Option<Arc<Mutex<Option<crate::audio_analysis::WaveformData>>>>,
	/// Successful thumbnail render result, transferred once to the caller.
	thumbnail_slot: Option<Arc<Mutex<Option<crate::thumbnail::ThumbnailData>>>>,
}

static TASKS: OnceLock<Mutex<HashMap<u64, Arc<TaskEntry>>>> = OnceLock::new();
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

fn tasks() -> &'static Mutex<HashMap<u64, Arc<TaskEntry>>> {
	TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn task(id: u64) -> Option<Arc<TaskEntry>> {
	tasks()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.get(&id)
		.cloned()
}

/// Spawns `behavior` on its own thread, returning the registry id.
fn spawn_task(
	title: String,
	behavior: Box<dyn TaskBehavior + Send>,
	project_slot: Option<Arc<Mutex<Option<Arc<Mutex<oak_node::project::Project>>>>>>,
	waveform_slot: Option<Arc<Mutex<Option<crate::audio_analysis::WaveformData>>>>,
	thumbnail_slot: Option<Arc<Mutex<Option<crate::thumbnail::ThumbnailData>>>>,
) -> u64 {
	let mut task = Task::new(&title, None);
	let entry = Arc::new(TaskEntry {
		title,
		progress: AtomicU64::new(0.0f64.to_bits()),
		state: AtomicI32::new(OAK_TASK_RUNNING),
		error: Mutex::new(None),
		cancel: task.get_cancel_atom(),
		join: Mutex::new(None),
		project_slot,
		waveform_slot,
		thumbnail_slot,
	});

	let listener_entry = entry.clone();
	task.set_event_listener(Box::new(move |ev| {
		if let TaskEvent::Progress(p) = ev {
			listener_entry.progress.store(p.to_bits(), Ordering::SeqCst);
		}
	}));
	task.set_behavior(behavior);

	let id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
	let thread_entry = entry.clone();
	let join = std::thread::spawn(move || {
		let result = task.start();
		let state = match &result {
			Ok(()) => OAK_TASK_SUCCEEDED,
			Err(oak_task::error::Error::Cancelled) => OAK_TASK_CANCELLED,
			Err(_) => OAK_TASK_FAILED,
		};
		if let Err(e) = &result {
			*thread_entry.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
		}
		thread_entry.state.store(state, Ordering::SeqCst);
	});
	*entry.join.lock().unwrap_or_else(|e| e.into_inner()) = Some(join);
	tasks()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.insert(id, entry);
	id
}

/// Starts one proxy transcode on the common engine task registry.
///
/// The caller owns the returned task ID and must eventually release it through
/// `oak_task_release`. Proxy metadata is deliberately updated by the editor
/// session, not by this worker: the worker only performs file I/O and reports
/// progress, while the session serializes graph mutations and task completion.
pub(crate) fn spawn_proxy(
	source_filename: &str,
	output_filename: &str,
	stream_index: i32,
	params: &oak_codec::proxymanager::ProxyParams,
) -> u64 {
	fn text(field: &[u8]) -> String {
		let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
		String::from_utf8_lossy(&field[..end]).into_owned()
	}
	let (proxy_width, proxy_height) = if params.divider <= 1 {
		(params.width, params.height)
	} else {
		(0, 0)
	};
	let request = oak_codec::task::TaskRequest {
		kind: oak_codec::task::TaskKind::Proxy,
		input_filename: source_filename,
		output_filename,
		stream_index,
		sample_rate: 0,
		channel_layout: 0,
		sample_format: 0,
		proxy_width,
		proxy_height,
	};
	let task_params = oak_task::proxy::ProxyParams {
		width: params.width,
		height: params.height,
		divider: params.divider,
		version: params.version,
		crf: params.crf,
		include_audio: params.include_audio != 0,
		extension: text(&params.extension),
		preset: text(&params.preset),
	};
	spawn_task(
		format!("Generating Proxy {source_filename}"),
		Box::new(oak_task::proxy::ProxyTask::new(&request, task_params)),
		None,
		None,
		None,
	)
}

struct ThumbnailRenderBehavior {
	project: Arc<Mutex<oak_node::project::Project>>,
	viewer: NodeId,
	time: oak_core::Rational,
	width: i32,
	height: i32,
	slot: Arc<Mutex<Option<crate::thumbnail::ThumbnailData>>>,
}

impl TaskBehavior for ThumbnailRenderBehavior {
	fn run(&mut self, task: &mut Task) -> oak_task::error::Result<()> {
		if task.is_cancelled() {
			return Err(oak_task::error::Error::Cancelled);
		}
		let pixels = crate::render::render_frame_f32_data(
			self.project.clone(),
			self.viewer,
			self.time,
			self.width,
			self.height,
		)
		.map_err(oak_task::error::Error::Failed)?;
		if task.is_cancelled() {
			return Err(oak_task::error::Error::Cancelled);
		}
		*self.slot.lock().unwrap_or_else(|e| e.into_inner()) =
			Some(crate::thumbnail::ThumbnailData {
				width: self.width,
				height: self.height,
				pixels,
			});
		Ok(())
	}
}

struct WaveformExtractBehavior {
	filename: String,
	stream_index: i32,
	samples_per_point: i32,
	slot: Arc<Mutex<Option<crate::audio_analysis::WaveformData>>>,
}

impl TaskBehavior for WaveformExtractBehavior {
	fn run(&mut self, task: &mut Task) -> oak_task::error::Result<()> {
		if task.is_cancelled() {
			return Err(oak_task::error::Error::Cancelled);
		}
		let waveform = crate::audio_analysis::extract_waveform(
			&self.filename,
			self.stream_index,
			self.samples_per_point,
		)
		.map_err(oak_task::error::Error::Failed)?;
		if task.is_cancelled() {
			return Err(oak_task::error::Error::Cancelled);
		}
		*self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(waveform);
		Ok(())
	}
}

/// Starts full audio-stream peak extraction on an engine worker.
/// Returns 0 for arguments that are known to be invalid before submission.
pub(crate) fn spawn_waveform_extract(
	filename: &str,
	stream_index: i32,
	samples_per_point: i32,
) -> u64 {
	if filename.is_empty() || stream_index < 0 || samples_per_point <= 0 {
		return 0;
	}
	let slot = Arc::new(Mutex::new(None));
	let behavior = WaveformExtractBehavior {
		filename: filename.to_string(),
		stream_index,
		samples_per_point,
		slot: slot.clone(),
	};
	spawn_task(
		format!("Analyzing Audio {filename}"),
		Box::new(behavior),
		None,
		Some(slot),
		None,
	)
}

/// Starts an owned F32 thumbnail render on an engine worker.
pub(crate) fn spawn_thumbnail_render(
	project: Arc<Mutex<oak_node::project::Project>>,
	viewer: NodeId,
	time: oak_core::Rational,
	width: i32,
	height: i32,
) -> u64 {
	if width <= 0 || height <= 0 {
		return 0;
	}
	let slot = Arc::new(Mutex::new(None));
	let behavior = ThumbnailRenderBehavior {
		project,
		viewer,
		time,
		width,
		height,
		slot: slot.clone(),
	};
	spawn_task(
		format!("Rendering Thumbnail {width}x{height}"),
		Box::new(behavior),
		None,
		None,
		Some(slot),
	)
}

/// Returns the configured upper bound for simultaneous proxy workers.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_proxy_max_concurrent() -> i32 {
	oak_task::proxy::ProxyTask::proxy_max_concurrent()
}

// ---------------------------------------------------------------------------
// Task constructors
// ---------------------------------------------------------------------------

/// Starts an export of the viewer node (a sequence or footage) to
/// `filename` with `params`. Returns the task id, 0 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_export(
	project: *const ProjectHandle,
	viewer: u64,
	filename: *const u8,
	filename_len: usize,
	params: *const OakTaskEncodingParams,
) -> u64 {
	if project.is_null() || params.is_null() {
		return 0;
	}
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return 0;
	};
	let Some(viewer) = NodeId::from_identity(viewer) else {
		return 0;
	};
	let project_ref = unsafe { &*project }.project().clone();
	let params = unsafe { &*params }.to_native(filename.to_string());
	let export = ExportTask::new((project_ref, viewer), params);
	spawn_task(format!("Exporting {filename}"), Box::new(export), None, None, None)
}

/// Starts a project save to an interchange format (`.otio` /
/// `.fcpxml`, chosen by the filename extension). Returns the task id,
/// 0 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_project_save_otio(
	project: *const ProjectHandle,
	filename: *const u8,
	filename_len: usize,
) -> u64 {
	if project.is_null() {
		return 0;
	}
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return 0;
	};
	let save = SaveOTIOTask {
		base: Task::new("Save Project", None),
		project: unsafe { &*project }.project().clone(),
		filename: filename.to_string(),
	};
	spawn_task(format!("Saving {filename}"), Box::new(save), None, None, None)
}

/// A load behavior that parks the produced project in a shared slot
/// for [`oak_task_take_project`].
struct LoadBehavior {
	inner: LoadOTIOTask,
	slot: Arc<Mutex<Option<Arc<Mutex<oak_node::project::Project>>>>>,
}

impl TaskBehavior for LoadBehavior {
	fn run(&mut self, task: &mut Task) -> oak_task::error::Result<()> {
		let result = self.inner.run(task);
		if result.is_ok() {
			*self.slot.lock().unwrap_or_else(|e| e.into_inner()) =
				self.inner.base.take_project().ok();
		}
		result
	}
}

/// Starts loading an interchange document (`.otio` / `.fcpxml`) as a
/// new project. Returns the task id, 0 on invalid input. The produced
/// project is taken with [`oak_task_take_project`] once the task
/// succeeds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_project_load_otio(
	filename: *const u8,
	filename_len: usize,
) -> u64 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return 0;
	};
	let slot: Arc<Mutex<Option<Arc<Mutex<oak_node::project::Project>>>>> =
		Arc::new(Mutex::new(None));
	let behavior = LoadBehavior {
		inner: LoadOTIOTask::new(ProjectLoadBaseTask::new(
			Task::new("Load Project", None),
			filename.to_string(),
		)),
		slot: slot.clone(),
	};
	spawn_task(
		format!("Loading {filename}"),
		Box::new(behavior),
		Some(slot),
		None,
		None,
	)
}

// ---------------------------------------------------------------------------
// Task handle operations
// ---------------------------------------------------------------------------

/// Starts asynchronous waveform extraction for one zero-based audio-stream
/// ordinal. The result is transferred with [`oak_task_take_waveform`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_waveform_extract(
	filename: *const u8,
	filename_len: usize,
	stream_index: i32,
	samples_per_point: i32,
) -> u64 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return 0;
	};
	spawn_waveform_extract(filename, stream_index, samples_per_point)
}

/// Starts an asynchronous thumbnail render. The task owns a strong reference
/// to the project, so a host may close its own handle while the worker runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_thumbnail_render(
	project: *const ProjectHandle,
	viewer: u64,
	time: OakRational,
	width: i32,
	height: i32,
) -> u64 {
	let (Some(project), Some(viewer)) = (unsafe { project.as_ref() }, NodeId::from_identity(viewer))
	else {
		return 0;
	};
	spawn_thumbnail_render(
		project.project().clone(),
		viewer,
		time.to_native(),
		width,
		height,
	)
}

/// The task title, snprintf-style; -1 on an unknown id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_title(id: u64, buf: *mut u8, buf_len: usize) -> i32 {
	let Some(entry) = task(id) else {
		return -1;
	};
	copy_str_out(&entry.title, buf, buf_len)
}

/// The task's progress in 0.0..=1.0; -1 on an unknown id.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_progress(id: u64) -> f64 {
	let Some(entry) = task(id) else {
		return -1.0;
	};
	f64::from_bits(entry.progress.load(Ordering::SeqCst))
}

/// The task's state: `OAK_TASK_RUNNING`/`SUCCEEDED`/`FAILED`/
/// `CANCELLED`; -1 on an unknown id.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_state(id: u64) -> i32 {
	task(id)
		.map(|e| e.state.load(Ordering::SeqCst))
		.unwrap_or(-1)
}

/// The task's error message ("" when none), snprintf-style; -1 on an
/// unknown id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_task_error(id: u64, buf: *mut u8, buf_len: usize) -> i32 {
	let Some(entry) = task(id) else {
		return -1;
	};
	let error = entry
		.error
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.clone();
	copy_str_out(error.as_deref().unwrap_or(""), buf, buf_len)
}

/// Requests cancellation (the task aborts at its next cancellation
/// point). Unknown ids are a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_cancel(id: u64) {
	if let Some(entry) = task(id) {
		entry.cancel.cancel();
	}
}

/// Blocks until the task finishes. True on success, false on failure,
/// cancellation, or an unknown id.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_wait(id: u64) -> bool {
	let Some(entry) = task(id) else {
		return false;
	};
	let join = entry.join.lock().unwrap_or_else(|e| e.into_inner()).take();
	if let Some(join) = join {
		let _ = join.join();
	}
	entry.state.load(Ordering::SeqCst) == OAK_TASK_SUCCEEDED
}

/// Removes the task from the registry (a running task is cancelled
/// and joined first). Unknown ids are a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_release(id: u64) {
	let entry = tasks()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.remove(&id);
	if let Some(entry) = entry {
		entry.cancel.cancel();
		let join = entry.join.lock().unwrap_or_else(|e| e.into_inner()).take();
		if let Some(join) = join {
			let _ = join.join();
		}
	}
}

/// Takes the produced project of a succeeded load task (a
/// [`ProjectHandle`] with ref count 1, usable with `oak_project_*`).
/// Null when the task has no project (unknown id, not a load task,
/// not finished, or already taken).
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_take_project(id: u64) -> *mut ProjectHandle {
	let Some(entry) = task(id) else {
		return std::ptr::null_mut();
	};
	if entry.state.load(Ordering::SeqCst) != OAK_TASK_SUCCEEDED {
		return std::ptr::null_mut();
	}
	let Some(slot) = &entry.project_slot else {
		return std::ptr::null_mut();
	};
	match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
		Some(inner) => ProjectHandle::from_arc(inner),
		None => std::ptr::null_mut(),
	}
}

/// Transfers a successful waveform result out of the task registry.
///
/// The returned handle has one owned reference and must be released with
/// `oak_waveform_release`. Null means unknown/wrong task type, unfinished or
/// failed task, or that the result was already taken.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_take_waveform(
	id: u64,
) -> *const crate::audio_analysis::WaveformHandle {
	let Some(entry) = task(id) else {
		return std::ptr::null();
	};
	if entry.state.load(Ordering::SeqCst) != OAK_TASK_SUCCEEDED {
		return std::ptr::null();
	}
	let data = entry
		.waveform_slot
		.as_ref()
		.and_then(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).take());
	data.map(crate::audio_analysis::WaveformHandle::from_data)
		.unwrap_or(std::ptr::null())
}

/// Transfers a successful thumbnail result out of the task registry.
/// The caller owns one reference and releases it with `oak_thumbnail_release`.
#[unsafe(no_mangle)]
pub extern "C" fn oak_task_take_thumbnail(id: u64) -> *const crate::thumbnail::ThumbnailHandle {
	let Some(entry) = task(id) else {
		return std::ptr::null();
	};
	if entry.state.load(Ordering::SeqCst) != OAK_TASK_SUCCEEDED {
		return std::ptr::null();
	}
	let data = entry
		.thumbnail_slot
		.as_ref()
		.and_then(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).take());
	data.map(crate::thumbnail::ThumbnailHandle::from_data)
		.unwrap_or(std::ptr::null())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Save → load round-trip through the OTIO task path: the produced
	/// project comes back as an engine handle.
	#[test]
	fn save_then_load_otio_roundtrip() {
		// The save/load path pushes undo entries (clip placement);
		// serialize against the other undo-stack tests.
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		unsafe {
			let path = std::env::temp_dir()
				.join(format!("oakengine_task_{}.otio", std::process::id()))
				.to_string_lossy()
				.into_owned();

			let project = crate::node::oak_project_new();
			let (n, nl) = ("Seq 1".as_ptr(), 5);
			let seq = crate::node::oak_sequence_create(project, n, nl);
			assert!(crate::node::oak_node_id_is_valid(seq));
			// An empty sequence is not exported ("no edited sequences");
			// put one clip on the video track.
			let list = crate::node::oak_sequence_track_list_at(project, seq, 0);
			let track = crate::node::oak_tracklist_track_at(project, list, 0);
			let clip = crate::node::oak_block_create_clip(project);
			assert!(crate::node::oak_block_set_length(
				project,
				clip,
				crate::coretypes::OakRational { num: 2, den: 1 }
			));
			assert!(crate::node::oak_track_append_block(project, track, clip));

			let (f, fl) = (path.as_ptr(), path.len());
			let save = oak_task_project_save_otio(project, f, fl);
			assert_ne!(save, 0);
			if !oak_task_wait(save) {
				let mut buf = [0u8; 256];
				let n = oak_task_error(save, buf.as_mut_ptr(), 256);
				panic!(
					"save task failed: {}",
					std::str::from_utf8(&buf[..n.max(0) as usize]).unwrap_or("?")
				);
			}
			assert_eq!(oak_task_state(save), OAK_TASK_SUCCEEDED);
			oak_task_release(save);

			let load = oak_task_project_load_otio(f, fl);
			assert_ne!(load, 0);
			assert!(oak_task_wait(load));
			let loaded = oak_task_take_project(load);
			assert!(!loaded.is_null());
			assert!(crate::node::oak_node_id_is_valid(
				crate::node::oak_project_root(loaded)
			));
			// A second take yields nothing.
			assert!(oak_task_take_project(load).is_null());
			oak_task_release(load);

			crate::node::oak_project_release(loaded);
			crate::node::oak_project_release(project);
			let _ = std::fs::remove_file(&path);
		}
	}

	/// A real export: an (empty) sequence to a tiny mp4 through the
	/// task path (render + encode + progress events).
	#[test]
	fn export_empty_sequence_to_mp4() {
		unsafe {
			let path = std::env::temp_dir()
				.join(format!("oakengine_export_{}.mp4", std::process::id()))
				.to_string_lossy()
				.into_owned();
			// The default render backend is the worker process pool (no
			// oak-worker binary in tests); the test-only inline backend
			// runs jobs synchronously in-process.
			if oak_render::manager::RenderManager::global().is_none() {
				let _ = oak_render::manager::RenderManager::init_with_backend(
					oak_render::manager::RenderBackendChoice::Threads,
				);
			}
			let project = crate::node::oak_project_new();
			let (n, nl) = ("Seq".as_ptr(), 3);
			let seq = crate::node::oak_sequence_create(project, n, nl);
			assert!(crate::node::oak_node_id_is_valid(seq));

			let params = OakTaskEncodingParams {
				format: 2, // MPEG-4
				video_enabled: true,
				video_codec: 1, // H.264
				video_width: 64,
				video_height: 64,
				video_time_base_num: 1,
				video_time_base_den: 10,
				video_pixel_format: 4, // F32
				has_custom_range: true,
				custom_range_out_num: 1,
				custom_range_out_den: 2, // 0.5 s → 5 frames
				..Default::default()
			};
			let (f, fl) = (path.as_ptr(), path.len());
			let id = oak_task_export(project, seq, f, fl, &params);
			assert_ne!(id, 0);
			if !oak_task_wait(id) {
				let mut buf = [0u8; 256];
				let n = oak_task_error(id, buf.as_mut_ptr(), 256);
				panic!(
					"export failed: {}",
					std::str::from_utf8(&buf[..n.max(0) as usize]).unwrap_or("?")
				);
			}
			assert_eq!(oak_task_state(id), OAK_TASK_SUCCEEDED);
			assert!(
				std::path::Path::new(&path)
					.metadata()
					.map(|m| m.len() > 0)
					.unwrap_or(false)
			);
			oak_task_release(id);

			// Invalid input fails closed.
			assert_eq!(oak_task_export(std::ptr::null(), seq, f, fl, &params), 0);
			assert_eq!(oak_task_export(project, u64::MAX, f, fl, &params), 0);
			assert_eq!(oak_task_export(project, seq, f, fl, std::ptr::null()), 0);

			crate::node::oak_project_release(project);
			let _ = std::fs::remove_file(&path);
		}
	}

	#[test]
	fn unknown_ids_fail_closed() {
		unsafe {
			assert_eq!(oak_task_state(u64::MAX), -1);
			assert_eq!(oak_task_progress(u64::MAX), -1.0);
			assert!(!oak_task_wait(u64::MAX));
			assert_eq!(oak_task_title(u64::MAX, std::ptr::null_mut(), 0), -1);
			assert!(oak_task_take_project(u64::MAX).is_null());
			oak_task_cancel(u64::MAX);
			oak_task_release(u64::MAX);
			let mut buf = [0u8; 16];
			assert_eq!(oak_task_error(u64::MAX, buf.as_mut_ptr(), 16), -1);
		}
	}
}

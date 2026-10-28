//! C ABI for oak-render (prefix `oak_render_`): render a project's
//! viewer frame (CPU, F32 RGBA) and manage the disk cache.
//!
//! [`oak_render_frame_f32`] is the headless frame-grab a functional
//! plugin builds on ("render.get_frame"): render the graph of any
//! sequence/viewer node at a given time into a caller buffer. OFX
//! effect nodes on the graph need the plugin executor installed first
//! (`oak_ofx_scan` + `oak_ofx_install_render_executor`).
//!
//! The process pool / scheduler / pipeline are host-side
//! infrastructure and intentionally not exported; cancellation uses
//! the existing `oak_core_cancelatom_*` surface.

use oak_core::PixelFormat;
use std::sync::{Arc, Mutex};

use oak_node::id::NodeId;

use crate::codec::copy_str_out;
use crate::coretypes::OakRational;
use crate::handle::NativeMirror;
use crate::node::ProjectHandle;

// ---------------------------------------------------------------------------
// Render manager lifecycle
// ---------------------------------------------------------------------------

/// `oak_render_manager_init` backend: the `OAK_PIPELINE` environment
/// default (the process pool unless overridden).
pub const OAK_RENDER_BACKEND_DEFAULT: i32 = 0;
/// `oak_render_manager_init` backend: the test-only inline backend
/// (jobs run synchronously on the calling thread; the only backend
/// that works without the oak-worker binary — e.g. a plugin process).
pub const OAK_RENDER_BACKEND_THREADS: i32 = 1;
/// `oak_render_manager_init` backend: the M1 thread pipeline
/// (one render thread + one decode thread, in-process).
pub const OAK_RENDER_BACKEND_PIPELINE: i32 = 2;
/// `oak_render_manager_init` backend: the process-isolated oak-worker
/// pool (needs the oak-worker binary).
pub const OAK_RENDER_BACKEND_PROCESSES: i32 = 3;

/// Initializes the process-wide render manager with the chosen backend
/// (one of `OAK_RENDER_BACKEND_*`). The render/export tasks
/// (`oak_task_export`) need a manager — or a worker pool — to render.
/// False when already initialized or on an invalid backend.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_manager_init(backend: i32) -> bool {
	use oak_render::manager::{RenderBackendChoice, RenderManager};
	let choice = match backend {
		OAK_RENDER_BACKEND_DEFAULT => return RenderManager::init().is_ok(),
		OAK_RENDER_BACKEND_THREADS => RenderBackendChoice::Threads,
		OAK_RENDER_BACKEND_PIPELINE => RenderBackendChoice::Pipeline,
		OAK_RENDER_BACKEND_PROCESSES => RenderBackendChoice::Processes(Default::default()),
		_ => return false,
	};
	RenderManager::init_with_backend(choice).is_ok()
}

/// Whether the render manager is initialized.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_manager_is_initialized() -> bool {
	oak_render::manager::RenderManager::global().is_some()
}

/// Shuts the render manager down (drains pending work). No-op when not
/// initialized.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_manager_shutdown() {
	oak_render::manager::RenderManager::shutdown();
}

/// Installs the text measure/render backends (idempotent; text nodes
/// render empty without them).
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_textengine_install() {
	oak_render::textengine::install();
}

/// Number of font families exposed by the shared text backend.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_textengine_font_family_count() -> i32 {
	font_families().len() as i32
}

fn font_families() -> &'static [String] {
	static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
	FAMILIES.get_or_init(oak_render::textengine::font_families)
}

/// A font family name, snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_render_textengine_font_family_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if index < 0 {
		return -1;
	}
	match font_families().get(index as usize) {
		Some(name) => crate::codec::copy_str_out(name, buf, buf_len),
		None => -1,
	}
}

/// Swizzles BGRA8 pixels to RGBA8 (`src` and `dst` may not overlap);
/// both buffers are `pixel_count*4` bytes. False on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_render_swizzle_bgra8_to_rgba8(
	src: *const u8,
	dst: *mut u8,
	pixel_count: usize,
) -> bool {
	if src.is_null() || dst.is_null() {
		return false;
	}
	let src = unsafe { std::slice::from_raw_parts(src, pixel_count * 4) };
	let dst = unsafe { std::slice::from_raw_parts_mut(dst, pixel_count * 4) };
	for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
		d[0] = s[2]; // R
		d[1] = s[1]; // G
		d[2] = s[0]; // B
		d[3] = s[3]; // A
	}
	true
}

/// Renders a sequence or video-footage viewer at `time` into `dst` as
/// tight F32 RGBA (`width*height*4` floats). Returns the float count
/// written, -1 on a null handle, an unknown viewer, invalid sizes, an
/// undersized buffer, or a render failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_render_frame_f32(
	project: *const ProjectHandle,
	viewer: u64,
	time: OakRational,
	width: i32,
	height: i32,
	dst: *mut f32,
	dst_capacity: usize,
) -> i32 {
	if project.is_null() || dst.is_null() || width <= 0 || height <= 0 {
		return -1;
	}
	let needed = width as usize * height as usize * 4;
	if dst_capacity < needed {
		return -1;
	}
	let Some(viewer) = NodeId::from_identity(viewer) else {
		return -1;
	};
	let project = unsafe { &*project }.project();
	let Ok(pixels) = render_frame_f32_data(
		project.clone(),
		viewer,
		time.to_native(),
		width,
		height,
	) else {
		return -1;
	};
	unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), dst, needed) };
	needed as i32
}

/// Renders and tightly packs an owned F32 RGBA frame for synchronous callers
/// and asynchronous thumbnail tasks. Keeping stride handling here prevents the
/// C ABI and task DTO from disagreeing about padded GPU readbacks.
pub(crate) fn render_frame_f32_data(
	project: Arc<Mutex<oak_node::project::Project>>,
	viewer: NodeId,
	time: oak_core::Rational,
	width: i32,
	height: i32,
) -> Result<Vec<f32>, String> {
	if width <= 0 || height <= 0 {
		return Err("invalid frame dimensions".into());
	}
	enum Viewer {
		Sequence,
		Footage(String, i32),
	}
	let viewer_kind = {
		let guard = project.lock().unwrap_or_else(|e| e.into_inner());
		let entry = guard.graph.get(viewer).ok_or("viewer node does not exist")?;
		if entry
			.behavior
			.as_any()
			.and_then(|behavior| behavior.downcast_ref::<oak_node::sequence::SequenceBehavior>())
			.is_some()
		{
			Viewer::Sequence
		} else if let Some(footage) = entry
			.behavior
			.as_any()
			.and_then(|behavior| behavior.downcast_ref::<oak_node::footage::FootageBehavior>())
		{
			let stream = footage
				.streams
				.iter()
				.find(|stream| stream.is_video)
				.map(|stream| stream.index)
				.ok_or("footage has no video stream")?;
			Viewer::Footage(footage.filename.clone(), stream)
		} else {
			return Err("viewer is neither a sequence nor video footage".into());
		}
	};
	let texture = match viewer_kind {
		Viewer::Sequence => oak_render::eval::render_graph_frame(
			&project,
			viewer,
			time,
			(width, height),
			PixelFormat::F32,
		),
		Viewer::Footage(filename, stream) => oak_render::eval::render_footage_frame(
			&filename,
			stream,
			time,
			(width, height),
			PixelFormat::F32,
		),
	}
	.map_err(|e| e.to_string())?;
	let Ok(frame) = texture.to_frame() else {
		return Err("failed to read back the rendered frame".into());
	};
	let params = frame.video_params();
	if params.width != width || params.height != height || params.format != PixelFormat::F32 as i32
	{
		return Err("render returned an unexpected pixel format".into());
	}
	let tight = width as usize * 4 * 4;
	let row = frame.linesize_bytes().max(tight);
	if frame.data.len() < row * (height as usize - 1) + tight {
		return Err("render returned a truncated pixel buffer".into());
	}
	let mut pixels = vec![0.0f32; width as usize * height as usize * 4];
	let out = unsafe {
		std::slice::from_raw_parts_mut(pixels.as_mut_ptr() as *mut u8, tight * height as usize)
	};
	for y in 0..height as usize {
		out[y * tight..(y + 1) * tight].copy_from_slice(&frame.data[y * row..y * row + tight]);
	}
	Ok(pixels)
}

/// The disk cache directory, snprintf-style.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_render_disk_cache_path(buf: *mut u8, buf_len: usize) -> i32 {
	copy_str_out(&oak_render::manager::disk_cache_path(), buf, buf_len)
}

/// The disk cache size in bytes; -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_disk_cache_size() -> i64 {
	oak_render::manager::disk_cache_size().unwrap_or(-1)
}

/// Clears the disk cache. False on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_render_disk_cache_clear() -> bool {
	oak_render::manager::disk_cache_clear().is_ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A fresh sequence renders to transparent black through the C ABI.
	#[test]
	fn render_empty_sequence_frame() {
		unsafe {
			let project = crate::node::oak_project_new();
			assert!(!project.is_null());
			let (n, nl) = ("Seq".as_ptr(), 3);
			let seq = crate::node::oak_sequence_create(project, n, nl);
			assert!(crate::node::oak_node_id_is_valid(seq));

			let (w, h) = (16, 8);
			let mut dst = vec![1.0f32; w * h * 4];
			let written = oak_render_frame_f32(
				project,
				seq,
				OakRational { num: 0, den: 1 },
				w as i32,
				h as i32,
				dst.as_mut_ptr(),
				dst.len(),
			);
			assert_eq!(written, (w * h * 4) as i32);
			assert!(
				dst.iter().all(|&v| v == 0.0),
				"an empty sequence is transparent black"
			);

			// Fail closed: unknown viewer, bad sizes, undersized buffer.
			assert_eq!(
				oak_render_frame_f32(
					project,
					u64::MAX,
					OakRational { num: 0, den: 1 },
					w as i32,
					h as i32,
					dst.as_mut_ptr(),
					dst.len()
				),
				-1
			);
			assert_eq!(
				oak_render_frame_f32(
					project,
					seq,
					OakRational { num: 0, den: 1 },
					0,
					h as i32,
					dst.as_mut_ptr(),
					dst.len()
				),
				-1
			);
			assert_eq!(
				oak_render_frame_f32(
					project,
					seq,
					OakRational { num: 0, den: 1 },
					w as i32,
					h as i32,
					dst.as_mut_ptr(),
					4
				),
				-1
			);
			crate::node::oak_project_release(project);
		}
	}

	/// Manager lifecycle + swizzle + textengine install.
	#[test]
	fn manager_lifecycle_and_swizzle() {
		// The manager may already be initialized (the export test);
		// either way a second init is rejected and invalid backends fail.
		let was = oak_render_manager_is_initialized();
		if !was {
			assert!(oak_render_manager_init(OAK_RENDER_BACKEND_THREADS));
		}
		assert!(oak_render_manager_is_initialized());
		assert!(
			!oak_render_manager_init(OAK_RENDER_BACKEND_THREADS),
			"double init rejected"
		);
		assert!(!oak_render_manager_init(99), "invalid backend rejected");
		oak_render_textengine_install();
		oak_render_textengine_install();
		// No shutdown: other tests may be using the manager.

		let src = [10u8, 20, 30, 255, 1, 2, 3, 4];
		let mut dst = [0u8; 8];
		assert!(unsafe { oak_render_swizzle_bgra8_to_rgba8(src.as_ptr(), dst.as_mut_ptr(), 2) });
		assert_eq!(dst, [30, 20, 10, 255, 3, 2, 1, 4]);
		assert!(!unsafe {
			oak_render_swizzle_bgra8_to_rgba8(std::ptr::null(), dst.as_mut_ptr(), 2)
		});
	}

	#[test]
	fn disk_cache_entry_points_are_safe() {
		let mut buf = [0u8; 1024];
		let n = unsafe { oak_render_disk_cache_path(buf.as_mut_ptr(), buf.len()) };
		assert!(n > 0);
		// Size/clear are environment-dependent; they must not panic and
		// report a coherent shape.
		let size = oak_render_disk_cache_size();
		assert!(size >= -1);
	}
}

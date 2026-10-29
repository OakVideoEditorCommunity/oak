//! Safe wrapper over the engine's render C ABI (`oak_render_*`):
//! render a project's viewer frame (CPU, F32 RGBA) and manage the disk
//! cache.
//!
//! [`render_frame_f32`] is the headless frame-grab a functional plugin
//! builds on. OFX effect nodes on the graph need the plugin executor
//! installed first ([`crate::ofx::scan`] +
//! [`crate::ofx::install_render_executor`]).

use std::ffi::c_void;

use crate::codec::string_out;
use crate::node::{NodeId, Project};
use crate::types::Rational;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_render_frame_f32(
		project: *const c_void,
		viewer: u64,
		time: Rational,
		width: i32,
		height: i32,
		dst: *mut f32,
		dst_capacity: usize,
	) -> i32;
	fn oak_render_disk_cache_path(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_render_disk_cache_size() -> i64;
	fn oak_render_disk_cache_clear() -> bool;
	fn oak_render_manager_init(backend: i32) -> bool;
	fn oak_render_manager_is_initialized() -> bool;
	fn oak_render_manager_shutdown();
	fn oak_render_manager_poll();
	fn oak_render_manager_set_workspace_size(width: i32, height: i32, fps: u32);
	fn oak_render_manager_set_inline_project(project: *const c_void) -> bool;
	fn oak_render_manager_set_graph_snapshot(project: *const c_void, revision: u64) -> bool;
	fn oak_render_manager_resync_graph_snapshot(project: *const c_void, revision: u64) -> bool;
	fn oak_render_manager_clear_graph_snapshot();
	fn oak_render_textengine_install();
	fn oak_render_textengine_font_family_count() -> i32;
	fn oak_render_textengine_font_family_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_render_swizzle_bgra8_to_rgba8(src: *const u8, dst: *mut u8, pixel_count: usize) -> bool;
}

/// Renders a sequence or video-footage viewer at `time`, returning tight
/// F32 RGBA pixels (`width*height*4` floats). None on failure (unknown
/// viewer, invalid size, render error).
pub fn render_frame_f32(
	project: &Project,
	viewer: NodeId,
	time: Rational,
	size: (u32, u32),
) -> Option<Vec<f32>> {
	let (w, h) = size;
	if w == 0 || h == 0 {
		return None;
	}
	let mut dst = vec![0.0f32; (w * h * 4) as usize];
	let written = unsafe {
		oak_render_frame_f32(
			project.as_raw(),
			viewer.raw(),
			time,
			w as i32,
			h as i32,
			dst.as_mut_ptr(),
			dst.len(),
		)
	};
	if written < 0 {
		return None;
	}
	dst.truncate(written as usize);
	Some(dst)
}

/// The disk cache directory.
pub fn disk_cache_path() -> String {
	string_out(|buf, len| unsafe { oak_render_disk_cache_path(buf, len) }).unwrap_or_default()
}

/// The disk cache size in bytes.
pub fn disk_cache_size() -> Option<i64> {
	let size = unsafe { oak_render_disk_cache_size() };
	(size >= 0).then_some(size)
}

/// Clears the disk cache.
pub fn disk_cache_clear() -> bool {
	unsafe { oak_render_disk_cache_clear() }
}

/// The render backend for [`manager_init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBackend {
	/// The `OAK_PIPELINE` environment default (the process pool unless
	/// overridden).
	Default,
	/// The inline backend: jobs run synchronously on the calling
	/// thread — the only backend that works without the oak-worker
	/// binary (e.g. a plugin process).
	Threads,
	/// The M1 thread pipeline (one render thread + one decode thread,
	/// in-process).
	Pipeline,
	/// The process-isolated oak-worker pool (needs the oak-worker
	/// binary).
	Processes,
}

/// Initializes the process-wide render manager. The render/export
/// tasks ([`crate::task::export`]) need a manager — or a worker pool —
/// to render. False when already initialized.
pub fn manager_init(backend: RenderBackend) -> bool {
	let backend = match backend {
		RenderBackend::Default => 0,
		RenderBackend::Threads => 1,
		RenderBackend::Pipeline => 2,
		RenderBackend::Processes => 3,
	};
	unsafe { oak_render_manager_init(backend) }
}

/// Whether the render manager is initialized.
pub fn manager_is_initialized() -> bool {
	unsafe { oak_render_manager_is_initialized() }
}

/// Shuts the render manager down (drains pending work).
pub fn manager_shutdown() {
	unsafe { oak_render_manager_shutdown() }
}

/// Pumps the manager's completion dispatcher; call once per UI tick.
pub fn manager_poll() {
	unsafe { oak_render_manager_poll() }
}

/// Sizes the worker pool's workspace for the current sequence resolution.
pub fn manager_set_workspace_size(width: i32, height: i32, fps: u32) {
	unsafe { oak_render_manager_set_workspace_size(width, height, fps) }
}

/// Points the inline backend at the live project (same shared project).
pub fn manager_set_inline_project(project: &Project) -> bool {
	unsafe { oak_render_manager_set_inline_project(project.as_raw()) }
}

/// Uploads the project snapshot at `revision` to the worker pool.
pub fn manager_set_graph_snapshot(project: &Project, revision: u64) -> bool {
	unsafe { oak_render_manager_set_graph_snapshot(project.as_raw(), revision) }
}

/// Re-uploads the snapshot although the undo revision did not change.
pub fn manager_resync_graph_snapshot(project: &Project, revision: u64) -> bool {
	unsafe { oak_render_manager_resync_graph_snapshot(project.as_raw(), revision) }
}

/// Drops the manager's current snapshot (project close).
pub fn manager_clear_graph_snapshot() {
	unsafe { oak_render_manager_clear_graph_snapshot() }
}

/// Installs the text measure/render backends (idempotent; text nodes
/// render empty without them).
pub fn textengine_install() {
	unsafe { oak_render_textengine_install() }
}

/// System font families from the same backend used by render workers.
pub fn font_families() -> Vec<String> {
	let count = unsafe { oak_render_textengine_font_family_count() }.max(0);
	(0..count)
		.filter_map(|index| {
			string_out(|buf, len| unsafe {
				oak_render_textengine_font_family_at(index, buf, len)
			})
		})
		.collect()
}

/// Swizzles BGRA8 pixels to RGBA8 (buffers may not overlap).
pub fn swizzle_bgra8_to_rgba8(src: &[u8], dst: &mut [u8]) -> bool {
	if src.len() != dst.len() || !src.len().is_multiple_of(4) {
		return false;
	}
	unsafe { oak_render_swizzle_bgra8_to_rgba8(src.as_ptr(), dst.as_mut_ptr(), src.len() / 4) }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn render_empty_sequence_frame() {
		let project = Project::new();
		let sequence = project.create_sequence("Seq").expect("sequence");
		let frame = render_frame_f32(&project, sequence, Rational::new(0, 1), (16, 8))
			.expect("an empty sequence renders");
		assert_eq!(frame.len(), 16 * 8 * 4);
		assert!(frame.iter().all(|&v| v == 0.0), "transparent black");
		// An unknown viewer fails closed.
		assert!(
			render_frame_f32(&project, NodeId::INVALID, Rational::new(0, 1), (16, 8)).is_none()
		);
	}

	#[test]
	fn disk_cache_entry_points_are_safe() {
		assert!(!disk_cache_path().is_empty());
		let _ = disk_cache_size();
	}

	#[test]
	fn manager_and_swizzle() {
		let was = manager_is_initialized();
		if !was {
			assert!(manager_init(RenderBackend::Threads));
		}
		assert!(manager_is_initialized());
		assert!(
			!manager_init(RenderBackend::Threads),
			"double init rejected"
		);

		let src = [10u8, 20, 30, 255];
		let mut dst = [0u8; 4];
		assert!(swizzle_bgra8_to_rgba8(&src, &mut dst));
		assert_eq!(dst, [30, 20, 10, 255]);
		assert!(!swizzle_bgra8_to_rgba8(&src, &mut dst[..3]));
	}

	#[test]
	fn text_backend_install_and_font_families() {
		textengine_install();
		let families = font_families();
		assert!(!families.is_empty(), "the host exposes font families");
		assert!(families.iter().all(|name| !name.is_empty()));
		assert_eq!(families, font_families(), "the engine caches the enumeration");
	}
}

//! Owned CPU thumbnail DTO returned by engine render tasks.

use std::ffi::c_void;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_thumbnail_add_ref(thumbnail: *const c_void);
	fn oak_thumbnail_release(thumbnail: *const c_void);
	fn oak_thumbnail_info(thumbnail: *const c_void, out: *mut ThumbnailInfo) -> bool;
	fn oak_thumbnail_copy_pixels(thumbnail: *const c_void, dst: *mut f32, capacity: usize)
	-> usize;
}

/// Thumbnail dimensions and packed pixel-buffer length.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThumbnailInfo {
	/// The `width` field.
	pub width: i32,
	/// The `height` field.
	pub height: i32,
	/// Number of F32 values; equals `width * height * 4` for valid results.
	pub pixel_count: usize,
}

/// Reference-counted immutable F32 RGBA thumbnail.
pub struct Thumbnail {
	ptr: *const c_void,
}

// SAFETY: the engine allocation is reference counted and immutable.
unsafe impl Send for Thumbnail {}
// SAFETY: all shared accessors only read or copy the immutable pixels.
unsafe impl Sync for Thumbnail {}

impl Thumbnail {
	pub(crate) fn from_raw(ptr: *const c_void) -> Option<Self> {
		(!ptr.is_null()).then_some(Self { ptr })
	}

	/// Returns dimensions and the exact pixel-vector capacity.
	pub fn info(&self) -> ThumbnailInfo {
		let mut info = ThumbnailInfo::default();
		unsafe { oak_thumbnail_info(self.ptr, &mut info) }
			.then_some(info)
			.unwrap_or_default()
	}

	/// Copies the owned tight F32 RGBA pixels out of the engine handle.
	pub fn pixels(&self) -> Vec<f32> {
		let info = self.info();
		let mut pixels = vec![0.0; info.pixel_count];
		let written =
			unsafe { oak_thumbnail_copy_pixels(self.ptr, pixels.as_mut_ptr(), pixels.len()) };
		if written == pixels.len() {
			pixels
		} else {
			Vec::new()
		}
	}
}

impl Clone for Thumbnail {
	fn clone(&self) -> Self {
		unsafe { oak_thumbnail_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for Thumbnail {
	fn drop(&mut self) {
		unsafe { oak_thumbnail_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn thumbnail_task_returns_owned_rgba_pixels_once() {
		let project = crate::node::Project::new();
		let media = crate::test_support::make_test_clip("thumbnail_task");
		let footage = project.import_footage(&media).expect("footage");
		let task = crate::task::render_thumbnail(
			&project,
			footage,
			crate::types::Rational::new(0, 1),
			(16, 12),
		)
		.expect("thumbnail task");
		assert!(task.title().contains("Rendering Thumbnail"));
		assert!(task.wait(), "thumbnail render failed: {}", task.error());
		let thumbnail = task.take_thumbnail().expect("thumbnail result");
		assert!(task.take_thumbnail().is_none());
		let info = thumbnail.info();
		assert_eq!((info.width, info.height), (16, 12));
		assert_eq!(info.pixel_count, 16 * 12 * 4);
		let pixels = thumbnail.pixels();
		assert_eq!(pixels.len(), info.pixel_count);
		assert!(pixels.iter().all(|value| value.is_finite()));
		let cloned = thumbnail.clone();
		assert_eq!(cloned.pixels(), pixels);
		drop(cloned);
		assert_eq!(thumbnail.pixels(), pixels);
		let _ = std::fs::remove_file(media);
	}
}

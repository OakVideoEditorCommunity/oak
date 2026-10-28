//! Immutable owned thumbnail frames exported by asynchronous render tasks.

use crate::handle::NativeMirror;

/// CPU thumbnail pixels in tight F32 RGBA order.
#[derive(Clone, Debug)]
pub struct ThumbnailData {
	pub width: i32,
	pub height: i32,
	pub pixels: Vec<f32>,
}

/// Thumbnail layout without the variable-length pixel buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThumbnailMetadata {
	/// Image width in pixels.
	pub width: i32,
	/// Image height in pixels.
	pub height: i32,
	/// Number of F32 values in the packed RGBA buffer.
	pub pixel_count: usize,
}

impl ThumbnailData {
	fn metadata(&self) -> ThumbnailMetadata {
		ThumbnailMetadata {
			width: self.width,
			height: self.height,
			pixel_count: self.pixels.len(),
		}
	}
}

/// Opaque reference-counted thumbnail result.
pub struct ThumbnailHandle {
	pub(crate) data: ThumbnailData,
}

crate::export_handle!(
	ThumbnailHandle,
	oak_thumbnail_add_ref,
	oak_thumbnail_release
);

impl ThumbnailHandle {
	pub(crate) fn from_data(data: ThumbnailData) -> *const Self {
		crate::handle::into_ffi(Self { data })
	}
}

/// Width, height and number of floats in the packed RGBA pixel buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakThumbnailInfo {
	/// Image width in pixels.
	pub width: i32,
	/// Image height in pixels.
	pub height: i32,
	/// Number of F32 values in the packed RGBA buffer.
	pub pixel_count: usize,
}

impl NativeMirror for OakThumbnailInfo {
	type Native = ThumbnailMetadata;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			width: native.width,
			height: native.height,
			pixel_count: native.pixel_count,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			width: self.width,
			height: self.height,
			pixel_count: self.pixel_count,
		}
	}
}

/// Reads thumbnail metadata; false for null arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_thumbnail_info(
	thumbnail: *const ThumbnailHandle,
	out: *mut OakThumbnailInfo,
) -> bool {
	let (Some(thumbnail), Some(out)) = (unsafe { thumbnail.as_ref() }, unsafe { out.as_mut() })
	else {
		return false;
	};
	*out = OakThumbnailInfo::from_native(&thumbnail.data.metadata());
	true
}

/// Copies the packed F32 RGBA pixels into caller storage.
///
/// Returns the required float count, or 0 for invalid arguments/insufficient
/// capacity. Querying [`oak_thumbnail_info`] first provides the exact capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_thumbnail_copy_pixels(
	thumbnail: *const ThumbnailHandle,
	dst: *mut f32,
	dst_capacity: usize,
) -> usize {
	let Some(thumbnail) = (unsafe { thumbnail.as_ref() }) else {
		return 0;
	};
	if dst.is_null() || dst_capacity < thumbnail.data.pixels.len() {
		return 0;
	}
	unsafe {
		std::ptr::copy_nonoverlapping(
			thumbnail.data.pixels.as_ptr(),
			dst,
			thumbnail.data.pixels.len(),
		)
	};
	thumbnail.data.pixels.len()
}

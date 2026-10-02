//! C ABI for `oak_codec::frame::Frame` (prefix `oak_codec_frame_`): the
//! reference-counted CPU pixel buffer plugins read and write.
//!
//! The handle is Arc-backed (add_ref/release pair from
//! `export_handle!`). Mutation (`allocate`, `set_params`,
//! `set_timestamp`, writing through the `data` pointer) requires the
//! caller to hold the only live reference — the same discipline the C++
//! Frame has. The `data` pointer stays valid until the next `allocate`
//! or `set_params` on the frame, or its last release.

use oak_codec::frame::Frame;

use crate::coretypes::{OakRational, OakVideoParams};
use crate::handle::{NativeMirror, into_ffi};

crate::export_handle!(Frame, oak_codec_frame_add_ref, oak_codec_frame_release);

/// An empty frame with default (invalid) params, ref count 1.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_frame_new() -> *mut Frame {
	into_ffi(Frame::new()) as *mut Frame
}

/// A frame with a copy of `params` (line sizes computed), ref count 1.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_frame_with_params(params: OakVideoParams) -> *mut Frame {
	into_ffi(Frame::with_params(params.to_native())) as *mut Frame
}

/// Allocates the pixel buffer per the current params. False on null,
/// invalid params, or allocation failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_allocate(this: *mut Frame) -> bool {
	if this.is_null() {
		return false;
	}
	unsafe { &mut *this }.allocate().is_ok()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_is_allocated(this: *const Frame) -> bool {
	!this.is_null() && unsafe { &*this }.is_allocated()
}

/// The pixel buffer (writable); null until allocated. See the module
/// docs for the pointer's validity window.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_data(this: *mut Frame) -> *mut u8 {
	if this.is_null() {
		return std::ptr::null_mut();
	}
	match unsafe { &mut *this }.data_mut() {
		Some(data) => data.as_mut_ptr(),
		None => std::ptr::null_mut(),
	}
}

/// The allocated buffer size in bytes (0 when unallocated).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_allocated_size(this: *const Frame) -> usize {
	if this.is_null() {
		return 0;
	}
	unsafe { &*this }.allocated_size()
}

macro_rules! frame_getter {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("`Frame::", stringify!($native), "` (0 on null).")]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(this: *const Frame) -> i32 {
				if this.is_null() {
					return 0;
				}
				unsafe { &*this }.$native()
			}
		)*
	};
}

frame_getter! {
	oak_codec_frame_linesize_bytes => linesize_bytes,
	oak_codec_frame_linesize_pixels => linesize_pixels,
	oak_codec_frame_width => width,
	oak_codec_frame_height => height,
	oak_codec_frame_channel_count => channel_count,
}

/// The allocated pixel format as a PixelFormat code (0 on null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_format(this: *const Frame) -> i32 {
	if this.is_null() {
		return 0;
	}
	unsafe { &*this }.format() as i32
}

/// The frame timestamp (rational seconds).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_timestamp(this: *const Frame) -> OakRational {
	if this.is_null() {
		return OakRational { num: 0, den: 0 };
	}
	OakRational::from_native(&unsafe { &*this }.timestamp())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_set_timestamp(this: *mut Frame, ts: OakRational) {
	if this.is_null() {
		return;
	}
	unsafe { &mut *this }.set_timestamp(ts.to_native());
}

/// Copies the frame's params into `out`. False on null input or a frame
/// without params.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_get_params(
	this: *const Frame,
	out: *mut OakVideoParams,
) -> bool {
	if this.is_null() || out.is_null() {
		return false;
	}
	match unsafe { &*this }.params() {
		Some(params) => {
			unsafe { *out = OakVideoParams::from_native(params) };
			true
		}
		None => false,
	}
}

/// Replaces the frame's params (recomputes line sizes, does NOT
/// reallocate the buffer).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_frame_set_params(this: *mut Frame, params: OakVideoParams) {
	if this.is_null() {
		return;
	}
	unsafe { &mut *this }.set_params(params.to_native());
}

#[cfg(test)]
mod tests {
	use super::*;

	fn test_params(width: i32, height: i32) -> OakVideoParams {
		let mut p = crate::coretypes::oak_core_videoparams_default();
		p.width = width;
		p.height = height;
		p.format = 4; // F32
		p.channel_count = 4;
		p
	}

	#[test]
	fn frame_lifecycle() {
		unsafe {
			let frame = oak_codec_frame_with_params(test_params(64, 32));
			assert!(!frame.is_null());
			assert!(!oak_codec_frame_is_allocated(frame));
			assert!(oak_codec_frame_data(frame).is_null());

			assert!(oak_codec_frame_allocate(frame));
			assert!(oak_codec_frame_is_allocated(frame));
			assert_eq!(oak_codec_frame_width(frame), 64);
			assert_eq!(oak_codec_frame_height(frame), 32);
			assert_eq!(oak_codec_frame_format(frame), 4);
			assert_eq!(oak_codec_frame_channel_count(frame), 4);

			let size = oak_codec_frame_allocated_size(frame);
			assert_eq!(size, 64 * 32 * 4 * 4, "w*h*channels*f32");
			assert_eq!(oak_codec_frame_linesize_bytes(frame), 64 * 4 * 4);

			// The buffer is writable through the data pointer.
			let data = oak_codec_frame_data(frame);
			assert!(!data.is_null());
			std::ptr::write_bytes(data, 0xAB, 16);

			// Timestamp roundtrip.
			oak_codec_frame_set_timestamp(frame, OakRational { num: 3, den: 2 });
			assert_eq!(
				oak_codec_frame_timestamp(frame),
				OakRational { num: 3, den: 2 }
			);

			// Params roundtrip.
			let mut out = oak_codec_frame_new();
			let mut p = crate::coretypes::oak_core_videoparams_default();
			assert!(oak_codec_frame_get_params(frame, &mut p));
			assert_eq!((p.width, p.height), (64, 32));
			oak_codec_frame_release(out.cast());

			oak_codec_frame_release(frame);
		}
	}

	#[test]
	fn frame_refcount_keeps_buffer_alive() {
		unsafe {
			let frame = oak_codec_frame_with_params(test_params(8, 8));
			assert!(oak_codec_frame_allocate(frame));
			oak_codec_frame_add_ref(frame);
			oak_codec_frame_release(frame);
			assert!(oak_codec_frame_is_allocated(frame), "one reference remains");
			oak_codec_frame_release(frame);
		}
	}

	#[test]
	fn frame_null_is_safe() {
		unsafe {
			assert!(!oak_codec_frame_allocate(std::ptr::null_mut()));
			assert!(!oak_codec_frame_is_allocated(std::ptr::null()));
			assert!(oak_codec_frame_data(std::ptr::null_mut()).is_null());
			assert_eq!(oak_codec_frame_allocated_size(std::ptr::null()), 0);
			assert_eq!(oak_codec_frame_width(std::ptr::null()), 0);
			assert_eq!(
				oak_codec_frame_timestamp(std::ptr::null()),
				OakRational { num: 0, den: 0 }
			);
			oak_codec_frame_set_timestamp(std::ptr::null_mut(), OakRational { num: 1, den: 1 });
			assert!(!oak_codec_frame_get_params(std::ptr::null(), std::ptr::null_mut()));
			oak_codec_frame_set_params(std::ptr::null_mut(), test_params(1, 1));
			oak_codec_frame_add_ref(std::ptr::null());
			oak_codec_frame_release(std::ptr::null());
		}
	}
}

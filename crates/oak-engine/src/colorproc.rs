//! C ABI for `oak_core::color::ColorProcessor` (prefix
//! `oak_core_colorproc_`): color-space conversions for plugins.
//!
//! The handle is Arc-backed (add_ref/release pair from
//! `export_handle!`). Processors are immutable once created, so a shared
//! handle is safe to use from any thread. `create`/`create_lut`/
//! `create_display_icc` return null when the default OCIO config is not
//! loaded (see `oak_core_color_set_up_default_config`).

use oak_core::color::{ColorProcessor, Direction};

use crate::codec::{copy_str_out, str_arg};
use crate::handle::into_ffi;

crate::export_handle!(
	ColorProcessor,
	oak_core_colorproc_add_ref,
	oak_core_colorproc_release
);

fn direction(inverse: bool) -> Direction {
	if inverse { Direction::Inverse } else { Direction::Normal }
}

/// A processor from two colorspace names on the default config;
/// `inverse` runs dst → src. Null on invalid input or when no default
/// config is loaded. OCIO failures yield a pass-through processor
/// (check with `is_valid`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_create(
	src_space: *const u8,
	src_space_len: usize,
	dst_transform: *const u8,
	dst_transform_len: usize,
	inverse: bool,
) -> *mut ColorProcessor {
	let (Some(src), Some(dst)) = (
		unsafe { str_arg(src_space, src_space_len) },
		unsafe { str_arg(dst_transform, dst_transform_len) },
	) else {
		return std::ptr::null_mut();
	};
	match ColorProcessor::create(src, dst, direction(inverse)) {
		Some(p) => into_ffi(p) as *mut ColorProcessor,
		None => std::ptr::null_mut(),
	}
}

/// A processor applying the LUT file at `path`; `inverse` runs it
/// backwards. Null on invalid input or when no default config is loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_create_lut(
	path: *const u8,
	path_len: usize,
	inverse: bool,
) -> *mut ColorProcessor {
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return std::ptr::null_mut();
	};
	match ColorProcessor::create_lut(path, direction(inverse)) {
		Some(p) => into_ffi(p) as *mut ColorProcessor,
		None => std::ptr::null_mut(),
	}
}

/// A processor converting `src_space` to the display ICC profile at
/// `icc_path`. Null on invalid input or when no default config is
/// loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_create_display_icc(
	src_space: *const u8,
	src_space_len: usize,
	icc_path: *const u8,
	icc_path_len: usize,
) -> *mut ColorProcessor {
	let (Some(src), Some(icc)) = (
		unsafe { str_arg(src_space, src_space_len) },
		unsafe { str_arg(icc_path, icc_path_len) },
	) else {
		return std::ptr::null_mut();
	};
	match ColorProcessor::create_display_icc(src, icc) {
		Some(p) => into_ffi(p) as *mut ColorProcessor,
		None => std::ptr::null_mut(),
	}
}

/// A pass-through processor (no config needed).
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colorproc_pass_through() -> *mut ColorProcessor {
	into_ffi(ColorProcessor::pass_through()) as *mut ColorProcessor
}

/// Whether the processor has a real OCIO transform (false for the
/// pass-through fallback produced on OCIO failures).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_is_valid(this: *const ColorProcessor) -> bool {
	!this.is_null() && unsafe { &*this }.is_valid()
}

/// The processor's cache id, snprintf-style; -1 on null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_cache_id(
	this: *const ColorProcessor,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if this.is_null() {
		return -1;
	}
	copy_str_out(&unsafe { &*this }.cache_id(), buf, buf_len)
}

/// Converts one RGBA color in place: `rgba` points to 4 f64 values read
/// before and written after the conversion. False on null input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_convert_color(
	this: *const ColorProcessor,
	rgba: *mut f64,
) -> bool {
	if this.is_null() || rgba.is_null() {
		return false;
	}
	let mut color: [f64; 4] = unsafe { std::ptr::read(rgba.cast()) };
	color = unsafe { &*this }.convert_color(color);
	unsafe { std::ptr::write(rgba.cast(), color) };
	true
}

/// Converts `pixels * 4` f32 RGBA samples in place. False on null input
/// or engine error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_convert_f32_rgba(
	this: *const ColorProcessor,
	data: *mut f32,
	pixels: i64,
) -> bool {
	if this.is_null() || (data.is_null() && pixels > 0) {
		return false;
	}
	let samples = if data.is_null() {
		&mut []
	} else {
		unsafe { std::slice::from_raw_parts_mut(data, pixels.max(0) as usize * 4) }
	};
	unsafe { &*this }.convert_f32_rgba(samples, pixels).is_ok()
}

/// Converts `pixels * 4` u8 BGRA samples in place. False on null input
/// or engine error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colorproc_convert_bgra8(
	this: *const ColorProcessor,
	data: *mut u8,
	pixels: i64,
) -> bool {
	if this.is_null() || (data.is_null() && pixels > 0) {
		return false;
	}
	let buf = if data.is_null() {
		&mut []
	} else {
		unsafe { std::slice::from_raw_parts_mut(data, pixels.max(0) as usize * 4) }
	};
	unsafe { &*this }.convert_bgra8(buf, pixels).is_ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	#[test]
	fn pass_through_is_identity() {
		unsafe {
			let proc = oak_core_colorproc_pass_through();
			assert!(!proc.is_null());

			let mut rgba = [0.1f64, 0.5, 0.9, 1.0];
			assert!(oak_core_colorproc_convert_color(proc, rgba.as_mut_ptr()));
			assert_eq!(rgba, [0.1, 0.5, 0.9, 1.0]);

			let mut px = [0.25f32, 0.5, 0.75, 1.0];
			assert!(oak_core_colorproc_convert_f32_rgba(proc, px.as_mut_ptr(), 1));
			assert_eq!(px, [0.25, 0.5, 0.75, 1.0]);

			let mut bgra = [64u8, 128, 192, 255];
			assert!(oak_core_colorproc_convert_bgra8(proc, bgra.as_mut_ptr(), 1));
			assert_eq!(bgra, [64, 128, 192, 255]);

			oak_core_colorproc_release(proc);
		}
	}

	#[test]
	fn create_with_default_config() {
		// Loads the bundled OCIO config (idempotent, headless-safe).
		assert!(crate::corestate::oak_core_color_set_up_default_config());
		unsafe {
			let (src, dst) = ("acescg", "srgb");
			let proc =
				oak_core_colorproc_create(arg(src).0, src.len(), arg(dst).0, dst.len(), false);
			assert!(!proc.is_null(), "create after config load");

			if oak_core_colorproc_is_valid(proc) {
				// A real transform changes mid gray.
				let mut rgba = [0.18f64, 0.18, 0.18, 1.0];
				assert!(oak_core_colorproc_convert_color(proc, rgba.as_mut_ptr()));
				assert_ne!(rgba[0], 0.18);
			}
			oak_core_colorproc_release(proc);
		}
	}

	#[test]
	fn null_and_invalid_input_is_safe() {
		unsafe {
			assert!(oak_core_colorproc_create(std::ptr::null(), 0, arg("x").0, 1, false).is_null());
			assert!(!oak_core_colorproc_is_valid(std::ptr::null()));
			assert_eq!(oak_core_colorproc_cache_id(std::ptr::null(), std::ptr::null_mut(), 0), -1);
			assert!(!oak_core_colorproc_convert_color(std::ptr::null(), std::ptr::null_mut()));
			assert!(!oak_core_colorproc_convert_f32_rgba(std::ptr::null(), std::ptr::null_mut(), 1));
			assert!(!oak_core_colorproc_convert_bgra8(std::ptr::null(), std::ptr::null_mut(), 1));
			oak_core_colorproc_add_ref(std::ptr::null());
			oak_core_colorproc_release(std::ptr::null());
		}
	}
}

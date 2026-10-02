//! Safe wrapper over the engine's `ColorProcessor` C ABI
//! (`oak_core_colorproc_*`): color-space conversions for plugins.

use std::ffi::c_void;

use thiserror::Error;

use crate::codec::string_out;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_core_colorproc_create(
		src_space: *const u8,
		src_space_len: usize,
		dst_transform: *const u8,
		dst_transform_len: usize,
		inverse: bool,
	) -> *mut c_void;
	fn oak_core_colorproc_create_lut(path: *const u8, path_len: usize, inverse: bool)
	-> *mut c_void;
	fn oak_core_colorproc_create_display_icc(
		src_space: *const u8,
		src_space_len: usize,
		icc_path: *const u8,
		icc_path_len: usize,
	) -> *mut c_void;
	fn oak_core_colorproc_pass_through() -> *mut c_void;
	fn oak_core_colorproc_is_valid(this: *const c_void) -> bool;
	fn oak_core_colorproc_cache_id(this: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_colorproc_convert_color(this: *const c_void, rgba: *mut f64) -> bool;
	fn oak_core_colorproc_convert_f32_rgba(this: *const c_void, data: *mut f32, pixels: i64) -> bool;
	fn oak_core_colorproc_convert_bgra8(this: *const c_void, data: *mut u8, pixels: i64) -> bool;
	fn oak_core_colorproc_add_ref(this: *const c_void);
	fn oak_core_colorproc_release(this: *const c_void);
}

/// A failed color-processor call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the engine color processor call failed")]
pub struct Error;

/// A color-space converter. Reference-counted in the engine: `Clone`
/// adds a reference, `Drop` releases. Immutable once created, so clones
/// are safe to use from any thread.
pub struct ColorProcessor {
	ptr: *mut c_void,
}

unsafe impl Send for ColorProcessor {}
unsafe impl Sync for ColorProcessor {}

impl ColorProcessor {
	fn from_raw(ptr: *mut c_void) -> Option<Self> {
		if ptr.is_null() { None } else { Some(Self { ptr }) }
	}

	/// A processor from two colorspace names on the default OCIO config
	/// (`inverse` runs dst → src). `None` when no default config is
	/// loaded (see [`crate::state::color_set_up_default_config`]). OCIO
	/// failures yield a pass-through processor — check with
	/// [`ColorProcessor::is_valid`].
	pub fn create(src_space: &str, dst_transform: &str, inverse: bool) -> Option<Self> {
		Self::from_raw(unsafe {
			oak_core_colorproc_create(
				src_space.as_ptr(),
				src_space.len(),
				dst_transform.as_ptr(),
				dst_transform.len(),
				inverse,
			)
		})
	}

	/// A processor applying the LUT file at `path` (`inverse` runs it
	/// backwards).
	pub fn create_lut(path: &str, inverse: bool) -> Option<Self> {
		Self::from_raw(unsafe { oak_core_colorproc_create_lut(path.as_ptr(), path.len(), inverse) })
	}

	/// A processor converting `src_space` to the display ICC profile at
	/// `icc_path`.
	pub fn create_display_icc(src_space: &str, icc_path: &str) -> Option<Self> {
		Self::from_raw(unsafe {
			oak_core_colorproc_create_display_icc(
				src_space.as_ptr(),
				src_space.len(),
				icc_path.as_ptr(),
				icc_path.len(),
			)
		})
	}

	/// A pass-through processor (no config needed).
	pub fn pass_through() -> Self {
		Self {
			ptr: unsafe { oak_core_colorproc_pass_through() },
		}
	}

	/// Whether the processor has a real OCIO transform (false for the
	/// pass-through fallback produced on OCIO failures).
	pub fn is_valid(&self) -> bool {
		unsafe { oak_core_colorproc_is_valid(self.ptr) }
	}

	/// The processor's cache id.
	pub fn cache_id(&self) -> Option<String> {
		string_out(|buf, len| unsafe { oak_core_colorproc_cache_id(self.ptr, buf, len) })
	}

	/// Converts one RGBA color.
	pub fn convert_color(&self, rgba: [f64; 4]) -> [f64; 4] {
		let mut out = rgba;
		unsafe { oak_core_colorproc_convert_color(self.ptr, out.as_mut_ptr()) };
		out
	}

	/// Converts `pixels * 4` f32 RGBA samples in place.
	pub fn convert_f32_rgba(&self, samples: &mut [f32], pixels: i64) -> Result<(), Error> {
		if unsafe { oak_core_colorproc_convert_f32_rgba(self.ptr, samples.as_mut_ptr(), pixels) } {
			Ok(())
		} else {
			Err(Error)
		}
	}

	/// Converts `pixels * 4` u8 BGRA samples in place.
	pub fn convert_bgra8(&self, data: &mut [u8], pixels: i64) -> Result<(), Error> {
		if unsafe { oak_core_colorproc_convert_bgra8(self.ptr, data.as_mut_ptr(), pixels) } {
			Ok(())
		} else {
			Err(Error)
		}
	}
}

impl Clone for ColorProcessor {
	fn clone(&self) -> Self {
		unsafe { oak_core_colorproc_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for ColorProcessor {
	fn drop(&mut self) {
		unsafe { oak_core_colorproc_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pass_through_is_identity() {
		let proc = ColorProcessor::pass_through();
		assert!(!proc.is_valid(), "pass-through carries no OCIO transform");

		assert_eq!(proc.convert_color([0.1, 0.5, 0.9, 1.0]), [0.1, 0.5, 0.9, 1.0]);

		let mut px = [0.25f32, 0.5, 0.75, 1.0];
		proc.convert_f32_rgba(&mut px, 1).expect("convert");
		assert_eq!(px, [0.25, 0.5, 0.75, 1.0]);

		let mut bgra = [64u8, 128, 192, 255];
		proc.convert_bgra8(&mut bgra, 1).expect("convert");
		assert_eq!(bgra, [64, 128, 192, 255]);

		// Clones share the processor.
		let clone = proc.clone();
		assert_eq!(clone.convert_color([1.0, 0.0, 0.0, 1.0]), [1.0, 0.0, 0.0, 1.0]);
	}

	#[test]
	fn create_with_default_config() {
		crate::state::color_set_up_default_config(None).expect("load config");
		let proc = ColorProcessor::create("acescg", "srgb", false).expect("create");
		if proc.is_valid() {
			let gray = proc.convert_color([0.18, 0.18, 0.18, 1.0]);
			assert_ne!(gray[0], 0.18, "a real transform changes mid gray");
			assert!(proc.cache_id().is_some_and(|id| !id.is_empty()));
		}
	}
}

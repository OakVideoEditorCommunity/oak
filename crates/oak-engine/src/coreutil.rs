//! C ABI for oak-core's pure utilities: color math, the display-LUT
//! constants, and file/path helpers (prefix `oak_core_`).
//!
//! Pixel-buffer transforms take explicit color-setting strings (the same
//! persisted values the pipeline settings use) instead of reading the
//! process-wide pipeline state, so these entry points stay pure and
//! thread-safe.

use oak_core::colormath::{self, OutputColorSpec, WorkingColorSpace};
use oak_core::colormath::{PRIMARIES_AP1, PRIMARIES_BT2020, PRIMARIES_DISPLAY_P3, PRIMARIES_SRGB};
use oak_core::filefunctions::{FileFunctions, default_disk_cache_path};
use oak_core::lut::Lut3d;

use crate::codec::{copy_str_out, str_arg};

// ---------------------------------------------------------------------------
// Color math
// ---------------------------------------------------------------------------

/// Applies the working→display transform in place over packed f32
/// samples. Settings are the persisted strings (e.g. "acescg",
/// "srgb", "srgb", ...). False on null data with a nonzero length or
/// invalid strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colormath_working_to_display_target(
	data: *mut f32,
	len: usize,
	working: *const u8,
	working_len: usize,
	gamut: *const u8,
	gamut_len: usize,
	transfer: *const u8,
	transfer_len: usize,
) -> bool {
	if data.is_null() && len > 0 {
		return false;
	}
	let (Some(working), Some(gamut), Some(transfer)) = (
		unsafe { str_arg(working, working_len) },
		unsafe { str_arg(gamut, gamut_len) },
		unsafe { str_arg(transfer, transfer_len) },
	) else {
		return false;
	};
	let samples = if data.is_null() {
		&mut []
	} else {
		unsafe { std::slice::from_raw_parts_mut(data, len) }
	};
	colormath::working_to_display_target(
		samples,
		WorkingColorSpace::from_setting(working),
		OutputColorSpec::from_settings(gamut, transfer),
	);
	true
}

/// Converts packed f32 samples from the output spec into linear XYZ
/// (D65) in place. False on null data with a nonzero length or invalid
/// strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_colormath_output_spec_to_xyz_d65(
	data: *mut f32,
	len: usize,
	gamut: *const u8,
	gamut_len: usize,
	transfer: *const u8,
	transfer_len: usize,
) -> bool {
	if data.is_null() && len > 0 {
		return false;
	}
	let (Some(gamut), Some(transfer)) = (
		unsafe { str_arg(gamut, gamut_len) },
		unsafe { str_arg(transfer, transfer_len) },
	) else {
		return false;
	};
	let samples = if data.is_null() {
		&mut []
	} else {
		unsafe { std::slice::from_raw_parts_mut(data, len) }
	};
	colormath::output_spec_to_xyz_d65(samples, OutputColorSpec::from_settings(gamut, transfer));
	true
}

macro_rules! export_transfer_fns {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("Component-wise transfer function `", stringify!($native), "`.")]
			#[unsafe(no_mangle)]
			pub extern "C" fn $name(v: f32) -> f32 {
				colormath::$native(v)
			}
		)*
	};
}

export_transfer_fns! {
	oak_core_colormath_srgb_oetf => srgb_oetf,
	oak_core_colormath_srgb_eotf => srgb_eotf,
	oak_core_colormath_pq_oetf => pq_oetf,
	oak_core_colormath_pq_eotf => pq_eotf,
	oak_core_colormath_hlg_oetf => hlg_oetf,
	oak_core_colormath_hlg_eotf => hlg_eotf,
}

/// Component-wise gamma encode/decode with an explicit gamma exponent.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colormath_gamma_oetf(v: f32, gamma: f32) -> f32 {
	colormath::gamma_oetf(v, gamma)
}

/// See [`oak_core_colormath_gamma_oetf`].
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colormath_gamma_eotf(v: f32, gamma: f32) -> f32 {
	colormath::gamma_eotf(v, gamma)
}

/// `repr(C)` mirror of `colormath::Xy` (a chromaticity coordinate).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OakXy {
	pub x: f32,
	pub y: f32,
}

/// `repr(C)` mirror of `colormath::Primaries` (RGB primaries + white
/// point).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OakPrimaries {
	pub red: OakXy,
	pub green: OakXy,
	pub blue: OakXy,
	pub white: OakXy,
}

fn xy(p: OakXy) -> colormath::Xy {
	colormath::Xy { x: p.x, y: p.y }
}

fn primaries(p: OakPrimaries) -> colormath::Primaries {
	colormath::Primaries {
		red: xy(p.red),
		green: xy(p.green),
		blue: xy(p.blue),
		white: xy(p.white),
	}
}

fn oak_primaries(p: colormath::Primaries) -> OakPrimaries {
	let cvt = |c: colormath::Xy| OakXy { x: c.x, y: c.y };
	OakPrimaries {
		red: cvt(p.red),
		green: cvt(p.green),
		blue: cvt(p.blue),
		white: cvt(p.white),
	}
}

/// The Bradford chromatic adaptation matrix between two white points.
/// Mat3 is `[[f32; 3]; 3]` — plain C layout, returned by value.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colormath_chromatic_adaptation(
	src_white: OakXy,
	dst_white: OakXy,
) -> [[f32; 3]; 3] {
	colormath::chromatic_adaptation(xy(src_white), xy(dst_white))
}

/// The RGB→XYZ matrix for a set of primaries.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colormath_rgb_to_xyz(p: OakPrimaries) -> [[f32; 3]; 3] {
	colormath::rgb_to_xyz_matrix(primaries(p))
}

/// The direct RGB→RGB conversion matrix between two primary sets.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_colormath_rgb_to_rgb(
	src: OakPrimaries,
	dst: OakPrimaries,
) -> [[f32; 3]; 3] {
	colormath::rgb_to_rgb_matrix(primaries(src), primaries(dst))
}

macro_rules! export_primaries {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("The built-in `", stringify!($native), "` primaries.")]
			#[unsafe(no_mangle)]
			pub extern "C" fn $name() -> OakPrimaries {
				oak_primaries($native)
			}
		)*
	};
}

export_primaries! {
	oak_core_colormath_primaries_srgb => PRIMARIES_SRGB,
	oak_core_colormath_primaries_display_p3 => PRIMARIES_DISPLAY_P3,
	oak_core_colormath_primaries_bt2020 => PRIMARIES_BT2020,
	oak_core_colormath_primaries_ap1 => PRIMARIES_AP1,
}

// ---------------------------------------------------------------------------
// Display LUT constants
// ---------------------------------------------------------------------------

/// The display LUT lattice edge length (65).
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_lut_display_edge() -> u32 {
	Lut3d::DISPLAY_EDGE
}

/// The display LUT input domain: writes 3 floats to `lo` and 3 to `hi`.
/// False when either pointer is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_lut_display_domain(lo: *mut f32, hi: *mut f32) -> bool {
	if lo.is_null() || hi.is_null() {
		return false;
	}
	unsafe {
		std::ptr::copy_nonoverlapping(Lut3d::DISPLAY_LO.as_ptr(), lo, 3);
		std::ptr::copy_nonoverlapping(Lut3d::DISPLAY_HI.as_ptr(), hi, 3);
	}
	true
}

// ---------------------------------------------------------------------------
// File functions
// ---------------------------------------------------------------------------

/// The default disk cache path, snprintf-style.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_file_default_disk_cache_path(buf: *mut u8, buf_len: usize) -> i32 {
	copy_str_out(&default_disk_cache_path(), buf, buf_len)
}

macro_rules! export_path_getters {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("`FileFunctions::", stringify!($native), "`, snprintf-style; -1 on error.")]
			#[unsafe(no_mangle)]
			pub extern "C" fn $name(buf: *mut u8, buf_len: usize) -> i32 {
				match FileFunctions::new().$native() {
					Ok(path) => copy_str_out(&path, buf, buf_len),
					Err(_) => -1,
				}
			}
		)*
	};
}

export_path_getters! {
	oak_core_file_get_configuration_location => get_configuration_location,
	oak_core_file_get_application_path => get_application_path,
	oak_core_file_get_temp_file_path => get_temp_file_path,
	oak_core_file_get_auto_recovery_root => get_auto_recovery_root,
}

/// The unique file identifier for `filename` (empty for missing files),
/// snprintf-style; -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_get_unique_file_identifier(
	filename: *const u8,
	filename_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	match FileFunctions::new().get_unique_file_identifier(filename) {
		Ok(id) => copy_str_out(&id, buf, buf_len),
		Err(_) => -1,
	}
}

/// Appends `extension` to `filename` when missing, snprintf-style;
/// -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_ensure_filename_extension(
	filename: *const u8,
	filename_len: usize,
	extension: *const u8,
	extension_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let (Some(filename), Some(extension)) = (
		unsafe { str_arg(filename, filename_len) },
		unsafe { str_arg(extension, extension_len) },
	) else {
		return -1;
	};
	match FileFunctions::new().ensure_filename_extension(filename, extension) {
		Ok(name) => copy_str_out(&name, buf, buf_len),
		Err(_) => -1,
	}
}

/// Reads a whole text file, snprintf-style. Unreadable/missing files
/// read as an empty string (C++ parity), so -1 means invalid input only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_read_file_as_string(
	filename: *const u8,
	filename_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	match FileFunctions::new().read_file_as_string(filename) {
		Ok(text) => copy_str_out(&text, buf, buf_len),
		Err(_) => -1,
	}
}

/// Whether `dir` is a valid directory; with `try_to_create`, attempts to
/// create it first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_directory_is_valid(
	dir: *const u8,
	dir_len: usize,
	try_to_create: bool,
) -> bool {
	let Some(dir) = (unsafe { str_arg(dir, dir_len) }) else {
		return false;
	};
	FileFunctions::new().directory_is_valid(dir, try_to_create)
}

/// A collision-free temporary filename derived from `original`,
/// snprintf-style; -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_get_safe_temporary_filename(
	original: *const u8,
	original_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(original) = (unsafe { str_arg(original, original_len) }) else {
		return -1;
	};
	match FileFunctions::new().get_safe_temporary_filename(original) {
		Ok(name) => copy_str_out(&name, buf, buf_len),
		Err(_) => -1,
	}
}

/// The platform-specific executable name for `unformatted` (e.g. adds
/// ".exe" on Windows), snprintf-style; -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_get_formatted_executable_for_platform(
	unformatted: *const u8,
	unformatted_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(unformatted) = (unsafe { str_arg(unformatted, unformatted_len) }) else {
		return -1;
	};
	match FileFunctions::new().get_formatted_executable_for_platform(unformatted) {
		Ok(name) => copy_str_out(&name, buf, buf_len),
		Err(_) => -1,
	}
}

/// Renames `from` to `to`, replacing `to` when it exists. False on
/// failure or invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_file_rename_file_allow_overwrite(
	from: *const u8,
	from_len: usize,
	to: *const u8,
	to_len: usize,
) -> bool {
	let (Some(from), Some(to)) = (
		unsafe { str_arg(from, from_len) },
		unsafe { str_arg(to, to_len) },
	) else {
		return false;
	};
	FileFunctions::new().rename_file_allow_overwrite(from, to)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn read_string(f: impl Fn(*mut u8, usize) -> i32) -> Option<String> {
		let n = f(std::ptr::null_mut(), 0);
		if n < 0 {
			return None;
		}
		let mut buf = vec![0u8; n as usize + 1];
		let written = f(buf.as_mut_ptr(), buf.len());
		assert_eq!(written, n);
		buf.truncate(n as usize);
		String::from_utf8(buf).ok()
	}

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	#[test]
	fn transfer_functions_roundtrip() {
		assert_eq!(oak_core_colormath_srgb_oetf(0.0), 0.0);
		assert!((oak_core_colormath_srgb_eotf(1.0) - 1.0).abs() < 1e-6);
		for v in [0.0f32, 0.18, 0.5, 1.0] {
			let rt = oak_core_colormath_srgb_eotf(oak_core_colormath_srgb_oetf(v));
			assert!((rt - v).abs() < 1e-5, "sRGB roundtrip failed for {v}");
			let rt = oak_core_colormath_gamma_eotf(oak_core_colormath_gamma_oetf(v, 2.2), 2.2);
			assert!((rt - v).abs() < 1e-5, "gamma roundtrip failed for {v}");
		}
		assert!((oak_core_colormath_pq_eotf(oak_core_colormath_pq_oetf(0.5)) - 0.5).abs() < 1e-5);
		assert!((oak_core_colormath_hlg_eotf(oak_core_colormath_hlg_oetf(0.5)) - 0.5).abs() < 1e-5);
	}

	#[test]
	fn display_target_transform() {
		let mut data = vec![0.18f32, 0.5, 0.9, 1.0];
		// srgb_legacy working space is a pass-through.
		unsafe {
			assert!(oak_core_colormath_working_to_display_target(
				data.as_mut_ptr(),
				data.len(),
				arg("srgb_legacy").0,
				11,
				arg("srgb").0,
				4,
				arg("srgb").0,
				4,
			));
		}
		assert_eq!(data, vec![0.18, 0.5, 0.9, 1.0]);

		// acescg transforms the values.
		let mut data = vec![0.18f32, 0.5, 0.9, 1.0];
		unsafe {
			assert!(oak_core_colormath_working_to_display_target(
				data.as_mut_ptr(),
				data.len(),
				arg("acescg").0,
				6,
				arg("srgb").0,
				4,
				arg("srgb").0,
				4,
			));
		}
		assert_ne!(data[0], 0.18);

		// Null with data is an error; null with len 0 is fine.
		unsafe {
			assert!(!oak_core_colormath_working_to_display_target(
				std::ptr::null_mut(),
				4,
				arg("acescg").0,
				6,
				arg("srgb").0,
				4,
				arg("srgb").0,
				4,
			));
			assert!(oak_core_colormath_working_to_display_target(
				std::ptr::null_mut(),
				0,
				arg("acescg").0,
				6,
				arg("srgb").0,
				4,
				arg("srgb").0,
				4,
			));
		}
	}

	#[test]
	fn display_lut_constants() {
		assert_eq!(oak_core_lut_display_edge(), 65);
		let mut lo = [0.0f32; 3];
		let mut hi = [0.0f32; 3];
		unsafe {
			assert!(oak_core_lut_display_domain(lo.as_mut_ptr(), hi.as_mut_ptr()));
			assert!(!oak_core_lut_display_domain(std::ptr::null_mut(), hi.as_mut_ptr()));
		}
		assert_eq!(lo, [-0.25, -0.25, -0.25]);
		assert_eq!(hi, [4.0, 4.0, 4.0]);
	}

	#[test]
	fn file_path_getters() {
		assert!(read_string(|b, l| oak_core_file_default_disk_cache_path(b, l)).is_some());
		assert!(read_string(|b, l| oak_core_file_get_configuration_location(b, l)).is_some());
		assert!(read_string(|b, l| oak_core_file_get_application_path(b, l)).is_some());
		assert!(read_string(|b, l| oak_core_file_get_temp_file_path(b, l)).is_some());
		assert!(read_string(|b, l| oak_core_file_get_auto_recovery_root(b, l)).is_some());
	}

	#[test]
	fn file_string_operations() {
		let mut buf = [0u8; 1024];
		unsafe {
			let n = oak_core_file_ensure_filename_extension(
				arg("clip").0,
				4,
				arg("mp4").0,
				3,
				buf.as_mut_ptr(),
				buf.len(),
			);
			assert_eq!(std::str::from_utf8(&buf[..n as usize]).unwrap(), "clip.mp4");

			// Missing file: empty identifier, not an error.
			let n = oak_core_file_get_unique_file_identifier(
				arg("no-such-file.mp4").0,
				16,
				buf.as_mut_ptr(),
				buf.len(),
			);
			assert_eq!(n, 0);

			assert!(!oak_core_file_directory_is_valid(
				arg("/nonexistent/oak-util-test").0,
				26,
				false
			));
			// Unreadable files read as empty (C++ parity), not an error.
			assert_eq!(
				oak_core_file_read_file_as_string(
					arg("/nonexistent/oak-util-test.txt").0,
					30,
					buf.as_mut_ptr(),
					buf.len(),
				),
				0
			);

			let n = oak_core_file_get_formatted_executable_for_platform(
				arg("ffmpeg").0,
				6,
				buf.as_mut_ptr(),
				buf.len(),
			);
			assert!(n >= 6);
		}
	}

	#[test]
	fn file_rename_allow_overwrite() {
		let dir = std::env::temp_dir().join(format!("oakengine_util_{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let from = dir.join("from.txt");
		let to = dir.join("to.txt");
		std::fs::write(&from, b"a").unwrap();
		std::fs::write(&to, b"b").unwrap();

		let (f, t) = (from.to_string_lossy(), to.to_string_lossy());
		unsafe {
			assert!(oak_core_file_rename_file_allow_overwrite(
				arg(&f).0,
				f.len(),
				arg(&t).0,
				t.len(),
			));
		}
		assert!(!from.exists());
		assert_eq!(std::fs::read(&to).unwrap(), b"a");
	}
}

//! Safe wrapper over the engine's utility C ABI: color math
//! (`oak_core_colormath_*`), the display-LUT constants
//! (`oak_core_lut_*`), and file/path helpers (`oak_core_file_*`).

use thiserror::Error;

use crate::codec::string_out;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_core_colormath_working_to_display_target(
		data: *mut f32,
		len: usize,
		working: *const u8,
		working_len: usize,
		gamut: *const u8,
		gamut_len: usize,
		transfer: *const u8,
		transfer_len: usize,
	) -> bool;
	fn oak_core_colormath_output_spec_to_xyz_d65(
		data: *mut f32,
		len: usize,
		gamut: *const u8,
		gamut_len: usize,
		transfer: *const u8,
		transfer_len: usize,
	) -> bool;
	fn oak_core_colormath_srgb_oetf(v: f32) -> f32;
	fn oak_core_colormath_srgb_eotf(v: f32) -> f32;
	fn oak_core_colormath_pq_oetf(v: f32) -> f32;
	fn oak_core_colormath_pq_eotf(v: f32) -> f32;
	fn oak_core_colormath_hlg_oetf(v: f32) -> f32;
	fn oak_core_colormath_hlg_eotf(v: f32) -> f32;
	fn oak_core_colormath_gamma_oetf(v: f32, gamma: f32) -> f32;
	fn oak_core_colormath_gamma_eotf(v: f32, gamma: f32) -> f32;

	fn oak_core_lut_display_edge() -> u32;
	fn oak_core_lut_display_domain(lo: *mut f32, hi: *mut f32) -> bool;

	fn oak_core_colormath_chromatic_adaptation(src_white: Xy, dst_white: Xy) -> [[f32; 3]; 3];
	fn oak_core_colormath_rgb_to_xyz(primaries: Primaries) -> [[f32; 3]; 3];
	fn oak_core_colormath_rgb_to_rgb(src: Primaries, dst: Primaries) -> [[f32; 3]; 3];
	fn oak_core_colormath_primaries_srgb() -> Primaries;
	fn oak_core_colormath_primaries_display_p3() -> Primaries;
	fn oak_core_colormath_primaries_bt2020() -> Primaries;
	fn oak_core_colormath_primaries_ap1() -> Primaries;

	fn oak_core_file_default_disk_cache_path(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_file_get_configuration_location(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_file_get_application_path(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_file_get_temp_file_path(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_file_get_auto_recovery_root(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_file_get_unique_file_identifier(
		filename: *const u8,
		filename_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_file_ensure_filename_extension(
		filename: *const u8,
		filename_len: usize,
		extension: *const u8,
		extension_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_file_read_file_as_string(
		filename: *const u8,
		filename_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_file_directory_is_valid(dir: *const u8, dir_len: usize, try_to_create: bool) -> bool;
	fn oak_core_file_get_safe_temporary_filename(
		original: *const u8,
		original_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_file_get_formatted_executable_for_platform(
		unformatted: *const u8,
		unformatted_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_file_rename_file_allow_overwrite(
		from: *const u8,
		from_len: usize,
		to: *const u8,
		to_len: usize,
	) -> bool;
}

/// A failed engine utility call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the engine utility call failed")]
pub struct Error;

fn arg(s: &str) -> (*const u8, usize) {
	(s.as_ptr(), s.len())
}

// ---------------------------------------------------------------------------
// Color math
// ---------------------------------------------------------------------------

/// Applies the working→display transform in place over packed f32
/// samples. Settings are the persisted strings (see
/// [`crate::state::color_set_pipeline_settings`]). Infallible for valid
/// settings from Rust (strings are always UTF-8).
pub fn working_to_display_target(
	samples: &mut [f32],
	working: &str,
	gamut: &str,
	transfer: &str,
) {
	let (w, wl) = arg(working);
	let (g, gl) = arg(gamut);
	let (t, tl) = arg(transfer);
	unsafe {
		oak_core_colormath_working_to_display_target(
			samples.as_mut_ptr(),
			samples.len(),
			w,
			wl,
			g,
			gl,
			t,
			tl,
		)
	};
}

/// Converts packed f32 samples from the output spec into linear XYZ
/// (D65) in place.
pub fn output_spec_to_xyz_d65(samples: &mut [f32], gamut: &str, transfer: &str) {
	let (g, gl) = arg(gamut);
	let (t, tl) = arg(transfer);
	unsafe {
		oak_core_colormath_output_spec_to_xyz_d65(
			samples.as_mut_ptr(),
			samples.len(),
			g,
			gl,
			t,
			tl,
		)
	};
}

macro_rules! transfer_fns {
	($( $name:ident => $ffi:ident ),* $(,)?) => {
		$(
			#[doc = concat!("Component-wise `", stringify!($ffi), "` transfer function.")]
			pub fn $name(v: f32) -> f32 {
				unsafe { $ffi(v) }
			}
		)*
	};
}

transfer_fns! {
	srgb_oetf => oak_core_colormath_srgb_oetf,
	srgb_eotf => oak_core_colormath_srgb_eotf,
	pq_oetf => oak_core_colormath_pq_oetf,
	pq_eotf => oak_core_colormath_pq_eotf,
	hlg_oetf => oak_core_colormath_hlg_oetf,
	hlg_eotf => oak_core_colormath_hlg_eotf,
}

/// Component-wise gamma encode with an explicit exponent.
pub fn gamma_oetf(v: f32, gamma: f32) -> f32 {
	unsafe { oak_core_colormath_gamma_oetf(v, gamma) }
}

/// Component-wise gamma decode with an explicit exponent.
pub fn gamma_eotf(v: f32, gamma: f32) -> f32 {
	unsafe { oak_core_colormath_gamma_eotf(v, gamma) }
}

// ---------------------------------------------------------------------------
// Display LUT constants
// ---------------------------------------------------------------------------

/// The display LUT lattice edge length (65).
pub fn lut_display_edge() -> u32 {
	unsafe { oak_core_lut_display_edge() }
}

/// The display LUT input domain: (lo, hi), three channels each.
pub fn lut_display_domain() -> ([f32; 3], [f32; 3]) {
	let mut lo = [0.0; 3];
	let mut hi = [0.0; 3];
	unsafe { oak_core_lut_display_domain(lo.as_mut_ptr(), hi.as_mut_ptr()) };
	(lo, hi)
}

// ---------------------------------------------------------------------------
// Color matrices
// ---------------------------------------------------------------------------

/// A chromaticity coordinate (white point / primary).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xy {
	pub x: f32,
	pub y: f32,
}

/// A set of RGB primaries plus the white point.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Primaries {
	pub red: Xy,
	pub green: Xy,
	pub blue: Xy,
	pub white: Xy,
}

/// A 3x3 color matrix (`[[f32; 3]; 3]`, plain C layout).
pub type Mat3 = [[f32; 3]; 3];

/// The Bradford chromatic adaptation matrix between two white points.
pub fn chromatic_adaptation(src_white: Xy, dst_white: Xy) -> Mat3 {
	unsafe { oak_core_colormath_chromatic_adaptation(src_white, dst_white) }
}

/// The RGB→XYZ matrix for a set of primaries.
pub fn rgb_to_xyz_matrix(primaries: Primaries) -> Mat3 {
	unsafe { oak_core_colormath_rgb_to_xyz(primaries) }
}

/// The direct RGB→RGB conversion matrix between two primary sets.
pub fn rgb_to_rgb_matrix(src: Primaries, dst: Primaries) -> Mat3 {
	unsafe { oak_core_colormath_rgb_to_rgb(src, dst) }
}

/// The built-in sRGB (Rec.709, D65) primaries.
pub fn primaries_srgb() -> Primaries {
	unsafe { oak_core_colormath_primaries_srgb() }
}

/// The built-in Display P3 (D65) primaries.
pub fn primaries_display_p3() -> Primaries {
	unsafe { oak_core_colormath_primaries_display_p3() }
}

/// The built-in Rec.2020 (D65) primaries.
pub fn primaries_bt2020() -> Primaries {
	unsafe { oak_core_colormath_primaries_bt2020() }
}

/// The built-in ACES AP1 (D60) primaries.
pub fn primaries_ap1() -> Primaries {
	unsafe { oak_core_colormath_primaries_ap1() }
}

// ---------------------------------------------------------------------------
// File functions
// ---------------------------------------------------------------------------

/// The default disk cache path.
pub fn default_disk_cache_path() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_file_default_disk_cache_path(buf, len) })
}

/// The per-user configuration directory.
pub fn configuration_location() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_file_get_configuration_location(buf, len) })
}

/// The application install path.
pub fn application_path() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_file_get_application_path(buf, len) })
}

/// A temporary file path.
pub fn temp_file_path() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_file_get_temp_file_path(buf, len) })
}

/// The auto-recovery root directory.
pub fn auto_recovery_root() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_file_get_auto_recovery_root(buf, len) })
}

/// The unique file identifier for `filename` (empty for missing files);
/// `None` on engine error.
pub fn unique_file_identifier(filename: &str) -> Option<String> {
	let (f, fl) = arg(filename);
	string_out(|buf, len| unsafe {
		oak_core_file_get_unique_file_identifier(f, fl, buf, len)
	})
}

/// Appends `extension` to `filename` when missing; `None` on engine
/// error.
pub fn ensure_filename_extension(filename: &str, extension: &str) -> Option<String> {
	let (f, fl) = arg(filename);
	let (e, el) = arg(extension);
	string_out(|buf, len| unsafe {
		oak_core_file_ensure_filename_extension(f, fl, e, el, buf, len)
	})
}

/// Reads a whole text file. Unreadable/missing files read as an empty
/// string (C++ parity); `None` only when the engine rejects the call.
pub fn read_file_as_string(filename: &str) -> Option<String> {
	let (f, fl) = arg(filename);
	string_out(|buf, len| unsafe { oak_core_file_read_file_as_string(f, fl, buf, len) })
}

/// Whether `dir` is a valid directory; with `try_to_create`, attempts to
/// create it first.
pub fn directory_is_valid(dir: &str, try_to_create: bool) -> bool {
	let (d, dl) = arg(dir);
	unsafe { oak_core_file_directory_is_valid(d, dl, try_to_create) }
}

/// A collision-free temporary filename derived from `original`.
pub fn safe_temporary_filename(original: &str) -> Option<String> {
	let (o, ol) = arg(original);
	string_out(|buf, len| unsafe {
		oak_core_file_get_safe_temporary_filename(o, ol, buf, len)
	})
}

/// The platform-specific executable name for `unformatted` (adds ".exe"
/// on Windows).
pub fn formatted_executable_for_platform(unformatted: &str) -> Option<String> {
	let (u, ul) = arg(unformatted);
	string_out(|buf, len| unsafe {
		oak_core_file_get_formatted_executable_for_platform(u, ul, buf, len)
	})
}

/// Renames `from` to `to`, replacing `to` when it exists.
pub fn rename_file_allow_overwrite(from: &str, to: &str) -> Result<(), Error> {
	let (f, fl) = arg(from);
	let (t, tl) = arg(to);
	if unsafe { oak_core_file_rename_file_allow_overwrite(f, fl, t, tl) } {
		Ok(())
	} else {
		Err(Error)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn transfer_roundtrips() {
		for v in [0.0f32, 0.18, 0.5, 1.0] {
			assert!((srgb_eotf(srgb_oetf(v)) - v).abs() < 1e-5);
			assert!((gamma_eotf(gamma_oetf(v, 2.2), 2.2) - v).abs() < 1e-5);
			assert!((pq_eotf(pq_oetf(v)) - v).abs() < 1e-5);
			assert!((hlg_eotf(hlg_oetf(v)) - v).abs() < 1e-5);
		}
	}

	#[test]
	fn display_target_transform() {
		let mut data = vec![0.18f32, 0.5, 0.9, 1.0];
		working_to_display_target(&mut data, "srgb_legacy", "srgb", "srgb");
		assert_eq!(data, vec![0.18, 0.5, 0.9, 1.0], "srgb_legacy passes through");

		let mut data = vec![0.18f32, 0.5, 0.9, 1.0];
		working_to_display_target(&mut data, "acescg", "srgb", "srgb");
		assert_ne!(data[0], 0.18, "acescg transforms");
	}

	#[test]
	fn color_matrices() {
		let srgb = primaries_srgb();
		assert!((srgb.white.x - 0.3127).abs() < 1e-3, "sRGB D65 white");

		// Identity transform: same primaries in and out.
		let id = rgb_to_rgb_matrix(srgb, srgb);
		for (i, row) in id.iter().enumerate() {
			for (j, &v) in row.iter().enumerate() {
				let expected = if i == j { 1.0 } else { 0.0 };
				assert!((v - expected).abs() < 1e-4, "identity at [{i}][{j}]: {v}");
			}
		}

		// sRGB -> Display P3 is a real (non-identity) matrix.
		let m = rgb_to_rgb_matrix(srgb, primaries_display_p3());
		assert!((m[0][0] - 1.0).abs() > 1e-3);

		let _ = rgb_to_xyz_matrix(primaries_bt2020());
		let _ = chromatic_adaptation(srgb.white, primaries_ap1().white);
	}

	#[test]
	fn lut_constants() {
		assert_eq!(lut_display_edge(), 65);
		let (lo, hi) = lut_display_domain();
		assert_eq!(lo, [-0.25; 3]);
		assert_eq!(hi, [4.0; 3]);
	}

	#[test]
	fn file_helpers() {
		assert!(default_disk_cache_path().is_some_and(|p| !p.is_empty()));
		assert!(configuration_location().is_some());
		assert_eq!(
			ensure_filename_extension("clip", "mp4").as_deref(),
			Some("clip.mp4")
		);
		assert_eq!(unique_file_identifier("no-such-file.mp4"), Some(String::new()));
		// Unreadable files read as empty (C++ parity), not as an error.
	assert_eq!(read_file_as_string("/nonexistent/oak-sdk-util.txt"), Some(String::new()));
		assert!(!directory_is_valid("/nonexistent/oak-sdk-util", false));
		assert!(formatted_executable_for_platform("ffmpeg").is_some());
	}

	#[test]
	fn rename_replaces_existing() {
		let dir = std::env::temp_dir().join(format!("oaksdk_util_{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let from = dir.join("from.txt");
		let to = dir.join("to.txt");
		std::fs::write(&from, b"a").unwrap();
		std::fs::write(&to, b"b").unwrap();
		rename_file_allow_overwrite(from.to_str().unwrap(), to.to_str().unwrap()).expect("rename");
		assert!(!from.exists());
		assert_eq!(std::fs::read(&to).unwrap(), b"a");
	}
}

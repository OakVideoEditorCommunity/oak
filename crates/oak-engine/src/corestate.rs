//! C ABI for oak-core's process-wide singletons and global state:
//! the config store, the color pipeline settings, and display ICC
//! queries (prefix `oak_core_`).
//!
//! Strings cross as borrowed `(ptr, len)` inputs and snprintf-style
//! output buffers (see `codec::copy_str_out`); nullable group/path
//! pointers mean "no group" / "default". `GpuContext` is deliberately
//! NOT exported: it bridges wgpu objects, which C consumers cannot
//! construct — it needs a separate engine-owns-the-device design.

use oak_core::color;
use oak_core::colormath::{OutputColorSpec, WorkingColorSpace};
use oak_core::configstore::ConfigStore;
use oak_core::displayicc;

use crate::codec::{copy_str_out, str_arg};

// ---------------------------------------------------------------------------
// Config store
// ---------------------------------------------------------------------------

/// Parses the nullable group argument: null pointer → `None` (no group),
/// invalid UTF-8 → error (the whole call fails).
unsafe fn group_arg<'a>(ptr: *const u8, len: usize) -> Option<Option<&'a str>> {
	if ptr.is_null() {
		return Some(None);
	}
	unsafe { str_arg(ptr, len) }.map(Some)
}

/// Sets a string config value; false on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_set(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	value: *const u8,
	value_len: usize,
) -> bool {
	let (Some(group), Some(key), Some(value)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
		unsafe { str_arg(value, value_len) },
	) else {
		return false;
	};
	ConfigStore::instance().set(group, key, value);
	true
}

/// Reads a string config value, snprintf-style; -1 when the key does
/// not exist (or the input is invalid).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_get(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return -1;
	};
	match ConfigStore::instance().get(group, key) {
		Ok(value) => copy_str_out(&value, buf, buf_len),
		Err(_) => -1,
	}
}

/// Reads a boolean config value (0/1), `fallback` when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_get_bool(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	fallback: i32,
) -> i32 {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return fallback;
	};
	ConfigStore::instance().get_bool(group, key, fallback)
}

/// Writes a boolean config value (0/1); false on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_set_bool(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	value: i32,
) -> bool {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return false;
	};
	ConfigStore::instance().set_bool(group, key, value);
	true
}

/// Reads an i64 config value, `fallback` when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_get_int64(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	fallback: i64,
) -> i64 {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return fallback;
	};
	ConfigStore::instance().get_int64(group, key, fallback)
}

/// Writes an i64 config value; false on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_set_int64(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	value: i64,
) -> bool {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return false;
	};
	ConfigStore::instance().set_int64(group, key, value);
	true
}

/// Reads an f64 config value, `fallback` when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_get_double(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	fallback: f64,
) -> f64 {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return fallback;
	};
	ConfigStore::instance().get_double(group, key, fallback)
}

/// Writes an f64 config value; false on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_config_set_double(
	group: *const u8,
	group_len: usize,
	key: *const u8,
	key_len: usize,
	value: f64,
) -> bool {
	let (Some(group), Some(key)) = (
		unsafe { group_arg(group, group_len) },
		unsafe { str_arg(key, key_len) },
	) else {
		return false;
	};
	ConfigStore::instance().set_double(group, key, value);
	true
}

/// Loads the config store from disk; false on IO error.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_config_load() -> bool {
	ConfigStore::instance().load().is_ok()
}

/// Saves the config store to disk; false on IO error.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_config_save() -> bool {
	ConfigStore::instance().save().is_ok()
}

// ---------------------------------------------------------------------------
// Color pipeline (process-wide)
// ---------------------------------------------------------------------------

/// Sets the pipeline color settings from their persisted setting
/// strings ("acescg"/"srgb_legacy", "srgb"/"displayp3"/"bt2020",
/// transfer names); false on invalid UTF-8 input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_color_set_pipeline_settings(
	working: *const u8,
	working_len: usize,
	gamut: *const u8,
	gamut_len: usize,
	transfer: *const u8,
	transfer_len: usize,
) -> bool {
	let (Some(working), Some(gamut), Some(transfer)) = (
		unsafe { str_arg(working, working_len) },
		unsafe { str_arg(gamut, gamut_len) },
		unsafe { str_arg(transfer, transfer_len) },
	) else {
		return false;
	};
	color::set_pipeline_color_settings(
		WorkingColorSpace::from_setting(working),
		OutputColorSpec::from_settings(gamut, transfer),
	);
	true
}

/// The working color space's setting string, snprintf-style.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_color_get_working_space(buf: *mut u8, buf_len: usize) -> i32 {
	copy_str_out(color::pipeline_working_space().as_setting(), buf, buf_len)
}

/// The output gamut's setting string, snprintf-style.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_color_get_output_gamut(buf: *mut u8, buf_len: usize) -> i32 {
	copy_str_out(color::pipeline_output_spec().gamut.as_setting(), buf, buf_len)
}

/// The output transfer's setting string, snprintf-style.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_color_get_output_transfer(buf: *mut u8, buf_len: usize) -> i32 {
	copy_str_out(color::pipeline_output_spec().transfer.as_setting(), buf, buf_len)
}

/// Loads the default OCIO config (`$OCIO` when set, else the bundled
/// one); false on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_color_set_up_default_config() -> bool {
	color::set_up_default_config().is_ok()
}

/// Loads the default OCIO config from an explicit path (null = the
/// bundled default); false on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_color_set_up_default_config_from(
	path: *const u8,
	path_len: usize,
) -> bool {
	let path = if path.is_null() {
		None
	} else {
		match unsafe { str_arg(path, path_len) } {
			Some(p) => Some(p),
			None => return false,
		}
	};
	color::set_up_default_config_from(path).is_ok()
}

/// The active OCIO config path, snprintf-style; -1 when none is loaded.
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_color_config_path(buf: *mut u8, buf_len: usize) -> i32 {
	match color::config_path() {
		Some(path) => copy_str_out(&path, buf, buf_len),
		None => -1,
	}
}

// ---------------------------------------------------------------------------
// Display ICC (system queries)
// ---------------------------------------------------------------------------

/// Whether Windows ACM (Auto Color Management) is active.
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_displayicc_windows_acm_active() -> bool {
	displayicc::windows_acm_active()
}

/// The system display ICC profile path, snprintf-style; -1 when
/// unavailable (headless, unsupported platform).
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_displayicc_system_display_icc(buf: *mut u8, buf_len: usize) -> i32 {
	match displayicc::system_display_icc() {
		Some(path) => copy_str_out(&path, buf, buf_len),
		None => -1,
	}
}

/// The display ICC profile path for the monitor identified by
/// `fingerprint`, snprintf-style; -1 when unknown/unavailable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_displayicc_system_display_icc_for_monitor(
	fingerprint: *const u8,
	fingerprint_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(fingerprint) = (unsafe { str_arg(fingerprint, fingerprint_len) }) else {
		return -1;
	};
	let path = displayicc::monitor_ref_from_fingerprint(fingerprint)
		.and_then(|monitor| displayicc::system_display_icc_for(&monitor));
	match path {
		Some(path) => copy_str_out(&path, buf, buf_len),
		None => -1,
	}
}

/// The monitor fingerprint for a Windows HMONITOR (cast to u64),
/// snprintf-style; -1 when unavailable.
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_displayicc_windows_monitor_fingerprint(
	hmonitor: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	match displayicc::windows_monitor_fingerprint(hmonitor) {
		Some(fp) => copy_str_out(&fp, buf, buf_len),
		None => -1,
	}
}

/// The fingerprint of the X11 monitor covering (`x`, `y`) (global
/// coordinates), snprintf-style; -1 when unavailable.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_displayicc_x11_monitor_fingerprint_at(
	x: f64,
	y: f64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	match displayicc::x11_monitor_fingerprint_at(x, y) {
		Some(fp) => copy_str_out(&fp, buf, buf_len),
		None => -1,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cstr_arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	unsafe fn read_out(buf: &[u8], n: i32) -> &str {
		std::str::from_utf8(&buf[..n as usize]).unwrap()
	}

	/// Reads one snprintf-style setting string into an owned String.
	fn read_setting(f: impl Fn(*mut u8, usize) -> i32) -> String {
		let mut buf = [0u8; 64];
		let n = f(buf.as_mut_ptr(), buf.len());
		assert!(n >= 0);
		std::str::from_utf8(&buf[..n as usize]).unwrap().to_string()
	}

	#[test]
	fn config_string_and_typed_roundtrip() {
		let unique = format!("OakEngineTest/{}", std::process::id());
		let (g, gl) = cstr_arg(&unique);

		unsafe {
			assert!(oak_core_config_set(g, gl, cstr_arg("greeting").0, 8, cstr_arg("hello").0, 5));
			let mut buf = [0u8; 32];
			let n = oak_core_config_get(g, gl, cstr_arg("greeting").0, 8, buf.as_mut_ptr(), buf.len());
			assert_eq!(read_out(&buf, n), "hello");

			assert!(oak_core_config_set_bool(g, gl, cstr_arg("flag").0, 4, 1));
			assert_eq!(oak_core_config_get_bool(g, gl, cstr_arg("flag").0, 4, 0), 1);
			// Unset key: fallback comes back.
			assert_eq!(oak_core_config_get_bool(g, gl, cstr_arg("unset").0, 5, 7), 7);

			assert!(oak_core_config_set_int64(g, gl, cstr_arg("count").0, 5, -42));
			assert_eq!(oak_core_config_get_int64(g, gl, cstr_arg("count").0, 5, 0), -42);

			assert!(oak_core_config_set_double(g, gl, cstr_arg("ratio").0, 5, 2.5));
			assert_eq!(oak_core_config_get_double(g, gl, cstr_arg("ratio").0, 5, 0.0), 2.5);

			// Missing key reads as -1.
			assert_eq!(
				oak_core_config_get(g, gl, cstr_arg("missing").0, 7, buf.as_mut_ptr(), buf.len()),
				-1
			);
			// Null group = global group.
			assert!(oak_core_config_set(
				std::ptr::null(),
				0,
				cstr_arg("nogroup").0,
				7,
				cstr_arg("v").0,
				1
			));
			assert!(oak_core_config_get(
				std::ptr::null(),
				0,
				cstr_arg("nogroup").0,
				7,
				buf.as_mut_ptr(),
				buf.len()
			) >= 0);
		}
	}

	#[test]
	fn color_settings_roundtrip_and_restore() {
		let saved_working = read_setting(|b, l| oak_core_color_get_working_space(b, l));
		let saved_gamut = read_setting(|b, l| oak_core_color_get_output_gamut(b, l));
		let saved_transfer = read_setting(|b, l| oak_core_color_get_output_transfer(b, l));

		unsafe {
			assert!(oak_core_color_set_pipeline_settings(
				cstr_arg("srgb_legacy").0,
				11,
				cstr_arg("displayp3").0,
				9,
				cstr_arg("srgb").0,
				4,
			));
		}
		assert_eq!(read_setting(|b, l| oak_core_color_get_working_space(b, l)), "srgb_legacy");
		assert_eq!(read_setting(|b, l| oak_core_color_get_output_gamut(b, l)), "displayp3");

		// Restore so later tests in this process see the defaults.
		unsafe {
			assert!(oak_core_color_set_pipeline_settings(
				saved_working.as_ptr(),
				saved_working.len(),
				saved_gamut.as_ptr(),
				saved_gamut.len(),
				saved_transfer.as_ptr(),
				saved_transfer.len(),
			));
		}
	}

	#[test]
	fn color_default_config_loads() {
		// The bundled OCIO config must load headless.
		assert!(oak_core_color_set_up_default_config());
		assert!(unsafe {
			oak_core_color_set_up_default_config_from(std::ptr::null(), 0)
		});
		// config_path may legitimately be None for a builtin config.
		let mut buf = [0u8; 1024];
		let _ = oak_core_color_config_path(buf.as_mut_ptr(), buf.len());
	}

	#[test]
	fn displayicc_queries_never_crash() {
		let mut buf = [0u8; 1024];
		// Headless CI may have no display: -1 is a valid answer.
		let _ = oak_core_displayicc_system_display_icc(buf.as_mut_ptr(), buf.len());
		#[cfg(target_os = "windows")]
		{
			let _ = oak_core_displayicc_windows_acm_active();
			let _ = oak_core_displayicc_windows_monitor_fingerprint(0, buf.as_mut_ptr(), buf.len());
		}
		#[cfg(target_os = "linux")]
		let _ = oak_core_displayicc_x11_monitor_fingerprint_at(0.0, 0.0, buf.as_mut_ptr(), buf.len());
		assert_eq!(
			unsafe {
				oak_core_displayicc_system_display_icc_for_monitor(
					cstr_arg("no-such-monitor").0,
					15,
					buf.as_mut_ptr(),
					buf.len(),
				)
			},
			-1
		);
	}
}

// ---------------------------------------------------------------------------
// CancelAtom (cooperative cancellation token)
// ---------------------------------------------------------------------------

use oak_core::cancelatom::CancelAtom;

use crate::handle::into_ffi;

crate::export_handle!(
	CancelAtom,
	oak_core_cancelatom_add_ref,
	oak_core_cancelatom_release
);

/// A not-cancelled cancellation token (ref count 1).
#[unsafe(no_mangle)]
pub extern "C" fn oak_core_cancelatom_new() -> *mut CancelAtom {
	into_ffi(CancelAtom::new()) as *mut CancelAtom
}

/// Sets the cancel flag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_cancelatom_cancel(this: *const CancelAtom) {
	if this.is_null() {
		return;
	}
	unsafe { &*this }.cancel();
}

/// Reads the cancel flag (reading a set flag records the cancellation
/// as heard).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_cancelatom_is_cancelled(this: *const CancelAtom) -> bool {
	!this.is_null() && unsafe { &*this }.is_cancelled()
}

/// Whether any consumer has observed the cancel flag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_core_cancelatom_heard_cancel(this: *const CancelAtom) -> bool {
	!this.is_null() && unsafe { &*this }.heard_cancel()
}

#[cfg(test)]
mod cancel_tests {
	#[test]
	fn cancel_lifecycle() {
		unsafe {
			let atom = super::oak_core_cancelatom_new();
			assert!(!super::oak_core_cancelatom_is_cancelled(atom));
			assert!(!super::oak_core_cancelatom_heard_cancel(atom));
			super::oak_core_cancelatom_cancel(atom);
			assert!(super::oak_core_cancelatom_is_cancelled(atom));
			assert!(super::oak_core_cancelatom_heard_cancel(atom));

			// Null is a no-op everywhere.
			super::oak_core_cancelatom_cancel(std::ptr::null());
			assert!(!super::oak_core_cancelatom_is_cancelled(std::ptr::null()));
			assert!(!super::oak_core_cancelatom_heard_cancel(std::ptr::null()));

			super::oak_core_cancelatom_add_ref(atom);
			super::oak_core_cancelatom_release(atom);
			super::oak_core_cancelatom_release(atom);
		}
	}
}

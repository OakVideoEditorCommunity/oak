//! Safe wrapper over the engine's process-wide state C ABI: the config
//! store (`oak_core_config_*`), the color pipeline settings
//! (`oak_core_color_*`), and display ICC queries (`oak_core_displayicc_*`).
//!
//! These are singletons inside the engine, so — like the audio manager —
//! the SDK exposes plain free functions.

use thiserror::Error;

use crate::codec::string_out;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_core_config_set(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		value: *const u8,
		value_len: usize,
	) -> bool;
	fn oak_core_config_get(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_core_config_get_bool(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		fallback: i32,
	) -> i32;
	fn oak_core_config_set_bool(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		value: i32,
	) -> bool;
	fn oak_core_config_get_int64(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		fallback: i64,
	) -> i64;
	fn oak_core_config_set_int64(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		value: i64,
	) -> bool;
	fn oak_core_config_get_double(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		fallback: f64,
	) -> f64;
	fn oak_core_config_set_double(
		group: *const u8,
		group_len: usize,
		key: *const u8,
		key_len: usize,
		value: f64,
	) -> bool;
	fn oak_core_config_load() -> bool;
	fn oak_core_config_save() -> bool;

	fn oak_core_color_set_pipeline_settings(
		working: *const u8,
		working_len: usize,
		gamut: *const u8,
		gamut_len: usize,
		transfer: *const u8,
		transfer_len: usize,
	) -> bool;
	fn oak_core_color_get_working_space(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_color_get_output_gamut(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_color_get_output_transfer(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_color_set_up_default_config() -> bool;
	fn oak_core_color_set_up_default_config_from(path: *const u8, path_len: usize) -> bool;
	fn oak_core_color_config_path(buf: *mut u8, buf_len: usize) -> i32;

	fn oak_core_displayicc_system_display_icc(buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_displayicc_system_display_icc_for_monitor(
		fingerprint: *const u8,
		fingerprint_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	#[cfg(target_os = "windows")]
	fn oak_core_displayicc_windows_acm_active() -> bool;
	#[cfg(target_os = "windows")]
	fn oak_core_displayicc_windows_monitor_fingerprint(
		hmonitor: u64,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	#[cfg(target_os = "linux")]
	fn oak_core_displayicc_x11_monitor_fingerprint_at(x: f64, y: f64, buf: *mut u8, buf_len: usize) -> i32;
}

/// A failed engine state call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the engine state call failed")]
pub struct Error;

fn arg(s: &str) -> (*const u8, usize) {
	(s.as_ptr(), s.len())
}

fn opt_arg(s: Option<&str>) -> (*const u8, usize) {
	match s {
		Some(s) => (s.as_ptr(), s.len()),
		None => (std::ptr::null(), 0),
	}
}

// ---------------------------------------------------------------------------
// Config store
// ---------------------------------------------------------------------------

/// Sets a string config value (`group` = None for the global group).
pub fn config_set(group: Option<&str>, key: &str, value: &str) -> Result<(), Error> {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	let (v, vl) = arg(value);
	if unsafe { oak_core_config_set(g, gl, k, kl, v, vl) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Reads a string config value; `None` when the key does not exist.
pub fn config_get(group: Option<&str>, key: &str) -> Option<String> {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	string_out(|buf, len| unsafe { oak_core_config_get(g, gl, k, kl, buf, len) })
}

/// Reads a boolean config value, `fallback` when unset (0/1 over the
/// ABI, like the C++ store).
pub fn config_get_bool(group: Option<&str>, key: &str, fallback: bool) -> bool {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	unsafe { oak_core_config_get_bool(g, gl, k, kl, fallback as i32) != 0 }
}

pub fn config_set_bool(group: Option<&str>, key: &str, value: bool) -> Result<(), Error> {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	if unsafe { oak_core_config_set_bool(g, gl, k, kl, value as i32) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Reads an i64 config value, `fallback` when unset.
pub fn config_get_i64(group: Option<&str>, key: &str, fallback: i64) -> i64 {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	unsafe { oak_core_config_get_int64(g, gl, k, kl, fallback) }
}

pub fn config_set_i64(group: Option<&str>, key: &str, value: i64) -> Result<(), Error> {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	if unsafe { oak_core_config_set_int64(g, gl, k, kl, value) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Reads an f64 config value, `fallback` when unset.
pub fn config_get_f64(group: Option<&str>, key: &str, fallback: f64) -> f64 {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	unsafe { oak_core_config_get_double(g, gl, k, kl, fallback) }
}

pub fn config_set_f64(group: Option<&str>, key: &str, value: f64) -> Result<(), Error> {
	let (g, gl) = opt_arg(group);
	let (k, kl) = arg(key);
	if unsafe { oak_core_config_set_double(g, gl, k, kl, value) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Loads the config store from disk.
pub fn config_load() -> Result<(), Error> {
	if unsafe { oak_core_config_load() } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Saves the config store to disk.
pub fn config_save() -> Result<(), Error> {
	if unsafe { oak_core_config_save() } {
		Ok(())
	} else {
		Err(Error)
	}
}

// ---------------------------------------------------------------------------
// Color pipeline (process-wide)
// ---------------------------------------------------------------------------

/// Sets the pipeline color settings from their persisted setting
/// strings ("acescg"/"srgb_legacy", "srgb"/"displayp3"/"bt2020", and the
/// transfer names).
pub fn color_set_pipeline_settings(
	working: &str,
	gamut: &str,
	transfer: &str,
) -> Result<(), Error> {
	let (w, wl) = arg(working);
	let (g, gl) = arg(gamut);
	let (t, tl) = arg(transfer);
	if unsafe { oak_core_color_set_pipeline_settings(w, wl, g, gl, t, tl) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// The working color space's setting string.
pub fn color_working_space() -> String {
	string_out(|buf, len| unsafe { oak_core_color_get_working_space(buf, len) }).unwrap_or_default()
}

/// The output gamut's setting string.
pub fn color_output_gamut() -> String {
	string_out(|buf, len| unsafe { oak_core_color_get_output_gamut(buf, len) }).unwrap_or_default()
}

/// The output transfer's setting string.
pub fn color_output_transfer() -> String {
	string_out(|buf, len| unsafe { oak_core_color_get_output_transfer(buf, len) })
		.unwrap_or_default()
}

/// Loads the default OCIO config: `Some(path)` for an explicit file,
/// `None` for `$OCIO`/the bundled default.
pub fn color_set_up_default_config(path: Option<&str>) -> Result<(), Error> {
	let ok = match path {
		Some(path) => {
			let (p, pl) = arg(path);
			unsafe { oak_core_color_set_up_default_config_from(p, pl) }
		}
		None => unsafe { oak_core_color_set_up_default_config() },
	};
	if ok { Ok(()) } else { Err(Error) }
}

/// The active OCIO config path; `None` when none is loaded (or the
/// bundled config is in use).
pub fn color_config_path() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_color_config_path(buf, len) })
}

// ---------------------------------------------------------------------------
// Display ICC (system queries)
// ---------------------------------------------------------------------------

/// The system display ICC profile path; `None` when unavailable
/// (headless, unsupported platform).
pub fn displayicc_system_display_icc() -> Option<String> {
	string_out(|buf, len| unsafe { oak_core_displayicc_system_display_icc(buf, len) })
}

/// The display ICC profile path for the monitor identified by
/// `fingerprint`; `None` when unknown/unavailable.
pub fn displayicc_system_display_icc_for(fingerprint: &str) -> Option<String> {
	let (f, fl) = arg(fingerprint);
	string_out(|buf, len| unsafe {
		oak_core_displayicc_system_display_icc_for_monitor(f, fl, buf, len)
	})
}

/// Whether Windows ACM (Auto Color Management) is active.
#[cfg(target_os = "windows")]
pub fn displayicc_windows_acm_active() -> bool {
	unsafe { oak_core_displayicc_windows_acm_active() }
}

/// The monitor fingerprint for a Windows HMONITOR (cast to u64).
#[cfg(target_os = "windows")]
pub fn displayicc_windows_monitor_fingerprint(hmonitor: u64) -> Option<String> {
	string_out(|buf, len| unsafe {
		oak_core_displayicc_windows_monitor_fingerprint(hmonitor, buf, len)
	})
}

/// The fingerprint of the X11 monitor covering (`x`, `y`) in global
/// coordinates.
#[cfg(target_os = "linux")]
pub fn displayicc_x11_monitor_fingerprint_at(x: f64, y: f64) -> Option<String> {
	string_out(|buf, len| unsafe {
		oak_core_displayicc_x11_monitor_fingerprint_at(x, y, buf, len)
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn config_roundtrip_and_fallbacks() {
		let group = Some("OakSdkTest");
		let key = "greeting";
		config_set(group, key, "hello").expect("set");
		assert_eq!(config_get(group, key).as_deref(), Some("hello"));
		assert_eq!(config_get(group, "missing"), None);

		config_set_bool(group, "flag", true).expect("set_bool");
		assert!(config_get_bool(group, "flag", false));
		assert!(config_get_bool(group, "unset", true), "fallback on missing key");

		config_set_i64(group, "count", -42).expect("set_i64");
		assert_eq!(config_get_i64(group, "count", 0), -42);

		config_set_f64(group, "ratio", 2.5).expect("set_f64");
		assert_eq!(config_get_f64(group, "ratio", 0.0), 2.5);

		// The global (group-less) lane works too.
		config_set(None, "nogroup", "v").expect("set global");
		assert_eq!(config_get(None, "nogroup").as_deref(), Some("v"));
	}

	#[test]
	fn color_settings_roundtrip_and_restore() {
		let saved = (
			color_working_space(),
			color_output_gamut(),
			color_output_transfer(),
		);

		color_set_pipeline_settings("srgb_legacy", "displayp3", "srgb").expect("set");
		assert_eq!(color_working_space(), "srgb_legacy");
		assert_eq!(color_output_gamut(), "displayp3");

		color_set_pipeline_settings(&saved.0, &saved.1, &saved.2).expect("restore");
	}

	#[test]
	fn color_default_config_loads_headless() {
		color_set_up_default_config(None).expect("bundled config");
		let _ = color_config_path();
	}

	#[test]
	fn displayicc_queries_never_crash() {
		let _ = displayicc_system_display_icc();
		assert_eq!(displayicc_system_display_icc_for("no-such-monitor"), None);
	}
}

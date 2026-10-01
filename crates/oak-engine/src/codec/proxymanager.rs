//! C ABI for `oak_codec::proxymanager` (prefix `oak_codec_proxy_`).
//!
//! `ProxyParams` is `repr(C)` and crosses by value; the C header must
//! declare the identical layout (fixed 32-byte NUL-terminated string
//! fields). Filenames cross as borrowed `(ptr, len)` input pairs and
//! snprintf-style output buffers.

use oak_codec::proxymanager::{ProxyManager, ProxyParams, ProxyState};

/// POD result of [`oak_codec_proxy_get_or_start`].
#[repr(C)]
pub struct OakCodecProxyResult {
	/// `ProxyState` discriminant (Missing/Generating/Ready/Failed =
	/// 0/1/2/3); -1 on error.
	pub state: i32,
	/// NUL-terminated proxy filename (empty on error).
	pub filename: [u8; 1024],
}

/// The compiled-in default proxy parameters.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_proxy_params_default() -> ProxyParams {
	ProxyManager::proxy_params_default()
}

/// The proxy parameters from the user's configuration (defaults when
/// unset).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_proxy_params_from_config() -> ProxyParams {
	ProxyManager::proxy_params_from_config()
}

/// The on-disk proxy state for `proxy_filename` as a `ProxyState`
/// discriminant (Missing/Generating/Ready/Failed = 0/1/2/3). Null or
/// non-UTF-8 input reads as `Missing` (0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_proxy_get_state(
	proxy_filename: *const u8,
	proxy_filename_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { super::str_arg(proxy_filename, proxy_filename_len) }) else {
		return ProxyState::Missing as i32;
	};
	ProxyManager::get_proxy_state(filename) as i32
}

/// Whether the proxy file carries an audio stream; false on null or
/// non-UTF-8 input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_proxy_filename_has_audio(
	proxy_filename: *const u8,
	proxy_filename_len: usize,
) -> bool {
	let Some(filename) = (unsafe { super::str_arg(proxy_filename, proxy_filename_len) }) else {
		return false;
	};
	ProxyManager::proxy_filename_has_audio(filename)
}

/// Derives the proxy filename for a source stream, snprintf-style (see
/// `copy_str_out`); -1 on error. `params` crosses by value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_proxy_get_filename(
	cache_path: *const u8,
	cache_path_len: usize,
	source_filename: *const u8,
	source_filename_len: usize,
	stream_index: i32,
	params: ProxyParams,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let (Some(cache), Some(source)) = (
		unsafe { super::str_arg(cache_path, cache_path_len) },
		unsafe { super::str_arg(source_filename, source_filename_len) },
	) else {
		return -1;
	};
	match ProxyManager::get_proxy_filename(cache, source, stream_index, &params) {
		Ok(filename) => super::copy_str_out(&filename, buf, buf_len),
		Err(_) => -1,
	}
}

/// The working (in-progress) filename for a proxy, snprintf-style;
/// -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_proxy_get_working_filename(
	proxy_filename: *const u8,
	proxy_filename_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { super::str_arg(proxy_filename, proxy_filename_len) }) else {
		return -1;
	};
	match ProxyManager::get_working_filename(filename) {
		Ok(working) => super::copy_str_out(&working, buf, buf_len),
		Err(_) => -1,
	}
}

/// Returns the proxy for a source stream, starting generation through
/// the registered task callback when it is missing. Without a registered
/// callback the state reports `Missing`; on submission failure it
/// reports `Failed`. `state` is -1 only for invalid input/engine errors
/// (then `filename` is empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_proxy_get_or_start(
	cache_path: *const u8,
	cache_path_len: usize,
	source_filename: *const u8,
	source_filename_len: usize,
	stream_index: i32,
	params: ProxyParams,
) -> OakCodecProxyResult {
	let error = OakCodecProxyResult {
		state: -1,
		filename: [0; 1024],
	};
	let (Some(cache), Some(source)) = (
		unsafe { super::str_arg(cache_path, cache_path_len) },
		unsafe { super::str_arg(source_filename, source_filename_len) },
	) else {
		return error;
	};
	match ProxyManager::instance().get_or_start(cache, source, stream_index, &params) {
		Ok((state, filename)) => {
			let bytes = filename.as_bytes();
			let n = bytes.len().min(1023);
			let mut out = OakCodecProxyResult {
				state: state as i32,
				filename: [0; 1024],
			};
			out.filename[..n].copy_from_slice(&bytes[..n]);
			out.filename[n] = 0;
			out
		}
		Err(_) => error,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cstr32(bytes: &[u8]) -> [u8; 32] {
		let mut out = [0u8; 32];
		out[..bytes.len()].copy_from_slice(bytes);
		out
	}

	/// Unique temp cache dir per test (proxy files persist across runs).
	fn temp_cache(name: &str) -> String {
		let dir = std::env::temp_dir().join(format!(
			"oakengine_proxy_{name}_{}",
			std::process::id()
		));
		let _ = std::fs::create_dir_all(&dir);
		dir.to_string_lossy().into_owned()
	}

	unsafe extern "C" fn accept_cb(
		_req: *const crate::codec::task::OakCodecTaskRequest,
		_ud: *mut std::ffi::c_void,
	) -> i32 {
		0 // OAKCODEC_OK
	}

	#[test]
	fn get_or_start_reports_missing_without_registrar() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		unsafe { crate::codec::task::oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) };

		let cache = temp_cache("missing");
		let source = "/tmp/oak-gos-source.mp4";
		let result = unsafe {
			oak_codec_proxy_get_or_start(
				cache.as_ptr(),
				cache.len(),
				source.as_ptr(),
				source.len(),
				0,
				oak_codec_proxy_params_default(),
			)
		};
		assert_eq!(result.state, 0, "Missing without a registered callback");
		assert!(result.filename[0] != 0, "the filename is still derived");
	}

	#[test]
	fn get_or_start_reports_ready_when_file_exists() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		unsafe { crate::codec::task::oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) };

		let cache = temp_cache("ready");
		let source = "/tmp/oak-gos-source.mp4";
		let mut buf = [0u8; 1024];
		let n = unsafe {
			oak_codec_proxy_get_filename(
				cache.as_ptr(),
				cache.len(),
				source.as_ptr(),
				source.len(),
				0,
				oak_codec_proxy_params_default(),
				buf.as_mut_ptr(),
				buf.len(),
			)
		};
		assert!(n > 0);
		let filename = std::str::from_utf8(&buf[..n as usize]).unwrap();
		std::fs::create_dir_all(std::path::Path::new(filename).parent().unwrap()).unwrap();
		std::fs::write(filename, b"proxy").unwrap();

		let result = unsafe {
			oak_codec_proxy_get_or_start(
				cache.as_ptr(),
				cache.len(),
				source.as_ptr(),
				source.len(),
				0,
				oak_codec_proxy_params_default(),
			)
		};
		assert_eq!(result.state, 2, "Ready");
		let returned = std::str::from_utf8(&result.filename[..n as usize]).unwrap();
		assert_eq!(returned, filename);
	}

	#[test]
	fn get_or_start_reports_generating_with_registrar() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		unsafe {
			crate::codec::task::oak_codec_task_set_submit_cb(
				Some(accept_cb),
				std::ptr::null_mut(),
			)
		};

		let cache = temp_cache("generating");
		let source = "/tmp/oak-gos-source.mp4";
		let result = unsafe {
			oak_codec_proxy_get_or_start(
				cache.as_ptr(),
				cache.len(),
				source.as_ptr(),
				source.len(),
				0,
				oak_codec_proxy_params_default(),
			)
		};
		unsafe { crate::codec::task::oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) };
		assert_eq!(result.state, 1, "Generating once the task is accepted");
	}

	#[test]
	fn get_or_start_rejects_invalid_input() {
		let source = "/tmp/oak-gos-source.mp4";
		let result = unsafe {
			oak_codec_proxy_get_or_start(
				std::ptr::null(),
				0,
				source.as_ptr(),
				source.len(),
				0,
				oak_codec_proxy_params_default(),
			)
		};
		assert_eq!(result.state, -1);
		assert_eq!(result.filename[0], 0);
	}

	#[test]
	fn default_params_match_oak_codec_defaults() {
		let p = oak_codec_proxy_params_default();
		assert_eq!((p.width, p.height, p.divider), (1280, 720, 1));
		assert_eq!(&p.extension[..4], b"mp4\0");
		assert_eq!(&p.preset[..9], b"veryfast\0");
	}

	#[test]
	fn state_of_nonexistent_proxy_is_missing() {
		let path = "/nonexistent/oak-proxy-test.mp4";
		assert_eq!(
			unsafe { oak_codec_proxy_get_state(path.as_ptr(), path.len()) },
			0
		);
		assert_eq!(unsafe { oak_codec_proxy_get_state(std::ptr::null(), 0) }, 0);
	}

	#[test]
	fn filename_derivation_and_working_suffix() {
		let params = ProxyParams {
			extension: cstr32(b"mp4"),
			preset: cstr32(b"veryfast"),
			..oak_codec_proxy_params_default()
		};
		let cache = std::env::temp_dir().to_string_lossy().into_owned();
		let source = "oak-source-clip.mp4";
		let mut buf = [0u8; 1024];
		let n = unsafe {
			oak_codec_proxy_get_filename(
				cache.as_ptr(),
				cache.len(),
				source.as_ptr(),
				source.len(),
				0,
				params.clone(),
				buf.as_mut_ptr(),
				buf.len(),
			)
		};
		assert!(n > 0);
		let filename = std::str::from_utf8(&buf[..n as usize]).unwrap();
		// Platform-agnostic: the derived file lives somewhere under the
		// cache directory (which may gain a proxy subdirectory).
		assert!(
			filename.starts_with(cache.trim_end_matches(['/', '\\'])),
			"unexpected: {filename} (cache {cache})"
		);
		assert!(filename.ends_with(".v1.a1.mp4"), "unexpected: {filename}");

		let n = unsafe {
			oak_codec_proxy_get_working_filename(
				filename.as_ptr(),
				filename.len(),
				buf.as_mut_ptr(),
				buf.len(),
			)
		};
		assert!(n > 0);
		assert!(std::str::from_utf8(&buf[..n as usize])
			.unwrap()
			.ends_with(".working.mp4"));

		// Null source: error, not a panic.
		assert_eq!(
			unsafe {
				oak_codec_proxy_get_filename(
					std::ptr::null(),
					0,
					source.as_ptr(),
					source.len(),
					0,
					params,
					buf.as_mut_ptr(),
					buf.len(),
				)
			},
			-1
		);
	}
}

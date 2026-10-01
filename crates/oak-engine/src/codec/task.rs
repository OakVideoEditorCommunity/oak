//! C ABI for the oak-codec background-task submit lane (prefix
//! `oak_codec_task_`).
//!
//! The host (e.g. the app or a worker) registers one submit callback;
//! producers call [`oak_codec_task_submit`] to request background work
//! (proxy transcodes, audio conforms). `OakCodecTaskRequest` is
//! `repr(C)` from oak-codec and is declared identically in the C header.

use std::ffi::{CStr, c_void};

use oak_codec::error::{Error, OAKCODEC_OK};
use oak_codec::task::{
	TaskKind, TaskRequest, set_task_submit_cb, submit_task, task_submit_is_registered,
};

/// C ABI mirror of oak-codec's `TaskRequest` for the submit callback.
/// Strings are borrowed C pointers, valid only for the duration of the
/// call; the callback must copy anything it retains.
#[repr(C)]
pub struct OakCodecTaskRequest {
	/// `TaskKind` value (Conform = 0, Proxy = 1).
	pub kind: i32,
	/// Source media filename.
	pub input_filename: *const std::ffi::c_char,
	/// Final destination path.
	pub output_filename: *const std::ffi::c_char,
	/// Stream inside the source media.
	pub stream_index: i32,
	/// Conform: target sample rate.
	pub sample_rate: i32,
	/// Conform: target channel-layout mask.
	pub channel_layout: u64,
	/// Conform: target sample format (enum as int).
	pub sample_format: i32,
	/// Proxy: target width (0 = unspecified/divider-based).
	pub proxy_width: i32,
	/// Proxy: target height (0 = unspecified/divider-based).
	pub proxy_height: i32,
}

/// The extern-C submit callback typedef. Returns `OAKCODEC_OK` (0) on
/// accept, else a negative `OAKCODEC_E_*` code.
pub type OakCodecTaskSubmitFn =
	unsafe extern "C" fn(req: *const OakCodecTaskRequest, userdata: *mut c_void) -> i32;

/// Whether a submit callback is currently registered.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_task_submit_is_registered() -> bool {
	task_submit_is_registered()
}

/// Registers (or replaces, or with `None` clears) the process-wide task
/// submit callback. The callback receives a borrowed
/// `OakCodecTaskRequest` valid only for the call duration; return
/// `OAKCODEC_OK` (0) to accept, a negative `OAKCODEC_E_*` to reject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_task_set_submit_cb(
	cb: Option<OakCodecTaskSubmitFn>,
	userdata: *mut c_void,
) {
	match cb {
		None => set_task_submit_cb(None, std::ptr::null_mut()),
		Some(cb) => {
			// The registry is a process-lifetime singleton, so leaking one
			// adapter closure per registration is deliberate (the common
			// case registers exactly once, at startup).
			let adapter: &'static oak_codec::task::TaskSubmitFn =
				Box::leak(Box::new(move |req: &TaskRequest, _ud| {
					// Bind the C strings to locals so their pointers stay
					// valid for the whole callback call.
					let input = std::ffi::CString::new(req.input_filename).unwrap_or_default();
					let output = std::ffi::CString::new(req.output_filename).unwrap_or_default();
					let c_req = OakCodecTaskRequest {
						kind: req.kind as i32,
						input_filename: input.as_ptr(),
						output_filename: output.as_ptr(),
						stream_index: req.stream_index,
						sample_rate: req.sample_rate,
						channel_layout: req.channel_layout,
						sample_format: req.sample_format,
						proxy_width: req.proxy_width,
						proxy_height: req.proxy_height,
					};
					let rc = unsafe { cb(&c_req, userdata) };
					if rc == OAKCODEC_OK {
						Ok(())
					} else {
						Err(Error::Failed(format!("task submit rejected (code {rc})")))
					}
				}));
			set_task_submit_cb(Some(adapter), userdata);
		}
	}
}

/// Submits one task request. Returns 1 when the task was accepted, 0
/// when no callback is registered, -1 on invalid input or engine error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_task_submit(req: *const OakCodecTaskRequest) -> i32 {
	if req.is_null() {
		return -1;
	}
	let req = unsafe { &*req };
	let c_str = |p: *const std::ffi::c_char| -> Option<&str> {
		if p.is_null() {
			return Some("");
		}
		unsafe { CStr::from_ptr(p) }.to_str().ok()
	};
	let (Some(input), Some(output)) = (c_str(req.input_filename), c_str(req.output_filename))
	else {
		return -1;
	};
	let kind = match req.kind {
		0 => TaskKind::Conform,
		1 => TaskKind::Proxy,
		_ => return -1,
	};
	let request = TaskRequest {
		kind,
		input_filename: input,
		output_filename: output,
		stream_index: req.stream_index,
		sample_rate: req.sample_rate,
		channel_layout: req.channel_layout,
		sample_format: req.sample_format,
		proxy_width: req.proxy_width,
		proxy_height: req.proxy_height,
	};
	match submit_task(&request) {
		Ok(accepted) => accepted as i32,
		Err(_) => -1,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
	static CALLS: AtomicUsize = AtomicUsize::new(0);
	static SEEN_KIND: AtomicI32 = AtomicI32::new(-1);
	static SEEN_STREAM: AtomicI32 = AtomicI32::new(-1);

	unsafe extern "C" fn recording_cb(
		req: *const OakCodecTaskRequest,
		_userdata: *mut c_void,
	) -> i32 {
		CALLS.fetch_add(1, Ordering::SeqCst);
		unsafe {
			SEEN_KIND.store((*req).kind, Ordering::SeqCst);
			SEEN_STREAM.store((*req).stream_index, Ordering::SeqCst);
			// The borrowed strings must be readable for the call duration.
			let _ = CStr::from_ptr((*req).input_filename);
		}
		OAKCODEC_OK
	}

	fn proxy_request(input: &std::ffi::CString, output: &std::ffi::CString) -> OakCodecTaskRequest {
		OakCodecTaskRequest {
			kind: 1,
			input_filename: input.as_ptr(),
			output_filename: output.as_ptr(),
			stream_index: 3,
			sample_rate: 0,
			channel_layout: 0,
			sample_format: 0,
			proxy_width: 640,
			proxy_height: 360,
		}
	}

	#[test]
	fn submit_without_registrar_is_not_accepted() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		unsafe { oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) };
		assert!(!oak_codec_task_submit_is_registered());

		let input = std::ffi::CString::new("/tmp/a.mp4").unwrap();
		let output = std::ffi::CString::new("/tmp/a.proxy.mp4").unwrap();
		let req = proxy_request(&input, &output);
		assert_eq!(unsafe { oak_codec_task_submit(&req) }, 0);
	}

	#[test]
	fn registered_callback_receives_the_request() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		unsafe { oak_codec_task_set_submit_cb(Some(recording_cb), std::ptr::null_mut()) };
		assert!(oak_codec_task_submit_is_registered());

		let input = std::ffi::CString::new("/tmp/a.mp4").unwrap();
		let output = std::ffi::CString::new("/tmp/a.proxy.mp4").unwrap();
		let req = proxy_request(&input, &output);
		assert_eq!(unsafe { oak_codec_task_submit(&req) }, 1);
		assert_eq!(CALLS.load(Ordering::SeqCst), 1);
		assert_eq!(SEEN_KIND.load(Ordering::SeqCst), 1, "TaskKind::Proxy");
		assert_eq!(SEEN_STREAM.load(Ordering::SeqCst), 3);

		unsafe { oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) };
		assert!(!oak_codec_task_submit_is_registered());
	}

	#[test]
	fn submit_rejects_invalid_input() {
		let _guard = crate::codec::TASK_REG_SERIAL.lock().unwrap();
		assert_eq!(unsafe { oak_codec_task_submit(std::ptr::null()) }, -1);

		let input = std::ffi::CString::new("/tmp/a.mp4").unwrap();
		let output = std::ffi::CString::new("/tmp/a.proxy.mp4").unwrap();
		let mut req = proxy_request(&input, &output);
		req.kind = 99;
		assert_eq!(unsafe { oak_codec_task_submit(&req) }, -1);
	}
}

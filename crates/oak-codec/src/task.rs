// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Background task submission hook (`include/codec/task.h`).
//!
//! The codec module needs occasional background work (audio conforms,
//! proxy transcodes). The task system itself splits out at milestone M8;
//! until then oakcodec exposes a single global submit callback. A host
//! (M8: oaktask) registers with [`set_task_submit_cb`]; the conform/proxy
//! managers call it whenever they need a task. With no callback, managers
//! report work as unavailable — they never crash and never block.

use std::ffi::c_void;
use std::sync::Mutex;



/// Kinds of background tasks oakcodec can request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum TaskKind {
	/// Audio conform to pcm cache files.
	Conform = 0,
	/// Video proxy transcode.
	Proxy = 1,
}

/// Description of one background task request.
///
/// All strings are borrowed and only valid for the duration of the submit
/// call; the callback must copy anything it retains.
#[repr(C)]
pub struct TaskRequest<'a> {
	/// `TaskKind`.
	pub kind: TaskKind,
	/// Source media filename.
	pub input_filename: &'a str,
	/// Final destination path (see field docs in include/codec/task.h).
	pub output_filename: &'a str,
	/// Stream inside the source media.
	pub stream_index: i32,
	/// Conform: target sample rate.
	pub sample_rate: i32,
	/// Conform: target channel layout mask.
	pub channel_layout: u64,
	/// Conform: target sample format (enum as int).
	pub sample_format: i32,
	/// Proxy: target width (0 = unspecified/divider-based).
	pub proxy_width: i32,
	/// Proxy: target height (0 = unspecified/divider-based).
	pub proxy_height: i32,
}

/// Task submit callback signature.
///
/// Returns `Ok(())` if the task was accepted (completed synchronously or
/// queued), `Err` if the request was rejected.
pub type TaskSubmitFn = dyn Fn(&TaskRequest, *mut std::ffi::c_void) -> crate::error::Result<()>;

/// One registered submit callback. Mirrors the C++
/// `g_task_cb`/`g_task_cb_userdata` pair.
enum SubmitCb {
	/// No callback registered.
	None,
	/// Crate-internal Rust closure registered via [`set_task_submit_cb`].
	Rust {
		/// Raw fat-pointer to the `&'static TaskSubmitFn` (kept `*const` so
		/// the registry is `Send`).
		cb: *const TaskSubmitFn,
		/// Opaque userdata passed back on each call.
		userdata: *mut c_void,
	},
}

// # Safety: the stored fat pointer to a 'static closure is only
// dereferenced/called while holding the registry mutex, and the closure
// outlives registration by contract. Moving the enum between threads under
// the lock therefore cannot alias.
unsafe impl Send for SubmitCb {}

/// The global task submit callback registry. Only one callback is held at
/// a time; registering replaces it, `None` clears it. Thread-safe.
static TASK_SUBMIT: Mutex<SubmitCb> = Mutex::new(SubmitCb::None);

/// Registers (or replaces) the global task submit callback. Pass `None` to
/// unregister. Thread-safe. Interim state (pre-M8): nobody registers and all
/// task-dependent work reports unavailable.
pub fn set_task_submit_cb(cb: Option<&'static TaskSubmitFn>, userdata: *mut std::ffi::c_void) {
	let mut g = TASK_SUBMIT.lock().unwrap();
	*g = match cb {
		Some(cb) => SubmitCb::Rust {
			cb: cb as *const TaskSubmitFn,
			userdata,
		},
		None => SubmitCb::None,
	};
}

/// Returns 1 if a submit callback is currently registered, else 0.
/// Thread-safe.
pub fn task_submit_is_registered() -> bool {
	let g = TASK_SUBMIT.lock().unwrap();
	!matches!(&*g, SubmitCb::None)
}

/// Submit a task through the registered callback, if any.
///
/// Returns `Ok(false)` when no callback is registered (nothing submitted),
/// `Ok(true)` when accepted, or `Err` when the callback rejected it.
pub fn submit_task(req: &TaskRequest) -> crate::error::Result<bool> {
	let g = TASK_SUBMIT.lock().unwrap();
	match &*g {
		SubmitCb::None => Ok(false),
		SubmitCb::Rust { cb, userdata } => {
			// # Safety: the fat pointer was stored by set_task_submit_cb and
			// points to a 'static closure that outlives this call.
			let cb = unsafe { &**cb };
			match cb(req, *userdata) {
				Ok(()) => Ok(true),
				Err(e) => Err(e),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	// Same registry lock the conform/proxy/ffi tests use: the submit
	// callback is process-global and every test that mutates it must
	// serialize on the same mutex.
	use crate::conformmanager::test_util::REG_LOCK;

	#[test]
	fn submit_via_rust_closure_and_clear() {
		let _g = REG_LOCK.lock().unwrap();

		// A Rust closure that accepts and records the request.
		let accepted = std::sync::Arc::new(std::sync::Mutex::new(false));
		let recorded = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
		let acc = accepted.clone();
		let rec = recorded.clone();
		let cb: &'static TaskSubmitFn = Box::leak(Box::new(
			move |req: &TaskRequest, _ud: *mut std::ffi::c_void| {
				*acc.lock().unwrap() = true;
				*rec.lock().unwrap() = Some(req.output_filename.to_string());
				Ok(())
			},
		));
		set_task_submit_cb(Some(cb), std::ptr::null_mut());
		assert!(task_submit_is_registered());

		let req = TaskRequest {
			kind: TaskKind::Conform,
			input_filename: "in.mp4",
			output_filename: "out.pcm",
			stream_index: 1,
			sample_rate: 48000,
			channel_layout: 0x3,
			sample_format: 10,
			proxy_width: 0,
			proxy_height: 0,
		};
		assert!(submit_task(&req).unwrap());
		assert!(*accepted.lock().unwrap());
		assert_eq!(recorded.lock().unwrap().as_deref(), Some("out.pcm"));

		// Clearing the callback: nothing submitted.
		set_task_submit_cb(None, std::ptr::null_mut());
		assert!(!task_submit_is_registered());
		assert!(!submit_task(&req).unwrap());
	}

	#[test]
	fn task_kind_values_match_abi() {
		assert_eq!(TaskKind::Conform as i32, OAKCODEC_TASK_CONFORM);
		assert_eq!(TaskKind::Proxy as i32, OAKCODEC_TASK_PROXY);
	}

	// ABI constants mirrored from include/codec/task.h.
	const OAKCODEC_TASK_CONFORM: i32 = 0;
	const OAKCODEC_TASK_PROXY: i32 = 1;
}

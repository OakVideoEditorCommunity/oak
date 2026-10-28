//! Safe proxy lifecycle operations for an editor session.

use std::ffi::c_void;

use crate::codec::{ProxyState, string_out};
use crate::edit::EditCommand;
use crate::node::NodeId;
use crate::task::Task;

use super::EditorSession;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_proxy_generate(
		session: *const c_void,
		footage: u64,
		cache_path: *const u8,
		cache_path_len: usize,
		error: *mut u8,
		error_len: usize,
	) -> u64;
	fn oak_editor_proxy_complete(
		session: *const c_void,
		footage: u64,
		task_id: u64,
		succeeded: bool,
	) -> bool;
	fn oak_editor_proxy_status(
		session: *const c_void,
		footage: u64,
		out: *mut RawProxyStatus,
	) -> bool;
	fn oak_editor_proxy_path(
		session: *const c_void,
		footage: u64,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct RawProxyStatus {
	state: i32,
	enabled: bool,
	has_custom_params: bool,
	can_generate: bool,
	has_proxy: bool,
}

/// UI-independent proxy metadata for one footage node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyStatus {
	/// Live task state first, then on-disk state and the last recorded failure.
	pub state: ProxyState,
	/// Whether decoding may use the proxy instead of the original media.
	pub enabled: bool,
	/// Whether this footage overrides global generation parameters.
	pub has_custom_params: bool,
	/// Whether the footage is valid and contains a video stream.
	pub can_generate: bool,
	/// Whether a proxy path is recorded, even if its file is currently missing.
	pub has_proxy: bool,
}

impl EditorSession {
	/// Starts an asynchronous proxy transcode and returns its pollable task.
	///
	/// No transcoding runs on the caller's thread. Completion must be applied with
	/// [`EditorSession::complete_proxy`] after the task reaches a terminal state.
	pub fn generate_proxy(&self, footage: NodeId, cache_path: &str) -> Result<Task, String> {
		let mut error = [0u8; 1024];
		let id = unsafe {
			oak_editor_proxy_generate(
				self.ptr,
				footage.raw(),
				cache_path.as_ptr(),
				cache_path.len(),
				error.as_mut_ptr(),
				error.len(),
			)
		};
		Task::new(id).ok_or_else(|| {
			let end = error.iter().position(|&b| b == 0).unwrap_or(error.len());
			String::from_utf8_lossy(&error[..end]).into_owned()
		})
	}

	/// Finalizes only the matching active task, preventing stale-worker updates.
	pub fn complete_proxy(&self, footage: NodeId, task: &Task, succeeded: bool) -> bool {
		unsafe { oak_editor_proxy_complete(self.ptr, footage.raw(), task.id(), succeeded) }
	}

	/// Reads proxy lifecycle and footage capability flags without exposing codec
	/// or node behavior types through the SDK.
	pub fn proxy_status(&self, footage: NodeId) -> Option<ProxyStatus> {
		let mut raw = RawProxyStatus::default();
		unsafe { oak_editor_proxy_status(self.ptr, footage.raw(), &mut raw) }.then(|| ProxyStatus {
			state: match raw.state {
				1 => ProxyState::Generating,
				2 => ProxyState::Ready,
				3 => ProxyState::Failed,
				_ => ProxyState::Missing,
			},
			enabled: raw.enabled,
			has_custom_params: raw.has_custom_params,
			can_generate: raw.can_generate,
			has_proxy: raw.has_proxy,
		})
	}

	/// Returns the recorded proxy filename, or `None` for invalid footage.
	pub fn proxy_path(&self, footage: NodeId) -> Option<String> {
		string_out(|buf, len| unsafe { oak_editor_proxy_path(self.ptr, footage.raw(), buf, len) })
	}

	/// Deletes the final and working proxy files and clears the association.
	///
	/// Active generation is rejected so a worker cannot recreate a deleted file.
	/// This operation has no undo entry because undo cannot restore file contents.
	pub fn delete_proxy(&self, footage: NodeId) -> Result<(), String> {
		self.execute(EditCommand::DeleteProxy { footage })
			.map(|_| ())
	}

	/// Enables or disables proxy decoding for one footage without starting work.
	pub fn set_proxy_enabled(&self, footage: NodeId, enabled: bool) -> Result<(), String> {
		self.execute(EditCommand::SetProxyEnabled { footage, enabled })
			.map(|_| ())
	}
}

#[cfg(test)]
mod tests {
	use crate::codec::{ProxyState, proxy_get_filename};
	use crate::editor::EditorSession;
	use crate::task::TaskState;

	#[test]
	fn proxy_generation_progress_completion_enable_disable_and_delete() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let media = crate::test_support::make_test_clip("sdk_proxy_lifecycle");
		let footage = session.project().import_footage(&media).expect("footage");
		let cache = std::env::temp_dir()
			.join(format!("oak_sdk_proxy_{}", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let _ = std::fs::remove_dir_all(&cache);
		std::fs::create_dir_all(&cache).expect("cache directory");

		let initial = session.proxy_status(footage).expect("initial status");
		assert_eq!(initial.state, ProxyState::Missing);
		assert!(initial.can_generate);
		assert!(!initial.has_proxy);
		let mut custom = crate::codec::ProxyParams::default();
		custom.width = 32;
		custom.height = 32;
		custom.divider = 1;
		session
			.execute(crate::edit::EditCommand::SetFootageProxyParams {
				footage,
				params: custom,
			})
			.expect("custom proxy params");
		let params = session
			.project()
			.footage_effective_proxy_params(footage)
			.expect("effective proxy params");
		assert_eq!((params.width, params.height, params.divider), (32, 32, 1));
		let source = session.project().footage_filename(footage).expect("source");
		let expected_path = proxy_get_filename(&cache, &source, 0, &params).expect("proxy path");

		let task = session
			.generate_proxy(footage, &cache)
			.expect("start generation");
		assert!(task.title().contains("Generating Proxy"));
		assert_eq!(
			session.proxy_status(footage).unwrap().state,
			ProxyState::Generating
		);
		assert!(session.generate_proxy(footage, &cache).is_err());
		assert!(session.delete_proxy(footage).is_err());
		assert!(task.wait(), "proxy transcode failed: {}", task.error());
		assert_eq!(task.state(), TaskState::Succeeded);
		assert!((0.0..=1.0).contains(&task.progress()));
		assert!(session.complete_proxy(footage, &task, true));
		assert!(!session.complete_proxy(footage, &task, true));

		let ready = session.proxy_status(footage).expect("ready status");
		assert_eq!(ready.state, ProxyState::Ready);
		assert!(ready.enabled);
		assert_eq!(
			session.proxy_path(footage).as_deref(),
			Some(expected_path.as_str())
		);
		assert!(std::path::Path::new(&expected_path).is_file());
		session.set_proxy_enabled(footage, false).expect("disable");
		assert!(!session.proxy_status(footage).unwrap().enabled);
		session.set_proxy_enabled(footage, true).expect("enable");
		assert!(session.proxy_status(footage).unwrap().enabled);

		session.delete_proxy(footage).expect("delete proxy");
		assert!(!std::path::Path::new(&expected_path).exists());
		let deleted = session.proxy_status(footage).expect("deleted status");
		assert_eq!(deleted.state, ProxyState::Missing);
		assert!(!deleted.has_proxy);
		assert!(session.delete_proxy(crate::node::NodeId::INVALID).is_err());

		drop(task);
		std::fs::remove_file(&media).expect("remove source before failed generation");
		let failed = session
			.generate_proxy(footage, &cache)
			.expect("missing input still starts an asynchronous task");
		assert!(!failed.wait());
		assert_eq!(failed.state(), TaskState::Failed);
		assert!(!failed.error().is_empty());
		assert!(session.complete_proxy(footage, &failed, false));
		assert_eq!(session.proxy_status(footage).unwrap().state, ProxyState::Failed);
		drop(failed);
		let _ = std::fs::remove_dir_all(cache);
	}
}

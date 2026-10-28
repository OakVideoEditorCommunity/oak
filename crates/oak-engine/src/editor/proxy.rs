//! Session-scoped proxy generation, completion and status queries.
//!
//! Transcodes run in the engine task registry. The session owns the association
//! between a footage node and its active task, while this module is the only
//! worker-lifecycle path allowed to change proxy generation state in the graph.

use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;

use super::EditorSessionHandle;
use crate::codec::{copy_str_out, str_arg};

/// POD proxy metadata returned by [`oak_editor_proxy_status`].
///
/// `state` uses the public codec proxy-state numbering: 0 missing, 1 generating,
/// 2 ready and 3 failed. A live session task takes precedence over disk state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakEditorProxyStatus {
	pub state: i32,
	pub enabled: bool,
	pub has_custom_params: bool,
	pub can_generate: bool,
	pub has_proxy: bool,
}

fn footage_mut(
	project: &mut oak_node::project::Project,
	footage: NodeId,
) -> Option<&mut FootageBehavior> {
	project
		.graph
		.get_mut(footage)
		.and_then(|entry| entry.behavior.as_any_mut())
		.and_then(|behavior| behavior.downcast_mut::<FootageBehavior>())
}

/// Starts a proxy transcode without blocking the calling/UI thread.
///
/// On success the footage is immediately marked generating and the returned
/// nonzero task ID can be polled through `oak_task_*`. The caller must eventually
/// call [`oak_editor_proxy_complete`] with that same ID and release the task.
/// Duplicate generation for one footage in the same session is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_proxy_generate(
	session: *const EditorSessionHandle,
	footage: u64,
	cache_path: *const u8,
	cache_path_len: usize,
	error: *mut u8,
	error_len: usize,
) -> u64 {
	let result = (|| {
		let session = unsafe { session.as_ref() }.ok_or("no editor session")?;
		let footage = NodeId::from_identity(footage).ok_or("invalid footage id")?;
		let cache_path =
			unsafe { str_arg(cache_path, cache_path_len) }.ok_or("invalid proxy cache path")?;
		let mut state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
		if state.proxy_tasks.contains_key(&footage.identity()) {
			return Err("a proxy is already generating for this footage");
		}
		let (filename, stream_index, params) = {
			let mut project = state.project.lock().unwrap_or_else(|e| e.into_inner());
			let footage_behavior =
				footage_mut(&mut project, footage).ok_or("entry is not footage")?;
			if footage_behavior.filename.is_empty() {
				return Err("the footage has no media file");
			}
			let stream_index = footage_behavior
				.streams
				.iter()
				.find(|stream| stream.is_video)
				.map(|stream| stream.index)
				.ok_or("the footage has no video stream")?;
			(
				footage_behavior.filename.clone(),
				stream_index,
				footage_behavior.effective_proxy_params(),
			)
		};
		let proxy_path = oak_codec::proxymanager::ProxyManager::get_proxy_filename(
			cache_path,
			&filename,
			stream_index,
			&params,
		)
		.map_err(|_| "failed to build the proxy filename")?;
		let task_id = crate::task::spawn_proxy(&filename, &proxy_path, stream_index, &params);
		let mut project = state.project.lock().unwrap_or_else(|e| e.into_inner());
		let Some(footage_behavior) = footage_mut(&mut project, footage) else {
			drop(project);
			crate::task::oak_task_cancel(task_id);
			crate::task::oak_task_release(task_id);
			return Err("entry is not footage");
		};
		footage_behavior.set_proxy(&proxy_path, 1, stream_index, params.version, true);
		drop(project);
		state.proxy_tasks.insert(footage.identity(), task_id);
		Ok(task_id)
	})();
	match result {
		Ok(task_id) => task_id,
		Err(message) => {
			copy_str_out(message, error, error_len);
			0
		}
	}
}

/// Applies the terminal state of a previously started proxy task.
///
/// Returns false when the footage/task pair is not the active generation. This
/// guards against an old worker overwriting metadata for a newer request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_proxy_complete(
	session: *const EditorSessionHandle,
	footage: u64,
	task_id: u64,
	succeeded: bool,
) -> bool {
	let Some(session) = (unsafe { session.as_ref() }) else {
		return false;
	};
	let Some(footage) = NodeId::from_identity(footage) else {
		return false;
	};
	let mut state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
	if state.proxy_tasks.get(&footage.identity()) != Some(&task_id) {
		return false;
	}
	state.proxy_tasks.remove(&footage.identity());
	let mut project = state.project.lock().unwrap_or_else(|e| e.into_inner());
	if let Some(footage) = footage_mut(&mut project, footage) {
		footage.proxy_state = if succeeded { 2 } else { 3 };
		if succeeded {
			footage.proxy_enabled = true;
		}
	}
	true
}

/// Reads one footage's proxy status, giving active session tasks priority over
/// working files left by an interrupted process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_proxy_status(
	session: *const EditorSessionHandle,
	footage: u64,
	out: *mut OakEditorProxyStatus,
) -> bool {
	let (Some(session), Some(footage), Some(out)) = (
		unsafe { session.as_ref() },
		NodeId::from_identity(footage),
		unsafe { out.as_mut() },
	) else {
		return false;
	};
	let state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
	let project = state.project.lock().unwrap_or_else(|e| e.into_inner());
	let Some(footage_behavior) = project
		.graph
		.get(footage)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>())
	else {
		return false;
	};
	let disk_state = if footage_behavior.proxy.is_empty() {
		oak_codec::proxymanager::ProxyState::Missing
	} else {
		oak_codec::proxymanager::ProxyManager::get_proxy_state(&footage_behavior.proxy)
	};
	let proxy_state = if state.proxy_tasks.contains_key(&footage.identity()) {
		oak_codec::proxymanager::ProxyState::Generating
	} else {
		match disk_state {
			oak_codec::proxymanager::ProxyState::Ready => {
				oak_codec::proxymanager::ProxyState::Ready
			}
			oak_codec::proxymanager::ProxyState::Generating => {
				oak_codec::proxymanager::ProxyState::Generating
			}
			_ if footage_behavior.proxy_state == 3 => oak_codec::proxymanager::ProxyState::Failed,
			_ => oak_codec::proxymanager::ProxyState::Missing,
		}
	};
	*out = OakEditorProxyStatus {
		state: proxy_state as i32,
		enabled: footage_behavior.proxy_enabled,
		has_custom_params: footage_behavior.has_custom_proxy_params(),
		can_generate: footage_behavior.valid
			&& footage_behavior
				.streams
				.iter()
				.any(|stream| stream.is_video),
		has_proxy: !footage_behavior.proxy.is_empty(),
	};
	true
}

/// Returns the recorded proxy path, including a path whose file is missing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_proxy_path(
	session: *const EditorSessionHandle,
	footage: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let (Some(session), Some(footage)) =
		(unsafe { session.as_ref() }, NodeId::from_identity(footage))
	else {
		return -1;
	};
	let state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
	let project = state.project.lock().unwrap_or_else(|e| e.into_inner());
	project
		.graph
		.get(footage)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>())
		.map(|footage| copy_str_out(&footage.proxy, buf, buf_len))
		.unwrap_or(-1)
}

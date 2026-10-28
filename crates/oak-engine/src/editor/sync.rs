//! Read-only clip metadata used by synchronization eligibility and placement.

use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;

use super::EditorSessionHandle;
use crate::codec::copy_str_out;
use crate::coretypes::OakRational;
use crate::handle::NativeMirror;

/// POD timing and source-timecode metadata for one timeline block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakEditorClipSyncInfo {
	pub timeline_in: OakRational,
	pub timeline_out: OakRational,
	pub media_in: OakRational,
	pub source_start_time: OakRational,
	pub has_source_start_time: bool,
	pub media_length_seconds: f64,
}

/// Reads one block's synchronization metadata without exposing graph nodes.
///
/// Source footage is resolved through the block's upstream input graph. Blocks
/// without footage still return their timeline range but report no source start.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_clip_sync_info(
	session: *const EditorSessionHandle,
	clip: u64,
	out: *mut OakEditorClipSyncInfo,
) -> bool {
	let (Some(session), Some(clip), Some(out)) = (
		unsafe { session.as_ref() },
		NodeId::from_identity(clip),
		unsafe { out.as_mut() },
	) else {
		return false;
	};
	let state = session.inner.lock().unwrap_or_else(|e| e.into_inner());
	let project = state.project.lock().unwrap_or_else(|e| e.into_inner());
	let Some((timeline_in, timeline_out, media_in)) =
		crate::editor_commands::clip_range(&project.graph, clip)
	else {
		return false;
	};
	let source = crate::editor_commands::find_input_footage(&project.graph, clip)
		.and_then(|footage| project.graph.get(footage))
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>());
	*out = OakEditorClipSyncInfo {
		timeline_in: OakRational::from_native(&timeline_in),
		timeline_out: OakRational::from_native(&timeline_out),
		media_in: OakRational::from_native(&media_in),
		source_start_time: source
			.map(|footage| OakRational::from_native(&footage.source_start_time))
			.unwrap_or_default(),
		has_source_start_time: source.is_some_and(|footage| footage.has_source_start_time),
		media_length_seconds: (timeline_out - timeline_in).to_f64(),
	};
	true
}

/// Returns the media filename of the first upstream footage node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_clip_media_filename(
	session: *const EditorSessionHandle,
	clip: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let (Some(session), Some(clip)) = (unsafe { session.as_ref() }, NodeId::from_identity(clip))
	else {
		return -1;
	};
	let project = session.project();
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	crate::editor_commands::clip_media_filename(&guard.graph, clip)
		.map(|filename| copy_str_out(&filename, buf, buf_len))
		.unwrap_or(-1)
}

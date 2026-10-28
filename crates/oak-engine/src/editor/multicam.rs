//! Read-only multicam graph queries for editor sessions.

use oak_node::id::NodeId;

use super::EditorSessionHandle;
use crate::coretypes::OakRational;
use crate::editor_commands as ops;
use crate::handle::NativeMirror;
use crate::node::OAK_NODE_ID_INVALID;

/// ABI mirror of the semantic multicam state returned by editor commands.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakEditorMulticamState {
	pub sequence: u64,
	pub multicam: u64,
	pub clip: u64,
	pub source_count: i32,
	pub current_source: i32,
}

impl NativeMirror for OakEditorMulticamState {
	type Native = ops::MulticamState;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			sequence: native.sequence.identity(),
			multicam: native.multicam.identity(),
			clip: native.clip.identity(),
			source_count: native.source_count,
			current_source: native.current_source,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			sequence: NodeId::from_identity(self.sequence).unwrap_or(NodeId::INVALID),
			multicam: NodeId::from_identity(self.multicam).unwrap_or(NodeId::INVALID),
			clip: NodeId::from_identity(self.clip).unwrap_or(NodeId::INVALID),
			source_count: self.source_count,
			current_source: self.current_source,
		}
	}
}

/// Resolves a clip's multicam state; false when it is not multicam-enabled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_state(
	session: *const EditorSessionHandle,
	clip: u64,
	out: *mut OakEditorMulticamState,
) -> bool {
	let (Some(session), Some(clip), Some(out)) = (
		unsafe { session.as_ref() },
		NodeId::from_identity(clip),
		unsafe { out.as_mut() },
	) else {
		return false;
	};
	let project = session.project();
	let Some(state) = ops::multicam_state(&project, clip) else {
		return false;
	};
	*out = OakEditorMulticamState::from_native(&state);
	true
}

/// Returns the first sequence feeding a clip, or the invalid-node sentinel.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_connected_sequence(
	session: *const EditorSessionHandle,
	clip: u64,
) -> u64 {
	let (Some(session), Some(clip)) = (unsafe { session.as_ref() }, NodeId::from_identity(clip))
	else {
		return OAK_NODE_ID_INVALID;
	};
	let project = session.project();
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	ops::clip_connected_sequence(&guard.graph, clip)
		.map(|sequence| sequence.identity())
		.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Whether a clip's direct texture source is a multicam node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_is_clip(
	session: *const EditorSessionHandle,
	clip: u64,
) -> bool {
	let (Some(session), Some(clip)) = (unsafe { session.as_ref() }, NodeId::from_identity(clip))
	else {
		return false;
	};
	let project = session.project();
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	ops::clip_is_multicam(&guard.graph, clip)
}

/// Finds the multicam clip covering `time` on a sequence's video tracks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_clip_at(
	session: *const EditorSessionHandle,
	sequence: u64,
	time: OakRational,
) -> u64 {
	let (Some(session), Some(sequence)) =
		(unsafe { session.as_ref() }, NodeId::from_identity(sequence))
	else {
		return OAK_NODE_ID_INVALID;
	};
	let project = session.project();
	ops::multicam_clip_at(&project, sequence, time.to_native())
		.map(|clip| clip.identity())
		.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Returns a multicam node's connected source sequence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_sequence(
	session: *const EditorSessionHandle,
	multicam: u64,
) -> u64 {
	let (Some(session), Some(multicam)) =
		(unsafe { session.as_ref() }, NodeId::from_identity(multicam))
	else {
		return OAK_NODE_ID_INVALID;
	};
	let project = session.project();
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	ops::multicam_sequence(&guard.graph, multicam)
		.map(|sequence| sequence.identity())
		.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Returns one angle's track inside the connected source sequence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_multicam_source_track(
	session: *const EditorSessionHandle,
	multicam: u64,
	source: i32,
) -> u64 {
	let (Some(session), Some(multicam)) =
		(unsafe { session.as_ref() }, NodeId::from_identity(multicam))
	else {
		return OAK_NODE_ID_INVALID;
	};
	let project = session.project();
	ops::multicam_source_track(&project, multicam, source)
		.map(|track| track.identity())
		.unwrap_or(OAK_NODE_ID_INVALID)
}

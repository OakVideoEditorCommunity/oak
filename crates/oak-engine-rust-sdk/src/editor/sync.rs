//! Safe synchronization metadata for timeline clips.

use std::ffi::c_void;

use crate::node::NodeId;
use crate::codec::string_out;
use crate::types::Rational;

use super::EditorSession;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_clip_sync_info(session: *const c_void, clip: u64, out: *mut ClipSyncInfo)
	-> bool;
	fn oak_editor_clip_media_filename(
		session: *const c_void,
		clip: u64,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
}

/// Timeline/media timing and optional source timecode for one block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipSyncInfo {
	/// The `timeline_in` field.
	pub timeline_in: Rational,
	/// The `timeline_out` field.
	pub timeline_out: Rational,
	/// The `media_in` field.
	pub media_in: Rational,
	/// The `source_start_time` field.
	pub source_start_time: Rational,
	/// False means `source_start_time` is unset and must not be used to align.
	pub has_source_start_time: bool,
	/// Timeline length in seconds, matching the waveform media-range convention.
	pub media_length_seconds: f64,
}

impl EditorSession {
	/// Resolves a block and its upstream footage into an owned sync DTO.
	pub fn clip_sync_info(&self, clip: NodeId) -> Option<ClipSyncInfo> {
		let mut info = ClipSyncInfo::default();
		unsafe { oak_editor_clip_sync_info(self.ptr, clip.raw(), &mut info) }.then_some(info)
	}

	/// Returns the first upstream footage filename without exposing the graph.
	pub fn clip_media_filename(&self, clip: NodeId) -> Option<String> {
		string_out(|buf, len| unsafe {
			oak_editor_clip_media_filename(self.ptr, clip.raw(), buf, len)
		})
	}
}

#[cfg(test)]
mod tests {
	use crate::edit::EditCommand;
	use crate::editor::{EditorSession, TrackKind};

	#[test]
	fn clip_sync_info_resolves_range_and_optional_source_time() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		session.create_sequence("Sync").expect("sequence");
		let media = crate::test_support::make_test_clip("sync_info");
		let footage = session.project().import_footage(&media).expect("footage");
		let clip = session
			.execute(EditCommand::PlaceFootageClip {
				footage,
				kind: TrackKind::Video,
				track_index: 0,
				in_ts: 0,
				out_ts: 30,
				media_in_ts: 0,
			})
			.expect("place")
			.primary
			.expect("clip");
		let info = session.clip_sync_info(clip).expect("sync info");
		assert_eq!(info.timeline_in, crate::types::Rational::new(0, 1));
		assert!(info.media_length_seconds > 0.0);
		assert_eq!(
			info.media_length_seconds,
			(info.timeline_out - info.timeline_in).to_f64()
		);
		assert!(!info.has_source_start_time);
		assert!(
			session
				.clip_sync_info(crate::node::NodeId::INVALID)
				.is_none()
		);
		let _ = std::fs::remove_file(media);
	}
}

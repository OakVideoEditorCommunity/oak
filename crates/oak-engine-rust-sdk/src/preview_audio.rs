//! Owned preview-audio buffers and level analysis.

use std::ffi::c_void;

use crate::audio::LevelStats;
use crate::types::TimeRange;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_audio_buffer_add_ref(audio: *const c_void);
	fn oak_audio_buffer_release(audio: *const c_void);
	fn oak_audio_buffer_info(audio: *const c_void, out: *mut AudioBlockInfo) -> bool;
	fn oak_audio_buffer_copy_samples(audio: *const c_void, dst: *mut f32, capacity: usize)
	-> usize;
}

/// Timing, output layout and interleaved-element count of an audio block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioBlockInfo {
	/// The `range` field.
	pub range: TimeRange,
	/// The `sample_rate` field.
	pub sample_rate: i32,
	/// The `channel_layout` field.
	pub channel_layout: u64,
	/// The `channel_count` field.
	pub channel_count: i32,
	/// Number of F32 values (`frames * channels`), not frame count.
	pub sample_count: usize,
}

/// Reference-counted immutable F32 audio block.
pub struct AudioBlock {
	ptr: *const c_void,
}

// SAFETY: the engine buffer is reference counted and immutable.
unsafe impl Send for AudioBlock {}
// SAFETY: shared accessors only copy samples/metadata.
unsafe impl Sync for AudioBlock {}

impl AudioBlock {
	pub(crate) fn from_raw(ptr: *const c_void) -> Option<Self> {
		(!ptr.is_null()).then_some(Self { ptr })
	}

	/// Returns timing and output-layout metadata.
	pub fn info(&self) -> AudioBlockInfo {
		let mut info = AudioBlockInfo::default();
		unsafe { oak_audio_buffer_info(self.ptr, &mut info) }
			.then_some(info)
			.unwrap_or_default()
	}

	/// Copies the interleaved F32 samples into an owned vector.
	pub fn samples(&self) -> Vec<f32> {
		let info = self.info();
		let mut samples = vec![0.0; info.sample_count];
		let written =
			unsafe { oak_audio_buffer_copy_samples(self.ptr, samples.as_mut_ptr(), samples.len()) };
		if written == samples.len() {
			samples
		} else {
			Vec::new()
		}
	}

	/// Number of sample frames per channel.
	pub fn frame_count(&self) -> usize {
		let info = self.info();
		info.sample_count / info.channel_count.max(1) as usize
	}

	/// Computes per-channel and aggregate levels independently of output-device
	/// playback state. Analysis runs on the caller's owned copy.
	pub fn levels(&self) -> LevelStats {
		let info = self.info();
		let samples = self.samples();
		let channels = info.channel_count.max(1) as usize;
		let mut planar = vec![Vec::new(); channels];
		for (index, sample) in samples.into_iter().enumerate() {
			planar[index % channels].push(sample);
		}
		let lanes: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
		crate::audio::analyze_samples(&lanes)
	}
}

impl Clone for AudioBlock {
	fn clone(&self) -> Self {
		unsafe { oak_audio_buffer_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for AudioBlock {
	fn drop(&mut self) {
		unsafe { oak_audio_buffer_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	use crate::editor::EditorSession;

	#[test]
	fn source_program_audio_blocks_levels_and_muted_tracks() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let sequence = session.create_sequence("Audio").expect("sequence");
		let media = crate::test_support::make_test_clip_with_audio("preview_audio");
		let footage = session.project().import_footage(&media).expect("footage");
		session
			.execute(crate::edit::EditCommand::PlaceFootageClip {
				footage,
				kind: crate::editor::TrackKind::Audio,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
				media_in_ts: 0,
			})
			.expect("place audio");

		let source_session =
			EditorSession::from_project(session.project()).expect("source session");
		let program_session =
			EditorSession::from_project(session.project()).expect("program session");
		let source = crate::preview::PreviewSession::source(source_session, footage, (16, 16))
			.expect("source preview");
		let mut program =
			crate::preview::PreviewSession::program(program_session, sequence, (16, 16))
				.expect("program preview");
		let source_request = source.request_audio(0.25).expect("source audio request");
		assert_eq!(source_request.generation(), source.generation());
		assert!(source_request.wait(), "source audio failed");
		let source_block = source_request.take().expect("source audio").block;
		assert_eq!(source_block.info().sample_rate, 48_000);
		assert_eq!(source_block.info().channel_count, 2);
		assert_eq!(source_block.frame_count(), 12_000);
		assert!(source_block.levels().max_peak_linear > 0.1);

		let program_request = program.request_audio(0.25).expect("program audio request");
		assert!(program_request.wait(), "program audio failed");
		let program_block = program_request.take().expect("program audio").block;
		assert_eq!(source_block.samples(), program_block.samples());
		assert!(program_block.levels().max_peak_linear > 0.1);
		if program.start_audio_output().is_ok() {
			program
				.push_audio_output(&program_block)
				.expect("push audio");
			program.play().expect("play with audio clock");
			std::thread::sleep(std::time::Duration::from_millis(20));
			program.tick(0.02).expect("audio clock tick");
			let _ = crate::preview::PreviewSession::audio_output_seconds();
			let _ = crate::preview::PreviewSession::audio_output_levels();
			program.pause().expect("pause output");
			program.stop_audio_output().expect("stop output");
		} else {
			println!("SKIP: no audio output device in this environment");
		}

		let snapshot = session.snapshot();
		let track = snapshot
			.sequences
			.iter()
			.find(|item| item.id == sequence)
			.and_then(|item| {
				item.tracks
					.iter()
					.find(|track| track.kind == Some(crate::editor::TrackKind::Audio))
			})
			.expect("audio track")
			.id;
		session
			.execute(crate::edit::EditCommand::SetTrackMuted { track, muted: true })
			.expect("mute");
		let muted_session = EditorSession::from_project(session.project()).expect("muted session");
		let muted = crate::preview::PreviewSession::program(muted_session, sequence, (16, 16))
			.expect("muted preview");
		let request = muted.request_audio(0.25).expect("muted request");
		assert!(request.wait());
		let block = request.take().expect("muted block").block;
		assert!(block.samples().iter().all(|sample| *sample == 0.0));
		assert!(block.levels().silence);
		let _ = std::fs::remove_file(media);
	}
}

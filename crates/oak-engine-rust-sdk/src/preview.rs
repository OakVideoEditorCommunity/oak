//! Source/program preview transport and owned asynchronous frame requests.
//!
//! A `PreviewSession` owns its editor session and viewer identity. Transport
//! state is local to the preview; program playhead changes still pass through
//! the normal session command path. Frame work always runs on engine threads.

use crate::editor::EditorSession;
use crate::node::{NodeId, NodeKind};
use crate::task::{Task, TaskCancellation, TaskState};
use crate::types::Rational;
use crate::types::TimeRange;

/// One timestamped audio-block request.
pub struct PreviewAudioRequest {
	task: Task,
	range: TimeRange,
	generation: u64,
}

/// A completed audio block tagged with its submission generation.
pub struct PreviewAudio {
	/// The `generation` field.
	pub generation: u64,
	/// The `block` field.
	pub block: crate::preview_audio::AudioBlock,
}

impl PreviewAudioRequest {
	/// The requested time range captured at submission.
	pub fn range(&self) -> TimeRange {
		self.range
	}

	/// The preview generation captured at submission.
	pub fn generation(&self) -> u64 {
		self.generation
	}

	/// Pollable engine task state.
	pub fn state(&self) -> TaskState {
		self.task.state()
	}

	/// Independent cooperative cancellation handle.
	pub fn cancellation(&self) -> TaskCancellation {
		self.task.cancellation()
	}

	/// Waits for the background mixer.
	pub fn wait(&self) -> bool {
		self.task.wait()
	}

	/// Transfers the completed audio block exactly once.
	pub fn take(self) -> Option<PreviewAudio> {
		let generation = self.generation;
		self.task
			.take_audio()
			.map(|block| PreviewAudio { generation, block })
	}
}

/// The viewer domain targeted by a preview session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
	/// The source monitor.
	Source,
	/// The program monitor.
	Program,
}

/// One timestamped frame request. The captured time prevents a delayed result
/// from being mistaken for a newer seek/playhead position.
pub struct PreviewFrameRequest {
	task: Task,
	time: Rational,
	generation: u64,
}

impl PreviewFrameRequest {
	/// The frame time captured when this request was submitted.
	pub fn time(&self) -> Rational {
		self.time
	}

	/// The preview generation captured at submission.
	pub fn generation(&self) -> u64 {
		self.generation
	}

	/// Pollable engine task state.
	pub fn state(&self) -> TaskState {
		self.task.state()
	}

	/// Independent cooperative cancellation handle.
	pub fn cancellation(&self) -> TaskCancellation {
		self.task.cancellation()
	}

	/// Waits on the engine worker, never starts rendering on the caller thread.
	pub fn wait(&self) -> bool {
		self.task.wait()
	}

	/// Transfers the completed owned frame exactly once.
	pub fn take(self) -> Option<PreviewFrame> {
		let time = self.time;
		let generation = self.generation;
		self.task.take_video().map(|frame| PreviewFrame {
			time,
			generation,
			frame,
		})
	}
}

/// An owned, timestamped F32 RGBA preview frame.
pub struct PreviewFrame {
	/// The `time` field.
	pub time: Rational,
	/// The `generation` field.
	pub generation: u64,
	/// The `frame` field.
	pub frame: crate::preview_video::VideoFrame,
}

/// One source/program transport with asynchronous frame production.
pub struct PreviewSession {
	session: EditorSession,
	kind: PreviewKind,
	viewer: NodeId,
	frame_rate: Rational,
	size: (u32, u32),
	time: Rational,
	playing: bool,
	rate: f64,
	generation: u64,
	/// `(preview time, device seconds)` captured when audio output was anchored.
	audio_anchor: Option<(Rational, f64)>,
	audio_output_enabled: bool,
}

impl PreviewSession {
	/// Creates a source preview over one video-footage node.
	pub fn source(
		session: EditorSession,
		footage: NodeId,
		size: (u32, u32),
	) -> Result<Self, String> {
		let project = session.project();
		if project.node_kind(footage) != Some(NodeKind::Footage) {
			return Err("source viewer is not footage".into());
		}
		let params = project
			.footage_video_params(footage, 0)
			.ok_or("source footage has no video stream")?;
		Self::new(
			session,
			PreviewKind::Source,
			footage,
			params.frame_rate,
			size,
		)
	}

	/// Creates a program preview over one sequence node.
	pub fn program(
		session: EditorSession,
		sequence: NodeId,
		size: (u32, u32),
	) -> Result<Self, String> {
		if session.project().node_kind(sequence) != Some(NodeKind::Sequence) {
			return Err("program viewer is not a sequence".into());
		}
		let snapshot = session.snapshot();
		let sequence = snapshot
			.sequences
			.iter()
			.find(|item| item.id == sequence)
			.ok_or("program sequence is not mounted")?;
		Self::new(
			session,
			PreviewKind::Program,
			sequence.id,
			sequence.frame_rate,
			size,
		)
	}

	fn new(
		session: EditorSession,
		kind: PreviewKind,
		viewer: NodeId,
		frame_rate: Rational,
		size: (u32, u32),
	) -> Result<Self, String> {
		if size.0 == 0 || size.1 == 0 {
			return Err("preview dimensions must be positive".into());
		}
		if frame_rate.is_null() || frame_rate.num <= 0 || frame_rate.den <= 0 {
			return Err("preview has no valid frame rate".into());
		}
		let time = if kind == PreviewKind::Program {
			session.project().sequence_playhead(viewer)
		} else {
			Rational::new(0, 1)
		};
		Ok(Self {
			session,
			kind,
			viewer,
			frame_rate,
			size,
			time,
			playing: false,
			rate: 1.0,
			generation: 0,
			audio_anchor: None,
			audio_output_enabled: false,
		})
	}

	/// Current preview time in seconds.
	pub fn time(&self) -> Rational {
		self.time
	}

	/// Whether transport is playing.
	pub fn is_playing(&self) -> bool {
		self.playing
	}

	/// Current playback-rate multiplier.
	pub fn rate(&self) -> f64 {
		self.rate
	}

	/// Current content/cache generation.
	pub fn generation(&self) -> u64 {
		self.generation
	}

	/// Invalidates outstanding frame/audio requests without changing transport.
	pub fn invalidate(&mut self) {
		self.generation = self.generation.wrapping_add(1);
	}

	/// Starts audio output and anchors transport to the device's consumption
	/// clock. The next `tick` follows that clock instead of wall time.
	pub fn start_audio_output(&mut self) -> Result<(), String> {
		crate::audio::create_instance().map_err(|error| error.to_string())?;
		crate::audio::clear_buffered_output().map_err(|error| error.to_string())?;
		crate::audio::reset_output_clock().map_err(|error| error.to_string())?;
		self.audio_output_enabled = true;
		self.audio_anchor = Some((self.time, crate::audio::seconds().unwrap_or(0.0)));
		Ok(())
	}

	/// Stops audio output and returns transport to the external-clock path.
	pub fn stop_audio_output(&mut self) -> Result<(), String> {
		self.audio_anchor = None;
		self.audio_output_enabled = false;
		crate::audio::stop_output().map_err(|error| error.to_string())
	}

	/// Seconds consumed by the output device, if a stream has run.
	pub fn audio_output_seconds() -> Option<f64> {
		crate::audio::seconds()
	}

	/// Current per-channel output peaks, if the device is running.
	pub fn audio_output_levels() -> Option<Vec<f32>> {
		let mut peaks = [0.0f32; 64];
		let count = crate::audio::output_levels(&mut peaks)?;
		Some(peaks[..count.min(peaks.len())].to_vec())
	}

	/// Pushes one owned audio block to the output device as interleaved F32.
	pub fn push_audio_output(
		&self,
		block: &crate::preview_audio::AudioBlock,
	) -> Result<(), String> {
		let info = block.info();
		let samples = block.samples();
		let mut bytes = Vec::with_capacity(samples.len() * 4);
		for sample in samples {
			bytes.extend_from_slice(&sample.to_le_bytes());
		}
		crate::audio::push_to_output(
			crate::audio::AudioParams::new(
				info.sample_rate,
				info.channel_layout,
				crate::audio::SampleFormat::F32,
			),
			&bytes,
		)
		.map_err(|error| error.to_string())
	}

	/// Borrows the underlying editor session.
	pub fn session(&self) -> &EditorSession {
		&self.session
	}

	/// Transfers the underlying editor session to a host integration.
	pub fn into_session(self) -> EditorSession {
		let mut this = std::mem::ManuallyDrop::new(self);
		if this.audio_output_enabled {
			let _ = crate::audio::stop_output();
			this.audio_output_enabled = false;
			this.audio_anchor = None;
		}
		// SAFETY: `self` is pinned only by value, `session` is not referenced
		// afterward, and ManuallyDrop prevents a second drop of that field.
		unsafe { std::ptr::read(&this.session) }
	}

	fn sync_playhead(&self) -> Result<(), String> {
		if self.kind == PreviewKind::Program {
			self.session
				.execute(crate::edit::EditCommand::SetSequencePlayhead {
					sequence: self.viewer,
					time: self.time,
				})
				.map(|_| ())?;
		}
		Ok(())
	}

	/// Seeks to an absolute nonnegative time and pauses transport.
	pub fn seek(&mut self, time: Rational) -> Result<(), String> {
		if time.is_nan() || time.to_f64() < 0.0 {
			return Err("preview time must be nonnegative".into());
		}
		self.playing = false;
		self.time = time;
		self.generation = self.generation.wrapping_add(1);
		if self.audio_output_enabled {
			crate::audio::clear_buffered_output().map_err(|error| error.to_string())?;
			crate::audio::reset_output_clock().map_err(|error| error.to_string())?;
			self.audio_anchor = Some((time, crate::audio::seconds().unwrap_or(0.0)));
		}
		self.sync_playhead()
	}

	/// Starts or resumes transport from the current time.
	pub fn play(&mut self) -> Result<(), String> {
		self.playing = true;
		if self.audio_output_enabled {
			crate::audio::reset_output_clock().map_err(|error| error.to_string())?;
			self.audio_anchor = Some((self.time, crate::audio::seconds().unwrap_or(0.0)));
		}
		self.sync_playhead()
	}

	/// Pauses without changing the current time.
	pub fn pause(&mut self) -> Result<(), String> {
		self.playing = false;
		if self.audio_output_enabled {
			self.audio_anchor = None;
			crate::audio::stop_output().map_err(|error| error.to_string())?;
		}
		self.sync_playhead()
	}

	/// Changes the playback-rate multiplier.
	pub fn set_rate(&mut self, rate: f64) -> Result<(), String> {
		if !rate.is_finite() || rate <= 0.0 {
			return Err("playback rate must be finite and positive".into());
		}
		self.rate = rate;
		if self.audio_anchor.is_some() {
			self.audio_anchor = Some((self.time, crate::audio::seconds().unwrap_or(0.0)));
		}
		Ok(())
	}

	/// Steps by whole viewer frames and pauses. Negative steps clamp at zero.
	pub fn step(&mut self, frames: i32) -> Result<(), String> {
		let duration = Rational::new(
			i64::from(self.frame_rate.den) * i64::from(frames),
			i64::from(self.frame_rate.num),
		);
		self.seek((self.time + duration).max(Rational::new(0, 1)))
	}

	/// Advances transport by an external clock interval.
	///
	/// Hosts may drive this from their UI tick or a dedicated playback clock;
	/// rendering remains asynchronous and is requested separately.
	pub fn tick(&mut self, elapsed_seconds: f64) -> Result<Rational, String> {
		if !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
			return Err("clock interval must be finite and nonnegative".into());
		}
		if self.playing {
			if let Some((anchor_time, anchor_seconds)) = self.audio_anchor {
				if let Some(seconds) = crate::audio::seconds() {
					let elapsed = (seconds - anchor_seconds).max(0.0);
					self.time = anchor_time + Rational::from_f64(elapsed * self.rate);
				} else {
					self.time = self.time + Rational::from_f64(elapsed_seconds * self.rate);
				}
			} else {
				self.time = self.time + Rational::from_f64(elapsed_seconds * self.rate);
			}
			self.sync_playhead()?;
		}
		Ok(self.time)
	}

	/// Requests the current frame on an engine worker with its timestamp captured.
	pub fn request_frame(&self) -> Option<PreviewFrameRequest> {
		let task = crate::task::render_video(
			&self.session.project(),
			self.viewer,
			self.time,
			self.size,
			crate::preview_video::VideoPixelFormat::F32Rgba,
		)?;
		Some(PreviewFrameRequest {
			task,
			time: self.time,
			generation: self.generation,
		})
	}

	/// Requests an audio block beginning at the current preview time. The block
	/// uses 48 kHz stereo and the same captured range for result correlation.
	pub fn request_audio(&self, duration_seconds: f64) -> Option<PreviewAudioRequest> {
		if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
			return None;
		}
		let range = TimeRange::new(self.time, self.time + Rational::from_f64(duration_seconds));
		let task =
			crate::task::render_audio(&self.session.project(), self.viewer, range, 48_000, 0x3)?;
		Some(PreviewAudioRequest {
			task,
			range,
			generation: self.generation,
		})
	}
}

impl Drop for PreviewSession {
	fn drop(&mut self) {
		if self.audio_output_enabled {
			self.audio_anchor = None;
			let _ = crate::audio::stop_output();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn source_program_transport_and_timestamped_async_frames() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let sequence = session.create_sequence("Program").expect("sequence");
		let media = crate::test_support::make_test_clip("preview_transport");
		let footage = session.project().import_footage(&media).expect("footage");
		session
			.execute(crate::edit::EditCommand::PlaceFootageClip {
				footage,
				kind: crate::editor::TrackKind::Video,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
				media_in_ts: 0,
			})
			.expect("place clip");

		let source_session =
			EditorSession::from_project(session.project()).expect("source session");
		let program_session =
			EditorSession::from_project(session.project()).expect("program session");
		let mut source = PreviewSession::source(source_session, footage, (16, 12)).expect("source");
		let mut program =
			PreviewSession::program(program_session, sequence, (16, 12)).expect("program");

		source.step(1).expect("source step");
		assert_eq!(source.time(), Rational::new(1, 10));
		source.seek(Rational::new(0, 1)).expect("source seek");
		source.play().expect("source play");
		source.set_rate(2.0).expect("source rate");
		source.tick(0.25).expect("source clock");
		assert_eq!(source.time(), Rational::new(1, 2));
		source.pause().expect("source pause");
		source.tick(1.0).expect("paused clock");
		assert_eq!(source.time(), Rational::new(1, 2));
		source.step(-100).expect("step clamps at zero");
		assert_eq!(source.time(), Rational::new(0, 1));

		program.seek(Rational::new(0, 1)).expect("program seek");
		program.step(-1).expect("program step");
		assert_eq!(program.time(), Rational::new(0, 1));
		program.play().expect("program play");
		program.tick(0.04).expect("program clock");
		let program_time = program.time();
		assert!(program_time.to_f64() > 0.0);
		assert_eq!(
			program.session().project().sequence_playhead(sequence),
			program_time
		);
		program.pause().expect("program pause");

		let source_frame = source.request_frame().expect("source request");
		assert_eq!(source_frame.time(), Rational::new(0, 1));
		assert!(source_frame.wait());
		let source_frame = source_frame.take().expect("source frame");
		assert_eq!(source_frame.frame.f32_samples().len(), 16 * 12 * 4);
		let stale = source.request_frame().expect("stale request");
		let stale_generation = stale.generation();
		source.invalidate();
		assert!(source.generation() > stale_generation);
		assert!(stale.wait());
		let stale = stale.take().expect("stale frame");
		assert_eq!(stale.generation, stale_generation);
		assert_ne!(stale.generation, source.generation());
		let program_frame = program.request_frame().expect("program request");
		assert_eq!(program_frame.time(), program_time);
		assert!(program_frame.wait());
		let program_frame = program_frame.take().expect("program frame");
		assert_eq!(program_frame.frame.f32_samples().len(), 16 * 12 * 4);
		let _ = std::fs::remove_file(media);
	}
}

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

//! Pull-style sample buffer (`olive::PreviewAudioDevice`). The audio backend
//! (PortAudio, in `engine::audio::AudioManager`) pulls samples through
//! [`read`](PreviewAudioDevice::read) from its stream callback; the render
//! side pushes samples through [`write`](PreviewAudioDevice::write). The
//! callback-driven pull semantics are unchanged from the former QIODevice.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use crate::params::AudioParams;

/// A thread-safe ring of raw interleaved sample bytes feeding the audio
/// output callback.
///
/// `// CPP-PARITY: src/audio/src/previewaudiodevice.h`
/// (`PreviewAudioDevice`).
pub struct PreviewAudioDevice {
	lock: Mutex<PreviewAudioDeviceInner>,
	/// Frames consumed by the output callback (playback clock, includes
	/// underrun zero-fill).
	output_frames_consumed: AtomicI64,
	/// Frames the output callback had to zero-fill (buffer was empty).
	/// Kept separate from the playback clock so the engine can detect
	/// underruns and resync — zero-fill advances the playback clock past
	/// the queued content, so everything queued afterwards would play
	/// late (the classic audio-behind-video drift).
	underrun_frames: AtomicI64,
}

struct PreviewAudioDeviceInner {
	/// Queued sample bytes.
	buffer: Vec<u8>,
	/// Bytes per output frame (0 = unknown until params are set).
	bytes_per_frame: i32,
	/// Bytes that trigger a notify callback when crossed.
	notify_interval: i64,
	/// Bytes read so far in the current interval.
	bytes_read: i64,
	/// Callback fired when a notify interval boundary is crossed.
	notify_callback: Option<Box<dyn FnMut() + Send>>,
}

impl PreviewAudioDevice {
	/// Create an empty device with unknown frame size.
	pub fn new() -> PreviewAudioDevice {
		PreviewAudioDevice {
			lock: Mutex::new(PreviewAudioDeviceInner {
				buffer: Vec::new(),
				bytes_per_frame: 0,
				notify_interval: 0,
				bytes_read: 0,
				notify_callback: None,
			}),
			output_frames_consumed: AtomicI64::new(0),
			underrun_frames: AtomicI64::new(0),
		}
	}

	/// Read up to `data.len()` bytes from the queued buffer. Called from the
	/// audio output callback; returns bytes actually copied (0 on underrun).
	///
	/// `// CPP-PARITY: src/audio/src/previewaudiodevice.cpp:36`
	/// (`PreviewAudioDevice::read`): the notify callback fires AFTER the
	/// internal lock is released, and only when an interval boundary is
	/// crossed by this read.
	pub fn read(&self, data: &mut [u8]) -> i64 {
		let mut notify = false;
		let copy_length;
		{
			let mut inner = self.lock.lock().unwrap();
			copy_length = (data.len() as i64).min(inner.buffer.len() as i64);

			if copy_length > 0 {
				let new_bytes_read = inner.bytes_read + copy_length;

				if inner.notify_interval > 0
					&& inner.notify_callback.is_some()
					&& (inner.bytes_read / inner.notify_interval)
						!= (new_bytes_read / inner.notify_interval)
				{
					notify = true;
				}

				inner.bytes_read = new_bytes_read;

				data[..copy_length as usize].copy_from_slice(&inner.buffer[..copy_length as usize]);
				inner.buffer.drain(..copy_length as usize);
			}
		}

		// Fired outside the lock (see set_notify_callback())
		if notify {
			let mut cb = self.lock.lock().unwrap().notify_callback.take();
			if let Some(c) = cb.as_mut() {
				c();
			}
			let mut inner = self.lock.lock().unwrap();
			if inner.notify_callback.is_none() {
				inner.notify_callback = cb;
			}
		}

		copy_length
	}

	/// Append `data` to the queued buffer.
	///
	/// `// CPP-PARITY: src/audio/src/previewaudiodevice.cpp:69`
	/// (`PreviewAudioDevice::write`).
	pub fn write(&self, data: &[u8]) -> i64 {
		let mut inner = self.lock.lock().unwrap();
		inner.buffer.extend_from_slice(data);
		data.len() as i64
	}

	/// Derive the frame size from the audio format (bytes per sample per
	/// channel * channel count).
	///
	/// `// CPP-PARITY: src/audio/src/previewaudiodevice.cpp:31`
	/// (`PreviewAudioDevice::set_params` = `samples_to_bytes(1)`).
	pub fn set_params(&self, params: AudioParams) {
		self.set_bytes_per_frame(params.samples_to_bytes(1) as i32);
	}

	/// Current bytes per frame (0 = unknown).
	pub fn bytes_per_frame(&self) -> i32 {
		self.lock.lock().unwrap().bytes_per_frame
	}

	/// Override the frame size directly.
	pub fn set_bytes_per_frame(&self, bytes: i32) {
		self.lock.lock().unwrap().bytes_per_frame = bytes;
	}

	/// Set the notify interval in bytes.
	pub fn set_notify_interval(&self, interval: i64) {
		self.lock.lock().unwrap().notify_interval = interval;
	}

	/// Install the callback fired when a notify interval boundary is crossed.
	///
	/// Invoked from [`read`](PreviewAudioDevice::read) (the audio output
	/// callback thread) after the internal lock is released. Must be
	/// thread-safe and must not call back into this device.
	pub fn set_notify_callback<F>(&mut self, callback: F)
	where
		F: FnMut() + Send + 'static,
	{
		self.lock.lock().unwrap().notify_callback = Some(Box::new(callback));
	}

	/// Drop all queued bytes and reset the byte counters.
	///
	/// `// CPP-PARITY: src/audio/src/previewaudiodevice.cpp:77`
	/// (`PreviewAudioDevice::clear`): also resets the consumed-frames clock.
	pub fn clear(&self) {
		let mut inner = self.lock.lock().unwrap();
		inner.buffer.clear();
		inner.bytes_read = 0;
		self.output_frames_consumed.store(0, Ordering::Relaxed);
	}

	/// Frames currently queued for the output callback (diagnostics).
	pub fn queued_frames(&self) -> i64 {
		let inner = self.lock.lock().unwrap();
		if inner.bytes_per_frame <= 0 {
			return 0;
		}
		inner.buffer.len() as i64 / inner.bytes_per_frame as i64
	}

	/// Drop up to `frames` frames from the FRONT of the queued buffer,
	/// returning the frames actually dropped. Underrun resync: zero-fill
	/// advanced the output clock past this much queued content, so it
	/// would all play late — trimming just the stale lead resyncs playback
	/// WITHOUT the full clear + re-render storm a `clear()` causes (the
	/// rest of the queue stays audible and the master-clock anchor stays
	/// valid, since the anchor maps the clock to sequence frames and the
	/// clock already advanced past the zero-fill).
	pub fn drop_front_frames(&self, frames: i64) -> i64 {
		if frames <= 0 {
			return 0;
		}
		let mut inner = self.lock.lock().unwrap();
		if inner.bytes_per_frame <= 0 {
			return 0;
		}
		let bytes = (frames * inner.bytes_per_frame as i64).min(inner.buffer.len() as i64) as usize;
		inner.buffer.drain(..bytes);
		bytes as i64 / inner.bytes_per_frame as i64
	}

	/// Account for frames consumed by the output callback (including
	/// underrun zero-fill).
	pub fn add_output_frames(&self, frame_count: i64) {
		self.output_frames_consumed
			.fetch_add(frame_count, Ordering::Relaxed);
	}

	/// Account for frames the output callback zero-filled (underrun).
	pub fn add_underrun_frames(&self, frame_count: i64) {
		if frame_count > 0 {
			self.underrun_frames
				.fetch_add(frame_count, Ordering::Relaxed);
		}
	}

	/// Read and reset the underrun counter (the engine polls it per tick).
	pub fn take_underrun_frames(&self) -> i64 {
		self.underrun_frames.swap(0, Ordering::Relaxed)
	}

	/// Frames consumed by the output callback.
	pub fn output_frames_consumed(&self) -> i64 {
		self.output_frames_consumed.load(Ordering::Relaxed)
	}

	/// Reset the consumed-frames counter.
	pub fn reset_output_frames(&self) {
		self.output_frames_consumed.store(0, Ordering::Relaxed);
	}
}

impl Default for PreviewAudioDevice {
	fn default() -> PreviewAudioDevice {
		PreviewAudioDevice::new()
	}
}

impl PreviewAudioDevice {
	/// Copy up to `data.len()` bytes from the TAIL of the queued buffer
	/// without consuming them (the level meter peeks at what is about to
	/// play; the oldest bytes are irrelevant for that). Returns the byte
	/// count copied.
	pub fn peek_tail(&self, data: &mut [u8]) -> i64 {
		let inner = self.lock.lock().unwrap();
		let copy = (data.len() as i64).min(inner.buffer.len() as i64);
		if copy > 0 {
			let start = inner.buffer.len() - copy as usize;
			data[..copy as usize].copy_from_slice(&inner.buffer[start..]);
		}
		copy
	}
}

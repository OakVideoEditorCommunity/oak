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

//! AudioManager contract tests (manager.rs), calling the public Rust API.
//! The manager is a process-wide singleton, so every test holds the shared
//! `MANAGER_LOCK`.

mod common;

use common::lock_manager;
use oak_audio::error::{Error, OAKAUDIO_E_INVALID};
use oak_audio::manager::instance;
use oak_audio::params::{AudioParams, SampleFormat};
use oak_codec::encodingparams::EncodingParams;

/// Audio params for the tests: stereo f32 at 48 kHz.
fn stereo() -> AudioParams {
	AudioParams {
		sample_rate: 48000,
		channel_layout: 0x3,
		format: SampleFormat::F32,
	}
}

/// A WAV recording config (format 7 = WAV).
fn wav_params(filename: &str) -> EncodingParams {
	let mut params = EncodingParams::default();
	params.format = 7;
	params.audio_enabled = 1;
	params.audio_codec = 13; // PCM_S16LE
	params.audio_sample_rate = 48000;
	params.audio_channel_layout = 0x3;
	params.audio_sample_format = SampleFormat::S16;
	params.audio_bit_rate = 128000;
	let bytes = filename.as_bytes();
	params.filename[..bytes.len()].copy_from_slice(bytes);
	params
}

/// create_instance/destroy_instance toggle the singleton; instance()
/// returns a live guard between them and None after destroy.
#[test]
fn singleton_lifecycle() {
	let _guard = lock_manager();
	oak_audio::manager::ManagerInner::destroy_instance();
	assert!(instance().is_none());

	oak_audio::manager::ManagerInner::create_instance().unwrap();
	assert!(instance().is_some());

	oak_audio::manager::ManagerInner::destroy_instance();
	assert!(instance().is_none());
	oak_audio::manager::ManagerInner::create_instance().unwrap();
	assert!(instance().is_some());
}

/// A fresh singleton for a test: destroy resets the playback state
/// (`output_started`, buffered params) that earlier tests in this binary
/// may have left behind.
fn fresh_instance() {
	oak_audio::manager::ManagerInner::destroy_instance();
	oak_audio::manager::ManagerInner::create_instance().unwrap();
}

/// push_to_output accepts raw interleaved bytes and starts the virtual
/// playback clock; without a device the push still succeeds and keeps the
/// samples buffered (unavailable devices play silence instead of failing).
///
/// The clock reads 0.0 after the push starts; a real audio device may have
/// consumed a few frames by the time the assertion runs, so the bound is
/// `>= 0` rather than exact (the callback-advances-clock behavior is pinned
/// by manager.rs' own unit test).
#[test]
fn push_output_starts_clock() {
	let _guard = lock_manager();
	fresh_instance();
	let mut m = instance().unwrap();

	// No stream yet: seconds() reports -1.
	let mut secs = 0.0f64;
	m.seconds(&mut secs).unwrap();
	assert_eq!(secs, -1.0);

	// With no explicit device the push still succeeds (M12 P1).
	m.set_output_device(-1).unwrap();
	let samples = vec![0u8; 480 * 2 * 4];
	m.push_to_output(stereo(), &samples, &mut [0u8; 64])
		.unwrap();

	// After selecting a device the push succeeds and the clock starts at 0.
	m.set_output_device(0).unwrap();
	m.push_to_output(stereo(), &samples, &mut [0u8; 64])
		.unwrap();
	m.seconds(&mut secs).unwrap();
	assert!(secs >= 0.0, "clock started (got {secs})");
}

/// set/get output & input device: getters report a device, setters persist
/// it; hard_reset keeps the device indices (it only stops the stream and
/// clears buffers).
#[test]
fn device_selection_roundtrip() {
	let _guard = lock_manager();
	fresh_instance();
	let mut m = instance().unwrap();

	m.set_output_device(42).unwrap();
	assert_eq!(m.get_output_device().unwrap(), 42);
	m.set_input_device(7).unwrap();
	assert_eq!(m.get_input_device().unwrap(), 7);

	m.hard_reset().unwrap();
	assert_eq!(m.get_output_device().unwrap(), 42);
	assert_eq!(m.get_input_device().unwrap(), 7);

	// The stream stopped, so the clock is back at -1.
	let mut secs = 0.0f64;
	m.seconds(&mut secs).unwrap();
	assert_eq!(secs, -1.0);
}

/// set_output_notify_interval stores the interval (negative is rejected);
/// clear_buffered_output drops queued bytes, stop_output halts the stream,
/// and reset_output_clock restarts the counter.
#[test]
fn output_control_flags() {
	let _guard = lock_manager();
	fresh_instance();
	let mut m = instance().unwrap();

	m.set_output_notify_interval(1024).unwrap();
	let err = m.set_output_notify_interval(-1).unwrap_err();
	assert_eq!(
		err.downcast_ref::<oak_audio::error::Error>()
			.map(|e| e.code()),
		Some(OAKAUDIO_E_INVALID)
	);
	m.clear_buffered_output().unwrap();
	m.reset_output_clock().unwrap();

	// Push starts the stream, then stop_output halts it (clock -> -1).
	m.set_output_device(0).unwrap();
	let samples = vec![0u8; 480 * 2 * 4];
	m.push_to_output(stereo(), &samples, &mut [0u8; 64])
		.unwrap();
	m.stop_output().unwrap();
	let mut secs = 1.0f64;
	m.seconds(&mut secs).unwrap();
	assert_eq!(secs, -1.0);
}

/// start_recording validates its state: with no input device it fails with
/// a reason; with a device the encoder open either succeeds (OAKAUDIO_OK,
/// environment-dependent) or reports the encoder's diagnostic — both are
/// correct manager behavior, so the test pins the manager's own handling
/// only.
#[test]
fn recording_start_stop() {
	let _guard = lock_manager();
	fresh_instance();
	let mut m = instance().unwrap();

	// No input device -> explainable failure, no crash.
	m.set_input_device(-1).unwrap();
	assert!(m
		.start_recording(&wav_params("unused.wav"), &mut [0u8; 64])
		.is_err());

	m.set_input_device(0).unwrap();
	let path = std::env::temp_dir().join(format!("oakaudio_rec_{}.wav", std::process::id()));
	let filename = path.to_str().unwrap();
	let mut err = vec![0u8; 256];
	match m.start_recording(&wav_params(filename), &mut err) {
		Ok(()) => {
			assert!(m.stop_recording().is_ok());
		}
		Err(e) => {
			assert!(!e.to_string().is_empty(), "failure must carry a reason");
		}
	}
	let _ = std::fs::remove_file(path);
}

/// Config defaults are read from the (empty) oakcommon store: buffer size
/// falls back to 0 and device names are absent.
#[test]
fn config_defaults() {
	assert_eq!(oak_audio::config::output_buffer_size(), 0);
	assert!(
		oak_audio::config::device_name(true).is_err(),
		"no configured output device"
	);
	assert!(
		oak_audio::config::device_name(false).is_err(),
		"no configured input device"
	);
}

/// PreviewAudioDevice pull-side plumbing (read/notify callback/clock).
#[test]
fn preview_device_pull_side() {
	use oak_audio::previewdevice::PreviewAudioDevice;

	let mut dev = PreviewAudioDevice::new();
	dev.set_params(AudioParams {
		sample_rate: 48000,
		channel_layout: 3,
		format: SampleFormat::F32,
	});
	assert_eq!(dev.bytes_per_frame(), 8);

	let callbacks = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
	let cb = std::sync::Arc::clone(&callbacks);
	dev.set_notify_callback(move || {
		cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	});
	dev.set_notify_interval(4);
	dev.write(&[1u8; 10]);

	// Reading 6 bytes crosses a 4-byte notify boundary.
	let mut buf = [0u8; 6];
	assert_eq!(dev.read(&mut buf), 6);
	assert!(callbacks.load(std::sync::atomic::Ordering::Relaxed) >= 1);
	assert_eq!(buf, [1u8; 6]);

	// Clock accounting.
	dev.add_output_frames(3);
	assert_eq!(dev.output_frames_consumed(), 3);
	dev.reset_output_frames();
	assert_eq!(dev.output_frames_consumed(), 0);
	dev.clear();
	assert_eq!(dev.output_frames_consumed(), 0);
}

/// output_levels: the no-output case, and a real peak readback over pushed
/// packed-F32 stereo samples (left ramps to 0.05, right to ~0.2 — the
/// per-channel linear peaks).
///
/// The push can open a real cpal stream whose callback consumes queued
/// samples concurrently, so a long ramp is pushed and the peaks are
/// asserted with a tolerance that absorbs partial consumption (the
/// analysis window is the most recent 8192 frames of the queue).
#[test]
fn output_levels_reports_buffered_peaks() {
	let _guard = lock_manager();
	fresh_instance();
	let mut m = instance().unwrap();

	// Nothing configured yet: no channels.
	assert_eq!(m.output_levels(&mut [0.0f32; 4]).unwrap(), 0);

	m.set_output_device(42).unwrap();
	m.clear_buffered_output().unwrap();

	// Push 2 seconds of packed F32 stereo ramp (format 10), stereo layout
	// 0x3. Left ramps 0 -> 0.05, right 0 -> 0.2.
	let frames = 96000usize;
	let mut packed = Vec::with_capacity(frames * 2 * 4);
	for i in 0..frames {
		let t = i as f32 / frames as f32;
		packed.extend_from_slice(&(0.05f32 * t).to_le_bytes());
		packed.extend_from_slice(&(0.2f32 * t).to_le_bytes());
	}
	m.push_to_output(stereo(), &packed, &mut [0u8; 64]).unwrap();

	let mut peaks = [0.0f32; 4];
	let n = m.output_levels(&mut peaks).unwrap();
	assert_eq!(n, 2);
	assert!(
		(peaks[0] - 0.05).abs() < 1e-3 && peaks[0] > 0.04,
		"left peak: {}",
		peaks[0]
	);
	assert!(
		(peaks[1] - 0.2).abs() < 1e-3 && peaks[1] > 0.19,
		"right peak: {}",
		peaks[1]
	);

	// A cleared buffer reports zeroed peaks over the configured channels.
	m.clear_buffered_output().unwrap();
	let mut cleared = [0.0f32; 4];
	assert_eq!(m.output_levels(&mut cleared).unwrap(), 2);
	for p in &cleared[..2] {
		assert_eq!(*p, 0.0, "cleared buffer must have silent peaks");
	}

	// The error mapping is intact.
	assert_eq!(Error::Invalid.code(), OAKAUDIO_E_INVALID);
	assert_eq!(
		Error::Failed("x".to_string()).code(),
		oak_audio::error::OAKAUDIO_E_FAILED
	);

	// Leave the singleton as we found it: the push flipped
	// `output_started`, which other tests' seconds() assertions depend on.
	m.stop_output().unwrap();
	m.clear_buffered_output().unwrap();
}

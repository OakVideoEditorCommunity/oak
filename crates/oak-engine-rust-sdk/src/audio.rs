//! Safe wrapper over the engine's audio C ABI (`oak_audio_manager_*`).
//!
//! The audio manager is a process-wide singleton inside the engine, so
//! the SDK exposes plain free functions: no handle type, no ownership
//! games — call [`create_instance`] once at startup, use the functions,
//! optionally [`destroy_instance`] at shutdown.

use std::ffi::c_void;

// The engine cdylib: cargo builds it as a path dependency into the
// dependency search path (target/<profile>/deps), so the plain name
// resolves at link time on every platform (dylib/so/dll).
#[link(name = "oak_engine")]
unsafe extern "C" {
	fn oak_audio_manager_create_instance() -> bool;
	fn oak_audio_manager_destroy_instance();
	fn oak_audio_manager_output_device_names() -> *mut c_void;
	fn oak_audio_manager_input_device_names() -> *mut c_void;
	fn oak_audio_manager_device_index_by_name(name: *const u8, len: usize, is_output: bool) -> i32;
	fn oak_audio_manager_set_output_device(device: i32) -> bool;
	fn oak_audio_manager_set_input_device(device: i32) -> bool;
	fn oak_audio_manager_output_levels(peaks: *mut f32, capacity: usize) -> i32;
	fn oak_audio_manager_seconds(seconds: *mut f64) -> bool;
	fn oak_audio_manager_clear_buffered_output() -> bool;
	fn oak_audio_manager_reset_output_clock() -> bool;
	fn oak_audio_manager_stop_output() -> bool;
	fn oak_audio_manager_take_output_underrun_frames() -> i64;
	fn oak_audio_manager_output_queued_frames() -> i64;
	fn oak_audio_manager_drop_output_frames(frames: i64) -> i64;
	fn oak_audio_manager_push_to_output(
		params: AudioParams,
		sample: *const u8,
		sample_len: usize,
		error_buf: *mut u8,
		error_buf_len: usize,
	) -> bool;
}

use crate::vecs::VecString;

/// A failed engine audio call. The C ABI carries no detail beyond the
/// failure itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the engine audio call failed")]
pub struct Error;

/// `push_to_output` failure, carrying the engine's error text when it
/// provides one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("audio push failed: {0}")]
pub struct PushError(pub String);

/// Sample formats, mirroring `oak_core::SampleFormat`'s explicit
/// discriminants (the C ABI carries them as i32).
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
	Invalid = -1,
	U8Planar = 0,
	S16Planar = 1,
	S32Planar = 2,
	S64Planar = 3,
	F32Planar = 4,
	F64Planar = 5,
	U8 = 6,
	S16 = 7,
	S32 = 8,
	S64 = 9,
	F32 = 10,
	F64 = 11,
}

/// Audio stream parameters. `repr(C)` mirror of the engine's
/// `OakAudioParams`; passed by value over the ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioParams {
	/// Sample rate in Hz.
	pub sample_rate: i32,
	/// ffmpeg-style channel layout mask (0 = unknown/unspecified).
	pub channel_layout: u64,
	/// A `SampleFormat` discriminant (kept as the raw ABI value so unknown
	/// formats round-trip instead of being rejected at the boundary).
	format: i32,
}

impl AudioParams {
	pub fn new(sample_rate: i32, channel_layout: u64, format: SampleFormat) -> Self {
		Self {
			sample_rate,
			channel_layout,
			format: format as i32,
		}
	}

	pub fn format(&self) -> SampleFormat {
		match self.format {
			0 => SampleFormat::U8Planar,
			1 => SampleFormat::S16Planar,
			2 => SampleFormat::S32Planar,
			3 => SampleFormat::S64Planar,
			4 => SampleFormat::F32Planar,
			5 => SampleFormat::F64Planar,
			6 => SampleFormat::U8,
			7 => SampleFormat::S16,
			8 => SampleFormat::S32,
			9 => SampleFormat::S64,
			10 => SampleFormat::F32,
			11 => SampleFormat::F64,
			_ => SampleFormat::Invalid,
		}
	}
}

/// Initializes the audio manager singleton. Idempotent; call once at
/// startup (without it, the calls below fail or report `None`).
pub fn create_instance() -> Result<(), Error> {
	if unsafe { oak_audio_manager_create_instance() } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Destroys the audio manager singleton (no-op when absent); a later
/// [`create_instance`] resurrects it with fresh playback state.
pub fn destroy_instance() {
	unsafe { oak_audio_manager_destroy_instance() };
}

pub fn output_device_names() -> VecString {
	unsafe { VecString::from_raw(oak_audio_manager_output_device_names()) }
}

pub fn input_device_names() -> VecString {
	unsafe { VecString::from_raw(oak_audio_manager_input_device_names()) }
}

/// The enumeration index of the device named `name`, or `None` when no
/// such device exists. The index is what the `set_*_device` calls take.
pub fn device_index_by_name(name: &str, is_output: bool) -> Option<i32> {
	let index = unsafe {
		oak_audio_manager_device_index_by_name(name.as_ptr(), name.len(), is_output)
	};
	(index >= 0).then_some(index)
}

/// Selects the output device by enumeration index (-1 = system
/// default). The output stream reopens on the next pushed samples.
pub fn set_output_device(device: i32) -> Result<(), Error> {
	if unsafe { oak_audio_manager_set_output_device(device) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Selects the input device by enumeration index (-1 = system default);
/// used by recording.
pub fn set_input_device(device: i32) -> Result<(), Error> {
	if unsafe { oak_audio_manager_set_input_device(device) } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Writes per-channel linear peaks into `peaks`, returning the live
/// channel count (0 when nothing is playing). `None` on engine failure.
/// When the buffer is smaller than the channel count, only `peaks.len()`
/// entries are written but the true count comes back.
pub fn output_levels(peaks: &mut [f32]) -> Option<usize> {
	let n = unsafe { oak_audio_manager_output_levels(peaks.as_mut_ptr(), peaks.len()) };
	(n >= 0).then_some(n as usize)
}

/// Seconds of audio consumed by the output device since the last clock
/// reset; `None` when no output stream has run yet.
pub fn seconds() -> Option<f64> {
	let mut seconds = 0.0;
	unsafe { oak_audio_manager_seconds(&mut seconds) }.then_some(seconds)
}

/// Drops the queued-but-unplayed output samples.
pub fn clear_buffered_output() -> Result<(), Error> {
	if unsafe { oak_audio_manager_clear_buffered_output() } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Re-anchors the output clock at zero.
pub fn reset_output_clock() -> Result<(), Error> {
	if unsafe { oak_audio_manager_reset_output_clock() } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Stops the output stream (queued samples are dropped).
pub fn stop_output() -> Result<(), Error> {
	if unsafe { oak_audio_manager_stop_output() } {
		Ok(())
	} else {
		Err(Error)
	}
}

/// Frames the device had to pad with silence since the last call (the
/// counter resets on read). `None` when the instance is gone.
pub fn take_output_underrun_frames() -> Option<i64> {
	let n = unsafe { oak_audio_manager_take_output_underrun_frames() };
	(n >= 0).then_some(n)
}

/// Frames currently queued on the output device.
pub fn output_queued_frames() -> Option<i64> {
	let n = unsafe { oak_audio_manager_output_queued_frames() };
	(n >= 0).then_some(n)
}

/// Drops up to `frames` stale frames from the device queue, returning
/// how many were actually dropped.
pub fn drop_output_frames(frames: i64) -> Option<i64> {
	let n = unsafe { oak_audio_manager_drop_output_frames(frames) };
	(n >= 0).then_some(n)
}

/// Pushes one chunk of interleaved samples to the output device. On
/// failure the engine's error text (when it provides one) comes back in
/// the `Err`.
pub fn push_to_output(params: AudioParams, samples: &[u8]) -> Result<(), PushError> {
	let mut error_buf = [0u8; 256];
	let ok = unsafe {
		oak_audio_manager_push_to_output(
			params,
			samples.as_ptr(),
			samples.len(),
			error_buf.as_mut_ptr(),
			error_buf.len(),
		)
	};
	if ok {
		return Ok(());
	}
	let end = error_buf
		.iter()
		.position(|&b| b == 0)
		.unwrap_or(error_buf.len());
	let message = std::str::from_utf8(&error_buf[..end])
		.unwrap_or("")
		.trim();
	Err(PushError(if message.is_empty() {
		"unknown engine error".to_string()
	} else {
		message.to_string()
	}))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;

	// The manager is a process-wide singleton; the tests below create and
	// destroy it, so they must not run concurrently.
	static SERIAL: Mutex<()> = Mutex::new(());

	#[test]
	fn create_and_destroy_roundtrip() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		destroy_instance();
		// The engine resurrects the singleton on the next create.
		create_instance().expect("re-create after destroy");
		destroy_instance();
	}

	#[test]
	fn device_names_are_structurally_consistent() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");

		// Works on headless machines too: whatever the enumeration finds
		// (possibly nothing), the wrapper's views must agree with it.
		for names in [output_device_names(), input_device_names()] {
			assert_eq!(names.iter().count(), names.len());
			assert_eq!(names.to_vec().len(), names.len());
			assert_eq!(names.is_empty(), names.len() == 0);
			assert!(names.get(names.len()).is_none(), "out-of-bounds must be None");
			for (i, name) in names.iter().enumerate() {
				assert_eq!(names.get(i), Some(name));
			}
		}
	}

	#[test]
	fn device_index_of_missing_name_is_none() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		assert_eq!(device_index_by_name("__oak_no_such_device__", true), None);
	}

	#[test]
	fn set_default_devices() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		// -1 = system default; valid even on headless machines (the stream
		// only opens when samples are pushed).
		set_output_device(-1).expect("set output");
		set_input_device(-1).expect("set input");
	}

	#[test]
	fn levels_and_counters_have_safe_shapes() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");

		// No playback yet: zero channels, but the call itself works.
		let mut peaks = [1.0f32; 8];
		assert_eq!(output_levels(&mut peaks), Some(0));
		// The query-only mode (empty buffer) must not write anything.
		assert_eq!(output_levels(&mut []), Some(0));

		assert_eq!(take_output_underrun_frames(), Some(0));
		assert!(output_queued_frames().is_some());
		assert_eq!(drop_output_frames(0), Some(0));

		// No stream has run: the clock may legitimately report None.
		let _ = seconds();
	}

	#[test]
	fn playback_controls_accept_calls_without_a_stream() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		clear_buffered_output().expect("clear");
		reset_output_clock().expect("reset clock");
		stop_output().expect("stop");
	}

	#[test]
	fn push_to_output_reports_errors_with_text() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		let params = AudioParams::new(44_100, 0x3, SampleFormat::F32); // stereo
		let silence = vec![0u8; 4096];
		match push_to_output(params, &silence) {
			Ok(()) => {}
			// Headless machines may have no output device; the error must
			// still surface with a non-empty message.
			Err(e) => assert!(!e.0.is_empty()),
		}
	}

	#[test]
	fn device_names_move_across_threads() {
		let _guard = SERIAL.lock().unwrap();
		create_instance().expect("create");
		let names = output_device_names();
		let len = names.len();
		let moved_len = std::thread::spawn(move || names.len()).join().unwrap();
		assert_eq!(len, moved_len);
	}
}

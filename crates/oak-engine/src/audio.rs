use oak_audio::manager::*;
use oak_audio::params::AudioParams;
use oak_core::SampleFormat;
use crate::vecs::{IntoFfiVec, OakVecString};
/// Audio stream parameters. Bridge to C value handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OakAudioParams{
    /// Sample rate in Hz.
    pub sample_rate: i32,
    /// ffmpeg-style channel layout mask (0 = unknown/unspecified).
    pub channel_layout: u64,
    /// Sample format (see [`SampleFormat`]).
    pub format: i32,
}

impl OakAudioParams{
    pub fn from_audio_params(params: AudioParams) -> Self{
        Self{
            sample_rate: params.sample_rate,
            channel_layout: params.channel_layout,
            format: params.format as i32,
        }
    }
    pub fn into_audio_params(self) -> AudioParams{
        AudioParams{
            sample_rate: self.sample_rate,
            channel_layout: self.channel_layout,
            format: match self.format{
                // Unsigned 8-bit planar.
                0 => SampleFormat::U8Planar,
                // Signed 16-bit planar.
                1 => SampleFormat::S16Planar,
                // Signed 32-bit planar.
                2 => SampleFormat::S32Planar,
                // Signed 64-bit planar.
                3 => SampleFormat::S64Planar,
                // 32-bit float planar.
                4 => SampleFormat::F32Planar,
                // 64-bit float planar.
                5 => SampleFormat::F64Planar,
                // Unsigned 8-bit packed.
                6 => SampleFormat::U8,
                // Signed 16-bit packed.
                7 => SampleFormat::S16,
                // Signed 32-bit packed.
                8 => SampleFormat::S32,
                // Signed 64-bit packed.
                9 => SampleFormat::S64,
                // 32-bit float packed.
                10 => SampleFormat::F32,
                // 64-bit float packed.
                11 => SampleFormat::F64,
                // Invalid/unspecified.
                _ => SampleFormat::Invalid
            } ,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_create_instance() -> bool{
    ManagerInner::create_instance().is_ok()
}
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_destroy_instance(){
    ManagerInner::destroy_instance()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_output_device_names() -> *mut OakVecString{
    output_device_names().into_ffi_vec()
}
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_input_device_names() -> *mut OakVecString{
    input_device_names().into_ffi_vec()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_manager_device_index_by_name(name: *const u8, len: usize, is_output: bool) -> i32{
    if name.is_null() {
        return -1;
    }
    // The buffer is borrowed from C: build a &str view over it (no copy,
    // no ownership transfer), validating UTF-8 at the boundary.
    let bytes = unsafe { std::slice::from_raw_parts(name, len) };
    let Ok(name) = std::str::from_utf8(bytes) else {
        return -1;
    };
    device_index_by_name(name, is_output).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_set_output_device(device: i32) -> bool{
    let Some(mut inst) = instance() else { return false; };
    inst.set_output_device(device).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_set_input_device(device: i32) -> bool{
    let Some(mut inst) = instance() else { return false; };
    inst.set_input_device(device).is_ok()
}

/// Writes per-channel linear peaks into the caller's buffer.
/// Returns the channel count (>= 0), -1 on error. At most `capacity`
/// entries are written; pass null + 0 to just query the count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_manager_output_levels(peaks: *mut f32, capacity: usize) -> i32{
    if peaks.is_null() && capacity > 0 {
        return -1;
    }
    let Some(inst) = instance() else { return -1; };
    let peaks = if peaks.is_null() {
        // from_raw_parts_mut(null, 0) is UB; an empty slice is not.
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(peaks, capacity) }
    };
    inst.output_levels(peaks).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_manager_seconds(seconds: *mut f64) -> bool{
    if seconds.is_null() {
        return false;
    }
    let Some(inst) = instance() else { return false; };
    let mut seconds_got:f64 = 0.0;
    if !inst.seconds(&mut seconds_got).is_ok(){
        return false;
    }
    unsafe { *seconds = seconds_got };
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_clear_buffered_output() -> bool{
    let Some(inst) = instance() else { return false; };
    inst.clear_buffered_output().is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_reset_output_clock() -> bool{
    let Some(inst) = instance() else { return false; };
    inst.reset_output_clock().is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_stop_output() -> bool{
    let Some(mut inst) = instance() else { return false; };
    inst.stop_output().is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_take_output_underrun_frames() -> i64{
    let Some(inst) = instance() else { return -1; };
    inst.take_output_underrun_frames()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_output_queued_frames() -> i64{
    let Some(inst) = instance() else { return -1; };
    inst.output_queued_frames()
}

#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_manager_drop_output_frames(frame: i64) -> i64{
    let Some(inst) = instance() else { return -1; };
    inst.drop_output_frames(frame)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_manager_push_to_output(params: OakAudioParams,
                                                          sample: *const u8, sample_len: usize,
                                                          error_buf: *mut u8, error_buf_len: usize
) -> bool{
    // from_raw_parts on null is UB even with len 0; empty slices are not.
    if sample.is_null() && sample_len > 0 {
        return false;
    }
    if error_buf.is_null() && error_buf_len > 0 {
        return false;
    }
    let sample_buf = if sample.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(sample, sample_len) }
    };
    let error_buf = if error_buf.is_null() {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(error_buf, error_buf_len) }
    };
    let Some(mut inst) = instance() else { return false; };
    inst.push_to_output(params.into_audio_params(), sample_buf, error_buf).is_ok()
}

// ---------------------------------------------------------------------------
// Audio params conversions (pure functions)
// ---------------------------------------------------------------------------

/// Channel count of an ffmpeg-style channel layout mask.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_channel_count(channel_layout: u64) -> i32 {
	oak_audio::params::AudioParams {
		sample_rate: 0,
		channel_layout,
		format: oak_core::SampleFormat::Invalid,
	}
	.channel_count()
}

/// Bytes per sample per channel for a SampleFormat discriminant.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_bytes_per_sample_per_channel(format: i32) -> i64 {
	oak_audio::params::AudioParams {
		sample_rate: 0,
		channel_layout: 0,
		format: oak_audio::params::sample_format_from_i32(format),
	}
	.bytes_per_sample_per_channel()
}

/// Byte count of `samples` frames in the given format/layout.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_samples_to_bytes(
	sample_rate: i32,
	channel_layout: u64,
	format: i32,
	samples: i64,
) -> i64 {
	oak_audio::params::AudioParams {
		sample_rate,
		channel_layout,
		format: oak_audio::params::sample_format_from_i32(format),
	}
	.samples_to_bytes(samples)
}

/// Frame count as rational seconds at `sample_rate`.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_frames_to_rational(
	frames: i64,
	sample_rate: i32,
) -> crate::coretypes::OakRational {
	crate::handle::NativeMirror::from_native(&oak_audio::params::frames_to_rational(
		frames,
		sample_rate,
	))
}

/// Rational seconds as a frame count at `sample_rate`.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_rational_to_samples(
	time: crate::coretypes::OakRational,
	sample_rate: i32,
) -> i64 {
	oak_audio::params::rational_to_samples(
		crate::handle::NativeMirror::to_native(&time),
		sample_rate,
	)
}

/// `f64` seconds as a rational.
#[unsafe(no_mangle)]
pub extern "C" fn oak_audio_params_rational_from_double(
	value: f64,
) -> crate::coretypes::OakRational {
	crate::handle::NativeMirror::from_native(&oak_audio::params::rational_from_double(value))
}

// ---------------------------------------------------------------------------
// Level meter
// ---------------------------------------------------------------------------

/// Per-channel level statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OakChannelStats {
	/// Peak amplitude, linear scale (0.0 when silent).
	pub peak_linear: f64,
	/// Peak amplitude, decibel scale (-200.0 when silent).
	pub peak_db: f64,
	/// RMS level, linear scale.
	pub rms_linear: f64,
	/// RMS level, decibel scale (-200.0 when silent).
	pub rms_db: f64,
	/// VU-meter ballistics reading, decibel scale.
	pub vu_db: f64,
}

/// Whole-buffer level statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OakLevelStats {
	/// Maximum peak across all channels, linear scale.
	pub max_peak_linear: f64,
	/// Integrated loudness (EBU R128 LUFS; -200.0 for silence).
	pub integrated_lufs: f64,
	/// Every channel below the noise gate.
	pub silence: i32,
}

/// Analyzes a planar f32 sample buffer: `planar` points to `channels`
/// channel pointers, each with `samples_per_channel` samples.
/// `channel_stats` must hold `channels` entries (may be null to skip
/// per-channel stats). False on null/invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_levelmeter_analyze(
	planar: *const *const f32,
	samples_per_channel: usize,
	channels: usize,
	channel_stats: *mut OakChannelStats,
	out: *mut OakLevelStats,
) -> bool {
	if out.is_null() || (planar.is_null() && channels > 0) {
		return false;
	}
	if channels > 0 && channel_stats.is_null() {
		return false;
	}
	let channels_ptrs =
		unsafe { std::slice::from_raw_parts(planar, channels) };
	let mut lanes: Vec<&[f32]> = Vec::with_capacity(channels);
	for &ptr in channels_ptrs {
		if ptr.is_null() && samples_per_channel > 0 {
			return false;
		}
		lanes.push(if ptr.is_null() {
			&[]
		} else {
			unsafe { std::slice::from_raw_parts(ptr, samples_per_channel) }
		});
	}
	let stats = oak_audio::levelmeter::analyze_sample_buffer(&lanes);
	for (i, c) in stats.channels.iter().enumerate() {
		unsafe {
			*channel_stats.add(i) = OakChannelStats {
				peak_linear: c.peak_linear,
				peak_db: c.peak_db,
				rms_linear: c.rms_linear,
				rms_db: c.rms_db,
				vu_db: c.vu_db,
			};
		}
	}
	unsafe {
		*out = OakLevelStats {
			max_peak_linear: stats.max_peak_linear,
			integrated_lufs: stats.integrated_lufs,
			silence: stats.silence as i32,
		};
	}
	true
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn params_conversions() {
		// Stereo (layout 0x3) F32 (discriminant 10): 4 bytes/sample/channel.
		assert_eq!(oak_audio_params_channel_count(0x3), 2);
		assert_eq!(oak_audio_params_bytes_per_sample_per_channel(10), 4);
		assert_eq!(oak_audio_params_samples_to_bytes(48000, 0x3, 10, 100), 800);

		let r = oak_audio_params_frames_to_rational(48, 48_000);
		assert_eq!((r.num, r.den), (1, 1000));
		assert_eq!(oak_audio_params_rational_to_samples(r, 48_000), 48);
		let d = oak_audio_params_rational_from_double(0.5);
		assert_eq!((d.num, d.den), (1, 2));
	}

	#[test]
	fn levelmeter_analyze() {
		unsafe {
			let left = [0.5f32, -0.5, 0.25];
			let right = [0.25f32, 0.25, -0.25];
			let planar = [left.as_ptr(), right.as_ptr()];
			let mut channels = [OakChannelStats {
				peak_linear: 0.0,
				peak_db: 0.0,
				rms_linear: 0.0,
				rms_db: 0.0,
				vu_db: 0.0,
			}; 2];
			let mut stats = OakLevelStats {
				max_peak_linear: 0.0,
				integrated_lufs: 0.0,
				silence: 1,
			};
			assert!(oak_audio_levelmeter_analyze(
				planar.as_ptr(),
				3,
				2,
				channels.as_mut_ptr(),
				&mut stats,
			));
			assert_eq!(stats.max_peak_linear, 0.5);
			assert_eq!(stats.silence, 0);
			assert_eq!(channels[0].peak_linear, 0.5);
			assert_eq!(channels[1].peak_linear, 0.25);

			// Null safety.
			assert!(!oak_audio_levelmeter_analyze(
				std::ptr::null(),
				3,
				1,
				channels.as_mut_ptr(),
				&mut stats,
			));
			assert!(!oak_audio_levelmeter_analyze(
				planar.as_ptr(),
				3,
				2,
				std::ptr::null_mut(),
				&mut stats,
			));
		}
	}
}

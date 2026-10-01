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

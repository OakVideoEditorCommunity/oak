//! C ABI for encoder queries (prefix `oak_codec_encoder_`): the
//! image-sequence placeholder filename helpers.
//!
//! Encode sessions (create_from_params/write_frame/close) are
//! deliberately not exported yet: they need a session-handle design of
//! their own.

use oak_codec::encoder;

use crate::codec::{copy_str_out, str_arg};

/// Whether `filename` contains an image-sequence digit placeholder;
/// false on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_filename_contains_digit_placeholder(
	filename: *const u8,
	filename_len: usize,
) -> bool {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return false;
	};
	encoder::filename_contains_digit_placeholder(filename)
}

/// The digit count of the sequence placeholder in `filename` (0 when
/// none); -1 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_image_sequence_placeholder_digit_count(
	filename: *const u8,
	filename_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	encoder::image_sequence_placeholder_digit_count(filename)
}

/// `filename` with the digit placeholder removed, snprintf-style; -1 on
/// invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_filename_remove_digit_placeholder(
	filename: *const u8,
	filename_len: usize,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	copy_str_out(&encoder::filename_remove_digit_placeholder(filename), buf, buf_len)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn placeholder_helpers() {
		unsafe {
			let with = "export.[####].exr";
			assert!(oak_codec_encoder_filename_contains_digit_placeholder(
				with.as_ptr(),
				with.len()
			));
			assert_eq!(
				oak_codec_encoder_image_sequence_placeholder_digit_count(
					with.as_ptr(),
					with.len()
				),
				4
			);

			let without = "export.exr";
			assert!(!oak_codec_encoder_filename_contains_digit_placeholder(
				without.as_ptr(),
				without.len()
			));

			let mut buf = [0u8; 256];
			let n = oak_codec_encoder_filename_remove_digit_placeholder(
				with.as_ptr(),
				with.len(),
				buf.as_mut_ptr(),
				buf.len(),
			);
			assert!(n > 0);
			assert!(!std::str::from_utf8(&buf[..n as usize])
				.unwrap()
				.contains('#'));
		}
	}
}

// ---------------------------------------------------------------------------
// Encode sessions (opaque handle over a configured encoder)
// ---------------------------------------------------------------------------
//
// Lifecycle: create → open → write_* × N → flush → close → release.
// `write_*` before `open` fails (false), `close` is idempotent.

use std::sync::Arc;

use oak_codec::encoder::Encoder;
use crate::handle::into_ffi;
use oak_codec::encodingparams::EncodingParams;
use oak_codec::frame::Frame;

/// An encode session. Refcounted like every engine handle.
pub struct EncoderSession {
	encoder: Arc<dyn Encoder>,
}

crate::export_handle!(
	EncoderSession,
	oak_codec_encoder_add_ref,
	oak_codec_encoder_release
);

/// The default encoding parameters (invalid format, everything off).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_encoding_params_default() -> EncodingParams {
	EncodingParams::default()
}

/// Creates an encoder session for `params` (the encoder is configured,
/// not yet open). `EncodingParams` is `repr(C)` and crosses by reference
/// — declare the identical layout in the C header. Null when no encoder
/// supports the requested format or configuration fails.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_create(
	params: *const EncodingParams,
) -> *mut EncoderSession {
	if params.is_null() {
		return std::ptr::null_mut();
	}
	let params = unsafe { &*params };
	match encoder::create_from_params(params) {
		Some(encoder) => {
			if encoder.configure(params).is_err() {
				return std::ptr::null_mut();
			}
			into_ffi(EncoderSession { encoder }) as *mut EncoderSession
		}
		None => std::ptr::null_mut(),
	}
}

/// Opens the output file and writes the headers. False on null or
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_open(session: *mut EncoderSession) -> bool {
	if session.is_null() {
		return false;
	}
	unsafe { &*session }.encoder.open().is_ok()
}

/// Encodes one video frame (a `Frame` handle from
/// `oak_codec_frame_*`; the caller keeps its reference). False on null
/// input, a closed session, or encode failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_write_video(
	session: *mut EncoderSession,
	frame: *const Frame,
) -> bool {
	if session.is_null() || frame.is_null() {
		return false;
	}
	unsafe { &*session }
		.encoder
		.write_video(unsafe { &*frame })
		.is_ok()
}

/// Encodes `frame_count` frames of interleaved f32 audio from
/// `samples`. False on null input or encode failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_write_audio(
	session: *mut EncoderSession,
	samples: *const f32,
	sample_count: usize,
	frame_count: i32,
) -> bool {
	if session.is_null() || (samples.is_null() && sample_count > 0) {
		return false;
	}
	let samples = if samples.is_null() {
		&[]
	} else {
		unsafe { std::slice::from_raw_parts(samples, sample_count) }
	};
	unsafe { &*session }
		.encoder
		.write_audio(samples, frame_count)
		.is_ok()
}

/// Flushes the encoders. False on null or failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_flush(session: *mut EncoderSession) -> bool {
	if session.is_null() {
		return false;
	}
	unsafe { &*session }.encoder.flush().is_ok()
}

/// Writes the trailer and closes the output (idempotent). False on null
/// or failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_close(session: *mut EncoderSession) -> bool {
	if session.is_null() {
		return false;
	}
	unsafe { &*session }.encoder.close().is_ok()
}

#[cfg(test)]
mod session_tests {
	use super::*;
	use crate::codec::frame::{
		oak_codec_frame_allocate, oak_codec_frame_data, oak_codec_frame_release,
		oak_codec_frame_set_timestamp, oak_codec_frame_with_params,
	};
	use crate::coretypes::{OakRational, oak_core_videoparams_default};

	fn h264_params(out: &std::path::Path) -> EncodingParams {
		let mut p = EncodingParams::default();
		let name = out.as_os_str().as_encoded_bytes();
		p.filename[..name.len()].copy_from_slice(name);
		p.format = 2; // MPEG-4 video
		p.video_enabled = 1;
		p.video_codec = 1; // H.264
		p.video_width = 64;
		p.video_height = 64;
		p.video_time_base_num = 1;
		p.video_time_base_den = 10;
		p.video_pixel_format = oak_core::PixelFormat::F32;
		p.video_pixel_aspect_num = 1;
		p.video_pixel_aspect_den = 1;
		p
	}

	/// An allocated F32-RGBA frame via the frame C ABI.
	unsafe fn test_frame(i: i32) -> *mut Frame {
		unsafe {
			let mut vp = oak_core_videoparams_default();
			vp.width = 64;
			vp.height = 64;
			vp.format = 4; // F32
			vp.channel_count = 4;
			let frame = oak_codec_frame_with_params(vp);
			assert!(oak_codec_frame_allocate(frame));
			oak_codec_frame_set_timestamp(frame, OakRational { num: i as i64, den: 10 });
			let data = oak_codec_frame_data(frame);
			let size = 64 * 64 * 16;
			for px in 0..size / 16 {
				let r = 0.4f32 + i as f32 * 0.05;
				data.add(px * 16).write_bytes(r.to_le_bytes()[0], 4);
			}
			frame
		}
	}

	#[test]
	fn encode_h264_roundtrip_via_c_abi() {
		let out = std::env::temp_dir()
			.join(format!("oakengine_encode_{}.mp4", std::process::id()));
		let params = h264_params(&out);
		unsafe {
			let session = oak_codec_encoder_create(&params);
			assert!(!session.is_null(), "create the ffmpeg encoder");
			assert!(oak_codec_encoder_open(session));
			for i in 0..10 {
				let frame = test_frame(i);
				assert!(oak_codec_encoder_write_video(session, frame));
				oak_codec_frame_release(frame);
			}
			assert!(oak_codec_encoder_flush(session));
			assert!(oak_codec_encoder_close(session));
			oak_codec_encoder_release(session);
		}
		assert!(out.exists(), "the output file was not created");
		assert!(out.metadata().unwrap().len() > 1000, "the output is empty");
		let _ = std::fs::remove_file(&out);
	}

	#[test]
	fn write_subtitle_forwards_the_encoder_result() {
		// The ffmpeg encoder does not implement subtitle encoding yet
		// (returns Err by design); the binding must forward that as false
		// without crashing. A positive round-trip lands when the encoder
		// gains subtitle support.
		let out = std::env::temp_dir()
			.join(format!("oakengine_subs_{}.mp4", std::process::id()));
		let mut params = h264_params(&out);
		params.subtitles_enabled = 1;
		params.subtitles_codec = 17; // SRT
		params.subtitles_are_sidecar = 1;
		params.subtitles_sidecar_format = 13; // SRT
		unsafe {
			let session = oak_codec_encoder_create(&params);
			assert!(!session.is_null());
			assert!(oak_codec_encoder_open(session));
			let text = "Hello, subtitles";
			assert!(!oak_codec_encoder_write_subtitle(
				session,
				text.as_ptr(),
				text.len(),
				0.0,
				2.5,
			));
			// Null/invalid input is rejected before reaching the encoder.
			assert!(!oak_codec_encoder_write_subtitle(
				session,
				std::ptr::null(),
				1,
				0.0,
				1.0,
			));
			assert!(!oak_codec_encoder_write_subtitle(
				std::ptr::null_mut(),
				text.as_ptr(),
				text.len(),
				0.0,
				1.0,
			));
			assert!(oak_codec_encoder_close(session));
			oak_codec_encoder_release(session);
		}
		let _ = std::fs::remove_file(&out);
		let _ = std::fs::remove_file(out.with_extension("srt"));
	}

	#[test]
	fn encoder_negative_paths() {
		unsafe {
			// Unknown format: no encoder.
			let mut p = EncodingParams::default();
			p.format = 999;
			assert!(oak_codec_encoder_create(&p).is_null());

			// Null params.
			assert!(oak_codec_encoder_create(std::ptr::null()).is_null());

			// Null session calls are safe.
			assert!(!oak_codec_encoder_open(std::ptr::null_mut()));
			assert!(!oak_codec_encoder_write_video(std::ptr::null_mut(), std::ptr::null()));
			assert!(!oak_codec_encoder_flush(std::ptr::null_mut()));
			assert!(!oak_codec_encoder_close(std::ptr::null_mut()));
			oak_codec_encoder_release(std::ptr::null());

			// Write before open fails cleanly.
			let params = h264_params(
				&std::env::temp_dir()
					.join(format!("oakengine_unopened_{}.mp4", std::process::id())),
			);
			let session = oak_codec_encoder_create(&params);
			assert!(!session.is_null());
			let frame = test_frame(0);
			assert!(!oak_codec_encoder_write_video(session, frame));
			oak_codec_frame_release(frame);
			oak_codec_encoder_close(session);
			oak_codec_encoder_release(session);
		}
	}
}

/// Encodes one subtitle entry (`in_seconds`/`out_seconds` in seconds).
/// False on null input, unsupported encoder, or encode failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_encoder_write_subtitle(
	session: *mut EncoderSession,
	text: *const u8,
	text_len: usize,
	in_seconds: f64,
	out_seconds: f64,
) -> bool {
	if session.is_null() {
		return false;
	}
	let Some(text) = (unsafe { super::str_arg(text, text_len) }) else {
		return false;
	};
	unsafe { &*session }
		.encoder
		.write_subtitle(text, in_seconds, out_seconds)
		.is_ok()
}

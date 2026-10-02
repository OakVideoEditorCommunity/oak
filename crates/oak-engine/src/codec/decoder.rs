//! C ABI for decoder queries (prefix `oak_codec_decoder_`): the decoder
//! id registry and the image-sequence filename helpers.
//!
//! Decode sessions (open/retrieve/conform) are deliberately not exported
//! yet: they need a session-handle design (CodecStream,
//! RetrieveVideoParams, color meta) of their own.

use oak_codec::decoder;

use crate::codec::{copy_str_out, str_arg};
use crate::vecs::{IntoFfiVec, OakVecString};

/// The ids of all registered decoders (e.g. "ffmpeg", "oiio"), as an
/// `OakVecString` (released with `oak_vec_string_release`).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_decoder_ids() -> *mut OakVecString {
	decoder::receive_list_of_all_decoders()
		.iter()
		.map(|d| d.id())
		.collect::<Vec<String>>()
		.into_ffi_vec()
}

/// Substitutes the frame number into an image-sequence filename,
/// snprintf-style; -1 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_transform_image_sequence_filename(
	filename: *const u8,
	filename_len: usize,
	number: i64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	copy_str_out(
		&decoder::transform_image_sequence_file_name(filename, number),
		buf,
		buf_len,
	)
}

/// The digit count of the sequence placeholder in `filename` (0 when it
/// is not an image sequence); -1 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_image_sequence_digit_count(
	filename: *const u8,
	filename_len: usize,
) -> i32 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	decoder::get_image_sequence_digit_count(filename)
}

/// The sequence index encoded in `filename`; -1 on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_image_sequence_index(
	filename: *const u8,
	filename_len: usize,
) -> i64 {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return -1;
	};
	decoder::get_image_sequence_index(filename)
}

#[cfg(test)]
mod tests {
	use super::*;

	unsafe extern "C" {
		#[link_name = "oak_vec_string_len"]
		fn vec_len(this: *const OakVecString) -> usize;
		#[link_name = "oak_vec_string_get"]
		fn vec_get(this: *const OakVecString, index: usize, out_len: *mut usize) -> *const u8;
		#[link_name = "oak_vec_string_release"]
		unsafe fn vec_release(this: *const OakVecString);
	}

	#[test]
	fn decoder_ids_include_ffmpeg() {
		unsafe {
			let ids = oak_codec_decoder_ids();
			let mut found = false;
			for i in 0..vec_len(ids) {
				let mut len = 0usize;
				let ptr = vec_get(ids, i, &mut len);
				let id = std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap();
				found |= id == "ffmpeg";
			}
			vec_release(ids);
			assert!(found, "the FFmpeg decoder must be registered");
		}
	}

	#[test]
	fn image_sequence_helpers() {
		unsafe {
			// Trailing-digit stems mark a sequence: "frame_0001.png".
			let name = "frame_0001.png";
			let digits = oak_codec_decoder_image_sequence_digit_count(name.as_ptr(), name.len());
			assert_eq!(digits, 4);

			let mut buf = [0u8; 256];
			let n = oak_codec_decoder_transform_image_sequence_filename(
				name.as_ptr(),
				name.len(),
				42,
				buf.as_mut_ptr(),
				buf.len(),
			);
			assert!(n > 0);
			let out = std::str::from_utf8(&buf[..n as usize]).unwrap();
			assert_eq!(out, "frame_0042.png");

			// No trailing digits: not a sequence.
			let plain = "frame.png";
			assert_eq!(
				oak_codec_decoder_image_sequence_digit_count(plain.as_ptr(), plain.len()),
				0
			);
			assert_eq!(
				oak_codec_decoder_image_sequence_digit_count(std::ptr::null(), 0),
				-1
			);
		}
	}
}

// ---------------------------------------------------------------------------
// Probe (read-only opaque handle over FootageDescription)
// ---------------------------------------------------------------------------

use oak_codec::footagedescription::FootageDescription;

use crate::coretypes::{OakRational, OakTimeRange, OakVideoParams};
use crate::handle::{NativeMirror, into_ffi};

crate::export_handle!(
	FootageDescription,
	oak_codec_footage_add_ref,
	oak_codec_footage_release
);

/// An audio stream's parameters as reported by a probe.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OakAudioStreamParams {
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub format: i32,
	pub stream_index: i32,
	pub duration: i64,
	pub time_base_num: i32,
	pub time_base_den: i32,
}

impl NativeMirror for OakAudioStreamParams {
	type Native = oak_codec::audioparams::AudioParams;

	fn from_native(p: &Self::Native) -> Self {
		Self {
			sample_rate: p.sample_rate,
			channel_layout: p.channel_layout,
			format: p.format,
			stream_index: p.stream_index,
			duration: p.duration,
			time_base_num: p.time_base.0,
			time_base_den: p.time_base.1,
		}
	}

	fn to_native(&self) -> Self::Native {
		oak_codec::audioparams::AudioParams {
			sample_rate: self.sample_rate,
			channel_layout: self.channel_layout,
			format: self.format,
			stream_index: self.stream_index,
			duration: self.duration,
			time_base: (self.time_base_num, self.time_base_den),
			..Default::default()
		}
	}
}

/// A subtitle stream's parameters as reported by a probe (the subtitle
/// entries themselves are content, not exposed here).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OakSubtitleParams {
	pub stream_index: i32,
	pub enabled: i32,
	pub entry_count: i32,
}

/// Probes `filename` and returns its stream inventory as an opaque
/// handle (release with `oak_codec_footage_release`). `decoder_id` null
/// or empty tries every registered decoder in order; otherwise only the
/// named decoder. Null when nothing can read the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_probe(
	decoder_id: *const u8,
	decoder_id_len: usize,
	filename: *const u8,
	filename_len: usize,
) -> *mut FootageDescription {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return std::ptr::null_mut();
	};
	if decoder_id.is_null() || decoder_id_len == 0 {
		for d in decoder::receive_list_of_all_decoders() {
			if let Some(desc) = d.probe(filename, None) {
				return into_ffi(desc) as *mut FootageDescription;
			}
		}
		return std::ptr::null_mut();
	}
	let Some(id) = (unsafe { str_arg(decoder_id, decoder_id_len) }) else {
		return std::ptr::null_mut();
	};
	match decoder::create_from_id(id).and_then(|d| d.probe(filename, None)) {
		Some(desc) => into_ffi(desc) as *mut FootageDescription,
		None => std::ptr::null_mut(),
	}
}

/// The probing decoder's id, snprintf-style; -1 on null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_decoder_name(
	fd: *const FootageDescription,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if fd.is_null() {
		return -1;
	}
	copy_str_out(unsafe { &*fd }.decoder(), buf, buf_len)
}

/// Total stream count (video + audio + subtitle); 0 on null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_total_stream_count(
	fd: *const FootageDescription,
) -> i32 {
	if fd.is_null() {
		return 0;
	}
	unsafe { &*fd }.total_stream_count() as i32
}

/// The kind of the `index`-th stream in probe order: 0 = video,
/// 1 = audio, 2 = subtitle, -1 = invalid index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_stream_type(
	fd: *const FootageDescription,
	index: i32,
) -> i32 {
	if fd.is_null() || index < 0 {
		return -1;
	}
	let fd = unsafe { &*fd };
	let index = index as usize;
	if fd.stream_is_video(index) {
		0
	} else if fd.stream_is_audio(index) {
		1
	} else if fd.stream_is_subtitle(index) {
		2
	} else {
		-1
	}
}

macro_rules! footage_count_getter {
	($( $name:ident => $native:ident ),* $(,)?) => {
		$(
			#[doc = concat!("`FootageDescription::", stringify!($native), "` (0 on null).")]
			#[unsafe(no_mangle)]
			pub unsafe extern "C" fn $name(fd: *const FootageDescription) -> i32 {
				if fd.is_null() {
					return 0;
				}
				unsafe { &*fd }.$native() as i32
			}
		)*
	};
}

footage_count_getter! {
	oak_codec_footage_video_stream_count => video_stream_count,
	oak_codec_footage_audio_stream_count => audio_stream_count,
	oak_codec_footage_subtitle_stream_count => subtitle_stream_count,
}

/// The `ordinal`-th video stream's params (per-type ordinal, NOT the
/// probe-order index). False when out of range or on null input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_get_video_stream(
	fd: *const FootageDescription,
	ordinal: i32,
	out: *mut OakVideoParams,
) -> bool {
	if fd.is_null() || out.is_null() || ordinal < 0 {
		return false;
	}
	match unsafe { &*fd }.get_video_stream(ordinal as usize) {
		Some(params) => {
			unsafe { *out = OakVideoParams::from_native(params) };
			true
		}
		None => false,
	}
}

/// The `ordinal`-th audio stream's params (per-type ordinal).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_get_audio_stream(
	fd: *const FootageDescription,
	ordinal: i32,
	out: *mut OakAudioStreamParams,
) -> bool {
	if fd.is_null() || out.is_null() || ordinal < 0 {
		return false;
	}
	match unsafe { &*fd }.get_audio_stream(ordinal as usize) {
		Some(params) => {
			unsafe { *out = OakAudioStreamParams::from_native(params) };
			true
		}
		None => false,
	}
}

/// The `ordinal`-th subtitle stream's params (per-type ordinal).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_get_subtitle_stream(
	fd: *const FootageDescription,
	ordinal: i32,
	out: *mut OakSubtitleParams,
) -> bool {
	if fd.is_null() || out.is_null() || ordinal < 0 {
		return false;
	}
	match unsafe { &*fd }.get_subtitle_stream(ordinal as usize) {
		Some(params) => {
			unsafe {
				*out = OakSubtitleParams {
					stream_index: params.stream_index(),
					enabled: params.enabled() as i32,
					entry_count: params.count(),
				}
			};
			true
		}
		None => false,
	}
}

/// Whether the media carries a source start time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_has_source_start_time(
	fd: *const FootageDescription,
) -> bool {
	!fd.is_null() && unsafe { &*fd }.has_source_start_time()
}

/// The source start time (the null rational when absent).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_source_start_time(
	fd: *const FootageDescription,
) -> OakRational {
	if fd.is_null() {
		return OakRational { num: 0, den: 0 };
	}
	OakRational::from_native(&unsafe { &*fd }.source_start_time())
}

/// The total duration; false when unknown or on null input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_footage_duration(
	fd: *const FootageDescription,
	out: *mut OakTimeRange,
) -> bool {
	if fd.is_null() || out.is_null() {
		return false;
	}
	match unsafe { &*fd }.duration() {
		Some(d) => {
			unsafe { *out = OakTimeRange::from_native(&d) };
			true
		}
		None => false,
	}
}

#[cfg(test)]
mod probe_tests {
	use super::*;
	use crate::coretypes::{OakRational, OakTimeRange};

	fn temp_clip(name: &str) -> String {
		let dir = std::env::temp_dir().join(format!("oakengine_probe_{}_{}", name, std::process::id()));
		let _ = std::fs::create_dir_all(&dir);
		let path = dir.join("clip.mp4");
		oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10)
			.expect("generate test media");
		path.to_string_lossy().into_owned()
	}

	#[test]
	fn probe_any_decoder_reports_streams() {
		let clip = temp_clip("any");
		unsafe {
			let fd = oak_codec_decoder_probe(std::ptr::null(), 0, clip.as_ptr(), clip.len());
			assert!(!fd.is_null(), "probe must succeed for a test clip");

			assert!(oak_codec_footage_total_stream_count(fd) >= 1);
			assert!(oak_codec_footage_video_stream_count(fd) >= 1);

			// Probe order: stream 0 is video in the test clip.
			assert_eq!(oak_codec_footage_stream_type(fd, 0), 0);

			let mut vp = crate::coretypes::oak_core_videoparams_default();
			assert!(oak_codec_footage_get_video_stream(fd, 0, &mut vp));
			assert_eq!((vp.width, vp.height), (64, 64));

			// Out-of-range ordinal fails cleanly.
			assert!(!oak_codec_footage_get_video_stream(fd, 99, &mut vp));

			let mut dur = OakTimeRange {
				in_: OakRational { num: 0, den: 0 },
				out: OakRational { num: 0, den: 0 },
			};
			// Duration is optional in a probe (the ffmpeg probe of the
			// synthetic clip leaves it empty); the call must be safe either
			// way, and a present duration must be sane.
			if oak_codec_footage_duration(fd, &mut dur) {
				assert!(dur.out.num > 0, "a present duration is positive: {dur:?}");
			}

			oak_codec_footage_release(fd);
		}
	}

	#[test]
	fn probe_with_named_and_unknown_decoders() {
		let clip = temp_clip("named");
		unsafe {
			let ffmpeg = "ffmpeg";
			let fd =
				oak_codec_decoder_probe(ffmpeg.as_ptr(), ffmpeg.len(), clip.as_ptr(), clip.len());
			assert!(!fd.is_null());
			let mut buf = [0u8; 64];
			let n = oak_codec_footage_decoder_name(fd, buf.as_mut_ptr(), buf.len());
			assert_eq!(std::str::from_utf8(&buf[..n as usize]).unwrap(), "ffmpeg");
			oak_codec_footage_release(fd);

			let bogus = "no-such-decoder";
			assert!(
				oak_codec_decoder_probe(bogus.as_ptr(), bogus.len(), clip.as_ptr(), clip.len())
					.is_null()
			);
			let missing = "/nonexistent/oak-probe.mp4";
			assert!(
				oak_codec_decoder_probe(std::ptr::null(), 0, missing.as_ptr(), missing.len())
					.is_null()
			);
		}
	}

	#[test]
	fn footage_null_is_safe() {
		unsafe {
			assert_eq!(oak_codec_footage_total_stream_count(std::ptr::null()), 0);
			assert_eq!(oak_codec_footage_stream_type(std::ptr::null(), 0), -1);
			assert!(!oak_codec_footage_has_source_start_time(std::ptr::null()));
			assert!(!oak_codec_footage_duration(std::ptr::null(), std::ptr::null_mut()));
			oak_codec_footage_release(std::ptr::null());
		}
	}
}

// ---------------------------------------------------------------------------
// Decode sessions (opaque handle over an open decoder stream)
// ---------------------------------------------------------------------------
//
// Lifecycle: open → retrieve* × N → close → release. `retrieve_*` before
// `open` completed or after `close` fails cleanly (null handle / status
// -1), never crashes.

use std::sync::Arc;

use oak_codec::decoder::{
	CodecStream, K_COLOR_RANGE_DEFAULT, RenderMode, RetrieveVideoParams,
};

/// An open decode session. Refcounted like every engine handle.
pub struct DecoderSession {
	decoder: Arc<dyn decoder::Decoder>,
}

crate::export_handle!(
	DecoderSession,
	oak_codec_decoder_add_ref,
	oak_codec_decoder_release
);

crate::export_handle!(
	RetrieveVideoParams,
	oak_codec_retrieve_params_add_ref,
	oak_codec_retrieve_params_release
);

/// A retrieve-parameter set with engine-side defaults: time 0, no range
/// forcing, no image sequence, offline mode, no premultiply, native
/// target size. Mutate through the setters, pass to
/// `oak_codec_decoder_retrieve_video_frame`, release when done.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_retrieve_params_new() -> *mut RetrieveVideoParams {
	into_ffi(RetrieveVideoParams {
		stream: CodecStream::new(),
		time: oak_core::Rational::new(0, 1),
		length: oak_core::TimeRange::default(),
		force_range: K_COLOR_RANGE_DEFAULT,
		is_image_sequence: false,
		image_sequence_digits: 0,
		image_sequence_number: 0,
		mode: RenderMode::Offline,
		alpha_is_premultiplied: false,
		target_size: None,
	}) as *mut RetrieveVideoParams
}

/// The stream to read from (filename + stream index). False on invalid
/// input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_stream(
	p: *mut RetrieveVideoParams,
	filename: *const u8,
	filename_len: usize,
	stream_index: i32,
) -> bool {
	if p.is_null() {
		return false;
	}
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return false;
	};
	unsafe { &mut *p }.stream =
		CodecStream::with_block(filename.to_string(), stream_index, None);
	true
}

/// The timestamp to retrieve (rational seconds).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_time(
	p: *mut RetrieveVideoParams,
	time: OakRational,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.time = time.to_native();
}

/// The footage range (for early-seek semantics).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_length(
	p: *mut RetrieveVideoParams,
	length: OakTimeRange,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.length = length.to_native();
}

/// Force a color range (`-1` = don't force, the default).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_force_range(
	p: *mut RetrieveVideoParams,
	range: i32,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.force_range = range;
}

/// Marks the source as an image sequence and bakes in the frame number.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_image_sequence(
	p: *mut RetrieveVideoParams,
	digits: i32,
	number: i64,
) {
	if p.is_null() {
		return;
	}
	let p = unsafe { &mut *p };
	p.is_image_sequence = true;
	p.image_sequence_digits = digits;
	p.image_sequence_number = number;
}

/// The target output size (`Some((w, h))`); decode scales in one step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_target_size(
	p: *mut RetrieveVideoParams,
	width: u32,
	height: u32,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.target_size = Some((width, height));
}

/// Decode at native size (the default).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_clear_target_size(
	p: *mut RetrieveVideoParams,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.target_size = None;
}

/// Render mode: 0 = offline, 1 = online.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_mode(
	p: *mut RetrieveVideoParams,
	mode: i32,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.mode = if mode == 1 {
		RenderMode::Online
	} else {
		RenderMode::Offline
	};
}

/// Whether the frame's alpha is premultiplied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_retrieve_params_set_premultiplied_alpha(
	p: *mut RetrieveVideoParams,
	premultiplied: bool,
) {
	if p.is_null() {
		return;
	}
	unsafe { &mut *p }.alpha_is_premultiplied = premultiplied;
}

/// Opens a decode session for `filename`'s `stream_index`. `decoder_id`
/// null/empty auto-picks by probing every registered decoder (first
/// success wins); otherwise only the named decoder. Null when nothing
/// can open the stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_open(
	decoder_id: *const u8,
	decoder_id_len: usize,
	filename: *const u8,
	filename_len: usize,
	stream_index: i32,
) -> *mut DecoderSession {
	let Some(filename) = (unsafe { str_arg(filename, filename_len) }) else {
		return std::ptr::null_mut();
	};
	let decoder = if decoder_id.is_null() || decoder_id_len == 0 {
		decoder::receive_list_of_all_decoders()
			.into_iter()
			.find(|d| d.probe(filename, None).is_some())
	} else {
		let Some(id) = (unsafe { str_arg(decoder_id, decoder_id_len) }) else {
			return std::ptr::null_mut();
		};
		decoder::create_from_id(id)
	};
	let Some(decoder) = decoder else {
		return std::ptr::null_mut();
	};
	let stream = CodecStream::with_block(filename.to_string(), stream_index, None);
	if decoder.open(&stream).is_err() {
		return std::ptr::null_mut();
	}
	into_ffi(DecoderSession { decoder }) as *mut DecoderSession
}

/// Closes the session's stream (safe when already closed). False on
/// null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_close(session: *mut DecoderSession) -> bool {
	if session.is_null() {
		return false;
	}
	unsafe { &*session }.decoder.close().is_ok()
}

/// Retrieves one video frame into CPU memory as a `Frame` handle
/// (release with `oak_codec_frame_release`). Null on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_retrieve_video_frame(
	session: *mut DecoderSession,
	params: *const RetrieveVideoParams,
) -> *mut oak_codec::frame::Frame {
	if session.is_null() || params.is_null() {
		return std::ptr::null_mut();
	}
	match unsafe { &*session }
		.decoder
		.retrieve_video_frame(unsafe { &*params })
	{
		// The Arc moves across the boundary directly; the frame release
		// pair reconstructs it.
		Ok(frame) => Arc::into_raw(frame) as *mut oak_codec::frame::Frame,
		Err(_) => std::ptr::null_mut(),
	}
}

/// Retrieves interleaved f32 audio covering [in, out) into `dest`.
/// Returns a RetrieveAudioStatus code (0=success, 1=invalid range,
/// 2=unsupported, 3=conform needed, 4=decoder error); -1 on ABI misuse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_codec_decoder_retrieve_audio(
	session: *mut DecoderSession,
	dest: *mut f32,
	dest_len: usize,
	in_: OakRational,
	out: OakRational,
	sample_rate: i32,
	channel_layout: u64,
) -> i32 {
	use oak_codec::decoder::RetrieveAudioStatus;
	if session.is_null() || (dest.is_null() && dest_len > 0) {
		return -1;
	}
	let dest = if dest.is_null() {
		&mut []
	} else {
		unsafe { std::slice::from_raw_parts_mut(dest, dest_len) }
	};
	let range = oak_core::TimeRange::new(in_.to_native(), out.to_native());
	let status = unsafe { &*session }
		.decoder
		.retrieve_audio(dest, &range, sample_rate, channel_layout);
	match status {
		Ok(RetrieveAudioStatus::Success) => 0,
		Ok(RetrieveAudioStatus::InvalidRange) => 1,
		Ok(RetrieveAudioStatus::Unsupported) => 2,
		Ok(RetrieveAudioStatus::ConformNeeded) => 3,
		Ok(RetrieveAudioStatus::Error) => 4,
		Err(_) => -1,
	}
}

#[cfg(test)]
mod session_tests {
	use super::*;
	use crate::coretypes::OakRational;

	fn temp_clip(name: &str) -> String {
		let dir = std::env::temp_dir().join(format!(
			"oakengine_session_{}_{}",
			name,
			std::process::id()
		));
		let _ = std::fs::create_dir_all(&dir);
		let path = dir.join("clip.mp4");
		oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("generate test media");
		path.to_string_lossy().into_owned()
	}

	#[test]
	fn open_retrieve_close_roundtrip() {
		let clip = temp_clip("video");
		unsafe {
			let ffmpeg = "ffmpeg";
			let session = oak_codec_decoder_open(
				ffmpeg.as_ptr(),
				ffmpeg.len(),
				clip.as_ptr(),
				clip.len(),
				0,
			);
			assert!(!session.is_null(), "open must succeed for the test clip");

			let params = oak_codec_retrieve_params_new();
			assert!(oak_codec_retrieve_params_set_stream(
				params,
				clip.as_ptr(),
				clip.len(),
				0
			));
			oak_codec_retrieve_params_set_time(params, OakRational { num: 0, den: 1 });

			let frame = oak_codec_decoder_retrieve_video_frame(session, params);
			assert!(!frame.is_null(), "frame 0 must decode");
			assert_eq!(crate::codec::frame::oak_codec_frame_width(frame), 64);
			assert_eq!(crate::codec::frame::oak_codec_frame_height(frame), 64);
			crate::codec::frame::oak_codec_frame_release(frame);

			// Missing params / closed session fail cleanly.
			assert!(oak_codec_decoder_retrieve_video_frame(session, std::ptr::null()).is_null());
			assert!(oak_codec_decoder_close(session));
			assert!(oak_codec_decoder_retrieve_video_frame(session, params).is_null());

			oak_codec_retrieve_params_release(params);
			oak_codec_decoder_release(session);
		}
	}

	#[test]
	fn open_auto_picks_decoder_by_probe() {
		let clip = temp_clip("auto");
		unsafe {
			let session =
				oak_codec_decoder_open(std::ptr::null(), 0, clip.as_ptr(), clip.len(), 0);
			assert!(!session.is_null());
			oak_codec_decoder_close(session);
			oak_codec_decoder_release(session);

			let missing = "/nonexistent/oak-session.mp4";
			assert!(
				oak_codec_decoder_open(std::ptr::null(), 0, missing.as_ptr(), missing.len(), 0)
					.is_null()
			);
		}
	}

	#[test]
	fn audio_retrieve_on_video_only_clip_is_unsupported() {
		let clip = temp_clip("noaudio");
		unsafe {
			let session =
				oak_codec_decoder_open(std::ptr::null(), 0, clip.as_ptr(), clip.len(), 0);
			assert!(!session.is_null());
			let mut dest = [0.0f32; 1024];
			let status = oak_codec_decoder_retrieve_audio(
				session,
				dest.as_mut_ptr(),
				dest.len(),
				OakRational { num: 0, den: 1 },
				OakRational { num: 1, den: 1 },
				48_000,
				0x3,
			);
			assert!(status == 2 || status == 1 || status == 4, "video-only clip: {status}");
			oak_codec_decoder_close(session);
			oak_codec_decoder_release(session);
		}
	}

	#[test]
	fn params_setters_and_null_safety() {
		unsafe {
			let p = oak_codec_retrieve_params_new();
			oak_codec_retrieve_params_set_time(p, OakRational { num: 3, den: 2 });
			oak_codec_retrieve_params_set_force_range(p, -1);
			oak_codec_retrieve_params_set_image_sequence(p, 4, 7);
			oak_codec_retrieve_params_set_target_size(p, 320, 240);
			oak_codec_retrieve_params_clear_target_size(p);
			oak_codec_retrieve_params_set_mode(p, 1);
			oak_codec_retrieve_params_set_premultiplied_alpha(p, true);

			// Verify through the native struct (same crate).
			let native = &*p;
			assert_eq!(native.time, oak_core::Rational::new(3, 2));
			assert!(native.is_image_sequence);
			assert_eq!(native.image_sequence_number, 7);
			assert_eq!(native.target_size, None);
			assert!(native.alpha_is_premultiplied);

			// Null is a no-op everywhere.
			oak_codec_retrieve_params_set_time(std::ptr::null_mut(), OakRational { num: 0, den: 1 });
			assert!(!oak_codec_retrieve_params_set_stream(std::ptr::null_mut(), std::ptr::null(), 0, 0));
			oak_codec_retrieve_params_release(p);
			oak_codec_retrieve_params_release(std::ptr::null());
		}
	}
}

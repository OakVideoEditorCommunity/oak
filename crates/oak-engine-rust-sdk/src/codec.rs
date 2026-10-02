//! Safe wrapper over the engine's codec C ABI (`oak_codec_*`): export
//! format/codec enumerations, the proxy manager, and the background-task
//! submit lane.

use std::ffi::{CStr, CString, c_void};

use crate::vecs::VecI32;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	// exportcodec
	fn oak_codec_codec_is_valid(codec: i32) -> bool;
	fn oak_codec_codec_count() -> i32;
	fn oak_codec_codec_get_name(codec: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_codec_codec_is_still_image(codec: i32) -> bool;
	fn oak_codec_codec_is_lossless(codec: i32) -> bool;

	// exportformat
	fn oak_codec_format_is_valid(format: i32) -> bool;
	fn oak_codec_format_count() -> i32;
	fn oak_codec_format_get_name(format: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_codec_format_get_extension(format: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_codec_format_get_video_codecs(format: i32) -> *mut c_void;
	fn oak_codec_format_get_audio_codecs(format: i32) -> *mut c_void;

	// proxymanager
	fn oak_codec_proxy_params_default() -> ProxyParams;
	fn oak_codec_proxy_params_from_config() -> ProxyParams;
	fn oak_codec_proxy_get_state(proxy_filename: *const u8, len: usize) -> i32;
	fn oak_codec_proxy_filename_has_audio(proxy_filename: *const u8, len: usize) -> bool;
	fn oak_codec_proxy_get_filename(
		cache_path: *const u8,
		cache_path_len: usize,
		source_filename: *const u8,
		source_filename_len: usize,
		stream_index: i32,
		params: ProxyParams,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_codec_proxy_get_working_filename(
		proxy_filename: *const u8,
		len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_codec_proxy_get_or_start(
		cache_path: *const u8,
		cache_path_len: usize,
		source_filename: *const u8,
		source_filename_len: usize,
		stream_index: i32,
		params: ProxyParams,
	) -> OakCodecProxyResult;

	// task
	fn oak_codec_task_submit_is_registered() -> bool;
	fn oak_codec_task_set_submit_cb(cb: Option<TaskSubmitFn>, userdata: *mut c_void);
	fn oak_codec_task_submit(req: *const OakCodecTaskRequest) -> i32;

	// frame (the reference-counted CPU pixel buffer)
	fn oak_codec_frame_new() -> *mut c_void;
	fn oak_codec_frame_with_params(params: crate::types::VideoParams) -> *mut c_void;
	fn oak_codec_frame_allocate(this: *mut c_void) -> bool;
	fn oak_codec_frame_is_allocated(this: *const c_void) -> bool;
	fn oak_codec_frame_data(this: *mut c_void) -> *mut u8;
	fn oak_codec_frame_allocated_size(this: *const c_void) -> usize;
	fn oak_codec_frame_linesize_bytes(this: *const c_void) -> i32;
	fn oak_codec_frame_linesize_pixels(this: *const c_void) -> i32;
	fn oak_codec_frame_width(this: *const c_void) -> i32;
	fn oak_codec_frame_height(this: *const c_void) -> i32;
	fn oak_codec_frame_format(this: *const c_void) -> i32;
	fn oak_codec_frame_channel_count(this: *const c_void) -> i32;
	fn oak_codec_frame_timestamp(this: *const c_void) -> crate::types::Rational;
	fn oak_codec_frame_set_timestamp(this: *mut c_void, ts: crate::types::Rational);
	fn oak_codec_frame_get_params(this: *const c_void, out: *mut crate::types::VideoParams) -> bool;
	fn oak_codec_frame_set_params(this: *mut c_void, params: crate::types::VideoParams);
	fn oak_codec_frame_add_ref(this: *const c_void);
	fn oak_codec_frame_release(this: *const c_void);
}

/// A failed engine codec call. The C ABI carries no detail beyond the
/// failure itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the engine codec call failed")]
pub struct Error;

/// Why a [`ProxyParams`] string field was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyParamsError {
	/// Longer than the fixed 32-byte field allows (31 bytes + NUL).
	#[error("proxy params field too long: {0:?} (max 31 bytes)")]
	TooLong(String),
	/// C strings cannot contain NUL.
	#[error("proxy params field contains a NUL byte: {0:?}")]
	InteriorNul(String),
}

/// Runs an engine snprintf-style string getter to completion: query the
/// length, allocate, fetch. `None` on engine error (-1).
pub(crate) fn string_out(f: impl Fn(*mut u8, usize) -> i32) -> Option<String> {
	let n = f(std::ptr::null_mut(), 0);
	if n < 0 {
		return None;
	}
	let mut buf = vec![0u8; n as usize + 1];
	let written = f(buf.as_mut_ptr(), buf.len());
	debug_assert_eq!(written, n, "engine string changed between calls");
	buf.truncate(n as usize);
	String::from_utf8(buf).ok()
}

// ---------------------------------------------------------------------------
// Export codec
// ---------------------------------------------------------------------------

/// Export codecs, mirroring `oak_codec::exportcodec::Codec` (the C ABI
/// carries the `repr(i32)` discriminants).
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
	DNxHD = 0,
	H264 = 1,
	H264RGB = 2,
	H265 = 3,
	OpenEXR = 4,
	PNG = 5,
	ProRes = 6,
	CineForm = 7,
	TIFF = 8,
	VP9 = 9,
	MP2 = 10,
	MP3 = 11,
	AAC = 12,
	PCM = 13,
	Opus = 14,
	Vorbis = 15,
	FLAC = 16,
	SRT = 17,
	AV1 = 18,
}

impl Codec {
	/// The discriminant for a raw ABI value; `None` outside `0..=18`.
	pub fn from_i32(v: i32) -> Option<Codec> {
		if unsafe { oak_codec_codec_is_valid(v) } {
			// Validity was checked against the engine's own table.
			Some(unsafe { std::mem::transmute::<i32, Codec>(v) })
		} else {
			None
		}
	}

	/// The number of real codecs (the engine's `Count` sentinel).
	pub fn count() -> i32 {
		unsafe { oak_codec_codec_count() }
	}

	/// The display name (e.g. "H.264"). Infallible for valid codecs.
	pub fn name(self) -> String {
		string_out(|buf, len| unsafe { oak_codec_codec_get_name(self as i32, buf, len) })
			.unwrap_or_default()
	}

	/// Whether the codec produces still images (OpenEXR/PNG/TIFF).
	pub fn is_still_image(self) -> bool {
		unsafe { oak_codec_codec_is_still_image(self as i32) }
	}

	/// Whether the codec is lossless (PCM/FLAC).
	pub fn is_lossless(self) -> bool {
		unsafe { oak_codec_codec_is_lossless(self as i32) }
	}
}

// ---------------------------------------------------------------------------
// Export format
// ---------------------------------------------------------------------------

/// Export container formats, mirroring `oak_codec::exportformat::Format`.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
	DNxHD = 0,
	Matroska = 1,
	MPEG4Video = 2,
	OpenEXR = 3,
	QuickTime = 4,
	PNG = 5,
	TIFF = 6,
	WAV = 7,
	AIFF = 8,
	MP3 = 9,
	FLAC = 10,
	Ogg = 11,
	WebM = 12,
	SRT = 13,
	MPEG4Audio = 14,
}

impl Format {
	/// The discriminant for a raw ABI value; `None` outside `0..=14`.
	pub fn from_i32(v: i32) -> Option<Format> {
		if unsafe { oak_codec_format_is_valid(v) } {
			Some(unsafe { std::mem::transmute::<i32, Format>(v) })
		} else {
			None
		}
	}

	/// The number of real formats (the engine's `Count` sentinel).
	pub fn count() -> i32 {
		unsafe { oak_codec_format_count() }
	}

	/// The display name (e.g. "Matroska Video").
	pub fn name(self) -> String {
		string_out(|buf, len| unsafe { oak_codec_format_get_name(self as i32, buf, len) })
			.unwrap_or_default()
	}

	/// The file extension (e.g. "mkv").
	pub fn extension(self) -> String {
		string_out(|buf, len| unsafe { oak_codec_format_get_extension(self as i32, buf, len) })
			.unwrap_or_default()
	}

	/// The video codecs this format can carry, as raw discriminants in an
	/// engine vector; convert with [`Codec::from_i32`].
	pub fn video_codecs(self) -> VecI32 {
		unsafe { VecI32::from_raw(oak_codec_format_get_video_codecs(self as i32)) }
	}

	/// The audio codecs this format can carry (see [`Format::video_codecs`]).
	pub fn audio_codecs(self) -> VecI32 {
		unsafe { VecI32::from_raw(oak_codec_format_get_audio_codecs(self as i32)) }
	}
}

// ---------------------------------------------------------------------------
// Proxy manager
// ---------------------------------------------------------------------------

/// On-disk state of a proxy file.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyState {
	Missing = 0,
	Generating = 1,
	Ready = 2,
	Failed = 3,
}

impl ProxyState {
	fn from_i32(v: i32) -> Option<ProxyState> {
		match v {
			0 => Some(ProxyState::Missing),
			1 => Some(ProxyState::Generating),
			2 => Some(ProxyState::Ready),
			3 => Some(ProxyState::Failed),
			_ => None,
		}
	}
}

/// Proxy generation parameters. `repr(C)` mirror of the engine's
/// `ProxyParams`; crosses the ABI by value. The string fields are fixed
/// 32-byte NUL-terminated buffers, exactly like the C struct.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct ProxyParams {
	/// Absolute target width (0 when divider-based).
	pub width: i32,
	/// Absolute target height (0 when divider-based).
	pub height: i32,
	/// Source resolution divider (1 = absolute width/height, 2/4/8).
	pub divider: i32,
	/// Proxy format version.
	pub version: i32,
	/// x264 crf.
	pub crf: i32,
	/// Include the audio track (1/0).
	pub include_audio: i32,
	/// ffmpeg output container (NUL-terminated, e.g. "mp4").
	extension: [u8; 32],
	/// ffmpeg encoder preset (NUL-terminated, e.g. "veryfast").
	preset: [u8; 32],
}

impl Default for ProxyParams {
	fn default() -> Self {
		unsafe { oak_codec_proxy_params_default() }
	}
}

impl ProxyParams {
	/// The proxy parameters from the user's configuration (defaults when
	/// unset).
	pub fn from_config() -> Self {
		unsafe { oak_codec_proxy_params_from_config() }
	}

	fn read_cstr(field: &[u8; 32]) -> &str {
		let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
		std::str::from_utf8(&field[..end]).unwrap_or("")
	}

	fn write_cstr(field: &mut [u8; 32], value: &str) -> Result<(), ProxyParamsError> {
		if value.len() > 31 {
			return Err(ProxyParamsError::TooLong(value.to_string()));
		}
		let bytes = value.as_bytes();
		if bytes.contains(&0) {
			return Err(ProxyParamsError::InteriorNul(value.to_string()));
		}
		field.fill(0);
		field[..bytes.len()].copy_from_slice(bytes);
		Ok(())
	}

	pub fn extension(&self) -> &str {
		Self::read_cstr(&self.extension)
	}

	pub fn preset(&self) -> &str {
		Self::read_cstr(&self.preset)
	}

	pub fn set_extension(&mut self, value: &str) -> Result<(), ProxyParamsError> {
		Self::write_cstr(&mut self.extension, value)
	}

	pub fn set_preset(&mut self, value: &str) -> Result<(), ProxyParamsError> {
		Self::write_cstr(&mut self.preset, value)
	}
}

/// The on-disk proxy state for `proxy_filename`. Null/invalid input
/// reads as `Missing` on the engine side; from Rust this always
/// succeeds.
pub fn proxy_get_state(proxy_filename: &str) -> ProxyState {
	let v = unsafe { oak_codec_proxy_get_state(proxy_filename.as_ptr(), proxy_filename.len()) };
	ProxyState::from_i32(v).unwrap_or(ProxyState::Missing)
}

/// Whether the proxy file carries an audio stream.
pub fn proxy_filename_has_audio(proxy_filename: &str) -> bool {
	unsafe { oak_codec_proxy_filename_has_audio(proxy_filename.as_ptr(), proxy_filename.len()) }
}

/// Derives the proxy filename for a source stream; `None` on engine
/// error.
pub fn proxy_get_filename(
	cache_path: &str,
	source_filename: &str,
	stream_index: i32,
	params: &ProxyParams,
) -> Option<String> {
	string_out(|buf, len| unsafe {
		oak_codec_proxy_get_filename(
			cache_path.as_ptr(),
			cache_path.len(),
			source_filename.as_ptr(),
			source_filename.len(),
			stream_index,
			params.clone(),
			buf,
			len,
		)
	})
}

/// The working (in-progress) filename for a proxy; `None` on engine
/// error.
pub fn proxy_get_working_filename(proxy_filename: &str) -> Option<String> {
	string_out(|buf, len| unsafe {
		oak_codec_proxy_get_working_filename(proxy_filename.as_ptr(), proxy_filename.len(), buf, len)
	})
}

/// `repr(C)` mirror of the engine's `OakCodecProxyResult`.
#[repr(C)]
struct OakCodecProxyResult {
	state: i32,
	filename: [u8; 1024],
}

/// The outcome of [`proxy_get_or_start`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyResult {
	pub state: ProxyState,
	pub filename: String,
}

/// Returns the proxy for a source stream, starting generation through
/// the registered task callback when it is missing.
pub fn proxy_get_or_start(
	cache_path: &str,
	source_filename: &str,
	stream_index: i32,
	params: &ProxyParams,
) -> Result<ProxyResult, Error> {
	let raw = unsafe {
		oak_codec_proxy_get_or_start(
			cache_path.as_ptr(),
			cache_path.len(),
			source_filename.as_ptr(),
			source_filename.len(),
			stream_index,
			params.clone(),
		)
	};
	let state = ProxyState::from_i32(raw.state).ok_or(Error)?;
	let end = raw
		.filename
		.iter()
		.position(|&b| b == 0)
		.unwrap_or(raw.filename.len());
	let filename = String::from_utf8(raw.filename[..end].to_vec()).map_err(|_| Error)?;
	Ok(ProxyResult { state, filename })
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

/// Kinds of background tasks the codec module can request.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
	Conform = 0,
	Proxy = 1,
}

/// One background task request. Strings are borrowed for the submit
/// call only.
#[derive(Clone, Debug)]
pub struct TaskRequest<'a> {
	pub kind: TaskKind,
	pub input_filename: &'a str,
	pub output_filename: &'a str,
	pub stream_index: i32,
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub sample_format: i32,
	pub proxy_width: i32,
	pub proxy_height: i32,
}

/// `repr(C)` mirror of the engine's `OakCodecTaskRequest`.
#[repr(C)]
struct OakCodecTaskRequest {
	kind: i32,
	input_filename: *const std::ffi::c_char,
	output_filename: *const std::ffi::c_char,
	stream_index: i32,
	sample_rate: i32,
	channel_layout: u64,
	sample_format: i32,
	proxy_width: i32,
	proxy_height: i32,
}

type TaskSubmitFn = unsafe extern "C" fn(req: *const OakCodecTaskRequest, userdata: *mut c_void) -> i32;

fn borrow_cstr<'a>(p: *const std::ffi::c_char) -> &'a str {
	if p.is_null() {
		return "";
	}
	unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
}

fn request_from_c(raw: &OakCodecTaskRequest) -> TaskRequest<'_> {
	TaskRequest {
		kind: if raw.kind == 1 { TaskKind::Proxy } else { TaskKind::Conform },
		input_filename: borrow_cstr(raw.input_filename),
		output_filename: borrow_cstr(raw.output_filename),
		stream_index: raw.stream_index,
		sample_rate: raw.sample_rate,
		channel_layout: raw.channel_layout,
		sample_format: raw.sample_format,
		proxy_width: raw.proxy_width,
		proxy_height: raw.proxy_height,
	}
}

/// Whether a task submit callback is currently registered.
pub fn task_submit_is_registered() -> bool {
	unsafe { oak_codec_task_submit_is_registered() }
}

/// Registers (or replaces, or with `None` clears) the process-wide task
/// submit callback. The closure receives a borrowed [`TaskRequest`]
/// valid only for the call duration and returns 0 to accept or a
/// negative code to reject.
///
/// Registration is process-lifetime: re-registering leaks the previous
/// closure (the common case registers exactly once, at startup).
pub fn set_task_submit_cb<F>(cb: Option<F>)
where
	F: Fn(&TaskRequest) -> i32 + Send + 'static,
{
	match cb {
		None => unsafe { oak_codec_task_set_submit_cb(None, std::ptr::null_mut()) },
		Some(f) => {
			unsafe extern "C" fn trampoline<F>(
				req: *const OakCodecTaskRequest,
				userdata: *mut c_void,
			) -> i32
			where
				F: Fn(&TaskRequest) -> i32 + Send + 'static,
			{
				if req.is_null() || userdata.is_null() {
					return -1;
				}
				let f = unsafe { &*(userdata as *const F) };
				let req = unsafe { &*req };
				f(&request_from_c(req))
			}
			let f: *mut F = Box::into_raw(Box::new(f));
			unsafe { oak_codec_task_set_submit_cb(Some(trampoline::<F>), f.cast()) };
		}
	}
}

/// Submits one task request. `Ok(true)` when the task was accepted,
/// `Ok(false)` when no callback is registered, `Err` on invalid input
/// (filename containing NUL) or engine error.
pub fn task_submit(req: &TaskRequest) -> Result<bool, Error> {
	let input = CString::new(req.input_filename).map_err(|_| Error)?;
	let output = CString::new(req.output_filename).map_err(|_| Error)?;
	let raw = OakCodecTaskRequest {
		kind: req.kind as i32,
		input_filename: input.as_ptr(),
		output_filename: output.as_ptr(),
		stream_index: req.stream_index,
		sample_rate: req.sample_rate,
		channel_layout: req.channel_layout,
		sample_format: req.sample_format,
		proxy_width: req.proxy_width,
		proxy_height: req.proxy_height,
	};
	match unsafe { oak_codec_task_submit(&raw) } {
		-1 => Err(Error),
		0 => Ok(false),
		_ => Ok(true),
	}
}

// ---------------------------------------------------------------------------
// Frame (reference-counted CPU pixel buffer)
// ---------------------------------------------------------------------------

use crate::types::{PixelFormat, Rational, VideoParams};

/// A reference-counted CPU pixel buffer; the plugin I/O carrier.
///
/// `Clone` adds a reference, `Drop` releases. Mutation (`allocate`,
/// `set_params`, `set_timestamp`, writing through `data`) requires
/// holding the only live clone — the same discipline the engine's C++
/// Frame has. A `data` pointer stays valid until the next `allocate` or
/// `set_params`, or the last drop.
pub struct Frame {
	ptr: *mut c_void,
}

// Arc-backed on the engine side; pixel buffers are plain memory.
unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}

impl Frame {
	/// An empty frame with default (invalid) params.
	pub fn new() -> Self {
		Self {
			ptr: unsafe { oak_codec_frame_new() },
		}
	}

	/// A frame with a copy of `params` (line sizes computed).
	pub fn with_params(params: &VideoParams) -> Self {
		Self {
			ptr: unsafe { oak_codec_frame_with_params(params.clone()) },
		}
	}

	/// Allocates the pixel buffer per the current params.
	pub fn allocate(&mut self) -> Result<(), Error> {
		if unsafe { oak_codec_frame_allocate(self.ptr) } {
			Ok(())
		} else {
			Err(Error)
		}
	}

	pub fn is_allocated(&self) -> bool {
		unsafe { oak_codec_frame_is_allocated(self.ptr) }
	}

	/// The pixel buffer; `None` until allocated. See the type docs for
	/// the validity window.
	pub fn data(&mut self) -> Option<&mut [u8]> {
		let ptr = unsafe { oak_codec_frame_data(self.ptr) };
		if ptr.is_null() {
			return None;
		}
		let len = self.allocated_size();
		Some(unsafe { std::slice::from_raw_parts_mut(ptr, len) })
	}

	/// The allocated buffer size in bytes (0 when unallocated).
	pub fn allocated_size(&self) -> usize {
		unsafe { oak_codec_frame_allocated_size(self.ptr) }
	}

	pub fn linesize_bytes(&self) -> i32 {
		unsafe { oak_codec_frame_linesize_bytes(self.ptr) }
	}

	pub fn linesize_pixels(&self) -> i32 {
		unsafe { oak_codec_frame_linesize_pixels(self.ptr) }
	}

	pub fn width(&self) -> i32 {
		unsafe { oak_codec_frame_width(self.ptr) }
	}

	pub fn height(&self) -> i32 {
		unsafe { oak_codec_frame_height(self.ptr) }
	}

	/// The allocated pixel format.
	pub fn format(&self) -> PixelFormat {
		PixelFormat::from_i32(unsafe { oak_codec_frame_format(self.ptr) })
			.unwrap_or(PixelFormat::Invalid)
	}

	pub fn channel_count(&self) -> i32 {
		unsafe { oak_codec_frame_channel_count(self.ptr) }
	}

	/// The frame timestamp (rational seconds).
	pub fn timestamp(&self) -> Rational {
		unsafe { oak_codec_frame_timestamp(self.ptr) }
	}

	pub fn set_timestamp(&mut self, ts: Rational) {
		unsafe { oak_codec_frame_set_timestamp(self.ptr, ts) };
	}

	/// A copy of the frame's params.
	pub fn params(&self) -> Option<VideoParams> {
		let mut out = VideoParams::new();
		if unsafe { oak_codec_frame_get_params(self.ptr, &mut out) } {
			Some(out)
		} else {
			None
		}
	}

	/// Replaces the frame's params (recomputes line sizes, does NOT
	/// reallocate the buffer).
	pub fn set_params(&mut self, params: &VideoParams) {
		unsafe { oak_codec_frame_set_params(self.ptr, params.clone()) };
	}
}

impl Default for Frame {
	fn default() -> Self {
		Self::new()
	}
}

impl Clone for Frame {
	fn clone(&self) -> Self {
		unsafe { oak_codec_frame_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for Frame {
	fn drop(&mut self) {
		unsafe { oak_codec_frame_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;

	// The task-submit registry is process-global; serialize the tests
	// that touch it.
	static SERIAL: Mutex<()> = Mutex::new(());

	fn proxy_task<'a>(input: &'a str, output: &'a str) -> TaskRequest<'a> {
		TaskRequest {
			kind: TaskKind::Proxy,
			input_filename: input,
			output_filename: output,
			stream_index: 3,
			sample_rate: 0,
			channel_layout: 0,
			sample_format: 0,
			proxy_width: 640,
			proxy_height: 360,
		}
	}

	fn temp_cache(name: &str) -> String {
		let dir = std::env::temp_dir().join(format!(
			"oakengine_sdk_proxy_{name}_{}",
			std::process::id()
		));
		let _ = std::fs::create_dir_all(&dir);
		dir.to_string_lossy().into_owned()
	}

	#[test]
	fn codec_enum_roundtrip_and_flags() {
		assert_eq!(Codec::count(), 19);
		assert_eq!(Codec::from_i32(1), Some(Codec::H264));
		assert_eq!(Codec::from_i32(19), None);
		assert_eq!(Codec::from_i32(-1), None);
		assert_eq!(Codec::H264.name(), "H.264");
		assert!(Codec::OpenEXR.is_still_image());
		assert!(!Codec::H264.is_still_image());
		assert!(Codec::FLAC.is_lossless());
	}

	#[test]
	fn format_strings_and_codec_lists() {
		assert_eq!(Format::count(), 15);
		assert_eq!(Format::from_i32(1), Some(Format::Matroska));
		assert_eq!(Format::from_i32(15), None);
		assert_eq!(Format::Matroska.name(), "Matroska Video");
		assert_eq!(Format::Matroska.extension(), "mkv");

		let video = Format::Matroska.video_codecs();
		assert!(video.to_vec().contains(&(Codec::H264 as i32)));
		let audio = Format::Matroska.audio_codecs();
		assert!(audio.to_vec().contains(&(Codec::AAC as i32)));
	}

	#[test]
	fn proxy_params_roundtrip_and_validation() {
		let mut p = ProxyParams::default();
		assert_eq!((p.width, p.height, p.divider), (1280, 720, 1));
		assert_eq!(p.extension(), "mp4");
		assert_eq!(p.preset(), "veryfast");

		p.set_extension("mkv").unwrap();
		assert_eq!(p.extension(), "mkv");
		assert!(p.set_extension(&"x".repeat(32)).is_err());
		assert!(p.set_preset("a\0b").is_err());
	}

	#[test]
	fn proxy_state_and_filenames() {
		assert_eq!(
			proxy_get_state("/nonexistent/oak-sdk-proxy.mp4"),
			ProxyState::Missing
		);
		let params = ProxyParams::default();
		let cache = std::env::temp_dir().to_string_lossy().into_owned();
		let filename =
			proxy_get_filename(&cache, "oak-sdk-src.mp4", 0, &params).expect("filename");
		assert!(filename.ends_with(".v1.a1.mp4"), "unexpected: {filename}");
		let working = proxy_get_working_filename(&filename).expect("working");
		assert!(working.ends_with(".working.mp4"));
	}

	#[test]
	fn proxy_get_or_start_states() {
		let _guard = SERIAL.lock().unwrap();
		set_task_submit_cb::<fn(&TaskRequest) -> i32>(None);

		let cache = temp_cache("missing");
		let result = proxy_get_or_start(&cache, "/tmp/oak-sdk-gos.mp4", 0, &ProxyParams::default())
			.expect("call");
		assert_eq!(result.state, ProxyState::Missing);
		assert!(!result.filename.is_empty());

		// A registered, accepting callback flips it to Generating.
		set_task_submit_cb(Some(|_req: &TaskRequest| 0));
		let cache = temp_cache("generating");
		let result = proxy_get_or_start(&cache, "/tmp/oak-sdk-gos.mp4", 0, &ProxyParams::default())
			.expect("call");
		assert_eq!(result.state, ProxyState::Generating);
		set_task_submit_cb::<fn(&TaskRequest) -> i32>(None);
	}

	#[test]
	fn task_submit_without_and_with_registrar() {
		let _guard = SERIAL.lock().unwrap();
		set_task_submit_cb::<fn(&TaskRequest) -> i32>(None);
		assert!(!task_submit_is_registered());
		assert_eq!(task_submit(&proxy_task("/tmp/a.mp4", "/tmp/a.p.mp4")), Ok(false));

		use std::sync::atomic::{AtomicUsize, Ordering};
		static CALLS: AtomicUsize = AtomicUsize::new(0);
		CALLS.store(0, Ordering::SeqCst);
		set_task_submit_cb(Some(move |req: &TaskRequest| {
			CALLS.fetch_add(1, Ordering::SeqCst);
			assert_eq!(req.kind, TaskKind::Proxy);
			assert_eq!(req.input_filename, "/tmp/a.mp4");
			assert_eq!(req.stream_index, 3);
			0
		}));
		assert!(task_submit_is_registered());
		assert_eq!(task_submit(&proxy_task("/tmp/a.mp4", "/tmp/a.p.mp4")), Ok(true));
		assert_eq!(CALLS.load(Ordering::SeqCst), 1);

		set_task_submit_cb::<fn(&TaskRequest) -> i32>(None);
		assert!(!task_submit_is_registered());
	}

	#[test]
	fn task_submit_rejects_nul_filenames() {
		let _guard = SERIAL.lock().unwrap();
		set_task_submit_cb::<fn(&TaskRequest) -> i32>(None);
		let req = proxy_task("/tmp/a\0b.mp4", "/tmp/a.p.mp4");
		assert!(task_submit(&req).is_err());
	}

	#[test]
	fn frame_lifecycle() {
		let mut params = crate::types::VideoParams::new();
		params.width = 64;
		params.height = 32;
		params.format = crate::types::PixelFormat::F32 as i32;
		params.channel_count = 4;

		let mut frame = Frame::with_params(&params);
		assert!(!frame.is_allocated());
		frame.allocate().expect("allocate");
		assert_eq!((frame.width(), frame.height()), (64, 32));
		assert_eq!(frame.format(), crate::types::PixelFormat::F32);
		assert_eq!(frame.allocated_size(), 64 * 32 * 4 * 4);
		assert_eq!(frame.linesize_bytes(), 64 * 4 * 4);

		frame.data().expect("data")[..16].fill(0xAB);
		frame.set_timestamp(crate::types::Rational::new(3, 2));
		assert_eq!(frame.timestamp(), crate::types::Rational::new(3, 2));
		assert_eq!(frame.params().map(|p| p.width), Some(64));

		// Clones share the buffer until the last drop.
		let clone = frame.clone();
		drop(frame);
		assert!(clone.is_allocated());
	}
}

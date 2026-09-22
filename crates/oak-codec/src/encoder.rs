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

//! `olive::Encoder` — abstract base for media encoders.
//!
//! Mirrors `src/codec/src/encoder.h`. Implementations are
//! [`crate::ffmpeg::FFmpegEncoder`] and [`crate::oiio::OIIOEncoder`]. The
//! workflow (C ABI encoder.h) is: fill an `EncodingParams` → init → open →
//! write_video/audio/subtitle → flush. The trait mirrors the C++ virtual
//! surface.

use std::sync::{Arc, Mutex, OnceLock};

use oak_core::{PixelFormat, SampleFormat};

use crate::encodingparams::EncodingParams;
use crate::frame::Frame;

/// `olive::Encoder` — encoder trait. Instances are handed out as
/// `Arc<dyn Encoder>`.
pub trait Encoder: Send + Sync {
	/// Unique encoder id.
	fn id(&self) -> String;

	/// Whether this encoder writes video.
	fn supports_video(&self) -> bool {
		false
	}

	/// Whether this encoder writes audio.
	fn supports_audio(&self) -> bool {
		false
	}

	/// Whether this encoder writes subtitles.
	fn supports_subtitles(&self) -> bool {
		false
	}

	/// Whether this encoder writes an image sequence.
	fn supports_image_sequences(&self) -> bool {
		false
	}

	/// Whether this encoder is deterministic for a given config
	/// (used for cache keys).
	fn is_configurable(&self) -> bool {
		false
	}

	/// Configure the encoder (per-codec options like `crf`).
	fn configure(&self, params: &EncodingParams) -> crate::error::Result<()>;

	/// Open the output file and write headers.
	fn open(&self) -> crate::error::Result<()>;

	/// Close the output (write trailer); idempotent.
	fn close(&self) -> crate::error::Result<()>;

	/// Encode one video frame (converts delivery pixel format internally).
	fn write_video(&self, frame: &Frame) -> crate::error::Result<()>;

	/// Encode interleaved float audio samples.
	fn write_audio(&self, samples: &[f32], frame_count: i32) -> crate::error::Result<()>;

	/// Encode one subtitle entry (times in seconds).
	fn write_subtitle(
		&self,
		text: &str,
		in_seconds: f64,
		out_seconds: f64,
	) -> crate::error::Result<()>;

	/// Flush encoders, write the trailer, close the file.
	fn flush(&self) -> crate::error::Result<()>;

	/// The pixel format the encoder wants frames in (or `None`).
	fn desired_pixel_format(&self) -> Option<PixelFormat>;

	/// The sample format the encoder wants audio in (or `None`).
	fn desired_sample_format(&self) -> Option<SampleFormat>;

	/// The configured output filename.
	fn filename(&self) -> String;

	/// Human-readable detail of the last failed operation (empty when
	/// none). Mirrors the C++ `Encoder::get_error()` used by
	/// `oakcodec_encoder_last_error`.
	fn get_error(&self) -> String {
		String::new()
	}
}

/// Test-injected encoder registry (see [`set_test_encoders`]); empty when
/// not injected, in which case [`create_from_params`] falls back to the
/// built-in format mapping.
static TEST_ENCODERS: OnceLock<Mutex<Vec<Arc<dyn Encoder>>>> = OnceLock::new();

/// Replace the encoder registry with `list`; pass an empty list to restore
/// the built-in behavior.
///
/// Test/extension support (the C ABI has no way to register an encoder, so
/// the contract tests drive the encode state machine through a fake
/// encoder). Hidden from docs; never called by production code.
#[doc(hidden)]
pub fn set_test_encoders(list: Vec<Arc<dyn Encoder>>) {
	let store = TEST_ENCODERS.get_or_init(|| Mutex::new(Vec::new()));
	*store.lock().unwrap() = list;
}

/// Serializes every test that reads the built-in encoder registry and
/// clears the test injection on drop: a panicking assertion must not leak
/// fake encoders into the process-wide registry used by later tests. The
/// held [`crate::TestLock`] is released only after the clear.
#[cfg(test)]
struct RegistryGuard {
	/// Held for the test's duration; never read, only dropped.
	_lock: crate::TestLock,
}

#[cfg(test)]
impl Drop for RegistryGuard {
	fn drop(&mut self) {
		set_test_encoders(Vec::new());
	}
}

/// Take the shared test lock and return the guard.
#[cfg(test)]
fn registry_guard() -> RegistryGuard {
	RegistryGuard {
		_lock: crate::lock_tests(),
	}
}

/// `Encoder::create_from_params` — instantiate an encoder for `params`.
///
/// # CPP-PARITY
/// `src/codec/src/encoder.cpp` `create_from_params` → `create_from_format`
/// picks the FFmpeg/OIIO implementation from `params.format` (DNxHD,
/// Matroska, QuickTime, MPEG-4 video/audio, WAV, AIFF, MP3, FLAC, Ogg,
/// WebM, SRT → FFmpeg; OpenEXR, PNG, TIFF → OIIO; anything else → `None`).
/// A non-empty test-injected list (see [`set_test_encoders`]) wins over the
/// built-in mapping. The concrete implementations are dylib stubs whose
/// `open()` fails with a clear message, so an initialized encoder is always
/// constructible for a recognized format.
pub fn create_from_params(params: &EncodingParams) -> Option<Arc<dyn Encoder>> {
	if let Some(store) = TEST_ENCODERS.get() {
		let injected = store.lock().unwrap();
		if !injected.is_empty() {
			return injected.first().cloned();
		}
	}
	match encoder_type_from_format(params.format) {
		Some(EncoderType::FFmpeg) => Some(Arc::new(crate::ffmpeg::FFmpegEncoder::with_params(
			params.clone(),
		))),
		Some(EncoderType::OIIO) => Some(Arc::new(crate::oiio::OIIOEncoder {
			params: params.clone(),
		})),
		None => None,
	}
}

/// `Encoder::Type` mirror (`encoder.cpp` `get_type_from_format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EncoderType {
	/// `k_encoder_type_f_fmpeg`.
	FFmpeg,
	/// `k_encoder_type_oiio`.
	OIIO,
}

/// `Encoder::get_type_from_format` — the implementation family for an
/// `ExportFormat::Format` int; `None` for unknown/`Count`.
fn encoder_type_from_format(format: i32) -> Option<EncoderType> {
	match format {
		// FFmpeg-backed containers.
		0 | 1 | 2 | 4 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 => Some(EncoderType::FFmpeg),
		// OIIO-backed still-image formats.
		3 | 5 | 6 => Some(EncoderType::OIIO),
		_ => None,
	}
}

/// Range `(start, end)` of a "[#####]" digit placeholder beginning at
/// `pos` (`bytes[pos] == '['`), or `None`.
///
/// CPP-PARITY: `encoder.cpp` `k_image_sequence_contains_digits` regex
/// `\[[#]+\]` — a `[`, one or more `#`, then `]`.
fn placeholder_range(bytes: &[u8], pos: usize) -> Option<(usize, usize)> {
	if bytes.get(pos) != Some(&b'[') {
		return None;
	}
	let mut j = pos + 1;
	while bytes.get(j) == Some(&b'#') {
		j += 1;
	}
	if j > pos + 1 && bytes.get(j) == Some(&b']') {
		Some((pos, j + 1))
	} else {
		None
	}
}

/// `Encoder::filename_contains_digit_placeholder` — whether `filename`
/// contains a "[#####]" digit placeholder.
///
/// CPP-PARITY: `encoder.cpp:137` (`std::regex_search` on
/// `k_image_sequence_contains_digits`).
pub fn filename_contains_digit_placeholder(filename: &str) -> bool {
	let bytes = filename.as_bytes();
	(0..bytes.len()).any(|i| placeholder_range(bytes, i).is_some())
}

/// `Encoder::get_image_sequence_placeholder_digit_count` — number of `#` in
/// the filename's "[#####]" placeholder; 0 when none.
///
/// CPP-PARITY: `encoder.cpp:119` — the C++ finds the first
/// `k_image_sequence_contains_digits` match and counts its `#`s, which is
/// exactly the match length minus the two brackets.
pub fn image_sequence_placeholder_digit_count(filename: &str) -> i32 {
	let bytes = filename.as_bytes();
	for i in 0..bytes.len() {
		if let Some((start, end)) = placeholder_range(bytes, i) {
			return (end - start - 2) as i32;
		}
	}
	0
}

/// `Encoder::filename_remove_digit_placeholder` — `filename` with every
/// "[#####]" placeholder removed; an optional single separator char
/// (`-`, `.`, ` `, `_`) immediately before the placeholder goes with it.
///
/// CPP-PARITY: `encoder.cpp:142` (`std::regex_replace` on
/// `k_image_sequence_remove_digits` = `[\-\.\ \_]?\[[#]+\]`, empty
/// replacement, all matches).
pub fn filename_remove_digit_placeholder(filename: &str) -> String {
	let bytes = filename.as_bytes();
	let mut out = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		// A separator is consumed only when a placeholder follows it.
		let ph_start = match bytes[i] {
			b'-' | b'.' | b' ' | b'_' if placeholder_range(bytes, i + 1).is_some() => i + 1,
			_ => i,
		};
		match placeholder_range(bytes, ph_start) {
			Some((_, end)) => i = end,
			None => {
				out.push(bytes[i]);
				i += 1;
			}
		}
	}
	String::from_utf8(out).unwrap_or_else(|_| filename.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn create_from_params_maps_formats() {
		let _guard = registry_guard();
		let mut p = EncodingParams::default();

		// FFmpeg-backed containers.
		for fmt in [0, 1, 2, 4, 7, 8, 9, 10, 11, 12, 13, 14] {
			p.format = fmt;
			let e = create_from_params(&p).expect("format {fmt}");
			assert_eq!(e.id(), "ffmpeg", "format {fmt}");
		}
		// OIIO-backed still images.
		for fmt in [3, 5, 6] {
			p.format = fmt;
			let e = create_from_params(&p).expect("format {fmt}");
			assert_eq!(e.id(), "oiio", "format {fmt}");
		}
		// Unknown / Count -> None (C++ `k_encoder_type_none`).
		p.format = 15;
		assert!(create_from_params(&p).is_none());
		p.format = -1;
		assert!(create_from_params(&p).is_none());
	}

	#[test]
	fn get_error_defaults_to_empty() {
		let e = UnimplementedDummy;
		assert_eq!(e.get_error(), "");
	}

	#[test]
	fn image_sequence_placeholder_helpers() {
		// contains: "[#####]" style placeholder only.
		assert!(filename_contains_digit_placeholder("/tmp/out_[#####].png"));
		assert!(filename_contains_digit_placeholder("out[#].png"));
		assert!(!filename_contains_digit_placeholder("/tmp/out.png"));
		assert!(!filename_contains_digit_placeholder("out[####.png"));
		assert!(!filename_contains_digit_placeholder("out[].png"));

		// digit count: number of '#' in the first placeholder.
		assert_eq!(
			image_sequence_placeholder_digit_count("/tmp/out_[#####].png"),
			5
		);
		assert_eq!(image_sequence_placeholder_digit_count("out[#].png"), 1);
		assert_eq!(image_sequence_placeholder_digit_count("a[##]b[####]c"), 2);
		assert_eq!(image_sequence_placeholder_digit_count("/tmp/out.png"), 0);

		// remove: separator char before the placeholder goes with it.
		assert_eq!(
			filename_remove_digit_placeholder("/tmp/out_[#####].png"),
			"/tmp/out.png"
		);
		assert_eq!(filename_remove_digit_placeholder("out[###].png"), "out.png");
		assert_eq!(filename_remove_digit_placeholder("a_[#]b_[###]c"), "abc");
		assert_eq!(
			filename_remove_digit_placeholder("/tmp/out.png"),
			"/tmp/out.png"
		);
	}

	struct UnimplementedDummy;

	impl Encoder for UnimplementedDummy {
		fn id(&self) -> String {
			"dummy".to_string()
		}
		fn configure(&self, _p: &EncodingParams) -> crate::error::Result<()> {
			Ok(())
		}
		fn open(&self) -> crate::error::Result<()> {
			Ok(())
		}
		fn close(&self) -> crate::error::Result<()> {
			Ok(())
		}
		fn write_video(&self, _f: &Frame) -> crate::error::Result<()> {
			Ok(())
		}
		fn write_audio(&self, _s: &[f32], _c: i32) -> crate::error::Result<()> {
			Ok(())
		}
		fn write_subtitle(&self, _t: &str, _i: f64, _o: f64) -> crate::error::Result<()> {
			Ok(())
		}
		fn flush(&self) -> crate::error::Result<()> {
			Ok(())
		}
		fn desired_pixel_format(&self) -> Option<PixelFormat> {
			None
		}
		fn desired_sample_format(&self) -> Option<SampleFormat> {
			None
		}
		fn filename(&self) -> String {
			String::new()
		}
	}

	#[test]
	fn encoder_trait_defaults_are_conservative() {
		let e = UnimplementedDummy;
		assert!(!e.supports_video());
		assert!(!e.supports_audio());
		assert!(!e.supports_subtitles());
		assert!(!e.supports_image_sequences());
		assert!(!e.is_configurable());
	}

	#[test]
	fn encoder_registry_injection_wins_and_restores() {
		let _guard = registry_guard();
		// Unknown format: the built-in mapping returns None.
		let p = EncodingParams {
			format: 999,
			..EncodingParams::default()
		};
		assert!(create_from_params(&p).is_none());

		set_test_encoders(vec![Arc::new(UnimplementedDummy)]);
		let injected = create_from_params(&p).expect("injected encoder wins");
		assert_eq!(injected.id(), "dummy");

		set_test_encoders(Vec::new());
		assert!(create_from_params(&p).is_none());
	}

	/// The injection restore is panic-safe: the guard clears the registry
	/// on unwind, so a failing assertion cannot leave the fake encoder
	/// installed for later tests in the same process.
	#[test]
	fn encoder_registry_clears_after_a_panicking_injection() {
		let panicked = std::panic::catch_unwind(|| {
			let _guard = registry_guard();
			set_test_encoders(vec![Arc::new(UnimplementedDummy)]);
			let p = EncodingParams {
				format: 999,
				..EncodingParams::default()
			};
			assert_eq!(
				create_from_params(&p).map(|e| e.id()).as_deref(),
				Some("dummy"),
				"the fake encoder is installed"
			);
			panic!("simulated assertion failure after injection");
		});
		assert!(panicked.is_err(), "the closure must panic");
		let _guard = registry_guard();
		let p = EncodingParams {
			format: 999,
			..EncodingParams::default()
		};
		assert!(
			create_from_params(&p).is_none(),
			"no fake encoder may leak into the next test"
		);
	}
}

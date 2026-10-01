//! C ABI for `oak_codec::exportformat::Format` (prefix
//! `oak_codec_format_`). Formats cross as `repr(i32)` discriminants;
//! codec lists come back as `OakVecI32` handles (released with
//! `oak_vec_i32_release`).

use oak_codec::exportformat::Format;

use crate::vecs::{IntoFfiVec, OakVecI32};

/// Whether `format` is a valid Format discriminant.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_is_valid(format: i32) -> bool {
	Format::from_i32(format).is_some()
}

/// The number of real formats (the `Count` sentinel value).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_count() -> i32 {
	Format::Count as i32
}

/// The display name of `format`, snprintf-style (see `copy_str_out`);
/// -1 when `format` is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_get_name(format: i32, buf: *mut u8, buf_len: usize) -> i32 {
	let Some(format) = Format::from_i32(format) else {
		return -1;
	};
	super::copy_str_out(&Format::get_name(format), buf, buf_len)
}

/// The file extension of `format` (e.g. "mkv"), snprintf-style;
/// -1 when `format` is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_get_extension(
	format: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let Some(format) = Format::from_i32(format) else {
		return -1;
	};
	super::copy_str_out(&Format::get_extension(format), buf, buf_len)
}

/// The video codecs `format` can carry, as codec discriminants in an
/// `OakVecI32`. Invalid formats yield an empty vector (never null).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_get_video_codecs(format: i32) -> *mut OakVecI32 {
	let codecs = Format::from_i32(format)
		.map(Format::get_video_codecs)
		.unwrap_or_default();
	codecs
		.into_iter()
		.map(|c| c as i32)
		.collect::<Vec<i32>>()
		.into_ffi_vec()
}

/// The audio codecs `format` can carry (see
/// [`oak_codec_format_get_video_codecs`]).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_format_get_audio_codecs(format: i32) -> *mut OakVecI32 {
	let codecs = Format::from_i32(format)
		.map(Format::get_audio_codecs)
		.unwrap_or_default();
	codecs
		.into_iter()
		.map(|c| c as i32)
		.collect::<Vec<i32>>()
		.into_ffi_vec()
}

#[cfg(test)]
mod tests {
	use super::*;

	unsafe extern "C" {
		#[link_name = "oak_vec_i32_len"]
		fn vec_len(this: *const OakVecI32) -> usize;
		#[link_name = "oak_vec_i32_get"]
		fn vec_get(this: *const OakVecI32, index: usize, out: *mut i32) -> bool;
		#[link_name = "oak_vec_i32_release"]
		unsafe fn vec_release(this: *const OakVecI32);
	}

	unsafe fn vec_to_std(v: *mut OakVecI32) -> Vec<i32> {
		let mut out = Vec::new();
		unsafe {
			for i in 0..vec_len(v) {
				let mut e = 0;
				assert!(vec_get(v, i, &mut e));
				out.push(e);
			}
			vec_release(v);
		}
		out
	}

	#[test]
	fn validity_count_and_strings() {
		assert!(oak_codec_format_is_valid(0));
		assert!(oak_codec_format_is_valid(14));
		assert!(!oak_codec_format_is_valid(15));
		assert_eq!(oak_codec_format_count(), 15);

		let mut buf = [0u8; 32];
		let n = oak_codec_format_get_name(1, buf.as_mut_ptr(), buf.len());
		assert_eq!(n, "Matroska Video".len() as i32);
		assert_eq!(&buf[..=n as usize], b"Matroska Video\0");

		let n = oak_codec_format_get_extension(1, buf.as_mut_ptr(), buf.len());
		assert_eq!(n, 3);
		assert_eq!(&buf[..=n as usize], b"mkv\0");

		assert_eq!(oak_codec_format_get_name(42, buf.as_mut_ptr(), buf.len()), -1);
	}

	#[test]
	fn codec_lists_for_matroska() {
		let video = unsafe { vec_to_std(oak_codec_format_get_video_codecs(1)) };
		assert!(!video.is_empty());
		assert!(video.contains(&1), "Matroska should carry H.264: {video:?}");

		let audio = unsafe { vec_to_std(oak_codec_format_get_audio_codecs(1)) };
		assert!(audio.contains(&12), "Matroska should carry AAC: {audio:?}");

		let invalid = unsafe { vec_to_std(oak_codec_format_get_video_codecs(99)) };
		assert!(invalid.is_empty());
	}
}

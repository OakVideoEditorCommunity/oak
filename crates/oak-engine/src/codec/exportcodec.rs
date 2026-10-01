//! C ABI for `oak_codec::exportcodec::Codec` (prefix `oak_codec_codec_`).
//! Codecs cross the boundary as their `repr(i32)` discriminant values;
//! the C header defines the same enum.

use oak_codec::exportcodec::Codec;

/// Whether `codec` is a valid Codec discriminant.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_codec_is_valid(codec: i32) -> bool {
	Codec::from_i32(codec).is_some()
}

/// The number of real codecs (the `Count` sentinel value).
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_codec_count() -> i32 {
	Codec::Count as i32
}

/// The display name of `codec`, snprintf-style (see `copy_str_out`);
/// -1 when `codec` is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_codec_get_name(codec: i32, buf: *mut u8, buf_len: usize) -> i32 {
	let Some(codec) = Codec::from_i32(codec) else {
		return -1;
	};
	super::copy_str_out(&Codec::get_codec_name(codec), buf, buf_len)
}

/// Whether `codec` produces still images (OpenEXR/PNG/TIFF); false for
/// invalid values.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_codec_is_still_image(codec: i32) -> bool {
	Codec::from_i32(codec).is_some_and(Codec::is_codec_a_still_image)
}

/// Whether `codec` is lossless; false for invalid values.
#[unsafe(no_mangle)]
pub extern "C" fn oak_codec_codec_is_lossless(codec: i32) -> bool {
	Codec::from_i32(codec).is_some_and(Codec::is_codec_lossless)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn validity_and_count() {
		assert!(oak_codec_codec_is_valid(0));
		assert!(oak_codec_codec_is_valid(18));
		assert!(!oak_codec_codec_is_valid(19), "Count is not a real codec");
		assert!(!oak_codec_codec_is_valid(-1));
		assert_eq!(oak_codec_codec_count(), 19);
	}

	#[test]
	fn codec_name_roundtrip() {
		let mut buf = [0u8; 32];
		let n = oak_codec_codec_get_name(1, buf.as_mut_ptr(), buf.len());
		assert_eq!(n, 5);
		assert_eq!(&buf[..6], b"H.264\0");

		// Invalid codec: -1, buffer untouched.
		buf[0] = b'x';
		assert_eq!(oak_codec_codec_get_name(99, buf.as_mut_ptr(), buf.len()), -1);

		// Query mode still reports the required length.
		assert_eq!(oak_codec_codec_get_name(12, std::ptr::null_mut(), 0), 3);
	}

	#[test]
	fn still_image_and_lossless_flags() {
		assert!(oak_codec_codec_is_still_image(4)); // OpenEXR
		assert!(!oak_codec_codec_is_still_image(1)); // H264
		assert!(!oak_codec_codec_is_still_image(-1));
		assert!(oak_codec_codec_is_lossless(16)); // FLAC
		assert!(!oak_codec_codec_is_lossless(1));
	}
}

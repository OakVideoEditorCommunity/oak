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

//! C ABI bindings for oak-codec: proxy manager, export format/codec
//! enumerations, and the background-task submit lane.

mod exportcodec;
mod exportformat;
mod proxymanager;
mod task;

/// snprintf-style string out: writes `s` NUL-terminated into `buf`
/// (truncated to `buf_len - 1` when too small) and returns the full byte
/// length excluding the NUL — so a null/0 buffer queries the size, and
/// `ret >= buf_len` tells the caller the output was truncated.
pub(crate) fn copy_str_out(s: &str, buf: *mut u8, buf_len: usize) -> i32 {
	let bytes = s.as_bytes();
	if !buf.is_null() && buf_len > 0 {
		let n = bytes.len().min(buf_len - 1);
		unsafe {
			std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
			*buf.add(n) = 0;
		}
	}
	bytes.len() as i32
}

/// Borrows a `(ptr, len)` string argument from C: null or invalid UTF-8
/// maps to `None` (the caller turns that into the ABI's error value).
pub(crate) unsafe fn str_arg<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
	if ptr.is_null() {
		return None;
	}
	let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
	std::str::from_utf8(bytes).ok()
}

/// Serializes tests that touch the process-global task-submit registry
/// (the codec task tests and the proxy get_or_start tests).
#[cfg(test)]
pub(crate) static TASK_REG_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn copy_str_out_exact_fit_truncation_and_query() {
		// Exact fit: "abc" + NUL needs 4 bytes.
		let mut buf = [0u8; 4];
		assert_eq!(copy_str_out("abc", buf.as_mut_ptr(), 4), 3);
		assert_eq!(&buf, b"abc\0");

		// Truncated: required length still returned, output cut + NUL.
		let mut small = [0u8; 2];
		assert_eq!(copy_str_out("abcdef", small.as_mut_ptr(), 2), 6);
		assert_eq!(&small, b"a\0");

		// Query mode: null buffer, nothing written.
		assert_eq!(copy_str_out("hello", std::ptr::null_mut(), 0), 5);
	}

	#[test]
	fn str_arg_rejects_null_and_invalid_utf8() {
		unsafe {
			assert_eq!(str_arg("ok".as_ptr(), 2), Some("ok"));
			assert_eq!(str_arg(std::ptr::null(), 0), None);
			assert_eq!(str_arg([0xffu8].as_ptr(), 1), None);
		}
	}
}

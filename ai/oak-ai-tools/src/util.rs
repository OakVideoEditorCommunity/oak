//! Small helpers with no dependencies: FNV-1a hashing and standard
//! base64 (used to carry PNG bytes inside tool results).

/// FNV-1a 64-bit over `bytes`.
///
/// Deterministic across runs and platforms (unlike `DefaultHasher`,
/// whose seed is per-process), which is what the golden-frame helpers
/// and the mock renderer need.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
	let mut hash: u64 = 0xcbf29ce484222325;
	for &byte in bytes {
		hash ^= u64::from(byte);
		hash = hash.wrapping_mul(0x100000001b3);
	}
	hash
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with `=` padding.
pub fn base64_encode(data: &[u8]) -> String {
	let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
	for chunk in data.chunks(3) {
		let b0 = u32::from(chunk[0]);
		let b1 = chunk.get(1).copied().map(u32::from).unwrap_or(0);
		let b2 = chunk.get(2).copied().map(u32::from).unwrap_or(0);
		let triple = (b0 << 16) | (b1 << 8) | b2;
		out.push(B64[((triple >> 18) & 0x3f) as usize] as char);
		out.push(B64[((triple >> 12) & 0x3f) as usize] as char);
		out.push(if chunk.len() > 1 {
			B64[((triple >> 6) & 0x3f) as usize] as char
		} else {
			'='
		});
		out.push(if chunk.len() > 2 {
			B64[(triple & 0x3f) as usize] as char
		} else {
			'='
		});
	}
	out
}

/// Standard base64 decoder (padding optional); `None` on malformed input.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
	fn value(byte: u8) -> Option<u32> {
		match byte {
			b'A'..=b'Z' => Some(u32::from(byte - b'A')),
			b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
			b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
			b'+' => Some(62),
			b'/' => Some(63),
			_ => None,
		}
	}
	let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
	if clean.len() % 4 != 0 {
		return None;
	}
	let mut out = Vec::with_capacity(clean.len() / 4 * 3);
	for chunk in clean.chunks(4) {
		let pad = chunk.iter().filter(|&&b| b == b'=').count();
		if pad > 2 || chunk.len() != 4 {
			return None;
		}
		let mut triple = 0u32;
		for (index, &byte) in chunk.iter().enumerate() {
			if byte == b'=' {
				if index < 2 {
					return None;
				}
				// A padding slot still consumes a 6-bit position
				// (zero-valued) so the kept bytes stay aligned.
				triple <<= 6;
				continue;
			}
			let v = value(byte)?;
			triple = (triple << 6) | v;
		}
		out.push(((triple >> 16) & 0xff) as u8);
		if pad < 2 {
			out.push(((triple >> 8) & 0xff) as u8);
		}
		if pad < 1 {
			out.push((triple & 0xff) as u8);
		}
	}
	Some(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn base64_round_trip() {
		for data in [
			&b""[..],
			&b"f"[..],
			&b"fo"[..],
			&b"foo"[..],
			&b"foob"[..],
			&b"fooba"[..],
			&b"foobar"[..],
			&[0u8, 1, 2, 254, 255][..],
		] {
			let encoded = base64_encode(data);
			assert_eq!(base64_decode(&encoded).as_deref(), Some(data), "{encoded}");
		}
	}

	#[test]
	fn base64_known_vectors() {
		assert_eq!(base64_encode(b""), "");
		assert_eq!(base64_encode(b"f"), "Zg==");
		assert_eq!(base64_encode(b"fo"), "Zm8=");
		assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
		assert!(base64_decode("!!!").is_none());
		assert!(base64_decode("Zg").is_none()); // not a multiple of 4
	}

	#[test]
	fn fnv_is_stable_and_sensitive() {
		assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
		assert_ne!(fnv1a64(b"a"), fnv1a64(b"b"));
		assert_eq!(fnv1a64(b"oak"), fnv1a64(b"oak"));
	}
}

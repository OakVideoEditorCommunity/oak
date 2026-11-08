//! T6 replay library: NDJSON fixture format, session log, and the
//! golden-frame helpers shared by the Rust test and the CI harness.
//!
//! # Fixture format (`ai/ci/fixtures/*.ndjson`)
//!
//! One JSON object per line, `#` lines and blank lines ignored:
//!
//! ```json
//! {"call": "project.open", "args": {"name": "ci", "fps": 30}}
//! {"call": "render.get_frame", "args": {"sequence": "sequence-1", "time": {"num": 0, "den": 1}}}
//! ```
//!
//! `call` is a frozen tool name from `ai/schema/tools/*.json`; `args` is
//! the JSON-Schema-validated argument object. The token `OAK_CI_MEDIA`
//! inside any string argument is replaced at replay time by a per-run
//! temp media path, so the same bytes land at the same path on every run
//! (the mock backend colours frames from the path hash — determinism
//! depends on it).
//!
//! The replay runs the steps in order against one `MockBackend` through
//! `dispatch::execute_with` (so the T4 hooks fire) and writes a session
//! log:
//!
//! ```json
//! {"kind":"call","call":"project.open","args":{...},"ok":true,"error":null,
//!  "txn":"committed","undo_depth":1,"result_sha256":"…","frames":[]}
//! {"kind":"call","call":"render.get_frame","args":{...},"ok":true,"error":null,
//!  "txn":"committed","undo_depth":4,"result_sha256":"…",
//!  "frames":[{"tool":"render.get_frame","sha256":"…",
//!             "width":16,"height":16,"non_black_ratio":1.0}]}
//! ```
//!
//! Two runs of the same fixture over the same crate versions must produce
//! byte-identical logs — that equality is what CI diffs.

use serde_json::Value;

/// One recorded step: a frozen tool call and its arguments.
#[derive(Debug, Clone)]
pub struct ReplayEvent {
	/// Frozen tool name (a file name under `ai/schema/tools/`).
	pub call: String,
	/// Its JSON arguments.
	pub args: Value,
}

/// One frame as it appears in the session log.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FrameLog {
	/// Tool that returned it.
	pub tool: String,
	/// SHA-256 of the raw PNG bytes.
	pub sha256: String,
	/// Declared width.
	pub width: u32,
	/// Declared height.
	pub height: u32,
	/// Fraction of non-black pixels (golden-frame invariant).
	pub non_black_ratio: f64,
}

impl<'de> serde::Deserialize<'de> for ReplayEvent {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		#[derive(serde::Deserialize)]
		struct Raw {
			call: String,
			args: Value,
		}
		let raw = Raw::deserialize(de)?;
		Ok(Self {
			call: raw.call,
			args: raw.args,
		})
	}
}

/// The frame payloads of a result: `render.get_frame` returns the result
/// itself, `render.get_thumbnails` a `frames` array.
pub fn frame_values<'a>(tool: &str, result: &'a Value) -> Vec<&'a Value> {
	if tool == "render.get_thumbnails" {
		result
			.get("frames")
			.and_then(Value::as_array)
			.map(|frames| frames.iter().collect())
			.unwrap_or_default()
	} else {
		vec![result]
	}
}

/// SHA-256 of a frame's PNG payload, `None` when the result carries no
/// `png_base64`.
pub fn png_sha256(frame: &Value) -> Option<String> {
	let b64 = frame.get("png_base64")?.as_str()?;
	let bytes = oak_ai_tools::util::base64_decode(b64)?;
	Some(sha256_hex(&bytes))
}

/// Canonical (sorted-key, no whitespace) SHA-256 of any JSON value —
/// makes `result_sha256` stable across key ordering.
pub fn canonical_sha256(value: &Value) -> String {
	sha256_hex(canonical_bytes(value).as_bytes())
}

fn canonical_bytes(value: &Value) -> String {
	match value {
		Value::Object(map) => {
			let mut keys: Vec<&String> = map.keys().collect();
			keys.sort();
			let inner: Vec<String> = keys
				.into_iter()
				.map(|k| format!("{}:{}", escape(k), canonical_bytes(&map[k])))
				.collect();
			format!("{{{}}}", inner.join(","))
		}
		Value::Array(items) => {
			let inner: Vec<String> = items.iter().map(canonical_bytes).collect();
			format!("[{}]", inner.join(","))
		}
		other => other.to_string(),
	}
}

fn escape(text: &str) -> String {
	let mut out = String::with_capacity(text.len() + 2);
	out.push('"');
	for ch in text.chars() {
		match ch {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
			c => out.push(c),
		}
	}
	out.push('"');
	out
}

/// SHA-256 (hex) without pulling a crypto crate into this package: the
/// execution layer already exposes nothing, so use a tiny pure-Rust
/// implementation of FIPS 180-4.
pub fn sha256_hex(bytes: &[u8]) -> String {
	sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// FIPS 180-4 SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
	const K: [u32; 64] = [
		0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
		0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
		0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
		0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
		0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
		0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
		0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
		0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
		0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
		0xc67178f2,
	];
	let mut h: [u32; 8] = [
		0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
		0x5be0cd19,
	];
	let bit_len = (data.len() as u64) * 8;
	let mut padded = data.to_vec();
	padded.push(0x80);
	while padded.len() % 64 != 56 {
		padded.push(0);
	}
	padded.extend_from_slice(&bit_len.to_be_bytes());

	for chunk in padded.chunks(64) {
		let mut w = [0u32; 64];
		for (i, word) in chunk.chunks(4).enumerate() {
			w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
		}
		for i in 16..64 {
			let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
			let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
			w[i] = w[i - 16]
				.wrapping_add(s0)
				.wrapping_add(w[i - 7])
				.wrapping_add(s1);
		}
		let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
			(h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
		for i in 0..64 {
			let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
			let ch = (e & f) ^ ((!e) & g);
			let temp1 = hh
				.wrapping_add(s1)
				.wrapping_add(ch)
				.wrapping_add(K[i])
				.wrapping_add(w[i]);
			let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
			let maj = (a & b) ^ (a & c) ^ (b & c);
			let temp2 = s0.wrapping_add(maj);
			hh = g;
			g = f;
			f = e;
			e = d.wrapping_add(temp1);
			d = c;
			c = b;
			b = a;
			a = temp1.wrapping_add(temp2);
		}
		h[0] = h[0].wrapping_add(a);
		h[1] = h[1].wrapping_add(b);
		h[2] = h[2].wrapping_add(c);
		h[3] = h[3].wrapping_add(d);
		h[4] = h[4].wrapping_add(e);
		h[5] = h[5].wrapping_add(f);
		h[6] = h[6].wrapping_add(g);
		h[7] = h[7].wrapping_add(hh);
	}

	let mut out = [0u8; 32];
	for (i, word) in h.iter().enumerate() {
		out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sha256_matches_known_vectors() {
		assert_eq!(
			sha256_hex(b""),
			"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
		);
		assert_eq!(
			sha256_hex(b"abc"),
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
		);
	}

	#[test]
	fn canonical_json_ignores_key_order() {
		let a: Value = serde_json::from_str(r#"{"b":1,"a":[2,{"d":3,"c":4}]}"#).unwrap();
		let b: Value = serde_json::from_str(r#"{"a":[2,{"c":4,"d":3}],"b":1}"#).unwrap();
		assert_eq!(canonical_sha256(&a), canonical_sha256(&b));
		let c: Value = serde_json::from_str(r#"{"a":[2,{"c":4,"d":5}],"b":1}"#).unwrap();
		assert_ne!(canonical_sha256(&a), canonical_sha256(&c));
	}
}

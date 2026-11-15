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

use base64::Engine as _;
use sha2::Digest as _;
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
	let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
	Some(hex::encode(sha2::Sha256::digest(&bytes)))
}

/// Canonical (sorted-key, no whitespace) SHA-256 of any JSON value —
/// makes `result_sha256` stable across key ordering.
pub fn canonical_sha256(value: &Value) -> String {
	hex::encode(sha2::Sha256::digest(canonical_bytes(value).as_bytes()))
}

fn canonical_bytes(value: &Value) -> String {
	// Key order is the only thing `serde_json` will not normalise for us;
	// escaping and number formatting stay its job (hand-written JSON string
	// escaping used to live here and was a second implementation of the
	// serializer).
	fn write(value: &Value, out: &mut String) {
		match value {
			Value::Object(map) => {
				let mut keys: Vec<&String> = map.keys().collect();
				keys.sort();
				out.push('{');
				for (i, key) in keys.iter().enumerate() {
					if i > 0 {
						out.push(',');
					}
					out.push_str(&serde_json::to_string(*key).expect("a JSON string"));
					out.push(':');
					write(&map[*key], out);
				}
				out.push('}');
			}
			Value::Array(items) => {
				out.push('[');
				for (i, item) in items.iter().enumerate() {
					if i > 0 {
						out.push(',');
					}
					write(item, out);
				}
				out.push(']');
			}
			other => out.push_str(&serde_json::to_string(other).expect("a JSON scalar")),
		}
	}
	let mut out = String::new();
	write(value, &mut out);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sha256_matches_known_vectors() {
		assert_eq!(
			hex::encode(sha2::Sha256::digest(b"")),
			"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
		);
		assert_eq!(
			hex::encode(sha2::Sha256::digest(b"abc")),
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

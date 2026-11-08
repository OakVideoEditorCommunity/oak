//! JSON-RPC 2.0 over newline-delimited JSON — the MCP stdio transport.
//!
//! One JSON document per line, UTF-8, no `Content-Length` header. This is
//! the framing the MCP specification prescribes for stdio and therefore
//! needs no third-party transport crate: a buffered line reader plus a
//! serialiser is the whole implementation.
//!
//! The module only owns *framing* and the JSON-RPC envelope. Method
//! semantics (lifecycle, `tools/*`) live in [`crate::server`].

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The only `jsonrpc` value this server accepts and emits.
pub const JSONRPC_VERSION: &str = "2.0";

/// The frame could not be parsed as JSON (`-32700`).
pub const PARSE_ERROR: i64 = -32700;
/// The frame parsed but is not a valid JSON-RPC 2.0 request (`-32600`).
pub const INVALID_REQUEST: i64 = -32600;
/// The method is not implemented by this server (`-32601`).
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The method exists but its parameters are malformed (`-32602`).
pub const INVALID_PARAMS: i64 = -32602;
/// The server broke while handling an otherwise valid request (`-32603`).
pub const INTERNAL_ERROR: i64 = -32603;
/// A request arrived before `initialize` (MCP `-32002`).
pub const SERVER_NOT_INITIALIZED: i64 = -32002;

/// A successful response envelope.
pub fn success(id: Value, result: Value) -> Value {
	json!({
		"jsonrpc": JSONRPC_VERSION,
		"id": id,
		"result": result,
	})
}

/// An error response envelope. `data` is omitted when `None` (JSON-RPC
/// 2.0 makes it optional and MCP clients treat it as opaque).
pub fn error(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
	let mut body = json!({
		"code": code,
		"message": message.into(),
	});
	if let Some(data) = data {
		body["data"] = data;
	}
	json!({
		"jsonrpc": JSONRPC_VERSION,
		"id": id,
		"error": body,
	})
}

/// One frame read from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
	/// A parsed JSON document (object, array, scalar — validation is the
	/// server's job, framing only guarantees it parsed).
	Message(Value),
	/// A non-empty line that is not valid JSON; the reader keeps going.
	Malformed(String),
	/// End of input: the client closed the stream, the server may exit.
	Eof,
}

/// Reads one NDJSON frame.
///
/// Blank lines are skipped (some clients emit a trailing newline after
/// each frame). A line that does not parse is returned as
/// [`Inbound::Malformed`] so the caller can answer with `-32700` and keep
/// the session alive. `Ok(Inbound::Eof)` is returned when the stream ends;
/// I/O errors (including non-UTF-8 input) are propagated because the
/// session cannot be trusted after them.
pub fn read_inbound<R: BufRead>(reader: &mut R) -> std::io::Result<Inbound> {
	loop {
		let mut line = String::new();
		let read = reader.read_line(&mut line)?;
		if read == 0 {
			return Ok(Inbound::Eof);
		}
		let trimmed = line.trim();
		if trimmed.is_empty() {
			continue;
		}
		return Ok(match serde_json::from_str::<Value>(trimmed) {
			Ok(value) => Inbound::Message(value),
			Err(err) => Inbound::Malformed(format!("invalid JSON frame: {err}")),
		});
	}
}

/// Writes one NDJSON frame and flushes, so a client sees the response
/// before the next request is read.
pub fn write_frame<W: Write>(writer: &mut W, message: &Value) -> std::io::Result<()> {
	serde_json::to_writer(&mut *writer, message).map_err(std::io::Error::other)?;
	writer.write_all(b"\n")?;
	writer.flush()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Cursor;

	#[test]
	fn reads_frames_and_skips_blank_lines() {
		let mut input = Cursor::new(b"{\"a\":1}\n\n{\"b\":2}\n".to_vec());
		assert_eq!(
			read_inbound(&mut input).unwrap(),
			Inbound::Message(json!({"a": 1}))
		);
		assert_eq!(
			read_inbound(&mut input).unwrap(),
			Inbound::Message(json!({"b": 2}))
		);
		assert_eq!(read_inbound(&mut input).unwrap(), Inbound::Eof);
	}

	#[test]
	fn malformed_line_does_not_end_the_stream() {
		let mut input = Cursor::new(b"not json\n{\"ok\":true}\n".to_vec());
		match read_inbound(&mut input).unwrap() {
			Inbound::Malformed(message) => assert!(message.contains("invalid JSON")),
			other => panic!("expected Malformed, got {other:?}"),
		}
		assert_eq!(
			read_inbound(&mut input).unwrap(),
			Inbound::Message(json!({"ok": true}))
		);
	}

	#[test]
	fn writes_exactly_one_line_per_frame() {
		let mut out = Vec::new();
		write_frame(&mut out, &success(json!(1), json!({"ok": true}))).unwrap();
		write_frame(&mut out, &error(Value::Null, PARSE_ERROR, "bad", None)).unwrap();
		let text = String::from_utf8(out).unwrap();
		let lines: Vec<&str> = text.lines().collect();
		assert_eq!(lines.len(), 2, "one frame per line: {text:?}");
		assert!(text.ends_with('\n'));
		assert_eq!(
			serde_json::from_str::<Value>(lines[0]).unwrap()["result"]["ok"],
			true
		);
		assert_eq!(
			serde_json::from_str::<Value>(lines[1]).unwrap()["error"]["code"],
			PARSE_ERROR
		);
	}
}

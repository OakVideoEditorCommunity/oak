//! OPP/1 message envelope, error codes and NDJSON framing.
//!
//! Frozen reference: `docs/zh/plans/external-plugin-protocol.md`
//! §1 (framing), §2 (envelope), §3 (error codes), §4.1 (handshake).
//! This module is the type layer only: no process handling, no I/O policy.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::io::Read;

/// Protocol main version carried by `session.hello.api`.
pub const API_VERSION: u32 = 1;

/// Single-message size cap (§1): 16 MiB, measured on the raw line bytes
/// (excluding the terminating `\n`).
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

// ---- JSON-RPC 2.0 standard error codes (§3) ----
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// ---- OPP application error codes (§3) ----
pub const CAPABILITY_DENIED: i32 = -32000;
pub const NOT_IN_TRANSACTION: i32 = -32001;
pub const TRANSACTION_CONFLICT: i32 = -32002;
pub const ENTITY_NOT_FOUND: i32 = -32003;
pub const RATE_LIMITED: i32 = -32004;
pub const SHM_EXHAUSTED: i32 = -32005;
pub const CONFIRMATION_DENIED: i32 = -32006;
pub const FRAME_TOO_LARGE: i32 = -32007;
pub const INVALID_STATE: i32 = -32008;
/// Host-synthesized: a pending call died with its plugin (design doc §2.2,
/// 「未完成的宿主 API 调用全部以 PLUGIN_DEAD 错误返回」). Never sent on
/// the wire — it only surfaces through [`crate::session::RpcError`].
pub const PLUGIN_DEAD: i32 = -32100;

// ---------------------------------------------------------------------------
// Message id: string or integer, sender-namespaced (§2).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MsgId {
	Number(i64),
	String(String),
}

impl fmt::Display for MsgId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			MsgId::Number(n) => write!(f, "{n}"),
			MsgId::String(s) => write!(f, "{s}"),
		}
	}
}

impl From<i64> for MsgId {
	fn from(n: i64) -> Self {
		MsgId::Number(n)
	}
}
impl From<u64> for MsgId {
	fn from(n: u64) -> Self {
		MsgId::Number(n as i64)
	}
}
impl From<&str> for MsgId {
	fn from(s: &str) -> Self {
		MsgId::String(s.to_string())
	}
}
impl From<String> for MsgId {
	fn from(s: String) -> Self {
		MsgId::String(s)
	}
}

impl Serialize for MsgId {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		match self {
			MsgId::Number(n) => s.serialize_i64(*n),
			MsgId::String(v) => s.serialize_str(v),
		}
	}
}

impl<'de> Deserialize<'de> for MsgId {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		struct IdVisitor;
		impl<'de> serde::de::Visitor<'de> for IdVisitor {
			type Value = MsgId;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a string or integer id")
			}
			fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<MsgId, E> {
				Ok(MsgId::Number(v))
			}
			fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<MsgId, E> {
				Ok(MsgId::Number(v as i64))
			}
			fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<MsgId, E> {
				if v.fract() == 0.0 && v.abs() < i64::MAX as f64 {
					Ok(MsgId::Number(v as i64))
				} else {
					Err(E::custom("id must be an integer"))
				}
			}
			fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<MsgId, E> {
				Ok(MsgId::String(v.to_string()))
			}
		}
		d.deserialize_any(IdVisitor)
	}
}

// ---------------------------------------------------------------------------
// Envelope (JSON-RPC 2.0, bidirectional — §2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
	pub jsonrpc: String,
	pub id: MsgId,
	pub method: String,
	/// Always an object on the wire (§2); `Null` is omitted when serializing.
	#[serde(default, skip_serializing_if = "Value::is_null")]
	pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
	pub jsonrpc: String,
	pub method: String,
	#[serde(default, skip_serializing_if = "Value::is_null")]
	pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
	pub code: i32,
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
	pub jsonrpc: String,
	pub id: MsgId,
	/// Exactly one of `result` / `error` is present on the wire; both `None`
	/// here means the decoder rejected the frame before constructing this.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub result: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<ErrorObject>,
}

impl Request {
	pub fn new(id: impl Into<MsgId>, method: impl Into<String>, params: Value) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			id: id.into(),
			method: method.into(),
			params,
		}
	}
}

impl Notification {
	pub fn new(method: impl Into<String>, params: Value) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			method: method.into(),
			params,
		}
	}
}

impl ErrorObject {
	pub fn new(code: i32, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
			data: None,
		}
	}
	pub fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
		Self {
			code,
			message: message.into(),
			data: Some(data),
		}
	}
}

impl Response {
	pub fn success(id: impl Into<MsgId>, result: Value) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			id: id.into(),
			result: Some(result),
			error: None,
		}
	}
	pub fn failure(id: impl Into<MsgId>, code: i32, message: impl Into<String>) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			id: id.into(),
			result: None,
			error: Some(ErrorObject::new(code, message)),
		}
	}
	pub fn failure_data(
		id: impl Into<MsgId>,
		code: i32,
		message: impl Into<String>,
		data: Value,
	) -> Self {
		Self {
			jsonrpc: "2.0".to_string(),
			id: id.into(),
			result: None,
			error: Some(ErrorObject::with_data(code, message, data)),
		}
	}
	pub fn is_success(&self) -> bool {
		self.error.is_none()
	}
}

/// Any frame that survived [`decode_line`].
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
	Request(Request),
	Response(Response),
	Notification(Notification),
}

impl Message {
	pub fn id(&self) -> Option<&MsgId> {
		match self {
			Message::Request(r) => Some(&r.id),
			Message::Response(r) => Some(&r.id),
			Message::Notification(_) => None,
		}
	}
	pub fn method(&self) -> Option<&str> {
		match self {
			Message::Request(r) => Some(&r.method),
			Message::Notification(n) => Some(&n.method),
			Message::Response(_) => None,
		}
	}
}

// ---------------------------------------------------------------------------
// Framing (§1): one UTF-8 JSON object per line, no BOM, no bare CR, ≤ 16 MiB.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
	/// A single line exceeded [`MAX_MESSAGE_BYTES`]; the host treats this as
	/// a protocol error and kills the plugin (§1).
	TooLarge {
		bytes: usize,
	},
	/// UTF-8 BOM at the start of a message.
	Bom,
	/// Bare CR inside a message (only `\n` terminates a frame).
	BareNewline,
	/// Not valid JSON / not a JSON object.
	Parse,
	/// Well-formed JSON that violates the envelope shape.
	InvalidRequest,
	/// Stream ended in the middle of a message.
	Truncated,
	Io(String),
}

impl fmt::Display for FrameError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			FrameError::TooLarge { bytes } => {
				write!(f, "message of {bytes} bytes exceeds the 16 MiB cap")
			}
			FrameError::Bom => write!(f, "UTF-8 BOM is not allowed"),
			FrameError::BareNewline => write!(f, "bare CR inside a message"),
			FrameError::Parse => write!(f, "frame is not a JSON object"),
			FrameError::InvalidRequest => write!(f, "invalid JSON-RPC envelope"),
			FrameError::Truncated => write!(f, "stream ended mid-message"),
			FrameError::Io(e) => write!(f, "read error: {e}"),
		}
	}
}

impl std::error::Error for FrameError {}

/// Serialize `msg` as one NDJSON line (including the trailing `\n`).
pub fn encode_line<M: Serialize>(msg: &M) -> Result<Vec<u8>, FrameError> {
	let s = serde_json::to_string(msg).map_err(|_| FrameError::Parse)?;
	if s.len() > MAX_MESSAGE_BYTES {
		return Err(FrameError::TooLarge { bytes: s.len() });
	}
	// serde escapes newlines inside strings, so this is defensive.
	if s.contains('\n') || s.contains('\r') {
		return Err(FrameError::BareNewline);
	}
	let mut out = s.into_bytes();
	out.push(b'\n');
	Ok(out)
}

/// Decode one message line (the terminating `\n` already stripped).
pub fn decode_line(line: &[u8]) -> Result<Message, FrameError> {
	if line.starts_with(&[0xEF, 0xBB, 0xBF]) {
		return Err(FrameError::Bom);
	}
	if line.iter().any(|&b| b == b'\r') {
		return Err(FrameError::BareNewline);
	}
	if line.len() > MAX_MESSAGE_BYTES {
		return Err(FrameError::TooLarge { bytes: line.len() });
	}
	let value: Value = serde_json::from_slice(line).map_err(|_| FrameError::Parse)?;
	let obj = value.as_object().ok_or(FrameError::Parse)?;
	if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
		return Err(FrameError::InvalidRequest);
	}
	if let Some(p) = obj.get("params")
		&& !p.is_null()
		&& !p.is_object()
	{
		// §2: params is always an object (no positional params).
		return Err(FrameError::InvalidRequest);
	}
	let has_id = obj.contains_key("id");
	let has_method = obj.contains_key("method");
	let has_result = obj.contains_key("result");
	let has_error = obj.contains_key("error");
	fn to_msg<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, FrameError> {
		serde_json::from_value(v).map_err(|_| FrameError::InvalidRequest)
	}
	match (has_id, has_method, has_result, has_error) {
		(true, true, false, false) => Ok(Message::Request(to_msg(value)?)),
		(false, true, false, false) => Ok(Message::Notification(to_msg(value)?)),
		(true, false, true, false) | (true, false, false, true) => {
			Ok(Message::Response(to_msg(value)?))
		}
		_ => Err(FrameError::InvalidRequest),
	}
}

/// Incremental NDJSON reader over any [`Read`].
///
/// The size cap is enforced **while reading**: an over-long line aborts as
/// soon as the buffer passes the cap, without buffering the whole message.
pub struct FramedReader<R: Read> {
	inner: R,
	buf: Vec<u8>,
	/// Bytes already checked for `\n`; without this, each 8 KiB refill would
	/// rescan the whole partial line and a 17 MiB abort degenerates to a
	/// quadratic scan.
	scanned: usize,
}

impl<R: Read> FramedReader<R> {
	pub fn new(inner: R) -> Self {
		Self {
			inner,
			buf: Vec::new(),
			scanned: 0,
		}
	}

	/// Next message. `Ok(None)` = clean EOF with no partial frame pending.
	pub fn read_message(&mut self) -> Result<Option<Message>, FrameError> {
		let mut chunk = [0u8; 8192];
		loop {
			if let Some(rel) = self.buf[self.scanned..]
				.iter()
				.position(|&b| b == b'\n')
			{
				let i = self.scanned + rel;
				let line: Vec<u8> = self.buf.drain(..=i).collect();
				self.scanned = 0;
				return Ok(Some(decode_line(&line[..i])?));
			}
			// No newline yet: the buffered bytes are one partial line.
			self.scanned = self.buf.len();
			if self.buf.len() > MAX_MESSAGE_BYTES {
				return Err(FrameError::TooLarge {
					bytes: self.buf.len(),
				});
			}
			let n = match self.inner.read(&mut chunk) {
				Ok(0) => {
					return if self.buf.is_empty() {
						Ok(None)
					} else {
						Err(FrameError::Truncated)
					};
				}
				Ok(n) => n,
				Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(e) => return Err(FrameError::Io(e.to_string())),
			};
			self.buf.extend_from_slice(&chunk[..n]);
		}
	}
}

// ---------------------------------------------------------------------------
// Handshake (§4.1) and lifecycle messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSpec {
	pub id: String,
	pub title: String,
	#[serde(default)]
	pub ui: String,
}

/// `session.hello` params (plugin → Oak).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloParams {
	pub api: u32,
	pub name: String,
	pub version: String,
	pub capabilities: Vec<String>,
	#[serde(default)]
	pub panels: Vec<PanelSpec>,
	#[serde(default)]
	pub subscribe: Vec<String>,
}

/// `session.hello` result (Oak → plugin): grants + feature probe (§4.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
	pub api: u32,
	pub oak_version: String,
	pub granted: Vec<String>,
	pub features: Vec<String>,
	/// shm is created from P3 on; P1 handshakes omit it (spec §6.1).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub shm: Option<Value>,
}

/// Heartbeat request (§4.2): `{"id":"ping-317","method":"session.ping"}`.
pub fn ping_request(seq: u64) -> Request {
	Request::new(MsgId::String(format!("ping-{seq}")), "session.ping", Value::Null)
}

/// Shutdown notification (§4.4).
pub fn shutdown_notification(reason: &str) -> Notification {
	Notification::new("session.shutdown", serde_json::json!({ "reason": reason }))
}

// ---------------------------------------------------------------------------
// `ui.*` message types — P2 fills in the render/validate logic (§11).
// ---------------------------------------------------------------------------

pub mod ui {
	use serde::{Deserialize, Serialize};
	use serde_json::Value;

	/// Full replace of a panel's widget tree (`ui.set_tree`, §11.1).
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct SetTree {
		pub panel: String,
		pub tree: Value,
	}
	/// Incremental property updates (`ui.set_props`, append semantics).
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct SetProps {
		pub panel: String,
		pub ops: Vec<Value>,
	}
	/// Status-bar toast (`ui.notify`).
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Notify {
		pub kind: String,
		pub text: String,
	}
	/// Plugin → host widget event (`ui.event`, §11.1).
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct UiEvent {
		pub panel: String,
		pub id: String,
		pub kind: String,
		#[serde(flatten)]
		pub payload: Value,
	}
	/// Pixel-surface handshake (§11.2, P5 — typed placeholder only).
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct AttachSurface {
		pub panel: String,
		pub width: u32,
		pub height: u32,
	}
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct FrameReady {
		pub surface: String,
		pub seq: u64,
	}
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct SurfaceAck {
		pub surface: String,
		pub seq: u64,
	}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use std::io::Cursor;

	#[test]
	fn request_roundtrip() {
		let req = Request::new(42u64, "timeline.split_clip", json!({"clip": "n17"}));
		let line = encode_line(&req).unwrap();
		assert!(line.ends_with(b"\n"));
		let msg = decode_line(&line[..line.len() - 1]).unwrap();
		match msg {
			Message::Request(r) => {
				assert_eq!(r.id, MsgId::Number(42));
				assert_eq!(r.method, "timeline.split_clip");
				assert_eq!(r.params, json!({"clip": "n17"}));
			}
			other => panic!("wrong variant: {other:?}"),
		}
	}

	#[test]
	fn response_success_and_error_roundtrip() {
		let ok = Response::success("ping-1", json!({}));
		let line = encode_line(&ok).unwrap();
		match decode_line(&line[..line.len() - 1]).unwrap() {
			Message::Response(r) => {
				assert!(r.is_success());
				assert_eq!(r.result, Some(json!({})));
			}
			other => panic!("wrong variant: {other:?}"),
		}
		let err = Response::failure_data(
			7u64,
			NOT_IN_TRANSACTION,
			"not in edit transaction",
			json!({"txn": "t1"}),
		);
		let line = encode_line(&err).unwrap();
		match decode_line(&line[..line.len() - 1]).unwrap() {
			Message::Response(r) => {
				let e = r.error.expect("error must be present");
				assert_eq!(e.code, NOT_IN_TRANSACTION);
				assert_eq!(e.data, Some(json!({"txn": "t1"})));
			}
			other => panic!("wrong variant: {other:?}"),
		}
	}

	#[test]
	fn notification_has_no_id() {
		let n = Notification::new("session.shutdown", json!({"reason": "app_quit"}));
		let line = encode_line(&n).unwrap();
		match decode_line(&line[..line.len() - 1]).unwrap() {
			Message::Notification(n) => assert_eq!(n.method, "session.shutdown"),
			other => panic!("wrong variant: {other:?}"),
		}
		// ping omits params entirely (§4.2 example).
		let ping = ping_request(317);
		let s = String::from_utf8(encode_line(&ping).unwrap()).unwrap();
		assert!(s.contains("\"method\":\"session.ping\""), "{s}");
		assert!(!s.contains("params"), "null params must be omitted: {s}");
	}

	#[test]
	fn rejects_bom_cr_and_bad_shapes() {
		let mut with_bom = vec![0xEF, 0xBB, 0xBF];
		with_bom.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"m"}"#);
		assert_eq!(decode_line(&with_bom), Err(FrameError::Bom));

		// A literal CR byte inside the line (not the escaped form).
		let mut cr = br#"{"jsonrpc":"2.0","id":1,"method":"#.to_vec();
		cr.push(b'\r');
		cr.extend_from_slice(b"}");
		assert_eq!(decode_line(&cr), Err(FrameError::BareNewline));

		assert_eq!(
			decode_line(br#"{"id":1,"method":"m"}"#),
			Err(FrameError::InvalidRequest),
			"missing jsonrpc:2.0"
		);
		assert_eq!(
			decode_line(br#"{"jsonrpc":"2.0","id":1,"method":"m","params":[1]}"#),
			Err(FrameError::InvalidRequest),
			"positional params are forbidden"
		);
		assert_eq!(
			decode_line(br#"{"jsonrpc":"2.0","id":1,"result":1,"error":{"code":1,"message":"x"}}"#),
			Err(FrameError::InvalidRequest),
			"result and error are mutually exclusive"
		);
		assert_eq!(
			decode_line(br#"{"jsonrpc":"2.0","id":null,"method":"m"}"#),
			Err(FrameError::InvalidRequest),
			"null id is forbidden"
		);
		assert_eq!(decode_line(b"not json"), Err(FrameError::Parse));
		assert_eq!(
			decode_line(br#"{"jsonrpc":"1.0","id":1,"method":"m"}"#),
			Err(FrameError::InvalidRequest)
		);
	}

	#[test]
	fn oversize_line_is_rejected_while_reading() {
		// Direct decoder check on a 17 MiB line.
		let big = vec![b'a'; 17 * 1024 * 1024];
		match decode_line(&big) {
			Err(FrameError::TooLarge { bytes }) => assert!(bytes > MAX_MESSAGE_BYTES),
			other => panic!("expected TooLarge, got {other:?}"),
		}
		// Streaming check: the reader must abort without consuming the tail.
		let mut input = vec![b'a'; 17 * 1024 * 1024];
		input.push(b'\n');
		let mut reader = FramedReader::new(Cursor::new(input));
		match reader.read_message() {
			Err(FrameError::TooLarge { bytes }) => assert!(bytes > MAX_MESSAGE_BYTES),
			other => panic!("expected TooLarge, got {other:?}"),
		}
	}

	#[test]
	fn reader_handles_batched_lines_and_eof() {
		let mut input = Vec::new();
		input.extend_from_slice(
			&encode_line(&Request::new(1u64, "a", json!({}))).unwrap(),
		);
		input.extend_from_slice(
			&encode_line(&Notification::new("b", json!({"x": 1}))).unwrap(),
		);
		let mut reader = FramedReader::new(Cursor::new(input));
		assert!(matches!(reader.read_message(), Ok(Some(Message::Request(_)))));
		assert!(matches!(
			reader.read_message(),
			Ok(Some(Message::Notification(_)))
		));
		assert_eq!(reader.read_message().unwrap(), None, "clean EOF");
		// Truncated frame: bytes but no newline.
		let mut reader = FramedReader::new(Cursor::new(vec![b'{']));
		assert_eq!(reader.read_message(), Err(FrameError::Truncated));
	}
}

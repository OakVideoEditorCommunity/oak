//! JSON-RPC 2.0 response envelope helpers.
//!
//! The MCP transport, NDJSON framing and the request lifecycle are owned by
//! the official SDK (`rmcp`, wired in [`crate::rmcp_server`]). What is left
//! here is only the envelope shaping [`crate::server`] needs to answer a
//! method with `{"jsonrpc":"2.0","id":…,"result":…}` or an `error` object.

use serde_json::{Value, json};

/// The only `jsonrpc` value this server accepts and emits.
pub const JSONRPC_VERSION: &str = "2.0";

/// Parse error (JSON-RPC 2.0 §5.1).
pub const PARSE_ERROR: i64 = -32700;
/// The JSON sent is not a valid Request object.
pub const INVALID_REQUEST: i64 = -32600;
/// The method does not exist / is not available.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid method parameter(s).
pub const INVALID_PARAMS: i64 = -32602;
/// Internal JSON-RPC error.
pub const INTERNAL_ERROR: i64 = -32603;
/// MCP: a request arrived before `initialize`.
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

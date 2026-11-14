//! The MCP server core: lifecycle + `tools/list` + `tools/call`.
//!
//! This is deliberately a *thin* wrapper (plan section 3, T5): the tool
//! face is read straight out of [`oak_ai_tools::dispatch`] (registry +
//! compiled frozen schemas) and every call is forwarded to
//! [`oak_ai_tools::dispatch::execute`], which already performs schema
//! validation, the one-undo-group transaction and `tool` / `step` / `txn`
//! error annotation. Nothing in this crate re-implements validation,
//! transactions or the tool list, so the MCP face cannot drift from the
//! direct execution layer.
//!
//! Methods implemented:
//!
//! | method | kind | behaviour |
//! |---|---|---|
//! | `initialize` | request | version negotiation + capabilities |
//! | `notifications/initialized` | notification | no reply (lifecycle ack) |
//! | `tools/list` | request | 22 frozen tools, `inputSchema` verbatim |
//! | `tools/call` | request | forward to `dispatch::execute` |
//! | `ping` | request | empty result (liveness) |
//!
//! Everything else: unknown *requests* get `-32601`, unknown
//! *notifications* are ignored (JSON-RPC forbids replying to them), and
//! requests before `initialize` get MCP's `-32002`.

use serde_json::{Value, json};

use oak_ai_tools::backend::EngineBackend;
use oak_ai_tools::dispatch;
use oak_ai_tools::error::ToolError;

use crate::rpc;

/// `serverInfo.name` reported to clients.
pub const SERVER_NAME: &str = "oak-ai-mcp";

/// The protocol version this server always answers with when the client
/// asks for something it does not know.
pub const BASELINE_PROTOCOL_VERSION: &str = "2024-11-05";

/// Versions whose feature intersection this server implements. The
/// surface used here — lifecycle, `tools/list`, `tools/call`,
/// `content`/`isError` results — is unchanged across all three, so the
/// requested version is echoed back. See README "MCP 版本基线".
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// Text handed to the client model as `initialize.instructions`.
pub const INSTRUCTIONS: &str = "\
Oak video editor AI editing tools (frozen schema face, 22 tools).
Every tool name equals its schema file name under ai/schema/tools/.
Arguments are validated against that frozen JSON Schema before any edit;
one tool call = one undo group (edit.undo is the only exception).
A failed call changes nothing and is reported with isError=true and a
structured {code, message, tool, step, gap?, txn?} payload in
structuredContent, so the caller can re-plan from the code.";

/// The MCP session state: one backend plus the negotiated protocol.
pub struct Server {
	backend: Box<dyn EngineBackend>,
	backend_label: String,
	protocol_version: Option<String>,
}

impl Server {
	/// A session over `backend` (`"mock"` / `"sdk"` for logs and
	/// `initialize` diagnostics).
	pub fn new(backend: Box<dyn EngineBackend>, backend_label: impl Into<String>) -> Self {
		Self {
			backend,
			backend_label: backend_label.into(),
			protocol_version: None,
		}
	}

	/// Which backend this session executes against.
	pub fn backend_label(&self) -> &str {
		&self.backend_label
	}

	/// The negotiated protocol version, if `initialize` already ran.
	pub fn protocol_version(&self) -> Option<&str> {
		self.protocol_version.as_deref()
	}

	/// The `tools/list` payload (registry + frozen schemas, verbatim).
	///
	/// Exposed separately so the zero-drift test can compare it against
	/// `dispatch::tool_names()` and `ai/schema/tools/*.json` without
	/// going through the wire.
	pub fn tool_list(&self) -> Vec<Value> {
		let schemas = dispatch::default_schemas();
		let mut tools = Vec::with_capacity(dispatch::registry().len());
		for def in dispatch::registry() {
			// The registry and the compiled schemas are kept in lockstep by
			// `oak-ai-tools`'s own drift test; a mismatch here is a build
			// defect, not a runtime condition.
			let schema = schemas.get(def.name).unwrap_or_else(|| {
				panic!("registry tool '{}' has no compiled frozen schema", def.name)
			});
			let description = schema
				.schema
				.get("description")
				.and_then(Value::as_str)
				.unwrap_or_default();
			tools.push(json!({
				"name": def.name,
				"description": description,
				"inputSchema": schema.schema.clone(),
			}));
		}
		tools
	}

	/// Handles one parsed JSON-RPC document.
	pub fn handle(&mut self, message: &Value) -> Option<Value> {
		let Some(object) = message.as_object() else {
			// A batch array or a bare scalar: valid JSON-RPC would reject
			// batches on the MCP stdio transport, and MCP needs one
			// object per frame.
			let detail = if message.is_array() {
				"batch requests are not supported on the MCP stdio transport; send one JSON-RPC object per NDJSON line"
			} else {
				"each NDJSON frame must be a JSON-RPC 2.0 object"
			};
			return Some(rpc::error(Value::Null, rpc::INVALID_REQUEST, detail, None));
		};

		let id = object.get("id").cloned();
		let method = object.get("method").and_then(Value::as_str);

		if object.get("jsonrpc").and_then(Value::as_str) != Some(rpc::JSONRPC_VERSION) {
			return Some(rpc::error(
				id.unwrap_or(Value::Null),
				rpc::INVALID_REQUEST,
				"missing or wrong \"jsonrpc\" member: must be \"2.0\"",
				None,
			));
		}

		// `id` must be a string, a number or null (JSON-RPC 2.0 §4).
		if let Some(id) = &id
			&& !(id.is_string() || id.is_number() || id.is_null())
		{
			return Some(rpc::error(
				Value::Null,
				rpc::INVALID_REQUEST,
				"\"id\" must be a string, a number or null",
				None,
			));
		}

		let params = object.get("params").cloned().unwrap_or(Value::Null);

		match (id, method) {
			(Some(id), Some(method)) => Some(self.handle_request(id, method, params)),
			(None, Some(method)) => {
				self.handle_notification(method);
				None
			}
			// A response-shaped frame: this server issues no requests, so
			// there is nothing to correlate it with. Ignoring it is the only
			// reply JSON-RPC allows (responses are never answered).
			(Some(_), None) | (None, None) => None,
		}
	}

	fn handle_request(&mut self, id: Value, method: &str, params: Value) -> Value {
		if method != "initialize" && self.protocol_version.is_none() {
			return rpc::error(
				id,
				rpc::SERVER_NOT_INITIALIZED,
				"server not initialized: send `initialize` first",
				Some(json!({"method": method})),
			);
		}
		match method {
			"initialize" => self.initialize(id, params),
			"ping" => rpc::success(id, json!({})),
			"tools/list" => rpc::success(id, json!({ "tools": self.tool_list() })),
			"tools/call" => self.tools_call(id, params),
			other => rpc::error(
				id,
				rpc::METHOD_NOT_FOUND,
				format!("unknown method '{other}'"),
				Some(json!({
					"supported": [
						"initialize",
						"notifications/initialized",
						"tools/list",
						"tools/call",
						"ping",
					],
				})),
			),
		}
	}

	fn handle_notification(&mut self, method: &str) {
		// `notifications/initialized` is the lifecycle ack; the session is
		// already usable once `initialize` was answered (the notification
		// carries no information the server needs). Cancellation is a no-op
		// because requests are handled synchronously, one frame at a time —
		// there is never a second in-flight request to cancel.
		let _ = method;
	}

	fn initialize(&mut self, id: Value, params: Value) -> Value {
		let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
			return rpc::error(
				id,
				rpc::INVALID_PARAMS,
				"initialize requires params.protocolVersion (string)",
				None,
			);
		};
		let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
			requested
		} else {
			BASELINE_PROTOCOL_VERSION
		};
		self.protocol_version = Some(negotiated.to_string());
		rpc::success(
			id,
			json!({
				"protocolVersion": negotiated,
				"capabilities": {
					"tools": { "listChanged": false },
				},
				"serverInfo": {
					"name": SERVER_NAME,
					"version": env!("CARGO_PKG_VERSION"),
				},
				"instructions": INSTRUCTIONS,
			}),
		)
	}

	fn tools_call(&mut self, id: Value, params: Value) -> Value {
		let Some(name) = params.get("name").and_then(Value::as_str) else {
			return rpc::error(
				id,
				rpc::INVALID_PARAMS,
				"tools/call requires params.name (non-empty string)",
				None,
			);
		};
		if name.is_empty() {
			return rpc::error(
				id,
				rpc::INVALID_PARAMS,
				"tools/call params.name must not be empty",
				None,
			);
		}
		let arguments = match params.get("arguments") {
			None | Some(Value::Null) => json!({}),
			Some(value) if value.is_object() => value.clone(),
			Some(other) => {
				return rpc::error(
					id,
					rpc::INVALID_PARAMS,
					format!(
						"tools/call params.arguments must be a JSON object, got {}",
						json_kind(other)
					),
					None,
				);
			}
		};

		match dispatch::execute(&mut *self.backend, name, arguments) {
			Ok(value) => {
				eprintln!("[{SERVER_NAME}] tools/call {name}: ok (backend={})", self.backend_label);
				rpc::success(id, tool_result(&value, None))
			}
			Err(err) => {
				eprintln!(
					"[{SERVER_NAME}] tools/call {name}: {} step={:?} txn={:?} (backend={})",
					err.code, err.step, err.txn, self.backend_label
				);
				rpc::success(id, tool_result(&Value::Null, Some(&err)))
			}
		}
	}
}

/// Builds the MCP `tools/call` result.
///
/// Success: `content[0].text` is the JSON of the tool payload and
/// `structuredContent` is that payload itself.
///
/// Failure: `isError: true`, `content[0].text` is the one-line summary and
/// `structuredContent` is [`ToolError::to_json`] — the same
/// `{code, message, tool, step, gap?, txn?}` object the direct execution
/// layer returns, so an MCP client re-plans from identical information.
fn tool_result(value: &Value, error: Option<&ToolError>) -> Value {
	match error {
		None => json!({
			"content": [{ "type": "text", "text": value.to_string() }],
			"isError": false,
			"structuredContent": value,
		}),
		Some(err) => json!({
			"content": [{ "type": "text", "text": error_summary(err) }],
			"isError": true,
			"structuredContent": err.to_json(),
		}),
	}
}

fn error_summary(err: &ToolError) -> String {
	let mut line = format!("{}: {}", err.code, err.message);
	let mut context: Vec<String> = Vec::new();
	if let Some(tool) = &err.tool {
		context.push(format!("tool={tool}"));
	}
	if let Some(step) = &err.step {
		context.push(format!("step={step}"));
	}
	if let Some(gap) = &err.gap {
		context.push(format!("gap={gap}"));
	}
	if let Some(txn) = &err.txn {
		context.push(format!("txn={txn}"));
	}
	if !context.is_empty() {
		line.push_str(&format!(" [{}]", context.join(" ")));
	}
	line
}

fn json_kind(value: &Value) -> &'static str {
	match value {
		Value::Null => "null",
		Value::Bool(_) => "boolean",
		Value::Number(_) => "number",
		Value::String(_) => "string",
		Value::Array(_) => "array",
		Value::Object(_) => "object",
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use oak_ai_tools::mock::MockBackend;

	fn server() -> Server {
		Server::new(Box::new(MockBackend::new()), "mock")
	}

	fn init(server: &mut Server) -> Value {
		server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 1,
				"method": "initialize",
				"params": {
					"protocolVersion": "2024-11-05",
					"capabilities": {},
					"clientInfo": { "name": "unit-test", "version": "0" },
				},
			}))
			.expect("initialize is a request")
	}

	#[test]
	fn initialize_negotiates_and_advertises_tools() {
		let mut server = server();
		let response = init(&mut server);
		assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
		assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
		assert_eq!(response["result"]["capabilities"]["tools"]["listChanged"], false);
		assert_eq!(server.protocol_version(), Some("2024-11-05"));
	}

	#[test]
	fn unknown_protocol_version_falls_back_to_the_baseline() {
		let mut server = server();
		let response = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 2,
				"method": "initialize",
				"params": { "protocolVersion": "1999-01-01" },
			}))
			.unwrap();
		assert_eq!(response["result"]["protocolVersion"], BASELINE_PROTOCOL_VERSION);
	}

	#[test]
	fn requests_before_initialize_are_rejected() {
		let mut server = server();
		let response = server
			.handle(&json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"}))
			.unwrap();
		assert_eq!(response["error"]["code"], rpc::SERVER_NOT_INITIALIZED);
		assert_eq!(response["id"], 7);
	}

	#[test]
	fn initialized_notification_has_no_response_and_keeps_the_session() {
		let mut server = server();
		init(&mut server);
		let response =
			server.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
		assert!(response.is_none(), "notifications must not be answered");
		let response = server
			.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}))
			.unwrap();
		assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 22);
	}

	#[test]
	fn ping_is_answered() {
		let mut server = server();
		init(&mut server);
		let response = server
			.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}))
			.unwrap();
		assert_eq!(response["result"], json!({}));
	}

	#[test]
	fn unknown_method_is_method_not_found() {
		let mut server = server();
		init(&mut server);
		let response = server
			.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/teleport"}))
			.unwrap();
		assert_eq!(response["error"]["code"], rpc::METHOD_NOT_FOUND);
	}

	#[test]
	fn wrong_jsonrpc_member_is_invalid_request() {
		let mut server = server();
		init(&mut server);
		let response = server
			.handle(&json!({"jsonrpc": "1.0", "id": 6, "method": "tools/list"}))
			.unwrap();
		assert_eq!(response["error"]["code"], rpc::INVALID_REQUEST);
	}

	#[test]
	fn batch_frames_are_rejected() {
		let mut server = server();
		init(&mut server);
		let response = server.handle(&json!([])).unwrap();
		assert_eq!(response["error"]["code"], rpc::INVALID_REQUEST);
	}

	#[test]
	fn tools_call_requires_a_name_and_object_arguments() {
		let mut server = server();
		init(&mut server);
		let response = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 8,
				"method": "tools/call",
				"params": { "arguments": {} },
			}))
			.unwrap();
		assert_eq!(response["error"]["code"], rpc::INVALID_PARAMS);

		let response = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 9,
				"method": "tools/call",
				"params": { "name": "project.open", "arguments": [] },
			}))
			.unwrap();
		assert_eq!(response["error"]["code"], rpc::INVALID_PARAMS);
		assert!(
			response["error"]["message"].as_str().unwrap().contains("array"),
			"message should name the offending type: {response}"
		);
	}

	#[test]
	fn success_and_failure_share_the_tool_result_shape() {
		let mut server = server();
		init(&mut server);

		let ok = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 10,
				"method": "tools/call",
				"params": { "name": "project.open", "arguments": { "name": "unit", "fps": 30 } },
			}))
			.unwrap();
		assert_eq!(ok["result"]["isError"], false);
		assert_eq!(ok["result"]["structuredContent"]["created"], true);
		assert_eq!(ok["result"]["content"][0]["type"], "text");

		// Schema-level rejection: no transaction is started.
		let bad = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 11,
				"method": "tools/call",
				"params": {
					"name": "timeline.split_clip",
					"arguments": { "clip": "clip-1", "time": 1.5 },
				},
			}))
			.unwrap();
		assert_eq!(bad["result"]["isError"], true);
		let error = &bad["result"]["structuredContent"];
		assert_eq!(error["code"], "SCHEMA_VALIDATION_FAILED");
		assert_eq!(error["step"], "validate");
		assert_eq!(error["tool"], "timeline.split_clip");
		assert!(error.get("txn").is_none(), "schema failures open no txn");
	}

	#[test]
	fn backend_failure_is_marked_aborted() {
		let mut server = server();
		init(&mut server);
		server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 12,
				"method": "tools/call",
				"params": { "name": "project.open", "arguments": { "name": "unit", "fps": 30 } },
			}))
			.unwrap();
		let response = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 13,
				"method": "tools/call",
				"params": {
					"name": "timeline.split_clip",
					"arguments": { "clip": "clip-999", "time": { "num": 1, "den": 1 } },
				},
			}))
			.unwrap();
		let error = &response["result"]["structuredContent"];
		assert_eq!(response["result"]["isError"], true);
		assert_eq!(error["code"], "NOT_FOUND");
		assert_eq!(error["step"], "invoke");
		assert_eq!(error["txn"], "aborted");
	}

	#[test]
	fn unknown_tool_keeps_the_structured_lookup_error() {
		let mut server = server();
		init(&mut server);
		let response = server
			.handle(&json!({
				"jsonrpc": "2.0",
				"id": 14,
				"method": "tools/call",
				"params": { "name": "timeline.teleport", "arguments": {} },
			}))
			.unwrap();
		assert_eq!(response["result"]["isError"], true);
		let error = &response["result"]["structuredContent"];
		assert_eq!(error["code"], "UNKNOWN_TOOL");
		assert_eq!(error["step"], "lookup");
	}


}

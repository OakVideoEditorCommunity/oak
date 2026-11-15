//! rmcp adapter.
//!
//! The official MCP SDK (`rmcp`) owns the parts that are protocol plumbing:
//! the stdio transport, NDJSON framing, JSON-RPC error envelopes, the
//! lifecycle (initialize handshake, notifications, ping) and request routing.
//! What it must *not* own here is the frozen tool face: the 22 tool schemas,
//! the protocol-version negotiation rule and the `tools/call` result shape are
//! part of this repo's contract (`ai/schema/tools/*.json`). So every protocol
//! operation is delegated to [`Server::handle`], which already implements them
//! exactly, and its JSON result is deserialised into the SDK's types.
//!
//! Net effect: same bytes on the wire, ~600 lines of hand-rolled protocol code
//! retired in favour of the crate.

use std::sync::Mutex;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
	CallToolRequestParams, CallToolResponse, CallToolResult, InitializeRequestParams,
	InitializeResult, ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};
use serde_json::{json, Map, Value};

use crate::server::Server;

/// Wraps the frozen [`Server`] behind the MCP SDK handler trait.
pub struct OakMcp {
	server: Mutex<Server>,
}

impl OakMcp {
	pub fn new(server: Server) -> Self {
		Self { server: Mutex::new(server) }
	}

	/// Runs one JSON-RPC request through the frozen core and returns its
	/// `result` member, turning a JSON-RPC `error` into an [`McpError`].
	fn call(&self, method: &str, params: Value) -> Result<Value, McpError> {
		let message = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
		let response = self
			.server
			.lock()
			.expect("oak-ai-mcp server mutex")
			.handle(&message)
			.ok_or_else(|| McpError::internal_error("frozen core produced no response", None))?;
		if let Some(error) = response.get("error") {
			let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32603) as i32;
			let message = error
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or("protocol error")
				.to_string();
			return Err(McpError::new(rmcp::model::ErrorCode(code), message, None));
		}
		Ok(response.get("result").cloned().unwrap_or(Value::Null))
	}
}

impl ServerHandler for OakMcp {
	async fn initialize(
		&self,
		request: InitializeRequestParams,
		_context: RequestContext<RoleServer>,
	) -> Result<InitializeResult, McpError> {
		let params = serde_json::to_value(&request)
			.map_err(|e| McpError::internal_error(format!("serialise initialize: {e}"), None))?;
		let result = self.call("initialize", params)?;
		serde_json::from_value(result)
			.map_err(|e| McpError::internal_error(format!("initialize result: {e}"), None))
	}

	async fn list_tools(
		&self,
		_request: Option<PaginatedRequestParams>,
		_context: RequestContext<RoleServer>,
	) -> Result<ListToolsResult, McpError> {
		let tools = self
			.server
			.lock()
			.expect("oak-ai-mcp server mutex")
			.tool_list()
			.into_iter()
			.filter_map(tool_from_value)
			.collect::<Vec<_>>();
		Ok(ListToolsResult::with_all_items(tools))
	}

	async fn call_tool(
		&self,
		request: CallToolRequestParams,
		_context: RequestContext<RoleServer>,
	) -> Result<CallToolResponse, McpError> {
		let params = json!({
			"name": request.name,
			"arguments": request.arguments.map(Value::Object).unwrap_or(Value::Null),
		});
		let result = self.call("tools/call", params)?;
		let out: CallToolResult = serde_json::from_value(result)
			.map_err(|e| McpError::internal_error(format!("tools/call result: {e}"), None))?;
		Ok(CallToolResponse::Complete(out))
	}
}

/// One `tool_list()` entry (`{name, description, inputSchema}`) to the SDK's
/// [`Tool`]. The `inputSchema` is passed through verbatim, which is what keeps
/// the frozen face frozen.
fn tool_from_value(value: Value) -> Option<Tool> {
	let name = value.get("name")?.as_str()?.to_string();
	let description = value
		.get("description")
		.and_then(Value::as_str)
		.unwrap_or_default()
		.to_string();
	let schema = value.get("inputSchema")?.clone();
	let input_schema: Map<String, Value> = schema.as_object().cloned().unwrap_or_default();
	Some(Tool::new(name, description, input_schema))
}

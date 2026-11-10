//! OpenAI-compatible chat-completions client (T8.1).
//!
//! The protocol surface is deliberately small: one request shape, one
//! response shape, one error enum. [`LlmClient`] is the seam every test
//! injects a scripted mock through, so the agent loop never needs a network.

use std::fmt;

use oak_ai_tools::schema::SchemaRegistry;
use serde_json::{json, Value};

use super::config::AiConfig;

/// A chat message in wire shape (`role` + whichever optional block the
/// role needs).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
	pub role: String,
	pub content: Option<String>,
	/// `assistant` messages that asked for tool calls.
	pub tool_calls: Vec<ToolCall>,
	/// `tool` messages: which call this result answers.
	pub tool_call_id: Option<String>,
}

impl ChatMessage {
	pub fn system(text: impl Into<String>) -> Self {
		Self {
			role: "system".into(),
			content: Some(text.into()),
			tool_calls: Vec::new(),
			tool_call_id: None,
		}
	}

	pub fn user(text: impl Into<String>) -> Self {
		Self {
			role: "user".into(),
			content: Some(text.into()),
			tool_calls: Vec::new(),
			tool_call_id: None,
		}
	}

	pub fn assistant(text: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
		Self {
			role: "assistant".into(),
			content: text,
			tool_calls,
			tool_call_id: None,
		}
	}

	pub fn tool_result(tool_call_id: impl Into<String>, payload: &Value) -> Self {
		Self {
			role: "tool".into(),
			// Tool results are always a JSON string on the wire; the model
			// reads them as text.
			content: Some(payload.to_string()),
			tool_calls: Vec::new(),
			tool_call_id: Some(tool_call_id.into()),
		}
	}

	/// The openai-completions message object.
	pub fn to_wire(&self) -> Value {
		let mut msg = json!({ "role": self.role });
		if let Some(content) = &self.content {
			msg["content"] = json!(content);
		}
		if !self.tool_calls.is_empty() {
			msg["tool_calls"] = Value::Array(
				self.tool_calls
					.iter()
					.map(|tc| {
						json!({
							"id": tc.id,
							"type": "function",
							"function": {
								"name": tc.name,
								// Arguments are a JSON *string* in the protocol.
								"arguments": tc.arguments.to_string(),
							}
						})
					})
					.collect(),
			);
		}
		if let Some(id) = &self.tool_call_id {
			msg["tool_call_id"] = json!(id);
		}
		msg
	}
}

/// One tool call requested by the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
	pub id: String,
	pub name: String,
	/// Parsed arguments (already `Value`; a malformed JSON object becomes
	/// `Value::Null` and is rejected later by schema validation).
	pub arguments: Value,
}

/// One assistant turn as returned by the API.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatResponse {
	pub content: Option<String>,
	pub tool_calls: Vec<ToolCall>,
	pub finish_reason: Option<String>,
}

/// A single `chat/completions` call.
#[derive(Debug, Clone)]
pub struct ChatRequest<'a> {
	pub model: &'a str,
	pub messages: &'a [ChatMessage],
	/// Openai `tools` array (from [`openai_tools`]).
	pub tools: &'a [Value],
}

/// Structured client failure. No variant carries the key; HTTP errors keep
/// the URL but never the `Authorization` header.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
	/// Configuration is missing (propagated from [`crate::ai::config`]).
	NotConfigured,
	/// Transport-level failure (DNS, TLS, timeout…).
	Http(String),
	/// Non-2xx response with a readable body.
	Api { status: u16, message: String },
	/// The response body was not the documented shape.
	Parse(String),
}

impl LlmError {
	pub fn code(&self) -> &'static str {
		match self {
			Self::NotConfigured => "AI_NOT_CONFIGURED",
			Self::Http(_) => "AI_HTTP_ERROR",
			Self::Api { .. } => "AI_API_ERROR",
			Self::Parse(_) => "AI_BAD_RESPONSE",
		}
	}
}

impl fmt::Display for LlmError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::NotConfigured => write!(f, "AI backend is not configured"),
			Self::Http(e) => write!(f, "AI request failed: {e}"),
			Self::Api { status, message } => {
				write!(f, "AI API returned {status}: {message}")
			}
			Self::Parse(e) => write!(f, "AI response was not understood: {e}"),
		}
	}
}

impl std::error::Error for LlmError {}

/// The seam: anything that can answer one chat turn.
pub trait LlmClient {
	fn complete(&self, request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError>;
}

/// Builds the openai `tools` array from the frozen 22-tool schema registry.
///
/// Each tool file is already a JSON Schema document; its `description`
/// moves into the function envelope and the rest becomes `parameters`.
pub fn openai_tools(registry: &SchemaRegistry) -> Vec<Value> {
	registry
		.names()
		.into_iter()
		.filter_map(|name| registry.get(name))
		.map(|tool| {
			let description = tool
				.schema
				.get("description")
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string();
			let mut parameters = tool.schema.clone();
			if let Some(obj) = parameters.as_object_mut() {
				// These describe the file, not the argument object.
				obj.remove("name");
				obj.remove("description");
				obj.remove("$schema");
			}
			json!({
				"type": "function",
				"function": {
					"name": tool.name,
					"description": description,
					"parameters": parameters,
				}
			})
		})
		.collect()
}

/// Parses the documented response shape (`choices[0].message`).
pub fn parse_chat_response(body: &Value) -> Result<ChatResponse, LlmError> {
	let message = body
		.get("choices")
		.and_then(Value::as_array)
		.and_then(|choices| choices.first())
		.and_then(|choice| choice.get("message"))
		.ok_or_else(|| LlmError::Parse("missing choices[0].message".into()))?;

	let content = message
		.get("content")
		.and_then(Value::as_str)
		.map(str::to_string)
		.filter(|s| !s.is_empty());

	let tool_calls = message
		.get("tool_calls")
		.and_then(Value::as_array)
		.map(|calls| {
			calls
				.iter()
				.filter_map(|call| {
					let id = call.get("id")?.as_str()?.to_string();
					let function = call.get("function")?;
					let name = function.get("name")?.as_str()?.to_string();
					let raw = function
						.get("arguments")
						.and_then(Value::as_str)
						.unwrap_or("{}");
					let arguments = serde_json::from_str(raw).unwrap_or(Value::Null);
					Some(ToolCall {
						id,
						name,
						arguments,
					})
				})
				.collect()
		})
		.unwrap_or_default();

	let finish_reason = body
		.get("choices")
		.and_then(Value::as_array)
		.and_then(|choices| choices.first())
		.and_then(|choice| choice.get("finish_reason"))
		.and_then(Value::as_str)
		.map(str::to_string);

	Ok(ChatResponse {
		content,
		tool_calls,
		finish_reason,
	})
}

/// Blocking openai-completions client over [`ureq`] (already a dependency
/// for the update checker).
///
/// Never used in tests: the agent loop talks to [`LlmClient`], and tests
/// inject a scripted mock instead of a transport.
pub struct UreqClient {
	agent: ureq::Agent,
	base_url: String,
	model: String,
	/// Held only to be read into the Authorization header per request.
	#[allow(dead_code)]
	config: AiConfig,
}

impl UreqClient {
	pub fn new(config: AiConfig) -> Self {
		let agent_config = ureq::Agent::config_builder()
			.timeout_global(Some(std::time::Duration::from_secs(60)))
			.build();
		Self {
			agent: agent_config.into(),
			base_url: config.base_url.trim_end_matches('/').to_string(),
			model: config.model.clone(),
			config,
		}
	}

	/// The model name this client sends (the agent keeps its own copy in
	/// the request; this is for diagnostics).
	pub fn model(&self) -> &str {
		&self.model
	}
}

impl LlmClient for UreqClient {
	fn complete(&self, request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError> {
		let url = format!("{}/chat/completions", self.base_url);
		let body = json!({
			"model": request.model,
			"messages": request.messages.iter().map(ChatMessage::to_wire).collect::<Vec<_>>(),
			"tools": request.tools,
			"stream": false,
		});
		let payload = serde_json::to_string(&body)
			.map_err(|e| LlmError::Parse(e.to_string()))?;
		let mut response = self
			.agent
			.post(&url)
			.header(
				"authorization",
				&format!("Bearer {}", self.config.api_key),
			)
			.header("content-type", "application/json")
			.send(payload)
			.map_err(|e| LlmError::Http(e.to_string()))?;
		let status = response.status().as_u16();
		let text = response
			.body_mut()
			.read_to_string()
			.map_err(|e| LlmError::Http(e.to_string()))?;
		if !(200..300).contains(&status) {
			return Err(LlmError::Api {
				status,
				message: truncate(&text, 512),
			});
		}
		let parsed: Value =
			serde_json::from_str(&text).map_err(|e| LlmError::Parse(e.to_string()))?;
		parse_chat_response(&parsed)
	}
}

fn truncate(s: &str, max: usize) -> String {
	if s.len() <= max {
		s.to_string()
	} else {
		let mut end = max;
		while !s.is_char_boundary(end) {
			end -= 1;
		}
		s[..end].to_string()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The documented response shape, tool calls included.
	#[test]
	fn parses_tool_calls_and_content() {
		let body = json!({
			"choices": [{
				"message": {
					"content": null,
					"tool_calls": [{
						"id": "call_1",
						"type": "function",
						"function": {"name": "node.list_types", "arguments": "{}"}
					}]
				},
				"finish_reason": "tool_calls"
			}]
		});
		let resp = parse_chat_response(&body).expect("parses");
		assert_eq!(resp.tool_calls.len(), 1);
		assert_eq!(resp.tool_calls[0].name, "node.list_types");
		assert_eq!(resp.tool_calls[0].arguments, json!({}));
		assert_eq!(resp.finish_reason.as_deref(), Some("tool_calls"));
	}

	/// Malformed arguments degrade to `Null` (schema validation rejects
	/// them later) instead of failing the whole turn.
	#[test]
	fn malformed_arguments_become_null() {
		let body = json!({
			"choices": [{
				"message": {"tool_calls": [{
					"id": "c", "function": {"name": "x", "arguments": "{oops"}
				}]}
			}]
		});
		let resp = parse_chat_response(&body).expect("parses");
		assert_eq!(resp.tool_calls[0].arguments, Value::Null);
	}

	/// A body without `choices` is a structured parse error, not a panic.
	#[test]
	fn missing_choices_is_structured() {
		let err = parse_chat_response(&json!({"error": "quota"})).unwrap_err();
		assert_eq!(err.code(), "AI_BAD_RESPONSE");
	}

	/// The frozen registry converts to a 22-entry openai tools array with
	/// each function named after its tool.
	#[test]
	fn openai_tools_covers_the_frozen_registry() {
		let registry = SchemaRegistry::load_default();
		let tools = openai_tools(&registry);
		assert_eq!(tools.len(), 22, "frozen T0 face");
		assert_eq!(tools[0]["type"], "function");
		let name = tools[0]["function"]["name"].as_str().unwrap();
		assert!(registry.get(name).is_some(), "unknown name {name}");
		// parameters must still be an object schema
		assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
	}
}

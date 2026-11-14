//! JSON Schemas of the v2 tools (T9.2) — a **separate** face from
//! [`crate::schema`], which stays frozen at the 22 tools of T0.
//!
//! The documents are embedded rather than loaded from `ai/schema/tools/`
//! so the frozen schema directory keeps describing exactly the frozen 22:
//! `dispatch::tests::registry_drift` asserts a one-to-one match against
//! that directory, and adding files there would make the frozen contract
//! and the frozen registry disagree.
//!
//! Shape: each entry is the *parameters* object of an OpenAI function tool
//! plus the `description` the envelope carries.

use serde_json::{Value, json};

/// One v2 tool: its name, description and JSON Schema.
pub struct V2Tool {
	pub name: &'static str,
	pub description: &'static str,
	pub parameters: fn() -> Value,
}

/// The v2 tool table.
pub fn v2_tools() -> &'static [V2Tool] {
	static TOOLS: [V2Tool; 2] = [
		V2Tool {
			name: "sequence.get_settings",
			description: "Read a sequence's settings: name, width, height, \
				frame rate (exact rational), interlaced flag and duration.",
			parameters: get_settings_schema,
		},
		V2Tool {
			name: "sequence.set_format",
			description: "Change a sequence's settings (frame rate, size, name, \
				interlaced). Only the fields you pass change; the edit is one \
				undoable step.",
			parameters: set_format_schema,
		},
	];

	&TOOLS
}

fn get_settings_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"sequence": {
				"type": ["string", "integer"],
				"description": "Sequence id (string or numeric); omit for the current sequence."
			}
		},
		"additionalProperties": false
	})
}

/// The write schema.
///
/// Known keys, in two classes:
/// * **writable** — `sequence`, `name`, `width`, `height`, `fps`, `rate`
///   (`rate` is the alias under which `get_settings` reports it), `interlaced`;
/// * **read-only echo** — `id`, `duration`, `backend`, `undoable`: what
///   `sequence.get_settings` returns. They are accepted and silently ignored,
///   because the obvious model behaviour is to read the settings and write
///   them straight back; the keys carry no writable meaning, so ignoring
///   them is unambiguous.
///
/// `additionalProperties` stays **false** on purpose: a key that *looks*
/// writable but is misspelled (`frameRate`) must be refused and named, never
/// silently dropped. Ignorance is only licensed for the read-only echo.
fn set_format_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"sequence": {
				"type": ["string", "integer"],
				"description": "Sequence id (string or numeric); omit for the current sequence."
			},
			"name": { "type": "string", "description": "New display name." },
			"width": { "type": "integer", "minimum": 1, "description": "Frame width in pixels." },
			"height": { "type": "integer", "minimum": 1, "description": "Frame height in pixels." },
			"fps": {
				"description": "Frames per second. Pass {\"num\":30000,\"den\":1001} for an exact rate, or a number for a standard rate (23.976, 24, 25, 29.97, 30, 50, 59.94, 60).",
				"oneOf": [
					{ "type": "number", "exclusiveMinimum": 0 },
					{
						"type": "string",
						"description": "A quoted number, e.g. \"60\" or \"29.97\". Numeric validity is decided by the parser, which answers with an actionable error."
					},
					{
						"type": "object",
						"properties": {
							"num": { "type": "integer", "minimum": 1 },
							"den": { "type": "integer", "minimum": 1 }
						},
						"required": ["num"],
						"additionalProperties": false
					}
				]
			},
			"interlaced": { "type": "boolean", "description": "Interlaced flag." },
			"rate": {
				"description": "Alias of `fps` (the field name `sequence.get_settings` returns), handled identically.",
				"oneOf": [
					{ "type": "number", "exclusiveMinimum": 0 },
					{ "type": "string" },
					{
						"type": "object",
						"properties": {
							"num": { "type": "integer", "minimum": 1 },
							"den": { "type": "integer", "minimum": 1 }
						},
						"required": ["num"],
						"additionalProperties": false
					}
				]
			},
			"id": { "description": "Read-only echo from `sequence.get_settings`; ignored on write." },
			"duration": { "description": "Read-only echo from `sequence.get_settings`; ignored on write." },
			"backend": { "description": "Read-only echo from `sequence.get_settings`; ignored on write." },
			"undoable": { "description": "Read-only echo from `sequence.get_settings`; ignored on write." }
		},
		"additionalProperties": false
	})
}

/// The v2 tools in OpenAI `tools` array form (the same envelope
/// [`crate::dispatch`]'s frozen 22 use).
pub fn v2_openai_tools() -> Vec<Value> {
	v2_tools()
		.iter()
		.map(|tool| {
			json!({
				"type": "function",
				"function": {
					"name": tool.name,
					"description": tool.description,
					"parameters": (tool.parameters)(),
				}
			})
		})
		.collect()
}

/// Runtime validation of a v2 call (T9.3): the frozen 22 are validated by
/// [`crate::schema`] before the backend is entered; the v2 face must not be
/// laxer, or `additionalProperties: false` becomes decoration.
///
/// `additionalProperties` stays **false** on purpose: a misspelled key
/// (`frameRate`) must be refused rather than silently ignored, and the
/// message names the offending key so the model can fix it in one turn.
pub fn validate(tool: &str, args: &Value) -> Result<(), crate::error::ToolError> {
	use std::sync::OnceLock;

	static COMPILED: OnceLock<std::collections::BTreeMap<&'static str, jsonschema::Validator>> =
		OnceLock::new();

	let compiled = COMPILED.get_or_init(|| {
		v2_tools()
			.iter()
			.map(|tool| {
				let schema = (tool.parameters)();
				let validator = jsonschema::Validator::new(&schema).unwrap_or_else(|e| {
					panic!("v2 schema for '{}' does not compile: {e}", tool.name)
				});
				(tool.name, validator)
			})
			.collect()
	});

	let Some(validator) = compiled.get(tool) else {
		return Err(crate::error::ToolError::new(
			crate::error::CODE_UNKNOWN_TOOL,
			format!("no v2 schema for tool '{tool}'"),
		));
	};
	let mut messages: Vec<String> = Vec::new();
	for (index, error) in validator.iter_errors(args).enumerate() {
		if index == 4 {
			messages.push("...".to_string());
			break;
		}
		let path = error.instance_path().to_string();
		let path = if path.is_empty() { "$".to_string() } else { path };
		// `additionalProperties` errors name the rejected keys in their own
		// text; keeping it verbatim is what lets the model see `frameRate`.
		messages.push(format!("{path}: {error}"));
	}
	if messages.is_empty() {
		return Ok(());
	}
	Err(crate::error::ToolError::new(
		crate::error::CODE_SCHEMA_VALIDATION,
		format!(
			"arguments rejected by the schema of '{tool}': {}. Accepted keys: {}. \
			 Received: {args}",
			messages.join("; "),
			accepted_keys(tool).join(", ")
		),
	))
}

/// The keys a v2 tool accepts, for the actionable error above.
pub fn accepted_keys(tool: &str) -> Vec<String> {
	v2_tools()
		.iter()
		.find(|t| t.name == tool)
		.and_then(|t| (t.parameters)().get("properties").cloned())
		.and_then(|p| p.as_object().map(|o| o.keys().cloned().collect()))
		.unwrap_or_default()
}

/// `true` when `tool` belongs to the v2 face.
pub fn is_v2_tool(tool: &str) -> bool {
	v2_tools().iter().any(|t| t.name == tool)
}

/// The names of the v2 tools.
pub fn v2_tool_names() -> Vec<&'static str> {
	v2_tools().iter().map(|t| t.name).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_v2_face_is_disjoint_from_the_frozen_22() {
		let frozen = crate::dispatch::tool_names();
		for name in v2_tool_names() {
			assert!(
				!frozen.contains(&name),
				"{name} must not shadow a frozen tool"
			);
		}
	}

	#[test]
	fn every_v2_tool_carries_a_parameters_object() {
		for tool in v2_tools() {
			let params = (tool.parameters)();
			assert_eq!(params["type"], "object", "{}", tool.name);
			assert!(params["properties"].is_object(), "{}", tool.name);
			assert!(!tool.description.is_empty(), "{}", tool.name);
		}
	}

	/// The read-only echo list and the write schema must not drift: every
	/// key `get_settings` reports is admitted here (and ignored on write),
	/// while nothing else is.
	#[test]
	fn the_write_schema_admits_exactly_the_read_only_echo() {
		let params = set_format_schema();
		let props = params["properties"].as_object().expect("properties");
		for key in crate::tools::v2::sequence::READ_ONLY_ECHO_KEYS {
			assert!(
				props.contains_key(key),
				"`{key}` is a read-only echo of get_settings and must be admitted"
			);
		}
		// The writable keys plus the echo — nothing extra, nothing missing.
		let mut expected: Vec<&str> = vec![
			"sequence", "name", "width", "height", "fps", "rate", "interlaced",
		];
		expected.extend(crate::tools::v2::sequence::READ_ONLY_ECHO_KEYS);
		let mut actual: Vec<&str> = props.keys().map(String::as_str).collect();
		expected.sort_unstable();
		actual.sort_unstable();
		assert_eq!(actual, expected, "the write schema's key set changed");
		assert_eq!(params["additionalProperties"], serde_json::json!(false));
	}

	#[test]
	fn openai_envelopes_name_every_v2_tool() {
		let tools = v2_openai_tools();
		assert_eq!(tools.len(), 2);
		for tool in &tools {
			assert_eq!(tool["type"], "function");
			let name = tool["function"]["name"].as_str().expect("a name");
			assert!(is_v2_tool(name), "{name}");
		}
	}
}

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
				"type": "string",
				"description": "Sequence id; omit for the current sequence."
			}
		},
		"additionalProperties": false
	})
}

fn set_format_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"sequence": {
				"type": "string",
				"description": "Sequence id; omit for the current sequence."
			},
			"name": { "type": "string", "description": "New display name." },
			"width": { "type": "integer", "minimum": 1, "description": "Frame width in pixels." },
			"height": { "type": "integer", "minimum": 1, "description": "Frame height in pixels." },
			"fps": {
				"description": "Frames per second. Pass {\"num\":30000,\"den\":1001} for an exact rate, or a number for a standard rate (23.976, 24, 25, 29.97, 30, 50, 59.94, 60).",
				"oneOf": [
					{ "type": "number", "exclusiveMinimum": 0 },
					{
						"type": "object",
						"properties": {
							"num": { "type": "integer", "minimum": 1 },
							"den": { "type": "integer", "minimum": 1 }
						},
						"required": ["num", "den"],
						"additionalProperties": false
					}
				]
			},
			"interlaced": { "type": "boolean", "description": "Interlaced flag." }
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

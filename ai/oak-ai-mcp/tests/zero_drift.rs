//! Anti-drift gate: the MCP `tools/list` face vs. the direct execution
//! layer vs. the frozen schema directory.
//!
//! Three sources must agree **bidirectionally**, one-to-one:
//!
//! 1. `tools/list` as an MCP client sees it (driven through the real
//!    server handler over `dispatch::default_schemas()`);
//! 2. `dispatch::tool_names()` (the compiled registry);
//! 3. the file stems of `ai/schema/tools/*.json` (the frozen T0 contract).
//!
//! On top of the name sets, every `inputSchema` must equal the schema file
//! document exactly, and every `description` must be the file's
//! description. A tool added to one source only, or an MCP layer that
//! rewrites/filters/normalises a schema, turns this test red.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use oak_ai_mcp::server::Server;
use oak_ai_tools::dispatch;
use oak_ai_tools::mock::MockBackend;
use serde_json::{Value, json};

/// `ai/schema/tools`, resolved from the manifest dir (no absolute paths).
fn schema_dir() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../schema/tools")
}

/// Tool name -> raw schema document, read straight from disk.
fn schema_files() -> Vec<(String, Value)> {
	let dir = schema_dir();
	let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
		.unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()))
		.filter_map(|entry| entry.ok().map(|e| e.path()))
		.filter(|path| path.extension().is_some_and(|ext| ext == "json"))
		.collect();
	entries.sort();
	assert!(!entries.is_empty(), "no schema files in {}", dir.display());
	entries
		.into_iter()
		.map(|path| {
			let stem = path
				.file_stem()
				.expect("schema stem")
				.to_string_lossy()
				.to_string();
			let text = std::fs::read_to_string(&path)
				.unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
			let value: Value = serde_json::from_str(&text)
				.unwrap_or_else(|err| panic!("{} is not JSON: {err}", path.display()));
			(stem, value)
		})
		.collect()
}

/// The `tools/list` result as the wire would carry it: initialize, then
/// `tools/list` through the same handler the binary uses.
fn tools_list_over_mcp() -> Vec<Value> {
	let mut server = Server::new(Box::new(MockBackend::new()), "mock");
	let init = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": "initialize",
			"params": {
				"protocolVersion": "2024-11-05",
				"capabilities": {},
				"clientInfo": { "name": "zero-drift-test", "version": "0" },
			},
		}))
		.expect("initialize is a request");
	assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 2,
			"method": "tools/list",
			"params": {},
		}))
		.expect("tools/list is a request");
	assert!(response.get("error").is_none(), "tools/list failed: {response}");
	let tools = response["result"]["tools"]
		.as_array()
		.unwrap_or_else(|| panic!("tools/list result has no tools array: {response}"))
		.clone();

	// `Server::tool_list` is the same source the handler uses; keep both in
	// sync so a refactor cannot silently special-case the wire path.
	assert_eq!(tools, server.tool_list());
	tools
}

fn name_set(names: impl IntoIterator<Item = String>) -> BTreeSet<String> {
	names.into_iter().collect()
}

#[test]
fn mcp_registry_and_schema_dir_are_sets_equal_with_count_22() {
	let tools = tools_list_over_mcp();
	let mcp_names = name_set(
		tools
			.iter()
			.map(|tool| tool["name"].as_str().expect("tool name").to_string()),
	);
	let registry_names = name_set(dispatch::tool_names().iter().map(|n| n.to_string()));
	let file_names = name_set(schema_files().into_iter().map(|(name, _)| name));

	assert_eq!(file_names.len(), 22, "frozen schema dir must hold 22 tools");
	assert_eq!(registry_names.len(), 22, "registry must hold 22 tools");
	assert_eq!(mcp_names.len(), 22, "tools/list must advertise 22 tools");
	assert_eq!(
		tools.len(),
		mcp_names.len(),
		"tools/list must not list a name twice"
	);

	// tools/list ⊆ registry (nothing invented by the MCP layer).
	let invented: Vec<&String> = mcp_names.difference(&registry_names).collect();
	assert!(invented.is_empty(), "tools/list names absent from the registry: {invented:?}");

	// registry ⊆ tools/list (nothing hidden by the MCP layer).
	let hidden: Vec<&String> = registry_names.difference(&mcp_names).collect();
	assert!(hidden.is_empty(), "registry tools missing from tools/list: {hidden:?}");

	// registry == schema files, both directions.
	let unwired: Vec<&String> = registry_names.difference(&file_names).collect();
	assert!(unwired.is_empty(), "registry tools without a frozen schema: {unwired:?}");
	let orphans: Vec<&String> = file_names.difference(&registry_names).collect();
	assert!(orphans.is_empty(), "frozen schemas with no registry entry: {orphans:?}");

	// Explicit full equality so a regression names the pair that broke.
	assert_eq!(mcp_names, registry_names, "MCP face drifted from the registry");
	assert_eq!(registry_names, file_names, "registry drifted from the schema dir");
}

#[test]
fn every_input_schema_is_the_frozen_document_verbatim() {
	let tools = tools_list_over_mcp();
	let files: std::collections::BTreeMap<String, Value> = schema_files().into_iter().collect();
	assert_eq!(tools.len(), files.len());

	for tool in &tools {
		let name = tool["name"].as_str().expect("tool name");
		let file = files
			.get(name)
			.unwrap_or_else(|| panic!("tools/list advertised '{name}' with no schema file"));

		let input_schema = tool
			.get("inputSchema")
			.unwrap_or_else(|| panic!("'{name}' has no inputSchema"));
		assert_eq!(
			input_schema, file,
			"'{name}': MCP inputSchema is not the frozen schema document"
		);
		assert_eq!(input_schema["type"], "object", "'{name}' inputSchema must be an object schema");
		assert_eq!(
			input_schema["title"], name,
			"'{name}': schema title must equal the tool name"
		);

		// The MCP `description` is the schema's own description, not a
		// second hand-written blurb that could go stale.
		let expected = file.get("description").and_then(Value::as_str).unwrap_or("");
		assert_eq!(
			tool["description"].as_str().unwrap_or(""),
			expected,
			"'{name}': description drifted from the schema file"
		);
	}
}

#[test]
fn tool_list_is_stable_across_two_calls() {
	// `listChanged` is advertised as false, so the face must be immutable
	// for the lifetime of the process.
	let first = tools_list_over_mcp();
	let second = tools_list_over_mcp();
	assert_eq!(first, second, "tools/list changed between calls");
}

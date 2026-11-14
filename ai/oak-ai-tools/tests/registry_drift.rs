//! Anti-drift gate between the dispatch registry (code) and the frozen
//! T0 schema directory (`ai/schema/tools/*.json`).
//!
//! The assertion is a **bidirectional, one-to-one set equality** with a
//! count of 22: a schema added/removed without touching the registry, or
//! a registry entry added/removed without a schema, turns this test red.

use std::collections::BTreeSet;
use std::path::Path;

use oak_ai_tools::dispatch;

fn schema_dir() -> std::path::PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai/schema/tools")
}

fn schema_names() -> BTreeSet<String> {
	let dir = schema_dir();
	let mut names = BTreeSet::new();
	let entries = std::fs::read_dir(&dir)
		.unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()));
	for entry in entries {
		let path = entry.expect("schema entry").path();
		if path.extension().is_some_and(|ext| ext == "json") {
			let stem = path.file_stem().expect("stem").to_string_lossy().to_string();
			names.insert(stem);
		}
	}
	names
}

#[test]
fn registry_and_schema_dir_are_sets_equal_with_count_22() {
	let schemas = schema_names();
	let registry: BTreeSet<String> =
		dispatch::tool_names().iter().map(|name| name.to_string()).collect();

	assert_eq!(schemas.len(), 22, "frozen schema dir must hold 22 tools");
	assert_eq!(registry.len(), 22, "registry must hold 22 tools");

	// Registry ⊆ schemas: every registered tool has a frozen schema.
	let missing_schema: Vec<&String> = registry.difference(&schemas).collect();
	assert!(
		missing_schema.is_empty(),
		"registry tools without a frozen schema: {missing_schema:?}"
	);

	// Schemas ⊆ registry: every frozen schema is wired to a handler.
	let unwired: Vec<&String> = schemas.difference(&registry).collect();
	assert!(
		unwired.is_empty(),
		"frozen schemas with no registry entry: {unwired:?}"
	);

	// Exact set equality (implied by both directions; kept explicit so a
	// regression reports which direction broke first).
	assert_eq!(registry, schemas, "registry and schema dir drifted apart");
}

#[test]
fn every_registry_entry_has_a_compiled_validator_and_flags() {
	let schemas = dispatch::default_schemas();
	assert_eq!(schemas.len(), 22);
	for def in dispatch::registry() {
		let schema = schemas
			.get(def.name)
			.unwrap_or_else(|| panic!("no compiled schema for {}", def.name));
		// edit.undo is the only non-transactional tool (OPP/1 §6).
		if def.name == "edit.undo" {
			assert!(!def.transactional, "edit.undo must not open a txn");
		} else {
			assert!(def.transactional, "{} must be transactional", def.name);
		}
		// Schema title == file name == registry name (checked at load,
		// asserted here as the public contract).
		let title = schema.schema.get("title").and_then(|v| v.as_str());
		assert_eq!(title, Some(def.name), "schema title mismatch for {}", def.name);
	}
}

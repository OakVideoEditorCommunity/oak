//! Runtime loading and enforcement of the frozen T0 tool schemas
//! (`ai/schema/tools/*.json`, draft-07).
//!
//! The schema set is a **frozen contract** owned by T0; this crate only
//! reads it. Loading happens once per process (schemas compile to
//! validators eagerly); a schema that fails to compile is a programming
//! error and panics with the offending path, while an *instance* that
//! fails to validate returns a structured [`ToolError`] and never reaches
//! the backend.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jsonschema::Validator;
use serde_json::Value;

use crate::error::{ToolError, CODE_SCHEMA_VALIDATION};

/// `ai/schema/tools`, resolved from this crate's manifest directory (no
/// machine-local absolute paths are baked into the source).
pub fn default_schema_dir() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai/schema/tools")
}

/// One frozen tool schema plus its compiled validator.
pub struct ToolSchema {
	/// Tool name (= schema file stem = schema `title`).
	pub name: String,
	/// Source file (relative to the schema directory).
	pub source: PathBuf,
	/// The schema document as loaded.
	pub schema: Value,
	validator: Validator,
}

impl ToolSchema {
	/// Validates `instance` against this tool's schema.
	pub fn validate(&self, instance: &Value) -> Result<(), ToolError> {
		if let Err(err) = self.validator.validate(instance) {
			let mut messages: Vec<String> = Vec::new();
			for (index, error) in self.validator.iter_errors(instance).enumerate() {
				if index == 4 {
					messages.push("...".to_string());
					break;
				}
				let path = error.instance_path().to_string();
				let path = if path.is_empty() { "$".to_string() } else { path };
				messages.push(format!("{path}: {error}"));
			}
			return Err(ToolError::new(
				CODE_SCHEMA_VALIDATION,
				format!("arguments rejected by schema '{}': {}", self.name, messages.join("; ")),
			));
		}
		Ok(())
	}
}

/// The compiled registry of frozen tool schemas, keyed by tool name.
pub struct SchemaRegistry {
	tools: BTreeMap<String, ToolSchema>,
}

impl SchemaRegistry {
	/// Loads every `*.json` in the repository's frozen schema directory.
	pub fn load_default() -> Self {
		Self::load_dir(&default_schema_dir())
	}

	/// Loads every `*.json` in `dir`. Tool name = file stem; the schema's
	/// `title` must match it, otherwise the freeze is broken and we panic.
	pub fn load_dir(dir: &Path) -> Self {
		let mut tools = BTreeMap::new();
		let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
			.unwrap_or_else(|err| {
				panic!("cannot read the frozen tool schema directory {}: {err}", dir.display())
			})
			.filter_map(|entry| entry.ok().map(|e| e.path()))
			.filter(|path| path.extension().is_some_and(|ext| ext == "json"))
			.collect();
		entries.sort();
		if entries.is_empty() {
			panic!("no tool schemas found in {}", dir.display());
		}
		for path in entries {
			let source = path
				.file_name()
				.expect("schema file name")
				.to_string_lossy()
				.to_string();
			let name = source.trim_end_matches(".json").to_string();
			let text = std::fs::read_to_string(&path)
				.unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
			let schema: Value = serde_json::from_str(&text)
				.unwrap_or_else(|err| panic!("{} is not valid JSON: {err}", path.display()));
			let title = schema.get("title").and_then(Value::as_str).unwrap_or_default();
			if title != name {
				panic!(
					"schema title mismatch in {}: title '{title}' != file name '{name}'",
					path.display()
				);
			}
			let validator = jsonschema::options()
				.with_draft(jsonschema::Draft::Draft7)
				.offline()
				.build(&schema)
				.unwrap_or_else(|err| panic!("{} does not compile as draft-07: {err}", path.display()));
			tools.insert(
				name.clone(),
				ToolSchema {
					name,
					source: PathBuf::from(source),
					schema,
					validator,
				},
			);
		}
		Self { tools }
	}

	/// Every registered tool name, sorted.
	pub fn names(&self) -> Vec<&str> {
		self.tools.keys().map(String::as_str).collect()
	}

	/// Number of frozen tools.
	pub fn len(&self) -> usize {
		self.tools.len()
	}

	/// Whether no tool is registered (impossible for a loaded registry).
	pub fn is_empty(&self) -> bool {
		self.tools.is_empty()
	}

	/// Looks a tool up by name.
	pub fn get(&self, name: &str) -> Option<&ToolSchema> {
		self.tools.get(name)
	}

	/// Validates `args` against `tool`'s schema.
	pub fn validate(&self, tool: &str, args: &Value) -> Result<(), ToolError> {
		let schema = self.tools.get(tool).ok_or_else(|| {
			ToolError::new(
				crate::error::CODE_UNKNOWN_TOOL,
				format!("no frozen schema for tool '{tool}'"),
			)
		})?;
		schema.validate(args)
	}
}

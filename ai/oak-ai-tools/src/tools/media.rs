//! Arguments for `media.*`.

use serde_json::Value;

use crate::error::ToolError;
use crate::tools::common::req_str;

/// `media.probe`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeArgs {
	/// File to probe (no import side effect).
	pub path: String,
}

/// Parses `media.probe`.
pub fn parse_probe(args: &Value) -> Result<ProbeArgs, ToolError> {
	Ok(ProbeArgs {
		path: req_str(args, "path")?.to_string(),
	})
}

/// `media.import_footage`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportArgs {
	/// Files to import; failures are reported per path.
	pub paths: Vec<String>,
}

/// Parses `media.import_footage`.
pub fn parse_import(args: &Value) -> Result<ImportArgs, ToolError> {
	let paths = args
		.get("paths")
		.and_then(Value::as_array)
		.ok_or_else(|| ToolError::invalid_params("field 'paths' must be an array of strings"))?;
	let mut out = Vec::with_capacity(paths.len());
	for (index, path) in paths.iter().enumerate() {
		let text = path.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
			ToolError::invalid_params(format!("paths[{index}] must be a non-empty string"))
		})?;
		out.push(text.to_string());
	}
	if out.is_empty() {
		return Err(ToolError::invalid_params("field 'paths' must not be empty"));
	}
	Ok(ImportArgs { paths: out })
}

/// `media.list_footage` takes no arguments.
pub fn parse_list(args: &Value) -> Result<(), ToolError> {
	if let Some(key) = args.as_object().and_then(|map| map.keys().next()) {
		return Err(ToolError::invalid_params(format!(
			"media.list_footage takes no arguments (got '{key}')"
		)));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn import_parses_and_rejects_bad_entries() {
		let ok = parse_import(&json!({"paths": ["a.mp4", "b.mov"]})).unwrap();
		assert_eq!(ok.paths, vec!["a.mp4", "b.mov"]);
		assert!(parse_import(&json!({"paths": []})).is_err());
		assert!(parse_import(&json!({"paths": [42]})).is_err());
		assert!(parse_import(&json!({"paths": "a.mp4"})).is_err());
	}
}

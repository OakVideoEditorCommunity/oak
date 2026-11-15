//! Arguments for `project.*`.

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::{Rat, rate_arg};
use crate::tools::common::{opt_str, req_str};

/// `project.open`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenArgs {
	/// Existing project file; `None` creates a new empty project.
	pub path: Option<String>,
	/// Frame rate for a newly created project.
	pub fps: Option<Rat>,
	/// Display name for a newly created project.
	pub name: Option<String>,
}

/// Parses `project.open`.
pub fn parse_open(args: &Value) -> Result<OpenArgs, ToolError> {
	Ok(OpenArgs {
		path: opt_str(args, "path")?.map(str::to_string),
		fps: if args.get("fps").is_some_and(|v| !v.is_null()) {
			Some(rate_arg(args, "fps")?)
		} else {
			None
		},
		name: opt_str(args, "name")?.map(str::to_string),
	})
}

/// `project.save`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveArgs {
	/// Destination; `None` saves in place.
	pub path: Option<String>,
}

/// Parses `project.save`.
pub fn parse_save(args: &Value) -> Result<SaveArgs, ToolError> {
	Ok(SaveArgs {
		path: opt_str(args, "path")?.map(str::to_string),
	})
}

/// `project.get_overview` takes no arguments (the schema allows none).
pub fn parse_overview(args: &Value) -> Result<(), ToolError> {
	if let Some(key) = args.as_object().and_then(|map| map.keys().next()) {
		return Err(ToolError::invalid_params(format!(
			"project.get_overview takes no arguments (got '{key}')"
		)));
	}
	Ok(())
}

/// Convenience used by backends that must name a file.
pub fn file_stem(path: &str) -> String {
	std::path::Path::new(path)
		.file_stem()
		.map(|s| s.to_string_lossy().to_string())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "project".to_string())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn open_parses_all_fields() {
		let args = json!({"path": "/tmp/a.ove", "fps": {"num": 30000, "den": 1001}, "name": "cut"});
		let parsed = parse_open(&args).unwrap();
		assert_eq!(parsed.path.as_deref(), Some("/tmp/a.ove"));
		assert_eq!(parsed.fps, Some(Rat::new(30000, 1001).unwrap()));
		assert_eq!(parsed.name.as_deref(), Some("cut"));
	}

	#[test]
	fn open_accepts_bare_integer_fps_but_not_floats() {
		assert_eq!(
			parse_open(&json!({"fps": 30})).unwrap().fps,
			Some(Rat::new(30, 1).unwrap())
		);
		assert!(parse_open(&json!({"fps": 29.97})).is_err());
	}

	#[test]
	fn overview_rejects_arguments() {
		assert!(parse_overview(&json!({})).is_ok());
		assert!(parse_overview(&json!({"path": "x"})).is_err());
	}
}

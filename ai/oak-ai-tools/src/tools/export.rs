//! Arguments for `export.start` (and the no-argument `edit.undo`).

use serde_json::Value;

use crate::error::ToolError;
use crate::tools::common::req_str;

/// `export.start`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
	/// Sequence to export (required by OPP/1 section 8.8).
	pub sequence: String,
	/// Destination media file.
	pub output_path: String,
	/// Oak export preset name (open set, host-configured).
	pub preset: Option<String>,
}

/// Parses `export.start`.
pub fn parse_export(args: &Value) -> Result<ExportArgs, ToolError> {
	Ok(ExportArgs {
		sequence: req_str(args, "sequence")?.to_string(),
		output_path: req_str(args, "output_path")?.to_string(),
		preset: crate::tools::common::opt_str(args, "preset")?.map(str::to_string),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn export_parses() {
		let parsed =
			parse_export(&json!({"sequence": "seq-1", "output_path": "/tmp/out.mp4"})).unwrap();
		assert_eq!(parsed.sequence, "seq-1");
		assert_eq!(parsed.preset, None);
		let with_preset =
			parse_export(&json!({"sequence": "s", "output_path": "o.mp4", "preset": "h264-high"}))
				.unwrap();
		assert_eq!(with_preset.preset.as_deref(), Some("h264-high"));
		assert!(parse_export(&json!({"sequence": "s"})).is_err());
	}
}

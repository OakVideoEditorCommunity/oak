//! `edit.undo` takes no arguments (OPP/1 section 6; the legacy `count`
//! argument was dropped at the T0 freeze).

use serde_json::Value;

use crate::error::ToolError;

/// Parses `edit.undo`: any argument is rejected.
pub fn parse_undo(args: &Value) -> Result<(), ToolError> {
	if let Some(key) = args.as_object().and_then(|map| map.keys().next()) {
		return Err(ToolError::invalid_params(format!(
			"edit.undo takes no arguments (got '{key}')"
		)));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn undo_takes_no_arguments() {
		assert!(parse_undo(&json!({})).is_ok());
		assert!(parse_undo(&json!({"count": 1})).is_err());
	}
}

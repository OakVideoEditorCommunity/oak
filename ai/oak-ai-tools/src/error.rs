//! Structured errors for the T1 tool execution layer.
//!
//! Every failure that leaves the execution layer is a JSON object with at
//! least `{code, message, tool, step}`; capability gaps add a `gap` id and
//! an aborted transaction adds `txn`. This is the shape fed back to the
//! model for re-planning (`edit_failed`, plan section 4).

use serde_json::{Value, json};

/// The tool name is not in the frozen registry.
pub const CODE_UNKNOWN_TOOL: &str = "UNKNOWN_TOOL";
/// The arguments did not validate against the frozen JSON Schema.
pub const CODE_SCHEMA_VALIDATION: &str = "SCHEMA_VALIDATION_FAILED";
/// A time value was rejected by the parsing layer (bare float, string,
/// zero denominator, negative frame, ...).
pub const CODE_TIMECODE: &str = "TIMECODE_INVALID";
/// Arguments passed schema validation but are invalid for the backend
/// (overlap, out-of-range edge move, unknown entity, ...).
pub const CODE_INVALID_PARAMS: &str = "INVALID_PARAMS";
/// An entity id (footage / sequence / track / clip / node) does not exist.
pub const CODE_NOT_FOUND: &str = "NOT_FOUND";
/// The backend is not in the state the tool needs (no open project, ...).
pub const CODE_STATE: &str = "STATE_ERROR";
/// The backend failed while executing an otherwise valid call.
pub const CODE_BACKEND: &str = "BACKEND_FAILED";
/// The capability is not available through the current backend; `gap`
/// carries the gap identifier (see README "SDK capability gaps").
pub const CODE_UNSUPPORTED: &str = "UNSUPPORTED";
/// `edit.undo` on an empty undo stack.
pub const CODE_NOTHING_TO_UNDO: &str = "NOTHING_TO_UNDO";
/// begin/commit/abort plumbing failed.
pub const CODE_TRANSACTION: &str = "TRANSACTION_FAILED";
/// A file could not be read or written.
pub const CODE_IO: &str = "IO_ERROR";
/// An invariant inside this crate broke (should never reach a caller).
pub const CODE_INTERNAL: &str = "INTERNAL_ERROR";

/// A structured tool failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
	/// Stable machine-readable code.
	pub code: String,
	/// Human/model readable description.
	pub message: String,
	/// Tool the failure belongs to (filled in by the dispatcher).
	pub tool: Option<String>,
	/// Pipeline step that failed: `lookup` / `validate` / `begin` /
	/// `invoke` / `commit`.
	pub step: Option<String>,
	/// Capability gap id when `code == UNSUPPORTED`.
	pub gap: Option<String>,
	/// `"aborted"` when the surrounding transaction was rolled back.
	pub txn: Option<String>,
}

impl ToolError {
	/// An error with `code` and `message`.
	pub fn new(code: &str, message: impl Into<String>) -> Self {
		Self {
			code: code.to_string(),
			message: message.into(),
			tool: None,
			step: None,
			gap: None,
			txn: None,
		}
	}

	/// A capability gap: the backend does not implement this operation.
	///
	/// `gap` is the stable identifier listed in the README gap table; the
	/// execution layer never reports success for a capability it lacks.
	pub fn unsupported(gap: &str, message: impl Into<String>) -> Self {
		Self::new(CODE_UNSUPPORTED, message).with_gap(gap)
	}

	/// Convenience: invalid arguments.
	pub fn invalid_params(message: impl Into<String>) -> Self {
		Self::new(CODE_INVALID_PARAMS, message)
	}

	/// Convenience: unknown entity.
	pub fn not_found(message: impl Into<String>) -> Self {
		Self::new(CODE_NOT_FOUND, message)
	}

	/// Convenience: backend state problem.
	pub fn state(message: impl Into<String>) -> Self {
		Self::new(CODE_STATE, message)
	}

	/// Convenience: timecode rejection.
	pub fn timecode(message: impl Into<String>) -> Self {
		Self::new(CODE_TIMECODE, message)
	}

	/// Convenience: backend execution failure.
	pub fn backend(message: impl Into<String>) -> Self {
		Self::new(CODE_BACKEND, message)
	}

	/// Sets the gap id.
	pub fn with_gap(mut self, gap: &str) -> Self {
		self.gap = Some(gap.to_string());
		self
	}

	/// Fills `tool` when it is not set yet (first annotation wins, so a
	/// more specific inner annotation is preserved).
	pub fn with_tool(mut self, tool: &str) -> Self {
		if self.tool.is_none() {
			self.tool = Some(tool.to_string());
		}
		self
	}

	/// Fills `step` when it is not set yet.
	pub fn with_step(mut self, step: &str) -> Self {
		if self.step.is_none() {
			self.step = Some(step.to_string());
		}
		self
	}

	/// Marks the surrounding transaction as rolled back.
	pub fn aborted(mut self) -> Self {
		if self.txn.is_none() {
			self.txn = Some("aborted".to_string());
		}
		self
	}

	/// The wire form consumed by the re-planning loop.
	pub fn to_json(&self) -> Value {
		let mut out = json!({
			"code": self.code,
			"message": self.message,
		});
		if let Some(tool) = &self.tool {
			out["tool"] = json!(tool);
		}
		if let Some(step) = &self.step {
			out["step"] = json!(step);
		}
		if let Some(gap) = &self.gap {
			out["gap"] = json!(gap);
		}
		if let Some(txn) = &self.txn {
			out["txn"] = json!(txn);
		}
		out
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn json_carries_the_documented_fields() {
		let err = ToolError::unsupported("GAP_X", "not available")
			.with_tool("media.probe")
			.with_step("invoke")
			.aborted();
		let v = err.to_json();
		assert_eq!(v["code"], "UNSUPPORTED");
		assert_eq!(v["message"], "not available");
		assert_eq!(v["tool"], "media.probe");
		assert_eq!(v["step"], "invoke");
		assert_eq!(v["gap"], "GAP_X");
		assert_eq!(v["txn"], "aborted");
	}

	#[test]
	fn first_annotation_wins() {
		let err = ToolError::new(CODE_NOT_FOUND, "clip gone")
			.with_tool("timeline.split_clip")
			.with_step("invoke")
			.with_tool("other")
			.with_step("validate");
		assert_eq!(err.tool.as_deref(), Some("timeline.split_clip"));
		assert_eq!(err.step.as_deref(), Some("invoke"));
	}
}

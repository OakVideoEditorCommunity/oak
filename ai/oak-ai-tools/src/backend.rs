//! The backend contract of the tool execution layer (plan section 2).
//!
//! [`EngineBackend`] is the method set of the 22 frozen tools plus the
//! transactional primitives and the frame-grid lookup. Two
//! implementations ship with this crate:
//!
//! * [`crate::mock::MockBackend`] — an in-memory timeline model used by
//!   the unit tests and by the E2E chain on machines without the project
//!   FFmpeg;
//! * `crate::sdk::SdkBackend` — the real, in-process engine through
//!   `oak-engine-rust-sdk` (feature `sdk`).
//!
//! A backend that cannot perform an operation MUST return
//! [`ToolError::unsupported`] with a gap id — reporting success for a
//! capability it does not have is forbidden (plan section 5 iron rules).

use serde_json::Value;

use crate::error::ToolError;
use crate::timecode::{Rat, TimeContext, TimeValue};
use crate::txn::TxnOps;

/// The 22 frozen tools + transaction + time-grid primitives.
pub trait EngineBackend: TxnOps {
	/// Frame rate of the entity named by `ctx` (used to resolve frame
	/// indices onto the right grid).
	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError>;

	/// Resolves `value` against `ctx`'s frame grid (default helper for
	/// every backend).
	fn rational_at(&self, ctx: TimeContext, value: TimeValue) -> Result<Rat, ToolError> {
		let rate = self.time_rate(ctx)?;
		value.to_rational(rate)
	}

	// --- project -------------------------------------------------------
	/// `project.open`
	fn project_open(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `project.save`
	fn project_save(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `project.get_overview`
	fn project_get_overview(&mut self, args: &Value) -> Result<Value, ToolError>;

	// --- media ---------------------------------------------------------
	/// `media.probe`
	fn media_probe(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `media.import_footage`
	fn media_import_footage(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `media.list_footage`
	fn media_list_footage(&mut self, args: &Value) -> Result<Value, ToolError>;

	// --- timeline ------------------------------------------------------
	/// `timeline.add_track`
	fn timeline_add_track(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.place_clip`
	fn timeline_place_clip(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.split_clip`
	fn timeline_split_clip(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.trim_clip`
	fn timeline_trim_clip(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.move_clip`
	fn timeline_move_clip(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.ripple_delete`
	fn timeline_ripple_delete(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.add_transition`
	fn timeline_add_transition(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `timeline.add_marker`
	fn timeline_add_marker(&mut self, args: &Value) -> Result<Value, ToolError>;

	// --- nodes ---------------------------------------------------------
	/// `node.add_effect`
	fn node_add_effect(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `node.set_param`
	fn node_set_param(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `node.list_types`
	fn node_list_types(&mut self, args: &Value) -> Result<Value, ToolError>;

	// --- render --------------------------------------------------------
	/// `render.get_frame`
	fn render_get_frame(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `render.get_thumbnails`
	fn render_get_thumbnails(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `render.get_audio_levels`
	fn render_get_audio_levels(&mut self, args: &Value) -> Result<Value, ToolError>;

	// --- export / edit -------------------------------------------------
	/// `export.start`
	fn export_start(&mut self, args: &Value) -> Result<Value, ToolError>;
	/// `edit.undo` (non-transactional, OPP/1 section 6)
	fn edit_undo(&mut self, args: &Value) -> Result<Value, ToolError>;
}

/// The v2 extension face: tools added after the frozen 22 (T9.2).
///
/// This trait **extends** [`EngineBackend`] without changing it: the frozen
/// 22 method set, its mock, its SDK backend and every existing test stay
/// byte-identical. New tools are declared here and dispatched by
/// [`crate::dispatch::execute_v2_with`].
///
/// Every method has a default body returning
/// [`ToolError::unsupported`] with a gap id: a backend that cannot perform
/// the operation must say so structurally (plan section 5 iron rules), and
/// a backend that has not been taught the v2 face yet (the SDK backend)
/// keeps compiling.
pub trait EngineExt: EngineBackend {
	/// `sequence.get_settings` — reads a sequence's name / format / length.
	fn sequence_get_settings(&self, args: Value) -> Result<Value, ToolError> {
		let _ = args;
		Err(ToolError::unsupported(
			"GAP_SEQUENCE_SETTINGS",
			"this backend has no `sequence.get_settings` support",
		))
	}

	/// `sequence.set_format` — partial update of name / width / height /
	/// fps / interlaced, as ONE undo row.
	fn sequence_set_format(&mut self, args: Value) -> Result<Value, ToolError> {
		let _ = args;
		Err(ToolError::unsupported(
			"GAP_SEQUENCE_SET_FORMAT",
			"this backend has no `sequence.set_format` support",
		))
	}
}

//! `HostServices`: the host capability surface an OPP/1 plugin can reach.
//!
//! 首辅裁断 (T7.1): this crate must **not** depend on `oak-app` — the trait
//! is declared here against pure data types only and implemented app-side
//! (P1.2). Method names mirror the OPP method catalog
//! (`docs/zh/plans/completed/external-plugin-protocol.md` §8); every `txn`
//! argument is the transaction token from [`HostServices::edit_begin`].
//!
//! 裁断 (T7.1): `project.open` does **not** enter the undo stack — its `txn`
//! argument serializes and gates confirmation only (deviation recorded by
//! P1.2 in `docs/zh/plans/opp-implementation-notes.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Opaque entity id (protocol §5: plugins must never parse its format).
pub type EntityId = String;

/// Seconds as a rational (`{"num":3,"den":25}` — the protocol's only
/// time representation, §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rational {
	pub num: i64,
	pub den: i64,
}

impl Rational {
	pub fn new(num: i64, den: i64) -> Self {
		Self { num, den }
	}
}

// ---------------------------------------------------------------------------
// Read models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceInfo {
	pub id: EntityId,
	pub name: String,
	pub length: Rational,
	pub fps: Rational,
	pub width: u32,
	pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
	pub name: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub path: Option<String>,
	pub sequences: Vec<SequenceInfo>,
	pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaProbe {
	pub source: String,
	pub width: u32,
	pub height: u32,
	pub fps: Rational,
	pub duration: Rational,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub video_codec: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub audio_codec: Option<String>,
	pub has_video: bool,
	pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootageInfo {
	pub id: EntityId,
	pub name: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub path: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub duration: Option<Rational>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipInfo {
	pub id: EntityId,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub footage: Option<EntityId>,
	pub in_point: Rational,
	pub out_point: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
	pub id: EntityId,
	/// `"video"` | `"audio"`.
	pub kind: String,
	pub name: String,
	pub clips: Vec<ClipInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineStructure {
	pub sequence: EntityId,
	pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTypeInfo {
	pub type_id: String,
	pub label: String,
	pub category: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
	pub name: String,
	pub value: Value,
	/// Type hint for the inspector (`"float"`, `"color"`, …).
	pub kind: String,
}

// ---------------------------------------------------------------------------
// Request shapes (one struct per mutating method)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddTrackReq {
	pub kind: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceClipReq {
	pub footage: EntityId,
	pub track: EntityId,
	pub at: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitClipReq {
	pub clip: EntityId,
	pub at: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrimClipReq {
	pub clip: EntityId,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub in_point: Option<Rational>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub out_point: Option<Rational>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveClipReq {
	pub clip: EntityId,
	pub to_track: EntityId,
	pub to_at: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RippleDeleteReq {
	pub clip: EntityId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteClipReq {
	pub clip: EntityId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddTransitionReq {
	pub clip: EntityId,
	pub kind: String,
	pub duration: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddMarkerReq {
	pub at: Rational,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetWorkareaReq {
	pub in_point: Rational,
	pub out_point: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddEffectReq {
	pub target: EntityId,
	pub type_id: String,
	#[serde(default)]
	pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetParamReq {
	pub node: EntityId,
	pub param: String,
	pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetKeyframeReq {
	pub node: EntityId,
	pub param: String,
	pub at: Rational,
	pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameRequest {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub sequence: Option<EntityId>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub footage: Option<EntityId>,
	pub time: Rational,
	pub width: u32,
	pub height: u32,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub max_size: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThumbnailsRequest {
	pub clips: Vec<EntityId>,
	pub count: u32,
	pub width: u32,
	pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioLevelsRequest {
	pub clip: EntityId,
	pub buckets: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioLevelsResult {
	pub levels: Vec<f32>,
}

/// A frame bitmap reference in either wire form (§5): shm slot or
/// `inline_base64`. The shape is validated by the dispatcher, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameResult {
	pub frame: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportRequest {
	pub sequence: EntityId,
	pub preset: String,
	pub path: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub range: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportJob {
	pub job_id: EntityId,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Host-side failure mapped 1:1 onto a JSON-RPC error object (protocol §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostError {
	pub code: i32,
	pub message: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub data: Option<Value>,
}

impl HostError {
	pub fn new(code: i32, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
			data: None,
		}
	}
	pub fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
		Self {
			code,
			message: message.into(),
			data: Some(data),
		}
	}
	pub fn capability_denied(method: &str) -> Self {
		Self::new(
			crate::protocol::CAPABILITY_DENIED,
			format!("capability denied for {method}"),
		)
	}
	pub fn not_in_transaction() -> Self {
		Self::new(crate::protocol::NOT_IN_TRANSACTION, "not in edit transaction")
	}
	pub fn transaction_conflict() -> Self {
		Self::new(crate::protocol::TRANSACTION_CONFLICT, "transaction already held")
	}
	pub fn entity_not_found(id: &str) -> Self {
		Self::with_data(
			crate::protocol::ENTITY_NOT_FOUND,
			"entity not found",
			serde_json::json!({ "entity": id }),
		)
	}
	pub fn invalid_state(msg: impl Into<String>) -> Self {
		Self::new(crate::protocol::INVALID_STATE, msg)
	}
	pub fn internal(msg: impl Into<String>) -> Self {
		Self::new(crate::protocol::INTERNAL_ERROR, msg)
	}
}

impl fmt::Display for HostError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "[{}] {}", self.code, self.message)
	}
}

impl std::error::Error for HostError {}

impl From<HostError> for crate::protocol::ErrorObject {
	fn from(e: HostError) -> Self {
		crate::protocol::ErrorObject {
			code: e.code,
			message: e.message,
			data: e.data,
		}
	}
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Everything the host executes on a plugin's behalf. Implemented app-side
/// (P1.2); this crate only declares it.
///
/// `txn` is `Some` for confirmation-class methods (§7.3); passing `None`
/// where a transaction is required yields [`HostError::not_in_transaction`].
pub trait HostServices: Send + Sync {
	// ---- project.* ----
	fn project_get_info(&self) -> Result<ProjectInfo, HostError>;
	/// `txn` serializes + gates confirmation only; per 首辅裁断 the open
	/// itself does not enter the undo stack.
	fn project_open(&self, path: &str, txn: Option<&str>) -> Result<ProjectInfo, HostError>;
	fn project_save(&self, path: Option<&str>, txn: Option<&str>) -> Result<(), HostError>;

	// ---- media.* ----
	fn media_probe(&self, source: &str) -> Result<MediaProbe, HostError>;
	fn media_list_footage(&self) -> Result<Vec<FootageInfo>, HostError>;
	fn media_import_footage(&self, path: &str, txn: Option<&str>) -> Result<EntityId, HostError>;

	// ---- timeline.* ----
	fn timeline_get_structure(&self) -> Result<TimelineStructure, HostError>;
	fn timeline_add_track(&self, req: AddTrackReq, txn: Option<&str>)
	-> Result<EntityId, HostError>;
	fn timeline_place_clip(&self, req: PlaceClipReq, txn: Option<&str>)
	-> Result<EntityId, HostError>;
	fn timeline_split_clip(&self, req: SplitClipReq, txn: Option<&str>)
	-> Result<EntityId, HostError>;
	fn timeline_trim_clip(&self, req: TrimClipReq, txn: Option<&str>) -> Result<(), HostError>;
	fn timeline_move_clip(&self, req: MoveClipReq, txn: Option<&str>) -> Result<(), HostError>;
	fn timeline_ripple_delete(&self, req: RippleDeleteReq, txn: Option<&str>)
	-> Result<(), HostError>;
	fn timeline_delete_clip(&self, req: DeleteClipReq, txn: Option<&str>)
	-> Result<(), HostError>;
	fn timeline_add_transition(&self, req: AddTransitionReq, txn: Option<&str>)
	-> Result<EntityId, HostError>;
	fn timeline_add_marker(&self, req: AddMarkerReq, txn: Option<&str>)
	-> Result<EntityId, HostError>;
	fn timeline_set_workarea(&self, req: SetWorkareaReq, txn: Option<&str>)
	-> Result<(), HostError>;

	// ---- node.* ----
	fn node_list_types(&self) -> Result<Vec<NodeTypeInfo>, HostError>;
	fn node_get_params(&self, node: &EntityId) -> Result<Vec<ParamInfo>, HostError>;
	fn node_add_effect(&self, req: AddEffectReq, txn: Option<&str>) -> Result<EntityId, HostError>;
	fn node_set_param(&self, req: SetParamReq, txn: Option<&str>) -> Result<(), HostError>;
	fn node_set_keyframe(&self, req: SetKeyframeReq, txn: Option<&str>) -> Result<(), HostError>;
	fn node_remove(&self, node: &EntityId, txn: Option<&str>) -> Result<(), HostError>;

	// ---- render.* (never blocks the GUI thread — engine ticket path) ----
	fn render_get_frame(&self, req: FrameRequest) -> Result<FrameResult, HostError>;
	fn render_get_thumbnails(&self, req: ThumbnailsRequest)
	-> Result<Vec<FrameResult>, HostError>;
	fn render_get_audio_levels(&self, req: AudioLevelsRequest)
	-> Result<AudioLevelsResult, HostError>;

	// ---- export.* ----
	fn export_start(&self, req: ExportRequest, txn: Option<&str>) -> Result<ExportJob, HostError>;
	fn export_cancel(&self, job: &EntityId) -> Result<(), HostError>;

	// ---- edit.* (undo domain) ----
	/// Opens an edit transaction; returns its token. Global single-holder
	/// (OPP §6 rule 1): a second `begin` yields `TRANSACTION_CONFLICT`.
	fn edit_begin(&self, label: &str) -> Result<EntityId, HostError>;
	fn edit_commit(&self, txn: &str) -> Result<(), HostError>;
	fn edit_abort(&self, txn: &str) -> Result<(), HostError>;
	/// Undoes the whole stack including user operations (§6 rule 5).
	fn edit_undo(&self) -> Result<bool, HostError>;
	fn edit_redo(&self) -> Result<bool, HostError>;

	// ---- events.* (event-source subscription) ----
	/// Returns the granted subset (§7.2: events need read-class caps).
	fn events_subscribe(&self, events: &[String]) -> Result<Vec<String>, HostError>;
	fn events_unsubscribe(&self, events: &[String]) -> Result<(), HostError>;
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::protocol::ErrorObject;

	#[test]
	fn host_error_maps_to_the_wire_error_object() {
		let e = HostError::entity_not_found("n17");
		let wire: ErrorObject = e.clone().into();
		assert_eq!(wire.code, crate::protocol::ENTITY_NOT_FOUND);
		assert_eq!(wire.data, Some(serde_json::json!({"entity": "n17"})));
		assert_eq!(e.to_string(), format!("[{}] entity not found", crate::protocol::ENTITY_NOT_FOUND));
		assert_eq!(
			HostError::not_in_transaction().code,
			crate::protocol::NOT_IN_TRANSACTION
		);
		assert_eq!(
			HostError::transaction_conflict().code,
			crate::protocol::TRANSACTION_CONFLICT
		);
		assert_eq!(
			HostError::capability_denied("timeline.split_clip").code,
			crate::protocol::CAPABILITY_DENIED
		);
	}

	#[test]
	fn rational_is_the_only_time_representation() {
		let r = Rational::new(3, 25);
		let v = serde_json::to_value(&r).unwrap();
		assert_eq!(v, serde_json::json!({"num": 3, "den": 25}));
		let back: Rational = serde_json::from_value(v).unwrap();
		assert_eq!(back, r);
	}
}

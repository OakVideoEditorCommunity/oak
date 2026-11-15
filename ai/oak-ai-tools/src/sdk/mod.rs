//! The real engine backend: [`SdkBackend`], built on `oak-engine-rust-sdk`.
//!
//! This is the T1 "real machine" half of the crate. It implements the same
//! [`EngineBackend`](crate::backend::EngineBackend) / [`TxnOps`](crate::txn::TxnOps)
//! contract as [`crate::mock::MockBackend`], so [`crate::dispatch::execute`]
//! validates, wraps in one undo group and reports errors identically for both.
//!
//! # What is real here
//!
//! * one `oak_engine` project per backend, driven in-process through the SDK
//!   (no bridge process, no JSON-RPC);
//! * `txn_begin/commit/abort` map onto the engine's undo groups
//!   (`oak_undo_group_begin/end/abort`), so one tool call is one engine undo
//!   step and an aborted call rolls the document back;
//! * `render.get_frame` decodes the composed sequence (or the raw footage)
//!   through `oak_render_frame_f32` and ships real pixels;
//! * ids handed to the model are opaque tool-face ids minted here
//!   (`sequence-1`, `track-2`, `clip-3`, ...) and mapped onto the engine's
//!   `NodeId` tokens; the mapping never leaks into the wire format.
//!
//! # What is *not* real, and says so
//!
//! A few frozen-face tools have no counterpart in the SDK's C ABI. Those
//! return [`ToolError::unsupported`](crate::error::ToolError::unsupported)
//! with a stable gap id (see [`crate::sdk::gaps`] and the README's gap
//! table). Reporting success for a capability this backend does not have is
//! forbidden by the plan's iron rules, so every one of those is an explicit
//! `UNSUPPORTED` error, never a silently empty result.
//!
//! # Process-global state (documented engine property)
//!
//! The engine's undo stack is **process-wide**: `oak_undo_*` has no
//! per-project handle. Consequences, both handled here:
//!
//! * `project.open` clears the undo stack, so history never refers to nodes
//!   of a project this backend has replaced;
//! * two `SdkBackend`s must not interleave calls in one process (their undo
//!   groups and history would collide). [`SdkBackend`] therefore documents
//!   "one live backend per process"; the SDK E2E serialises its tests with a
//!   mutex for the same reason.

mod backend;
pub mod gaps;

pub use gaps::*;

use std::collections::{BTreeMap, HashMap};

use oak_engine_rust_sdk::node::{NodeId, NodeKind, Project};
use oak_engine_rust_sdk::types::Rational as SdkRational;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{ToolError, CODE_STATE, CODE_TRANSACTION};
use crate::timecode::{Rat, TimeContext, TimeValue};
use crate::txn::TxnOps;

/// Deterministic id counters for the tool-facing ids.
///
/// They live in the backend (not in the engine document) so the id sequence
/// is stable for the lifetime of the backend: a call that fails is rolled
/// back and mints nothing (ids are minted only after the engine call
/// succeeded), so a session's ids never repeat and a stale id never becomes
/// a live one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Counters {
	footage: u64,
	sequence: u64,
	track: u64,
	clip: u64,
	node: u64,
	marker: u64,
	transition: u64,
	gap: u64,
}

impl Counters {
	fn next(&mut self, kind: &str) -> u64 {
		let slot = match kind {
			"footage" => &mut self.footage,
			"sequence" => &mut self.sequence,
			"track" => &mut self.track,
			"clip" => &mut self.clip,
			"node" => &mut self.node,
			"marker" => &mut self.marker,
			"transition" => &mut self.transition,
			"gap" => &mut self.gap,
			other => unreachable!("unknown id kind {other}"),
		};
		*slot += 1;
		*slot
	}
}

/// An open undo group (engine side: one `oak_undo_group_*` scope).
struct Pending {
	label: String,
}

/// The real engine backend (feature `sdk`).
///
/// One instance owns one engine project and one live undo scope; see the
/// module docs for the process-global undo caveat.
pub struct SdkBackend {
	project: Option<Project>,
	/// Engine token to tool-face id.
	names: HashMap<NodeId, String>,
	/// Tool-face id to engine token.
	nodes: BTreeMap<String, NodeId>,
	counters: Counters,
	/// Clip id to footage id, remembered at `timeline.place_clip` time.
	///
	/// The SDK exposes no way to *read* a block's connected inputs (there is
	/// `oak_node_connect` but no edge query), so a clip's source cannot be
	/// recovered from the document. Recording it when the edit is made keeps
	/// `project.get_overview` honest for the current session; for a project
	/// loaded from disk the source is reported as `null` (see
	/// [`GAP_CLIP_SOURCE_QUERY`]).
	clip_source: BTreeMap<String, String>,
	pending: Option<Pending>,
}

impl SdkBackend {
	/// A backend with no project open.
	pub fn new() -> Self {
		Self {
			project: None,
			names: HashMap::new(),
			nodes: BTreeMap::new(),
			counters: Counters::default(),
			clip_source: BTreeMap::new(),
			pending: None,
		}
	}

	/// Whether an undo group is currently open.
	pub fn is_in_transaction(&self) -> bool {
		self.pending.is_some()
	}

	/// The label of the open undo group, when one is open.
	pub fn pending_label(&self) -> Option<&str> {
		self.pending.as_ref().map(|pending| pending.label.as_str())
	}

	/// Whether the backend has a project open.
	pub fn has_project(&self) -> bool {
		self.project.is_some()
	}

	/// The engine's history position (`oak_undo_index`: the number of done
	/// commands, including the empty "New/Open Project" bottom row).
	pub fn undo_index(&self) -> i64 {
		oak_engine_rust_sdk::node::undo_index()
	}

	/// A deterministic, serialisable description of the timeline half of the
	/// document (`footage` + `sequences`), in the same shape the mock's
	/// `serialize_timeline` uses.
	///
	/// This is the E2E's "state was restored" probe: it is derived from the
	/// *live* engine document on every call (nothing is cached), so comparing
	/// two calls really compares the engine's state.
	pub fn serialize_timeline(&mut self) -> Result<Value, ToolError> {
		if self.project.is_none() {
			return Ok(json!({ "footage": Value::Null, "sequences": Value::Null }));
		}
		let footage = self.footage_json_list()?;
		let sequences = self.sequences_json()?;
		Ok(json!({ "footage": footage, "sequences": sequences }))
	}

	// -- project / traversal -------------------------------------------

	fn project(&self) -> Result<&Project, ToolError> {
		self.project
			.as_ref()
			.ok_or_else(|| ToolError::new(CODE_STATE, "no project is open (call project.open first)"))
	}

	/// Every child of the bin root (footage and sequences live here).
	fn root_children(&self) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		let root = project.root();
		Ok((0..project.folder_child_count(root))
			.map(|index| project.folder_child_at(root, index))
			.filter(|node| node.is_valid())
			.collect())
	}

	/// Sequences, in bin order; the first one is the active sequence.
	fn sequence_nodes(&self) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		Ok(self
			.root_children()?
			.into_iter()
			.filter(|node| project.node_kind(*node) == Some(NodeKind::Sequence))
			.collect())
	}

	/// Footage nodes, in bin order.
	fn footage_nodes(&self) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		Ok(self
			.root_children()?
			.into_iter()
			.filter(|node| project.node_kind(*node) == Some(NodeKind::Footage))
			.collect())
	}

	/// The active sequence (the first one in the bin).
	fn active_sequence(&self) -> Result<NodeId, ToolError> {
		self.sequence_nodes()?.into_iter().next().ok_or_else(|| {
			ToolError::new(
				CODE_STATE,
				"the project has no sequence (call project.open, which creates the default \
				 sequence)",
			)
		})
	}

	/// Resolves an optional `sequence` argument (`None` = active).
	fn resolve_sequence(&mut self, id: Option<&str>) -> Result<NodeId, ToolError> {
		match id {
			None => self.active_sequence(),
			Some(id) => self.require_node(id, NodeKind::Sequence, "sequence"),
		}
	}

	/// Resolves a tool-face id to an engine node of `kind`, proving it is
	/// still part of the live document.
	fn require_node(&mut self, id: &str, kind: NodeKind, what: &str) -> Result<NodeId, ToolError> {
		let node = self
			.nodes
			.get(id)
			.copied()
			.ok_or_else(|| ToolError::not_found(format!("unknown {what} '{id}'")))?;
		if self.project()?.node_kind(node) != Some(kind) {
			return Err(ToolError::not_found(format!("unknown {what} '{id}'")));
		}
		// The engine token can outlive its node's removal (undo of a create);
		// only nodes still reachable from the document count.
		let live = match kind {
			NodeKind::Sequence => self.sequence_nodes()?.contains(&node),
			NodeKind::Footage => self.footage_nodes()?.contains(&node),
			NodeKind::Track => self.all_tracks()?.iter().any(|(track, _, _)| *track == node),
			NodeKind::Clip | NodeKind::Transition | NodeKind::Gap | NodeKind::Adjustment => {
				self.all_tracks()?.iter().any(|(track, _, _)| {
					self.blocks_of(*track)
						.map(|blocks| blocks.contains(&node))
						.unwrap_or(false)
				})
			}
			_ => true,
		};
		if !live {
			return Err(ToolError::not_found(format!(
				"unknown {what} '{id}' (it is no longer part of the document)"
			)));
		}
		Ok(node)
	}

	/// Track lists of a sequence (video list first).
	fn track_lists(&self, sequence: NodeId) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		Ok((0..project.sequence_track_list_count(sequence))
			.map(|index| project.sequence_track_list_at(sequence, index))
			.filter(|node| node.is_valid())
			.collect())
	}

	/// Tracks of a track list, in stack order.
	fn tracks_of(&self, list: NodeId) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		Ok((0..project.tracklist_track_count(list))
			.map(|index| project.tracklist_track_at(list, index))
			.filter(|node| node.is_valid())
			.collect())
	}

	/// Blocks of a track, in timeline order.
	fn blocks_of(&self, track: NodeId) -> Result<Vec<NodeId>, ToolError> {
		let project = self.project()?;
		Ok((0..project.track_block_count(track))
			.map(|index| project.track_block_at(track, index))
			.filter(|node| node.is_valid())
			.collect())
	}

	/// Every `(track, owning sequence, owning track list)` in document order.
	fn all_tracks(&self) -> Result<Vec<(NodeId, NodeId, NodeId)>, ToolError> {
		let mut out = Vec::new();
		for sequence in self.sequence_nodes()? {
			for list in self.track_lists(sequence)? {
				for track in self.tracks_of(list)? {
					out.push((track, sequence, list));
				}
			}
		}
		Ok(out)
	}

	/// Where a block lives: `(sequence, track, track list, track index)`.
	fn locate_block(&self, block: NodeId) -> Result<(NodeId, NodeId, NodeId, usize), ToolError> {
		for sequence in self.sequence_nodes()? {
			for list in self.track_lists(sequence)? {
				for (index, track) in self.tracks_of(list)?.into_iter().enumerate() {
					if self.blocks_of(track)?.contains(&block) {
						return Ok((sequence, track, list, index));
					}
				}
			}
		}
		Err(ToolError::not_found(
			"the block is not on any track of the document",
		))
	}

	/// The sequence owning a track.
	fn sequence_of_track(&self, track: NodeId) -> Result<NodeId, ToolError> {
		self.all_tracks()?
			.into_iter()
			.find(|(candidate, _, _)| *candidate == track)
			.map(|(_, sequence, _)| sequence)
			.ok_or_else(|| ToolError::not_found("the track is not part of the document"))
	}

	// -- ids ------------------------------------------------------------

	/// Mints the next tool-face id of `kind` and binds it to `node`.
	fn mint(&mut self, kind: &str, node: NodeId) -> String {
		let id = format!("{kind}-{}", self.counters.next(kind));
		self.bind(&id, node);
		id
	}

	/// Binds an existing tool-face id to an engine node.
	fn bind(&mut self, id: &str, node: NodeId) {
		self.names.insert(node, id.to_string());
		self.nodes.insert(id.to_string(), node);
	}

	/// The tool-face id of `node`, minting one on first sight (a project
	/// loaded from disk has nodes this backend has never named).
	fn name_of(&mut self, node: NodeId, kind: &str) -> String {
		if let Some(name) = self.names.get(&node) {
			return name.clone();
		}
		self.mint(kind, node)
	}

	/// Looks up a tool-face id without minting.
	fn node_of(&self, id: &str) -> Option<NodeId> {
		self.nodes.get(id).copied()
	}

	/// Remembers which footage a clip was placed from.
	fn remember_clip_source(&mut self, clip: &str, footage: &str) {
		self.clip_source.insert(clip.to_string(), footage.to_string());
	}

	/// The footage id a clip was placed from, when this session saw the
	/// placement (see [`GAP_CLIP_SOURCE_QUERY`]).
	pub fn clip_source(&self, clip: &str) -> Option<&str> {
		self.clip_source.get(clip).map(String::as_str)
	}

	// -- conversions -----------------------------------------------------

	/// Our rational to the SDK's ABI rational.
	pub(crate) fn sdk_rat(rate: Rat) -> SdkRational {
		SdkRational::new(rate.num, rate.den)
	}

	/// The SDK's ABI rational as our reduced rational; `what` names the
	/// value in the error message.
	pub(crate) fn rat(value: SdkRational, what: &str) -> Result<Rat, ToolError> {
		if value.den == 0 {
			return Err(ToolError::state(format!(
				"{what} has no rational value (the engine reported 0/0)"
			)));
		}
		Rat::new(value.num, value.den)
	}

	/// Resolves a tool-face time value against `ctx`'s frame grid, then
	/// converts to the ABI rational. Negative results are rejected (the tool
	/// face has no negative timeline positions).
	pub(crate) fn resolve_time(
		&self,
		ctx: TimeContext,
		value: TimeValue,
	) -> Result<(Rat, SdkRational), ToolError> {
		let resolved = crate::tools::common::resolve(self, ctx.clone(), value)?;
		if resolved.num < 0 {
			return Err(ToolError::invalid_params(format!(
				"{value} on {ctx} resolves to a negative time ({resolved})"
			)));
		}
		Ok((resolved, Self::sdk_rat(resolved)))
	}

	// -- document views --------------------------------------------------

	/// `footage` entries for `project.get_overview` / `media.list_footage`.
	fn footage_json_list(&mut self) -> Result<Vec<Value>, ToolError> {
		let nodes = self.footage_nodes()?;
		let mut out = Vec::with_capacity(nodes.len());
		for node in nodes {
			let id = self.name_of(node, "footage");
			out.push(self.footage_json(&id)?);
		}
		Ok(out)
	}

	/// One footage entry, read from the live document.
	pub(crate) fn footage_json(&mut self, id: &str) -> Result<Value, ToolError> {
		let node = self
			.node_of(id)
			.ok_or_else(|| ToolError::not_found(format!("unknown footage '{id}'")))?;
		let project = self.project()?;
		let path = project.footage_filename(node).unwrap_or_default();
		let params = project.footage_video_params(node, 0);
		// A still image sequence reports no total duration; fall back to the
		// video stream length and finally to zero rather than inventing one.
		let duration = {
			let total = project.footage_duration(node);
			let total = if total.den == 0 { project.footage_video_length(node) } else { total };
			Self::rat(total, "footage duration").unwrap_or(Rat { num: 0, den: 1 })
		};
		let mut out = json!({
			"id": id,
			"path": path,
			"fps": Value::Null,
			"duration": duration.to_json(),
			"width": Value::Null,
			"height": Value::Null,
			"video_streams": project.footage_video_stream_count(node) as u64,
			"audio_streams": project.footage_audio_stream_count(node) as u64,
			"synthetic": false,
		});
		if let Some(params) = params {
			let fps = Rat::new(params.frame_rate.num, params.frame_rate.den)
				.unwrap_or(Rat { num: 0, den: 1 });
			out["fps"] = fps.to_json();
			out["width"] = json!(params.width);
			out["height"] = json!(params.height);
		}
		Ok(out)
	}

	/// The `sequences` array of `project.get_overview`.
	fn sequences_json(&mut self) -> Result<Vec<Value>, ToolError> {
		let sequences = self.sequence_nodes()?;
		let mut out = Vec::with_capacity(sequences.len());
		for sequence in sequences {
			let sequence_id = self.name_of(sequence, "sequence");
			let mut tracks_json = Vec::new();
			for list in self.track_lists(sequence)? {
				for track in self.tracks_of(list)? {
					let track_id = self.name_of(track, "track");
					let kind = match self.project()?.track_kind(track) {
						0 => "video",
						1 => "audio",
						2 => "subtitle",
						_ => "unknown",
					};
					let mut clips_json = Vec::new();
					for block in self.blocks_of(track)? {
						let block_id = match self.project()?.node_kind(block) {
							Some(NodeKind::Transition) => self.name_of(block, "transition"),
							Some(NodeKind::Gap) => self.name_of(block, "gap"),
							_ => self.name_of(block, "clip"),
						};
						clips_json.push(self.block_json(&block_id, block)?);
					}
					tracks_json.push(json!({
						"id": track_id,
						"type": kind,
						"clips": clips_json,
					}));
				}
			}
			let name = self.project()?.node_label(sequence);
			out.push(json!({
				"id": sequence_id,
				"name": name,
				"tracks": tracks_json,
				// Markers have no SDK surface (see GAP_TIMELINE_MARKERS), so
				// the list is genuinely empty rather than unreported.
				"markers": Vec::<Value>::new(),
			}));
		}
		Ok(out)
	}

	/// One block entry of `project.get_overview`.
	fn block_json(&mut self, id: &str, block: NodeId) -> Result<Value, ToolError> {
		let project = self.project()?;
		let in_ = Self::rat(project.block_in(block), "block in")?;
		let out = Self::rat(project.block_out(block), "block out")?;
		let length = Self::rat(project.block_length(block), "block length")?;
		let media_in = Self::rat(project.block_media_in(block), "block media in")?;
		let kind = project.node_kind(block);
		let mut effects = Vec::new();
		if kind == Some(NodeKind::Clip) {
			for effect in project.effect_chain(block) {
				let effect_id = self.name_of(effect, "node");
				let type_id = self.project()?.node_type_id(effect);
				effects.push(json!({ "id": effect_id, "kind": type_id }));
			}
		}
		let mut transitions = Vec::new();
		if kind == Some(NodeKind::Transition) {
			transitions.push(json!({
				"id": id,
				// The block's range is the transition's span on the timeline.
				"duration": length.to_json(),
			}));
		}
		let footage = self.clip_source(id).map(str::to_string);
		Ok(json!({
			"id": id,
			"footage": footage,
			"in": in_.to_json(),
			"out": out.to_json(),
			"length": length.to_json(),
			"media_in": media_in.to_json(),
			"kind": match kind {
				Some(NodeKind::Clip) => "clip",
				Some(NodeKind::Transition) => "transition",
				Some(NodeKind::Gap) => "gap",
				Some(NodeKind::Adjustment) => "adjustment",
				_ => "unknown",
			},
			"effects": effects,
			"transitions": transitions,
		}))
	}

	/// The frame grid of the entity named by `ctx`, for `time_rate`.
	fn rate_of(&self, ctx: &TimeContext) -> Result<Rat, ToolError> {
		match ctx {
			TimeContext::Sequence(id) => {
				let sequence = if id.is_empty() {
					self.active_sequence()?
				} else {
					self.node_of(id)
						.ok_or_else(|| ToolError::not_found(format!("unknown sequence '{id}'")))?
				};
				Self::rat(self.project()?.sequence_frame_rate(sequence), "sequence frame rate")
			}
			TimeContext::Track(id) => {
				let track = self
					.node_of(id)
					.ok_or_else(|| ToolError::not_found(format!("unknown track '{id}'")))?;
				let sequence = self.sequence_of_track(track)?;
				Self::rat(self.project()?.sequence_frame_rate(sequence), "sequence frame rate")
			}
			TimeContext::Clip(id) => {
				let clip = self
					.node_of(id)
					.ok_or_else(|| ToolError::not_found(format!("unknown clip '{id}'")))?;
				let (sequence, _, _, _) = self.locate_block(clip)?;
				Self::rat(self.project()?.sequence_frame_rate(sequence), "sequence frame rate")
			}
			TimeContext::Footage(id) => {
				let footage = self
					.node_of(id)
					.ok_or_else(|| ToolError::not_found(format!("unknown footage '{id}'")))?;
				let params = self
					.project()?
					.footage_video_params(footage, 0)
					.ok_or_else(|| ToolError::state(format!("footage '{id}' has no video stream")))?;
				Rat::new(params.frame_rate.num, params.frame_rate.den)
			}
		}
	}
}

impl Default for SdkBackend {
	fn default() -> Self {
		Self::new()
	}
}

// ---------------------------------------------------------------------------
// Transaction primitives (one tool call = one engine undo group)
// ---------------------------------------------------------------------------

impl TxnOps for SdkBackend {
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
		if self.pending.is_some() {
			return Err(ToolError::new(
				CODE_TRANSACTION,
				"nested undo group: this backend allows one open group at a time",
			));
		}
		if !oak_engine_rust_sdk::node::undo_group_begin(label) {
			return Err(ToolError::new(
				CODE_TRANSACTION,
				"the engine refused to open an undo group (another group is already open in \
				 this process)",
			));
		}
		self.pending = Some(Pending {
			label: label.to_string(),
		});
		Ok(())
	}

	fn txn_commit(&mut self) -> Result<(), ToolError> {
		self.pending.take().ok_or_else(|| {
			ToolError::new(CODE_TRANSACTION, "txn_commit without an open undo group")
		})?;
		if !oak_engine_rust_sdk::node::undo_group_end() {
			return Err(ToolError::new(
				CODE_TRANSACTION,
				"the engine refused to close the undo group",
			));
		}
		Ok(())
	}

	fn txn_abort(&mut self) -> Result<(), ToolError> {
		self.pending.take().ok_or_else(|| {
			ToolError::new(CODE_TRANSACTION, "txn_abort without an open undo group")
		})?;
		if !oak_engine_rust_sdk::node::undo_group_abort() {
			return Err(ToolError::new(
				CODE_TRANSACTION,
				"the engine refused to abort the undo group",
			));
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::backend::EngineBackend;

	#[test]
	fn rail_errors_are_structured_without_a_project() {
		let mut backend = SdkBackend::new();
		assert!(!backend.has_project());
		let err = backend.project_get_overview(&json!({})).unwrap_err();
		assert_eq!(err.code, CODE_STATE);
		let err = backend
			.timeline_place_clip(&json!({"track": "track-1", "footage": "footage-1", "in": 0}))
			.unwrap_err();
		assert_eq!(err.code, CODE_STATE);
	}

	#[test]
	fn transaction_primitives_refuse_unbalanced_calls() {
		let mut backend = SdkBackend::new();
		assert!(backend.txn_commit().is_err());
		assert!(backend.txn_abort().is_err());
		backend.txn_begin("t").unwrap();
		assert!(backend.is_in_transaction());
		assert_eq!(backend.pending_label(), Some("t"));
		// A nested group is refused before the engine is touched.
		assert!(backend.txn_begin("nested").is_err());
		backend.txn_abort().unwrap();
		assert!(!backend.is_in_transaction());
	}

	#[test]
	fn rationals_convert_both_ways() {
		let rate = Rat::new(30000, 1001).unwrap();
		let sdk = SdkBackend::sdk_rat(rate);
		assert_eq!((sdk.num, sdk.den), (30000, 1001));
		assert_eq!(SdkBackend::rat(sdk, "rate").unwrap(), rate);
		let err = SdkBackend::rat(SdkRational::NULL, "missing").unwrap_err();
		assert_eq!(err.code, CODE_STATE);
	}

	#[test]
	fn counters_are_per_kind_and_monotonic() {
		let mut counters = Counters::default();
		assert_eq!(counters.next("clip"), 1);
		assert_eq!(counters.next("clip"), 2);
		assert_eq!(counters.next("track"), 1);
		assert_eq!(counters.next("footage"), 1);
	}
}

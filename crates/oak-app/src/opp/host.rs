//! The app-side `HostServices` implementation (T7.2 P1.2).
//!
//! `oak-extensions` declares the trait against pure data types only and
//! never depends on this crate (notes §2); this module is the other half of
//! that split: every method the OPP/1 catalog maps onto the engine is routed
//! to the real `oakui::graphops` / `renderops` / `effectchain` / undo
//! entry points, and everything the spec marks 🆕 (needs-new-code) answers a
//! structured `Unsupported` instead of pretending to succeed.
//!
//! Mapping source: `~/.oak-reviews/opp-spec.md` §1 (per-method `path:line`);
//! deviations: `docs/zh/plans/opp-implementation-notes.md`.
//!
//! Threading: an [`OppHost`] owns a [`ProjectRef`] clone (`Arc<Mutex<Project>>`,
//! a thread-safe domain handle) so plugin requests arriving on the IO thread
//! run pure engine code without a gpui `Context`. Methods that *would* need
//! the UI context (`node.add_effect` / `node.set_param`, which route through
//! `Engine::…(cx)`) and methods whose host-side plumbing notes §3 defers
//! (`export.*`, whose `ExportSession` carries a non-`Send` event receiver)
//! are reported `Unsupported` with that exact reason — never faked.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use oak_extensions::host_services::{
	AddEffectAtReq, AddEffectReq, AddMarkerReq, AddTrackReq, AddTransitionPlacedReq,
	AddTransitionReq, AudioLevelsRequest, EffectTypeInfo, TransitionPlaced, TransitionSide,
	AudioLevelsResult, ClipInfo, DeleteClipReq, EntityId, ExportJob, ExportRequest, FootageInfo,
	FrameRequest, FrameResult, HostError, HostServices, MediaProbe, MoveClipReq, NodeTypeInfo,
	ParamInfo, PlaceClipReq, ProjectInfo, Rational, SequenceInfo, SequenceSettings,
	SetKeyframeReq, SetParamReq, SetSequenceFormatReq, SetWorkareaReq,
	RippleDeleteReq, SplitClipReq, ThumbnailsRequest, TimelineStructure, TrackInfo, TrimClipReq,
};
use oak_extensions::protocol::{INVALID_PARAMS, METHOD_NOT_FOUND};
use serde_json::{json, Value};

use crate::oakui::effectchain;
use crate::oakui::graphops::{self, ProjectRef};
use crate::oakui::renderops::{self, RenderedFrame};
use quercus_engine_rs::node::NodeId;
use quercus_engine_rs::editor::TrackKind;

/// The six-domain host: the current project plus the single-holder
/// transaction state (OPP §6 rule 1: one open transaction per host).
#[derive(Default)]
pub struct OppHost {
	/// The live project (`None` before any project is opened/attached).
	/// The app calls [`OppHost::attach_project`] whenever it opens, creates
	/// or reloads a project so the plugin view never points at a replaced
	/// tree; `project.open` itself calls this after `load_ove` succeeds.
	project: RwLock<Option<ProjectRef>>,
	/// The open transaction (global single-holder, protocol §6 rule 1).
	txn: Mutex<TxnState>,
}

#[derive(Default)]
struct TxnState {
	/// The opaque token handed out by `edit.begin`.
	holder: Option<String>,
	/// Monotonic token source (unique per host instance).
	next: u64,
}

/// Tools whose engine entry point groups its commands itself, so the OPP
/// transaction must not open a second group (the undo stack cannot nest).
///
/// Only `node.add_effect` qualifies: `effect_chain_insert` wraps its own
/// commands in `grouped("Add Effect")`. `timeline.add_transition` looks
/// similar but does **not** group internally (measured 2026-11-13: dropping
/// its outer group leaves zero undo rows), so it keeps the OPP group.
const ENGINE_GROUPED_TOOLS: [&str; 1] = ["node.add_effect"];

/// Prefix marking a transaction token that opened no undo group of its own
/// (see [`ENGINE_GROUPED_TOOLS`]); `edit.commit`/`edit.abort` leave the
/// engine's group alone for these.
const JOINED_TXN_PREFIX: &str = "opp-txn-joined-";

/// `NodeId` has no `Display`; the wire id is the stable identity integer
/// (`graphops::id_of` is its inverse).
fn id_string(id: NodeId) -> String {
	identity(id).to_string()
}

/// The stable identity of a node id (pre-P7 `NodeId::identity`; the SDK
/// id's identity IS its raw value — `graphops::id_of` round-trips it).
fn identity(id: NodeId) -> u64 {
	id.raw()
}

fn parse_id(s: &str) -> Result<NodeId, HostError> {
	s.parse::<u64>()
		.ok()
		.and_then(graphops::id_of)
		.ok_or_else(|| HostError::entity_not_found(s))
}

fn parse_track_type(kind: &str) -> Result<TrackKind, HostError> {
	match kind {
		"video" => Ok(TrackKind::Video),
		"audio" => Ok(TrackKind::Audio),
		"subtitle" => Ok(TrackKind::Subtitle),
		other => Err(HostError::with_data(
			INVALID_PARAMS,
			format!("unknown track kind {other:?}"),
			json!({ "field": "kind", "allowed": ["video", "audio", "subtitle"] }),
		)),
	}
}

fn track_type_str(kind: TrackKind) -> &'static str {
	match kind {
		TrackKind::Video => "video",
		TrackKind::Audio => "audio",
		TrackKind::Subtitle => "subtitle",
	}
}

impl OppHost {
	pub fn new() -> Self {
		Self::default()
	}

	/// Point the host at a project the app just opened, created or reloaded.
	pub fn attach_project(&self, project: ProjectRef) {
		*self.project.write().unwrap_or_else(|e| e.into_inner()) = Some(project);
	}

	/// The current project, or `INVALID_STATE` when none is open.
	fn project(&self) -> Result<ProjectRef, HostError> {
		self.project
			.read()
			.unwrap_or_else(|e| e.into_inner())
			.clone()
			.ok_or_else(|| HostError::invalid_state("no project is open"))
	}

	/// The open transaction's token, or the protocol error for a missing /
	/// foreign token (protocol §7.2: `NOT_IN_TRANSACTION` / `TRANSACTION_CONFLICT`).
	/// Turns `effectchain::insert`'s refusal into an actionable error: the
	/// model does not know the type catalogue, so a bare "unknown type" would
	/// make it guess again next round. Every branch carries the addable list,
	/// already filtered by the clip's media kind.
	fn effect_error(
		&self,
		p: &ProjectRef,
		host: NodeId,
		type_id: &str,
		reason: &str,
	) -> HostError {
		let available = {
				let mut ids: Vec<String> = effectchain::addable_effects_for(&p, host)
				.into_iter()
				.map(|e| e.type_id)
				.collect();
			let total = ids.len();
			ids.truncate(24);
			if total > ids.len() {
				ids.push(format!("… ({total} total)"));
			}
			ids.join(", ")
		};
		let lower = reason.to_lowercase();
		let message = if lower.contains("unknown node type") {
			format!(
				"unknown effect type \"{type_id}\". Types this clip accepts: {available}. \
				 Pass one of those as `effect`."
			)
		} else if lower.contains("cannot host") || lower.contains("no effect input") {
			format!(
				"this node cannot host effects (it has no effect input); add the effect to a \
				 clip instead. Types a clip accepts: {available}"
			)
		} else if lower.contains("media") || lower.contains("kind") {
			format!(
				"this effect does not match the clip's media kind (a video effect cannot \
				 chain onto an audio clip, and vice versa). Types this clip accepts: {available}"
			)
		} else {
			format!("the engine refused the effect: {reason}. Types this clip accepts: {available}")
		};
		HostError::new(INVALID_PARAMS, message)
	}

	fn require_txn(&self, txn: Option<&str>) -> Result<(), HostError> {
		let t = self.txn.lock().unwrap_or_else(|e| e.into_inner());
		match (txn, t.holder.as_deref()) {
			(None, _) => Err(HostError::not_in_transaction()),
			(Some(got), Some(have)) if got == have => Ok(()),
			(Some(_), Some(_)) => Err(HostError::transaction_conflict()),
			(Some(_), None) => Err(HostError::not_in_transaction()),
		}
	}

	/// [`OppHost::require_txn`] for the `edit.*` methods, whose token is
	/// mandatory on the wire.
	fn require_txn_str(&self, txn: &str) -> Result<(), HostError> {
		self.require_txn(Some(txn))
	}

	/// Drop the held token after a successful `commit`/`abort`.
	fn release_txn(&self, txn: &str) {
		let mut t = self.txn.lock().unwrap_or_else(|e| e.into_inner());
		if t.holder.as_deref() == Some(txn) {
			t.holder = None;
		}
	}

	/// The sequence id (wire string) the request names, defaulting to the
	/// project's first sequence when the caller omits it (`null` = current).
	fn sequence_of(&self, p: &ProjectRef, seq: &Option<EntityId>) -> Result<NodeId, HostError> {
		let ids = graphops::sequence_ids(&p);
		match seq {
			Some(want) => ids
				.iter()
				.find(|id| id_string(**id) == *want)
				.copied()
				.ok_or_else(|| HostError::entity_not_found(want)),
			None => ids
				.first()
				.copied()
				.ok_or_else(|| HostError::invalid_state("the project has no sequence")),
		}
	}

	/// The project's first sequence id (render paths that take no explicit
	/// sequence).
	fn first_sequence(&self, p: &ProjectRef) -> Result<NodeId, HostError> {
		self.sequence_of(p, &None)
	}

	/// The wire form of a sequence's settings.
	fn settings_of(&self, p: &ProjectRef, seq: NodeId) -> Result<SequenceSettings, HostError> {
		let (name, width, height, rate, interlaced) =
			graphops::sequence_settings(&p, seq)
				.ok_or_else(|| HostError::invalid_state("the sequence has no video stream"))?;
		let duration = graphops::sequence_length(&p, seq);
		Ok(SequenceSettings {
			id: id_string(seq),
			name,
			width: width.max(0) as u32,
			height: height.max(0) as u32,
			rate: host_rational(&rate),
			interlaced,
			duration: host_rational(&duration),
		})
	}

	/// `(num, den)` frame timebase of `seq` (`graphops::sequence_time_base`).
	fn timebase(&self, p: &ProjectRef, seq: NodeId) -> Result<(i64, i64), HostError> {
		graphops::sequence_time_base(&p, seq).ok_or_else(|| {
			HostError::invalid_state("the sequence has no valid frame rate")
		})
	}
}

// ---------------------------------------------------------------------------
// Conversion helpers (host `Rational` ⇄ engine timebase)
// ---------------------------------------------------------------------------

/// Host rational → engine rational (rejects a zero denominator up front:
/// the SDK `Rational` treats `den = 0` as unusable).
fn core_rational(r: &Rational) -> Result<quercus_engine_rs::types::Rational, HostError> {
	if r.den == 0 {
		return Err(HostError::with_data(
			INVALID_PARAMS,
			"time has a zero denominator",
			json!({ "time": r }),
		));
	}
	Ok(quercus_engine_rs::types::Rational::new(r.num, r.den))
}

fn host_rational(r: &quercus_engine_rs::types::Rational) -> Rational {
	Rational::new(r.num, r.den)
}

/// A seconds-rational → frame timestamp on the sequence's timebase
/// (`graphops::rational_to_ts`).
fn to_ts(r: &Rational, tb: (i64, i64)) -> Result<i64, HostError> {
	Ok(graphops::rational_to_ts(core_rational(r)?, tb))
}

/// Seconds as a wire rational (exact to 1 µs, no invented frame rate).
fn secs_rational(secs: f64) -> Rational {
	Rational::new((secs * 1_000_000.0).round() as i64, 1_000_000)
}

/// Structured `Unsupported` (protocol: an unserved method answers
/// `METHOD_NOT_FOUND`; `data` carries why, so a caller can tell "not wired
/// yet" apart from "no such method").
fn unsupported(method: &str, reason: &str, spec: &str) -> HostError {
	HostError::with_data(
		METHOD_NOT_FOUND,
		format!("{method} is not wired to the engine in P1.2: {reason}"),
		json!({
			"unsupported": true,
			"method": method,
			"reason": reason,
			"spec": spec,
		}),
	)
}

// ---------------------------------------------------------------------------
// project.* / media.*
// ---------------------------------------------------------------------------

impl HostServices for OppHost {
	fn project_get_info(&self) -> Result<ProjectInfo, HostError> {
		let p = self.project()?;
		let name = graphops::project_name(&p);
		let filename = p.filename();
		let path = if filename.is_empty() {
			None
		} else {
			Some(filename)
		};
		let sequences = graphops::sequence_ids(&p)
			.into_iter()
			.map(|id| {
				let (width, height, fps) = graphops::sequence_video_params(&p, id)
					.map(|(w, h, rate)| (w as u32, h as u32, host_rational(&rate)))
					.unwrap_or((0, 0, Rational::new(30, 1)));
				SequenceInfo {
					id: id_string(id),
					name: graphops::node_label(&p, id),
					length: host_rational(&graphops::sequence_length(&p, id)),
					fps,
					width,
					height,
				}
			})
			.collect();
		Ok(ProjectInfo {
			name,
			path,
			sequences,
			dirty: p.modified(),
		})
	}

	/// Open a project file. Per notes §1 this **does not enter the undo
	/// stack**: the token serializes the call and carries the confirmation
	/// gate, and the tree swap leaves the SDK undo stack untouched — no
	/// `group_begin`/`group_end` is opened for a swap.
	fn project_open(&self, path: &str, txn: Option<&str>) -> Result<ProjectInfo, HostError> {
		self.require_txn(txn)?;
		let loaded = graphops::load_ove(Path::new(path))
			.map_err(|e| HostError::invalid_state(format!("open failed: {e}")))?;
		self.attach_project(loaded);
		self.project_get_info()
	}

	fn project_save(&self, path: Option<&str>, txn: Option<&str>) -> Result<(), HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let target = match path {
			Some(s) if !s.is_empty() => Path::new(s).to_path_buf(),
			_ => {
						let filename = p.filename();
				if filename.is_empty() {
					return Err(HostError::invalid_state("the project has no path to save to"));
				}
				PathBuf::from(filename)
			}
		};
		graphops::save_ove(&p, &target)
			.map_err(|e| HostError::internal(format!("save failed: {e}")))
	}

	fn media_probe(&self, _source: &str) -> Result<MediaProbe, HostError> {
		Err(unsupported(
			"media.probe",
			"the app crate has no standalone probe entry point; spec §1.3 lands it by \
			 calling `oak_codec` directly, which P1.2 does not do",
			"opp-spec.md §1.3",
		))
	}

	fn media_list_footage(&self) -> Result<Vec<FootageInfo>, HostError> {
		let p = self.project()?;
		Ok(graphops::footage_ids(&p)
			.into_iter()
			.map(|id| FootageInfo {
				id: id_string(id),
				name: graphops::node_label(&p, id),
				path: None,
				duration: graphops::footage_duration_seconds(&p, id)
					.map(secs_rational),
			})
			.collect())
	}

	fn media_import_footage(&self, path: &str, txn: Option<&str>) -> Result<EntityId, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let id = graphops::import_footage(&p, Path::new(path))
			.map_err(|e| HostError::internal(format!("import failed: {e}")))?;
		Ok(id_string(id))
	}

// ---------------------------------------------------------------------------
// timeline.*
// ---------------------------------------------------------------------------

	fn timeline_get_structure(&self) -> Result<TimelineStructure, HostError> {
		let p = self.project()?;
		let seq = self.first_sequence(&p)?;
		let mut tracks = Vec::new();
		for kind in [TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle] {
			let kind_str = track_type_str(kind);
			for tid in graphops::track_ids(&p, seq, kind) {
				let clips = graphops::clip_ids(&p, tid)
					.into_iter()
					.map(|cid| {
						let (in_p, out_p, _) = graphops::clip_range(&p, cid)
							.unwrap_or((
							quercus_engine_rs::types::Rational::new(0, 1),
							quercus_engine_rs::types::Rational::new(0, 1),
							quercus_engine_rs::types::Rational::new(0, 1),
						));
						ClipInfo {
							id: id_string(cid),
							footage: graphops::find_input_footage(&p, cid).map(id_string),
							in_point: host_rational(&in_p),
							out_point: host_rational(&out_p),
						}
					})
					.collect();
				tracks.push(TrackInfo {
					id: id_string(tid),
					kind: kind_str.to_string(),
					name: graphops::node_label(&p, tid),
					clips,
				});
			}
		}
		Ok(TimelineStructure {
			sequence: id_string(seq),
			tracks,
		})
	}

	fn timeline_add_track(
		&self,
		req: AddTrackReq,
		txn: Option<&str>,
	) -> Result<EntityId, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let seq = self.first_sequence(&p)?;
		let kind = parse_track_type(&req.kind)?;
		let index = graphops::add_track(&p, seq, kind)
			.map_err(|e| HostError::internal(format!("add track failed: {e}")))?;
		let ids = graphops::track_ids(&p, seq, kind);
		// `add_track` returns the fresh track's position; the id is read back
		// from the list so the caller gets a real entity, not a guess.
		ids.get(index)
			.copied()
			.or_else(|| ids.last().copied())
			.map(id_string)
			.ok_or_else(|| HostError::internal("add track produced no track"))
	}

	fn timeline_place_clip(
		&self,
		req: PlaceClipReq,
		txn: Option<&str>,
	) -> Result<EntityId, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let seq = self.first_sequence(&p)?;
		let footage = parse_id(&req.footage)?;
		let track = parse_id(&req.track)?;
		let (kind, index, tb, in_ts, out_ts) = {
				// The wire carries a track id; the engine wants (kind, index).
			let mut found = None;
			for kind in [TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle] {
				let ids = graphops::track_ids(&p, seq, kind);
				if let Some(pos) = ids.iter().position(|&t| t == track) {
					found = Some((kind, pos));
					break;
				}
			}
			let (kind, index) = found.ok_or_else(|| HostError::entity_not_found(&req.track))?;
			let tb = graphops::sequence_time_base(&p, seq)
				.ok_or_else(|| HostError::invalid_state("sequence has no frame rate"))?;
			let in_ts = to_ts(&req.at, tb)?;
			// The wire carries no duration: the clip ends where the footage
			// ends (media in = 0). When the engine cannot probe a duration
			// there is no honest default — guessing a length would fabricate
			// a cut, so the call fails with a structured reason instead.
			let dur = graphops::footage_duration_seconds(&p, footage).ok_or_else(|| {
				HostError::with_data(
					INVALID_PARAMS,
					"footage duration is not probeable; refusing to guess a clip length",
					json!({
						"footage": req.footage,
						"reason": "duration unknown",
						"hint": "probe or re-import the footage, then place the clip",
					}),
				)
			})?;
			let out_ts = in_ts + graphops::rational_to_ts(core_rational(&secs_rational(dur))?, tb);
			if out_ts <= in_ts {
				return Err(HostError::with_data(
					INVALID_PARAMS,
					"footage has no usable duration",
					json!({ "footage": req.footage }),
				));
			}
			(kind, index, tb, in_ts, out_ts)
		};
		let _ = tb;
		graphops::place_footage_clip(&p, seq, footage, kind, index, in_ts, out_ts, 0)
			.map(id_string)
			.map_err(|e| HostError::internal(format!("place clip failed: {e}")))
	}

	fn timeline_split_clip(
		&self,
		req: SplitClipReq,
		txn: Option<&str>,
	) -> Result<EntityId, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let clip = parse_id(&req.clip)?;
		let seq = self.sequence_of(&p, &None)?;
		let tb = self.timebase(&p, seq)?;
		let at = to_ts(&req.at, tb)?;
		graphops::split_clip(&p, clip, at)
			.map_err(|e| HostError::internal(format!("split failed: {e}")))?;
		// The engine keeps the original id on the left half; the right half
		// is a new node the caller can discover through `get_structure`.
		Ok(req.clip)
	}

	fn timeline_trim_clip(&self, req: TrimClipReq, txn: Option<&str>) -> Result<(), HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let clip = parse_id(&req.clip)?;
		let seq = self.first_sequence(&p)?;
		let tb = self.timebase(&p, seq)?;
		let (old_in, old_out) = {
				graphops::clip_range(&p, clip)
				.map(|(i, o, _)| (i, o))
				.ok_or_else(|| HostError::entity_not_found(&req.clip))?
		};
		// Only the side(s) the caller supplied move; the other keeps its
		// current position (spec §1.4: partial trims are legal).
		let new_in = match &req.in_point {
			Some(r) => to_ts(r, tb)?,
			None => graphops::rational_to_ts(old_in, tb),
		};
		let new_out = match &req.out_point {
			Some(r) => to_ts(r, tb)?,
			None => graphops::rational_to_ts(old_out, tb),
		};
		graphops::trim_clip(&p, clip, new_in, new_out)
			.map_err(|e| HostError::internal(format!("trim failed: {e}")))
	}

	fn timeline_move_clip(&self, req: MoveClipReq, txn: Option<&str>) -> Result<(), HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let clip = parse_id(&req.clip)?;
		let dest = parse_id(&req.to_track)?;
		let seq = self.first_sequence(&p)?;
		let tb = self.timebase(&p, seq)?;
		let at = to_ts(&req.to_at, tb)?;
		let same_track = {
				graphops::clip_track(&p, clip) == Some(dest)
		};
		let result = if same_track {
			graphops::move_clip(&p, clip, at)
		} else {
			graphops::move_clip_to_track(&p, clip, dest, at)
		};
		result.map_err(|e| HostError::internal(format!("move failed: {e}")))
	}

	fn timeline_ripple_delete(
		&self,
		req: RippleDeleteReq,
		txn: Option<&str>,
	) -> Result<(), HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let clip = parse_id(&req.clip)?;
		graphops::ripple_delete_clip(&p, clip)
			.map_err(|e| HostError::internal(format!("ripple delete failed: {e}")))
	}

	fn timeline_delete_clip(
		&self,
		_req: DeleteClipReq,
		_txn: Option<&str>,
	) -> Result<(), HostError> {
		Err(unsupported(
			"timeline.delete_clip",
			"only the ripple form is mapped (`ripple_delete_clip`); the spec marks the \
			 non-ripple variant 🔧 with its function not yet confirmed, so P1.2 does \
			 not guess one",
			"opp-spec.md §1.4",
		))
	}

	fn timeline_add_transition(
		&self,
		_req: AddTransitionReq,
		_txn: Option<&str>,
	) -> Result<EntityId, HostError> {
		Err(unsupported(
			"timeline.add_transition",
			"the wire carries one clip + side + duration, but `add_transition_at_seam` \
			 needs the opposite block; the seam-inference rule (GAP_TRANSITION_SEAM) is \
			 undefined, so P1.2 refuses rather than inventing it",
			"opp-spec.md §1.4",
		))
	}

	fn timeline_add_marker(
		&self,
		_req: AddMarkerReq,
		_txn: Option<&str>,
	) -> Result<EntityId, HostError> {
		Err(unsupported(
			"timeline.add_marker",
			"`marker_list_create` only mints a handle; where markers persist (sidecar vs \
			 graph node) is still an open deviation item (notes §6), so P1.2 does not \
			 write a handle that would not round-trip",
			"opp-spec.md §1.4 + notes §6",
		))
	}

	fn timeline_set_workarea(
		&self,
		_req: SetWorkareaReq,
		_txn: Option<&str>,
	) -> Result<(), HostError> {
		Err(unsupported(
			"timeline.set_workarea",
			"same open item as markers: `workarea_create` mints a handle but its \
			 persistence (sidecar vs graph node) is undecided (notes §6)",
			"opp-spec.md §1.4 + notes §6",
		))
	}

// ---------------------------------------------------------------------------
// node.*
// ---------------------------------------------------------------------------

	fn node_list_types(&self) -> Result<Vec<NodeTypeInfo>, HostError> {
		// The SDK's built-in factory table plus the runtime-registered
		// (OpenFX) table — the pre-P7 `oak_node` `Factory` split into these
		// two reads.
		let mut out: Vec<NodeTypeInfo> = quercus_engine_rs::effect::factory_entries()
			.into_iter()
			.map(|m| NodeTypeInfo {
				type_id: m.type_id,
				label: m.name,
				category: m
					.category
					.map(|c| format!("{c:?}"))
					.unwrap_or_default(),
			})
			.collect();
		out.extend(quercus_engine_rs::effect::dynamic_factory_entries().into_iter().map(|m| {
			NodeTypeInfo {
				type_id: m.type_id,
				label: m.name,
				category: m.sub_category,
			}
		}));
		Ok(out)
	}

	fn node_get_params(&self, node: &EntityId) -> Result<Vec<ParamInfo>, HostError> {
		let p = self.project()?;
		let id = parse_id(node)?;
		let params = effectchain::effect_params(&p, id)
			.ok_or_else(|| HostError::entity_not_found(node))?;
		Ok(params
			.into_iter()
			.map(|ep| ParamInfo {
				name: ep.display_name,
				// `NodeValue` has no `Serialize` impl; the wire gets its Debug
				// form rather than the whole call failing.
				value: Value::String(format!("{:?}", ep.value)),
				kind: format!("{:?}", ep.value_type).to_lowercase(),
			})
			.collect())
	}

	fn node_add_effect(&self, _req: AddEffectReq, _txn: Option<&str>) -> Result<EntityId, HostError> {
		Err(unsupported(
			"node.add_effect",
			"the engine entry point is `Engine::add_effect(index, type_id, cx)`, which \
			 needs a gpui `Context` on the UI thread; the host executes on the IO \
			 thread (notes §3), so P1.2 cannot route it without a UI-thread hop",
			"opp-spec.md §1.5",
		))
	}

	fn node_set_param(&self, _req: SetParamReq, _txn: Option<&str>) -> Result<(), HostError> {
		Err(unsupported(
			"node.set_param",
			"same UI-thread constraint as `node.add_effect`: the engine setter takes a \
			 gpui `Context`",
			"opp-spec.md §1.5",
		))
	}

	fn node_set_keyframe(
		&self,
		_req: SetKeyframeReq,
		_txn: Option<&str>,
	) -> Result<(), HostError> {
		Err(unsupported(
			"node.set_keyframe",
			"`set_value_at_time_command` needs an engine `NodeValue` plus the input's \
			 element index; the wire's free-form JSON value and element choice are not \
			 defined yet, so P1.2 refuses instead of coercing a value wrongly",
			"opp-spec.md §1.5",
		))
	}

	fn node_remove(&self, _node: &EntityId, _txn: Option<&str>) -> Result<(), HostError> {
		Err(unsupported(
			"node.remove",
			"`effectchain::remove(project, host, effect)` needs the chain's host node, \
			 but the wire passes only the effect id and the protocol defines no \
			 parent-lookup, so P1.2 cannot address the call safely",
			"opp-spec.md §1.5",
		))
	}

// ---------------------------------------------------------------------------
// render.*
// ---------------------------------------------------------------------------

	fn render_get_frame(&self, req: FrameRequest) -> Result<FrameResult, HostError> {
		if req.footage.is_some() {
			return Err(unsupported(
				"render.get_frame",
				"the footage branch decodes a clip directly through `oak_codec`, which \
				 spec §1.3 places outside this crate; only the sequence branch is wired",
				"opp-spec.md §1.6",
			));
		}
		let p = self.project()?;
		let seq = self.sequence_of(&p, &req.sequence)?;
		let tb = self.timebase(&p, seq)?;
		let ts = to_ts(&req.time, tb)?;
		let frame = renderops::render_sequence_frame(
			&p,
			seq,
			ts,
			tb,
			req.width as i32,
			req.height as i32,
			None,
		)
		.map_err(|e| HostError::internal(format!("render failed: {e}")))?;
		// P7 merge: upstream's pure-SDK `RenderedFrame` has a single `Sdk`
		// variant; the pre-P7 `CpuF32 { linesize, data }` payload is gone
		// (the SDK frame owns its bytes). `to_display` is the one surviving
		// read path: F32 frames come back as tightly packed display
		// samples, BGRA8 frames as a display-transformed image.
		let (width, height, png) = match frame.to_display() {
			Some((_image, _scope, Some(samples))) => {
				let w = frame.width();
				let h = frame.height();
				let png = encode_png_rgba_f32_samples(w, h, &samples)?;
				(w, h, png)
			}
			Some((image, _scope, None)) => {
				let w = frame.width();
				let h = frame.height();
				let bytes = image
					.as_bytes(0)
					.ok_or_else(|| HostError::internal("display image has no bytes"))?;
				let png = encode_png_bgra8(w, h, &bytes)?;
				(w, h, png)
			}
			None => {
				return Err(unsupported(
					"render.get_frame",
					"the frame could not be converted for display (invalid geometry \
					 or a conversion failure)",
					"opp-spec.md §1.6",
				));
			}
		};
		Ok(FrameResult {
			frame: json!({
				"inline_base64": png,
				"width": width,
				"height": height,
				"format": "png",
			}),
		})
	}

	fn render_get_thumbnails(&self, req: ThumbnailsRequest) -> Result<Vec<FrameResult>, HostError> {
		if req.clips.is_empty() || req.count == 0 {
			return Ok(Vec::new());
		}
		// The engine renders whole sequence frames (spec §1.6: "循环
		// render_sequence_frame, count ≤ 64"); each thumbnail samples the
		// sequence at the owning clip's in-point.
		let count = req.count.min(64) as usize;
		let p = self.project()?;
		let seq = self.first_sequence(&p)?;
		let tb = self.timebase(&p, seq)?;
		let mut out = Vec::new();
		for clip in req.clips.iter().take(count) {
			let cid = parse_id(clip)?;
			let (in_p, _, _) = {
						graphops::clip_range(&p, cid)
					.ok_or_else(|| HostError::entity_not_found(clip))?
			};
			let ts = graphops::rational_to_ts(in_p, tb);
			let frame = renderops::render_sequence_frame(
				&p,
				seq,
				ts,
				tb,
				req.width as i32,
				req.height as i32,
				None,
			)
			.map_err(|e| HostError::internal(format!("thumbnail render failed: {e}")))?;
			// Same single-variant read path as `render_get_frame` (P7).
			match frame.to_display() {
				Some((_image, _scope, Some(samples))) => out.push(FrameResult {
					frame: json!({
						"inline_base64": encode_png_rgba_f32_samples(
							frame.width(),
							frame.height(),
							&samples
						)?,
						"width": frame.width(),
						"height": frame.height(),
						"format": "png",
					}),
				}),
				Some((image, _scope, None)) => out.push(FrameResult {
					frame: json!({
						"inline_base64": encode_png_bgra8(
							frame.width(),
							frame.height(),
							&image.as_bytes(0).ok_or_else(|| {
								HostError::internal("display image has no bytes")
							})?
						)?,
						"width": frame.width(),
						"height": frame.height(),
						"format": "png",
					}),
				}),
				None => {
					return Err(unsupported(
						"render.get_thumbnails",
						"the frame could not be converted for display",
						"opp-spec.md §1.6",
					));
				}
			}
		}
		Ok(out)
	}

	fn render_get_audio_levels(
		&self,
		req: AudioLevelsRequest,
	) -> Result<AudioLevelsResult, HostError> {
		let p = self.project()?;
		let seq = self.first_sequence(&p)?;
		let tb = self.timebase(&p, seq)?;
		let clip = parse_id(&req.clip)?;
		let (in_p, out_p) = {
				graphops::clip_range(&p, clip)
				.map(|(i, o, _)| (i, o))
				.ok_or_else(|| HostError::entity_not_found(&req.clip))?
		};
		let start = graphops::rational_to_ts(in_p, tb);
		let end = graphops::rational_to_ts(out_p, tb);
		let audio = renderops::render_audio_range(&p, seq, start, (end - start).max(1), tb)
			.map_err(|e| HostError::internal(format!("audio render failed: {e}")))?;
		let buckets = (req.buckets as usize).clamp(1, 4096);
		let mut levels = Vec::with_capacity(buckets);
		if audio.data.is_empty() {
			levels.resize(buckets, 0.0f32);
		} else {
			let per = (audio.data.len() / buckets).max(1);
			for i in 0..buckets {
				let slice = &audio.data[i * per..((i + 1) * per).min(audio.data.len())];
				let peak = slice.iter().fold(0.0f32, |m, s| m.max(s.abs()));
				levels.push(peak);
			}
		}
		Ok(AudioLevelsResult { levels })
	}

// ---------------------------------------------------------------------------
// export.*
// ---------------------------------------------------------------------------

	fn export_start(&self, _req: ExportRequest, _txn: Option<&str>) -> Result<ExportJob, HostError> {
		Err(unsupported(
			"export.start",
			"`spawn_export` returns an `ExportSession` holding a non-`Send` mpsc event \
			 receiver and `EncodingParams` has no Default; routing it needs the notes §3 \
			 event pump, which P1.2 does not build — a session could not be stored on a \
			 `Send + Sync` host at all",
			"opp-spec.md §1.6",
		))
	}

	fn export_cancel(&self, _job: &EntityId) -> Result<(), HostError> {
		Err(unsupported(
			"export.cancel",
			"the cancel handle lives inside `ExportSession`, which `export.start` cannot \
			 hand out yet (see that method)",
			"opp-spec.md §1.6",
		))
	}

// ---------------------------------------------------------------------------
// edit.* (undo domain)
// ---------------------------------------------------------------------------

	fn edit_begin(&self, label: &str) -> Result<EntityId, HostError> {
		let mut t = self.txn.lock().unwrap_or_else(|e| e.into_inner());
		if t.holder.is_some() {
			return Err(HostError::transaction_conflict());
		}
		// Tools whose engine entry point already wraps its own commands in one
		// undo group — `effect_chain_insert` → `grouped("Add Effect")`, and the
		// transition router's counterpart. The undo stack cannot nest groups:
		// the engine's `group_begin` returns `Error::State` while one is open,
		// which surfaces to the model as "the engine refused…". For these tools
		// the grouping belongs to the engine, so we take the transaction token
		// (keeping `require_txn` and the conflict checks intact) without
		// opening a group of our own; the engine's group is the single row.
		let engine_grouped = ENGINE_GROUPED_TOOLS.contains(&label);
		if !engine_grouped && !graphops::undo_group_begin(label) {
			return Err(HostError::internal(
				"group_begin failed: a group is already open (stack busy)",
			));
		}
		t.next += 1;
		let token = if engine_grouped {
			format!("{}{}", JOINED_TXN_PREFIX, t.next)
		} else {
			format!("opp-txn-{}", t.next)
		};
		t.holder = Some(token.clone());
		Ok(token)
	}

	fn edit_commit(&self, txn: &str) -> Result<(), HostError> {
		self.require_txn_str(txn)?;
		// An empty group (only `project.open`/`save` ran) is discarded by the
		// stack itself (`push_pre_executed` drops an empty multi command), so
		// no blank undo row is left behind (notes §1).
		// A joined transaction opened no group: the engine closed its own.
		if !txn.starts_with(JOINED_TXN_PREFIX) && !graphops::undo_group_end() {
			return Err(HostError::internal("group_end failed: no group is open"));
		}
		self.release_txn(txn);
		Ok(())
	}

	fn edit_abort(&self, txn: &str) -> Result<(), HostError> {
		self.require_txn_str(txn)?;
		if !txn.starts_with(JOINED_TXN_PREFIX) && !graphops::undo_group_abort() {
			return Err(HostError::internal("group_abort failed: no group is open"));
		}
		self.release_txn(txn);
		Ok(())
	}

	fn edit_undo(&self) -> Result<bool, HostError> {
		if !graphops::undo_undo() {
			return Err(HostError::internal("undo failed: the stack is empty"));
		}
		Ok(graphops::undo_can_redo())
	}

	fn edit_redo(&self) -> Result<bool, HostError> {
		if !graphops::undo_redo() {
			return Err(HostError::internal("redo failed: nothing to redo"));
		}
		Ok(graphops::undo_can_undo())
	}

// ---------------------------------------------------------------------------
// events.* (routed by the dispatch layer, not here)
// ---------------------------------------------------------------------------

	fn events_subscribe(&self, _events: &[String]) -> Result<Vec<String>, HostError> {
		Err(unsupported(
			"events.subscribe",
			"subscription bookkeeping lives in the P1 host session; `opp::dispatch` \
			 routes it before this trait is reached, so a call that lands here means \
			 the dispatch layer did not run",
			"opp-spec.md §1.1",
		))
	}

	fn events_unsubscribe(&self, _events: &[String]) -> Result<(), HostError> {
		Err(unsupported(
			"events.unsubscribe",
			"same as `events.subscribe`: handled by the P1 host session at the \
			 dispatch layer",
			"opp-spec.md §1.1",
		))
	}

	// ---- sequence.* (v2 face) -----------------------------------------

	fn sequence_get_settings(
		&self,
		sequence: Option<&EntityId>,
	) -> Result<SequenceSettings, HostError> {
		let p = self.project()?;
		let seq = self.sequence_of(&p, &sequence.cloned())?;
		self.settings_of(&p, seq)
	}

	// ---- effect/transition edits (T10A; added after the frozen 22) ------

	fn node_addable_effects(&self, clip: &EntityId) -> Result<Vec<EffectTypeInfo>, HostError> {
		let p = self.project()?;
		let host = parse_id(clip)?;
		if p.node_kind(host).is_none() {
			return Err(HostError::entity_not_found(clip));
		}
		// Filtered by the host's own media kind: the list a caller gets is
		// exactly the set `insert` would accept.
		Ok(effectchain::addable_effects_for(&p, host)
			.into_iter()
			.map(|e| EffectTypeInfo {
				type_id: e.type_id,
				name: e.name,
			})
			.collect())
	}

	fn node_add_effect_at(&self, req: AddEffectAtReq, txn: Option<&str>) -> Result<EntityId, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let host = parse_id(&req.target)?;
		// `effectchain::insert` owns the whole edit: it validates the host,
		// the type and the media kind, and wraps its commands in one
		// `grouped("Add Effect")` row. The caller must NOT push anything,
		// must not pre-apply, and must not wrap it — that is the T9.2
		// double-row trap in reverse.
		match effectchain::insert_at_effect_index(&p, host, req.index, &req.type_id) {
			Ok(id) => Ok(identity(id).to_string()),
			Err(reason) => Err(self.effect_error(&p, host, &req.type_id, &reason)),
		}
	}

	fn timeline_add_transition_placed(
		&self,
		req: AddTransitionPlacedReq,
		txn: Option<&str>,
	) -> Result<TransitionPlaced, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let clip = parse_id(&req.clip)?;
		let seq = self.first_sequence(&p)?;
		let tb = self.timebase(&p, seq)?;
		let total = to_ts(&req.duration, tb)?;
		if total <= 0 {
			return Err(HostError::new(
				INVALID_PARAMS,
				"duration must be positive; give the transition's total length, e.g. {\"num\":1,\"den\":1} for one second",
			));
		}
		let kind_index = graphops::transition_kind_index(&req.kind).ok_or_else(|| {
			HostError::new(
				INVALID_PARAMS,
				format!(
					"unknown transition type '{}' (expected crossdissolve|fade|wipe|slide)",
					req.kind
				),
			)
		})?;

		// The router: `in` is always the clip's own head; `out` becomes a seam
		// transition only when the next clip on the same track starts exactly
		// where this one ends (no guessing — the test is strict equality).
		let seam_next = match req.side {
			TransitionSide::In => None,
			TransitionSide::Out => {
						graphops::next_contiguous_clip(&p, clip)
			}
		};

		match (req.side, seam_next) {
			(TransitionSide::Out, Some(next)) => {
				let half_frames = total / 2;
				if half_frames <= 0 {
					return Err(HostError::new(
						INVALID_PARAMS,
						"a seam transition needs at least two frames of total duration \
						 (the duration is split over both sides)",
					));
				}
				let half = graphops::ts_to_rational(half_frames, tb);
				let id = graphops::add_transition_at_seam_typed(
					&p,
					clip,
					next,
					half,
					Some(kind_index),
				)
				.map_err(|e| HostError::new(INVALID_PARAMS, format!("transition: {e}")))?;
				Ok(TransitionPlaced {
					id: identity(id).to_string(),
					placement: "seam".to_string(),
					// An odd frame count cannot split; the value reported is
					// the one the engine actually used, never the requested one.
					half: Some(host_rational(&half)),
				})
			}
			(side, _) => {
				let start_edge = matches!(side, TransitionSide::In);
				let length = graphops::ts_to_rational(total, tb);
				let id = graphops::add_transition_at_edge_typed(
					&p,
					clip,
					start_edge,
					length,
					Some(kind_index),
				)
				.map_err(|e| HostError::new(INVALID_PARAMS, format!("transition: {e}")))?;
				Ok(TransitionPlaced {
					id: identity(id).to_string(),
					placement: "edge".to_string(),
					half: None,
				})
			}
		}
	}

	fn sequence_set_format(
		&self,
		req: SetSequenceFormatReq,
		txn: Option<&str>,
	) -> Result<SequenceSettings, HostError> {
		self.require_txn(txn)?;
		let p = self.project()?;
		let seq = self.sequence_of(&p, &req.sequence)?;
		// Partial update: read the current tuple, overwrite only the fields
		// the caller supplied, then write the whole tuple back as ONE undo
		// row (changing the rate reinterprets every block's time base).
		let (name, width, height, rate, interlaced) = {
				graphops::sequence_settings(&p, seq)
				.ok_or_else(|| HostError::invalid_state("the sequence has no video stream"))?
		};
		let rate = match req.rate {
			Some(r) => core_rational(&r)?,
			None => rate,
		};
		let width = req.width.map(|w| w as i32).unwrap_or(width);
		let height = req.height.map(|h| h as i32).unwrap_or(height);
		if width <= 0 || height <= 0 {
			return Err(HostError::with_data(
				INVALID_PARAMS,
				"width and height must be positive",
				json!({ "width": width, "height": height }),
			));
		}
		if rate.num <= 0 || rate.den <= 0 {
			return Err(HostError::with_data(
				INVALID_PARAMS,
				"fps must be a positive rational",
				json!({ "rate": host_rational(&rate) }),
			));
		}
		let after = (
			req.name.unwrap_or(name),
			width,
			height,
			rate,
			req.interlaced.unwrap_or(interlaced),
		);
		graphops::set_sequence_parameters_undoable(&p, seq, after)
			.map_err(|e| HostError::internal(format!("set sequence format failed: {e}")))?;
		self.settings_of(&p, seq)
	}
}

// ---------------------------------------------------------------------------
// PNG (F32 RGBA) → base64 — the wire's `inline_base64` form (§5)
// ---------------------------------------------------------------------------

/// Encode an F32 RGBA frame as a base64 PNG. The stride (`linesize`) may
/// exceed `width * 16`; only the live bytes of each row are copied.
/// Tightly packed display samples (F32 RGBA) → base64 PNG.
///
/// P7: `RenderedFrame::to_display` hands back repacked, display-transformed
/// samples (no `linesize` anymore), so the old row-stride reader is gone.
fn encode_png_rgba_f32_samples(width: i32, height: i32, samples: &[f32]) -> Result<String, HostError> {
	use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
	if width <= 0 || height <= 0 {
		return Err(HostError::internal(format!(
			"bad frame size {width}x{height}"
		)));
	}
	let (w, h) = (width as u32, height as u32);
	let buf = ImageBuffer::<Rgba<f32>, _>::from_raw(w, h, samples.to_vec())
		.ok_or_else(|| HostError::internal("frame buffer shape mismatch"))?;
	let mut png = Vec::new();
	DynamicImage::ImageRgba32F(buf)
		.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
		.map_err(|e| HostError::internal(format!("png encode failed: {e}")))?;
	Ok(b64_encode(&png))
}

/// Display-ready BGRA8 bytes → base64 PNG (the `to_display` image path).
fn encode_png_bgra8(width: i32, height: i32, bgra: &[u8]) -> Result<String, HostError> {
	use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
	if width <= 0 || height <= 0 {
		return Err(HostError::internal(format!(
			"bad frame size {width}x{height}"
		)));
	}
	let (w, h) = (width as u32, height as u32);
	if bgra.len() < (w as usize) * (h as usize) * 4 {
		return Err(HostError::internal("frame buffer shape mismatch"));
	}
	let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
	for px in bgra.chunks_exact(4) {
		rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
	}
	let buf = ImageBuffer::<Rgba<u8>, _>::from_raw(w, h, rgba)
		.ok_or_else(|| HostError::internal("frame buffer shape mismatch"))?;
	let mut png = Vec::new();
	DynamicImage::ImageRgba8(buf)
		.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
		.map_err(|e| HostError::internal(format!("png encode failed: {e}")))?;
	Ok(b64_encode(&png))
}

fn b64_encode(data: &[u8]) -> String {
	const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
	for chunk in data.chunks(3) {
		let b0 = chunk[0];
		let b1 = *chunk.get(1).unwrap_or(&0);
		let b2 = *chunk.get(2).unwrap_or(&0);
		let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
		out.push(T[(n >> 18) as usize & 63] as char);
		out.push(T[(n >> 12) as usize & 63] as char);
		out.push(if chunk.len() > 1 {
			T[(n >> 6) as usize & 63] as char
		} else {
			'='
		});
		out.push(if chunk.len() > 2 {
			T[n as usize & 63] as char
		} else {
			'='
		});
	}
	out
}

// ---------------------------------------------------------------------------
// Tests (run with `cargo test -p oak-app opp::`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use oak_extensions::host_services::{AddTrackReq, AddTransitionReq};

	/// Every test takes the crate-wide engine test lock (the same one
	/// `graphops` / `effectchain` / `displaycolor` tests use) and zeroes the
	/// global undo stack first: the SDK stack is process-global, so without
	/// this the four tests would race each other on `group_begin` and on
	/// `count()`.
	fn setup() -> (OppHost, NodeId) {
		let _g = graphops::test_lock();
		graphops::undo_clear();
		let project = graphops::create_project();
		let seq = graphops::create_sequence(&project, "Seq");
		let host = OppHost::new();
		host.attach_project(project);
		(host, seq)
	}

	/// ① `timeline.add_track` really reaches the engine: the track shows up
	/// in `get_structure` and the command is pushed onto the global undo
	/// stack (graphops pushes `TimelineAddTrackCommand`).
	#[test]
	fn timeline_add_track_reaches_the_engine() {
		let _g = graphops::test_lock();
		graphops::undo_clear();
		let project = graphops::create_project();
		let seq = graphops::create_sequence(&project, "Seq");
		let host = OppHost::new();
		host.attach_project(project);

		let before = host.timeline_get_structure().unwrap().tracks.len();
		let before_count = graphops::undo_count();

		let token = host.edit_begin("opp: add track").unwrap();
		let id = host
			.timeline_add_track(AddTrackReq { kind: "video".into(), name: None }, Some(&token))
			.unwrap();
		assert!(!id.is_empty(), "add_track returned an empty entity id");
		// The row lands on the stack only at `edit_commit`: inside an open
		// group `count()` still shows the committed rows.
		host.edit_commit(&token).unwrap();

		let after = host.timeline_get_structure().unwrap();
		assert_eq!(after.tracks.len(), before + 1, "the new track is not visible");
		assert!(
			after.tracks.iter().any(|t| t.id == id),
			"the returned id is not one of the structure's tracks"
		);
		assert_eq!(after.sequence, id_string(seq), "structure reports another sequence");
		assert!(
			graphops::undo_count() > before_count,
			"add_track did not push an undo row"
		);
	}

	/// ② An unmapped method answers a structured `Unsupported`: protocol
	/// `METHOD_NOT_FOUND` plus `data.unsupported = true` and a reason —
	/// never a fake success.
	#[test]
	fn unmapped_method_answers_structured_unsupported() {
		let _g = graphops::test_lock();
		graphops::undo_clear();
		let project = graphops::create_project();
		let _seq = graphops::create_sequence(&project, "Seq");
		let host = OppHost::new();
		host.attach_project(project);

		let e = host.media_probe("/nonexistent/clip.mp4").unwrap_err();
		assert_eq!(e.code, METHOD_NOT_FOUND, "wrong error code for an unmapped method");
		let data = e.data.expect("Unsupported must carry structured data");
		assert_eq!(data["unsupported"], true, "data.unsupported must be true");
		assert_eq!(data["method"], "media.probe");
		let reason = data["reason"].as_str().expect("reason must be a string");
		assert!(!reason.is_empty(), "reason must not be empty");
		assert!(data["spec"].as_str().unwrap().starts_with("opp-spec"));

		// A second unmapped path (timeline seam) behaves the same way.
		let e2 = host
			.timeline_add_transition(
				AddTransitionReq {
					clip: "1".into(),
					kind: "cross".into(),
					duration: Rational::new(1, 2),
				},
				None,
			)
			.unwrap_err();
		assert_eq!(e2.code, METHOD_NOT_FOUND);
		assert_eq!(e2.data.unwrap()["unsupported"], true);
	}

	/// ③ `project.open` never enters the undo stack (notes §1): the token
	/// only serializes the call, the tree swap pushes nothing, and the
	/// committing empty group leaves no blank row either.
	#[test]
	fn project_open_never_enters_the_undo_stack() {
		let _g = graphops::test_lock();
		graphops::undo_clear();
		let dir = std::env::temp_dir().join(format!("opp-open-{}", std::process::id()));
		std::fs::create_dir_all(&dir).expect("temp dir");
		let file = dir.join("p.ove");
		let project = graphops::create_project();
		let _seq = graphops::create_sequence(&project, "Seq");
		let host = OppHost::new();
		host.attach_project(project);

		// One real edit, so the stack is non-empty and a stray push shows up.
		let t = host.edit_begin("setup").unwrap();
		host.timeline_add_track(AddTrackReq { kind: "video".into(), name: None }, Some(&t))
			.unwrap();
		host.edit_commit(&t).unwrap();
		let before = graphops::undo_count();
		assert!(before >= 1, "setup edit did not land on the stack");

		// Save + open both need a transaction token (confirmation class);
		// neither may push an undo row.
		let t2 = host.edit_begin("io").unwrap();
		host.project_save(file.to_str(), Some(&t2)).expect("save");
		let info = host.project_open(file.to_str().unwrap(), Some(&t2)).expect("open");
		assert!(
			!info.name.is_empty(),
			"open should report the loaded project's name"
		);
		assert_eq!(
			graphops::undo_count(),
			before,
			"project.open pushed an undo row"
		);

		// Committing a group that only did open/save must leave no blank row.
		host.edit_commit(&t2).expect("commit");
		assert_eq!(
			graphops::undo_count(),
			before,
			"the empty transaction left an undo row"
		);
		let _ = std::fs::remove_dir_all(&dir);
	}

	/// A transaction is single-holder: a second `begin` conflicts, and a
	/// foreign token cannot commit someone else's work (protocol §6 rule 1).
	#[test]
	fn transaction_is_single_holder() {
		let _g = graphops::test_lock();
		graphops::undo_clear();
		let project = graphops::create_project();
		let _seq = graphops::create_sequence(&project, "Seq");
		let host = OppHost::new();
		host.attach_project(project);

		let a = host.edit_begin("a").unwrap();
		let err = host.edit_begin("b").unwrap_err();
		assert_eq!(err.code, oak_extensions::protocol::TRANSACTION_CONFLICT);
		let err2 = host.edit_commit("not-my-token").unwrap_err();
		assert_eq!(err2.code, oak_extensions::protocol::TRANSACTION_CONFLICT);
		host.edit_commit(&a).unwrap();
		// After release a fresh begin works. Close it again before the test
		// ends: the token opens oakundo's *process-wide* group slot, so
		// leaving it open would make every later `group_begin` in this test
		// process fail with `undo: call not valid in current state`
		// (measured: this was the deterministic leak behind the undo failures
		// in `panels::ai_assistant` and `panels::history`).
		let b = host.edit_begin("b").unwrap();
		host.edit_abort(&b).unwrap();
	}
}

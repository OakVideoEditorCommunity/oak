//! In-memory backend: the feature-free implementation of
//! [`crate::backend::EngineBackend`].
//!
//! `MockBackend` is a small, deterministic timeline model used by the
//! unit tests and by the mock E2E chain on machines without the project
//! FFmpeg / engine build. It is a **test double**, not an engine:
//!
//! * media metadata it reports for a real file on disk (640x360, 30 fps,
//!   10 s) is a fixed synthetic fixture, flagged `"synthetic": true`;
//! * `render.get_frame` synthesises a solid colour per footage path
//!   (FNV-1a of the path) instead of decoding pixels;
//! * `render.get_audio_levels` reports zeros — the mock has no audio
//!   engine;
//! * `export.start` validates and acknowledges but writes no file.
//!
//! Everything that carries protocol semantics — schema gating, one undo
//! group per call, abort-rollback, structured errors, timecode grids —
//! behaves exactly as the SDK backend does, because both share
//! [`crate::dispatch`] and [`crate::txn`].

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::backend::EngineBackend;
use crate::error::{ToolError, CODE_NOTHING_TO_UNDO, CODE_STATE, CODE_TRANSACTION};
use crate::render::{Frame, frame_summary};
use crate::timecode::{Rat, TimeContext, TimeValue};
use crate::tools;
use crate::tools::timeline::{Side, TransitionType};
use crate::txn::TxnOps;
use std::hash::Hasher as _;

/// Synthetic geometry the mock reports for every probed/imported file.
pub const MOCK_WIDTH: u32 = 640;
/// Synthetic geometry the mock reports for every probed/imported file.
pub const MOCK_HEIGHT: u32 = 360;
/// Synthetic frame rate the mock reports for every imported footage.
pub const MOCK_FPS: (i64, i64) = (30, 1);
/// Synthetic duration (seconds) the mock reports for every imported
/// footage: `place_clip` fills the remaining media, so a 10 s fixture
/// gives the E2E chain room to split/trim.
pub const MOCK_DURATION_SECS: i64 = 10;

fn rat(num: i64, den: i64) -> Rat {
	Rat::new(num, den).expect("static rational")
}

fn zero() -> Rat {
	rat(0, 1)
}

/// Deterministic synthetic footage metadata (documented in the module
/// docs and the README: this is a fixture, not a probe result).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Footage {
	/// Opaque tool-face id.
	pub id: String,
	/// Source path as imported.
	pub path: String,
	/// Synthetic frame rate.
	pub fps: Rat,
	/// Synthetic duration.
	pub duration: Rat,
	/// Synthetic width.
	pub width: u32,
	/// Synthetic height.
	pub height: u32,
}

/// An effect instance hanging off a clip (mock `node.*` state).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effect {
	/// Opaque tool-face id (`node-<n>`).
	pub id: String,
	/// Effect type id as passed to `node.add_effect`.
	pub kind: String,
	/// Live parameter values, keyed by parameter name. `BTreeMap` keeps
	/// the serialised form deterministic.
	pub params: BTreeMap<String, Value>,
}

/// A transition attached to a clip (mock `timeline.add_transition`
/// state; the mock does not model neighbour wiring).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transition {
	/// Opaque tool-face id.
	pub id: String,
	/// `in` or `out`.
	pub side: String,
	/// Engine shader id.
	kind: String,
	/// Consumed duration on the sequence timeline.
	duration: Rat,
}

/// A clip on a track.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clip {
	/// Opaque tool-face id.
	pub id: String,
	/// Owning footage id.
	pub footage: String,
	/// Timeline start (rational seconds).
	pub in_: Rat,
	/// Duration on the timeline.
	pub length: Rat,
	/// In-point inside the source media.
	pub media_in: Rat,
	/// Effect chain, in insert order.
	pub effects: Vec<Effect>,
	/// Transitions attached to this clip.
	pub transitions: Vec<Transition>,
}

impl Clip {
	/// Timeline end (exclusive).
	pub fn out(&self) -> Result<Rat, ToolError> {
		self.in_.checked_add(self.length)
	}
}

/// A track of a sequence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
	/// Opaque tool-face id.
	pub id: String,
	/// `video` or `audio`.
	pub kind: String,
	/// Clips in timeline order (the mock keeps them sorted).
	pub clips: Vec<Clip>,
}

/// A sequence marker (`timeline.add_marker`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Marker {
	/// Opaque tool-face id.
	pub id: String,
	/// Marker position.
	time: Rat,
	/// Optional label.
	name: Option<String>,
	/// Optional `#RRGGBB[AA]`.
	color: Option<String>,
}

/// A sequence with its tracks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sequence {
	/// Opaque tool-face id.
	pub id: String,
	/// Sequence name.
	pub name: String,
	/// Tracks in insertion order.
	pub tracks: Vec<Track>,
	/// Markers in insertion order.
	pub markers: Vec<Marker>,
}

/// Deterministic id counters. They live inside the project state so an
/// undo restores them and subsequent ids stay stable — required by the
/// "serialisation equals the initial state" E2E assertion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Counters {
	footage: u64,
	sequence: u64,
	track: u64,
	clip: u64,
	node: u64,
	marker: u64,
	transition: u64,
}

impl Default for Counters {
	fn default() -> Self {
		Self {
			footage: 0,
			sequence: 0,
			track: 0,
			clip: 0,
			node: 0,
			marker: 0,
			transition: 0,
		}
	}
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
			other => unreachable!("unknown id kind {other}"),
		};
		*slot += 1;
		*slot
	}
}

/// The whole undoable mock state. `PartialEq` + deterministic
/// serialisation are the backbone of the undo-restore assertions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectState {
	/// Display name.
	pub name: String,
	/// Recorded save/open path (informational).
	pub path: Option<String>,
	/// Sequence frame rate (the grid for frame indices).
	pub fps: Rat,
	/// Imported footage, in import order.
	pub footage: Vec<Footage>,
	/// Sequences, in creation order. The first one is the active
	/// sequence when a tool omits `sequence`.
	pub sequences: Vec<Sequence>,
	/// Id counters.
	pub counters: Counters,
	/// Per-sequence format for the v2 `sequence.set_format` face, keyed by
	/// sequence id.
	///
	/// `#[serde(skip_serializing)]` keeps the frozen tool-face JSON
	/// byte-identical (the 22 tools never see this field), while deriving
	/// `PartialEq`/`Clone` through `ProjectState` means the mock's snapshot
	/// undo restores it like the rest of the timeline.
	#[serde(skip_serializing)]
	pub format: std::collections::BTreeMap<String, SequenceFormat>,
}

/// The v2 format of a mock sequence (defaults: the project grid, 1920×1080,
/// progressive).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SequenceFormat {
	pub name: String,
	pub width: u32,
	pub height: u32,
	pub rate: Rat,
	pub interlaced: bool,
}

/// An open undo group: the label (tool name) plus the state snapshot
/// taken at `txn_begin`.
struct Pending {
	label: String,
	snapshot: Model,
}

/// The undo stack entry: state snapshot taken before the group ran.
struct UndoEntry {
	label: String,
	snapshot: Model,
}

/// The mock's serialisable state = project + open transaction bookkeeping
/// that matters for equality (the undo stack itself is *not* part of the
/// timeline state; `edit.undo` empties it by construction).
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Model {
	project: Option<ProjectState>,
}

/// In-memory backend used by the unit tests and the mock E2E chain.
pub struct MockBackend {
	model: Model,
	pending: Option<Pending>,
	undo_stack: Vec<UndoEntry>,
}

impl MockBackend {
	/// A backend with no project open.
	pub fn new() -> Self {
		Self {
			model: Model { project: None },
			pending: None,
			undo_stack: Vec::new(),
		}
	}

	/// Whether at least one committed undo group can be undone.
	pub fn can_undo(&self) -> bool {
		!self.undo_stack.is_empty()
	}

	/// Number of committed undo groups.
	pub fn undo_depth(&self) -> usize {
		self.undo_stack.len()
	}

	/// The current state (project or `None`).
	pub fn project(&self) -> Option<&ProjectState> {
		self.model.project.as_ref()
	}

	/// Deterministic JSON serialisation of the whole mock state — the
	/// value the E2E compares before and after a full undo.
	pub fn serialize(&self) -> Value {
		serde_json::to_value(&self.model).expect("mock state serialisation")
	}

	/// JSON serialisation of the timeline half (sequences + footage).
	/// The mock E2E asserts this equals the initial snapshot after
	/// undoing every group.
	pub fn serialize_timeline(&self) -> Value {
		let project = self.model.project.as_ref();
		json!({
			"footage": project.map(|p| &p.footage),
			"sequences": project.map(|p| &p.sequences),
		})
	}

	// -- lookups --------------------------------------------------------

	fn require_project(&self) -> Result<&ProjectState, ToolError> {
		self.model.project.as_ref().ok_or_else(|| {
			ToolError::new(CODE_STATE, "no project is open (call project.open first)")
		})
	}

	fn project_mut(&mut self) -> Result<&mut ProjectState, ToolError> {
		self.model.project.as_mut().ok_or_else(|| {
			ToolError::new(CODE_STATE, "no project is open (call project.open first)")
		})
	}

	/// The active sequence (the first one) or a structured error.
	fn active_sequence_index(&self) -> Result<usize, ToolError> {
		let project = self.require_project()?;
		if project.sequences.is_empty() {
			return Err(ToolError::new(
				CODE_STATE,
				"the project has no sequence (call timeline.add_track after project.open \
				 creates the default sequence)"
			));
		}
		Ok(0)
	}

	/// Resolves an optional `sequence` argument to an index.
	fn sequence_index(&self, sequence: Option<&str>) -> Result<usize, ToolError> {
		let project = self.require_project()?;
		match sequence {
			None => self.active_sequence_index(),
			Some(id) => project
				.sequences
				.iter()
				.position(|s| s.id == id)
				.ok_or_else(|| ToolError::not_found(format!("unknown sequence '{id}'"))),
		}
	}

	/// Locates a track id as `(sequence, track)`.
	fn locate_track(&self, id: &str) -> Result<(usize, usize), ToolError> {
		let project = self.require_project()?;
		for (si, seq) in project.sequences.iter().enumerate() {
			if let Some(ti) = seq.tracks.iter().position(|t| t.id == id) {
				return Ok((si, ti));
			}
		}
		Err(ToolError::not_found(format!("unknown track '{id}'")))
	}

	/// Locates a clip id as `(sequence, track, clip)`.
	fn locate_clip(&self, id: &str) -> Result<(usize, usize, usize), ToolError> {
		let project = self.require_project()?;
		for (si, seq) in project.sequences.iter().enumerate() {
			for (ti, track) in seq.tracks.iter().enumerate() {
				if let Some(ci) = track.clips.iter().position(|c| c.id == id) {
					return Ok((si, ti, ci));
				}
			}
		}
		Err(ToolError::not_found(format!("unknown clip '{id}'")))
	}

	/// Locates an effect node id as `(sequence, track, clip, effect)`.
	fn locate_effect(&self, id: &str) -> Result<(usize, usize, usize, usize), ToolError> {
		let project = self.require_project()?;
		for (si, seq) in project.sequences.iter().enumerate() {
			for (ti, track) in seq.tracks.iter().enumerate() {
				for (ci, clip) in track.clips.iter().enumerate() {
					if let Some(ei) = clip.effects.iter().position(|e| e.id == id) {
						return Ok((si, ti, ci, ei));
					}
				}
			}
		}
		Err(ToolError::not_found(format!("unknown node '{id}'")))
	}

	/// Whether `[start, start+length)` intersects any clip of the track.
	fn overlaps(
		track: &Track,
		start: &Rat,
		length: &Rat,
		exclude_clip: Option<&str>,
	) -> Result<bool, ToolError> {
		let end = start.checked_add(*length)?;
		for clip in &track.clips {
			if exclude_clip == Some(clip.id.as_str()) {
				continue;
			}
			let clip_out = clip.out()?;
			if *start < clip_out && clip.in_ < end {
				return Ok(true);
			}
		}
		Ok(false)
	}

	/// Rejects a negative timeline position.
	fn require_non_negative(time: Rat, what: &str) -> Result<Rat, ToolError> {
		if time < zero() {
			return Err(ToolError::invalid_params(format!(
				"{what} must not be negative, got {time}"
			)));
		}
		Ok(time)
	}

	/// Frame grid of the sequence owning `ctx` (for `time_rate`).
	fn rate_of_context(&self, ctx: &TimeContext) -> Result<Rat, ToolError> {
		let project = self.require_project()?;
		match ctx {
			TimeContext::Sequence(id) => {
				let index = self.sequence_index(if id.is_empty() { None } else { Some(id) })?;
				Ok(project.sequences[index].fps_match())
			}
			TimeContext::Track(id) => {
				let (si, _) = self.locate_track(id)?;
				Ok(project.sequences[si].fps_match())
			}
			TimeContext::Clip(id) => {
				let (si, _, _) = self.locate_clip(id)?;
				Ok(project.sequences[si].fps_match())
			}
			TimeContext::Footage(id) => project
				.footage
				.iter()
				.find(|f| f.id == *id)
				.map(|f| f.fps)
				.ok_or_else(|| ToolError::not_found(format!("unknown footage '{id}'"))),
		}
	}
}

impl Default for MockBackend {
	fn default() -> Self {
		Self::new()
	}
}

/// Sequence fps helper kept next to the lookups above.
impl Sequence {
	fn fps_match(&self) -> Rat {
		// Sequences inherit the project frame rate in the mock; the
		// field lives on the project, so this is only reachable through
		// `rate_of_context`, which passes it in. Kept as a method for
		// readability at the call site.
		rat(MOCK_FPS.0, MOCK_FPS.1)
	}
}

// ---------------------------------------------------------------------------
// Transaction primitives (one tool call = one undo group)
// ---------------------------------------------------------------------------

impl TxnOps for MockBackend {
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
		if self.pending.is_some() {
			return Err(ToolError::new(
				CODE_TRANSACTION,
				"nested undo group: the mock backend allows one open group at a time",
			));
		}
		self.pending = Some(Pending {
			label: label.to_string(),
			snapshot: self.model.clone(),
		});
		Ok(())
	}

	fn txn_commit(&mut self) -> Result<(), ToolError> {
		let pending = self.pending.take().ok_or_else(|| {
			ToolError::new(CODE_TRANSACTION, "txn_commit without an open undo group")
		})?;
		self.undo_stack.push(UndoEntry {
			label: pending.label,
			snapshot: pending.snapshot,
		});
		Ok(())
	}

	fn txn_abort(&mut self) -> Result<(), ToolError> {
		let pending = self.pending.take().ok_or_else(|| {
			ToolError::new(CODE_TRANSACTION, "txn_abort without an open undo group")
		})?;
		self.model = pending.snapshot;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// EngineBackend
// ---------------------------------------------------------------------------

impl EngineBackend for MockBackend {
	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError> {
		self.rate_of_context(&ctx)
	}

	// --- project -------------------------------------------------------

	fn project_open(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::project::parse_open(args)?;
		let name = parsed.name.clone().unwrap_or_else(|| {
			parsed
				.path
				.as_deref()
				.map(tools::project::file_stem)
				.unwrap_or_else(|| "Untitled".to_string())
		});
		let fps = parsed.fps.unwrap_or_else(|| rat(MOCK_FPS.0, MOCK_FPS.1));
		let sequences = vec![Sequence {
			id: "sequence-1".to_string(),
			name: name.clone(),
			tracks: vec![
				Track {
					id: "track-1".to_string(),
					kind: "video".to_string(),
					clips: Vec::new(),
				},
				Track {
					id: "track-2".to_string(),
					kind: "audio".to_string(),
					clips: Vec::new(),
				},
			],
			markers: Vec::new(),
		}];
		let created = parsed.path.is_none();
		self.model.project = Some(ProjectState {
			name,
			path: parsed.path.clone(),
			fps,
			footage: Vec::new(),
			sequences,
			counters: Counters::default(),
			format: std::collections::BTreeMap::new(),
		});
		// The default sequence owns ids track-1/track-2 above; start the
		// counters past them so later allocations never collide.
		if let Some(project) = self.model.project.as_mut() {
			project.counters.sequence = 1;
			project.counters.track = 2;
		}
		let project = self.require_project()?;
		Ok(json!({
			"project": {
				"name": project.name,
				"path": project.path,
				"fps": project.fps.to_json(),
			},
			"created": created,
			"backend": "mock",
		}))
	}

	fn project_save(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::project::parse_save(args)?;
		// A save records the destination (state) but writes no bytes in
		// the mock; the byte count is the size of the serialised state,
		// which is what a caller could observe here.
		let bytes = serde_json::to_string(&self.model)
			.map_err(|err| ToolError::backend(format!("state serialisation failed: {err}")))?
			.len();
		let project = self.project_mut()?;
		let path = parsed
			.path
			.clone()
			.or_else(|| project.path.clone())
			.ok_or_else(|| {
				ToolError::invalid_params(
					"no 'path' argument and the project has no file path yet",
				)
			})?;
		project.path = Some(path.clone());
		Ok(json!({
			"path": path,
			"saved": true,
			"bytes": bytes,
			"note": "mock backend serialises state in memory; no file was written",
		}))
	}

	fn project_get_overview(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let project = self.require_project()?;
		let sequences: Vec<Value> = project
			.sequences
			.iter()
			.map(|seq| {
				let tracks: Vec<Value> = seq
					.tracks
					.iter()
					.map(|track| {
						let clips: Vec<Value> = track
							.clips
							.iter()
							.map(|clip| {
								let out = clip.out().ok().map(|o| o.to_json());
								json!({
									"id": clip.id,
									"footage": clip.footage,
									"in": clip.in_.to_json(),
									"out": out,
									"length": clip.length.to_json(),
									"media_in": clip.media_in.to_json(),
									"effects": clip.effects.iter().map(|e| json!({
										"id": e.id,
										"kind": e.kind,
										"params": e.params,
									})).collect::<Vec<_>>(),
									"transitions": clip.transitions.iter().map(|t| json!({
										"id": t.id,
										"side": t.side,
										"type": t.kind,
										"duration": t.duration.to_json(),
									})).collect::<Vec<_>>(),
								})
							})
							.collect();
						json!({
							"id": track.id,
							"type": track.kind,
							"clips": clips,
						})
					})
					.collect();
				let markers: Vec<Value> = seq
					.markers
					.iter()
					.map(|m| {
						json!({
							"id": m.id,
							"time": m.time.to_json(),
							"name": m.name,
							"color": m.color,
						})
					})
					.collect();
				json!({
					"id": seq.id,
					"name": seq.name,
					"tracks": tracks,
					"markers": markers,
				})
			})
			.collect();
		let footage: Vec<Value> = project
			.footage
			.iter()
			.map(|f| footage_json(f))
			.collect();
		Ok(json!({
			"project": {
				"name": project.name,
				"path": project.path,
				"fps": project.fps.to_json(),
			},
			"sequences": sequences,
			"footage": footage,
			"backend": "mock",
		}))
	}

	// --- media ---------------------------------------------------------

	fn media_probe(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::media::parse_probe(args)?;
		let meta = std::fs::metadata(&parsed.path).map_err(|err| {
			ToolError::not_found(format!("cannot probe '{}': {err}", parsed.path))
		})?;
		Ok(json!({
			"path": parsed.path,
			"exists": true,
			"bytes": meta.len(),
			"width": MOCK_WIDTH,
			"height": MOCK_HEIGHT,
			"fps": rat(MOCK_FPS.0, MOCK_FPS.1).to_json(),
			"duration": rat(MOCK_DURATION_SECS, 1).to_json(),
			"synthetic": true,
			"note": "mock backend: geometry/fps/duration are fixed fixtures, not a probe result",
		}))
	}

	fn media_import_footage(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::media::parse_import(args)?;
		let project = self.project_mut()?;
		let mut imported = Vec::new();
		let mut failed = Vec::new();
		for path in &parsed.paths {
			match std::fs::metadata(path) {
				Ok(_) => {
					let id = format!("footage-{}", project.counters.next("footage"));
					let footage = Footage {
						id: id.clone(),
						path: path.clone(),
						fps: rat(MOCK_FPS.0, MOCK_FPS.1),
						duration: rat(MOCK_DURATION_SECS, 1),
						width: MOCK_WIDTH,
						height: MOCK_HEIGHT,
					};
					imported.push(footage_json(&footage));
					project.footage.push(footage);
				}
				Err(err) => failed.push(json!({
					"path": path,
					"error": format!("cannot import '{path}': {err}"),
				})),
			}
		}
		Ok(json!({
			"footage": imported,
			"failed": failed,
			"backend": "mock",
		}))
	}

	fn media_list_footage(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let project = self.require_project()?;
		let footage: Vec<Value> = project.footage.iter().map(footage_json).collect();
		Ok(json!({ "footage": footage, "backend": "mock" }))
	}

	// --- timeline ------------------------------------------------------

	fn timeline_add_track(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_add_track(args)?;
		let sequence = self.sequence_index(parsed.sequence.as_deref())?;
		let project = self.project_mut()?;
		let seq = &mut project.sequences[sequence];
		let at = match parsed.index {
			Some(index) if index <= seq.tracks.len() => index,
			Some(index) => {
				return Err(ToolError::invalid_params(format!(
					"index {index} is out of range (the sequence has {} tracks)",
					seq.tracks.len()
				)));
			}
			None => seq.tracks.len(),
		};
		let id = format!("track-{}", project.counters.next("track"));
		let track = Track {
			id: id.clone(),
			kind: parsed.track_type.clone(),
			clips: Vec::new(),
		};
		seq.tracks.insert(at, track);
		Ok(json!({
			"track": id,
			"sequence": seq.id,
			"index": at,
			"type": parsed.track_type,
			"backend": "mock",
		}))
	}

	fn timeline_place_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_place_clip(args)?;
		let (si, ti) = self.locate_track(&parsed.track)?;
		let footage_id = parsed.footage.clone();
		let project = self.require_project()?;
		let footage = project
			.footage
			.iter()
			.find(|f| f.id == footage_id)
			.ok_or_else(|| ToolError::not_found(format!("unknown footage '{footage_id}'")))?
			.clone();
		let in_ = self.rational_at(
			TimeContext::Track(parsed.track.clone()),
			parsed.in_,
		)?;
		let in_ = Self::require_non_negative(in_, "clip start 'in'")?;
		let media_in = match parsed.media_in {
			Some(value) => self.rational_at(TimeContext::Footage(footage.id.clone()), value)?,
			None => zero(),
		};
		if media_in < zero() {
			return Err(ToolError::invalid_params(
				"'media_in' must not be negative",
			));
		}
		if media_in >= footage.duration {
			return Err(ToolError::invalid_params(format!(
				"'media_in' {media_in} is at/past the end of the footage ({}s)",
				footage.duration
			)));
		}
		// The frozen face has no 'out': the clip runs to the end of the
		// footage and is shortened afterwards with timeline.trim_clip.
		let length = footage.duration.checked_sub(media_in)?;
		let project = self.project_mut()?;
		let track = &project.sequences[si].tracks[ti];
		if Self::overlaps(track, &in_, &length, None)? {
			return Err(ToolError::invalid_params(format!(
				"placing [{in_}, {}) would overlap an existing clip on track {}",
				in_.checked_add(length)?,
				track.id
			)));
		}
		let id = format!("clip-{}", project.counters.next("clip"));
		let clip = Clip {
			id: id.clone(),
			footage: footage.id.clone(),
			in_,
			length,
			media_in,
			effects: Vec::new(),
			transitions: Vec::new(),
		};
		let track = &mut project.sequences[si].tracks[ti];
		track.clips.push(clip);
		track.clips.sort_by_key(|c| c.in_);
		let out = in_.checked_add(length)?;
		Ok(json!({
			"clip": id,
			"track": track.id,
			"in": in_.to_json(),
			"out": out.to_json(),
			"length": length.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_split_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_split(args)?;
		let (si, ti, ci) = self.locate_clip(&parsed.clip)?;
		let at = self.rational_at(TimeContext::Clip(parsed.clip.clone()), parsed.time)?;
		let project = self.require_project()?;
		let original = &project.sequences[si].tracks[ti].clips[ci];
		let out = original.out()?;
		if !(original.in_ < at && at < out) {
			return Err(ToolError::invalid_params(format!(
				"split point {at} is outside the clip [{}, {out})",
				original.in_
			)));
		}
		let left_length = at.checked_sub(original.in_)?;
		let right_length = out.checked_sub(at)?;
		let media_split = original.media_in.checked_add(left_length)?;
		let right_id = format!("clip-{}", {
			let project = self.project_mut()?;
			project.counters.next("clip")
		});
		let project = self.project_mut()?;
		let track = &mut project.sequences[si].tracks[ti];
		let mut right = track.clips[ci].clone();
		right.id = right_id.clone();
		right.in_ = at;
		right.length = right_length;
		right.media_in = media_split;
		// The original keeps the left half; transitions/effects stay on
		// the left half (the mock does not model seam redistribution).
		track.clips[ci].length = left_length;
		track.clips.insert(ci + 1, right);
		Ok(json!({
			"clip": parsed.clip,
			"new_clip": right_id,
			"time": at.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_trim_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_trim(args)?;
		let (si, ti, ci) = self.locate_clip(&parsed.clip)?;
		let at = self.rational_at(TimeContext::Clip(parsed.clip.clone()), parsed.time)?;
		if at < zero() {
			return Err(ToolError::invalid_params(
				"trim position must not be negative",
			));
		}
		let project = self.require_project()?;
		let track = &project.sequences[si].tracks[ti];
		let old_in = track.clips[ci].in_;
		let old_out = track.clips[ci].out()?;
		let media_in = track.clips[ci].media_in;
		let footage_id = track.clips[ci].footage.clone();
		let footage_duration = project
			.footage
			.iter()
			.find(|f| f.id == footage_id)
			.map(|f| f.duration)
			.ok_or_else(|| {
				ToolError::not_found(format!("footage '{footage_id}' disappeared"))
			})?;
		let (new_in, new_out, new_media_in) = match parsed.side {
			Side::In => {
				if at >= old_out {
					return Err(ToolError::invalid_params(format!(
						"trimming the in point to {at} leaves no clip (out is {old_out})"
					)));
				}
				let delta = at.checked_sub(old_in)?;
				(at, old_out, media_in.checked_add(delta)?)
			}
			Side::Out => {
				if at <= old_in {
					return Err(ToolError::invalid_params(format!(
						"trimming the out point to {at} leaves no clip (in is {old_in})"
					)));
				}
				(old_in, at, media_in)
			}
		};
		let new_length = new_out.checked_sub(new_in)?;
		let media_end = new_media_in.checked_add(new_length)?;
		if media_end > footage_duration {
			return Err(ToolError::invalid_params(format!(
				"trim would run past the end of the footage (media {} + length {} > {})",
				new_media_in, new_length, footage_duration
			)));
		}
		// No overlap with the neighbours after the trim.
		let project = self.require_project()?;
		let track = &project.sequences[si].tracks[ti];
		for (index, other) in track.clips.iter().enumerate() {
			if index == ci {
				continue;
			}
			let other_out = other.out()?;
			if new_in < other_out && other.in_ < new_out {
				return Err(ToolError::invalid_params(format!(
					"trim to [{new_in}, {new_out}) would overlap clip {}",
					other.id
				)));
			}
		}
		let project = self.project_mut()?;
		let clip = &mut project.sequences[si].tracks[ti].clips[ci];
		clip.in_ = new_in;
		clip.length = new_length;
		clip.media_in = new_media_in;
		Ok(json!({
			"clip": parsed.clip,
			"side": parsed.side.as_str(),
			"in": new_in.to_json(),
			"out": new_out.to_json(),
			"length": new_length.to_json(),
			"media_in": new_media_in.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_move_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_move(args)?;
		let (from_si, from_ti, ci) = self.locate_clip(&parsed.clip)?;
		let to_ti = match parsed.track.as_deref() {
			Some(track_id) => {
				let (si, ti) = self.locate_track(track_id)?;
				if si != from_si {
					return Err(ToolError::invalid_params(
						"moving across sequences is not supported",
					));
				}
				ti
			}
			None => from_ti,
		};
		let at = self.rational_at(TimeContext::Clip(parsed.clip.clone()), parsed.in_)?;
		let at = Self::require_non_negative(at, "new 'in'")?;
		let project = self.require_project()?;
		let length = project.sequences[from_si].tracks[from_ti].clips[ci].length;
		let track = &project.sequences[from_si].tracks[to_ti];
		if Self::overlaps(track, &at, &length, Some(&parsed.clip))? {
			return Err(ToolError::invalid_params(format!(
				"moving to [{at}, {}) would overlap an existing clip on track {}",
				at.checked_add(length)?,
				track.id
			)));
		}
		let project = self.project_mut()?;
		let mut clip = project.sequences[from_si].tracks[from_ti].clips.remove(ci);
		clip.in_ = at;
		let track_id = project.sequences[from_si].tracks[to_ti].id.clone();
		let track = &mut project.sequences[from_si].tracks[to_ti];
		track.clips.push(clip);
		track.clips.sort_by_key(|c| c.in_);
		Ok(json!({
			"clip": parsed.clip,
			"track": track_id,
			"in": at.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_ripple_delete(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_ripple_delete(args)?;
		let (si, ti, ci) = self.locate_clip(&parsed.clip)?;
		let project = self.project_mut()?;
		let length = {
			let track = &project.sequences[si].tracks[ti];
			track.clips[ci].length
		};
		let track = &mut project.sequences[si].tracks[ti];
		track.clips.remove(ci);
		for clip in track.clips.iter_mut().skip(ci) {
			clip.in_ = clip.in_.checked_sub(length)?;
		}
		Ok(json!({
			"clip": parsed.clip,
			"removed": true,
			"ripple": length.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_add_transition(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_add_transition(args)?;
		let (si, ti, ci) = self.locate_clip(&parsed.clip)?;
		let at = self.rational_at(
			TimeContext::Clip(parsed.clip.clone()),
			parsed.duration,
		)?;
		if at <= zero() {
			return Err(ToolError::invalid_params(
				"transition duration must be positive",
			));
		}
		let project = self.project_mut()?;
		let id = format!("transition-{}", project.counters.next("transition"));
		let transition = Transition {
			id: id.clone(),
			side: parsed.side.as_str().to_string(),
			kind: parsed.transition_type.as_str().to_string(),
			duration: at,
		};
		project.sequences[si].tracks[ti].clips[ci]
			.transitions
			.push(transition);
		Ok(json!({
			"transition": id,
			"clip": parsed.clip,
			"side": parsed.side.as_str(),
			"type": parsed.transition_type.as_str(),
			"duration": at.to_json(),
			"backend": "mock",
		}))
	}

	fn timeline_add_marker(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_add_marker(args)?;
		let sequence = self.sequence_index(parsed.sequence.as_deref())?;
		let at = self.rational_at(
			TimeContext::Sequence(
				self.require_project()?.sequences[sequence].id.clone(),
			),
			parsed.time,
		)?;
		let project = self.project_mut()?;
		let id = format!("marker-{}", project.counters.next("marker"));
		let marker = Marker {
			id: id.clone(),
			time: at,
			name: parsed.name.clone(),
			color: parsed.color.clone(),
		};
		let sequence_id = project.sequences[sequence].id.clone();
		project.sequences[sequence].markers.push(marker);
		Ok(json!({
			"marker": id,
			"sequence": sequence_id,
			"time": at.to_json(),
			"name": parsed.name,
			"color": parsed.color,
			"backend": "mock",
		}))
	}

	// --- nodes ---------------------------------------------------------

	fn node_add_effect(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_add_effect(args)?;
		let (si, ti, ci) = self.locate_clip(&parsed.clip)?;
		let project = self.project_mut()?;
		let id = format!("node-{}", project.counters.next("node"));
		let clip = &mut project.sequences[si].tracks[ti].clips[ci];
		let index = match parsed.index {
			Some(index) if index <= clip.effects.len() => index,
			Some(index) => {
				return Err(ToolError::invalid_params(format!(
					"index {index} is out of range (the chain has {} effects)",
					clip.effects.len()
				)));
			}
			None => clip.effects.len(),
		};
		clip.effects.insert(
			index,
			Effect {
				id: id.clone(),
				kind: parsed.effect.clone(),
				params: BTreeMap::new(),
			},
		);
		Ok(json!({
			"node": id,
			"clip": parsed.clip,
			"effect": parsed.effect,
			"index": index,
			"backend": "mock",
		}))
	}

	fn node_set_param(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_set_param(args)?;
		tools::node::check_range(&parsed.key, &parsed.value)?;
		let (si, ti, ci, ei) = self.locate_effect(&parsed.node)?;
		let project = self.project_mut()?;
		let effect = &mut project.sequences[si].tracks[ti].clips[ci].effects[ei];
		effect.params.insert(parsed.key.clone(), parsed.value.clone());
		Ok(json!({
			"node": parsed.node,
			"key": parsed.key,
			"value": parsed.value,
			"backend": "mock",
		}))
	}

	fn node_list_types(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_list_types(args)?;
		if let Some(node) = parsed.node.as_deref() {
			let (si, ti, ci, ei) = self.locate_effect(node)?;
			let project = self.require_project()?;
			let effect = &project.sequences[si].tracks[ti].clips[ci].effects[ei];
			let params: Vec<Value> = effect
				.params
				.iter()
				.map(|(key, value)| json!({ "key": key, "value": value }))
				.collect();
			return Ok(json!({
				"node": node,
				"kind": effect.kind,
				"params": params,
				"backend": "mock",
			}));
		}
		let category = parsed.category.as_deref().unwrap_or("all");
		let mut types = Vec::new();
		if category == "effect" || category == "all" {
			// Synthetic fixture list (documented): the mock does not
			// mirror the engine factory table.
			for (type_id, name) in [
				("org.olivevideoeditor.Olive.blur", "Blur"),
				("org.olivevideoeditor.Olive.brightnesscontrast", "Brightness & Contrast"),
				("org.olivevideoeditor.Olive.sharpen", "Sharpen"),
				("org.olivevideoeditor.Olive.opacity", "Opacity"),
				("org.olivevideoeditor.Olive.volumegain", "Volume (Gain)"),
			] {
				types.push(json!({
					"type_id": type_id,
					"name": name,
					"category": "effect",
				}));
			}
		}
		if category == "transition" || category == "all" {
			for kind in [
				TransitionType::Crossdissolve,
				TransitionType::Fade,
				TransitionType::Wipe,
				TransitionType::Slide,
			] {
				types.push(json!({
					"type_id": kind.as_str(),
					"name": kind.as_str(),
					"category": "transition",
				}));
			}
		}
		Ok(json!({
			"types": types,
			"category": category,
			"backend": "mock",
			"note": "mock backend: the effect list is a synthetic fixture",
		}))
	}

	// --- render --------------------------------------------------------

	fn render_get_frame(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::render_tools::parse_get_frame(args)?;
		let at = match &parsed.target {
			crate::tools::render_tools::FrameTarget::Sequence(id) => {
				let index = self.sequence_index(if id.is_empty() {
					None
				} else {
					Some(id)
				})?;
				let at = self.rational_at(TimeContext::Sequence(id.clone()), parsed.time)?;
				// The mock renders from the first video track that has
				// a clip covering `at`.
				let project = self.require_project()?;
				let sequence = &project.sequences[index];
				let colour = sequence
					.tracks
					.iter()
					.filter(|t| t.kind == "video")
					.find_map(|track| {
						track.clips.iter().find(|clip| {
							clip.in_ <= at && at < clip.out().unwrap_or(clip.in_)
						})
					})
					.and_then(|clip| {
						project
							.footage
							.iter()
							.find(|f| f.id == clip.footage)
							.map(|f| footage_rgb(&f.path))
					});
				colour
			}
			crate::tools::render_tools::FrameTarget::Footage(id) => {
				let at = self.rational_at(TimeContext::Footage(id.clone()), parsed.time)?;
				let project = self.require_project()?;
				project
					.footage
					.iter()
					.find(|f| f.id == *id)
					.map(|f| footage_rgb(&f.path))
			}
		};
		let (width, height) = parsed.max_size.unwrap_or((320, 180));
		let mut frame = Frame::black(width, height);
		if let Some(rgb) = at {
			for y in 0..height {
				for x in 0..width {
					frame.set_pixel(x, y, [rgb[0], rgb[1], rgb[2], 1.0]);
				}
			}
		}
		let mut out = frame_summary(&frame, &parsed.format);
		out["target"] = json!(parsed.target.id());
		out["time"] = json!(parsed.time.to_json());
		out["backend"] = json!("mock");
		Ok(out)
	}

	fn render_get_thumbnails(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::render_tools::parse_get_thumbnails(args)?;
		let height = parsed.height.unwrap_or(90);
		let in_r = self.rational_at(
			TimeContext::Sequence(parsed.target.id().to_string()),
			parsed.range.in_,
		)?;
		let out_r = self.rational_at(
			TimeContext::Sequence(parsed.target.id().to_string()),
			parsed.range.out,
		)?;
		if out_r <= in_r {
			return Err(ToolError::invalid_params(format!(
				"range out {} must be greater than in {}",
				parsed.range.out, parsed.range.in_
			)));
		}
		let span = out_r.checked_sub(in_r)?;
		let mut frames = Vec::with_capacity(parsed.count);
		for index in 0..parsed.count {
			// t_i = in + span * i / count (left-closed, right-open).
			let offset = span.checked_mul_int(index as i64)?;
			let at = offset.checked_div_int(parsed.count as i64)?;
			let time = in_r.checked_add(at)?;
			let mut frame_args = json!({
				"time": time.to_json(),
				"max_size": {"width": height.max(1) * 16 / 9, "height": height},
				"format": "png",
			});
			match &parsed.target {
				crate::tools::render_tools::FrameTarget::Sequence(id) => {
					frame_args["sequence"] = json!(id);
				}
				crate::tools::render_tools::FrameTarget::Footage(id) => {
					frame_args["footage"] = json!(id);
				}
			}
			let mut frame = self.render_get_frame(&frame_args)?;
			frame["index"] = json!(index);
			frame["time"] = json!(time.to_json());
			frames.push(frame);
		}
		Ok(json!({
			"frames": frames,
			"count": parsed.count,
			"backend": "mock",
		}))
	}

	fn render_get_audio_levels(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::render_tools::parse_get_audio_levels(args)?;
		let sequence = self.sequence_index(Some(&parsed.sequence))?;
		let sequence_id = self.require_project()?.sequences[sequence].id.clone();
		let in_r = self.rational_at(TimeContext::Sequence(sequence_id.clone()), parsed.range.in_)?;
		let out_r = self.rational_at(TimeContext::Sequence(sequence_id), parsed.range.out)?;
		if out_r <= in_r {
			return Err(ToolError::invalid_params(format!(
				"range out {} must be greater than in {}",
				parsed.range.out, parsed.range.in_
			)));
		}
		let resolution = parsed.resolution.unwrap_or(64).max(1);
		let project = self.require_project()?;
		Ok(json!({
			"sequence": project.sequences[sequence].id,
			"resolution": resolution,
			"levels": vec![0.0f64; resolution as usize],
			"backend": "mock",
			"note": "mock backend has no audio engine; every level is zero by construction",
		}))
	}

	// --- export / edit -------------------------------------------------

	fn export_start(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::export::parse_export(args)?;
		let sequence = self.sequence_index(Some(&parsed.sequence))?;
		let project = self.require_project()?;
		Ok(json!({
			"sequence": project.sequences[sequence].id,
			"output_path": parsed.output_path,
			"preset": parsed.preset,
			"started": true,
			"backend": "mock",
			"note": "mock backend validates and acknowledges only; no file was written",
		}))
	}

	fn edit_undo(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let entry = self.undo_stack.pop().ok_or_else(|| {
			ToolError::new(CODE_NOTHING_TO_UNDO, "the undo stack is empty")
		})?;
		self.model = entry.snapshot;
		Ok(json!({
			"undone": entry.label,
			"remaining": self.undo_stack.len(),
			"backend": "mock",
		}))
	}
}

fn footage_json(footage: &Footage) -> Value {
	json!({
		"id": footage.id,
		"path": footage.path,
		"fps": footage.fps.to_json(),
		"duration": footage.duration.to_json(),
		"width": footage.width,
		"height": footage.height,
		"synthetic": true,
	})
}

/// Deterministic synthetic colour for a footage path (FNV-1a → RGB).
/// Public so tests can assert a rendered pixel without re-deriving the
/// rule; the mock never decodes the file.
pub fn footage_rgb(path: &str) -> [f32; 4] {
	let mut hasher = fnv::FnvHasher::default();
		hasher.write(path.as_bytes());
		let hash = hasher.finish();
	let r = ((hash >> 16) & 0xff) as u8;
	let g = ((hash >> 8) & 0xff) as u8;
	let b = (hash & 0xff) as u8;
	// Keep colours clearly non-black so gap-vs-footage assertions have
	// headroom; a zero channel would read as "black background".
	[
		(f64::from(r.max(32)) / 255.0) as f32,
		(f64::from(g.max(32)) / 255.0) as f32,
		(f64::from(b.max(32)) / 255.0) as f32,
		1.0,
	]
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::dispatch::execute;
	use serde_json::json;

	/// Two real files on disk (the mock only checks existence).
	fn temp_file(tag: &str) -> String {
		let path = std::env::temp_dir().join(format!("oak_mock_{tag}.mp4"));
		std::fs::write(&path, b"mock media fixture").expect("write fixture");
		path.to_string_lossy().to_string()
	}

	#[test]
	fn new_backend_has_no_project() {
		let mut backend = MockBackend::new();
		let err = backend.project_get_overview(&json!({})).unwrap_err();
		assert_eq!(err.code, CODE_STATE);
		assert!(!backend.can_undo());
	}

	#[test]
	fn open_save_overview_round_trip() {
		let mut backend = MockBackend::new();
		let out = execute(&mut backend, "project.open", json!({"fps": 30, "name": "cut"}))
			.expect("open");
		assert_eq!(out["created"], true);
		assert_eq!(out["project"]["fps"]["num"], 30);
		let overview =
			execute(&mut backend, "project.get_overview", json!({})).expect("overview");
		assert_eq!(overview["sequences"].as_array().unwrap().len(), 1);
		assert_eq!(overview["sequences"][0]["tracks"].as_array().unwrap().len(), 2);
		let saved = execute(
			&mut backend,
			"project.save",
			json!({"path": "/tmp/oak_mock_roundtrip.ove"}),
		)
		.expect("save");
		assert_eq!(saved["saved"], true);
	}

	#[test]
	fn import_requires_existing_files_and_reports_per_path() {
		let mut backend = MockBackend::new();
		execute(&mut backend, "project.open", json!({})).expect("open");
		let good = temp_file("import_ok");
		let out = execute(
			&mut backend,
			"media.import_footage",
			json!({"paths": [good, "/no/such/file.mp4"]}),
		)
		.expect("import");
		assert_eq!(out["footage"].as_array().unwrap().len(), 1);
		assert_eq!(out["failed"].as_array().unwrap().len(), 1);
		assert!(out["footage"][0]["synthetic"] == true);
		let _ = std::fs::remove_file(&good);
	}

	#[test]
	fn unknown_tool_and_schema_failures_stay_structured() {
		let mut backend = MockBackend::new();
		let err = execute(&mut backend, "nope.nope", json!({})).unwrap_err();
		assert_eq!(err.tool.as_deref(), Some("nope.nope"));
		assert_eq!(err.step.as_deref(), Some("lookup"));
		let err = execute(
			&mut backend,
			"timeline.split_clip",
			json!({"clip": "clip-1", "time": 1.5}),
		)
		.unwrap_err();
		assert_eq!(err.step.as_deref(), Some("validate"));
		assert!(err.txn.is_none(), "validation failures start no transaction");
	}
}

// ---------------------------------------------------------------------------
// v2 extension face (`sequence.*`, T9.2)
// ---------------------------------------------------------------------------

impl SequenceFormat {
	/// The default format of a mock sequence: the project grid, 1920×1080,
	/// progressive.
	fn default_for(project: &ProjectState, seq: &Sequence) -> Self {
		SequenceFormat {
			name: seq.name.clone(),
			width: 1920,
			height: 1080,
			rate: project.fps,
			interlaced: false,
		}
	}
}

impl MockBackend {
	/// The sequence the args name (or the first one) plus its format.
	fn mock_sequence_mut(
		&mut self,
		wanted: Option<&str>,
	) -> Result<(SequenceFormat, usize), ToolError> {
		let project = self
			.model
			.project
			.as_mut()
			.ok_or_else(|| ToolError::invalid_params("no project open"))?;
		let index = match wanted {
			Some(id) => project
				.sequences
				.iter()
				.position(|s| s.id == id)
				.ok_or_else(|| ToolError::invalid_params(format!("no sequence '{id}'")))?,
			None => 0,
		};
		let seq = project
			.sequences
			.get(index)
			.ok_or_else(|| ToolError::invalid_params("the project has no sequence"))?;
		let base = SequenceFormat::default_for(project, seq);
		let current = project
			.format
			.get(&seq.id)
			.cloned()
			.unwrap_or(base);
		Ok((current, index))
	}

	/// The wire shape both v2 tools return.
	fn settings_json(project: &ProjectState, format: &SequenceFormat, index: usize) -> Value {
		let seq_id = project
			.sequences
			.get(index)
			.map(|s| s.id.clone())
			.unwrap_or_default();
		// Duration: the longest clip out point on the project grid, in
		// seconds — the mock's analogue of `graphops::sequence_length`.
		let mut longest = Rat::new(0, 1).expect("static rational");
		if let Some(seq) = project.sequences.get(index) {
			for track in &seq.tracks {
				for clip in &track.clips {
					if let Ok(out) = clip.out() {
						if out > longest {
							longest = out;
						}
					}
				}
			}
		}
		serde_json::json!({
			"id": seq_id,
			"name": format.name,
			"width": format.width,
			"height": format.height,
			"rate": {"num": format.rate.num, "den": format.rate.den},
			"interlaced": format.interlaced,
			"duration": {"num": longest.num, "den": longest.den},
			"backend": "mock",
			"undoable": true,
		})
	}
}

impl crate::backend::EngineExt for MockBackend {
	fn sequence_get_settings(&self, args: Value) -> Result<Value, ToolError> {
		let parsed = crate::tools::v2::sequence::parse_get_settings(&args)?;
		let project = self
			.model
			.project
			.as_ref()
			.ok_or_else(|| ToolError::invalid_params("no project open"))?;
		let index = match parsed.sequence.as_deref() {
			Some(id) => project
				.sequences
				.iter()
				.position(|s| s.id == id)
				.ok_or_else(|| ToolError::invalid_params(format!("no sequence '{id}'")))?,
			None => 0,
		};
		let seq = project
			.sequences
			.get(index)
			.ok_or_else(|| ToolError::invalid_params("the project has no sequence"))?;
		let format = project
			.format
			.get(&seq.id)
			.cloned()
			.unwrap_or_else(|| SequenceFormat::default_for(project, seq));
		Ok(Self::settings_json(project, &format, index))
	}

	fn sequence_set_format(&mut self, args: Value) -> Result<Value, ToolError> {
		let parsed = crate::tools::v2::sequence::parse_set_format(&args)?;
		let (current, index) = self.mock_sequence_mut(parsed.sequence.as_deref())?;
		// Partial update: only the supplied fields move.
		let after = SequenceFormat {
			name: parsed.name.unwrap_or(current.name),
			width: parsed.width.unwrap_or(current.width),
			height: parsed.height.unwrap_or(current.height),
			rate: parsed.fps.unwrap_or(current.rate),
			interlaced: parsed.interlaced.unwrap_or(current.interlaced),
		};
		let project = self
			.model
			.project
			.as_mut()
			.ok_or_else(|| ToolError::invalid_params("no project open"))?;
		let seq_id = project
			.sequences
			.get(index)
			.map(|s| s.id.clone())
			.ok_or_else(|| ToolError::invalid_params("the project has no sequence"))?;
		project.format.insert(seq_id, after.clone());
		Ok(Self::settings_json(project, &after, index))
	}
}

/// Guard tests for the v2 side table (T9.2).
///
/// `ProjectState::format` is `#[serde(skip_serializing)]` so that adding
/// the v2 face did **not** change the frozen 22-tool JSON by a single
/// byte. These tests are the tripwire: remove the attribute and they go
/// red before any frozen-surface test notices.
#[cfg(test)]
mod v2_serde_guard_tests {
	use super::*;
	use crate::backend::EngineExt;
	use serde_json::json;

	/// The serialised project state carries no `format` key at all.
	#[test]
	fn project_state_serialisation_omits_the_v2_format_table() {
		let mut mock = MockBackend::new();
		mock.project_open(&json!({ "name": "Serde Guard" }))
			.expect("open");
		mock.sequence_set_format(json!({ "fps": 24, "width": 1280 }))
			.expect("set format");

		let project = mock
			.model
			.project
			.as_ref()
			.expect("the mock holds a project");
		// The side table is populated (otherwise this guard would be vacuous).
		assert!(
			!project.format.is_empty(),
			"the test must actually fill the side table"
		);

		let value = serde_json::to_value(project).expect("serialise the project state");
		let obj = value.as_object().expect("an object");
		assert!(
			obj.get("format").is_none(),
			"the v2 side table leaked into the frozen JSON: {value}"
		);
		// Nothing nested either: the key name must not appear anywhere.
		let text = value.to_string();
		assert!(
			!text.contains("\"format\""),
			"no `format` key may appear in the frozen payload: {text}"
		);
	}

	/// The frozen tool-face payload is byte-identical before and after a
	/// v2 edit: `project.get_overview` is what the frozen 22 report, and a
	/// `sequence.set_format` call must not perturb it.
	#[test]
	fn a_v2_edit_does_not_perturb_the_frozen_overview_payload() {
		let mut mock = MockBackend::new();
		mock.project_open(&json!({ "name": "Byte Guard" }))
			.expect("open");
		let before = mock.project_get_overview(&Value::Null).expect("before");
		mock.sequence_set_format(json!({ "fps": 25, "name": "Renamed" }))
			.expect("set format");
		let after = mock.project_get_overview(&Value::Null).expect("after");
		assert_eq!(
			before, after,
			"a v2 edit must not change the frozen overview payload"
		);
	}
}

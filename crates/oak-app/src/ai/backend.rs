//! Engine backend for the agent loop (T8.1).
//!
//! The loop needs an [`EngineBackend`] (the frozen 22-tool face of
//! `oak-ai-tools`). The lead ruled **scheme B** on 2026-11-11: a thin
//! adapter over the `HostServices` **trait** — never over the concrete
//! `OppHost` — so the OPP implementation stays a replaceable part and the
//! coupling lives at the assembly point only.
//!
//! * [`MockBackend`] (shipped by `oak-ai-tools`) backs every test today;
//! * [`HostAdapter`] is the real-engine path: JSON in → typed
//!   `HostServices` call → JSON out, `HostError` → `ToolError`.
//!
//! ## Why scheme B (evidence)
//! `comm` over `fn` names of `ai/oak-ai-tools/src/backend.rs` vs
//! `crates/oak-app/src/opp/host.rs` → **21/24 shared**; backend-only:
//! `project_get_overview`, `time_rate`, `rational_at` (the last has a
//! default body, so only two need writing — both are implemented below
//! from `HostServices::project_get_info` / `timeline_get_structure`).
//! Per-method conversion is ~5 lines; scheme A would have re-implemented
//! ~700 lines of already-accepted engine glue a second time.
//!
//! ## Known mapping gaps (honest list, verified only against mocks)
//! Tools whose frozen schema and `HostServices` signature sit at different
//! semantic layers return a structured `UNSUPPORTED` instead of guessing:
//! `render.get_audio_levels` (schema `{sequence,range,resolution}` vs host
//! `{clip,buckets}`) and `render.get_thumbnails` (schema `range` vs host
//! `clips`). Parameters the host signature does not carry (`project.open`
//! `fps`/`name`, `timeline.add_track` `sequence`/`index`, …) are dropped
//! exactly as the existing OPP dispatch path drops them — never silently
//! reinterpreted. Field-level behaviour against the real engine is
//! **untested** until the real-engine task runs it.

pub use oak_ai_tools::backend::{EngineBackend, EngineExt};
use oak_ai_tools::error::{ToolError, CODE_BACKEND, CODE_INVALID_PARAMS, CODE_NOT_FOUND, CODE_STATE, CODE_UNSUPPORTED};
pub use oak_ai_tools::mock::MockBackend;
use oak_ai_tools::timecode::{Rat, TimeContext};
use oak_ai_tools::txn::TxnOps;
use oak_extensions::host_services::{
	AddEffectAtReq, AddEffectReq, AddMarkerReq, AddTrackReq, AddTransitionPlacedReq,
	AddTransitionReq, HostError, HostServices, MoveClipReq, PlaceClipReq, SetParamReq,
	SplitClipReq, TransitionSide, TrimClipReq,
};
use serde::Serialize;
use serde_json::{json, Value};

/// Real-engine backend: adapts any [`HostServices`] implementation to the
/// tool face. Generic over the trait on purpose (lead's constraint): the
/// agent must not name `OppHost`.
pub struct HostAdapter<H: HostServices> {
	host: H,
	/// The open undo group, if any (see [`TxnOps`]).
	txn: Option<String>,
}

impl<H: HostServices> HostAdapter<H> {
	pub fn new(host: H) -> Self {
		Self { host, txn: None }
	}

	/// The wrapped host, for the assembly point that owns it.
	pub fn host(&self) -> &H {
		&self.host
	}

	fn txn_id(&self) -> Result<&str, ToolError> {
		self.txn
			.as_deref()
			.ok_or_else(|| ToolError::new(CODE_BACKEND, "no transaction is open"))
	}
}

/// `HostError` (JSON-RPC numbers) → the tool face's stable string codes.
fn map_host(error: HostError) -> ToolError {
	let code = match error.code {
		-32601 => CODE_UNSUPPORTED,
		-32602 => CODE_INVALID_PARAMS,
		-32003 => CODE_NOT_FOUND,
		-32008 => CODE_STATE,
		_ => CODE_BACKEND,
	};
	ToolError::new(code, error.message)
}

fn owned<T: Serialize>(value: &T) -> Result<Value, ToolError> {
	serde_json::to_value(value).map_err(|e| ToolError::new(CODE_BACKEND, e.to_string()))
}

/// Required string argument.
fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
	args.get(key)
		.and_then(Value::as_str)
		.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("missing string field '{key}'")))
}

fn opt_arg(args: &Value, key: &str) -> Option<String> {
	args.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Required rational argument (`{"num":..,"den":..}` or `"n/d"`),
/// returned as the **host's** `Rational` (what every `*Req` field wants);
/// `Rat::new` still performs the `den > 0` validation first.
fn rat_arg(args: &Value, key: &str) -> Result<oak_extensions::host_services::Rational, ToolError> {
	let value = args
		.get(key)
		.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("missing field '{key}'")))?;
	if let Some(obj) = value.as_object() {
		let num = obj
			.get("num")
			.and_then(Value::as_i64)
			.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("'{key}.num' not an integer")))?;
		let den = obj
			.get("den")
			.and_then(Value::as_i64)
			.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("'{key}.den' not an integer")))?;
		let rat = Rat::new(num, den)?;
		return Ok(oak_extensions::host_services::Rational::new(rat.num, rat.den));
	}
	if let Some(text) = value.as_str() {
		let (num, den) = text
			.split_once('/')
			.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("'{key}' not n/d")))?;
		let num: i64 = num
			.trim()
			.parse()
			.map_err(|_| ToolError::new(CODE_INVALID_PARAMS, format!("'{key}.num' not an integer")))?;
		let den: i64 = den
			.trim()
			.parse()
			.map_err(|_| ToolError::new(CODE_INVALID_PARAMS, format!("'{key}.den' not an integer")))?;
		let rat = Rat::new(num, den)?;
		return Ok(oak_extensions::host_services::Rational::new(rat.num, rat.den));
	}
	Err(ToolError::new(
		CODE_INVALID_PARAMS,
		format!("'{key}' is not a rational"),
	))
}

fn unsupported(reason: &str) -> ToolError {
	ToolError::new(CODE_UNSUPPORTED, reason)
}

impl<H: HostServices> TxnOps for HostAdapter<H> {
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
		let id = self.host.edit_begin(label).map_err(map_host)?;
		self.txn = Some(id);
		Ok(())
	}

	fn txn_commit(&mut self) -> Result<(), ToolError> {
		let id = self
			.txn
			.take()
			.ok_or_else(|| ToolError::new(CODE_BACKEND, "no transaction to commit"))?;
		self.host.edit_commit(&id).map_err(map_host)
	}

	fn txn_abort(&mut self) -> Result<(), ToolError> {
		let id = self
			.txn
			.take()
			.ok_or_else(|| ToolError::new(CODE_BACKEND, "no transaction to abort"))?;
		self.host.edit_abort(&id).map_err(map_host)
	}
}

impl<H: HostServices> EngineBackend for HostAdapter<H> {
	// --- time grid ----------------------------------------------------

	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError> {
		match ctx {
			TimeContext::Footage(path) => {
				let probe = self.host.media_probe(&path).map_err(map_host)?;
				rational_to_rat(&probe.fps)
			}
			TimeContext::Sequence(id)
			| TimeContext::Clip(id)
			| TimeContext::Track(id) => {
				// fps lives on the sequence; resolve it through the project
				// info (TimelineStructure carries no rate).
				let info = self.host.project_get_info().map_err(map_host)?;
				let wanted = if id.is_empty() {
					None
				} else {
					Some(id.as_str())
				};
				let sequence = info
					.sequences
					.iter()
					.find(|s| wanted.map_or(true, |w| s.id == w))
					.ok_or_else(|| {
						ToolError::new(
							CODE_NOT_FOUND,
							format!("no sequence for time context {id:?}"),
						)
					})?;
				rational_to_rat(&sequence.fps)
			}
		}
	}

	// --- project -------------------------------------------------------

	fn project_open(&mut self, args: &Value) -> Result<Value, ToolError> {
		// `fps`/`name` are not on the HostServices signature; the existing
		// OPP dispatch path drops them the same way (documented gap).
		let path = opt_arg(args, "path").unwrap_or_default();
		let info = self
			.host
			.project_open(&path, self.txn.as_deref())
			.map_err(map_host)?;
		owned(&info)
	}

	fn project_save(&mut self, args: &Value) -> Result<Value, ToolError> {
		let path = opt_arg(args, "path");
		self.host
			.project_save(path.as_deref(), self.txn.as_deref())
			.map_err(map_host)?;
		Ok(Value::Null)
	}

	fn project_get_overview(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let info = self.host.project_get_info().map_err(map_host)?;
		owned(&info)
	}

	// --- media ---------------------------------------------------------

	fn media_probe(&mut self, args: &Value) -> Result<Value, ToolError> {
		let probe = self.host.media_probe(arg(args, "path")?).map_err(map_host)?;
		owned(&probe)
	}

	fn media_import_footage(&mut self, args: &Value) -> Result<Value, ToolError> {
		// The schema carries `paths` (array); the host takes one path per
		// call — import them all inside the open undo group.
		let paths = args
			.get("paths")
			.and_then(Value::as_array)
			.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, "missing 'paths' array"))?;
		let mut ids = Vec::new();
		for path in paths {
			let path = path
				.as_str()
				.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, "'paths' entries must be strings"))?;
			ids.push(
				self.host
					.media_import_footage(path, self.txn.as_deref())
					.map_err(map_host)?,
			);
		}
		Ok(json!({ "ids": ids, "imported": ids.len() }))
	}

	fn media_list_footage(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let list = self.host.media_list_footage().map_err(map_host)?;
		owned(&list)
	}

	// --- timeline ------------------------------------------------------

	fn timeline_add_track(&mut self, args: &Value) -> Result<Value, ToolError> {
		// schema `type` → host `kind`; `sequence`/`index` are not on the
		// signature (same drop as the OPP path).
		let req = AddTrackReq {
			kind: arg(args, "type")?.to_string(),
			name: opt_arg(args, "name"),
		};
		let id = self
			.host
			.timeline_add_track(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(json!(id))
	}

	fn timeline_place_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = PlaceClipReq {
			footage: arg(args, "footage")?.to_string(),
			track: arg(args, "track")?.to_string(),
			at: rat_arg(args, "in")?,
		};
		let id = self
			.host
			.timeline_place_clip(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(json!(id))
	}

	fn timeline_split_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = SplitClipReq {
			clip: arg(args, "clip")?.to_string(),
			at: rat_arg(args, "time")?,
		};
		let id = self
			.host
			.timeline_split_clip(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(json!(id))
	}

	fn timeline_trim_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let side = arg(args, "side")?;
		let time = rat_arg(args, "time")?;
		let (in_point, out_point) = match side {
			"in" => (Some(time), None),
			"out" => (None, Some(time)),
			other => {
				return Err(ToolError::new(
					CODE_INVALID_PARAMS,
					format!("unknown trim side '{other}'"),
				))
			}
		};
		let req = TrimClipReq {
			clip: arg(args, "clip")?.to_string(),
			in_point,
			out_point,
		};
		self.host
			.timeline_trim_clip(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(Value::Null)
	}

	fn timeline_move_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = MoveClipReq {
			clip: arg(args, "clip")?.to_string(),
			to_track: arg(args, "track")?.to_string(),
			to_at: rat_arg(args, "in")?,
		};
		self.host
			.timeline_move_clip(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(Value::Null)
	}

	fn timeline_ripple_delete(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = oak_extensions::host_services::RippleDeleteReq {
			clip: arg(args, "clip")?.to_string(),
		};
		self.host
			.timeline_ripple_delete(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(Value::Null)
	}

	fn timeline_add_transition(&mut self, args: &Value) -> Result<Value, ToolError> {
		let side = match arg(args, "side")? {
			"in" => TransitionSide::In,
			"out" => TransitionSide::Out,
			other => {
				return Err(ToolError::invalid_params(format!(
					"side must be 'in' or 'out', got '{other}'"
				)))
			}
		};
		let req = AddTransitionPlacedReq {
			clip: arg(args, "clip")?.to_string(),
			// schema `type` → host `kind` (the engine shader id).
			kind: arg(args, "type")?.to_string(),
			// The schema's duration is the **total**; the host splits it for a
			// seam placement and reports the half it actually used.
			duration: rat_arg(args, "duration")?,
			side,
		};
		let placed = self
			.host
			.timeline_add_transition_placed(req, self.txn.as_deref())
			.map_err(map_host)?;
		// Report where it landed: an edge and a seam transition are not the
		// same edit, and the caller must never have to guess which it got.
		Ok(json!({
			"id": placed.id,
			"placement": placed.placement,
			"half": placed.half,
		}))
	}

	fn timeline_add_marker(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = AddMarkerReq {
			at: rat_arg(args, "time")?,
			name: opt_arg(args, "name"),
		};
		let id = self
			.host
			.timeline_add_marker(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(json!(id))
	}

	// --- nodes ---------------------------------------------------------

	fn node_add_effect(&mut self, args: &Value) -> Result<Value, ToolError> {
		let index = match args.get("index") {
			None | Some(Value::Null) => None,
			Some(v) => Some(v.as_u64().ok_or_else(|| {
				ToolError::invalid_params("field 'index' must be an integer >= 0")
			})? as usize),
		};
		let req = AddEffectAtReq {
			target: arg(args, "clip")?.to_string(),
			// schema `effect` → host `type_id`.
			type_id: arg(args, "effect")?.to_string(),
			index,
			params: args.get("params").cloned().unwrap_or(Value::Null),
		};
		let id = self
			.host
			.node_add_effect_at(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(json!(id))
	}

	fn node_set_param(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = SetParamReq {
			node: arg(args, "node")?.to_string(),
			// schema `key` → host `param`.
			param: arg(args, "key")?.to_string(),
			value: args
				.get("value")
				.cloned()
				.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, "missing 'value'"))?,
		};
		self.host
			.node_set_param(req, self.txn.as_deref())
			.map_err(map_host)?;
		Ok(Value::Null)
	}

	fn node_list_types(&mut self, _args: &Value) -> Result<Value, ToolError> {
		// The host takes no filter; hand the model the full list (schema
		// `category`/`node` are advisory client-side filters).
		let list = self.host.node_list_types().map_err(map_host)?;
		owned(&list)
	}

	// --- render --------------------------------------------------------

	fn render_get_frame(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = oak_extensions::host_services::FrameRequest {
			sequence: opt_arg(args, "sequence"),
			footage: opt_arg(args, "footage"),
			time: rat_arg(args, "time")?,
			// Schema has no width/height; host requires them — use the
			// documented default square and let `max_size` scale it.
			width: 320,
			height: 180,
			max_size: args.get("max_size").and_then(Value::as_u64).map(|v| v as usize),
		};
		let frame = self.host.render_get_frame(req).map_err(map_host)?;
		owned(&frame)
	}

	fn render_get_thumbnails(&mut self, _args: &Value) -> Result<Value, ToolError> {
		// Schema `range` (start/end times) vs host `clips` (ids): different
		// semantic layers — refuse rather than guess.
		Err(unsupported(
			"render.get_thumbnails: schema range-based vs host clip-id based; mapping deferred to the real-engine task",
		))
	}

	fn render_get_audio_levels(&mut self, _args: &Value) -> Result<Value, ToolError> {
		// Schema `{sequence,range,resolution}` vs host `{clip,buckets}`:
		// refuse rather than guess.
		Err(unsupported(
			"render.get_audio_levels: schema sequence/range vs host clip/buckets; mapping deferred to the real-engine task",
		))
	}

	// --- export / edit -------------------------------------------------

	fn export_start(&mut self, args: &Value) -> Result<Value, ToolError> {
		let req = oak_extensions::host_services::ExportRequest {
			sequence: arg(args, "sequence")?.to_string(),
			// schema `output_path` → host `path`; schema `preset` is
			// optional while the host field is a plain String.
			path: arg(args, "output_path")?.to_string(),
			preset: opt_arg(args, "preset").unwrap_or_default(),
			range: None,
		};
		let job = self.host.export_start(req, self.txn.as_deref()).map_err(map_host)?;
		owned(&job)
	}

	fn edit_undo(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let undone = self.host.edit_undo().map_err(map_host)?;
		Ok(json!({ "undone": undone }))
	}
}

// ---------------------------------------------------------------------------
// v2 extension face (T9.2): `sequence.*` over the same `HostServices` seam
// ---------------------------------------------------------------------------

/// The v2 face for the real engine: JSON in → typed `HostServices` call →
/// JSON out (the same shape as the frozen 22 above, so the two faces stay
/// interchangeable at the dispatch layer).
impl<H: HostServices> EngineExt for HostAdapter<H> {
	fn sequence_get_settings(&self, args: Value) -> Result<Value, ToolError> {
		let sequence = args
			.get("sequence")
			.and_then(Value::as_str)
			.map(str::to_string);
		let settings = self
			.host
			.sequence_get_settings(sequence.as_ref())
			.map_err(map_host)?;
		Ok(serde_json::to_value(&settings)
			.map_err(|e| ToolError::new(oak_ai_tools::error::CODE_INTERNAL, format!("sequence settings encode: {e}")))?)
	}

	fn sequence_set_format(&mut self, args: Value) -> Result<Value, ToolError> {
		let parsed = oak_ai_tools::tools::v2::sequence::parse_set_format(&args)?;
		let req = oak_extensions::host_services::SetSequenceFormatReq {
			sequence: parsed.sequence,
			name: parsed.name,
			width: parsed.width,
			height: parsed.height,
			rate: parsed
				.fps
				.map(|r| oak_extensions::host_services::Rational::new(r.num, r.den)),
			interlaced: parsed.interlaced,
		};
		let settings = self
			.host
			.sequence_set_format(req, self.txn.as_deref())
			.map_err(map_host)?;
		let mut value = serde_json::to_value(&settings)
			.map_err(|e| ToolError::new(oak_ai_tools::error::CODE_INTERNAL, format!("sequence settings encode: {e}")))?;
		if let Some(obj) = value.as_object_mut() {
			// The AI reply must say the edit can be taken back (首辅 requirement).
			obj.insert("undoable".to_string(), Value::Bool(true));
		}
		Ok(value)
	}
}

fn rational_to_rat(value: &oak_extensions::host_services::Rational) -> Result<Rat, ToolError> {
	Rat::new(value.num, value.den)
}

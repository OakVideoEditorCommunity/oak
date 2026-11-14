//! [`EngineBackend`] for the real engine: the 22 frozen tools on top of
//! `oak-engine-rust-sdk`.
//!
//! Every method here follows the same three-step shape:
//!
//! 1. **parse** the frozen arguments with [`crate::tools`] (the same
//!    parsers the mock uses, so a malformed call dies identically);
//! 2. **resolve** the tool-face ids against the live document and validate
//!    the edit itself *before* touching the engine, so a rejected edit
//!    changes nothing (the dispatcher's abort is the second line of defence,
//!    not the only one);
//! 3. **apply** through the SDK and **read the result back** from the
//!    document — responses carry what the engine actually holds, never what
//!    we asked it to hold.
//!
//! Where the engine has no counterpart for a frozen tool the method returns
//! [`ToolError::unsupported`] with a gap id from [`super::gaps`].

use oak_engine_rust_sdk::codec::{Codec, Format, decoder_probe};
use oak_engine_rust_sdk::effect::{
	NODE_FLAG_AUDIO_EFFECT, NODE_FLAG_DONT_SHOW_IN_CREATE_MENU, NODE_FLAG_VIDEO_EFFECT,
	dynamic_factory_entries, factory_entries,
};
use oak_engine_rust_sdk::node::{
	NodeId, NodeKind, NodeValue, Project, undo, undo_can_undo, undo_command_name, undo_count,
	undo_index,
};
use oak_engine_rust_sdk::render::{
	RenderBackend, manager_init, manager_is_initialized, render_frame_f32,
};
use oak_engine_rust_sdk::task::{self, TaskState};
use oak_engine_rust_sdk::timeline;
use oak_engine_rust_sdk::types::PixelFormat;
use serde_json::{Value, json};

use crate::backend::EngineBackend;
use crate::error::{CODE_BACKEND, CODE_INTERNAL, CODE_IO, CODE_NOTHING_TO_UNDO, CODE_STATE, ToolError};
use crate::render::frame_summary;
use crate::timecode::{Rat, TimeContext};
use crate::tools;
use crate::tools::render_tools::FrameTarget;
use crate::tools::timeline::{Side, TransitionType};

use super::{
	GAP_NODE_INPUT_ENUM, GAP_NODE_STRING_PARAM, GAP_RENDER_AUDIO_SAMPLES, GAP_TIMELINE_MARKERS,
	GAP_TRACK_INSERT_INDEX, GAP_TRANSITION_SEAM, SdkBackend,
};

/// Default frame size when `max_size` is omitted (16:9, like the mock).
const DEFAULT_FRAME_SIZE: (u32, u32) = (320, 180);
/// Default thumbnail height when `height` is omitted.
const DEFAULT_THUMB_HEIGHT: u32 = 90;
/// The engine's `TrackType` codes (`OakNodeTrackType`).
const TRACK_TYPE_VIDEO: i32 = 0;
const TRACK_TYPE_AUDIO: i32 = 1;
/// The clip block's texture input (engine `block::clip_input::TEXTURE_INPUT`).
const CLIP_TEXTURE_INPUT: &str = "tex_in";
/// The zero rational, spelled once.
fn zero() -> Rat {
	Rat { num: 0, den: 1 }
}

impl SdkBackend {
	/// An owned project handle (refcounted clone), so a method can keep
	/// calling `&mut self` helpers while it reads the document without
	/// fighting the borrow checker.
	fn handle(&self) -> Result<Project, ToolError> {
		self.project().map(|project| project.clone())
	}

	/// Resolves a footage argument.
	fn footage(&mut self, id: &str) -> Result<NodeId, ToolError> {
		self.require_node(id, NodeKind::Footage, "footage")
	}

	/// Resolves a clip argument (mounted on a track).
	fn clip(&mut self, id: &str) -> Result<NodeId, ToolError> {
		self.require_node(id, NodeKind::Clip, "clip")
	}

	/// Locates a track id as `(sequence, track, track list, index in list)`.
	fn locate_track(&mut self, id: &str) -> Result<(NodeId, NodeId, NodeId, usize), ToolError> {
		let track = self.require_node(id, NodeKind::Track, "track")?;
		for sequence in self.sequence_nodes()? {
			for list in self.track_lists(sequence)? {
				for (index, current) in self.tracks_of(list)?.into_iter().enumerate() {
					if current == track {
						return Ok((sequence, track, list, index));
					}
				}
			}
		}
		Err(ToolError::not_found(format!("unknown track '{id}'")))
	}

	/// The append position of a track of `kind` in `sequence` (`0` when the
	/// sequence has no list of that media kind yet).
	fn track_append_position(project: &Project, sequence: NodeId, kind: i32) -> usize {
		let mut position = 0;
		for index in 0..project.sequence_track_list_count(sequence) {
			let list = project.sequence_track_list_at(sequence, index);
			if list.is_valid() && project.tracklist_kind(list) == kind {
				position = project.tracklist_track_count(list);
			}
		}
		position
	}

	/// Fails when `[start, start + length)` intersects any other block of
	/// `track`.
	fn reject_overlap(
		project: &Project,
		track: NodeId,
		track_id: &str,
		start: Rat,
		length: Rat,
		exclude: Option<NodeId>,
	) -> Result<(), ToolError> {
		let end = start.checked_add(length)?;
		for block in Self::blocks(project, track) {
			if Some(block) == exclude {
				continue;
			}
			// A gap is empty space, not an occupied interval: placing over it
			// is what the engine's placement does anyway (it removes the
			// covered area, gaps included).
			if project.node_kind(block) == Some(NodeKind::Gap) {
				continue;
			}
			let block_in = SdkBackend::rat(project.block_in(block), "block in")?;
			let block_out = SdkBackend::rat(project.block_out(block), "block out")?;
			if start < block_out && block_in < end {
				return Err(ToolError::invalid_params(format!(
					"placing [{start}, {end}) would overlap a block on track {track_id} \
					 ([{block_in}, {block_out}))"
				)));
			}
		}
		Ok(())
	}

	/// Blocks of a track of an owned handle (borrow-friendly twin of
	/// `blocks_of`).
	fn blocks(project: &Project, track: NodeId) -> Vec<NodeId> {
		(0..project.track_block_count(track))
			.map(|index| project.track_block_at(track, index))
			.filter(|node| node.is_valid())
			.collect()
	}

	/// The total (or video) duration of a footage node.
	fn footage_duration(project: &Project, footage: NodeId) -> Result<Rat, ToolError> {
		let total = project.footage_duration(footage);
		let total = if total.den == 0 {
			project.footage_video_length(footage)
		} else {
			total
		};
		SdkBackend::rat(total, "footage duration")
	}
}

impl EngineBackend for SdkBackend {
	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError> {
		self.rate_of(&ctx)
	}

	// --- project -------------------------------------------------------

	fn project_open(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::project::parse_open(args)?;
		// The engine's history is process-wide; opening must not leave rows
		// referring to a project this backend is about to drop.
		let _ = oak_engine_rust_sdk::node::undo_clear();

		let (project, created) = match parsed.path.as_deref() {
			Some(path) => {
				let project = Project::load(path)
					.ok_or_else(|| ToolError::not_found(format!("cannot open project '{path}'")))?;
				(project, false)
			}
			None => (Project::new(), true),
		};

		// A different project means a different id space.
		self.names.clear();
		self.nodes.clear();
		self.clip_source.clear();
		self.counters = super::Counters::default();
		self.project = Some(project);

		let fallback_name = parsed
			.name
			.clone()
			.or_else(|| parsed.path.as_deref().map(tools::project::file_stem))
			.unwrap_or_else(|| "Untitled".to_string());
		if created {
			let project = self.handle()?;
			let sequence = project
				.create_sequence(&fallback_name)
				.ok_or_else(|| ToolError::backend("the engine could not create the default sequence"))?;
			if let Some(fps) = parsed.fps {
				let (width, height) = project.sequence_geometry(sequence).unwrap_or((1920, 1080));
				let num = i32::try_from(fps.num)
					.map_err(|_| ToolError::invalid_params("fps numerator out of range"))?;
				let den = i32::try_from(fps.den)
					.map_err(|_| ToolError::invalid_params("fps denominator out of range"))?;
				if !project.sequence_set_video_params(sequence, width, height, num, den) {
					return Err(ToolError::backend(format!(
						"the engine rejected the sequence frame rate {fps}"
					)));
				}
			}
		}

		// Initialisation order (audited): the render manager first (the
		// inline/threads backend needs no oak-worker binary), then the
		// process-wide active-project slot so observers and the host see the
		// same document this backend edits.
		if !manager_is_initialized() && !manager_init(RenderBackend::Threads) {
			return Err(ToolError::backend(
				"the engine's render manager could not be initialized (inline backend)",
			));
		}
		let project = self.handle()?;
		oak_engine_rust_sdk::node::set_active_project(Some(&project));

		let sequence = self.active_sequence()?;
		self.name_of(sequence, "sequence");
		let fps = SdkBackend::rat(project.sequence_frame_rate(sequence), "sequence frame rate")?;
		let engine_name = project.name();
		let name = if engine_name.is_empty() {
			fallback_name
		} else {
			engine_name
		};
		let filename = project.filename();
		Ok(json!({
			"project": {
				"name": name,
				"path": (!filename.is_empty()).then_some(filename),
				"fps": fps.to_json(),
			},
			"created": created,
			"backend": "sdk",
		}))
	}

	fn project_save(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::project::parse_save(args)?;
		let project = self.handle()?;
		let path = parsed
			.path
			.clone()
			.or_else(|| {
				let filename = project.filename();
				(!filename.is_empty()).then_some(filename)
			})
			.ok_or_else(|| {
				ToolError::invalid_params("no 'path' argument and the project has no file path yet")
			})?;
		if !project.save(Some(&path)) {
			return Err(ToolError::backend(format!(
				"the engine could not save the project to '{path}'"
			)));
		}
		let bytes = std::fs::metadata(&path).map(|meta| meta.len()).map_err(|err| {
			ToolError::new(
				CODE_IO,
				format!("the project was saved but '{path}' cannot be read back: {err}"),
			)
		})?;
		Ok(json!({
			"path": path,
			"saved": true,
			"bytes": bytes,
			"backend": "sdk",
		}))
	}

	fn project_get_overview(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let project = self.handle()?;
		let sequence = self.active_sequence()?;
		let footage = self.footage_json_list()?;
		let sequences = self.sequences_json()?;
		let sequence_id = self.name_of(sequence, "sequence");
		let fps = SdkBackend::rat(project.sequence_frame_rate(sequence), "sequence frame rate")?;
		let length = SdkBackend::rat(project.sequence_length(sequence), "sequence length")?;
		let geometry = project.sequence_geometry(sequence);
		let name = project.name();
		let filename = project.filename();
		Ok(json!({
			"project": {
				"name": name,
				"path": (!filename.is_empty()).then_some(filename),
				"fps": fps.to_json(),
			},
			"active_sequence": sequence_id,
			"sequence_geometry": geometry.map(|(width, height)| json!({
				"width": width,
				"height": height,
			})),
			"sequence_length": length.to_json(),
			"sequences": sequences,
			"footage": footage,
			"backend": "sdk",
		}))
	}

	// --- media ---------------------------------------------------------

	fn media_probe(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::media::parse_probe(args)?;
		let meta = std::fs::metadata(&parsed.path)
			.map_err(|err| ToolError::not_found(format!("cannot probe '{}': {err}", parsed.path)))?;
		let description = decoder_probe(None, &parsed.path, None).ok_or_else(|| {
			ToolError::backend(format!("no registered decoder could read '{}'", parsed.path))
		})?;
		let mut out = json!({
			"path": parsed.path,
			"exists": true,
			"bytes": meta.len(),
			"decoder": description.decoder_name(),
			"video_streams": description.video_stream_count() as u64,
			"audio_streams": description.audio_stream_count() as u64,
			"subtitle_streams": description.subtitle_stream_count() as u64,
			"width": Value::Null,
			"height": Value::Null,
			"fps": Value::Null,
			"duration": Value::Null,
			"synthetic": false,
			"backend": "sdk",
		});
		let video = description.video_stream(0);
		if let Some(video) = &video {
			out["width"] = json!(video.width);
			out["height"] = json!(video.height);
			let fps = Rat::new(i64::from(video.frame_rate_num), i64::from(video.frame_rate_den))
				.map_err(|err| {
					ToolError::backend(format!("the probe reported a bad frame rate: {err}"))
				})?;
			out["fps"] = fps.to_json();
		}
		// The container duration is what the probe reports when the format
		// carries one; some containers (and the ffmpeg probe on a bare
		// stream) do not, in which case the video stream's own
		// `duration × time base` is the honest fallback. `null` means neither
		// was available — never a guessed number.
		if let Some(range) = description.duration() {
			let in_ = SdkBackend::rat(range.in_, "probed duration in")?;
			let out_at = SdkBackend::rat(range.out, "probed duration out")?;
			out["duration"] = out_at.checked_sub(in_)?.to_json();
			out["duration_source"] = json!("container");
		} else if let Some(video) = &video {
			if video.duration > 0 && video.time_base_num > 0 && video.time_base_den != 0 {
				let ticks = video
					.duration
					.checked_mul(i64::from(video.time_base_num))
					.ok_or_else(|| ToolError::backend("probed duration overflows"))?;
				let duration =
					Rat::new(ticks, i64::from(video.time_base_den)).map_err(|err| {
						ToolError::backend(format!("the probe reported a bad duration: {err}"))
					})?;
				out["duration"] = duration.to_json();
				out["duration_source"] = json!("video stream time base");
			}
		}
		Ok(out)
	}

	fn media_import_footage(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::media::parse_import(args)?;
		let project = self.handle()?;
		let mut imported = Vec::new();
		let mut failed = Vec::new();
		for path in &parsed.paths {
			match project.import_footage(path) {
				Some(node) => {
					let id = self.mint("footage", node);
					imported.push(self.footage_json(&id)?);
				}
				None => failed.push(json!({
					"path": path,
					"error": format!("the engine could not import '{path}'"),
				})),
			}
		}
		Ok(json!({
			"footage": imported,
			"failed": failed,
			"backend": "sdk",
		}))
	}

	fn media_list_footage(&mut self, _args: &Value) -> Result<Value, ToolError> {
		let footage = self.footage_json_list()?;
		Ok(json!({ "footage": footage, "backend": "sdk" }))
	}

	// --- timeline ------------------------------------------------------

	fn timeline_add_track(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_add_track(args)?;
		let project = self.handle()?;
		let sequence = self.resolve_sequence(parsed.sequence.as_deref())?;
		let kind = if parsed.track_type == "video" {
			TRACK_TYPE_VIDEO
		} else {
			TRACK_TYPE_AUDIO
		};
		let append_at = Self::track_append_position(&project, sequence, kind);
		if let Some(index) = parsed.index {
			if index != append_at {
				return Err(ToolError::unsupported(
					GAP_TRACK_INSERT_INDEX,
					format!(
						"track insertion at index {index} is not available: the SDK's \
						 oak_track_add appends to the sequence's {} track list (the append \
						 position is {append_at})",
						parsed.track_type
					),
				));
			}
		}
		let track = project.add_track(sequence, kind).ok_or_else(|| {
			ToolError::backend(format!(
				"the engine could not add a {} track to the sequence",
				parsed.track_type
			))
		})?;
		let track_id = self.mint("track", track);
		let sequence_id = self.name_of(sequence, "sequence");
		Ok(json!({
			"track": track_id,
			"sequence": sequence_id,
			"index": append_at,
			"type": parsed.track_type,
			"backend": "sdk",
		}))
	}

	fn timeline_place_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_place_clip(args)?;
		let project = self.handle()?;
		let (_, track, list, track_index) = self.locate_track(&parsed.track)?;
		let footage = self.footage(&parsed.footage)?;
		let (in_, in_sdk) =
			self.resolve_time(TimeContext::Track(parsed.track.clone()), parsed.in_)?;
		let media_in = match parsed.media_in {
			Some(value) => {
				self.resolve_time(TimeContext::Footage(parsed.footage.clone()), value)?
					.0
			}
			None => zero(),
		};
		let duration = Self::footage_duration(&project, footage)?;
		if media_in >= duration {
			return Err(ToolError::invalid_params(format!(
				"'media_in' {media_in} is at/past the end of the footage ({duration}s)"
			)));
		}
		// The frozen face has no 'out': the clip runs to the end of the
		// footage and is shortened afterwards with timeline.trim_clip.
		let length = duration.checked_sub(media_in)?;
		Self::reject_overlap(&project, track, &parsed.track, in_, length, None)?;

		// Validate first, mutate second: only now does the document change.
		let clip = project
			.create_clip()
			.ok_or_else(|| ToolError::backend("the engine could not create a clip block"))?;
		if !project.connect(footage, clip, CLIP_TEXTURE_INPUT, -1) {
			return Err(ToolError::backend(format!(
				"the engine refused to connect footage '{}' to the new clip",
				parsed.footage
			)));
		}
		if !project.block_set_length(clip, SdkBackend::sdk_rat(length)) {
			return Err(ToolError::backend("the engine refused to set the clip length"));
		}
		if media_in != zero() && !project.block_set_media_in(clip, SdkBackend::sdk_rat(media_in)) {
			return Err(ToolError::backend("the engine refused to set the clip media in"));
		}
		if !timeline::block_place(&project, list, track_index as i32, clip, in_sdk) {
			return Err(ToolError::backend(format!(
				"the engine refused to place the clip at {in_} on track '{}'",
				parsed.track
			)));
		}
		let clip_id = self.mint("clip", clip);
		self.remember_clip_source(&clip_id, &parsed.footage);
		Ok(json!({
			"clip": clip_id,
			"track": parsed.track,
			"in": SdkBackend::rat(project.block_in(clip), "block in")?.to_json(),
			"out": SdkBackend::rat(project.block_out(clip), "block out")?.to_json(),
			"length": SdkBackend::rat(project.block_length(clip), "block length")?.to_json(),
			"media_in": SdkBackend::rat(project.block_media_in(clip), "block media in")?.to_json(),
			"backend": "sdk",
		}))
	}

	fn timeline_split_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_split(args)?;
		let project = self.handle()?;
		let clip = self.clip(&parsed.clip)?;
		let (_, track, _, _) = self.locate_block(clip)?;
		let (at, at_sdk) = self.resolve_time(TimeContext::Clip(parsed.clip.clone()), parsed.time)?;
		let in_ = SdkBackend::rat(project.block_in(clip), "block in")?;
		let out = SdkBackend::rat(project.block_out(clip), "block out")?;
		if !(in_ < at && at < out) {
			return Err(ToolError::invalid_params(format!(
				"split point {at} is outside the clip [{in_}, {out})"
			)));
		}
		let before = Self::blocks(&project, track);
		if !timeline::block_split(&project, clip, at_sdk) {
			return Err(ToolError::backend(format!(
				"the engine refused to split clip '{}' at {at}",
				parsed.clip
			)));
		}
		let after = Self::blocks(&project, track);
		let created: Vec<NodeId> = after
			.into_iter()
			.filter(|node| !before.contains(node))
			.collect();
		let new_clip = match created.as_slice() {
			[one] => *one,
			[] => {
				return Err(ToolError::new(
					CODE_INTERNAL,
					"the engine reported a successful split but the track has no new block",
				));
			}
			many => {
				return Err(ToolError::new(
					CODE_INTERNAL,
					format!(
						"the engine's split created {} blocks; the tool face expects one",
						many.len()
					),
				));
			}
		};
		let new_id = self.mint("clip", new_clip);
		// The right half inherits the source media of the original.
		if let Some(source) = self.clip_source(&parsed.clip).map(str::to_string) {
			self.remember_clip_source(&new_id, &source);
		}
		Ok(json!({
			"clip": parsed.clip,
			"new_clip": new_id,
			"time": SdkBackend::rat(project.block_in(new_clip), "block in")?.to_json(),
			"backend": "sdk",
		}))
	}

	fn timeline_trim_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_trim(args)?;
		let project = self.handle()?;
		let clip = self.clip(&parsed.clip)?;
		let (_, track, _, _) = self.locate_block(clip)?;
		let (at, _) = self.resolve_time(TimeContext::Clip(parsed.clip.clone()), parsed.time)?;
		let old_in = SdkBackend::rat(project.block_in(clip), "block in")?;
		let old_out = SdkBackend::rat(project.block_out(clip), "block out")?;
		let media_in = SdkBackend::rat(project.block_media_in(clip), "block media in")?;
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
		if new_length.is_zero() {
			return Err(ToolError::invalid_params("the trim would leave an empty clip"));
		}
		// The media may not run past the source, when this session knows the
		// source (see GAP_CLIP_SOURCE_QUERY for the loaded-project case).
		if let Some(source) = self.clip_source(&parsed.clip).map(str::to_string) {
			if let Some(node) = self.node_of(&source) {
				if let Ok(total) = Self::footage_duration(&project, node) {
					let media_end = new_media_in.checked_add(new_length)?;
					if media_end > total {
						return Err(ToolError::invalid_params(format!(
							"trim would run past the end of the footage (media {new_media_in} + \
							 length {new_length} > {total})"
						)));
					}
				}
			}
		}
		Self::reject_overlap(&project, track, &parsed.clip, new_in, new_length, Some(clip))?;

		// Engine trim semantics: a head trim anchors the timeline OUT point and
		// shifts the media in point with it (`BlockTrimCommand`, TrimIn); a tail
		// trim moves the out point and leaves the media untouched. The head trim
		// compensates the freed span with a gap block, which the tool face cannot
		// address (it reports such elements as `kind: "gap"`).
		let ok = match parsed.side {
			Side::In => timeline::block_trim(
				&project,
				track,
				clip,
				SdkBackend::sdk_rat(new_length),
				timeline::TrimMode::TrimIn,
			),
			Side::Out => project.block_set_length(clip, SdkBackend::sdk_rat(new_length)),
		};
		if !ok {
			return Err(ToolError::backend(format!(
				"the engine refused to trim clip '{}' ({}) to {new_length}",
				parsed.clip,
				parsed.side.as_str()
			)));
		}
		Ok(json!({
			"clip": parsed.clip,
			"side": parsed.side.as_str(),
			"in": SdkBackend::rat(project.block_in(clip), "block in")?.to_json(),
			"out": SdkBackend::rat(project.block_out(clip), "block out")?.to_json(),
			"length": SdkBackend::rat(project.block_length(clip), "block length")?.to_json(),
			"media_in": SdkBackend::rat(project.block_media_in(clip), "block media in")?.to_json(),
			"backend": "sdk",
		}))
	}

	fn timeline_move_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_move(args)?;
		let project = self.handle()?;
		let clip = self.clip(&parsed.clip)?;
		let (sequence, _, _, _) = self.locate_block(clip)?;
		let (list, track_index, track_id) = match parsed.track.as_deref() {
			Some(track_id) => {
				let (to_sequence, _, list, index) = self.locate_track(track_id)?;
				if to_sequence != sequence {
					return Err(ToolError::invalid_params(
						"moving across sequences is not supported",
					));
				}
				(list, index, track_id.to_string())
			}
			None => {
				let (_, track, list, index) = self.locate_block(clip)?;
				let track_id = self.name_of(track, "track");
				(list, index, track_id)
			}
		};
		let (at, at_sdk) = self.resolve_time(TimeContext::Clip(parsed.clip.clone()), parsed.in_)?;
		let length = SdkBackend::rat(project.block_length(clip), "block length")?;
		let destination_track = project.tracklist_track_at(list, track_index);
		if !destination_track.is_valid() {
			return Err(ToolError::not_found(format!(
				"track '{track_id}' is not part of the document"
			)));
		}
		Self::reject_overlap(
			&project,
			destination_track,
			&track_id,
			at,
			length,
			Some(clip),
		)?;
		if !timeline::block_move(&project, list, track_index as i32, clip, at_sdk) {
			return Err(ToolError::backend(format!(
				"the engine refused to move clip '{}' to {at} on track '{track_id}'",
				parsed.clip
			)));
		}
		Ok(json!({
			"clip": parsed.clip,
			"track": track_id,
			"in": SdkBackend::rat(project.block_in(clip), "block in")?.to_json(),
			"backend": "sdk",
		}))
	}

	fn timeline_ripple_delete(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::timeline::parse_ripple_delete(args)?;
		let project = self.handle()?;
		let clip = self.clip(&parsed.clip)?;
		let (_, track, list, track_index) = self.locate_block(clip)?;
		let length = SdkBackend::rat(project.block_length(clip), "block length")?;
		let before = Self::blocks(&project, track);
		let position = before
			.iter()
			.position(|block| *block == clip)
			.ok_or_else(|| ToolError::not_found(format!("unknown clip '{}'", parsed.clip)))?;
		let following: Vec<NodeId> = before[position + 1..].to_vec();

		// "the gap it leaves is closed" is not one engine call: the module
		// stores each block's in/out point (unlike Olive's contiguous track
		// order), so neither `track_ripple_remove_block` nor the area-based
		// `TimelineRippleRemoveAreaCommand` moves the later blocks — the area
		// command explicitly leaves a successor starting at/after the cleared
		// range untouched. So: unlink the clip, then shift every following
		// block of the same track left by the removed length with the
		// undoable move primitive (`TrackMoveBlockCommand` rewrites the stored
		// in point). Everything happens inside this call's undo group.
		if !project.track_ripple_remove_block(track, clip) {
			return Err(ToolError::backend(format!(
				"the engine refused to remove clip '{}'",
				parsed.clip
			)));
		}
		for block in following {
			if project.node_kind(block) == Some(NodeKind::Gap) {
				// A gap between the removed clip and its follower is a hole the
				// ripple closes too.
				let _ = project.track_ripple_remove_block(track, block);
				continue;
			}
			let old_in = SdkBackend::rat(project.block_in(block), "block in")?;
			let new_in = old_in.checked_sub(length)?;
			if !timeline::block_move(&project, list, track_index as i32, block, SdkBackend::sdk_rat(new_in))
			{
				return Err(ToolError::backend(format!(
					"the engine refused to shift a following clip from {old_in} to {new_in}"
				)));
			}
		}
		// `TrackMoveBlockCommand` fills the vacated slot with a gap; unlink the
		// gaps this call created so the track has no litter (still inside the
		// group, so undo restores them).
		for block in Self::blocks(&project, track) {
			if !before.contains(&block) && project.node_kind(block) == Some(NodeKind::Gap) {
				let _ = project.track_ripple_remove_block(track, block);
			}
		}
		// Read back: the clip must really be gone.
		if Self::blocks(&project, track).contains(&clip) {
			return Err(ToolError::backend(format!(
				"the engine's ripple left clip '{}' on its track",
				parsed.clip
			)));
		}
		Ok(json!({
			"clip": parsed.clip,
			"removed": true,
			"ripple": length.to_json(),
			"backend": "sdk",
		}))
	}

	fn timeline_add_transition(&mut self, _args: &Value) -> Result<Value, ToolError> {
		Err(ToolError::unsupported(
			GAP_TRANSITION_SEAM,
			"the frozen tool face addresses one clip edge plus a total duration, while an \
			 engine transition is a block spanning a seam between two blocks, with the \
			 duration split into in/out offsets and both neighbours connected; the face does \
			 not name the other side of the seam, so this backend cannot build a transition \
			 it can prove the renderer will honour",
		))
	}

	fn timeline_add_marker(&mut self, _args: &Value) -> Result<Value, ToolError> {
		Err(ToolError::unsupported(
			GAP_TIMELINE_MARKERS,
			"the engine exports no timeline marker API (no oak_timeline_marker_* symbol), so \
			 markers cannot be added, read back or persisted through the SDK",
		))
	}

	// --- nodes ---------------------------------------------------------

	fn node_add_effect(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_add_effect(args)?;
		let project = self.handle()?;
		let clip = self.clip(&parsed.clip)?;
		// Validate the type against the engine's own factory table: an unknown
		// id would be silently dropped by the bridge otherwise.
		let known = factory_entries()
			.into_iter()
			.any(|entry| entry.type_id == parsed.effect)
			|| dynamic_factory_entries()
				.into_iter()
				.any(|entry| entry.type_id == parsed.effect);
		if !known {
			return Err(ToolError::invalid_params(format!(
				"unknown effect type '{}' (it is not in the engine's node factory table; see \
				 node.list_types)",
				parsed.effect
			)));
		}
		let chain = project.effect_chain(clip);
		let index = match parsed.index {
			Some(index) if index <= chain.len() => index,
			Some(index) => {
				return Err(ToolError::invalid_params(format!(
					"index {index} is out of range (the chain has {} effects)",
					chain.len()
				)));
			}
			None => chain.len(),
		};
		// The bridge clamps to the ends; `usize::MAX` is its documented append.
		let insert_at = if index >= chain.len() {
			usize::MAX
		} else {
			index
		};
		let effect = project
			.effect_chain_insert(clip, insert_at, &parsed.effect)
			.ok_or_else(|| {
				ToolError::backend(format!(
					"the engine refused to add effect '{}' to clip '{}'",
					parsed.effect, parsed.clip
				))
			})?;
		let node_id = self.mint("node", effect);
		let index_now = project
			.effect_chain(clip)
			.iter()
			.position(|node| *node == effect)
			.unwrap_or(index);
		Ok(json!({
			"node": node_id,
			"clip": parsed.clip,
			"effect": parsed.effect,
			"index": index_now,
			"backend": "sdk",
		}))
	}

	fn node_set_param(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_set_param(args)?;
		tools::node::check_range(&parsed.key, &parsed.value)?;
		let project = self.handle()?;
		let node = self
			.node_of(&parsed.node)
			.ok_or_else(|| ToolError::not_found(format!("unknown node '{}'", parsed.node)))?;
		if !node.is_valid() {
			return Err(ToolError::not_found(format!("unknown node '{}'", parsed.node)));
		}
		let current = project
			.standard_value(node, &parsed.key, -1)
			.ok_or_else(|| {
				ToolError::not_found(format!(
					"node '{}' has no parameter '{}'",
					parsed.node, parsed.key
				))
			})?;
		let encoded = encode_node_value(&parsed.key, &parsed.value, current.kind, &parsed.node)?;
		if !project.set_standard_value(node, &parsed.key, -1, encoded) {
			return Err(ToolError::backend(format!(
				"the engine refused to set parameter '{}' on node '{}'",
				parsed.key, parsed.node
			)));
		}
		// Read back: a write the engine silently dropped must not look like a
		// success (plan section 5 iron rules).
		let stored = project
			.standard_value(node, &parsed.key, -1)
			.ok_or_else(|| ToolError::backend("the parameter disappeared after the write"))?;
		if !same_value(&encoded, &stored) {
			return Err(ToolError::invalid_params(format!(
				"parameter '{}' on node '{}' did not take the requested value: the parameter's \
				 declared type rejected {}",
				parsed.key, parsed.node, parsed.value
			)));
		}
		Ok(json!({
			"node": parsed.node,
			"key": parsed.key,
			"value": parsed.value,
			"backend": "sdk",
		}))
	}

	fn node_list_types(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::node::parse_list_types(args)?;
		if let Some(node) = parsed.node.as_deref() {
			return Err(ToolError::unsupported(
				GAP_NODE_INPUT_ENUM,
				format!(
					"cannot read back the live parameters of node '{node}': the SDK bridges \
					 single-input reads (standard_value per input id) but exports no input \
					 list, so a parameter inventory would have to be invented"
				),
			));
		}
		let category = parsed.category.as_deref().unwrap_or("all");
		let mut types = Vec::new();
		if category == "effect" || category == "all" {
			for entry in factory_entries() {
				let video = entry.flags & NODE_FLAG_VIDEO_EFFECT != 0;
				let audio = entry.flags & NODE_FLAG_AUDIO_EFFECT != 0;
				if !video && !audio {
					continue;
				}
				if entry.flags & NODE_FLAG_DONT_SHOW_IN_CREATE_MENU != 0 {
					continue;
				}
				types.push(json!({
					"type_id": entry.type_id,
					"name": entry.name,
					"category": "effect",
					"media": if video { "video" } else { "audio" },
					"source": "engine-factory",
				}));
			}
		}
		if category == "transition" || category == "all" {
			// The transition styles are part of the frozen tool face (the T0
			// schema enum), not factory rows: the engine models transitions as
			// timeline blocks, not as node types.
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
					"source": "frozen-face",
				}));
			}
		}
		Ok(json!({
			"types": types,
			"category": category,
			"backend": "sdk",
			"note": "effects come from the engine's node factory table; transition styles are \
			         the frozen T0 enum (the engine models transitions as timeline blocks)",
		}))
	}

	// --- render --------------------------------------------------------

	fn render_get_frame(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::render_tools::parse_get_frame(args)?;
		let project = self.handle()?;
		let (viewer, ctx, size) = match &parsed.target {
			FrameTarget::Sequence(id) => {
				let sequence = if id.is_empty() {
					self.active_sequence()?
				} else {
					self.require_node(id, NodeKind::Sequence, "sequence")?
				};
				let size = match parsed.max_size {
					Some(size) => size,
					None => native_size(project.sequence_geometry(sequence)),
				};
				(sequence, TimeContext::Sequence(id.clone()), size)
			}
			FrameTarget::Footage(id) => {
				let footage = self.footage(id)?;
				let size = match parsed.max_size {
					Some(size) => size,
					None => native_size(
						project
							.footage_video_params(footage, 0)
							.map(|params| (params.width, params.height)),
					),
				};
				(footage, TimeContext::Footage(id.clone()), size)
			}
		};
		let (at, at_sdk) = self.resolve_time(ctx, parsed.time)?;
		let (width, height) = size;
		let rgba = render_frame_f32(&project, viewer, at_sdk, size).ok_or_else(|| {
			ToolError::new(
				CODE_BACKEND,
				format!(
					"the engine could not render '{}' at {at} at {width}x{height}",
					parsed.target.id()
				),
			)
		})?;
		let frame = crate::render::Frame::from_rgba(width, height, rgba).map_err(|err| {
			ToolError::new(
				CODE_BACKEND,
				format!("the engine returned a malformed frame: {}", err.message),
			)
		})?;
		let mut out = frame_summary(&frame, &parsed.format);
		out["target"] = json!(parsed.target.id());
		out["time"] = parsed.time.to_json();
		out["backend"] = json!("sdk");
		Ok(out)
	}

	fn render_get_thumbnails(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::render_tools::parse_get_thumbnails(args)?;
		let height = parsed.height.unwrap_or(DEFAULT_THUMB_HEIGHT).max(1);
		let width = (height * 16 / 9).max(1);
		// The range is resolved on the target's own grid (footage against the
		// media's frame rate, a sequence against the sequence's).
		let ctx = match &parsed.target {
			FrameTarget::Sequence(id) => TimeContext::Sequence(id.clone()),
			FrameTarget::Footage(id) => TimeContext::Footage(id.clone()),
		};
		let in_ = self.resolve_time(ctx.clone(), parsed.range.in_)?.0;
		let out_at = self.resolve_time(ctx, parsed.range.out)?.0;
		if out_at <= in_ {
			return Err(ToolError::invalid_params(format!(
				"range out {} must be greater than in {}",
				parsed.range.out, parsed.range.in_
			)));
		}
		let span = out_at.checked_sub(in_)?;
		let mut frames = Vec::with_capacity(parsed.count);
		for index in 0..parsed.count {
			// t_i = in + span * i / count (left-closed, right-open).
			let offset = span.checked_mul_int(index as i64)?;
			let at = in_.checked_add(offset.checked_div_int(parsed.count as i64)?)?;
			let mut frame_args = json!({
				"time": at.to_json(),
				"max_size": {"width": width, "height": height},
				"format": "png",
			});
			match &parsed.target {
				FrameTarget::Sequence(id) => frame_args["sequence"] = json!(id),
				FrameTarget::Footage(id) => frame_args["footage"] = json!(id),
			}
			let mut frame = self.render_get_frame(&frame_args)?;
			frame["index"] = json!(index);
			frame["time"] = at.to_json();
			frames.push(frame);
		}
		Ok(json!({
			"frames": frames,
			"count": parsed.count,
			"backend": "sdk",
		}))
	}

	fn render_get_audio_levels(&mut self, _args: &Value) -> Result<Value, ToolError> {
		Err(ToolError::unsupported(
			GAP_RENDER_AUDIO_SAMPLES,
			"the engine exports no sequence audio-mix renderer (render_audio_samples has no C \
			 wrapper), and the SDK's per-file DecoderSession cannot be combined into a \
			 sequence mix because a block's connected source cannot be read back",
		))
	}

	// --- export / edit -------------------------------------------------

	fn export_start(&mut self, args: &Value) -> Result<Value, ToolError> {
		let parsed = tools::export::parse_export(args)?;
		let project = self.handle()?;
		let sequence = self.require_node(&parsed.sequence, NodeKind::Sequence, "sequence")?;
		// The SDK's export needs a render manager; the inline ("threads")
		// backend is the headless one that needs no oak-worker binary.
		if !manager_is_initialized() && !manager_init(RenderBackend::Threads) {
			return Err(ToolError::backend(
				"the engine's render manager could not be initialized (inline backend)",
			));
		}
		let (width, height) = project.sequence_geometry(sequence).ok_or_else(|| {
			ToolError::state(format!(
				"sequence '{}' has no video geometry to export",
				parsed.sequence
			))
		})?;
		let fps = SdkBackend::rat(project.sequence_frame_rate(sequence), "sequence frame rate")?;
		let length = SdkBackend::rat(project.sequence_length(sequence), "sequence length")?;
		let params = task::EncodingParams {
			format: Format::MPEG4Video as i32,
			video_enabled: true,
			video_codec: Codec::H264 as i32,
			video_width: width,
			video_height: height,
			// The time base is the frame duration: denominator over numerator.
			video_time_base_num: i32::try_from(fps.den)
				.map_err(|_| ToolError::backend("frame rate denominator out of range"))?,
			video_time_base_den: i32::try_from(fps.num)
				.map_err(|_| ToolError::backend("frame rate numerator out of range"))?,
			video_pixel_format: PixelFormat::U8 as i32,
			audio_enabled: false,
			audio_codec: Codec::AAC as i32,
			export_length_num: i32::try_from(length.num)
				.map_err(|_| ToolError::backend("sequence length numerator out of range"))?,
			export_length_den: i32::try_from(length.den)
				.map_err(|_| ToolError::backend("sequence length denominator out of range"))?,
			..Default::default()
		};
		let handle = task::export(&project, sequence, &parsed.output_path, &params)
			.ok_or_else(|| ToolError::backend("the engine refused to start the export task"))?;
		// The SDK task handle must outlive the call, so the export runs to
		// completion here and reports its real outcome (documented as
		// synchronous semantics in the README).
		let ok = handle.wait();
		let state = handle.state();
		if !ok || state != TaskState::Succeeded {
			return Err(ToolError::backend(format!(
				"the export task finished in state {:?} ({}): {}",
				state,
				handle.title(),
				handle.error()
			)));
		}
		let bytes = std::fs::metadata(&parsed.output_path)
			.map(|meta| meta.len())
			.unwrap_or(0);
		Ok(json!({
			"sequence": parsed.sequence,
			"output_path": parsed.output_path,
			"preset": parsed.preset,
			"started": true,
			"completed": true,
			"state": "succeeded",
			"bytes": bytes,
			"backend": "sdk",
		}))
	}

	fn edit_undo(&mut self, _args: &Value) -> Result<Value, ToolError> {
		if self.pending.is_some() {
			return Err(ToolError::new(
				crate::error::CODE_TRANSACTION,
				"an undo group is open; undo is only available between tool calls",
			));
		}
		if !undo_can_undo() {
			return Err(ToolError::new(
				CODE_NOTHING_TO_UNDO,
				"the engine's undo stack is empty",
			));
		}
		let label = undo_command_name(undo_count() - 1).unwrap_or_else(|| "unknown".to_string());
		if !undo() {
			return Err(ToolError::new(
				CODE_STATE,
				"the engine reported an undoable step but refused to undo it",
			));
		}
		// Row 0 is the engine's empty "New/Open Project" row, which is not an
		// undoable tool call.
		let remaining = (undo_index() - 1).max(0);
		Ok(json!({
			"undone": label,
			"remaining": remaining,
			"backend": "sdk",
		}))
	}
}

/// A native geometry capped to the default frame box, preserving aspect.
fn native_size(geometry: Option<(i32, i32)>) -> (u32, u32) {
	match geometry {
		Some((width, height)) if width > 0 && height > 0 => {
			let (width, height) = (width as u32, height as u32);
			if width <= DEFAULT_FRAME_SIZE.0 && height <= DEFAULT_FRAME_SIZE.1 {
				return (width, height);
			}
			let scale = f64::from(DEFAULT_FRAME_SIZE.0) / f64::from(width)
				.min(f64::from(DEFAULT_FRAME_SIZE.1) / f64::from(height));
			(
				((f64::from(width) * scale).round() as u32).max(1),
				((f64::from(height) * scale).round() as u32).max(1),
			)
		}
		_ => DEFAULT_FRAME_SIZE,
	}
}

/// Encodes a JSON parameter value into the engine's POD value, using the
/// parameter's current value kind (the engine stores values untyped, so the
/// existing kind is the only available statement of the declared type).
fn encode_node_value(
	key: &str,
	value: &Value,
	kind: i32,
	node: &str,
) -> Result<NodeValue, ToolError> {
	// `oak::*` discriminants: 1 INT, 2 FLOAT, 3 BOOL, 4 RATIONAL, 5 COLOR,
	// 6 VEC2, 7 VEC3, 8 VEC4, 9 COMBO, 10 STRING.
	const NONE: i32 = 0;
	const INT: i32 = 1;
	const FLOAT: i32 = 2;
	const BOOL: i32 = 3;
	const RATIONAL: i32 = 4;
	const COLOR: i32 = 5;
	const VEC2: i32 = 6;
	const VEC4: i32 = 8;
	const COMBO: i32 = 9;
	const STRING: i32 = 10;

	let kind = if kind == NONE {
		// The input has no stored value yet: infer from the JSON shape; the
		// caller verifies the write by reading it back.
		match value {
			Value::Bool(_) => BOOL,
			Value::Number(number) if number.is_i64() || number.is_u64() => INT,
			Value::Number(_) => FLOAT,
			Value::Array(items) => match items.len() {
				2 => VEC2,
				3 => 7,
				_ => VEC4,
			},
			Value::Object(_) => RATIONAL,
			Value::String(_) => STRING,
			_ => FLOAT,
		}
	} else {
		kind
	};

	let mut out = NodeValue {
		kind,
		num: 0,
		den: 0,
		f: [0.0; 4],
	};
	match kind {
		INT | COMBO => {
			out.num = value.as_i64().ok_or_else(|| {
				ToolError::invalid_params(format!(
					"parameter '{key}' on node '{node}' is an integer parameter; {value} is not \
					 one"
				))
			})?;
		}
		FLOAT => {
			out.f[0] = value.as_f64().ok_or_else(|| {
				ToolError::invalid_params(format!(
					"parameter '{key}' on node '{node}' is a numeric parameter; {value} is not \
					 one"
				))
			})?;
		}
		BOOL => {
			let flag = value.as_bool().ok_or_else(|| {
				ToolError::invalid_params(format!(
					"parameter '{key}' on node '{node}' is a boolean parameter; {value} is not \
					 one"
				))
			})?;
			out.num = i64::from(flag);
		}
		RATIONAL => {
			let num = value.get("num").and_then(Value::as_i64);
			let den = value.get("den").and_then(Value::as_i64);
			match (num, den) {
				(Some(num), Some(den)) if den != 0 => {
					out.num = num;
					out.den = den;
				}
				_ => {
					return Err(ToolError::invalid_params(format!(
						"parameter '{key}' on node '{node}' is a rational parameter; expected \
						 {{num, den}} with den != 0, got {value}"
					)));
				}
			}
		}
		COLOR | VEC2 | 7 | VEC4 => {
			let items = value.as_array().ok_or_else(|| {
				ToolError::invalid_params(format!(
					"parameter '{key}' on node '{node}' expects an array, got {value}"
				))
			})?;
			let needed = match kind {
				VEC2 => 2,
				7 => 3,
				_ => 4,
			};
			if items.len() != needed {
				return Err(ToolError::invalid_params(format!(
					"parameter '{key}' on node '{node}' expects {needed} values, got {}",
					items.len()
				)));
			}
			for (index, item) in items.iter().enumerate() {
				out.f[index] = item.as_f64().ok_or_else(|| {
					ToolError::invalid_params(format!(
						"parameter '{key}' on node '{node}' expects numbers, got {item}"
					))
				})?;
			}
		}
		STRING => {
			return Err(ToolError::unsupported(
				GAP_NODE_STRING_PARAM,
				format!(
					"parameter '{key}' on node '{node}' is a string parameter; the C ABI's \
					 OakNodeValue has no string payload"
				),
			));
		}
		other => {
			return Err(ToolError::invalid_params(format!(
				"parameter '{key}' on node '{node}' has a value kind ({other}) this backend \
				 cannot write"
			)));
		}
	}
	Ok(out)
}

/// Whether the engine stored what we asked for.
fn same_value(requested: &NodeValue, stored: &NodeValue) -> bool {
	if requested.kind != stored.kind || requested.num != stored.num || requested.den != stored.den {
		return false;
	}
	requested
		.f
		.iter()
		.zip(stored.f.iter())
		.all(|(a, b)| (a - b).abs() <= 1e-9)
}

/// The v2 extension face keeps its trait defaults here: this backend has
/// not been taught `sequence.*` yet, so those tools answer with the
/// structured `UNSUPPORTED` + gap id the defaults provide (never a fake
/// success).
impl crate::backend::EngineExt for SdkBackend {}

// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! An independent Oak editor built only on `oak-engine-rust-sdk`.
//!
//! This crate is the P8 proof: its Cargo manifest has exactly one Oak
//! dependency (the Rust SDK), and the whole workflow below uses only public
//! SDK types. It is deliberately small and readable so a new frontend can
//! copy the session/task/ownership patterns.

pub mod shell;

use std::path::{Path, PathBuf};

use oak_engine_rust_sdk::audio;
use oak_engine_rust_sdk::edit::{EditCommand, EditResult};
use oak_engine_rust_sdk::editor::{EditorSession, KeyframeInterpolation, TrackKind};
use oak_engine_rust_sdk::node::{self, NodeId, NodeKind, NodeValue};
use oak_engine_rust_sdk::ofx;
use oak_engine_rust_sdk::preview::PreviewSession;
use oak_engine_rust_sdk::render::{self, RenderBackend};
use oak_engine_rust_sdk::state;
use oak_engine_rust_sdk::task::{self, EncodingParams, TaskState};
use oak_engine_rust_sdk::types::Rational;

/// One run's observable results (used by the CLI summary and the tests).
#[derive(Debug, Clone)]
pub struct DemoReport {
	pub sequence: NodeId,
	pub footage: NodeId,
	pub clip: NodeId,
	pub effect: NodeId,
	/// Project-explorer entries visited while browsing the tree.
	pub explorer_entries: usize,
	pub sequence_length: Rational,
	pub preview_bytes: usize,
	pub preview_checksum: u64,
	pub audio_samples: usize,
	pub audio_peak: f32,
	pub keyframes: usize,
	pub reopened_clips: usize,
	pub undo_rows: i64,
	pub export_path: PathBuf,
	pub export_bytes: u64,
}

/// Media shipped with the example (a 2 s 160×90 H.264/AAC test clip).
pub fn test_media() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test_clip.mp4")
}

/// Brings up the engine backends the workflow needs: color config, text
/// backend, audio device (best effort), OFX scan and the inline render
/// manager (headless; no worker process required).
pub fn init_engine() {
	let _ = state::color_set_up_default_config(None);
	render::textengine_install();
	let _ = audio::create_instance();
	let _ = ofx::scan();
	ofx::register_plugin_nodes();
	ofx::install_render_executor();
	// Threads backend: renders inline on the calling worker, so the example
	// also runs in CI/containers without the oak-worker binary.
	let _ = render::manager_init(RenderBackend::Threads);
}

fn seq(session: &EditorSession) -> Result<NodeId, String> {
	session
		.current_sequence()
		.ok_or_else(|| "the session has no current sequence".to_string())
}

fn primary(result: EditResult, what: &str) -> Result<NodeId, String> {
	result
		.primary
		.ok_or_else(|| format!("{what} returned no node id"))
}

fn revision() -> i64 {
	node::undo_count()
}

/// Runs the complete headless workflow in `work_dir` and returns what it did.
///
/// The steps mirror the P8 acceptance list: init → new project/import/browse →
/// sequence/track edits → effects/keyframes → preview/audio → undo/redo →
/// save/reopen → export/poll.
pub fn run(work_dir: &Path) -> Result<DemoReport, String> {
	let media = test_media();
	if !media.is_file() {
		return Err(format!(
			"test media is missing at {} (run ffmpeg or check out the assets)",
			media.display()
		));
	}
	init_engine();
	std::fs::create_dir_all(work_dir)
		.map_err(|e| format!("cannot create {}: {e}", work_dir.display()))?;

	// 1. Project + sequence. `create_sequence` also installs the default
	// V1/V2/A1/A2 track layout and selects the sequence.
	let session = EditorSession::new().map_err(|e| format!("new session: {e}"))?;
	let sequence = session
		.create_sequence("SDK Editor Demo")
		.map_err(|e| format!("create sequence: {e}"))?;
	let project = session.project();

	// 2. Import + browse the project tree.
	let imported = session
		.execute(EditCommand::ImportFootage {
			path: media.to_string_lossy().into_owned(),
		})
		.map_err(|e| format!("import footage: {e}"))?;
	let footage = primary(imported, "import")?;
	let snapshot = session.snapshot();
	let explorer_entries = count_entries(&snapshot);
	if !snapshot.footages.iter().any(|item| item.id == footage) {
		return Err("imported footage is not in the project snapshot".to_string());
	}

	// 3. Place the clip on V1 and a second linked copy on A1.
	let clip = primary(
		session
			.execute(EditCommand::PlaceFootageClip {
				footage,
				kind: TrackKind::Video,
				track_index: 0,
				in_ts: 0,
				out_ts: 40,
				media_in_ts: 0,
			})
			.map_err(|e| format!("place video clip: {e}"))?,
		"place video",
	)?;
	let audio_clip = primary(
		session
			.execute(EditCommand::PlaceFootageClip {
				footage,
				kind: TrackKind::Audio,
				track_index: 0,
				in_ts: 0,
				out_ts: 40,
				media_in_ts: 0,
			})
			.map_err(|e| format!("place audio clip: {e}"))?,
		"place audio",
	)?;

	// 4. Edit: split, trim, move, ripple-delete the rear half.
	session
		.execute(EditCommand::SplitClip {
			clip,
			time_ts: 20,
		})
		.map_err(|e| format!("split: {e}"))?;
	// SplitClip returns no node id; the rear half is the block that now
	// starts at the split time on the same track.
	let rate = project.sequence_frame_rate(sequence);
	let split_time = Rational::new(20 * rate.den.max(1), rate.num.max(1));
	let rear = video_track(&project, sequence)
		.and_then(|track| {
			(0..project.track_block_count(track))
				.map(|index| project.track_block_at(track, index))
				.find(|&block| {
					block != clip
						&& project.node_kind(block) == Some(NodeKind::Clip)
						&& project.block_in(block) == split_time
				})
		})
		.ok_or_else(|| "split did not produce a rear half".to_string())?;
	session
		.execute(EditCommand::TrimClip {
			clip,
			new_in_ts: 4,
			new_out_ts: 20,
		})
		.map_err(|e| format!("trim: {e}"))?;
	session
		.execute(EditCommand::MoveClip {
			clip,
			new_in_ts: 6,
		})
		.map_err(|e| format!("move: {e}"))?;
	session
		.execute(EditCommand::RippleDeleteClip { clip: rear })
		.map_err(|e| format!("ripple delete: {e}"))?;
	let _ = audio_clip;

	// 5. Undo/redo around the move: the undo restores the trimmed range,
	// the redo re-applies it. One user action = one history row.
	let rows_before = revision();
	session
		.execute(EditCommand::MoveClip {
			clip,
			new_in_ts: 8,
		})
		.map_err(|e| format!("move again: {e}"))?;
	let rows_after = revision();
	if rows_after != rows_before + 1 {
		return Err(format!(
			"expected one undo row for the move ({rows_before} -> {rows_after})"
		));
	}
	node::undo();
	let moved_back = Rational::new(6 * rate.den.max(1), rate.num.max(1));
	let moved_forward = Rational::new(8 * rate.den.max(1), rate.num.max(1));
	if project.block_in(clip) != moved_back {
		return Err(format!(
			"undo did not restore the previous in point ({} != {})",
			project.block_in(clip).to_display_string(),
			moved_back.to_display_string()
		));
	}
	node::redo();
	if project.block_in(clip) != moved_forward {
		return Err("redo did not re-apply the move".to_string());
	}

	// 6. Effect + keyframes + parameter read-back.
	let effect = primary(
		session
			.execute(EditCommand::EffectInsert {
				host: clip,
				index: usize::MAX,
				type_id: "org.olivevideoeditor.Olive.clamp".to_string(),
			})
			.map_err(|e| format!("insert effect: {e}"))?,
		"effect insert",
	)?;
	for (time, value) in [(0.0, 1.0), (1.0, 0.4)] {
		session
			.execute(EditCommand::UpsertNodeKeyframe {
				node: effect,
				input_id: "max_in".to_string(),
				element: -1,
				time: Rational::from_f64(time),
				value: NodeValue::float(value),
				interpolation: KeyframeInterpolation::Linear,
				bezier_in: (0.0, 0.0),
				bezier_out: (0.0, 0.0),
			})
			.map_err(|e| format!("keyframe: {e}"))?;
	}
	let params = session
		.parameters(effect)
		.ok_or_else(|| "effect parameters unavailable".to_string())?;
	let keyframes = params
		.get("max_in")
		.and_then(|parameter| parameter.track(-1))
		.map(|track| track.len())
		.unwrap_or(0);
	if keyframes != 2 {
		return Err(format!("expected 2 keyframes, found {keyframes}"));
	}

	// 7. Preview source/program + audio block (owned frames, generation
	// checked). Preview before the GPU shader effect is exercised visually:
	// the headless inline backend renders the CPU graph.
	let mut program = PreviewSession::program(session.clone(), sequence, (160, 90))
		.map_err(|e| format!("program preview: {e}"))?;
	program
		.seek(Rational::new(1, 2))
		.map_err(|e| format!("seek: {e}"))?;
	let frame = program
		.request_frame()
		.ok_or_else(|| "frame request rejected".to_string())?;
	if !frame.wait() {
		return Err("preview frame render failed".to_string());
	}
	let frame = frame.take().ok_or_else(|| "no preview frame".to_string())?;
	let samples = frame.frame.f32_samples();
	// A non-black test pattern (luma varies across the frame).
	let preview_checksum = samples
		.chunks_exact(4)
		.map(|pixel| u64::from(pixel[0] > 0.01) | (u64::from(pixel[1] > 0.01) << 1))
		.fold(0u64, |acc, bit| acc.wrapping_mul(31).wrapping_add(bit));
	if samples.iter().all(|sample| *sample < 0.01) {
		return Err("preview rendered a black frame".to_string());
	}
	let mut source = PreviewSession::source(session.clone(), footage, (160, 90))
		.map_err(|e| format!("source preview: {e}"))?;
	source
		.seek(Rational::new(0, 1))
		.map_err(|e| format!("source seek: {e}"))?;
	let source_frame = source
		.request_frame()
		.ok_or_else(|| "source frame request rejected".to_string())?;
	if !source_frame.wait() {
		return Err("source frame render failed".to_string());
	}
	let _ = source_frame.take().ok_or_else(|| "no source frame".to_string())?;

	let audio_request = program
		.request_audio(0.25)
		.ok_or_else(|| "audio request rejected".to_string())?;
	if !audio_request.wait() {
		return Err("audio render failed".to_string());
	}
	let audio_block = audio_request
		.take()
		.ok_or_else(|| "no audio block".to_string())?;
	let audio_samples = audio_block.block.samples();
	let audio_peak = audio_samples
		.iter()
		.fold(0.0f32, |peak, sample| peak.max(sample.abs()));

	// 8. Save and reopen the exact same project through the SDK.
	let project_path = work_dir.join("sdk_editor_demo.ove");
	session
		.save(&project_path.to_string_lossy())
		.map_err(|e| format!("save: {e}"))?;
	let reopened =
		EditorSession::open(&project_path.to_string_lossy()).map_err(|e| format!("reopen: {e}"))?;
	let reopened_snapshot = reopened.snapshot();
	let reopened_sequence = reopened_snapshot
		.sequences
		.iter()
		.find(|item| item.id == sequence)
		.ok_or_else(|| "reopened project lost the sequence".to_string())?;
	let reopened_clips: usize = reopened_sequence.tracks.iter().map(|t| t.clips.len()).sum();
	if reopened_clips == 0 {
		return Err("reopened project has no clips".to_string());
	}
	// Parameters (including keyframes) survive the round trip. The
	// serializer deliberately renumbers node ids on load, so resolve the
	// reopened clip and effect from the graph snapshot instead of reusing
	// the runtime ids of the live session.
	let reopened_clip = reopened_sequence
		.tracks
		.iter()
		.filter(|track| track.kind == Some(TrackKind::Video))
		.flat_map(|track| track.clips.iter())
		.filter(|clip| clip.kind == Some(NodeKind::Clip))
		.map(|clip| clip.id)
		.next()
		.ok_or_else(|| "reopened project has no video clip".to_string())?;
	let graph = reopened
		.graph_snapshot(reopened_sequence.id, Some(reopened_clip))
		.ok_or_else(|| "reopened graph snapshot unavailable".to_string())?;
	let reopened_effect = graph
		.nodes
		.iter()
		.find(|node| node.type_id == "org.olivevideoeditor.Olive.clamp")
		.map(|node| node.id)
		.ok_or_else(|| "reopened clip lost its effect".to_string())?;
	let reopened_params = reopened
		.parameters(reopened_effect)
		.ok_or_else(|| "reopened effect parameters unavailable".to_string())?;
	if reopened_params
		.get("max_in")
		.and_then(|parameter| parameter.track(-1))
		.map(|track| track.len())
		!= Some(2)
	{
		return Err("keyframes did not survive save/reopen".to_string());
	}

	// 9. Export and poll/cancel through the unified task API.
	let geometry = project
		.sequence_geometry(sequence)
		.ok_or_else(|| "sequence has no geometry".to_string())?;
	// The engine's cached `sequence_length` only advances through length-
	// invalidating edits; take the content end from the blocks so the export
	// covers everything the timeline actually shows.
	let length = content_length(&project, sequence);
	let export_path = work_dir.join("sdk_editor_demo.mp4");
	let params = EncodingParams {
		format: oak_engine_rust_sdk::codec::Format::MPEG4Video as i32,
		video_enabled: true,
		video_codec: oak_engine_rust_sdk::codec::Codec::H264 as i32,
		video_width: geometry.0.max(2),
		video_height: geometry.1.max(2),
		video_time_base_num: rate.den.clamp(1, i32::MAX as i64) as i32,
		video_time_base_den: rate.num.clamp(1, i32::MAX as i64) as i32,
		video_pixel_format: 0,
		audio_enabled: false,
		export_length_num: length.num.max(0) as i32,
		export_length_den: length.den.max(1) as i32,
		..EncodingParams::default()
	};
	let export = task::export(
		&project,
		sequence,
		&export_path.to_string_lossy(),
		&params,
	)
	.ok_or_else(|| "export task rejected".to_string())?;
	if !export.wait() {
		return Err(format!("export failed: {}", export.error()));
	}
	if export.state() != TaskState::Succeeded {
		return Err(format!("export did not succeed: {:?}", export.state()));
	}
	let export_bytes = std::fs::metadata(&export_path)
		.map(|meta| meta.len())
		.map_err(|e| format!("export file missing: {e}"))?;
	if export_bytes == 0 {
		return Err("export produced an empty file".to_string());
	}

	Ok(DemoReport {
		sequence,
		footage,
		clip,
		effect,
		explorer_entries,
		sequence_length: length,
		preview_bytes: samples.len(),
		preview_checksum,
		audio_samples: audio_samples.len(),
		audio_peak,
		keyframes,
		reopened_clips,
		undo_rows: revision(),
		export_path,
		export_bytes,
	})
}

fn content_length(project: &oak_engine_rust_sdk::node::Project, sequence: NodeId) -> Rational {
	let mut end = Rational::new(0, 1);
	for list_index in 0..project.sequence_track_list_count(sequence) {
		let list = project.sequence_track_list_at(sequence, list_index);
		for track_index in 0..project.tracklist_track_count(list) {
			let track = project.tracklist_track_at(list, track_index);
			for block_index in 0..project.track_block_count(track) {
				let block = project.track_block_at(track, block_index);
				let out = project.block_out(block);
				if out > end {
					end = out;
				}
			}
		}
	}
	end
}

fn video_track(project: &oak_engine_rust_sdk::node::Project, sequence: NodeId) -> Option<NodeId> {
	for list_index in 0..project.sequence_track_list_count(sequence) {
		let list = project.sequence_track_list_at(sequence, list_index);
		if project.tracklist_kind(list) == 0 && project.tracklist_track_count(list) > 0 {
			return Some(project.tracklist_track_at(list, 0));
		}
	}
	None
}

fn count_entries(snapshot: &oak_engine_rust_sdk::editor::ProjectSnapshot) -> usize {
	fn walk(entry: &oak_engine_rust_sdk::editor::EntrySnapshot) -> usize {
		1 + entry.children.iter().map(walk).sum::<usize>()
	}
	walk(&snapshot.root)
}

/// Formats the report for the CLI (and keeps the GUI's status line short).
pub fn summary(report: &DemoReport) -> String {
	format!(
		"sequence {} · footage {} · clip {} · effect {}\n\
		 explorer entries: {} · length {}s · keyframes {}\n\
		 preview: {} floats (checksum {:#x}) · audio: {} samples (peak {:.3})\n\
		 reopened clips: {} · undo rows: {}\n\
		 export: {} ({} bytes)",
		report.sequence.raw(),
		report.footage.raw(),
		report.clip.raw(),
		report.effect.raw(),
		report.explorer_entries,
		format!("{:.2}", report.sequence_length.to_f64()),
		report.keyframes,
		report.preview_bytes,
		report.preview_checksum,
		report.audio_samples,
		report.audio_peak,
		report.reopened_clips,
		report.undo_rows,
		report.export_path.display(),
		report.export_bytes,
	)
}

/// A helper for the GUI shell: renders one program frame at `time`.
pub fn render_program_frame(
	session: &EditorSession,
	sequence: NodeId,
	time: Rational,
	size: (u32, u32),
) -> Option<Vec<f32>> {
	render::render_frame_f32(&session.project(), sequence, time, size)
}

/// Whether a node is a clip (used by the tiny GUI timeline).
pub fn is_clip(session: &EditorSession, node: NodeId) -> bool {
	session.project().node_kind(node) == Some(NodeKind::Clip)
}

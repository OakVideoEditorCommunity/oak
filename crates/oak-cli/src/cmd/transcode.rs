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

//! `oak-cli transcode <input_media> <out> [width] [--format ppm|mp4]` —
//! "media in, renders out" round trip, entirely through the Rust SDK.
//!
//! The source is probed with the SDK's codec decoder registry, a temporary
//! `EditorSession` assembles a one-clip sequence, frames/audio are rendered
//! through the SDK task/preview surface, and `--format mp4` exports through
//! the unified task API (H.264/AAC).
//!
//! `--format ppm` renders the frame range into P6 PPM frames plus a PCM s16
//! WAV when the source has audio streams.

use oak_engine_rust_sdk::codec;
use oak_engine_rust_sdk::editor::{EditorSession, TrackKind};
use oak_engine_rust_sdk::edit::EditCommand;
use oak_engine_rust_sdk::node::{NodeId, Project};
use oak_engine_rust_sdk::types::{Rational, TimeRange};

use crate::cmd::{EXIT_ERROR, EXIT_OK, EXIT_USAGE};
use crate::engine;
use crate::ppm;
use crate::wav;

/// Source description distilled from the probe.
struct SourceInfo {
	width: i32,
	height: i32,
	fr_num: i32,
	fr_den: i32,
	duration: f64,
	audio_streams: i32,
}

/// Run `transcode`. `width`/`format` are validated exactly like the C++
/// loop over `argv[4..]`.
pub fn run(input_media: String, out: String, width: Option<String>, format: Option<String>) -> i32 {
	if let Some(w) = &width {
		match w.parse::<i64>() {
			Ok(n) if n > 0 => {}
			_ => {
				eprintln!("error: invalid width \"{w}\"");
				return EXIT_USAGE;
			}
		}
	}
	if let Some(f) = &format {
		if f != "ppm" && f != "mp4" {
			eprintln!("error: unknown --format \"{f}\" (ppm|mp4)");
			return EXIT_USAGE;
		}
	}

	let is_ppm = format.as_deref().unwrap_or("mp4") == "ppm";
	run_transcode(&input_media, &out, width.as_deref(), is_ppm)
}

/// The transcode body.
fn run_transcode(input: &str, out: &str, width: Option<&str>, is_ppm: bool) -> i32 {
	let src = match probe_source(input) {
		Ok(s) => s,
		Err(msg) => {
			eprintln!("error: transcode: {msg}");
			return EXIT_ERROR;
		}
	};

	let out_w = width
		.and_then(|w| w.parse::<i32>().ok())
		.unwrap_or(src.width);
	let out_h = if src.width > 0 && src.height > 0 {
		((out_w as f64 * src.height as f64 / src.width as f64).round() as i32).max(1)
	} else {
		(out_w as f64 * 9.0 / 16.0).round() as i32
	};
	let fr_num = if src.fr_num > 0 { src.fr_num } else { 25 };
	let fr_den = if src.fr_den > 0 { src.fr_den } else { 1 };
	// A still/duration-less source counts as a single frame.
	let frames = (src.duration * fr_num as f64 / fr_den as f64).round() as i64;
	let frames = frames.max(1);

	let assembly = match assemble_sequence(input, out_w, out_h, fr_num, fr_den, frames, src.audio_streams) {
		Ok(a) => a,
		Err(msg) => {
			eprintln!("error: transcode: {msg}");
			return EXIT_ERROR;
		}
	};

	let code = if is_ppm {
		transcode_ppm(&assembly, out, out_w, out_h, fr_num, fr_den, frames, src.audio_streams > 0)
	} else {
		transcode_mp4(&assembly, out, fr_num, fr_den, frames)
	};
	engine::render_manager_shutdown();
	code
}

/// The assembled temporary sequence: the session (which owns the project)
/// and the sequence node.
struct Assembly {
	session: EditorSession,
	project: Project,
	sequence: NodeId,
}

/// Build the temporary sequence for the render/export stage.
fn assemble_sequence(
	input: &str,
	out_w: i32,
	out_h: i32,
	fr_num: i32,
	fr_den: i32,
	frames: i64,
	audio_streams: i32,
) -> Result<Assembly, String> {
	engine::render_manager_init()?;
	let session = EditorSession::new().map_err(|e| e.to_string())?;
	let sequence = session
		.create_sequence("transcode")
		.map_err(|e| e.to_string())?;
	let project = session.project();
	if !project.sequence_set_video_params(sequence, out_w, out_h, fr_num, fr_den) {
		return Err("cannot set the sequence video parameters".to_string());
	}
	let _ = session.set_current_sequence(sequence);

	let footage = session
		.execute(EditCommand::ImportFootage {
			path: input.to_string(),
		})
		.map_err(|e| format!("cannot import the source: {e}"))?
		.primary
		.ok_or_else(|| "the import returned no footage".to_string())?;
	session
		.execute(EditCommand::PlaceFootageClip {
			footage,
			kind: TrackKind::Video,
			track_index: 0,
			in_ts: 0,
			out_ts: frames,
			media_in_ts: 0,
		})
		.map_err(|e| format!("cannot place the video clip: {e}"))?;
	if audio_streams > 0 {
		session
			.execute(EditCommand::PlaceFootageClip {
				footage,
				kind: TrackKind::Audio,
				track_index: 0,
				in_ts: 0,
				out_ts: frames,
				media_in_ts: 0,
			})
			.map_err(|e| format!("cannot place the audio clip: {e}"))?;
	}

	Ok(Assembly {
		session,
		project,
		sequence,
	})
}

/// `--format ppm`: render the frame range into PPM frames (+ the audio
/// range into a WAV when the source has audio).
fn transcode_ppm(
	assembly: &Assembly,
	out: &str,
	out_w: i32,
	out_h: i32,
	fr_num: i32,
	fr_den: i32,
	frames: i64,
	audio: bool,
) -> i32 {
	if let Err(e) = std::fs::create_dir_all(out) {
		eprintln!("error: transcode: cannot create output directory \"{out}\": {e}");
		return EXIT_ERROR;
	}

	for i in 0..frames {
		let time = Rational::new(i * i64::from(fr_den), i64::from(fr_num));
		let frame = match engine::render_frame(&assembly.project, assembly.sequence, time, out_w, out_h) {
			Ok(f) => f,
			Err(e) => {
				let msg = if e.is_empty() {
					format!("frame {i} failed to render")
				} else {
					format!("frame {i} failed to render: {e}")
				};
				eprintln!("error: transcode: {msg}");
				return EXIT_ERROR;
			}
		};
		if let Err(msg) = write_frame_ppm(&frame, out, i) {
			eprintln!("error: transcode: {msg}");
			return EXIT_ERROR;
		}
	}
	eprintln!("transcoded {frames} frames to \"{out}\"");

	// Audio range (the assembly only adds audio clips when the source has
	// audio streams).
	if audio {
		return write_audio_wav(assembly, out, frames, fr_num, fr_den);
	}
	EXIT_OK
}

/// `--format mp4`: H.264/AAC through the unified export task.
fn transcode_mp4(assembly: &Assembly, out: &str, fr_num: i32, fr_den: i32, frames: i64) -> i32 {
	match engine::export_sequence(&assembly.project, assembly.sequence, out, fr_num, fr_den, frames) {
		Ok(()) => EXIT_OK,
		Err(msg) => {
			let err = if msg.is_empty() {
				"export failed".to_string()
			} else {
				msg
			};
			eprintln!("error: transcode: {err}");
			EXIT_ERROR
		}
	}
}

/// Write a rendered frame as `frame_%05d.ppm` in `out` (RGBA F32, 4
/// channels).
fn write_frame_ppm(frame: &engine::RenderedFrame, out: &str, index: i64) -> Result<(), String> {
	if frame.data.is_empty() || frame.width <= 0 || frame.height <= 0 || frame.linesize <= 0 {
		return Err(format!(
			"frame {index} has no pixel data ({}x{}, linesize {})",
			frame.width, frame.height, frame.linesize
		));
	}
	let path = std::path::Path::new(out).join(format!("frame_{index:05}.ppm"));
	ppm::write_ppm(
		&path,
		frame.width,
		frame.height,
		frame.format,
		4,
		frame.linesize,
		&frame.data,
	)
	.map_err(|e| format!("cannot write \"{}\": {e}", path.display()))
}

/// Render the audio range and write it as `audio.wav` in `out`.
fn write_audio_wav(assembly: &Assembly, out: &str, frames: i64, fr_num: i32, fr_den: i32) -> i32 {
	let range = TimeRange::new(
		Rational::new(0, 1),
		Rational::new(frames * i64::from(fr_den), i64::from(fr_num)),
	);
	let audio = match engine::render_audio(&assembly.project, assembly.sequence, range) {
		Ok(a) => a,
		Err(e) => {
			eprintln!("error: transcode: audio render failed: {e}");
			return EXIT_ERROR;
		}
	};
	if audio.data.is_empty() || audio.sample_rate <= 0 || audio.channel_count <= 0 {
		eprintln!("error: transcode: audio buffer is empty");
		return EXIT_ERROR;
	}
	let samples = (audio.data.len() / audio.channel_count as usize) as i64;
	let path = std::path::Path::new(out).join("audio.wav");
	if let Err(e) = wav::write_wav(&path, audio.sample_rate, audio.channel_count, samples, &audio.data) {
		eprintln!("error: transcode: cannot write \"{}\": {e}", path.display());
		return EXIT_ERROR;
	}
	EXIT_OK
}

/// Probe the source media into a [`SourceInfo`] through the SDK codec
/// registry.
fn probe_source(path: &str) -> Result<SourceInfo, String> {
	if !std::path::Path::new(path).exists() {
		return Err(format!("file does not exist: {path}"));
	}
	let footage = codec::decoder_probe(None, path, None)
		.ok_or_else(|| format!("could not probe \"{path}\""))?;
	let duration = footage
		.duration()
		.map(|range| range.length().to_f64())
		.unwrap_or(0.0);
	let audio_streams = footage.audio_stream_count() as i32;
	let video = footage.video_stream(0);
	let (width, height, fr_num, fr_den) = match video {
		Some(params) if params.width > 0 && params.height > 0 => (
			params.width,
			params.height,
			params.frame_rate_num,
			params.frame_rate_den,
		),
		_ => (1920, 1080, 25, 1),
	};
	Ok(SourceInfo {
		width,
		height,
		fr_num,
		fr_den,
		duration,
		audio_streams,
	})
}

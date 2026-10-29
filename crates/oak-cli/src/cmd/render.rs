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

//! `oak-cli render <project.ove> <start_seconds> <end_seconds> <out_dir>` —
//! render the first sequence to PPM frames plus a PCM s16 WAV (port of
//! `cmd_render()` in cli/main.cpp).
//!
//! Runs entirely through the module crates (M14 R2):
//!
//!   1. `crate::engine::load_project` — the oaknode serializer (the
//!      process chdirs into the project directory first, like the C++
//!      CLI, so relative footage paths resolve during rendering).
//!   2. Sequence 0's frame rate and geometry via the sequence behavior.
//!   3. `crate::engine::render_manager_init` — the oakrender manager's
//!      ticket arena drives the render.
//!   4. For every frame timestamp in `[start, end)` the video montage at
//!      that time is submitted as a ticket
//!      (`crate::engine::render_frame`) → [`crate::ppm::write_ppm`]
//!      (P6, 8-bit RGB).
//!   5. The audio range through the audio montage + ticket
//!      (`crate::engine::render_audio`) → [`crate::wav::write_wav`].
//!
//! Exit codes: a frame/audio render failure exits 2 (rendering
//! unavailable, mirroring the C++ code for a missing render backend);
//! project/sequence/argument failures exit 1; bad seconds exit 64
//! (usage). Frame progress goes to stderr (`frame N: T s`).

use oak_engine_rust_sdk::types::{Rational, TimeRange};

use crate::cmd::{EXIT_ERROR, EXIT_OK, EXIT_RENDER_UNAVAILABLE, EXIT_USAGE};
use crate::engine;
use crate::ppm;
use crate::wav;

/// Run `render` with the validated (or rejected) seconds arguments.
pub fn run(project: String, start_seconds: &str, end_seconds: &str, out_dir: &str) -> i32 {
	let start: f64 = match start_seconds.parse() {
		Ok(v) => v,
		Err(_) => {
			eprintln!("error: invalid start seconds \"{start_seconds}\"");
			return EXIT_USAGE;
		}
	};
	let end: f64 = match end_seconds.parse() {
		Ok(v) => v,
		Err(_) => {
			eprintln!("error: invalid end seconds \"{end_seconds}\"");
			return EXIT_USAGE;
		}
	};
	if end <= start {
		eprintln!("error: invalid end seconds \"{end_seconds}\"");
		return EXIT_USAGE;
	}

	run_render(&project, start, end, out_dir)
}

/// The render body.
fn run_render(project: &str, start: f64, end: f64, out_dir: &str) -> i32 {
	// Absolute project path first — the C++ CLI chdirs into the project
	// directory so relative footage paths resolve during rendering.
	let abs = match std::fs::canonicalize(project) {
		Ok(p) => p,
		Err(e) => {
			eprintln!("error: render: cannot open project \"{project}\": {e}");
			return EXIT_ERROR;
		}
	};
	if let Some(dir) = abs.parent() {
		let _ = std::env::set_current_dir(dir);
	}

	let loaded = match engine::load_project(&abs.to_string_lossy()) {
		Ok(p) => p,
		Err(detail) => {
			if detail.is_empty() {
				eprintln!("error: render: cannot load project \"{project}\"");
			} else {
				eprintln!("error: render: {detail}");
			}
			return EXIT_ERROR;
		}
	};

	// The render manager must be up before any ticket submission (the
	// facade's OAKENGINE_INIT_RENDER render boot).
	if let Err(e) = engine::render_manager_init() {
		eprintln!("error: render: cannot initialize the render manager: {e}");
		return EXIT_RENDER_UNAVAILABLE;
	}

	let project_ref = &loaded.project;
	let sequences = engine::sequence_ids(project_ref);
	let seq_id = match sequences.first() {
		Some(id) => *id,
		None => {
			eprintln!("error: render: project has no sequences");
			engine::render_manager_shutdown();
			return EXIT_ERROR;
		}
	};

	let (fr_num, fr_den, width, height) = {
		let fr = engine::sequence_frame_rate(project_ref, seq_id);
		let (w, h) = engine::sequence_geometry(project_ref, seq_id);
		(fr.num as i32, fr.den as i32, w, h)
	};
	if fr_num <= 0 || fr_den <= 0 {
		eprintln!("error: render: invalid sequence frame rate {fr_num}/{fr_den}");
		engine::render_manager_shutdown();
		return EXIT_ERROR;
	}
	if width <= 0 || height <= 0 {
		eprintln!("error: render: sequence has no video geometry");
		engine::render_manager_shutdown();
		return EXIT_ERROR;
	}

	if let Err(e) = std::fs::create_dir_all(out_dir) {
		eprintln!("error: render: cannot create output directory \"{out_dir}\": {e}");
		engine::render_manager_shutdown();
		return EXIT_ERROR;
	}

	// Frame loop: timestamps in the sequence time base (1/fr_num s).
	let start_frames = (start * fr_num as f64 / fr_den as f64).round() as i64;
	let mut index = start_frames;
	let mut written: u64 = 0;
	loop {
		let time = index as f64 * fr_den as f64 / fr_num as f64;
		if time >= end {
			break;
		}
		let time_r = Rational::new(index * i64::from(fr_den), i64::from(fr_num));
		let frame = match engine::render_frame(project_ref, seq_id, time_r, width, height) {
			Ok(f) => f,
			Err(e) => {
				let msg = if e.is_empty() {
					format!("frame at {time:.6} s failed to render")
				} else {
					format!("frame at {time:.6} s failed: {e}")
				};
				eprintln!("error: render: {msg}");
				engine::render_manager_shutdown();
				return EXIT_RENDER_UNAVAILABLE;
			}
		};
		if let Err(msg) = write_frame_ppm(&frame, out_dir, written) {
			eprintln!("error: render: {msg}");
			engine::render_manager_shutdown();
			return EXIT_ERROR;
		}
		eprintln!("frame {written}: {time:.6} s");
		written += 1;
		index += 1;
	}

	if written == 0 {
		eprintln!("error: render: empty frame range");
		engine::render_manager_shutdown();
		return EXIT_ERROR;
	}

	// Audio range in the sequence time base.
	let start_ts = (start * fr_num as f64 / fr_den as f64).round() as i64;
	let length_ts = ((end - start) * fr_num as f64 / fr_den as f64).round() as i64;
	let range = TimeRange::new(
		Rational::new(start_ts * i64::from(fr_den), i64::from(fr_num)),
		Rational::new(
			(start_ts + length_ts) * i64::from(fr_den),
			i64::from(fr_num),
		),
	);
	let code = match engine::render_audio(project_ref, seq_id, range) {
		Ok(audio) => write_audio_wav(&audio, out_dir),
		Err(e) => {
			let msg = if e.is_empty() {
				"audio render failed".to_string()
			} else {
				format!("audio render failed: {e}")
			};
			eprintln!("error: render: {msg}");
			EXIT_RENDER_UNAVAILABLE
		}
	};
	engine::render_manager_shutdown();
	code
}

/// Write a rendered frame as `frame_%05d.ppm` in `out_dir`. The render
/// frames are always in the internal RGBA layout
/// (`VideoParams::k_internal_channel_count == 4`), so 4 channels feed the
/// [`crate::ppm`] writer.
fn write_frame_ppm(frame: &engine::RenderedFrame, out_dir: &str, index: u64) -> Result<(), String> {
	if frame.data.is_empty() || frame.width <= 0 || frame.height <= 0 || frame.linesize <= 0 {
		return Err(format!(
			"frame {index} has no pixel data ({}x{}, linesize {})",
			frame.width, frame.height, frame.linesize
		));
	}
	let path = std::path::Path::new(out_dir).join(format!("frame_{index:05}.ppm"));
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

/// Write the rendered audio buffer as `audio.wav` in `out_dir` (the
/// buffer is interleaved f32).
fn write_audio_wav(audio: &engine::RenderedAudio, out_dir: &str) -> i32 {
	let rate = audio.sample_rate;
	let channels = audio.channel_count;
	if rate <= 0 || channels <= 0 || audio.data.is_empty() {
		eprintln!("error: render: audio buffer is empty");
		return EXIT_RENDER_UNAVAILABLE;
	}
	let samples = (audio.data.len() / channels as usize) as i64;
	let path = std::path::Path::new(out_dir).join("audio.wav");
	if let Err(e) = wav::write_wav(&path, rate, channels, samples, &audio.data) {
		eprintln!("error: render: cannot write \"{}\": {e}", path.display());
		return EXIT_ERROR;
	}
	EXIT_OK
}

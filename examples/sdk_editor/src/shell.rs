// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! A deliberately minimal second frontend with no third-party UI code: it
//! renders program frames from the same `EditorSession` as an ANSI
//! "framebuffer" and accepts transport commands on stdin. It proves the SDK
//! session, not the app shell, is what drives a frontend.
//!
//! Commands: `p` play ~1 s of frames, `+`/`-` step one frame, `s <seconds>`
//! seek, `l` print the audio peak of the next 100 ms, `q` quit.

use std::io::{BufRead, Write};
use std::path::Path;

use oak_engine_rust_sdk::editor::EditorSession;
use oak_engine_rust_sdk::preview::PreviewSession;
use oak_engine_rust_sdk::types::Rational;

const COLS: u32 = 72;
const ROWS: u32 = 22;

const RAMP: &[u8] = b" .:-=+*#%@";

pub fn run(work_dir: &Path) -> Result<(), String> {
	let report = super::run(work_dir)?;
	println!("{}", super::summary(&report));
	let project_path = report.export_path.with_extension("ove");
	let session = EditorSession::open(&project_path.to_string_lossy())
		.map_err(|e| format!("reopen for the shell: {e}"))?;
	// Node ids are renumbered by the serializer: resolve the sequence and
	// content length from the reopened snapshot.
	let snapshot = session.snapshot();
	let sequence = snapshot
		.sequences
		.first()
		.map(|item| item.id)
		.ok_or_else(|| "the saved project has no sequence".to_string())?;
	let length = snapshot
		.sequences
		.iter()
		.flat_map(|item| item.tracks.iter())
		.flat_map(|track| track.clips.iter())
		.map(|clip| clip.out_time)
		.fold(Rational::new(0, 1), |end, out| if out > end { out } else { end });
	let mut time = Rational::new(0, 1);
	let step = Rational::new(1, 25);
	let mut program = PreviewSession::program(session.clone(), sequence, (COLS, ROWS))
		.map_err(|e| format!("program preview: {e}"))?;

	println!("commands: p=play  +/-=step  s <sec>=seek  l=levels  q=quit");
	render(&mut program, time)?;
	let stdin = std::io::stdin();
	for line in stdin.lock().lines() {
		let line = line.map_err(|e| format!("stdin: {e}"))?;
		match line.trim() {
			"" => {}
			"q" => break,
			"p" => {
				let frames = 25;
				for _ in 0..frames {
					time = time + step;
					if time >= length {
						time = Rational::new(0, 1);
					}
					render(&mut program, time)?;
					std::thread::sleep(std::time::Duration::from_millis(40));
				}
			}
			"+" => {
				time = time + step;
				if time >= length {
					time = length;
				}
				render(&mut program, time)?;
			}
			"-" => {
				time = time - step;
				if time.num < 0 {
					time = Rational::new(0, 1);
				}
				render(&mut program, time)?;
			}
			"l" => {
				let request = program
					.request_audio(0.1)
					.ok_or_else(|| "audio request rejected".to_string())?;
				if request.wait() {
					if let Some(block) = request.take() {
						let samples = block.block.samples();
						let peak = samples
							.iter()
							.fold(0.0f32, |peak, sample| peak.max(sample.abs()));
						println!("audio peak: {peak:.4} ({} samples)", samples.len());
					}
				}
			}
			other if other.starts_with("s ") => {
				if let Ok(seconds) = other[2..].trim().parse::<f64>() {
					time = Rational::from_f64(seconds.clamp(0.0, length.to_f64()));
					render(&mut program, time)?;
				}
			}
			command => println!("unknown command: {command}"),
		}
	}
	Ok(())
}

fn render(program: &mut PreviewSession, time: Rational) -> Result<(), String> {
	program
		.seek(time)
		.map_err(|e| format!("seek: {e}"))?;
	let request = program
		.request_frame()
		.ok_or_else(|| "frame request rejected".to_string())?;
	if !request.wait() {
		return Err("frame render failed".to_string());
	}
	let frame = request.take().ok_or_else(|| "no frame".to_string())?;
	let samples = frame.frame.f32_samples();
	if samples.len() < (COLS * ROWS * 4) as usize {
		return Err("short frame buffer".to_string());
	}
	let mut out = String::with_capacity((COLS * (ROWS + 2)) as usize);
	for row in 0..ROWS {
		for col in 0..COLS {
			let index = ((row * COLS + col) * 4) as usize;
			let luma = 0.2126 * samples[index]
				+ 0.7152 * samples[index + 1]
				+ 0.0722 * samples[index + 2];
			let ramp = (luma.clamp(0.0, 1.0) * (RAMP.len() - 1) as f32).round() as usize;
			out.push(RAMP[ramp] as char);
		}
		out.push('\n');
	}
	print!("\x1b[2J\x1b[H{}", out);
	println!(
		"t={:.3}s  [p]lay  [+]  [-]  [s sec]eek  [l]evels  [q]uit",
		time.to_f64()
	);
	std::io::stdout().flush().ok();
	Ok(())
}

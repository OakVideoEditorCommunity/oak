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

//! `oak-cli info <project.ove>` — print the project name, its sequences and
//! its footage (port of `cmd_info()` in cli/main.cpp).
//!
//! Runs entirely through the module crates (M14 R2):
//! [`crate::engine::load_project`] (the oaknode serializer) produces the
//! project, and the project's graph supplies the sequences and footage
//! (arena order, like the facade's `project_sequence_at` walk). The output
//! is formatted by `crate::fmt` exactly like the C++ binary.
//!
//! Footage filenames stored relative to the `.ove` file are resolved
//! against the project directory for display (the C++ project-dir
//! convention); the online flag reports whether the resolved file exists.
//! A load failure prints the module's error and exits 1.

use std::path::Path;

use crate::cmd::{EXIT_ERROR, EXIT_OK};
use crate::engine;
use crate::fmt;

/// Run `info`. `project` is the .ove path from the command line.
pub fn run(project: String) -> i32 {
	run_info(&project)
}

/// The info body.
fn run_info(project: &str) -> i32 {
	let loaded = match engine::load_project(project) {
		Ok(p) => p,
		Err(detail) => {
			if detail.is_empty() {
				eprintln!("error: info: cannot load project \"{project}\"");
			} else {
				eprintln!("error: info: {detail}");
			}
			return EXIT_ERROR;
		}
	};

	// The module serializer swaps a fresh project payload in on load,
	// wiping the pre-load filename; when the project reports an empty
	// filename the CLI falls back to the path it loaded — the C++ CLI's own
	// project filename convention.
	let abs = std::fs::canonicalize(project).unwrap_or_else(|_| Path::new(project).to_path_buf());
	let project_ref = &loaded.project;
	let name = {
		let n = engine::project_name(project_ref);
		if n.is_empty() || n == "(untitled)" {
			abs.file_name()
				.map(|f| f.to_string_lossy().into_owned())
				.and_then(|f| f.split('.').next().map(|s| s.to_string()))
				.unwrap_or_else(|| "(untitled)".to_string())
		} else {
			n
		}
	};
	let filename = {
		let f = engine::project_filename(project_ref);
		if f.is_empty() {
			abs.to_string_lossy().into_owned()
		} else {
			f
		}
	};
	let modified = engine::project_modified(project_ref);

	println!("{}", fmt::project_line(&name));
	println!("{}", fmt::file_line(&filename));
	println!("{}", fmt::modified_line(modified));

	// Footage paths in .ove files can be relative to the project file;
	// resolve them for the online check (the C++ project-dir convention).
	let project_dir = Path::new(project).parent().map(|p| p.to_path_buf());

	let sequences = engine::sequence_ids(project_ref);
	println!("{}", fmt::sequences_line(sequences.len() as i64));
	for (index, seq_id) in sequences.iter().enumerate() {
		print_sequence(project_ref, *seq_id, index as i64);
	}

	let footage = engine::footage_ids(project_ref);
	println!("{}", fmt::footage_line(footage.len() as i64));
	for (index, footage_id) in footage.iter().enumerate() {
		let stored = engine::footage_filename(project_ref, *footage_id);
		let resolved = resolve_footage(&stored, project_dir.as_deref());
		let online = resolved.is_file();
		println!(
			"{}",
			fmt::footage_entry(index as i64, &resolved.to_string_lossy(), online)
		);
	}

	EXIT_OK
}

/// Print one sequence block (`print_sequence` in cli/main.cpp) through the
/// SDK project queries.
fn print_sequence(project: &engine::ProjectRef, seq_id: oak_engine_rust_sdk::node::NodeId, index: i64) {
	let name = engine::node_label(project, seq_id);
	let length = engine::sequence_length(project, seq_id);
	let frame_rate = engine::sequence_frame_rate(project, seq_id);
	let track_counts = engine::sequence_track_counts(project, seq_id);
	let playhead = engine::sequence_playhead(project, seq_id);

	let length_secs = if length.den != 0 {
		length.num as f64 / length.den as f64
	} else {
		0.0
	};
	let playhead_secs = if playhead.den != 0 {
		playhead.num as f64 / playhead.den as f64
	} else {
		0.0
	};
	// The playhead is printed as a frame timestamp in the sequence frame-rate
	// timebase (round-half-up, like the engine's rational_to_ts).
	let playhead_ts = {
		let fr = frame_rate;
		if fr.den == 0 {
			0
		} else {
			let n = playhead.num as i128 * fr.den as i128;
			let d = playhead.den as i128 * fr.num as i128;
			if d == 0 {
				0
			} else {
				let q = n / d;
				let r = (n % d).abs();
				let dd = d.abs();
				(q + if r * 2 >= dd { 1 } else { 0 }) as i64
			}
		}
	};

	println!(
		"{}",
		fmt::sequence(
			index,
			&name,
			length_secs,
			length.num,
			length.den,
			frame_rate.num,
			frame_rate.den,
			track_counts.0,
			track_counts.1,
			track_counts.2,
			playhead_ts,
			playhead_secs,
		)
	);
}

/// Resolve a stored footage filename against the project directory (the
/// C++ project-dir convention); absolute paths pass through.
fn resolve_footage(stored: &str, project_dir: Option<&Path>) -> std::path::PathBuf {
	let p = Path::new(stored);
	if p.is_absolute() || project_dir.is_none() {
		p.to_path_buf()
	} else {
		project_dir.unwrap().join(p)
	}
}

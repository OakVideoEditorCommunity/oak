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

//! C++→Rust parity tests for pure domain helpers. Each case is locked
//! against the C++ implementation (golden); see COVERAGE.md for the
//! parity/golden table.
//!
//! These helpers are pure (no external dylibs), so no stub module is
//! needed — the expected values below are transcribed from the C++ oracle
//! (`src/task/src/conform/conform.cpp` `derive_filenames` and
//! `src/task/src/proxy/proxy.cpp` `build_arguments`/`parse_progress`).

use oak_task::conform::ConformTask;
use oak_task::proxy::{ProxyParams, ProxyTask};

/// Given a stereo (2-channel) first-channel final filename, derive_filenames
/// returns two working and two final filenames whose suffixes match the C++
/// `derive_filenames` contract.
#[test]
fn conform_derive_filenames_stereo() {
	let (final_names, working_names) =
		ConformTask::derive_filenames("/cache/audio.0.pcm", 2).expect("valid stereo input");

	assert_eq!(final_names, ["/cache/audio.0.pcm", "/cache/audio.1.pcm"]);
	assert_eq!(
		working_names,
		["/cache/audio.0.pcm.working", "/cache/audio.1.pcm.working"]
	);
}

/// Given a mono (1-channel) first-channel final filename, derive_filenames
/// returns exactly one working and one final filename.
#[test]
fn conform_derive_filenames_mono() {
	let (final_names, working_names) =
		ConformTask::derive_filenames("/cache/audio.0.pcm", 1).expect("valid mono input");
	assert_eq!(final_names, ["/cache/audio.0.pcm"]);
	assert_eq!(working_names, ["/cache/audio.0.pcm.working"]);
}

/// Given a first-channel final filename that does not match the `.0.pcm`
/// suffix contract (or a zero channel count), derive_filenames errors
/// exactly like the C++ `derive_filenames` returning false.
#[test]
fn conform_derive_filenames_rejects_bad_input() {
	assert!(ConformTask::derive_filenames("/cache/audio.pcm", 2).is_err());
	assert!(ConformTask::derive_filenames("/cache/audio.0.wav", 2).is_err());
	assert!(ConformTask::derive_filenames("/cache/audio.0.pcm", 0).is_err());
	assert!(ConformTask::derive_filenames(".0.pcm", 2).is_err());
}

/// Given a source filename, stream index, proxy params and output filename,
/// build_arguments returns the argument vector in the exact order and flag
/// spelling produced by the C++ `build_arguments`.
#[test]
fn proxy_build_arguments_matches_cpp() {
	let params = ProxyParams {
		width: 0,
		height: 0,
		divider: 2,
		version: 0,
		crf: 18,
		include_audio: false,
		extension: String::new(),
		preset: String::new(),
	};
	let args = ProxyTask::build_arguments("/src.mov", 0, &params, "/dst.mov");

	// Golden vector transcribed from proxy.cpp `build_arguments` for a
	// divider-based, audio-less proxy.
	let expected = [
		"-y",
		"-nostats",
		"-progress",
		"pipe:1",
		"-i",
		"/src.mov",
		"-map",
		"0:0",
		"-an",
		"-vf",
		"scale=w=trunc(iw/2/2)*2:h=trunc(ih/2/2)*2",
		"-c:v",
		"libx264",
		"-preset",
		"",
		"-crf",
		"18",
		"-pix_fmt",
		"yuv420p",
		"-movflags",
		"+faststart",
		"-f",
		"mp4",
		"/dst.mov",
	];
	assert_eq!(args, expected);
}

#[test]
fn proxy_custom_size_forces_even_dimensions() {
	let params = ProxyParams {
		width: 1280,
		height: 720,
		divider: 1,
		version: 0,
		crf: 18,
		include_audio: false,
		extension: String::new(),
		preset: String::new(),
	};
	let args = ProxyTask::build_arguments("/odd-height.mov", 0, &params, "/dst.mov");
	let filter_index = args.iter().position(|arg| arg == "-vf").unwrap();
	assert_eq!(
		args[filter_index + 1],
		"scale=w=1280:h=720:force_original_aspect_ratio=decrease:force_divisible_by=2"
	);
}

/// Given a line reporting 50% of a 10-second duration, parse_progress
/// returns 0.5; malformed lines return None.
#[test]
fn proxy_parse_progress_reports_fraction() {
	// 5_000_000 us = 5 s of a 10 s source.
	assert_eq!(
		ProxyTask::parse_progress("out_time_us=5000000", 10.0),
		Some(0.5)
	);
	// ffmpeg's "ms" key is also microseconds (C++ reads both as us).
	assert_eq!(
		ProxyTask::parse_progress("out_time_ms=5000000", 10.0),
		Some(0.5)
	);
}

/// Given an out-of-range/invalid progress line, parse_progress clamps or
/// returns None exactly as the C++ parse_progress does.
#[test]
fn proxy_parse_progress_handles_invalid_lines() {
	// No timestamp key -> no progress.
	assert_eq!(ProxyTask::parse_progress("frame=100", 10.0), None);
	// Negative timestamp -> no progress.
	assert_eq!(ProxyTask::parse_progress("out_time_us=-5", 10.0), None);
	// Unknown duration -> no progress regardless of the line.
	assert_eq!(ProxyTask::parse_progress("out_time_us=5000000", 0.0), None);
	assert_eq!(ProxyTask::parse_progress("out_time_us=5000000", -1.0), None);
	// Over-range clamps to 1.0.
	assert_eq!(
		ProxyTask::parse_progress("out_time_us=20000000", 10.0),
		Some(1.0)
	);
	// Garbage value -> no progress.
	assert_eq!(ProxyTask::parse_progress("out_time_us=abc", 10.0), None);
}

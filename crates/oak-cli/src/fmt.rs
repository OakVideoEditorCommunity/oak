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

//! Output formatters matching the C++ `cli/main.cpp` byte for byte.
//!
//! The golden reference is the output of the C++ binary on the test
//! fixtures (`tests/project_with_footage.ove`, `tests/demo.mp4`), captured
//! before this crate existed. Each function below takes the plain data a
//! facade call would produce and formats it exactly like the C++ `printf`
//! call (`%.6f`, `%.3f`, `%lld`, `%d`, ...).
//!
//! The facade families that produce this data are still deferred
//! (`crate::deferred`), so the formatters are exercised by unit tests
//! against the golden text; the subcommands wire them in once the families
//! land (`dead_code` until then).

#![allow(dead_code)]

/// Formats the `info` header line `Project: <name>`.
///
/// `name` is the project's display name and is printed verbatim (an empty
/// name yields `Project: `); the returned line has no trailing newline.
pub fn project_line(name: &str) -> String {
	format!("Project: {name}")
}

/// Formats the `info` path line `File: <filename>`.
///
/// `filename` is the project's stored filename/path and is printed verbatim
/// (empty values produce `File: `); no trailing newline is added.
pub fn file_line(filename: &str) -> String {
	format!("File: {filename}")
}

/// Formats the `info` modified flag as `Modified: yes` or `Modified: no`.
///
/// `modified` selects `yes` when true and `no` when false; the line has no
/// trailing newline.
pub fn modified_line(modified: bool) -> String {
	format!("Modified: {}", if modified { "yes" } else { "no" })
}

/// Formats the `info` summary line `Sequences: <n>`.
///
/// `count` is the number of sequence nodes in the project; printed verbatim
/// and without a trailing newline.
pub fn sequences_line(count: i64) -> String {
	format!("Sequences: {count}")
}

/// Formats the `info` summary line `Footage: <n>`.
///
/// `count` is the number of footage entries in the project; printed verbatim
/// and without a trailing newline.
pub fn footage_line(count: i64) -> String {
	format!("Footage: {count}")
}

/// One sequence block (`print_sequence` in cli/main.cpp).
///
/// ```
///   [0] "Fixture Sequence"
///       length: 0.000000 s (0/1)
///       frame rate: 30000/1001 (29.970 fps)
///       tracks: video=0 audio=0 subtitle=0
///       playhead: 0 (0.000000 s)
/// ```
pub fn sequence(
	index: i64,
	name: &str,
	length_seconds: f64,
	len_num: i64,
	len_den: i64,
	fr_num: i64,
	fr_den: i64,
	video: i64,
	audio: i64,
	subtitle: i64,
	playhead: i64,
	playhead_seconds: f64,
) -> String {
	let fps = if fr_den != 0 {
		fr_num as f64 / fr_den as f64
	} else {
		0.0
	};
	format!(
		"  [{index}] \"{name}\"\n      length: {length_seconds:.6} s ({len_num}/{len_den})\n      \
         frame rate: {fr_num}/{fr_den} ({fps:.3} fps)\n      tracks: video={video} audio={audio} \
         subtitle={subtitle}\n      playhead: {playhead} ({playhead_seconds:.6} s)"
	)
}

/// One footage entry (`cmd_info`).
///
/// ```
///   [0] "/abs/path/demo.mp4" online
/// ```
pub fn footage_entry(index: i64, filename: &str, online: bool) -> String {
	format!(
		"  [{index}] \"{filename}\" {}",
		if online { "online" } else { "offline" }
	)
}

/// Formats the `probe` header line `Decoder: <name>`.
///
/// `decoder` is the decoder identifier returned by the SDK probe; it is
/// printed verbatim, with no trailing newline.
pub fn decoder_line(decoder: &str) -> String {
	format!("Decoder: {decoder}")
}

/// Formats the `probe` duration line `Duration: <seconds> s`.
///
/// `seconds` is the overall duration in seconds; it is printed with six
/// decimals and a trailing ` s`. The line has no trailing newline.
pub fn duration_line(seconds: f64) -> String {
	format!("Duration: {seconds:.6} s")
}

/// Formats the `probe` summary line `Video streams: <n>`.
///
/// `count` is the number of video streams reported by the probe; printed
/// verbatim, without a trailing newline.
pub fn video_streams_line(count: i64) -> String {
	format!("Video streams: {count}")
}

/// Formats one `probe` video-stream line:
/// `  [i] stream S: <w>x<h>, <num>/<den> fps (<fps>), duration <ts>/<den> (<s> s), primaries=<p> trc=<t>, interlaced|progressive`.
///
/// # Arguments
///
/// * `index` - display ordinal within the probe output (0-based, not the
///   container stream index).
/// * `stream_index` - the container's stream index.
/// * `width` / `height` - frame size in pixels.
/// * `frame_rate_num` / `frame_rate_den` - frame rate as a rational; a zero
///   denominator prints `0.000` for the decimal fps instead of dividing.
/// * `duration_ts` - stream duration in time-base ticks.
/// * `time_base_den` - denominator of the stream time base.
/// * `seconds` - the duration already converted to seconds; printed with six
///   decimals.
/// * `color_primaries` / `color_trc` - raw enum codes, printed as-is
///   (0 means unspecified).
/// * `interlaced` - selects the trailing `interlaced` or `progressive` word.
///
/// Returns the line without a trailing newline.
pub fn video_stream(
	index: i64,
	stream_index: i64,
	width: i64,
	height: i64,
	frame_rate_num: i64,
	frame_rate_den: i64,
	duration_ts: i64,
	time_base_den: i64,
	seconds: f64,
	color_primaries: i64,
	color_trc: i64,
	interlaced: bool,
) -> String {
	let fps = if frame_rate_den != 0 {
		frame_rate_num as f64 / frame_rate_den as f64
	} else {
		0.0
	};
	let interlace = if interlaced {
		"interlaced"
	} else {
		"progressive"
	};
	format!(
		"  [{index}] stream {stream_index}: {width}x{height}, {frame_rate_num}/{frame_rate_den} \
         fps ({fps:.3}), duration {duration_ts}/{time_base_den} ({seconds:.6} s), \
         primaries={color_primaries} trc={color_trc}, {interlace}"
	)
}

/// Formats the `probe` summary line `Audio streams: <n>`.
///
/// `count` is the number of audio streams reported by the probe; printed
/// verbatim, without a trailing newline.
pub fn audio_streams_line(count: i64) -> String {
	format!("Audio streams: {count}")
}

/// Formats one `probe` audio-stream line:
/// `  [i] stream S: <rate> Hz, <channels> channels, duration <ts>/<den> (<s> s)`.
///
/// # Arguments
///
/// * `index` - display ordinal within the probe output (0-based, not the
///   container stream index).
/// * `stream_index` - the container's stream index.
/// * `sample_rate` - sample rate in Hz.
/// * `channel_count` - channel count (the source's layout, not a bitmask).
/// * `duration_ts` - stream duration in time-base ticks.
/// * `time_base_den` - denominator of the stream time base.
/// * `seconds` - the duration already converted to seconds; printed with six
///   decimals.
///
/// Returns the line without a trailing newline.
pub fn audio_stream(
	index: i64,
	stream_index: i64,
	sample_rate: i64,
	channel_count: i64,
	duration_ts: i64,
	time_base_den: i64,
	seconds: f64,
) -> String {
	format!(
		"  [{index}] stream {stream_index}: {sample_rate} Hz, {channel_count} channels, \
         duration {duration_ts}/{time_base_den} ({seconds:.6} s)"
	)
}

/// Formats the `info` summary line `Subtitle streams: <n>`.
///
/// `count` is the number of subtitle streams reported by the probe; the value
/// is printed verbatim and the returned line has no trailing newline.
pub fn subtitle_streams_line(count: i64) -> String {
	format!("Subtitle streams: {count}")
}

#[cfg(test)]
mod tests {
	use super::*;

	// Golden text captured from the C++ binary:
	//   cmake-build-debug/cli/oak-cli info tests/project_with_footage.ove
	//   cmake-build-debug/cli/oak-cli probe tests/demo.mp4

	#[test]
	fn golden_info_output() {
		let mut out = String::new();
		out.push_str(&project_line("project_with_footage"));
		out.push('\n');
		out.push_str(&file_line(
			"/Users/sunyu/Projects/oak/tests/project_with_footage.ove",
		));
		out.push('\n');
		out.push_str(&modified_line(false));
		out.push('\n');
		out.push_str(&sequences_line(1));
		out.push('\n');
		out.push_str(&sequence(
			0,
			"Fixture Sequence",
			0.0,
			0,
			1,
			30000,
			1001,
			0,
			0,
			0,
			0,
			0.0,
		));
		out.push('\n');
		out.push_str(&footage_line(1));
		out.push('\n');
		out.push_str(&footage_entry(
			0,
			"/Users/sunyu/Projects/oak/tests/demo.mp4",
			true,
		));

		const GOLDEN: &str = concat!(
			"Project: project_with_footage\n",
			"File: /Users/sunyu/Projects/oak/tests/project_with_footage.ove\n",
			"Modified: no\n",
			"Sequences: 1\n",
			"  [0] \"Fixture Sequence\"\n",
			"      length: 0.000000 s (0/1)\n",
			"      frame rate: 30000/1001 (29.970 fps)\n",
			"      tracks: video=0 audio=0 subtitle=0\n",
			"      playhead: 0 (0.000000 s)\n",
			"Footage: 1\n",
			"  [0] \"/Users/sunyu/Projects/oak/tests/demo.mp4\" online",
		);
		assert_eq!(out, GOLDEN);
	}

	#[test]
	fn golden_probe_output() {
		let mut out = String::new();
		out.push_str(&decoder_line("ffmpeg"));
		out.push('\n');
		out.push_str(&duration_line(17.0));
		out.push('\n');
		out.push_str(&video_streams_line(1));
		out.push('\n');
		out.push_str(&video_stream(
			0, 0, 1920, 1080, 25, 1, 217600, 12800, 17.0, 1, 1, false,
		));
		out.push('\n');
		out.push_str(&audio_streams_line(1));
		out.push('\n');
		out.push_str(&audio_stream(0, 1, 48000, 2, 816000, 48000, 17.0));
		out.push('\n');
		out.push_str(&subtitle_streams_line(0));

		const GOLDEN: &str = concat!(
            "Decoder: ffmpeg\n",
            "Duration: 17.000000 s\n",
            "Video streams: 1\n",
            "  [0] stream 0: 1920x1080, 25/1 fps (25.000), duration 217600/12800 (17.000000 s), primaries=1 trc=1, progressive\n",
            "Audio streams: 1\n",
            "  [0] stream 1: 48000 Hz, 2 channels, duration 816000/48000 (17.000000 s)\n",
            "Subtitle streams: 0",
        );
		assert_eq!(out, GOLDEN);
	}

	#[test]
	fn fps_rounding_matches_printf() {
		// 30000/1001 = 29.970029... -> %.3f -> "29.970"
		let s = sequence(0, "S", 0.0, 0, 1, 30000, 1001, 0, 0, 0, 0, 0.0);
		assert!(s.contains("frame rate: 30000/1001 (29.970 fps)"), "{s}");
	}

	#[test]
	fn offline_footage_prints_offline() {
		assert_eq!(
			footage_entry(2, "gone.mp4", false),
			"  [2] \"gone.mp4\" offline"
		);
	}
}

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

//! `oak-cli probe <mediafile>` — probe a media file and print its decoder,
//! duration and video/audio/subtitle streams (port of `cmd_probe()` in
//! cli/main.cpp).
//!
//! Runs through the SDK codec probe (`codec::decoder_probe`): the engine's
//! decoder registry returns the decoder id, the per-stream inventory and
//! each stream's duration in rational seconds. A missing file prints
//! `error: probe: file does not exist: <path>` on stderr and exits 1.

use oak_engine_rust_sdk::codec;
use oak_engine_rust_sdk::types::Rational;

use crate::cmd::{EXIT_ERROR, EXIT_OK};
use crate::fmt;

/// Run `probe`. `mediafile` is the media path from the command line.
pub fn run(mediafile: String) -> i32 {
	run_probe(&mediafile)
}

/// The probe body.
fn run_probe(mediafile: &str) -> i32 {
	if !std::path::Path::new(mediafile).exists() {
		eprintln!("error: probe: file does not exist: {mediafile}");
		return EXIT_ERROR;
	}

	// The engine probes through the codec decoder registry and returns an
	// owned stream inventory.
	let Some(footage) = codec::decoder_probe(None, mediafile, None) else {
		eprintln!("error: probe: could not probe \"{mediafile}\"");
		return EXIT_ERROR;
	};

	println!("{}", fmt::decoder_line(&footage.decoder_name()));

	// Stream durations are `duration` ticks in each stream's time base.
	let video: Vec<_> = (0..footage.video_stream_count())
		.filter_map(|ordinal| footage.video_stream(ordinal).map(|params| (ordinal, params)))
		.collect();
	let audio: Vec<_> = (0..footage.audio_stream_count())
		.filter_map(|ordinal| footage.audio_stream(ordinal).map(|params| (ordinal, params)))
		.collect();
	let video_durations: Vec<Rational> = video
		.iter()
		.map(|(_, params)| {
			stream_duration(params.duration, params.time_base_num, params.time_base_den)
		})
		.collect();
	let audio_durations: Vec<Rational> = audio
		.iter()
		.map(|(_, params)| {
			stream_duration(params.duration, params.time_base_num, params.time_base_den)
		})
		.collect();
	let duration = video_durations
		.iter()
		.chain(audio_durations.iter())
		.copied()
		.fold(Rational::new(0, 1), |longest, value| {
			if value > longest { value } else { longest }
		});
	println!("{}", fmt::duration_line(rational_secs(duration)));

	println!("{}", fmt::video_streams_line(video.len() as i64));
	for (index, ((ordinal, params), duration)) in video.iter().zip(video_durations).enumerate() {
		println!(
			"{}",
			fmt::video_stream(
				index as i64,
				params.stream_index as i64,
				params.width as i64,
				params.height as i64,
				params.frame_rate_num as i64,
				params.frame_rate_den as i64,
				duration.num,
				duration.den,
				rational_secs(duration),
				0,
				0,
				false,
			)
		);
		let _ = ordinal;
	}

	println!("{}", fmt::audio_streams_line(audio.len() as i64));
	for (index, ((ordinal, params), duration)) in audio.iter().zip(audio_durations).enumerate() {
		println!(
			"{}",
			fmt::audio_stream(
				index as i64,
				params.stream_index as i64,
				params.sample_rate as i64,
				params.channel_layout.count_ones() as i64,
				duration.num,
				duration.den,
				rational_secs(duration),
			)
		);
		let _ = ordinal;
	}

	println!("{}", fmt::subtitle_streams_line(footage.subtitle_stream_count() as i64));

	EXIT_OK
}

/// `duration` ticks in `(num, den)` time base as rational seconds.
fn stream_duration(duration: i64, time_base_num: i32, time_base_den: i32) -> Rational {
	if duration <= 0 || time_base_num <= 0 || time_base_den <= 0 {
		return Rational::new(0, 1);
	}
	Rational::new(
		duration.saturating_mul(i64::from(time_base_num)),
		i64::from(time_base_den),
	)
}

/// A rational as floating-point seconds (0 on a zero denominator).
fn rational_secs(r: Rational) -> f64 {
	if r.den != 0 {
		r.num as f64 / r.den as f64
	} else {
		0.0
	}
}

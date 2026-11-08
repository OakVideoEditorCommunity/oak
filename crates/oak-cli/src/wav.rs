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

//! PCM s16 WAV writer — the exact port of `write_wav()` in `cli/main.cpp`.
//!
//! Takes interleaved float samples (the order `cmd_render`/`cmd_transcode`
//! produce by interleaving the planar `OakEngineAudioBuffer` channels) and
//! writes a classic 44-byte-header PCM WAV. Float samples are clamped to
//! [-1, 1] and converted with `v * 32767.0` truncated toward zero, exactly
//! like the C++ `static_cast<int16_t>(clamped * 32767.0f)`.
//!
//! `dead_code` until the render/transcode ports call it (it is exercised by
//! the unit tests below).

#![allow(dead_code)]

use std::io::{self, Write};
use std::path::Path;

/// Write `v` as two little-endian bytes to `f`.
///
/// Header/sample helper for [`write_wav`]: it emits the channel count, PCM
/// tag, block alignment and bits-per-sample in the `fmt ` chunk, and each
/// clamped sample in the data chunk. Uses [`Write::write_all`], so the call
/// either accepts both bytes or reports an error.
///
/// # Errors
///
/// Returns the writer's I/O error when `write_all` fails.
fn write_u16_le(f: &mut impl Write, v: u16) -> io::Result<()> {
	f.write_all(&[v as u8, (v >> 8) as u8])
}

/// Write `v` as four little-endian bytes to `f`.
///
/// Header helper for [`write_wav`] (RIFF chunk size, `fmt ` chunk size,
/// sample rate, byte rate and data-chunk size). Uses [`Write::write_all`],
/// so the call either accepts all four bytes or reports an error.
///
/// # Errors
///
/// Returns the writer's I/O error when `write_all` fails.
fn write_u32_le(f: &mut impl Write, v: u32) -> io::Result<()> {
	f.write_all(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8])
}

/// Write interleaved float samples as a PCM s16 WAV file.
///
/// `data` must hold `samples * channels` values in interleaved order
/// (`[s0c0, s0c1, s1c0, s1c1, ...]`), matching what the C++ loop over
/// `oakengine_audio_data(audio, ch)[i]` emits.
pub fn write_wav(
	path: &Path,
	rate: i32,
	channels: i32,
	samples: i64,
	data: &[f32],
) -> io::Result<()> {
	let rate = u32::try_from(rate).map_err(|_| invalid_data("negative sample rate"))?;
	let channels = u32::try_from(channels).map_err(|_| invalid_data("negative channel count"))?;
	let samples = u64::try_from(samples).map_err(|_| invalid_data("negative sample count"))?;
	if channels == 0 {
		return Err(invalid_data("zero channel count"));
	}
	let expected = samples
		.checked_mul(u64::from(channels))
		.ok_or_else(|| invalid_data("sample count overflow"))?;
	if data.len() as u64 != expected {
		return Err(invalid_data(
			"sample buffer length does not match rate/channels/samples",
		));
	}

	let data_size = expected
		.checked_mul(2)
		.and_then(|v| u32::try_from(v).ok())
		.ok_or_else(|| invalid_data("WAV data chunk exceeds 4 GiB"))?;
	let byte_rate = rate
		.checked_mul(channels)
		.and_then(|v| v.checked_mul(2))
		.ok_or_else(|| invalid_data("byte rate overflow"))?;
	let block_align = channels
		.checked_mul(2)
		.and_then(|v| u16::try_from(v).ok())
		.ok_or_else(|| invalid_data("block align overflow"))?;

	let mut f = std::fs::File::create(path)?;
	f.write_all(b"RIFF")?;
	write_u32_le(&mut f, 36 + data_size)?;
	f.write_all(b"WAVE")?;
	f.write_all(b"fmt ")?;
	write_u32_le(&mut f, 16)?; // fmt chunk size
	write_u16_le(&mut f, 1)?; // PCM
	write_u16_le(&mut f, channels as u16)?;
	write_u32_le(&mut f, rate)?;
	write_u32_le(&mut f, byte_rate)?;
	write_u16_le(&mut f, block_align)?;
	write_u16_le(&mut f, 16)?; // bits per sample
	f.write_all(b"data")?;
	write_u32_le(&mut f, data_size)?;

	for &v in data {
		let clamped = if v < -1.0 {
			-1.0
		} else if v > 1.0 {
			1.0
		} else {
			v
		};
		// static_cast<int16_t>(clamped * 32767.0f): truncation toward zero.
		let s = (clamped * 32767.0) as i16;
		write_u16_le(&mut f, s as u16)?;
	}
	f.flush()
}

fn invalid_data(msg: &str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn golden_mono_wav() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_wav_mono.wav");
		// 2 samples mono at 44100 Hz: 0.0, 0.5
		write_wav(&path, 44100, 1, 2, &[0.0, 0.5]).unwrap();

		let got = std::fs::read(&path).unwrap();
		// 44-byte header + 2 samples * 2 bytes.
		assert_eq!(got.len(), 48);
		assert_eq!(&got[0..4], b"RIFF");
		// chunk size = 36 + 4 = 40
		assert_eq!(&got[4..8], &[40, 0, 0, 0]);
		assert_eq!(&got[8..12], b"WAVE");
		assert_eq!(&got[12..16], b"fmt ");
		assert_eq!(&got[16..20], &[16, 0, 0, 0]);
		assert_eq!(&got[20..22], &[1, 0]); // PCM
		assert_eq!(&got[22..24], &[1, 0]); // mono
		assert_eq!(&got[24..28], &[0x44, 0xAC, 0, 0]); // 44100
		assert_eq!(&got[28..32], &[0x88, 0x58, 0x01, 0]); // byte rate 88200
		assert_eq!(&got[32..34], &[2, 0]); // block align
		assert_eq!(&got[34..36], &[16, 0]); // bits per sample
		assert_eq!(&got[36..40], b"data");
		assert_eq!(&got[40..44], &[4, 0, 0, 0]); // data size
										   // 0.0 -> 0; 0.5 * 32767 = 16383.5 -> truncates to 16383 (0x3FFF)
		assert_eq!(&got[44..48], &[0x00, 0x00, 0xFF, 0x3F]);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn stereo_interleaving_and_clamping() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_wav_stereo.wav");
		// 1 sample stereo at 48000: (-1.0, 1.0) interleaved.
		write_wav(&path, 48000, 2, 1, &[-1.0, 1.0]).unwrap();

		let got = std::fs::read(&path).unwrap();
		assert_eq!(&got[22..24], &[2, 0]); // stereo
		assert_eq!(&got[32..34], &[4, 0]); // block align
									 // -1.0 -> -32767 = 0x8001; 1.0 -> 32767 = 0x7FFF
		assert_eq!(&got[44..48], &[0x01, 0x80, 0xFF, 0x7F]);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn sample_count_mismatch_is_an_error() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_wav_bad.wav");
		let err = write_wav(&path, 48000, 2, 10, &[0.0f32; 3]).unwrap_err();
		assert!(err.to_string().contains("does not match"));
	}
}

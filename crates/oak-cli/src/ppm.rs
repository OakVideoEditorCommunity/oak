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

//! PPM (P6, 8-bit RGB) frame writer — the exact port of `write_ppm()` in
//! `cli/main.cpp`.
//!
//! Takes the raw pixel data an `OakEngineFrame` facade handle would expose
//! (linesize-strided rows of `channels` values per pixel) and writes a P6
//! file. Pixel formats: `f32` ([`PIXEL_FORMAT_F32`], 4 bytes per channel,
//! clamped to [0,1]) and `u8` (format 0, 1 byte per channel). Any other
//! format is an error, mirroring the C++ throw.
//!
//! `dead_code` until the render/transcode ports call it (it is exercised by
//! the unit tests below).

#![allow(dead_code)]

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// olive::core::PixelFormat::f32 — the renderer's default frame format
/// (`k_pixel_format_f32` in cli/main.cpp).
pub const PIXEL_FORMAT_F32: i32 = 4;

/// Write `width` x `height` rows of pixel data as a P6 PPM file.
///
/// `data` must hold `linesize * height` bytes; each row starts `linesize`
/// bytes apart (stride). `channels` is the per-pixel channel count in the
/// source data; only the first three channels are emitted.
pub fn write_ppm(
	path: &Path,
	width: i32,
	height: i32,
	format: i32,
	channels: i32,
	linesize: i32,
	data: &[u8],
) -> io::Result<()> {
	let width = usize::try_from(width).map_err(|_| invalid_data("negative width"))?;
	let height = usize::try_from(height).map_err(|_| invalid_data("negative height"))?;
	let linesize = usize::try_from(linesize).unwrap_or(0);
	let channels = usize::try_from(channels).map_err(|_| invalid_data("negative channel count"))?;
	if channels < 3 {
		return Err(invalid_data("channel count below 3"));
	}

	let mut out =
		Vec::with_capacity(format!("P6\n{width} {height}\n255\n").len() + width * height * 3);
	out.extend_from_slice(format!("P6\n{width} {height}\n255\n").as_bytes());

	let mut row = vec![0u8; width * 3];
	for y in 0..height {
		let line_start = y * linesize;
		let line_end = line_start.checked_add(linesize);
		let line = match line_end {
			Some(end) if end <= data.len() => &data[line_start..end],
			_ => {
				return Err(invalid_data(
					"pixel data buffer is shorter than the frame geometry",
				));
			}
		};
		for x in 0..width {
			for c in 0..3 {
				let v = if format == PIXEL_FORMAT_F32 {
					// f32: 4 bytes per channel.
					let off = (x * channels + c) * 4;
					let px = f32::from_ne_bytes([
						line[off],
						line[off + 1],
						line[off + 2],
						line[off + 3],
					]);
					let clamped = if px < 0.0 {
						0.0
					} else if px > 1.0 {
						1.0
					} else {
						px
					};
					// static_cast<unsigned char>(clamped * 255.0f + 0.5f):
					// truncation toward zero, same as Rust `as u8`.
					(clamped * 255.0 + 0.5) as u8
				} else if format == 0 {
					// u8: 1 byte per channel.
					line[x * channels + c]
				} else {
					return Err(invalid_data(&format!(
						"unsupported frame pixel format {format}"
					)));
				};
				row[x * 3 + c] = v;
			}
		}
		out.extend_from_slice(&row);
	}

	let mut f = fs::File::create(path)?;
	f.write_all(&out)
}

fn invalid_data(msg: &str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn bytes(hex: &str) -> Vec<u8> {
		let mut v = Vec::new();
		for pair in hex.as_bytes().chunks(2) {
			let s = std::str::from_utf8(pair).unwrap();
			v.push(u8::from_str_radix(s, 16).unwrap());
		}
		v
	}

	#[test]
	fn writes_p6_header_and_u8_rows() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_ppm_u8.ppm");
		// 2x2, 3 channels, linesize 6, u8.
		let data = vec![
			1, 2, 3, 4, 5, 6, //
			7, 8, 9, 10, 11, 12, //
		];
		write_ppm(&path, 2, 2, 0, 3, 6, &data).unwrap();

		let got = std::fs::read(&path).unwrap();
		let mut expected = b"P6\n2 2\n255\n".to_vec();
		expected.extend_from_slice(&data);
		assert_eq!(got, expected);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn f32_rows_are_clamped_and_quantized() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_ppm_f32.ppm");
		// 1x1, RGBA (4 channels), linesize 16, f32.
		let data = bytes("0000803f0000803f0000803f00000000"); // 1.0, 1.0, 1.0, 0.0
		write_ppm(&path, 1, 1, PIXEL_FORMAT_F32, 4, 16, &data).unwrap();

		let got = std::fs::read(&path).unwrap();
		assert_eq!(&got[..11], b"P6\n1 1\n255\n");
		assert_eq!(&got[11..], &[255, 255, 255]); // 1.0 -> 255 (clamped * 255 + 0.5, truncated)
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn clamps_f32_negative_and_over_one() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_ppm_clamp.ppm");
		// 2x1 RGB f32: (-0.5, 0.25, 2.0) | (0.0, 0.5, 1.0)
		let mut data = Vec::new();
		for v in [-0.5f32, 0.25, 2.0, 0.0, 0.5, 1.0] {
			data.extend_from_slice(&v.to_ne_bytes());
		}
		write_ppm(&path, 2, 1, PIXEL_FORMAT_F32, 3, 24, &data).unwrap();

		let got = std::fs::read(&path).unwrap();
		// 0.0 -> 0, 0.25*255+0.5=64.25 -> 64, 1.0 -> 255, 0.5*255+0.5=128.0 -> 128
		assert_eq!(&got[11..], &[0, 64, 255, 0, 128, 255]);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn unsupported_format_is_an_error() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_ppm_bad.ppm");
		let err = write_ppm(&path, 1, 1, 7, 3, 3, &[0, 0, 0]).unwrap_err();
		assert!(err.to_string().contains("unsupported frame pixel format 7"));
	}

	#[test]
	fn short_buffer_is_an_error() {
		let dir = std::env::temp_dir();
		let path = dir.join("oak_cli_test_ppm_short.ppm");
		let err = write_ppm(&path, 4, 4, 0, 3, 12, &[0u8; 10]).unwrap_err();
		assert!(err.to_string().contains("shorter than the frame geometry"));
	}
}

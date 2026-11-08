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

//! The synthetic CPU viewer frame both engines display.
//!
//! The real engine delivers frames through the render worker (a separate
//! process speaking the NDJSON control-plane protocol, `oakengine::worker`),
//! which is out of scope for this increment. Until that transport is wired,
//! both the mock and the real engine feed the viewers the same SMPTE-style
//! test pattern, so playback is visibly moving while the engine metadata
//! (project / sequence / tracks) comes from the real facade in real mode.

use crate::widgets::timeline::Frame;
use gpui_kit::RenderImage;
use std::borrow::Cow;

/// Width of the synthetic test frame (a small proxy size; the real engine
/// will deliver full-resolution frames).
pub(crate) const SYNTH_FRAME_WIDTH: u32 = 384;
/// Height of the synthetic test frame.
pub(crate) const SYNTH_FRAME_HEIGHT: u32 = 216;

/// Generates the F32 RGBA samples of the synthetic test frame: SMPTE-style
/// color bars with a white sweep whose x position follows `frame`, so
/// transport playback shows up as motion across the picture.
///
/// The samples mirror the real engine's pixel format; callers downconvert
/// them to BGRA8 for the viewer's CPU-frame path and analyze the scope
/// samples from the very same buffer, so the scopes read exactly what the
/// viewer displays.
pub(crate) fn synthetic_frame_samples(frame: Frame) -> (u32, u32, Vec<f32>) {
	let width = SYNTH_FRAME_WIDTH;
	let height = SYNTH_FRAME_HEIGHT;

	// F32 RGBA samples; the caller downconverts to BGRA8 for the sprite atlas.
	let mut samples = vec![0.0f32; (width * height * 4) as usize];
	// SMPTE bars: 75% white, yellow, cyan, green, magenta, red, blue.
	let bars: [(f32, f32, f32); 7] = [
		(1.0, 1.0, 1.0),
		(1.0, 1.0, 0.0),
		(0.0, 1.0, 1.0),
		(0.0, 1.0, 0.0),
		(1.0, 0.0, 1.0),
		(1.0, 0.0, 0.0),
		(0.0, 0.0, 1.0),
	];
	// Bottom strip: blue, magenta, 75% white, black.
	let strip: [(f32, f32, f32); 4] = [
		(0.0, 0.0, 1.0),
		(1.0, 0.0, 1.0),
		(0.75, 0.75, 0.75),
		(0.0, 0.0, 0.0),
	];
	// The sweep moves 6 px per frame and wraps around the width, so
	// transport playback shows up as motion across the picture.
	let sweep = (frame.0 as f32 * 6.0) % width as f32;
	let bars_top = height as f32 * 0.66;

	for y in 0..height {
		for x in 0..width {
			let in_sweep = (x as f32 - sweep).abs() < 6.0;
			let color = if in_sweep {
				(1.0, 1.0, 1.0)
			} else if (y as f32) < bars_top {
				bars[((x as f32 / width as f32) * 7.0) as usize]
			} else {
				strip[((x as f32 / width as f32) * 4.0) as usize]
			};
			let i = ((y * width + x) * 4) as usize;
			samples[i] = color.0;
			samples[i + 1] = color.1;
			samples[i + 2] = color.2;
			samples[i + 3] = 1.0;
		}
	}

	(width, height, samples)
}

/// Downconverts an F32 RGBA frame (the engine pipeline's pixel format) to a
/// BGRA8 [`RenderImage`] for the viewers' CPU-frame path. Samples are
/// clamped to `0.0..=1.0` before quantization; `samples` must hold exactly
/// `width * height * 4` values (tightly packed rows).
///
/// Shared by the synthetic test pattern and the real engine's rendered
/// frames ([`super::real`]).
pub(crate) fn f32_rgba_to_bgra_image(width: u32, height: u32, samples: &[f32]) -> RenderImage {
	assert_eq!(
		samples.len(),
		(width * height * 4) as usize,
		"F32 RGBA frame must be tightly packed"
	);
	let bytes = f32_rgba_to_bgra_bytes(samples);
	let buffer = image::RgbaImage::from_raw(width, height, bytes).expect("BGRA frame");
	RenderImage::new(smallvec::SmallVec::from_elem(image::Frame::new(buffer), 1))
}

pub(crate) fn lazy_gpu_f32_rgba_to_bgra_image(
	width: u32,
	height: u32,
	frame: quercus_engine_rs::preview_video::VideoFrame,
) -> RenderImage {
	RenderImage::new_lazy(width, height, move || {
		let samples = frame.f32_samples();
		if samples.len() != (width * height * 4) as usize {
			return smallvec::SmallVec::new();
		}
		let bytes = f32_rgba_to_bgra_bytes(&samples);
		let buffer = image::RgbaImage::from_raw(width, height, bytes).expect("BGRA frame");
		smallvec::SmallVec::from_elem(image::Frame::new(buffer), 1)
	})
}

fn f32_rgba_to_bgra_bytes(samples: &[f32]) -> Vec<u8> {
	let mut bytes = Vec::with_capacity(samples.len());
	for i in (0..samples.len()).step_by(4) {
		bytes.push((samples[i + 2].clamp(0.0, 1.0) * 255.0) as u8); // B
		bytes.push((samples[i + 1].clamp(0.0, 1.0) * 255.0) as u8); // G
		bytes.push((samples[i].clamp(0.0, 1.0) * 255.0) as u8); // R
		bytes.push((samples[i + 3].clamp(0.0, 1.0) * 255.0) as u8); // A
	}
	bytes
}

/// Wraps a BGRA8 pixel block into the viewers' display image (M15 S2
/// zero-copy onscreen path). The bytes come straight from a worker's
/// shared-memory slot (already in display order) — this is the
/// GPU-upload staging buffer, the single permitted main-process copy on
/// the preview path (design §3.5). Returns `None` when `bytes` is
/// shorter than `width * height * 4`.
pub(crate) fn bgra_bytes_to_render_image(
	width: u32,
	height: u32,
	bytes: &[u8],
) -> Option<RenderImage> {
	let need = (width * height * 4) as usize;
	if bytes.len() < need {
		return None;
	}
	let buffer = image::RgbaImage::from_raw(width, height, bytes[..need].to_vec())?;
	Some(RenderImage::new(smallvec::SmallVec::from_elem(
		image::Frame::new(buffer),
		1,
	)))
}

/// Packs F32 RGBA samples (the engine pipeline's pixel format) into the
/// half-float RGBA16F texture bytes of the 10-bit display path (the viewer
/// uploads these into a GPU texture and samples it straight to a 10-bit
/// swapchain — no 8-bit quantization in between). The 10-bit code step
/// `1/1023 ≈ 0.000977` is resolvable in f16 (whose ULP is ~0.000977 at
/// 1.0), so every displayable code survives the round trip. Samples must
/// hold exactly `width * height * 4` values (tightly packed rows); values
/// clamp to `0.0..=1.0` before packing.
///
/// Returns `(bytes, bytes_per_row)`: rows are padded to wgpu's
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256) because `write_texture` requires
/// an explicit, aligned row pitch for multi-row copies — a `None` pitch
/// fails validation and the texture stays black.
#[cfg(test)]
pub(crate) fn f32_rgba_to_16f_bytes(width: u32, height: u32, samples: &[f32]) -> Option<(Vec<u8>, u32)> {
		if samples.len() != (width * height * 4) as usize {
			return None;
		}
		let row_samples = (width * 4) as usize;
		let unpadded_row_bytes = row_samples * 2;
		let row_bytes = unpadded_row_bytes.next_multiple_of(256);
		let mut bytes = vec![0u8; row_bytes * height as usize];
		for (row, chunk) in samples.chunks_exact(row_samples).enumerate() {
			let dst = &mut bytes[row * row_bytes..row * row_bytes + unpadded_row_bytes];
			for (i, &v) in chunk.iter().enumerate() {
				let h = half::f16::from_f32(v.clamp(0.0, 1.0));
				dst[i * 2..i * 2 + 2].copy_from_slice(&h.to_bits().to_le_bytes());
			}
		}
		Some((bytes, row_bytes as u32))
	}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn f32_rgba_converts_to_bgra_bytes() {
		// One red-ish pixel and one pixel exercising clamping.
		let samples = [1.0, 0.0, 0.5, 1.0, 2.0, -1.0, 0.25, 1.0];
		let image = f32_rgba_to_bgra_image(2, 1, &samples);
		let frame = image.as_bytes(0).expect("one frame"); // BGRA8, tightly packed
		assert_eq!(&frame[0..4], &[127, 0, 255, 255]);
		assert_eq!(&frame[4..8], &[63, 0, 255, 255]);
	}

	#[test]
	fn f32_round_trips_to_16f_without_8bit_quantization() {
		// Two adjacent 10-bit codes near white: `v1 = 1022/1023` and
		// `v2 = 1023/1023 = 1.0`. The 16f round trip keeps them on distinct
		// codes (the f16 grid at 1.0 is 2^-10, ~the 10-bit step 1/1023, so
		// the codes align), while the 8-bit path (×255) collapses both to the
		// same code 255.
		let (v1, v2) = (1022.0f32 / 1023.0, 1.0f32);
		let samples = [v1, 0.0, 0.0, 1.0, v2, 0.0, 0.0, 1.0];
		let (bytes, row_bytes) = f32_rgba_to_16f_bytes(2, 1, &samples).expect("packed 16f bytes");
		// Two pixels = 16 content bytes; the row pads to the 256-byte
		// copy alignment.
		assert_eq!(bytes.len(), 256, "one padded row");
		assert_eq!(row_bytes, 256);
		let code_of = |value: f32| {
			let h = half::f16::from_f32(value);
			((h.to_f32() * 1023.0).round()) as u32
		};
		assert_eq!(
			code_of(v1),
			1022,
			"v1 stays on 10-bit code 1022 after the 16f round trip"
		);
		assert_eq!(code_of(v2), 1023, "white stays on code 1023");
		assert_ne!(
			code_of(v1),
			code_of(v2),
			"adjacent 10-bit codes near white stay distinct through 16f"
		);
		assert_eq!(
			(v1 * 255.0).round() as u32,
			(v2 * 255.0).round() as u32,
			"the same two values collapse in 8-bit — proving the 16f path carries the resolution"
		);
	}

	#[test]
	fn f32_to_16f_packs_multi_row_with_padding() {
		// A 3x2 frame: unpadded row is 3*4*2 = 24 bytes, padded to 256.
		let samples = vec![0.5f32; 3 * 2 * 4];
		let (bytes, row_bytes) = f32_rgba_to_16f_bytes(3, 2, &samples).expect("packed");
		assert_eq!(row_bytes, 256);
		assert_eq!(bytes.len(), 512);
		// First pixel of each row carries the 0.5 code; padding is zero.
		let half_bits = half::f16::from_f32(0.5).to_bits();
		for row in 0..2 {
			let off = row * 256;
			assert_eq!(&bytes[off..off + 2], &half_bits.to_le_bytes());
			assert_eq!(&bytes[off + 24..off + 28], &[0, 0, 0, 0], "padding zeroed");
		}
	}

	#[test]
	fn lazy_render_image_defers_and_caches_cpu_materialization() {
		let loads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let loader_loads = loads.clone();
		let image = RenderImage::new_lazy(2, 1, move || {
			loader_loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
			let buffer = image::RgbaImage::from_raw(2, 1, vec![7; 8]).unwrap();
			smallvec::SmallVec::from_elem(image::Frame::new(buffer), 1)
		});
		assert_eq!(image.size(0).width.0, 2);
		assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 0);
		assert_eq!(image.as_bytes(0), Some(&[7; 8][..]));
		assert_eq!(image.as_bytes(0), Some(&[7; 8][..]));
		assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
		assert_ne!(image.id, RenderImage::new(smallvec::SmallVec::new()).id);
	}

	#[test]
	fn f32_upload_borrows_aligned_rows_and_pads_unaligned_rows() {
		let aligned = [0.5f32; 16 * 4];
		let (bytes, pitch) = f32_rgba_to_32f_bytes(16, 1, &aligned).unwrap();
		assert_eq!(pitch, 256);
		assert!(matches!(bytes, Cow::Borrowed(_)));
		let unaligned = [0.25f32; 3 * 4];
		let (bytes, pitch) = f32_rgba_to_32f_bytes(3, 1, &unaligned).unwrap();
		assert_eq!(pitch, 256);
		assert_eq!(&bytes[..48], unaligned_as_bytes(&unaligned).as_slice());
		assert!(bytes[48..].iter().all(|byte| *byte == 0));
	}

	fn unaligned_as_bytes(samples: &[f32]) -> Vec<u8> {
		samples
			.iter()
			.flat_map(|sample| sample.to_ne_bytes())
			.collect()
	}
}

pub(crate) fn f32_rgba_to_32f_bytes<'a>(
	width: u32,
	height: u32,
	samples: &'a [f32],
) -> Option<(Cow<'a, [u8]>, u32)> {
	if samples.len() != (width * height * 4) as usize {
		return None;
	}
	let row_bytes = width as usize * 16;
	let pitch = row_bytes.next_multiple_of(256);
	let raw = unsafe {
		std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 4)
	};
	if row_bytes == pitch {
		return Some((Cow::Borrowed(raw), pitch as u32));
	}
	let mut bytes = vec![0u8; pitch * height as usize];
	for row in 0..height as usize {
		bytes[row * pitch..row * pitch + row_bytes]
			.copy_from_slice(&raw[row * row_bytes..(row + 1) * row_bytes]);
	}
	Some((Cow::Owned(bytes), pitch as u32))
}

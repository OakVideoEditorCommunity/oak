//! Frame data, PNG transport and the golden-frame assertion helpers.
//!
//! The execution layer moves pixels as tight F32 RGBA buffers (the shape
//! `oak-engine-rust-sdk::render::render_frame_f32` returns) and as PNG
//! bytes for the model-facing `render.get_frame` result. Assertions are
//! tolerance-configurable so a golden frame can be compared either
//! bit-exactly (tolerance `0.0`) or statistically.

use std::io::Cursor;

use serde_json::{Value, json};

use crate::error::ToolError;
use crate::util::{base64_encode, fnv1a64};

/// A decoded/constructed frame: `width * height` pixels of RGBA F32 in
/// `[0, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
	/// Width in pixels.
	pub width: u32,
	/// Height in pixels.
	pub height: u32,
	/// Tightly packed RGBA, `width * height * 4` values.
	pub rgba: Vec<f32>,
}

/// Summary statistics of a frame (pixel statistics for assertions and
/// for the compact `render.get_thumbnails` result).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameStats {
	/// Mean of the R/G/B channels (A excluded).
	pub mean: [f64; 3],
	/// Per-channel minimum.
	pub min: [f64; 3],
	/// Per-channel maximum.
	pub max: [f64; 3],
	/// Fraction of pixels that are not exactly black.
	pub non_black_ratio: f64,
	/// FNV-1a over the quantised RGBA bytes (stable across runs).
	pub fnv64: u64,
}

impl Frame {
	/// An all-black frame.
	pub fn black(width: u32, height: u32) -> Frame {
		Frame {
			width,
			height,
			rgba: vec![0.0; (width as usize) * (height as usize) * 4],
		}
	}

	/// Wraps raw RGBA data, checking the length.
	pub fn from_rgba(width: u32, height: u32, rgba: Vec<f32>) -> Result<Frame, ToolError> {
		let expected = (width as usize) * (height as usize) * 4;
		if rgba.len() != expected {
			return Err(ToolError::backend(format!(
				"frame buffer length {} != {expected} for {width}x{height}",
				rgba.len()
			)));
		}
		Ok(Frame {
			width,
			height,
			rgba,
		})
	}

	/// Pixel `(x, y)` as `[r, g, b, a]`.
	pub fn pixel(&self, x: u32, y: u32) -> [f32; 4] {
		let base = ((y as usize * self.width as usize) + x as usize) * 4;
		[
			self.rgba[base],
			self.rgba[base + 1],
			self.rgba[base + 2],
			self.rgba[base + 3],
		]
	}

	/// Sets pixel `(x, y)`.
	pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [f32; 4]) {
		let base = ((y as usize * self.width as usize) + x as usize) * 4;
		self.rgba[base..base + 4].copy_from_slice(&rgba);
	}

	fn quantised(&self) -> Vec<u8> {
		let mut out = Vec::with_capacity(self.rgba.len());
		for &value in &self.rgba {
			let clamped = value.clamp(0.0, 1.0);
			out.push((clamped * 255.0).round() as u8);
		}
		out
	}

	/// Encodes as an 8-bit RGBA PNG.
	pub fn to_png(&self) -> Result<Vec<u8>, ToolError> {
		let bytes = self.quantised();
		let mut buffer = Vec::new();
		let mut encoder = png::Encoder::new(&mut buffer, self.width, self.height);
		encoder.set_color(png::ColorType::Rgba);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder
			.write_header()
			.map_err(|err| ToolError::backend(format!("PNG header write failed: {err}")))?;
		writer
			.write_image_data(&bytes)
			.map_err(|err| ToolError::backend(format!("PNG data write failed: {err}")))?;
		drop(writer);
		Ok(buffer)
	}

	/// Decodes an 8-bit RGBA PNG (the round trip used by the pixel
	/// assertions).
	pub fn from_png(bytes: &[u8]) -> Result<Frame, ToolError> {
		let decoder = png::Decoder::new(Cursor::new(bytes));
		let mut reader = decoder
			.read_info()
			.map_err(|err| ToolError::backend(format!("PNG header read failed: {err}")))?;
		let mut buffer = vec![0u8; reader.output_buffer_size()];
		let info = reader
			.next_frame(&mut buffer)
			.map_err(|err| ToolError::backend(format!("PNG frame read failed: {err}")))?;
		if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
			return Err(ToolError::backend(format!(
				"expected 8-bit RGBA PNG, got {:?}/{:?}",
				info.color_type, info.bit_depth
			)));
		}
		let rgba = buffer[..info.buffer_size()]
			.iter()
			.map(|&b| f32::from(b) / 255.0)
			.collect();
		Frame::from_rgba(info.width, info.height, rgba)
	}

	/// BGRA8 bytes (`render.get_frame`'s non-PNG format).
	pub fn to_bgra8(&self) -> Vec<u8> {
		let bytes = self.quantised();
		let mut out = Vec::with_capacity(bytes.len());
		for pixel in bytes.chunks_exact(4) {
			out.push(pixel[2]);
			out.push(pixel[1]);
			out.push(pixel[0]);
			out.push(pixel[3]);
		}
		out
	}

	/// Pixel statistics (mean / min / max / non-black ratio / hash).
	pub fn stats(&self) -> FrameStats {
		let pixels = (self.width as usize) * (self.height as usize);
		let mut sum = [0.0f64; 3];
		let mut min = [f64::INFINITY; 3];
		let mut max = [f64::NEG_INFINITY; 3];
		let mut non_black = 0usize;
		for index in 0..pixels {
			let base = index * 4;
			let rgb = [
				f64::from(self.rgba[base]),
				f64::from(self.rgba[base + 1]),
				f64::from(self.rgba[base + 2]),
			];
			if rgb.iter().any(|&c| c > 0.0) {
				non_black += 1;
			}
			for channel in 0..3 {
				sum[channel] += rgb[channel];
				min[channel] = min[channel].min(rgb[channel]);
				max[channel] = max[channel].max(rgb[channel]);
			}
		}
		let denom = pixels.max(1) as f64;
		FrameStats {
			mean: [sum[0] / denom, sum[1] / denom, sum[2] / denom],
			min,
			max,
			non_black_ratio: non_black as f64 / denom,
			fnv64: fnv1a64(&self.quantised()),
		}
	}

	/// Largest per-channel absolute difference against `other`.
	///
	/// `Err` when the geometries differ.
	pub fn max_abs_diff(&self, other: &Frame) -> Result<f32, String> {
		if self.width != other.width || self.height != other.height {
			return Err(format!(
				"geometry mismatch: {}x{} vs {}x{}",
				self.width, self.height, other.width, other.height
			));
		}
		let mut worst = 0.0f32;
		for (a, b) in self.rgba.iter().zip(&other.rgba) {
			worst = worst.max((a - b).abs());
		}
		Ok(worst)
	}

	/// Number of pixels whose any channel differs by more than
	/// `tolerance`.
	pub fn differing_pixels(&self, other: &Frame, tolerance: f32) -> Result<usize, String> {
		if self.width != other.width || self.height != other.height {
			return Err(format!(
				"geometry mismatch: {}x{} vs {}x{}",
				self.width, self.height, other.width, other.height
			));
		}
		let mut count = 0usize;
		for (a, b) in self
			.rgba
			.chunks_exact(4)
			.zip(other.rgba.chunks_exact(4))
		{
			let diff = a
				.iter()
				.zip(b)
				.map(|(x, y)| (x - y).abs())
				.fold(0.0f32, f32::max);
			if diff > tolerance {
				count += 1;
			}
		}
		Ok(count)
	}
}

/// Golden-frame assertion: every pixel of `actual` must be within
/// `tolerance` of `expected`.
///
/// Returns a structured description on mismatch (tests turn it into the
/// assertion message), so callers can also log `FrameStats`.
pub fn assert_frames_close(
	actual: &Frame,
	expected: &Frame,
	tolerance: f32,
) -> Result<FrameStats, String> {
	let diff = actual.max_abs_diff(expected)?;
	if diff > tolerance {
		let differing = actual.differing_pixels(expected, tolerance)?;
		return Err(format!(
			"frame differs: max |Δ| = {diff:.6} > tolerance {tolerance:.6} \
			 ({differing} pixels differ); actual stats {:?}, expected stats {:?}",
			actual.stats(),
			expected.stats()
		));
	}
	Ok(actual.stats())
}

/// Compact JSON view of a frame for tool results.
pub fn frame_summary(frame: &Frame, format: &str) -> Value {
	let stats = frame.stats();
	let mut out = json!({
		"format": format,
		"width": frame.width,
		"height": frame.height,
		"fnv64": format!("{:016x}", stats.fnv64),
		"mean_rgb": [
			(stats.mean[0] * 1000.0).round() / 1000.0,
			(stats.mean[1] * 1000.0).round() / 1000.0,
			(stats.mean[2] * 1000.0).round() / 1000.0,
		],
	});
	match format {
		"png" => {
			let png = frame.to_png().expect("PNG encoding");
			out["png_base64"] = json!(base64_encode(&png));
			out["png_bytes"] = json!(png.len());
		}
		"bgra8" => {
			out["bgra8"] = json!(frame.to_bgra8());
		}
		other => {
			out["error"] = json!(format!("unknown frame format '{other}'"));
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn gradient(width: u32, height: u32, shift: f32) -> Frame {
		let mut frame = Frame::black(width, height);
		for y in 0..height {
			for x in 0..width {
				let t = x as f32 / width.max(1) as f32;
				frame.set_pixel(
					x,
					y,
					[(t + shift).clamp(0.0, 1.0), 0.5, y as f32 / height.max(1) as f32, 1.0],
				);
			}
		}
		frame
	}

	#[test]
	fn png_round_trip_is_lossless_after_quantisation() {
		let frame = gradient(17, 9, 0.0);
		let png = frame.to_png().unwrap();
		let back = Frame::from_png(&png).unwrap();
		assert_eq!(back.width, 17);
		assert_eq!(back.height, 9);
		let diff = frame.max_abs_diff(&back).unwrap();
		assert!(diff <= 1.0 / 255.0 + 1e-6, "diff {diff}");
	}

	#[test]
	fn golden_assertion_respects_tolerance() {
		let a = gradient(8, 8, 0.0);
		let mut b = a.clone();
		b.rgba[0] += 0.01;
		assert!(assert_frames_close(&a, &a, 0.0).is_ok());
		assert!(assert_frames_close(&a, &b, 0.5).is_ok());
		let err = assert_frames_close(&a, &b, 0.0).unwrap_err();
		assert!(err.contains("max |Δ|"), "{err}");
	}

	#[test]
	fn geometry_mismatch_is_an_error_not_a_panic() {
		let a = gradient(4, 4, 0.0);
		let b = gradient(5, 4, 0.0);
		assert!(a.max_abs_diff(&b).is_err());
		assert!(assert_frames_close(&a, &b, 1.0).is_err());
	}

	#[test]
	fn stats_track_the_pixels() {
		let frame = gradient(10, 10, 0.0);
		let stats = frame.stats();
		assert!(stats.non_black_ratio > 0.9);
		assert!(stats.max[1] == 0.5);
		assert_ne!(stats.fnv64, gradient(10, 10, 0.1).stats().fnv64);
	}

	#[test]
	fn bgra_layout_swaps_channels() {
		let mut frame = Frame::black(1, 1);
		frame.set_pixel(0, 0, [1.0, 0.0, 0.0, 1.0]);
		assert_eq!(frame.to_bgra8(), vec![0, 0, 255, 255]);
	}
}

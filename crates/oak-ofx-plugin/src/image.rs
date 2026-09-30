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

//! OFX image: an OFX view of a frame buffer (CPU path).
//!
//! Counterpart of the C++ `OliveImage`. Pixel memory is owned by this
//! crate (this was historically a hotspot of memory bugs: ownership
//! must be single, and lifetime is guaranteed by this type).
//! GL texture views are deferred to phase 2 (`// [P2]`).
//!
//! Property writes mirror the C++ `Image::allocate`
//! (image.cpp:132-172) and the HostSupport image property table
//! (HS: ofxhClip.cpp:458-472): Data/RowBytes/Bounds/
//! RegionOfDefinition/Components/PixelDepth/UniqueIdentifier.
//! This struct does not distinguish bounds from ROD (unified), and
//! does not model field/renderScale/pixelAspect/premultiplication
//! (`// [P2]`).

use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::instance::OfxRectD;
use crate::property::{PropertySet, Value};

// ---- OFX property names (macro strings of ofxCore.h / ofxImageEffect.h) ----

/// kOfxImagePropData (ofxCore.h:1275): pixel data pointer.
pub(crate) const K_IMAGE_PROP_DATA: &str = "OfxImagePropData";
/// kOfxImagePropRowBytes (ofxCore.h:1322): row byte count.
pub(crate) const K_IMAGE_PROP_ROW_BYTES: &str = "OfxImagePropRowBytes";
/// kOfxImagePropBounds (ofxCore.h:1291): pixel coordinates, Int x 4.
pub(crate) const K_IMAGE_PROP_BOUNDS: &str = "OfxImagePropBounds";
/// kOfxImagePropRegionOfDefinition (ofxCore.h:1307): pixel coordinates, Int x 4.
pub(crate) const K_IMAGE_PROP_ROD: &str = "OfxImagePropRegionOfDefinition";
/// kOfxImageEffectPropComponents (ofxImageEffect.h:915).
pub(crate) const K_IMAGE_EFFECT_PROP_COMPONENTS: &str = "OfxImageEffectPropComponents";
/// kOfxImageEffectPropPixelDepth (ofxImageEffect.h:901).
pub(crate) const K_IMAGE_EFFECT_PROP_PIXEL_DEPTH: &str = "OfxImageEffectPropPixelDepth";
/// kOfxImagePropUniqueIdentifier (ofxCore.h:927): host-assigned unique id.
pub(crate) const K_IMAGE_PROP_UNIQUE_ID: &str = "OfxImagePropUniqueIdentifier";

/// Process-wide monotonically increasing id (zero-dependency
/// replacement for HostSupport's UUID generation).
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(0);

/// Pixel bit depth (OFX kOfxBitDepth*; the full pipeline only uses
/// Float, the rest are kept for compatibility).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
	/// 8-bit integer (compat).
	Byte,
	/// 16-bit integer (compat).
	Short,
	/// 16-bit half float (compat).
	Half,
	/// 32-bit float (main path).
	Float,
}

impl BitDepth {
	/// Bytes per component.
	pub(crate) fn bytes_per_component(self) -> usize {
		match self {
			BitDepth::Byte => 1,
			BitDepth::Short | BitDepth::Half => 2,
			BitDepth::Float => 4,
		}
	}

	/// OFX bit depth string (kOfxBitDepth*, ofxCore.h:866-880).
	pub(crate) fn to_ofx(self) -> &'static str {
		match self {
			BitDepth::Byte => "OfxBitDepthByte",
			BitDepth::Short => "OfxBitDepthShort",
			BitDepth::Half => "OfxBitDepthHalf",
			BitDepth::Float => "OfxBitDepthFloat",
		}
	}

	/// Parse a kOfxBitDepth* string (the clip-preferences negotiation
	/// value); `None` for unknown depths.
	pub(crate) fn from_ofx(s: &str) -> Option<BitDepth> {
		match s {
			"OfxBitDepthByte" => Some(BitDepth::Byte),
			"OfxBitDepthShort" => Some(BitDepth::Short),
			"OfxBitDepthHalf" => Some(BitDepth::Half),
			"OfxBitDepthFloat" => Some(BitDepth::Float),
			_ => None,
		}
	}
}

/// IEEE 754 half → single (no `half` dependency; subnormals/Inf/NaN
/// follow the standard expansion).
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
	let sign = ((bits >> 15) & 0x1) as u32;
	let exp = ((bits >> 10) & 0x1f) as u32;
	let mant = (bits & 0x3ff) as u32;
	let f32_bits = if exp == 0 {
		if mant == 0 {
			sign << 31
		} else {
			// Subnormal: normalize into the f32 exponent domain.
			let mut m = mant;
			let mut e = 127 - 15;
			while m & 0x400 == 0 {
				m <<= 1;
				e -= 1;
			}
			let m = (m & 0x3ff) << 13;
			(sign << 31) | (((e + 1) as u32) << 23) | m
		}
	} else if exp == 0x1f {
		(sign << 31) | (0xff << 23) | (mant << 13)
	} else {
		(sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
	};
	f32::from_bits(f32_bits)
}

/// Single → IEEE 754 half (round-to-nearest-even; overflow → Inf,
/// NaN/Inf map per the standard).
pub(crate) fn f32_to_f16(value: f32) -> u16 {
	let bits = value.to_bits();
	let sign = ((bits >> 16) & 0x8000) as u16;
	let exp = ((bits >> 23) & 0xff) as i32;
	let mant = bits & 0x7fffff;
	if exp == 255 {
		// Inf/NaN.
		return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
	}
	let e = exp - 127 + 15;
	if e >= 31 {
		return sign | 0x7c00; // overflow → Inf
	}
	if e <= 0 {
		// Half subnormal or zero.
		if e < -10 {
			return sign;
		}
		let mant = mant | 0x800000;
		let shift = (14 - e) as u32;
		let mut half = (mant >> shift) as u16;
		let halfway = 1u32 << (shift - 1);
		if mant & halfway != 0 && ((half & 1) == 1 || mant & (halfway - 1) != 0) {
			half += 1;
		}
		return sign | half;
	}
	let mut half = ((e as u16) << 10) | ((mant >> 13) as u16);
	let rem = mant & 0x1fff;
	if rem > 0x1000 || (rem == 0x1000 && (half & 1) == 1) {
		half += 1;
	}
	sign | half
}

/// Component layout (OFX kOfxImageComponent*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Components {
	/// RGBA.
	Rgba,
	/// RGB.
	Rgb,
	/// Single-channel Alpha.
	Alpha,
}

impl Components {
	/// Channel count.
	pub(crate) fn channel_count(self) -> usize {
		match self {
			Components::Rgba => 4,
			Components::Rgb => 3,
			Components::Alpha => 1,
		}
	}

	/// OFX component string (kOfxImageComponent*, ofxImageEffect.h:46-55).
	pub(crate) fn to_ofx(self) -> &'static str {
		match self {
			Components::Rgba => "OfxImageComponentRGBA",
			Components::Rgb => "OfxImageComponentRGB",
			Components::Alpha => "OfxImageComponentAlpha",
		}
	}
}

/// A single frame. `data` is row-major; the row stride may be padded
/// for alignment; bounds are in pixel coordinates.
/// `#[repr(C)]` with `props` at offset 0 (handle convention,
/// see [`crate::suites::tag`]).
#[repr(C)]
pub struct Image {
	/// Image-level properties (bounds, row bytes, depth, components,
	/// unique identifier).
	pub props: PropertySet,
	/// Pixel buffer (length = row_bytes * height).
	data: Vec<u8>,
	/// Bit depth.
	depth: BitDepth,
	/// Components.
	components: Components,
	/// Pixel bounds.
	bounds: OfxRectD,
	/// Row byte count.
	row_bytes: usize,
}

impl Image {
	/// Allocate by format (uninitialized pixels).
	///
	/// Property writes mirror the C++ `Image::allocate`
	/// (image.cpp:156-172): Data/RowBytes/Bounds/RegionOfDefinition/
	/// Components/PixelDepth; UniqueIdentifier is also written per the
	/// image property table (HS: ofxhClip.cpp:470).
	/// ROD is unified with bounds (this struct does not distinguish
	/// the two; both are pixel coordinates). A zero-size or inverted
	/// rectangle yields an empty buffer (mirrors the
	/// `buffer_size < 0 -> 0` guard in image.cpp:147-149).
	pub fn allocate(depth: BitDepth, components: Components, bounds: OfxRectD) -> Self {
		let (w, h) = {
			let width = (bounds.x2 - bounds.x1).round();
			let height = (bounds.y2 - bounds.y1).round();
			if width > 0.0 && height > 0.0 {
				(width as usize, height as usize)
			} else {
				(0, 0)
			}
		};

		let row_bytes = w * components.channel_count() * depth.bytes_per_component();

		let mut img = Self {
			props: PropertySet::new(),
			data: vec![0u8; row_bytes * h],
			depth,
			components,
			bounds,
			row_bytes,
		};

		// `data` is a heap buffer; moving/borrowing the vec does not move
		// the buffer, and it is never resized after allocation, so the
		// pointer stays valid for the whole Image lifetime (ownership
		// discipline: this is the only place that holds the buffer).
		img.props.define(
			K_IMAGE_PROP_DATA,
			vec![Value::Pointer(img.data.as_mut_ptr() as *mut c_void)],
		);
		img.props
			.define(K_IMAGE_PROP_ROW_BYTES, vec![Value::Int(row_bytes as i32)]);
		let b = |v: f64| Value::Int(v.round() as i32);
		img.props.define(
			K_IMAGE_PROP_BOUNDS,
			vec![b(bounds.x1), b(bounds.y1), b(bounds.x2), b(bounds.y2)],
		);
		img.props.define(
			K_IMAGE_PROP_ROD,
			vec![b(bounds.x1), b(bounds.y1), b(bounds.x2), b(bounds.y2)],
		);
		img.props.define(
			K_IMAGE_EFFECT_PROP_COMPONENTS,
			vec![Value::String(CString::new(components.to_ofx()).unwrap())],
		);
		img.props.define(
			K_IMAGE_EFFECT_PROP_PIXEL_DEPTH,
			vec![Value::String(CString::new(depth.to_ofx()).unwrap())],
		);
		img.props.define(
			K_IMAGE_PROP_UNIQUE_ID,
			vec![Value::String(unique_identifier())],
		);
		// OfxPropType="OfxTypeImage"：图像实例的类型标识（支持库
		// validateImageBaseProperties 的必备项，带可校验默认值）。
		img.props.define(
			"OfxPropType",
			vec![Value::String(std::ffi::CString::new("OfxTypeImage").unwrap())],
		);
		// OFX 必备图像属性（支持库 ImageBase/Image 构造的无默认值强读；
		// 缺失即抛 PropertyUnknownToHost → MissingHostFeature 紫帧）：
		// 方形像素 1.0；预乘声明（本管线按预乘 alpha 处理）；无场。
		img.props.define(
			"OfxImagePropPixelAspectRatio",
			vec![Value::Double(1.0)],
		);
		img.props.define(
			"OfxImageEffectPropPreMultiplication",
			// kOfxImagePreMultiplied 的真实字符串值是
			// "OfxImageAlphaPremultiplied"（ofxImageEffect.h），
			// ofxs mapStrToPreMultiplicationEnum 只认这三个精确值。
			vec![Value::String(
				std::ffi::CString::new("OfxImageAlphaPremultiplied").unwrap(),
			)],
		);
		img.props.define(
			"OfxImagePropField",
			vec![Value::String(std::ffi::CString::new("OfxFieldNone").unwrap())],
		);
		// OfxImageEffectPropRenderScale：openfx-misc 的
		// checkBadRenderScaleOrField 用它比对渲染参数（1:1）。
		img.props.define(
			"OfxImageEffectPropRenderScale",
			vec![Value::Double(1.0), Value::Double(1.0)],
		);

		img
	}

	/// Mutable pixel slice (for writing plugin output). The length is
	/// consistent with the format by type construction.
	pub fn pixels_mut(&mut self) -> &mut [u8] {
		&mut self.data
	}

	/// Read-only pixel slice.
	pub fn pixels(&self) -> &[u8] {
		&self.data
	}

	/// Pixel bounds (canonical coordinates; the Image keeps bounds and
	/// ROD unified).
	pub fn bounds(&self) -> OfxRectD {
		self.bounds
	}

	/// Bit depth.
	pub fn depth(&self) -> BitDepth {
		self.depth
	}

	/// Components.
	pub fn components(&self) -> Components {
		self.components
	}

	/// Row byte count.
	pub fn row_bytes(&self) -> usize {
		self.row_bytes
	}

	/// Convert the pixel buffer to another bit depth. The pipeline's
	/// working format is ACEScg + F32 end to end; an OFX plugin that
	/// negotiates Byte/Short/Half gets its inputs converted down before
	/// the render action and its output converted back afterwards
	/// (values are [0,1]-normalized across depths; same-depth calls just
	/// re-allocate and copy).
	pub(crate) fn convert_depth(&self, depth: BitDepth) -> Image {
		let mut out = Image::allocate(depth, self.components, self.bounds);
		if depth == self.depth {
			out.data.copy_from_slice(&self.data);
			return out;
		}
		let sb = self.depth.bytes_per_component();
		let n = self.data.len() / sb;
		let read = |i: usize| -> f32 {
			let o = i * sb;
			match self.depth {
				BitDepth::Byte => self.data[o] as f32 / 255.0,
				BitDepth::Short | BitDepth::Half => {
					let bits = u16::from_le_bytes([self.data[o], self.data[o + 1]]);
					if self.depth == BitDepth::Half {
						f16_to_f32(bits)
					} else {
						bits as f32 / 65535.0
					}
				}
				BitDepth::Float => f32::from_le_bytes(self.data[o..o + 4].try_into().unwrap()),
			}
		};
		let ob = depth.bytes_per_component();
		let od = &mut out.data;
		for i in 0..n {
			let v = read(i);
			let o = i * ob;
			match depth {
				BitDepth::Byte => od[o] = (v.clamp(0.0, 1.0) * 255.0).round() as u8,
				BitDepth::Short => {
					let q = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
					od[o..o + 2].copy_from_slice(&q.to_le_bytes());
				}
				BitDepth::Half => {
					od[o..o + 2].copy_from_slice(&f32_to_f16(v).to_le_bytes());
				}
				BitDepth::Float => od[o..o + 4].copy_from_slice(&v.to_le_bytes()),
			}
		}
		out
	}
}

/// Unique identifier string (monotonically increasing per process,
/// hexadecimal; corresponds to the `uniqueIdentifier` parameter of
/// HS: ofxhClip.cpp:537).
pub(crate) fn unique_identifier() -> CString {
	let n = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
	// Hexadecimal ASCII, no NUL; unwrap cannot fail.
	CString::new(format!("{:x}", n)).unwrap()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn f32_image(values: &[f32]) -> Image {
		let mut img = Image::allocate(
			BitDepth::Float,
			Components::Alpha,
			crate::instance::OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: values.len() as f64,
				y2: 1.0,
			},
		);
		for (i, v) in values.iter().enumerate() {
			img.pixels_mut()[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
		}
		img
	}

	fn samples(img: &Image) -> Vec<f32> {
		let f = img.convert_depth(BitDepth::Float);
		f.pixels()
			.chunks_exact(4)
			.map(|c| f32::from_le_bytes(c.try_into().unwrap()))
			.collect()
	}

	/// F32 → U8/U16/F16 → F32 的往返保持 [0,1] 归一化语义（OFX 低位深
	/// 协商插件的输入转低、输出转回路径）。
	#[test]
	fn convert_depth_roundtrips() {
		let src = f32_image(&[0.0, 0.25, 0.5, 1.0]);

		let u8img = src.convert_depth(BitDepth::Byte);
		assert_eq!(u8img.depth(), BitDepth::Byte);
		assert_eq!(u8img.pixels(), &[0, 64, 128, 255]);
		let back = samples(&u8img);
		for (a, b) in back.iter().zip([0.0, 0.25, 0.5, 1.0]) {
			assert!((a - b).abs() < 0.003, "u8 roundtrip: {a} vs {b}");
		}

		let u16img = src.convert_depth(BitDepth::Short);
		assert_eq!(u16img.depth(), BitDepth::Short);
		let back = samples(&u16img);
		for (a, b) in back.iter().zip([0.0, 0.25, 0.5, 1.0]) {
			assert!((a - b).abs() < 0.0001, "u16 roundtrip: {a} vs {b}");
		}

		let f16img = src.convert_depth(BitDepth::Half);
		assert_eq!(f16img.depth(), BitDepth::Half);
		let back = samples(&f16img);
		for (a, b) in back.iter().zip([0.0, 0.25, 0.5, 1.0]) {
			assert!((a - b).abs() < 0.001, "f16 roundtrip: {a} vs {b}");
		}

		// 同深度 = 重新分配 + 拷贝（props 的数据指针必须指向新缓冲）。
		let same = src.convert_depth(BitDepth::Float);
		assert_eq!(same.pixels(), src.pixels());
		assert!(!std::ptr::eq(same.pixels().as_ptr(), src.pixels().as_ptr()));
	}

	/// f16 转换的边界：零/次规格数/Inf/NaN。
	#[test]
	fn f16_edges() {
		assert_eq!(f32_to_f16(0.0), 0);
		assert_eq!(f16_to_f32(0), 0.0);
		assert!(f16_to_f32(f32_to_f16(1.0)) == 1.0);
		assert!(f16_to_f32(f32_to_f16(f32::INFINITY)).is_infinite());
		assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
		// 超 half 范围 → Inf。
		assert!(f16_to_f32(f32_to_f16(1e10)).is_infinite());
	}
}

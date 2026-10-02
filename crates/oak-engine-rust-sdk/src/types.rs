//! Safe wrapper over the engine's pure-value-type C ABI
//! (`oak_core_rational_*` / `oak_core_timerange_*` / `oak_core_pixelformat_*`).
//!
//! [`Rational`] and [`TimeRange`] are `repr(C)` mirrors that cross by
//! value; their methods call into the engine so C++-compatible reduction
//! and sentinel semantics are never reimplemented on this side.

use std::cmp::Ordering;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_core_rational_new(num: i64, den: i64) -> Rational;
	fn oak_core_rational_null() -> Rational;
	fn oak_core_rational_is_null(r: Rational) -> bool;
	fn oak_core_rational_is_nan(r: Rational) -> bool;
	fn oak_core_rational_to_f64(r: Rational) -> f64;
	fn oak_core_rational_from_f64(value: f64) -> Rational;
	fn oak_core_rational_add(a: Rational, b: Rational) -> Rational;
	fn oak_core_rational_sub(a: Rational, b: Rational) -> Rational;
	fn oak_core_rational_mul(a: Rational, b: Rational) -> Rational;
	fn oak_core_rational_div(a: Rational, b: Rational) -> Rational;
	fn oak_core_rational_cmp(a: Rational, b: Rational) -> i32;
	fn oak_core_rational_to_string(r: Rational, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_core_rational_from_string(s: *const u8, len: usize) -> Rational;
	fn oak_core_rational_time_to_timestamp(timebase: Rational, time: Rational) -> i64;
	fn oak_core_rational_timestamp_to_time(timebase: Rational, ts: i64) -> Rational;

	fn oak_core_timerange_new(in_: Rational, out: Rational) -> TimeRange;
	fn oak_core_timerange_length(r: TimeRange) -> Rational;
	fn oak_core_timerange_contains(r: TimeRange, t: Rational) -> bool;
	fn oak_core_timerange_intersected(a: TimeRange, b: TimeRange) -> TimeRange;
	fn oak_core_timerange_combined(a: TimeRange, b: TimeRange) -> TimeRange;

	fn oak_core_pixelformat_is_valid(format: i32) -> bool;
	fn oak_core_pixelformat_bytes_per_channel(format: i32) -> i32;
	fn oak_core_pixelformat_bytes_per_pixel(format: i32, channels: i32) -> i32;

	fn oak_core_videoparams_default() -> VideoParams;
	fn oak_core_videoparams_is_valid(params: *const VideoParams) -> bool;
	fn oak_core_videoparams_effective_width(params: *const VideoParams) -> i32;
	fn oak_core_videoparams_effective_height(params: *const VideoParams) -> i32;
	fn oak_core_videoparams_bytes_per_pixel(params: *const VideoParams) -> i32;
	fn oak_core_videoparams_buffer_size(params: *const VideoParams) -> i32;
}

/// A rational number, always reduced with a non-negative denominator;
/// `0/0` is the null/invalid sentinel. Layout matches the engine's
/// `OakRational`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rational {
	pub num: i64,
	pub den: i64,
}

impl Rational {
	/// The null/invalid sentinel (0/0).
	pub const NULL: Rational = Rational { num: 0, den: 0 };

	/// A reduced `num/den` rational (0/0 when `den` is 0). Reduction runs
	/// in the engine, so the result is bit-identical to oak-core's.
	pub fn new(num: i64, den: i64) -> Rational {
		unsafe { oak_core_rational_new(num, den) }
	}

	pub fn is_null(self) -> bool {
		unsafe { oak_core_rational_is_null(self) }
	}

	/// True for the NaN sentinel (produced by overflowed arithmetic).
	pub fn is_nan(self) -> bool {
		unsafe { oak_core_rational_is_nan(self) }
	}

	pub fn to_f64(self) -> f64 {
		unsafe { oak_core_rational_to_f64(self) }
	}

	/// The rational closest to `value` within oak-core's precision.
	pub fn from_f64(value: f64) -> Rational {
		unsafe { oak_core_rational_from_f64(value) }
	}

	/// The project-XML text form (e.g. "30000/1001").
	pub fn to_display_string(self) -> String {
		crate::codec::string_out(|buf, len| unsafe {
			oak_core_rational_to_string(self, buf, len)
		})
		.unwrap_or_default()
	}

	/// Parses the text form; invalid input yields the null sentinel.
	pub fn from_display_string(s: &str) -> Rational {
		unsafe { oak_core_rational_from_string(s.as_ptr(), s.len()) }
	}

	/// Rational `time` as an integer timestamp in `self` as timebase.
	pub fn time_to_timestamp(self, time: Rational) -> i64 {
		unsafe { oak_core_rational_time_to_timestamp(self, time) }
	}

	/// Integer timestamp `ts` back to rational time in `self` as
	/// timebase.
	pub fn timestamp_to_time(self, ts: i64) -> Rational {
		unsafe { oak_core_rational_timestamp_to_time(self, ts) }
	}
}

impl std::ops::Add for Rational {
	type Output = Rational;
	fn add(self, rhs: Rational) -> Rational {
		unsafe { oak_core_rational_add(self, rhs) }
	}
}

impl std::ops::Sub for Rational {
	type Output = Rational;
	fn sub(self, rhs: Rational) -> Rational {
		unsafe { oak_core_rational_sub(self, rhs) }
	}
}

impl std::ops::Mul for Rational {
	type Output = Rational;
	fn mul(self, rhs: Rational) -> Rational {
		unsafe { oak_core_rational_mul(self, rhs) }
	}
}

impl std::ops::Div for Rational {
	type Output = Rational;
	fn div(self, rhs: Rational) -> Rational {
		unsafe { oak_core_rational_div(self, rhs) }
	}
}

impl PartialOrd for Rational {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Rational {
	fn cmp(&self, other: &Self) -> Ordering {
		match unsafe { oak_core_rational_cmp(*self, *other) } {
			-1 => Ordering::Less,
			0 => Ordering::Equal,
			_ => Ordering::Greater,
		}
	}
}

/// A half-open time range [in, out) (normalized so `in <= out`). Layout
/// matches the engine's `OakTimeRange`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TimeRange {
	/// Inclusive start.
	pub in_: Rational,
	/// Exclusive end.
	pub out: Rational,
}

impl TimeRange {
	/// A normalized range: the endpoints are swapped when `out < in`.
	pub fn new(in_: Rational, out: Rational) -> TimeRange {
		unsafe { oak_core_timerange_new(in_, out) }
	}

	/// `out - in`.
	pub fn length(&self) -> Rational {
		unsafe { oak_core_timerange_length(*self) }
	}

	/// True when `t` lies in [in, out).
	pub fn contains(&self, t: Rational) -> bool {
		unsafe { oak_core_timerange_contains(*self, t) }
	}

	/// The overlap of two ranges (empty when disjoint).
	pub fn intersected(&self, other: &TimeRange) -> TimeRange {
		unsafe { oak_core_timerange_intersected(*self, *other) }
	}

	/// The smallest range covering both inputs.
	pub fn combined(&self, other: &TimeRange) -> TimeRange {
		unsafe { oak_core_timerange_combined(*self, *other) }
	}
}

/// Pixel formats (values identical to oak-core's `PixelFormat`, carried
/// as i32 over the ABI).
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
	Invalid = -1,
	U8 = 0,
	U10 = 1,
	U16 = 2,
	F16 = 3,
	F32 = 4,
}

impl PixelFormat {
	/// The discriminant for a raw ABI value; `None` outside -1..=4.
	pub fn from_i32(v: i32) -> Option<PixelFormat> {
		if unsafe { oak_core_pixelformat_is_valid(v) } {
			Some(unsafe { std::mem::transmute::<i32, PixelFormat>(v) })
		} else {
			None
		}
	}

	pub fn bytes_per_channel(self) -> i32 {
		unsafe { oak_core_pixelformat_bytes_per_channel(self as i32) }
	}

	pub fn bytes_per_pixel(self, channels: i32) -> i32 {
		unsafe { oak_core_pixelformat_bytes_per_pixel(self as i32, channels) }
	}
}

// ---------------------------------------------------------------------------
// VideoParams
// ---------------------------------------------------------------------------

/// A video parameter set. Layout matches the engine's `OakVideoParams`;
/// enums cross as their discriminant ints (`format` is a
/// [`PixelFormat`] code, `interlacing` 0/1/2 = none/top/bottom,
/// `video_type` 0/1/2 = video/still/sequence, `color_range` 0/1 =
/// limited/full, bools as 0/1).
#[repr(C)]
#[derive(Clone, Debug)]
pub struct VideoParams {
	pub width: i32,
	pub height: i32,
	pub depth: i32,
	pub time_base_num: i32,
	pub time_base_den: i32,
	pub frame_rate_num: i32,
	pub frame_rate_den: i32,
	pub pixel_aspect_num: i32,
	pub pixel_aspect_den: i32,
	pub format: i32,
	pub channel_count: i32,
	pub interlacing: i32,
	pub divider: i32,
	pub enabled: i32,
	pub x: f32,
	pub y: f32,
	pub stream_index: i32,
	pub video_type: i32,
	pub start_time: i64,
	pub duration: i64,
	pub premultiplied_alpha: i32,
	pub color_range: i32,
	pub color_primaries: i32,
	pub color_transfer: i32,
	/// Colorspace name, NUL-terminated (63 bytes max).
	colorspace: [u8; 64],
}

/// Why a [`VideoParams::set_colorspace`] call was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColorspaceError {
	/// Longer than the fixed 64-byte field allows (63 bytes + NUL).
	#[error("colorspace name too long: {0:?} (max 63 bytes)")]
	TooLong(String),
	/// C strings cannot contain NUL.
	#[error("colorspace name contains a NUL byte: {0:?}")]
	InteriorNul(String),
}

impl VideoParams {
	/// The default (invalid) parameter set.
	pub fn new() -> Self {
		unsafe { oak_core_videoparams_default() }
	}

	/// Whether the parameter set describes a valid video stream.
	pub fn is_valid(&self) -> bool {
		unsafe { oak_core_videoparams_is_valid(self) }
	}

	/// The width after applying the resolution divider.
	pub fn effective_width(&self) -> i32 {
		unsafe { oak_core_videoparams_effective_width(self) }
	}

	/// The height after applying the resolution divider.
	pub fn effective_height(&self) -> i32 {
		unsafe { oak_core_videoparams_effective_height(self) }
	}

	/// Bytes per pixel for this parameter set.
	pub fn bytes_per_pixel(&self) -> i32 {
		unsafe { oak_core_videoparams_bytes_per_pixel(self) }
	}

	/// Total frame buffer size in bytes.
	pub fn buffer_size(&self) -> i32 {
		unsafe { oak_core_videoparams_buffer_size(self) }
	}

	/// The colorspace name ("" when unset).
	pub fn colorspace(&self) -> &str {
		let end = self
			.colorspace
			.iter()
			.position(|&b| b == 0)
			.unwrap_or(self.colorspace.len());
		std::str::from_utf8(&self.colorspace[..end]).unwrap_or("")
	}

	/// Sets the colorspace name (validated: 63 bytes max, no NUL).
	pub fn set_colorspace(&mut self, value: &str) -> Result<(), ColorspaceError> {
		if value.len() > 63 {
			return Err(ColorspaceError::TooLong(value.to_string()));
		}
		let bytes = value.as_bytes();
		if bytes.contains(&0) {
			return Err(ColorspaceError::InteriorNul(value.to_string()));
		}
		self.colorspace.fill(0);
		self.colorspace[..bytes.len()].copy_from_slice(bytes);
		Ok(())
	}
}

impl Default for VideoParams {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rational_reduce_ops_and_sentinels() {
		assert_eq!(Rational::new(2, 4), Rational { num: 1, den: 2 });
		assert_eq!(Rational::new(1, -2), Rational { num: -1, den: 2 });
		assert!(Rational::NULL.is_null());
		assert!(!Rational::new(1, 2).is_nan());

		assert_eq!(Rational::new(1, 2) + Rational::new(1, 3), Rational::new(5, 6));
		assert_eq!(Rational::new(1, 2) * Rational::new(2, 1), Rational::new(1, 1));
		assert!(Rational::new(1, 2) < Rational::new(2, 3));

		let nan = Rational::new(i32::MAX as i64, 1) + Rational::new(i32::MAX as i64, 1);
		assert!(nan.is_nan());
	}

	#[test]
	fn rational_string_f64_and_timebase() {
		let rate = Rational::new(30000, 1001);
		assert_eq!(rate.to_display_string(), "30000/1001");
		assert_eq!(Rational::from_display_string("30000/1001"), rate);
		assert!(Rational::from_display_string("nope").is_null());

		assert_eq!(Rational::new(1, 2).to_f64(), 0.5);
		assert_eq!(Rational::from_f64(0.5), Rational::new(1, 2));

		let timebase = Rational::new(1, 25);
		let ts = timebase.time_to_timestamp(Rational::new(2, 1));
		assert_eq!(ts, 50);
		assert_eq!(timebase.timestamp_to_time(ts), Rational::new(2, 1));
	}

	#[test]
	fn timerange_semantics() {
		let range = TimeRange::new(Rational::new(10, 1), Rational::new(5, 1));
		assert_eq!(range.in_, Rational::new(5, 1));
		assert_eq!(range.out, Rational::new(10, 1));
		assert_eq!(range.length(), Rational::new(5, 1));
		assert!(range.contains(Rational::new(5, 1)));
		assert!(!range.contains(Rational::new(10, 1)));

		let other = TimeRange::new(Rational::new(7, 1), Rational::new(12, 1));
		assert_eq!(range.intersected(&other).length(), Rational::new(3, 1));
		assert_eq!(range.combined(&other).length(), Rational::new(7, 1));
	}

	#[test]
	fn pixelformat_roundtrip() {
		assert_eq!(PixelFormat::from_i32(4), Some(PixelFormat::F32));
		assert_eq!(PixelFormat::from_i32(-1), Some(PixelFormat::Invalid));
		assert_eq!(PixelFormat::from_i32(5), None);

		assert_eq!(PixelFormat::F32.bytes_per_channel(), 4);
		assert_eq!(PixelFormat::F32.bytes_per_pixel(4), 16);
		assert_eq!(PixelFormat::U8.bytes_per_channel(), 1);
	}

	#[test]
	fn videoparams_defaults_and_computed() {
		let mut p = VideoParams::new();
		assert!(!p.is_valid());
		assert_eq!(p.divider, 1);

		p.width = 1920;
		p.height = 1080;
		p.format = PixelFormat::F32 as i32;
		p.channel_count = 4;
		p.time_base_num = 1;
		p.time_base_den = 25;
		p.set_colorspace("acescg").unwrap();
		assert_eq!(p.colorspace(), "acescg");
		assert!(p.set_colorspace(&"x".repeat(64)).is_err());
		assert!(p.set_colorspace("a\0b").is_err());

		assert!(p.is_valid());
		p.divider = 2;
		assert_eq!(p.effective_width(), 960);
		assert_eq!(p.effective_height(), 540);
		assert!(p.buffer_size() > 0);
	}
}

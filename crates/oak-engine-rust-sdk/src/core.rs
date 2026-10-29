//! oak-core's SDK surface (the engine's `oak_core_*` ABI), one stop.
//!
//! oak-core is the shared foundation crate: value types (rational time,
//! video params, pixel formats), the process-wide config store, color
//! pipeline state, color math, file utilities, LUT plumbing, and the
//! cooperative cancellation token. In this SDK its functions live in
//! topic modules; this module re-exports them so consumers can use
//! `core::` paths exclusively:
//!
//! | topic | module | contents |
//! |---|---|---|
//! | value types | [`types`] | `Rational`/`TimeRange` arithmetic + timestamp conversion, `VideoParams`, `PixelFormat` |
//! | process state | [`state`] | the config store, color pipeline settings, display ICC probing, `CancelAtom` |
//! | utilities | [`utils`] | color math (transfer functions, primaries, chromatic adaptation), file helpers, LUT display plumbing |
//! | color processing | [`color`] | `ColorProcessor`/`ColorTransform` handles (OCIO-backed conversion) |
//!
//! Platform note: the display-ICC entry points are gated per platform
//! (`windows`/`linux`) on both sides of the ABI.

pub use crate::color;
pub use crate::state;
pub use crate::types;
pub use crate::utils;

// ---------------------------------------------------------------------------
// Constants (oak-core's pub consts — compile-time values, no FFI)
// ---------------------------------------------------------------------------

/// oak-core's error codes (`OAKCORE_*`).
pub const OK: i32 = 0;
pub const E_INVALID: i32 = -10001;
pub const E_STATE: i32 = -10002;
pub const E_FAILED: i32 = -10003;
pub const E_NOT_FOUND: i32 = -10004;
pub const E_NOMEM: i32 = -10005;

/// The config-store key for the on-screen presentation bit depth.
pub const CONFIG_KEY_DISPLAY_BIT_DEPTH: &str = "DisplayBitDepth";

/// On-screen presentation bit depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayBitDepth {
	Bit8,
	Bit10,
}

impl DisplayBitDepth {
	/// Parses the settings string; unknown values select the 10-bit default.
	pub fn from_config_string(value: &str) -> Self {
		match value.trim() {
			"8" => Self::Bit8,
			_ => Self::Bit10,
		}
	}

	/// Returns the persisted settings value.
	pub fn to_config_string(self) -> &'static str {
		match self {
			Self::Bit8 => "8",
			Self::Bit10 => "10",
		}
	}
}

/// The project's output gamut and transfer settings as owned strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputColorSpec {
	pub gamut: String,
	pub transfer: String,
}

impl Default for OutputColorSpec {
	fn default() -> Self {
		Self {
			gamut: "srgb".into(),
			transfer: "srgb".into(),
		}
	}
}

/// Returns the current project output specification.
pub fn pipeline_output_spec() -> OutputColorSpec {
	OutputColorSpec {
		gamut: crate::state::color_output_gamut(),
		transfer: crate::state::color_output_transfer(),
	}
}

/// The minimum decibel level (the logarithmic slider floor).
pub const DECIBEL_MINIMUM: f64 = -200.0;

/// The default render size.
pub const DEFAULT_WIDTH: i32 = 1920;
pub const DEFAULT_HEIGHT: i32 = 1080;

/// The pipeline's internal channel count (RGBA F32).
pub const INTERNAL_CHANNEL_COUNT: i32 = 4;

/// RGB/RGBA channel counts (flattened from `VideoParams`).
pub const RGB_CHANNEL_COUNT: i32 = 3;
pub const RGBA_CHANNEL_COUNT: i32 = 4;

/// The display LUT's edge size (65³ grid).
pub const DISPLAY_EDGE: u32 = 65;
/// The display LUT's input domain: working-space scene-linear values.
pub const DISPLAY_LO: [f32; 3] = [-0.25, -0.25, -0.25];
pub const DISPLAY_HI: [f32; 3] = [4.0, 4.0, 4.0];

/// The supported LUT file extensions (dot-less, case-insensitive).
pub const SUPPORTED_LUT_EXTENSIONS: [&str; 9] = [
	"cube", "3dl", "spi1d", "spi3d", "spimtx", "csp", "clf", "ctf", "cub",
];

/// The D65 white point.
pub const WHITE_D65: utils::Xy = utils::Xy { x: 0.3127, y: 0.3290 };
/// The ACES white point (≈D60; the exact ACES specification values).
pub const WHITE_ACES: utils::Xy = utils::Xy { x: 0.32168, y: 0.33767 };

/// sRGB / Rec.709 primaries (D65).
pub const PRIMARIES_SRGB: utils::Primaries = utils::Primaries {
	red: utils::Xy { x: 0.640, y: 0.330 },
	green: utils::Xy { x: 0.300, y: 0.600 },
	blue: utils::Xy { x: 0.150, y: 0.060 },
	white: WHITE_D65,
};

/// Display P3 primaries (D65).
pub const PRIMARIES_DISPLAY_P3: utils::Primaries = utils::Primaries {
	red: utils::Xy { x: 0.680, y: 0.320 },
	green: utils::Xy { x: 0.265, y: 0.690 },
	blue: utils::Xy { x: 0.150, y: 0.060 },
	white: WHITE_D65,
};

/// Rec.2020 / BT.2020 primaries (D65).
pub const PRIMARIES_BT2020: utils::Primaries = utils::Primaries {
	red: utils::Xy { x: 0.708, y: 0.292 },
	green: utils::Xy { x: 0.170, y: 0.797 },
	blue: utils::Xy { x: 0.131, y: 0.046 },
	white: WHITE_D65,
};

/// ACES AP1 primaries (the ACEScg gamut; ACES white).
pub const PRIMARIES_AP1: utils::Primaries = utils::Primaries {
	red: utils::Xy { x: 0.713, y: 0.293 },
	green: utils::Xy { x: 0.165, y: 0.830 },
	blue: utils::Xy { x: 0.128, y: 0.044 },
	white: WHITE_ACES,
};

#[cfg(test)]
mod tests {
	/// The umbrella paths resolve and reach the engine.
	#[test]
	fn umbrella_paths_work() {
		use super::*;
		let r = types::Rational::new(30000, 1001);
		assert!((r.to_f64() - 29.97).abs() < 1e-3);
		assert!(!state::color_working_space().is_empty());
		assert!(utils::primaries_srgb().red.x > 0.3);
		let atom = state::CancelAtom::new();
		assert!(!atom.is_cancelled());
		// Pass-through carries no OCIO transform (identity conversion).
		let proc = color::ColorProcessor::pass_through();
		assert_eq!(proc.convert_color([0.1, 0.5, 0.9, 1.0]), [0.1, 0.5, 0.9, 1.0]);
	}

	/// The constants match the engine where a live cross-check exists.
	#[test]
	fn constants_match_the_engine() {
		use super::*;
		// The primaries consts equal the engine's function results.
		assert_eq!(PRIMARIES_SRGB, utils::primaries_srgb());
		assert_eq!(PRIMARIES_BT2020, utils::primaries_bt2020());
		assert_eq!(PRIMARIES_DISPLAY_P3, utils::primaries_display_p3());
		assert_eq!(PRIMARIES_AP1, utils::primaries_ap1());
		// The display LUT plumbing matches the engine's.
		assert_eq!(DISPLAY_EDGE, utils::lut_display_edge());
		assert_eq!((DISPLAY_LO, DISPLAY_HI), utils::lut_display_domain());
	}
}

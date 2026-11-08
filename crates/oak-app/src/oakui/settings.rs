// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Temporary app-side adapter for configuration shared by every engine
//! implementation. P0 keeps the implementation here so the public
//! `AppEngine` contract contains no engine-internal types or calls; P1
//! moves these operations behind the engine SDK.

use quercus_engine_rs::codec::ProxyParams as SdkProxyParams;
use quercus_engine_rs::state::Config;

use super::engine::ProxyParamsUi;
use super::real::{config_set_bool, config_set_int};

/// Whether previews use generated proxy media instead of the originals.
///
/// Reads the global "UseProxyMedia" bool config, defaulting to `true` when the
/// key is unset. Preview-only: exports ignore it and decode the original
/// media. The engine also consults it when deciding to generate proxies in the
/// background. Counterpart of [`set_use_proxy_media`].
pub fn use_proxy_media() -> bool {
	Config::global().get_bool("UseProxyMedia", true)
}

/// Store whether previews use generated proxy media.
///
/// Toggles the global "UseProxyMedia" bool config; when on, footage frames are
/// resolved through the generated proxy files instead of the originals. This is
/// preview-only — exports always decode the original media — and proxies must
/// already exist for the switch to have an effect. Counterpart of
/// [`use_proxy_media`].
pub fn set_use_proxy_media(enabled: bool) {
	config_set_bool("UseProxyMedia", enabled);
}

/// The playback-resolution divider, clamped to `1..=8` (default 1 = full
/// preview size).
///
/// 2/4/8 render the preview at progressively smaller sizes for machines that
/// cannot keep up; the setting is preview-only, exports always render native.
/// Stored as a config integer and clamped on read, so a corrupt value cannot
/// yield a zero or negative scale. Counterpart of [`set_playback_divider`].
pub fn playback_divider() -> i64 {
	Config::global().get_i64("PlaybackDivider", 1).clamp(1, 8)
}

/// Store the playback-resolution divider.
///
/// `divider` is clamped to `1..=8` (1 = full preview size, 2/4/8 =
/// progressively smaller preview renders) before being written to the
/// "PlaybackDivider" config, so the engine never sees an invalid scale. This
/// only affects the preview; exports always render at native resolution.
/// Counterpart of [`playback_divider`].
pub fn set_playback_divider(divider: i64) {
	config_set_int("PlaybackDivider", divider.clamp(1, 8));
}

/// Whether playback stops when it reaches the last frame.
///
/// Reads the "StopOnLastFrame" bool config, defaulting to `true` when the key
/// is unset. `false` means playback wraps around instead of pausing.
/// Counterpart of [`set_stop_on_last`].
pub fn stop_on_last() -> bool {
	Config::global().get_bool("StopOnLastFrame", true)
}

/// Store whether playback stops on the last frame of the sequence.
///
/// `enabled == true` (the default) pauses at the last frame; `false` lets
/// playback wrap past the end. Writes the "StopOnLastFrame" bool config, which
/// the next playback tick reads; persisting to disk still needs a separate
/// `config_save`. Counterpart of [`stop_on_last`].
pub fn set_stop_on_last(enabled: bool) {
	config_set_bool("StopOnLastFrame", enabled);
}

/// The viewer's audio-waveform overlay mode, clamped to `0..=2`.
///
/// 0 = automatic (overlay only when relevant), 1 = only on the active
/// monitor, 2 = on both monitors. Reads the "ViewerWaveformMode" config
/// integer, defaulting to 0 when unset; a corrupt or out-of-range value is
/// clamped, so this always returns a valid mode. Counterpart of
/// [`set_waveform_mode`].
pub fn waveform_mode() -> i32 {
	Config::global().get_i64("ViewerWaveformMode", 0).clamp(0, 2) as i32
}

/// Store the viewer's audio-waveform overlay mode.
///
/// `mode` is clamped to `0..=2` (0 = automatic, 1 = only the active monitor,
/// 2 = both monitors) before it is written to the "ViewerWaveformMode" config,
/// so an out-of-range value can never reach the viewer. The write is
/// fire-and-forget; persist it separately with `config_save` to survive a
/// restart. Counterpart of [`waveform_mode`].
pub fn set_waveform_mode(mode: i32) {
	config_set_int("ViewerWaveformMode", mode.clamp(0, 2) as i64);
}

/// The default color-management roles for a new project, as
/// `(config, display, view)`.
///
/// Returns `("acescg", "srgb", "srgb")`: ACEScg as the working space and an
/// sRGB display with the sRGB view transform. Used to seed project settings
/// when the user has not picked an OCIO config; it takes no input and always
/// succeeds.
pub fn default_project_color_settings() -> (String, String, String) {
	(
		"acescg".to_string(),
		"srgb".to_string(),
		"srgb".to_string(),
	)
}

/// The proxy-generation parameters currently stored in the global config.
///
/// Repackages the keys read by `ProxyParams::from_config` as the UI-facing
/// [`ProxyParamsUi`]: absolute `width`/`height` (ignored when `divider > 1`),
/// source `divider`, x264 `crf`, the ffmpeg `preset` name ("" when the SDK
/// reports none) and `include_audio` (the SDK's non-zero C flag turned into a
/// `bool`). Values are passed through unvalidated; this only seeds the proxy
/// dialog.
pub fn proxy_params_from_config() -> ProxyParamsUi {
	let params = SdkProxyParams::from_config();
	ProxyParamsUi {
		width: params.width,
		height: params.height,
		divider: params.divider,
		crf: params.crf,
		preset: params.preset().to_string(),
		include_audio: params.include_audio != 0,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn proxy_params_from_config_returns_the_defaults() {
		let params = proxy_params_from_config();
		let defaults = SdkProxyParams::default();
		assert_eq!(params.width, defaults.width);
		assert_eq!(params.height, defaults.height);
		assert_eq!(params.divider, defaults.divider);
		assert_eq!(params.crf, defaults.crf);
	}
}

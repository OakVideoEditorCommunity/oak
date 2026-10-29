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

use oak_engine_rust_sdk::codec::ProxyParams as SdkProxyParams;
use oak_engine_rust_sdk::state::Config;

use super::engine::ProxyParamsUi;
use super::real::{config_set_bool, config_set_int};

pub fn use_proxy_media() -> bool {
	Config::global().get_bool("UseProxyMedia", true)
}

pub fn set_use_proxy_media(enabled: bool) {
	config_set_bool("UseProxyMedia", enabled);
}

pub fn playback_divider() -> i64 {
	Config::global().get_i64("PlaybackDivider", 1).clamp(1, 8)
}

pub fn set_playback_divider(divider: i64) {
	config_set_int("PlaybackDivider", divider.clamp(1, 8));
}

pub fn stop_on_last() -> bool {
	Config::global().get_bool("StopOnLastFrame", false)
}

pub fn set_stop_on_last(enabled: bool) {
	config_set_bool("StopOnLastFrame", enabled);
}

pub fn waveform_mode() -> i32 {
	Config::global().get_i64("ViewerWaveformMode", 0).clamp(0, 2) as i32
}

pub fn set_waveform_mode(mode: i32) {
	config_set_int("ViewerWaveformMode", mode.clamp(0, 2) as i64);
}

pub fn default_project_color_settings() -> (String, String, String) {
	(
		"acescg".to_string(),
		"srgb".to_string(),
		"srgb".to_string(),
	)
}

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

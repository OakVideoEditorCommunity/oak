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

use oak_codec::proxymanager::ProxyParams;
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

pub fn proxy_params_to_codec(ui: &ProxyParamsUi) -> ProxyParams {
	let mut params = ProxyParams::default();
	params.width = ui.width;
	params.height = ui.height;
	params.divider = ui.divider;
	params.crf = ui.crf;
	params.include_audio = if ui.include_audio { 1 } else { 0 };
	let bytes = ui.preset.as_bytes();
	let len = bytes.len().min(31);
	params.preset[..len].copy_from_slice(&bytes[..len]);
	params
}

#[cfg(test)]
pub fn proxy_params_from_codec(codec: &ProxyParams) -> ProxyParamsUi {
	let end = codec
		.preset
		.iter()
		.position(|&byte| byte == 0)
		.unwrap_or(codec.preset.len());
	ProxyParamsUi {
		width: codec.width,
		height: codec.height,
		divider: codec.divider,
		crf: codec.crf,
		preset: String::from_utf8_lossy(&codec.preset[..end]).into_owned(),
		include_audio: codec.include_audio != 0,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn proxy_params_round_trip_through_the_codec_type() {
		let codec = ProxyParams {
			width: 1280,
			height: 720,
			divider: 2,
			crf: 23,
			include_audio: 0,
			..ProxyParams::default()
		};
		let ui = proxy_params_from_codec(&codec);
		assert_eq!(ui.width, 1280);
		assert_eq!(ui.height, 720);
		assert_eq!(ui.divider, 2);
		assert_eq!(ui.crf, 23);
		assert!(!ui.include_audio);
		let back = proxy_params_to_codec(&ui);
		assert_eq!(back.width, 1280);
		assert_eq!(back.divider, 2);
		assert_eq!(back.include_audio, 0);

		let long = ProxyParamsUi {
			preset: "x".repeat(40),
			include_audio: true,
			..ui
		};
		let codec = proxy_params_to_codec(&long);
		assert_eq!(&codec.preset[..31], "x".repeat(31).as_bytes());
		assert_eq!(codec.preset[31], 0);
		assert_eq!(codec.include_audio, 1);
	}

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

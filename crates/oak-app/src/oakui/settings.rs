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

use oak_codec::proxymanager::{ProxyManager, ProxyParams};
use oak_core::colormath::{OutputGamut, OutputTransfer, WorkingColorSpace};
use oak_core::configstore::ConfigStore;

use super::engine::ProxyParamsUi;

pub fn use_proxy_media() -> bool {
	ConfigStore::instance().get_bool(None, "UseProxyMedia", 1) != 0
}

pub fn set_use_proxy_media(enabled: bool) {
	ConfigStore::instance().set(
		None,
		"UseProxyMedia",
		if enabled { "true" } else { "false" },
	);
}

pub fn playback_divider() -> i64 {
	ConfigStore::instance()
		.get_int(None, "PlaybackDivider", 1)
		.clamp(1, 8) as i64
}

pub fn set_playback_divider(divider: i64) {
	ConfigStore::instance().set(None, "PlaybackDivider", &divider.clamp(1, 8).to_string());
}

pub fn stop_on_last() -> bool {
	ConfigStore::instance().get_bool(None, "StopOnLastFrame", 0) != 0
}

pub fn set_stop_on_last(enabled: bool) {
	ConfigStore::instance().set(
		None,
		"StopOnLastFrame",
		if enabled { "true" } else { "false" },
	);
}

pub fn waveform_mode() -> i32 {
	ConfigStore::instance()
		.get_int(None, "ViewerWaveformMode", 0)
		.clamp(0, 2)
}

pub fn set_waveform_mode(mode: i32) {
	ConfigStore::instance().set(None, "ViewerWaveformMode", &mode.clamp(0, 2).to_string());
}

pub fn default_project_color_settings() -> (String, String, String) {
	(
		WorkingColorSpace::default().as_setting().to_string(),
		OutputGamut::default().as_setting().to_string(),
		OutputTransfer::default().as_setting().to_string(),
	)
}

pub fn proxy_params_from_config() -> ProxyParamsUi {
	proxy_params_from_codec(&ProxyManager::proxy_params_from_config())
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
		let defaults = ProxyManager::proxy_params_default();
		assert_eq!(params.width, defaults.width);
		assert_eq!(params.height, defaults.height);
		assert_eq!(params.divider, defaults.divider);
		assert_eq!(params.crf, defaults.crf);
	}
}

//! Test-only media fixtures built through the public SDK.

use crate::codec::{Codec, EncoderSession, EncodingParams, Format, Frame};
use crate::types::{PixelFormat, Rational, VideoParams};

/// A real 64x64, ten-frame H.264 clip with no app-side dependencies.
pub(crate) fn make_test_clip(name: &str) -> String {
	let out = std::env::temp_dir().join(format!(
		"oaksdk_editor_fixture_{name}_{}.mp4",
		std::process::id()
	));
	let mut params = EncodingParams::new();
	params.set_filename(out.to_str().unwrap()).unwrap();
	params.format = Format::MPEG4Video as i32;
	params.video_enabled = 1;
	params.video_codec = Codec::H264 as i32;
	params.video_width = 64;
	params.video_height = 64;
	params.video_time_base_num = 1;
	params.video_time_base_den = 10;
	params.video_pixel_format = PixelFormat::F32 as i32;
	params.video_pixel_aspect_num = 1;
	params.video_pixel_aspect_den = 1;

	let session = EncoderSession::create(&params).expect("create encoder");
	session.open().expect("open encoder");
	let mut video = VideoParams::new();
	video.width = 64;
	video.height = 64;
	video.format = PixelFormat::F32 as i32;
	video.channel_count = 4;
	for index in 0..10 {
		let mut frame = Frame::with_params(&video);
		frame.allocate().expect("allocate frame");
		frame.set_timestamp(Rational::new(index, 10));
		session.write_video(&frame).expect("write frame");
	}
	session.flush().expect("flush encoder");
	session.close().expect("close encoder");
	out.to_string_lossy().into_owned()
}

/// Builds a scan directory containing the bundled CPU OFX test plugin.
/// Returns `None` when the build artifact is unavailable.
pub(crate) fn ofx_plugin_scan_dir() -> Option<std::path::PathBuf> {
	let profile_dir = std::path::Path::new(env!("OUT_DIR")).ancestors().nth(3)?.to_path_buf();
	let build = profile_dir.join("build");
	let mut library = None;
	for entry in std::fs::read_dir(&build).ok()? {
		let entry = entry.ok()?;
		if !entry.file_name().to_string_lossy().starts_with("oak-ofx-plugin-") {
			continue;
		}
		for artifact in ["oak_test_plugin.so", "oak_test_plugin.dylib"] {
			let candidate = entry.path().join("out").join(artifact);
			if candidate.exists() {
				library = Some(candidate);
				break;
			}
		}
		if library.is_some() {
			break;
		}
	}
	let library = library?;
	let bundle = std::env::temp_dir().join("oak-sdk-ofx-test/oak_test_plugin.ofx.bundle");
	let platform = if cfg!(target_os = "macos") {
		"MacOS"
	} else if cfg!(target_os = "windows") {
		"Win64"
	} else {
		"Linux-x86-64"
	};
	let bin_dir = bundle.join("Contents").join(platform);
	std::fs::create_dir_all(&bin_dir).ok()?;
	let target = bin_dir.join(if cfg!(target_os = "windows") { "plugin.dll" } else { "plugin" });
	if !target.exists() {
		std::fs::copy(&library, &target).ok()?;
	}
	bundle.parent().map(|path| path.to_path_buf())
}

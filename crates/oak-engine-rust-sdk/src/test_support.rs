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

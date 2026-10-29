//! Owned video frames with source and pixel-format metadata.

use std::ffi::c_void;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_video_buffer_add_ref(frame: *const c_void);
	fn oak_video_buffer_release(frame: *const c_void);
	fn oak_video_buffer_info(frame: *const c_void, out: *mut VideoFrameInfo) -> bool;
	fn oak_video_buffer_copy_bytes(frame: *const c_void, dst: *mut u8, capacity: usize) -> usize;
}

/// Original producer of an owned frame's bytes.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOrigin {
	/// The frame was produced on the CPU.
	Cpu = 0,
	/// The frame lives in a GPU texture.
	Gpu = 1,
	/// The frame came from a worker shared-memory slot.
	SharedMemory = 2,
}

impl Default for FrameOrigin {
	fn default() -> Self {
		Self::Cpu
	}
}

/// Pixel layout of an owned frame's byte buffer.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoPixelFormat {
	/// Tight F32 RGBA samples.
	F32Rgba = 0,
	/// Packed BGRA8 bytes.
	Bgra8 = 1,
}

impl Default for VideoPixelFormat {
	fn default() -> Self {
		Self::F32Rgba
	}
}

/// Geometry, origin, pixel format and byte count of an owned frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoFrameInfo {
	/// The `width` field.
	pub width: i32,
	/// The `height` field.
	pub height: i32,
	/// The `linesize` field.
	pub linesize: i32,
	/// The `origin` field.
	pub origin: FrameOrigin,
	/// The `pixel_format` field.
	pub pixel_format: VideoPixelFormat,
	/// The `byte_count` field.
	pub byte_count: usize,
}

/// Reference-counted immutable video frame.
pub struct VideoFrame {
	ptr: *const c_void,
}

// SAFETY: the engine allocation is reference counted and immutable.
unsafe impl Send for VideoFrame {}
// SAFETY: shared accessors only copy metadata or bytes.
unsafe impl Sync for VideoFrame {}

impl VideoFrame {
	pub(crate) fn from_raw(ptr: *const c_void) -> Option<Self> {
		(!ptr.is_null()).then_some(Self { ptr })
	}

	/// Returns geometry, origin and pixel layout.
	pub fn info(&self) -> VideoFrameInfo {
		let mut info = VideoFrameInfo::default();
		unsafe { oak_video_buffer_info(self.ptr, &mut info) }
			.then_some(info)
			.unwrap_or_default()
	}

	/// Copies the source bytes. F32 frames are native-endian RGBA; BGRA8 frames
	/// are packed blue/green/red/alpha bytes with `linesize` bytes per row.
	pub fn bytes(&self) -> Vec<u8> {
		let info = self.info();
		let mut bytes = vec![0; info.byte_count];
		let written =
			unsafe { oak_video_buffer_copy_bytes(self.ptr, bytes.as_mut_ptr(), bytes.len()) };
		if written == bytes.len() {
			bytes
		} else {
			Vec::new()
		}
	}

	/// Converts either representation into tightly packed F32 RGBA samples.
	pub fn f32_samples(&self) -> Vec<f32> {
		let info = self.info();
		let bytes = self.bytes();
		match info.pixel_format {
			VideoPixelFormat::F32Rgba => bytes
				.chunks_exact(4)
				.map(|value| f32::from_ne_bytes(value.try_into().expect("four-byte sample")))
				.collect(),
			VideoPixelFormat::Bgra8 => bytes
				.chunks_exact(4)
				.flat_map(|pixel| {
					[
						f32::from(pixel[2]) / 255.0,
						f32::from(pixel[1]) / 255.0,
						f32::from(pixel[0]) / 255.0,
						f32::from(pixel[3]) / 255.0,
					]
				})
				.collect(),
		}
	}
}

impl Clone for VideoFrame {
	fn clone(&self) -> Self {
		unsafe { oak_video_buffer_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for VideoFrame {
	fn drop(&mut self) {
		unsafe { oak_video_buffer_release(self.ptr) };
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	struct WorkerEnv(Option<std::ffi::OsString>);

	impl WorkerEnv {
		fn activate() -> Self {
			let previous = std::env::var_os("OAK_WORKER_BIN");
			let binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
				.join("../../target/debug/oak-worker");
			unsafe { std::env::set_var("OAK_WORKER_BIN", binary) };
			Self(previous)
		}
	}

	impl Drop for WorkerEnv {
		fn drop(&mut self) {
			match &self.0 {
				Some(value) => unsafe { std::env::set_var("OAK_WORKER_BIN", value) },
				None => unsafe { std::env::remove_var("OAK_WORKER_BIN") },
			}
		}
	}

	fn media_with_sequence(
		name: &str,
	) -> (crate::editor::EditorSession, crate::node::NodeId, String) {
		let session = crate::editor::EditorSession::new().expect("session");
		let sequence = session.create_sequence("Video").expect("sequence");
		let media = crate::test_support::make_test_clip(name);
		let footage = session.project().import_footage(&media).expect("footage");
		session
			.execute(crate::edit::EditCommand::PlaceFootageClip {
				footage,
				kind: crate::editor::TrackKind::Video,
				track_index: 0,
				in_ts: 0,
				out_ts: 25,
				media_in_ts: 0,
			})
			.expect("place clip");
		(session, sequence, media)
	}

	#[test]
	fn inline_video_frames_preserve_format_and_non_black_pixels() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let was_initialized = crate::render::manager_is_initialized();
		if was_initialized {
			crate::render::manager_shutdown();
		}
		let (session, sequence, media) = media_with_sequence("inline_video");
		let footage = session.snapshot().footages[0].id;
		for (viewer, pixel_format) in [
			(footage, VideoPixelFormat::F32Rgba),
			(sequence, VideoPixelFormat::Bgra8),
		] {
			let task = crate::task::render_video(
				&session.project(),
				viewer,
				crate::types::Rational::new(0, 1),
				(16, 12),
				pixel_format,
			)
			.expect("video task");
			assert!(task.wait(), "video render failed: {}", task.error());
			let frame = task.take_video().expect("video frame");
			assert_eq!(frame.info().origin, FrameOrigin::Cpu);
			assert_eq!(frame.info().pixel_format, pixel_format);
			let expected_bytes = 16
				* 12 * 4 * if pixel_format == VideoPixelFormat::F32Rgba {
				4
			} else {
				1
			};
			assert_eq!(frame.bytes().len(), expected_bytes);
			assert!(frame.f32_samples().iter().any(|sample| *sample > 0.01));
		}
		if was_initialized {
			assert!(crate::render::manager_init(
				crate::render::RenderBackend::Threads
			));
		}
		let _ = std::fs::remove_file(media);
	}

	#[test]
	fn worker_shared_memory_frames_release_and_recover_after_failure() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let _worker = WorkerEnv::activate();
		crate::render::manager_shutdown();
		if !crate::render::manager_init(crate::render::RenderBackend::Processes) {
			println!("SKIP: oak-worker is unavailable in this environment");
			let _ = crate::render::manager_init(crate::render::RenderBackend::Threads);
			return;
		}
		let (session, sequence, media) = media_with_sequence("worker_video");
		let bad_media = crate::test_support::make_test_clip("worker_bad_video");
		let bad = session
			.project()
			.import_footage(&bad_media)
			.expect("bad footage");
		std::fs::remove_file(&bad_media).expect("remove bad input");
		let failed = crate::task::render_video(
			&session.project(),
			bad,
			crate::types::Rational::new(0, 1),
			(16, 12),
			VideoPixelFormat::Bgra8,
		)
		.expect("failing video task");
		assert!(!failed.wait());

		let task = crate::task::render_video(
			&session.project(),
			sequence,
			crate::types::Rational::new(0, 1),
			(16, 12),
			VideoPixelFormat::Bgra8,
		)
		.expect("worker video task");
		assert!(task.wait(), "worker render failed: {}", task.error());
		let frame = task.take_video().expect("worker frame");
		assert_eq!(frame.info().origin, FrameOrigin::SharedMemory);
		assert_eq!(frame.info().pixel_format, VideoPixelFormat::Bgra8);
		assert_eq!(frame.bytes().len(), 16 * 12 * 4);
		assert!(frame.f32_samples().iter().any(|sample| *sample > 0.01));
		let cloned = frame.clone();
		drop(cloned);
		assert_eq!(frame.bytes().len(), 16 * 12 * 4);
		let _ = std::fs::remove_file(media);
		crate::render::manager_shutdown();
		let _ = crate::render::manager_init(crate::render::RenderBackend::Threads);
	}

	#[test]
	fn manager_video_tickets_apply_adjustments_and_proxy_limits() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		if !crate::render::manager_init(crate::render::RenderBackend::Threads) {
			assert!(crate::render::manager_is_initialized());
		}
		let (session, sequence, media) = media_with_sequence("manager_montage");
		let footage = session.snapshot().footages[0].id;
		assert!(
			session
				.snapshot()
				.sequences
				.iter()
				.find(|item| item.id == sequence)
				.expect("sequence snapshot")
				.tracks
				.iter()
				.any(|track| !track.clips.is_empty())
		);
		let track = session
			.snapshot()
			.sequences
			.iter()
			.find(|item| item.id == sequence)
			.and_then(|item| {
				item.tracks
					.iter()
					.filter(|track| track.kind == Some(crate::editor::TrackKind::Video))
					.nth(1)
			})
			.expect("video track")
			.id;
		let adjustment = session
			.execute(crate::edit::EditCommand::CreateAdjustmentLayer {
				track,
				in_ts: 0,
				out_ts: 25,
			})
			.expect("adjustment")
			.primary
			.expect("adjustment id");
		let effect = session
			.execute(crate::edit::EditCommand::EffectInsert {
				host: adjustment,
				index: usize::MAX,
				type_id: "org.olivevideoeditor.Olive.opacity".into(),
			})
			.expect("opacity")
			.primary
			.expect("effect id");
		session
			.execute(crate::edit::EditCommand::SetNodeStandardValue {
				node: effect,
				input_id: "opacity_in".into(),
				element: -1,
				value: crate::node::NodeValue {
					kind: 2,
					..Default::default()
				},
			})
			.expect("zero opacity");

		let mut params = crate::codec::ProxyParams::default();
		params.width = 32;
		params.height = 32;
		params.divider = 1;
		session
			.execute(crate::edit::EditCommand::SetFootageProxyParams { footage, params })
			.expect("proxy params");
		let cache = std::env::temp_dir()
			.join(format!("oak_manager_proxy_{}", std::process::id()))
			.to_string_lossy()
			.into_owned();
		let proxy = session.generate_proxy(footage, &cache).expect("proxy task");
		assert!(proxy.wait(), "proxy generation failed: {}", proxy.error());
		assert!(session.complete_proxy(footage, &proxy, true));
		session
			.set_proxy_enabled(footage, true)
			.expect("enable proxy");

		let task = crate::task::render_video(
			&session.project(),
			sequence,
			crate::types::Rational::new(0, 1),
			(128, 128),
			VideoPixelFormat::F32Rgba,
		)
		.expect("manager video task");
		assert!(task.wait(), "manager render failed: {}", task.error());
		let frame = task.take_video().expect("manager frame");
		assert_eq!((frame.info().width, frame.info().height), (32, 32));
		assert!(frame.f32_samples().iter().all(|sample| sample.abs() < 0.01));
		let _ = std::fs::remove_dir_all(cache);
		let _ = std::fs::remove_file(media);
	}

	#[test]
	fn playback_video_task_returns_an_owned_frame() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		if !crate::render::manager_init(crate::render::RenderBackend::Threads) {
			assert!(crate::render::manager_is_initialized());
		}
		let (session, sequence, media) = media_with_sequence("playback_video");
		let task = crate::task::render_playback_video(
			&session.project(),
			sequence,
			crate::types::Rational::new(0, 1),
			(16, 12),
			VideoPixelFormat::F32Rgba,
			0,
			0,
			9,
		)
		.expect("playback video task");
		assert!(task.wait(), "playback render failed: {}", task.error());
		let frame = task.take_video().expect("playback frame");
		assert_eq!(frame.f32_samples().len(), 16 * 12 * 4);
		let _ = std::fs::remove_file(media);
	}
}

//! Owned video frames with explicit source and pixel-format metadata.

mod montage;

use oak_core::{PixelFormat, Rational};
use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_render::manager::RenderManager;
use oak_render::ticket::{TicketPayload, VideoTicketParams};

use crate::handle::NativeMirror;
use montage::{
	clamp_render_size, preview_media, single_track_video_montage, video_montage_with_adjustments,
};

/// Original producer of an owned frame's bytes.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOrigin {
	Cpu = 0,
	Gpu = 1,
	SharedMemory = 2,
}

/// Wire format of an owned frame's byte buffer.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoPixelFormat {
	F32Rgba = 0,
	Bgra8 = 1,
}

/// Owned bytes and geometry for one video frame.
#[derive(Clone, Debug)]
pub struct VideoBufferData {
	pub width: i32,
	pub height: i32,
	pub linesize: i32,
	pub origin: FrameOrigin,
	pub pixel_format: VideoPixelFormat,
	pub bytes: Vec<u8>,
}

/// Video metadata without the variable-length byte buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoBufferMetadata {
	/// Frame width in pixels.
	pub width: i32,
	/// Frame height in pixels.
	pub height: i32,
	/// Bytes per scanline in the owned buffer.
	pub linesize: i32,
	/// Producer that supplied the original pixels.
	pub origin: FrameOrigin,
	/// Byte layout of the owned pixels.
	pub pixel_format: VideoPixelFormat,
	/// Number of bytes in the owned buffer.
	pub byte_count: usize,
}

impl VideoBufferData {
	fn metadata(&self) -> VideoBufferMetadata {
		VideoBufferMetadata {
			width: self.width,
			height: self.height,
			linesize: self.linesize,
			origin: self.origin,
			pixel_format: self.pixel_format,
			byte_count: self.bytes.len(),
		}
	}
}

/// Opaque reference-counted video result.
pub struct VideoBufferHandle {
	pub(crate) data: VideoBufferData,
}

crate::export_handle!(
	VideoBufferHandle,
	oak_video_buffer_add_ref,
	oak_video_buffer_release
);

impl VideoBufferHandle {
	pub(crate) fn from_data(data: VideoBufferData) -> *const Self {
		crate::handle::into_ffi(Self { data })
	}
}

/// Geometry, source, pixel format and byte count of a video frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakVideoBufferInfo {
	/// Frame width in pixels.
	pub width: i32,
	/// Frame height in pixels.
	pub height: i32,
	/// Bytes per scanline in the owned buffer.
	pub linesize: i32,
	/// `FrameOrigin` discriminant.
	pub origin: i32,
	/// `VideoPixelFormat` discriminant.
	pub pixel_format: i32,
	/// Number of bytes in the owned buffer.
	pub byte_count: usize,
}

impl NativeMirror for OakVideoBufferInfo {
	type Native = VideoBufferMetadata;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			width: native.width,
			height: native.height,
			linesize: native.linesize,
			origin: native.origin as i32,
			pixel_format: native.pixel_format as i32,
			byte_count: native.byte_count,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			width: self.width,
			height: self.height,
			linesize: self.linesize,
			origin: match self.origin {
				1 => FrameOrigin::Gpu,
				2 => FrameOrigin::SharedMemory,
				_ => FrameOrigin::Cpu,
			},
			pixel_format: match self.pixel_format {
				1 => VideoPixelFormat::Bgra8,
				_ => VideoPixelFormat::F32Rgba,
			},
			byte_count: self.byte_count,
		}
	}
}

/// Reads video metadata; false for null arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_video_buffer_info(
	frame: *const VideoBufferHandle,
	out: *mut OakVideoBufferInfo,
) -> bool {
	let (Some(frame), Some(out)) = (unsafe { frame.as_ref() }, unsafe { out.as_mut() }) else {
		return false;
	};
	*out = OakVideoBufferInfo::from_native(&frame.data.metadata());
	true
}

/// Copies packed source bytes into caller storage. Returns required bytes or
/// zero for invalid arguments/insufficient capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_video_buffer_copy_bytes(
	frame: *const VideoBufferHandle,
	dst: *mut u8,
	dst_capacity: usize,
) -> usize {
	let Some(frame) = (unsafe { frame.as_ref() }) else {
		return 0;
	};
	if dst.is_null() || dst_capacity < frame.data.bytes.len() {
		return 0;
	}
	unsafe {
		std::ptr::copy_nonoverlapping(frame.data.bytes.as_ptr(), dst, frame.data.bytes.len())
	};
	frame.data.bytes.len()
}

fn f32_to_bgra8(samples: &[f32]) -> Vec<u8> {
	samples
		.chunks_exact(4)
		.flat_map(|pixel| {
			let convert = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
			[
				convert(pixel[2]),
				convert(pixel[1]),
				convert(pixel[0]),
				convert(pixel[3]),
			]
		})
		.collect()
}

fn pack_f32_bytes(
	width: i32,
	height: i32,
	linesize: i32,
	bytes: &[u8],
	origin: FrameOrigin,
	pixel_format: VideoPixelFormat,
) -> Result<VideoBufferData, String> {
	let row_bytes = width.max(0) as usize * 16;
	let source_stride = (linesize.max(0) as usize).max(row_bytes);
	if width <= 0 || height <= 0 || bytes.len() < source_stride * (height as usize - 1) + row_bytes
	{
		return Err("video frame has invalid geometry or truncated pixels".into());
	}
	let mut samples = Vec::with_capacity(width as usize * height as usize * 4);
	for y in 0..height as usize {
		let row = &bytes[y * source_stride..y * source_stride + row_bytes];
		for value in row.chunks_exact(4) {
			samples.push(f32::from_ne_bytes(
				value.try_into().expect("four-byte sample"),
			));
		}
	}
	let bytes = match pixel_format {
		VideoPixelFormat::F32Rgba => samples
			.iter()
			.flat_map(|sample| sample.to_ne_bytes())
			.collect(),
		VideoPixelFormat::Bgra8 => f32_to_bgra8(&samples),
	};
	Ok(VideoBufferData {
		width,
		height,
		linesize: width
			* match pixel_format {
				VideoPixelFormat::F32Rgba => 16,
				VideoPixelFormat::Bgra8 => 4,
			},
		origin,
		pixel_format,
		bytes,
	})
}

fn frame_from_cpu(
	frame: oak_core::texture::Frame,
	origin: FrameOrigin,
	pixel_format: VideoPixelFormat,
) -> Result<VideoBufferData, String> {
	let params = frame.video_params();
	pack_f32_bytes(
		params.width,
		params.height,
		frame.linesize_bytes() as i32,
		&frame.data,
		origin,
		pixel_format,
	)
}

fn frame_from_shared(
	frame: &oak_render::procpool::ShmFrameRef,
	pixel_format: VideoPixelFormat,
) -> Result<VideoBufferData, String> {
	let meta = &frame.meta;
	let bytes = frame.shm.slot_to_vec(frame.slot);
	let bytes = bytes
		.get(..meta.data_size.max(0) as usize)
		.ok_or("shared-memory frame is truncated")?;
	if meta.format == PixelFormat::F32 as i32 {
		pack_f32_bytes(
			meta.width,
			meta.height,
			meta.linesize,
			bytes,
			FrameOrigin::SharedMemory,
			pixel_format,
		)
	} else if pixel_format == VideoPixelFormat::Bgra8 {
		let row_bytes = meta.width.max(0) as usize * 4;
		let source_stride = (meta.linesize.max(0) as usize).max(row_bytes);
		if meta.width <= 0
			|| meta.height <= 0
			|| bytes.len() < source_stride * (meta.height as usize - 1) + row_bytes
		{
			return Err("shared-memory BGRA frame is truncated".into());
		}
		let mut packed = Vec::with_capacity(row_bytes * meta.height as usize);
		for y in 0..meta.height as usize {
			packed.extend_from_slice(&bytes[y * source_stride..y * source_stride + row_bytes]);
		}
		Ok(VideoBufferData {
			width: meta.width,
			height: meta.height,
			linesize: meta.width * 4,
			origin: FrameOrigin::SharedMemory,
			pixel_format,
			bytes: packed,
		})
	} else {
		let mut converted = Vec::with_capacity(meta.width as usize * meta.height as usize * 16);
		for pixel in bytes.chunks_exact(4) {
			for index in [2, 1, 0, 3] {
				converted.extend_from_slice(&(f32::from(pixel[index]) / 255.0).to_ne_bytes());
			}
		}
		pack_f32_bytes(
			meta.width,
			meta.height,
			meta.width * 16,
			&converted,
			FrameOrigin::SharedMemory,
			pixel_format,
		)
	}
}

/// Renders through an existing manager or the inline evaluator and returns an
/// owned frame. Shared-memory slots are copied and released before returning.
pub(crate) fn render_video(
	project: std::sync::Arc<std::sync::Mutex<Project>>,
	viewer: NodeId,
	track: Option<NodeId>,
	playback: Option<(i64, i64, u64)>,
	time: Rational,
	width: i32,
	height: i32,
	pixel_format: VideoPixelFormat,
) -> Result<VideoBufferData, String> {
	if width <= 0 || height <= 0 {
		return Err("invalid video dimensions".into());
	}
	let (width, height) = {
		let guard = project.lock().unwrap_or_else(|e| e.into_inner());
		if guard
			.graph
			.get(viewer)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
			.is_some()
		{
			clamp_render_size(&guard, viewer, time, width, height)
		} else {
			(width, height)
		}
	};
	if let Some(manager) = RenderManager::global() {
		let (montage, adjustments, footage) = {
			let guard = project.lock().unwrap_or_else(|e| e.into_inner());
			if let Some(track) = track {
				(
					single_track_video_montage(&guard, viewer, track, time),
					Vec::new(),
					None,
				)
			} else {
				let is_sequence = guard
					.graph
					.get(viewer)
					.and_then(|entry| entry.behavior.as_any())
					.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
					.is_some();
				if is_sequence {
					let (montage, adjustments) =
						video_montage_with_adjustments(&guard, viewer, time);
					(montage, adjustments, None)
				} else {
					let footage = guard
						.graph
						.get(viewer)
						.and_then(|entry| entry.behavior.as_any())
						.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>())
						.and_then(preview_media);
					(Vec::new(), Vec::new(), footage)
				}
			}
		};
		let params = VideoTicketParams {
			viewer: if track.is_some() {
				0
			} else {
				viewer.identity()
			},
			// P5C exercises the semantic montage path explicitly. A project UUID
			// would select graph-snapshot mode and bypass adjustments/proxy limits.
			project: String::new(),
			time,
			force_size: Some((width, height)),
			force_format: (pixel_format == VideoPixelFormat::F32Rgba).then_some(PixelFormat::F32),
			cache: None,
			cache_dir: None,
			cache_id: None,
			cache_timebase: None,
			footage,
			montage,
			adjustments,
		};
		let id = manager.tickets.next_id();
		if let Some((frame, distance, generation)) = playback {
			manager.tickets.submit_playback_with_id(
				id,
				params,
				frame,
				distance,
				generation,
				Box::new(|_| {}),
			);
		} else {
			manager
				.tickets
				.submit_video_with_id(id, params, Box::new(|_| {}));
		}
		manager.tickets.wait(id).map_err(|e| e.to_string())?;
		let result = manager
			.tickets
			.result(id)
			.ok_or("video ticket produced no result")?;
		return match result {
			Ok(TicketPayload::Video(oak_core::texture::Texture::Cpu(frame))) => {
				frame_from_cpu(frame, FrameOrigin::Cpu, pixel_format)
			}
			Ok(TicketPayload::Video(texture)) => {
				let frame = texture.to_frame().map_err(|e| e.to_string())?;
				frame_from_cpu(frame, FrameOrigin::Gpu, pixel_format)
			}
			Ok(TicketPayload::ShmFrame(frame)) => {
				let converted = frame_from_shared(&frame, pixel_format);
				manager.release_frame(&frame);
				converted
			}
			_ => Err("video render produced no frame".into()),
		};
	}
	if track.is_some() {
		let montage = {
			let guard = project.lock().unwrap_or_else(|e| e.into_inner());
			single_track_video_montage(&guard, viewer, track.expect("checked track"), time)
		};
		let params = VideoTicketParams {
			viewer: 0,
			project: String::new(),
			time,
			force_size: Some((width, height)),
			force_format: Some(PixelFormat::F32),
			cache: None,
			cache_dir: None,
			cache_id: None,
			cache_timebase: None,
			footage: None,
			montage,
			adjustments: Vec::new(),
		};
		let mut bytes = vec![0; width as usize * height as usize * 16];
		oak_render::eval::render_montage_frame_into(
			time,
			&params,
			(width, height),
			&mut bytes,
			width * 16,
		)
		.map_err(|error| error.to_string())?;
		return pack_f32_bytes(
			width,
			height,
			width * 16,
			&bytes,
			FrameOrigin::Cpu,
			pixel_format,
		);
	}
	let samples = crate::render::render_frame_f32_data(project, viewer, time, width, height)?;
	let bytes: Vec<u8> = samples
		.iter()
		.flat_map(|sample| sample.to_ne_bytes())
		.collect();
	pack_f32_bytes(
		width,
		height,
		width * 16,
		&bytes,
		FrameOrigin::Cpu,
		pixel_format,
	)
}

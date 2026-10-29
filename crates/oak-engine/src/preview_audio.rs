//! Owned preview-audio mixing and C ABI buffers.

use oak_core::TimeRange;
use oak_node::block::ClipBlockBehavior;
use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior, TrackType};
use oak_render::manager::RenderManager;
use oak_render::ticket::{AudioTicketParams, MontageClip, TicketPayload};

use crate::coretypes::OakTimeRange;
use crate::handle::NativeMirror;

/// Owned interleaved F32 audio and its timing/layout metadata.
#[derive(Clone, Debug)]
pub struct AudioBufferData {
	pub range: TimeRange,
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub channel_count: i32,
	pub samples: Vec<f32>,
}

/// Metadata without the variable-length sample buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioBufferMetadata {
	/// Timeline range covered by this block.
	pub range: TimeRange,
	/// Output sample rate in Hz.
	pub sample_rate: i32,
	/// FFmpeg-style channel layout mask.
	pub channel_layout: u64,
	/// Number of interleaved channels.
	pub channel_count: i32,
	/// Number of F32 values (`sample frames * channels`).
	pub sample_count: usize,
}

impl AudioBufferData {
	fn metadata(&self) -> AudioBufferMetadata {
		AudioBufferMetadata {
			range: self.range,
			sample_rate: self.sample_rate,
			channel_layout: self.channel_layout,
			channel_count: self.channel_count,
			sample_count: self.samples.len(),
		}
	}
}

/// Opaque reference-counted audio result.
pub struct AudioBufferHandle {
	pub(crate) data: AudioBufferData,
}

crate::export_handle!(
	AudioBufferHandle,
	oak_audio_buffer_add_ref,
	oak_audio_buffer_release
);

impl AudioBufferHandle {
	pub(crate) fn from_data(data: AudioBufferData) -> *const Self {
		crate::handle::into_ffi(Self { data })
	}
}

/// Timing, layout and interleaved-element count of an audio block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OakAudioBufferInfo {
	pub range: OakTimeRange,
	pub sample_rate: i32,
	pub channel_layout: u64,
	pub channel_count: i32,
	pub sample_count: usize,
}

impl NativeMirror for OakAudioBufferInfo {
	type Native = AudioBufferMetadata;

	fn from_native(native: &Self::Native) -> Self {
		Self {
			range: OakTimeRange::from_native(&native.range),
			sample_rate: native.sample_rate,
			channel_layout: native.channel_layout,
			channel_count: native.channel_count,
			sample_count: native.sample_count,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			range: self.range.to_native(),
			sample_rate: self.sample_rate,
			channel_layout: self.channel_layout,
			channel_count: self.channel_count,
			sample_count: self.sample_count,
		}
	}
}

/// Reads audio metadata; false for null arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_buffer_info(
	audio: *const AudioBufferHandle,
	out: *mut OakAudioBufferInfo,
) -> bool {
	let (Some(audio), Some(out)) = (unsafe { audio.as_ref() }, unsafe { out.as_mut() }) else {
		return false;
	};
	*out = OakAudioBufferInfo::from_native(&audio.data.metadata());
	true
}

/// Copies interleaved F32 samples into caller storage. Returns the required
/// element count, or zero for invalid arguments/insufficient capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_audio_buffer_copy_samples(
	audio: *const AudioBufferHandle,
	dst: *mut f32,
	dst_capacity: usize,
) -> usize {
	let Some(audio) = (unsafe { audio.as_ref() }) else {
		return 0;
	};
	if dst.is_null() || dst_capacity < audio.data.samples.len() {
		return 0;
	}
	unsafe {
		std::ptr::copy_nonoverlapping(audio.data.samples.as_ptr(), dst, audio.data.samples.len())
	};
	audio.data.samples.len()
}

fn preview_media(footage: &FootageBehavior) -> Option<(String, i32)> {
	let original = footage
		.streams
		.iter()
		.find(|stream| !stream.is_video)
		.map(|stream| (footage.filename.clone(), stream.index))
		.or_else(|| (!footage.filename.is_empty()).then(|| (footage.filename.clone(), 1)))?;
	let use_proxy =
		oak_core::configstore::ConfigStore::instance().get_bool(None, "UseProxyMedia", 1) != 0;
	if use_proxy
		&& footage.proxy_enabled
		&& oak_codec::proxymanager::ProxyManager::get_proxy_state(&footage.proxy)
			== oak_codec::proxymanager::ProxyState::Ready
		&& oak_codec::proxymanager::ProxyManager::proxy_filename_has_audio(&footage.proxy)
	{
		Some((footage.proxy.clone(), 1))
	} else {
		Some(original)
	}
}

fn footage_of(project: &Project, clip: NodeId) -> Option<&FootageBehavior> {
	let footage = crate::editor_commands::find_input_footage(&project.graph, clip)?;
	project
		.graph
		.get(footage)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>())
}

fn audio_montage(project: &Project, sequence: NodeId, range: TimeRange) -> Vec<MontageClip> {
	let Some(sequence) = project
		.graph
		.get(sequence)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
	else {
		return Vec::new();
	};
	let mut clips = Vec::new();
	for list_id in &sequence.track_lists {
		let Some(list) = project
			.graph
			.get(*list_id)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<TrackListBehavior>())
		else {
			continue;
		};
		if list.kind != TrackType::Audio {
			continue;
		}
		for track_id in &list.tracks {
			let Some(track) = project
				.graph
				.get(*track_id)
				.and_then(|entry| entry.behavior.as_any())
				.and_then(|behavior| behavior.downcast_ref::<TrackBehavior>())
			else {
				continue;
			};
			if track.muted {
				continue;
			}
			for block_id in &track.blocks {
				let Some(clip) = project
					.graph
					.get(*block_id)
					.and_then(|entry| entry.behavior.as_any())
					.and_then(|behavior| behavior.downcast_ref::<ClipBlockBehavior>())
				else {
					continue;
				};
				let in_time = clip.core.in_();
				let out_time = clip.core.out();
				if out_time <= range.in_() || in_time >= range.out() {
					continue;
				}
				let Some(footage) = footage_of(project, *block_id) else {
					continue;
				};
				let Some((filename, stream_index)) = preview_media(footage) else {
					continue;
				};
				clips.push(MontageClip {
					filename,
					stream_index,
					in_time,
					out_time,
					media_in: clip.core.media_in,
					gain: 1.0,
					effects: Vec::new(),
				});
			}
		}
	}
	clips
}

/// Mixes one sequence or footage range. Existing render managers keep their
/// ticket/worker path; headless consumers use the same mixer on an engine task.
pub(crate) fn render_audio(
	project: std::sync::Arc<std::sync::Mutex<Project>>,
	viewer: NodeId,
	range: TimeRange,
	sample_rate: i32,
	channel_layout: u64,
) -> Result<AudioBufferData, String> {
	if range.out() <= range.in_() || sample_rate <= 0 || channel_layout == 0 {
		return Err("invalid audio range or output layout".into());
	}
	let montage = {
		let guard = project.lock().unwrap_or_else(|e| e.into_inner());
		if guard
			.graph
			.get(viewer)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
			.is_some()
		{
			audio_montage(&guard, viewer, range)
		} else {
			let footage = guard
				.graph
				.get(viewer)
				.and_then(|entry| entry.behavior.as_any())
				.and_then(|behavior| behavior.downcast_ref::<FootageBehavior>())
				.ok_or("audio viewer is not footage")?;
			let (filename, stream_index) = preview_media(footage).ok_or("footage has no audio")?;
			vec![MontageClip {
				filename,
				stream_index,
				in_time: range.in_(),
				out_time: range.out(),
				media_in: range.in_(),
				gain: 1.0,
				effects: Vec::new(),
			}]
		}
	};
	let params = AudioTicketParams {
		viewer: viewer.identity(),
		range,
		sample_rate,
		channel_layout,
		montage,
	};
	let samples = if let Some(manager) = RenderManager::global() {
		let id = manager.tickets.next_id();
		manager
			.tickets
			.submit_audio_with_id(id, params, Box::new(|_| {}));
		manager.tickets.wait(id).map_err(|e| e.to_string())?;
		let result = manager
			.tickets
			.result(id)
			.ok_or("audio ticket produced no result")?;
		match result {
			Ok(TicketPayload::Audio(samples)) => samples,
			Ok(TicketPayload::ShmAudio(audio)) => {
				let samples = audio.to_audio_samples();
				manager.release_audio_frame(&audio);
				samples
			}
			_ => return Err("audio render produced no samples".into()),
		}
	} else {
		match oak_render::eval::render_audio_samples(&params).map_err(|e| e.to_string())? {
			TicketPayload::Audio(samples) => samples,
			_ => return Err("audio render produced no samples".into()),
		}
	};
	Ok(AudioBufferData {
		range,
		sample_rate: samples.sample_rate,
		channel_layout: samples.channel_layout,
		channel_count: samples.channel_count,
		samples: samples.samples,
	})
}

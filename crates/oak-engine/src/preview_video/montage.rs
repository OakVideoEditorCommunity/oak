//! Sequence/footage montage construction shared by inline and worker renders.

use oak_core::Rational;
use oak_node::block::{AdjustmentBlockBehavior, ClipBlockBehavior, clip_input};
use oak_node::footage::FootageBehavior;
use oak_node::id::NodeId;
use oak_node::nodes::multicamnode::{CURRENT_INPUT, SEQUENCE_INPUT, SEQUENCE_TYPE_INPUT};
use oak_node::project::Project;
use oak_node::sequence::SequenceBehavior;
use oak_node::track::{TrackBehavior, TrackListBehavior, TrackType};
use oak_node::value::ValueType;
use oak_render::ticket::{AdjustmentSpan, MontageClip, MontageEffect};

use crate::editor_commands as ops;

/// Returns preview video media after proxy and source-stream selection.
pub(crate) fn preview_media(footage: &FootageBehavior) -> Option<(String, i32)> {
	let first = footage.streams.iter().find(|stream| stream.is_video);
	let original = first
		.map(|stream| (footage.filename.clone(), stream.index))
		.or_else(|| (!footage.filename.is_empty()).then(|| (footage.filename.clone(), 0)))?;
	let use_proxy =
		oak_core::configstore::ConfigStore::instance().get_bool(None, "UseProxyMedia", 1) != 0;
	if !use_proxy || !footage.proxy_enabled || footage.proxy.is_empty() {
		return Some(original);
	}
	if oak_codec::proxymanager::ProxyManager::get_proxy_state(&footage.proxy)
		!= oak_codec::proxymanager::ProxyState::Ready
	{
		return Some(original);
	}
	match first {
		Some(stream) if footage.proxy_video_stream_index == stream.index => {
			Some((footage.proxy.clone(), 0))
		}
		_ => Some(original),
	}
}

fn clip_effects(project: &Project, clip: NodeId) -> Vec<MontageEffect> {
	crate::effect::chain(&project.graph, clip)
		.into_iter()
		.filter(|&effect| crate::effect::effect_input_of(&project.graph, effect).is_some())
		.filter_map(|effect| {
			let entry = project.graph.get(effect)?;
			let params = entry
				.core
				.inputs
				.iter()
				.filter(|input| {
					!matches!(
						input.value_type,
						ValueType::Texture | ValueType::Samples | ValueType::Matrix
					) && input.flags & oak_node::input::flags::HIDDEN == 0
						&& input.id != oak_node::node::ENABLED_INPUT
				})
				.map(|input| (input.id.clone(), entry.core.standard_value(&input.id, -1)))
				.collect();
			Some(MontageEffect {
				type_id: entry.behavior.type_id().to_string(),
				enabled: crate::effect::is_enabled(&project.graph, effect),
				effect_input_id: crate::effect::effect_input_of(&project.graph, effect),
				params,
			})
		})
		.collect()
}

fn covering_adjustment(
	project: &Project,
	blocks: &[NodeId],
	time: Rational,
	below: usize,
) -> Option<AdjustmentSpan> {
	for &block in blocks {
		let Some(adjustment) = project
			.graph
			.get(block)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<AdjustmentBlockBehavior>())
		else {
			continue;
		};
		if adjustment.core.enabled && time >= adjustment.core.in_() && time < adjustment.core.out()
		{
			return Some(AdjustmentSpan {
				in_time: adjustment.core.in_(),
				out_time: adjustment.core.out(),
				track_index: below,
				effects: clip_effects(project, block),
			});
		}
	}
	None
}

fn multicam_media(
	project: &Project,
	clip: NodeId,
	host_in: Rational,
) -> Option<(String, i32, Rational)> {
	let multicam = project
		.graph
		.connected_output(clip, clip_input::TEXTURE_INPUT, -1)?;
	let entry = project.graph.get(multicam)?;
	if entry.behavior.type_id() != "org.olivevideoeditor.Olive.multicam" {
		return None;
	}
	let source = entry.core.standard_value(CURRENT_INPUT, -1).to_double() as i32;
	if source < 0 {
		return None;
	}
	let sequence = project
		.graph
		.connected_output(multicam, SEQUENCE_INPUT, -1)?;
	let kind = entry
		.core
		.standard_value(SEQUENCE_TYPE_INPUT, -1)
		.to_double() as i32;
	let kind = TrackType::from_c(kind).unwrap_or(TrackType::Video);
	let list = ops::track_list_of(&project.graph, sequence, kind)?;
	let list = ops::track_list_behavior(&project.graph, list)?;
	let track = *list.tracks.get(source as usize)?;
	let track = ops::track_behavior(&project.graph, track)?;
	track.blocks.iter().find_map(|&block| {
		let clip = ops::clip_behavior(&project.graph, block)?;
		if host_in < clip.core.in_() || host_in >= clip.core.out() {
			return None;
		}
		let footage_id = ops::find_input_footage(&project.graph, block)?;
		let footage = ops::footage_behavior(&project.graph, footage_id)?;
		let (filename, stream_index) = preview_media(footage)?;
		let media_in = clip.core.media_in + (host_in - clip.core.in_());
		Some((filename, stream_index, media_in))
	})
}

/// Builds the bottom-to-top video montage and adjustment spans for `time`.
pub(crate) fn video_montage_with_adjustments(
	project: &Project,
	sequence: NodeId,
	time: Rational,
) -> (Vec<MontageClip>, Vec<AdjustmentSpan>) {
	let mut clips = Vec::new();
	let mut adjustments = Vec::new();
	let Some(sequence) = project
		.graph
		.get(sequence)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
	else {
		return (clips, adjustments);
	};
	for list_id in &sequence.track_lists {
		let Some(list) = project
			.graph
			.get(*list_id)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<TrackListBehavior>())
		else {
			continue;
		};
		if list.kind != TrackType::Video {
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
			if let Some(span) = covering_adjustment(project, &track.blocks, time, clips.len()) {
				adjustments.push(span);
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
				if time < in_time || time >= out_time {
					continue;
				}
				let (filename, stream_index, media_in) =
					match multicam_media(project, *block_id, in_time) {
						Some(media) => media,
						None => {
							let Some(footage_id) =
								ops::find_input_footage(&project.graph, *block_id)
							else {
								continue;
							};
							let Some(footage) = ops::footage_behavior(&project.graph, footage_id)
							else {
								continue;
							};
							let Some((filename, stream_index)) = preview_media(footage) else {
								continue;
							};
							(filename, stream_index, clip.core.media_in)
						}
					};
				clips.push(MontageClip {
					filename,
					stream_index,
					in_time,
					out_time,
					media_in,
					gain: 1.0,
					effects: clip_effects(project, *block_id),
				});
			}
		}
	}
	(clips, adjustments)
}

/// Builds a multicam-angle montage containing only the requested video track.
pub(crate) fn single_track_video_montage(
	project: &Project,
	sequence: NodeId,
	track: NodeId,
	time: Rational,
) -> Vec<MontageClip> {
	let mut clips = Vec::new();
	let Some(sequence) = project
		.graph
		.get(sequence)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
	else {
		return clips;
	};
	let belongs_to_sequence = sequence.track_lists.iter().any(|list_id| {
		project
			.graph
			.get(*list_id)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<TrackListBehavior>())
			.is_some_and(|list| list.kind == TrackType::Video && list.tracks.contains(&track))
	});
	if !belongs_to_sequence {
		return clips;
	}
	let Some(track) = project
		.graph
		.get(track)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<TrackBehavior>())
	else {
		return clips;
	};
	if track.muted {
		return clips;
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
		if time < in_time || time >= out_time {
			continue;
		}
		let Some(footage_id) = ops::find_input_footage(&project.graph, *block_id) else {
			continue;
		};
		let Some(footage) = ops::footage_behavior(&project.graph, footage_id) else {
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
			effects: clip_effects(project, *block_id),
		});
	}
	clips
}

fn proxy_resolution(footage: &FootageBehavior) -> Option<(i32, i32)> {
	let params = footage
		.custom_proxy_params
		.clone()
		.unwrap_or_else(|| oak_codec::proxymanager::ProxyManager::proxy_params_from_config());
	if params.width > 0 && params.height > 0 {
		return Some((params.width, params.height));
	}
	let source = footage
		.streams
		.iter()
		.find(|stream| stream.is_video)?
		.video
		.as_ref()?;
	let divider = params.divider.max(1) as u32;
	Some((
		(source.width as u32 / divider).max(2) as i32,
		(source.height as u32 / divider).max(2) as i32,
	))
}

/// Clamps a sequence render to every active proxy's native resolution.
pub(crate) fn clamp_render_size(
	project: &Project,
	sequence: NodeId,
	time: Rational,
	width: i32,
	height: i32,
) -> (i32, i32) {
	let Some(sequence) = project
		.graph
		.get(sequence)
		.and_then(|entry| entry.behavior.as_any())
		.and_then(|behavior| behavior.downcast_ref::<SequenceBehavior>())
	else {
		return (width, height);
	};
	let mut limits = Vec::new();
	for list_id in &sequence.track_lists {
		let Some(list) = project
			.graph
			.get(*list_id)
			.and_then(|entry| entry.behavior.as_any())
			.and_then(|behavior| behavior.downcast_ref::<TrackListBehavior>())
		else {
			continue;
		};
		if list.kind != TrackType::Video {
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
			for block_id in &track.blocks {
				let Some(clip) = project
					.graph
					.get(*block_id)
					.and_then(|entry| entry.behavior.as_any())
					.and_then(|behavior| behavior.downcast_ref::<ClipBlockBehavior>())
				else {
					continue;
				};
				if time < clip.core.in_() || time >= clip.core.out() {
					continue;
				}
				let Some(footage_id) = ops::find_input_footage(&project.graph, *block_id) else {
					continue;
				};
				let Some(footage) = ops::footage_behavior(&project.graph, footage_id) else {
					continue;
				};
				if preview_media(footage).map(|media| media.0) != Some(footage.proxy.clone()) {
					continue;
				}
				if let Some(limit) = proxy_resolution(footage) {
					limits.push(limit);
				}
			}
		}
	}
	let mut scale = 1.0f64;
	for (limit_width, limit_height) in limits {
		if limit_width > 0 && limit_height > 0 {
			scale = scale
				.min(limit_width as f64 / width as f64)
				.min(limit_height as f64 / height as f64);
		}
	}
	if scale >= 1.0 {
		(width, height)
	} else {
		(
			((width as f64 * scale).round() as i32).max(2),
			((height as f64 * scale).round() as i32).max(2),
		)
	}
}

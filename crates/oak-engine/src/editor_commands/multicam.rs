//! Multicam graph queries and undoable session operations.
//!
//! Graph construction details and enable/disable/split behavior remain in
//! oak-timeline. This module resolves session nodes, groups related video/AFV
//! audio edits into one history entry, and exposes only semantic operations to
//! the dispatcher.

use super::prelude::*;
use super::{
	media::{push_command, push_multi_command},
	project::{
		clip_behavior, lock, node_ref, sequence_behavior, track_behavior, track_list_behavior,
	},
	sequence::{clip_track, find_input_footage, track_ids, track_list_of},
};
use oak_node::block::clip_input;
use oak_node::nodes::multicamnode::{CURRENT_INPUT, SEQUENCE_INPUT, SEQUENCE_TYPE_INPUT};

/// Resolved multicam state for UI and independent SDK consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MulticamState {
	pub sequence: NodeId,
	pub multicam: NodeId,
	pub clip: NodeId,
	pub source_count: i32,
	pub current_source: i32,
}

/// Finds the first sequence feeding a clip's texture chain.
pub fn clip_connected_sequence(graph: &Graph, clip: NodeId) -> Option<NodeId> {
	fn walk(graph: &Graph, node: NodeId, visited: &mut Vec<NodeId>) -> Option<NodeId> {
		if visited.contains(&node) {
			return None;
		}
		visited.push(node);
		for (from, _, _) in graph.input_connections(node) {
			if sequence_behavior(graph, from).is_some() {
				return Some(from);
			}
			if let Some(found) = walk(graph, from, visited) {
				return Some(found);
			}
		}
		None
	}
	let source = graph.connected_output(clip, clip_input::TEXTURE_INPUT, -1)?;
	if sequence_behavior(graph, source).is_some() {
		return Some(source);
	}
	walk(graph, source, &mut Vec::new())
}

/// Whether a clip's texture input is directly fed by a multicam node.
pub fn clip_is_multicam(graph: &Graph, clip: NodeId) -> bool {
	graph
		.connected_output(clip, clip_input::TEXTURE_INPUT, -1)
		.and_then(|source| graph.get(source))
		.is_some_and(|entry| entry.behavior.type_id() == "org.olivevideoeditor.Olive.multicam")
}

fn sequence_type(graph: &Graph, multicam: NodeId) -> TrackType {
	graph
		.get(multicam)
		.map(|entry| {
			let value = entry
				.core
				.standard_value(SEQUENCE_TYPE_INPUT, -1)
				.to_double() as i32;
			TrackType::from_c(value).unwrap_or(TrackType::Video)
		})
		.unwrap_or(TrackType::Video)
}

fn current_source(graph: &Graph, multicam: NodeId) -> i32 {
	graph
		.get(multicam)
		.map(|entry| entry.core.standard_value(CURRENT_INPUT, -1).to_double() as i32)
		.unwrap_or(-1)
}

fn source_count(graph: &Graph, multicam: NodeId) -> i32 {
	let Some(sequence) = graph.connected_output(multicam, SEQUENCE_INPUT, -1) else {
		return 0;
	};
	track_list_of(graph, sequence, sequence_type(graph, multicam))
		.and_then(|list| track_list_behavior(graph, list))
		.map(|list| list.tracks.len() as i32)
		.unwrap_or(0)
}

/// Returns the source sequence connected to a multicam node.
pub fn multicam_sequence(graph: &Graph, multicam: NodeId) -> Option<NodeId> {
	graph.connected_output(multicam, SEQUENCE_INPUT, -1)
}

/// Returns the angle's track inside the connected source sequence.
pub fn multicam_source_track(
	project: &ProjectRef,
	multicam: NodeId,
	source: i32,
) -> Option<NodeId> {
	if source < 0 {
		return None;
	}
	let guard = lock(project);
	let sequence = multicam_sequence(&guard.graph, multicam)?;
	track_ids(
		&guard.graph,
		sequence,
		sequence_type(&guard.graph, multicam),
	)
	.get(source as usize)
	.copied()
}

/// Resolves a clip's multicam node, source sequence and angle selection.
pub fn multicam_state(project: &ProjectRef, clip: NodeId) -> Option<MulticamState> {
	let multicam = oak_timeline::multicam::clip_find_multicam(&node_ref(project, clip))?;
	let guard = lock(project);
	let sequence = guard
		.graph
		.connected_output(multicam.id, SEQUENCE_INPUT, -1)?;
	Some(MulticamState {
		sequence,
		multicam: multicam.id,
		clip,
		source_count: source_count(&guard.graph, multicam.id),
		current_source: current_source(&guard.graph, multicam.id),
	})
}

/// Finds the multicam clip covering `time` on a sequence's video tracks.
pub fn multicam_clip_at(project: &ProjectRef, sequence: NodeId, time: Rational) -> Option<NodeId> {
	let candidates: Vec<NodeId> = {
		let guard = lock(project);
		track_ids(&guard.graph, sequence, TrackType::Video)
			.into_iter()
			.flat_map(|track| {
				track_behavior(&guard.graph, track)
					.map(|behavior| behavior.blocks.clone())
					.unwrap_or_default()
			})
			.filter(|&block| {
				clip_behavior(&guard.graph, block)
					.is_some_and(|clip| time >= clip.core.in_() && time < clip.core.out())
			})
			.collect()
	};
	candidates.into_iter().find(|&clip| {
		oak_timeline::multicam::clip_find_multicam(&node_ref(project, clip)).is_some()
	})
}

/// Enables or disables multicam on every valid selected clip as one undo entry.
pub fn set_multicam_enabled(
	project: &ProjectRef,
	clips: &[NodeId],
	enabled: bool,
) -> Result<(), String> {
	let mut all_clips = Vec::new();
	let mut eligible = Vec::new();
	{
		let guard = lock(project);
		for &clip in clips {
			if clip_behavior(&guard.graph, clip).is_none() {
				continue;
			}
			all_clips.push(clip);
			if let Some(sequence) = clip_connected_sequence(&guard.graph, clip) {
				eligible.push((clip, sequence));
			}
		}
	}
	if enabled {
		if eligible.is_empty() {
			return Ok(());
		}
		let mut groups: Vec<(NodeId, Vec<NodeRef>)> = Vec::new();
		for (clip, sequence) in eligible.iter().copied() {
			let index = groups
				.iter()
				.position(|(candidate, _)| *candidate == sequence)
				.unwrap_or_else(|| {
					groups.push((sequence, Vec::new()));
					groups.len() - 1
				});
			groups[index].1.push(node_ref(project, clip));
		}
		let children = groups
			.into_iter()
			.map(|(sequence, clips)| {
				oak_timeline::multicam::multicam_enable(clips, node_ref(project, sequence))
			})
			.collect();
		push_multi_command(
			children,
			&oak_timeline::multicam::enable_label(eligible.len()),
		)
	} else {
		if all_clips.is_empty() {
			return Ok(());
		}
		let clip_refs = all_clips
			.iter()
			.map(|&clip| node_ref(project, clip))
			.collect();
		push_command(
			oak_timeline::multicam::multicam_disable(clip_refs),
			&oak_timeline::multicam::disable_label(all_clips.len()),
		)
	}
}

fn afv_audio_switch_commands(
	project: &ProjectRef,
	clip: NodeId,
	source: i32,
) -> Option<Vec<oak_undo::undocommand::UndoCommand>> {
	let (audio_clip, old_footage) = {
		let guard = lock(project);
		let audio_clip = guard.graph.links_of(clip).into_iter().find(|&linked| {
			clip_track(&guard.graph, linked)
				.and_then(|track| track_behavior(&guard.graph, track))
				.is_some_and(|track| track.kind == TrackType::Audio)
		})?;
		let old_footage = find_input_footage(&guard.graph, audio_clip);
		(audio_clip, old_footage)
	};
	let old_footage = old_footage?;
	let target_footage = {
		let guard = lock(project);
		let multicam = guard
			.graph
			.connected_output(clip, clip_input::TEXTURE_INPUT, -1)?;
		let sequence = guard.graph.connected_output(multicam, SEQUENCE_INPUT, -1)?;
		let list = track_list_of(&guard.graph, sequence, TrackType::Audio)?;
		let audio_track = track_list_behavior(&guard.graph, list)?
			.tracks
			.get(source as usize)?;
		let angle_clip = track_behavior(&guard.graph, *audio_track)?.blocks.first()?;
		find_input_footage(&guard.graph, *angle_clip)?
	};
	if old_footage == target_footage {
		return Some(Vec::new());
	}
	let disconnect_project = project.clone();
	let reconnect_project = project.clone();
	let disconnect = oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut guard = lock(&disconnect_project);
			for (from, input, element) in guard.graph.input_connections(audio_clip) {
				if input == clip_input::TEXTURE_INPUT {
					guard.graph.disconnect(from, audio_clip, &input, element);
				}
			}
		},
		move || {
			let mut guard = lock(&reconnect_project);
			let _ = guard
				.graph
				.connect(old_footage, audio_clip, clip_input::TEXTURE_INPUT, -1);
		},
	);
	let connect_project = project.clone();
	let restore_project = project.clone();
	let connect = oak_undo::undocommand::UndoCommand::from_closures(
		move || {
			let mut guard = lock(&connect_project);
			for (from, input, element) in guard.graph.input_connections(audio_clip) {
				if input == clip_input::TEXTURE_INPUT {
					guard.graph.disconnect(from, audio_clip, &input, element);
				}
			}
			let _ = guard
				.graph
				.connect(target_footage, audio_clip, clip_input::TEXTURE_INPUT, -1);
		},
		move || {
			let mut guard = lock(&restore_project);
			for (from, input, element) in guard.graph.input_connections(audio_clip) {
				if input == clip_input::TEXTURE_INPUT {
					guard.graph.disconnect(from, audio_clip, &input, element);
				}
			}
			let _ = guard
				.graph
				.connect(old_footage, audio_clip, clip_input::TEXTURE_INPUT, -1);
		},
	);
	Some(vec![disconnect, connect])
}

/// Switches video and linked AFV audio in one undo group, optionally splitting
/// at `playhead` through oak-timeline's identity-preserving split command.
pub fn switch_multicam(
	project: &ProjectRef,
	clip: NodeId,
	source: i32,
	split_clip: bool,
	playhead: Rational,
) -> Result<(), String> {
	let Some(state) = multicam_state(project, clip) else {
		return Ok(());
	};
	if source < 0 || source >= state.source_count || source == state.current_source {
		return Ok(());
	}
	let mut commands = vec![oak_timeline::multicam::multicam_switch(
		node_ref(project, clip),
		source,
		split_clip,
		playhead,
	)];
	if let Some(audio) = afv_audio_switch_commands(project, clip, source) {
		commands.extend(audio);
	}
	push_multi_command(commands, oak_timeline::multicam::SWITCH_LABEL)
}

//! Static and animated parameter edits with exact per-track undo restoration.

use oak_core::Rational;
use oak_node::id::NodeId;
use oak_node::keyframe::{Interpolation, Keyframe, KeyframeTrack};
use oak_node::value::OakNodeValue;

use super::prelude::*;
use super::{push, push_command, project::lock};

fn input_type(p: &ProjectRef, node: NodeId, input: &str) -> Result<oak_node::value::ValueType, String> {
	let guard = lock(p);
	guard
		.graph
		.get(node)
		.and_then(|entry| entry.core.input_data_type(input))
		.ok_or_else(|| format!("parameter: node or input \"{input}\" not found"))
}

/// Replaces one `(input, element)` track on undo. A previously absent track is
/// restored as empty instead of deleting the node or unrelated parameter data.
fn restore_track_command(
	p: &ProjectRef,
	node: NodeId,
	input: &str,
	element: i32,
	old: Option<KeyframeTrack>,
	new: KeyframeTrack,
	label: &'static str,
) -> Result<(), String> {
	let redo_project = p.clone();
	let undo_project = p.clone();
	let redo_input = input.to_string();
	let undo_input = input.to_string();
	push_command(
		oak_undo::undocommand::UndoCommand::from_closures(
			move || {
				if let Some(entry) = lock(&redo_project).graph.get_mut(node) {
					*entry.core.keyframe_track_mut(&redo_input, element) = new.clone();
				}
			},
			move || {
				if let Some(entry) = lock(&undo_project).graph.get_mut(node) {
					*entry.core.keyframe_track_mut(&undo_input, element) = old.clone().unwrap_or_default();
				}
			},
		),
		label,
	)
}

/// Inserts or replaces a complete keyframe, including interpolation and both
/// relative Bezier handles. POD value conversion uses the declared input type.
pub fn upsert_keyframe(
	p: &ProjectRef,
	node: NodeId,
	input: &str,
	element: i32,
	time: Rational,
	value: OakNodeValue,
	interpolation: Interpolation,
	bezier_in: (f64, f64),
	bezier_out: (f64, f64),
) -> Result<(), String> {
	let declared = input_type(p, node, input)?;
	let value = value
		.to_node_value(declared)
		.map_err(|_| "keyframe: incompatible POD value".to_string())?;
	let old = lock(p)
		.graph
		.get(node)
		.and_then(|entry| entry.core.keyframe_track(input, element))
		.cloned();
	let mut new = old.clone().unwrap_or_default();
	new.set_key(Keyframe {
		time,
		value,
		interpolation,
		bezier_in,
		bezier_out,
	});
	restore_track_command(p, node, input, element, old, new, "Set Keyframe")
}

/// Removes the keyframe at an exact rational time. Missing tracks or keys are
/// errors; undo restores the complete prior track, including handles.
pub fn remove_keyframe(
	p: &ProjectRef,
	node: NodeId,
	input: &str,
	element: i32,
	time: Rational,
) -> Result<(), String> {
	let old = lock(p)
		.graph
		.get(node)
		.and_then(|entry| entry.core.keyframe_track(input, element))
		.cloned();
	let Some(old) = old else {
		return Err("keyframe: track not found".into());
	};
	if !old.keys().iter().any(|key| key.time == time) {
		return Err("keyframe: no key at that time".into());
	}
	let mut new = old.clone();
	if !new.remove_key(time) {
		return Err("keyframe: no key at that time".into());
	}
	restore_track_command(p, node, input, element, Some(old), new, "Remove Keyframe")
}

/// Sets a value at `time` using oak-node's standard/keyframed mutation rules.
/// Existing tracks insert or replace keys; unanimated inputs set standard value.
pub fn set_value_at_time(
	p: &ProjectRef,
	node: NodeId,
	input: &str,
	element: i32,
	time: Rational,
	value: OakNodeValue,
) -> Result<(), String> {
	let declared = input_type(p, node, input)?;
	let value = value
		.to_node_value(declared)
		.map_err(|_| "parameter: incompatible POD value".to_string())?;
	let command = {
		let guard = lock(p);
		oak_node::ops::set_value_at_time_command(p, &guard.graph, node, input, element, time, &value)
			.map_err(|e| e.to_string())?
	};
	push(command, "Set Parameter at Time")
}

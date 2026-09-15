// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Multi-camera editing commands (C++
//! `app/widget/multicam/multicamwidget.cpp::Switch`,
//! `app/widget/timelinewidget/timelinewidget.cpp::multicam_enabled_triggered`).
//!
//! A multi-cam clip is a clip whose effect input (`tex_in`, C++
//! `buffer_in`) is fed by a [`MultiCamNode`] (`oak_node::nodes::multicamnode`).
//! The node's `sequence_in` connects to the sequence whose track list
//! supplies the angles; `sources_in` carries no real edges — the render
//! resolves element `i` to track `i` of the selected track list
//! (`sequence_type_in`).
//!
//! The three operations:
//!
//! * [`multicam_enable`] inserts a fresh `MultiCamNode` between the
//!   sequence and each clip: every input the sequence fed along the clip's
//!   dependency chain is re-routed through the node, `sequence_in` is
//!   connected, and `sequence_type_in` mirrors the clip's track type.
//! * [`multicam_disable`] is the reverse: the node's outputs are re-wired
//!   straight to the sequence and the node is removed.
//! * [`multicam_switch`] changes `current_in`. With `split_clip` and the
//!   playhead strictly inside the clip, the clip (and its linked blocks)
//!   is first split preserving links — each new half owns an independent
//!   copy of the clip's dependency graph (incl. its own `MultiCamNode`),
//!   so the halves after the playhead switch source while the halves
//!   before keep theirs.
//!
//! Every operation is exposed as a single undo command ([`UndoCommand`]);
//! the caller pushes it with the labels
//! [`ENABLE_LABEL`]/[`DISABLE_LABEL`]/[`SWITCH_LABEL`] (or the
//! `enable_label`/`disable_label` helpers), matching the C++ undo names.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use oak_core::Rational;
use oak_node::block::clip_input;
use oak_node::graph::{Graph, NodeEntry};
use oak_node::id::NodeId;
use oak_node::nodes::multicamnode::{
	MultiCamNode, CURRENT_INPUT, SEQUENCE_INPUT, SEQUENCE_TYPE_INPUT,
};
use oak_node::project::Project;
use oak_node::track::TrackType;
use oak_node::value::NodeValue;
use oak_undo::undocommand::UndoCommand;

use crate::undosplit::BlockSplitPreservingLinksCommand;
use crate::util::{block_in, block_kind, block_out, block_track, BlockKind, NodeRef};

/// Undo label for [`multicam_enable`] (C++
/// `tr("Multi-Cam Enabled On %1 Clip(s)")`).
pub const ENABLE_LABEL: &str = "Multi-Cam Enabled On %1 Clip(s)";
/// Undo label for [`multicam_disable`] (C++
/// `tr("Multi-Cam Disabled On %1 Clip(s)")`).
pub const DISABLE_LABEL: &str = "Multi-Cam Disabled On %1 Clip(s)";
/// Undo label for [`multicam_switch`] (C++
/// `tr("Switched Multi-Camera Source")`).
pub const SWITCH_LABEL: &str = "Switched Multi-Camera Source";

/// Format [`ENABLE_LABEL`] with a clip count.
pub fn enable_label(clip_count: usize) -> String {
	ENABLE_LABEL.replace("%1", &clip_count.to_string())
}

/// Format [`DISABLE_LABEL`] with a clip count.
pub fn disable_label(clip_count: usize) -> String {
	DISABLE_LABEL.replace("%1", &clip_count.to_string())
}

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

/// `ClipBlock::find_multicam()` — the `MultiCamNode` feeding the clip's
/// effect input (`tex_in`, C++ `buffer_in`), searched at depth 1 and then
/// along the dependency chain exactly like the C++
/// `find_input_nodes_connected_to_input<MultiCamNode>(input, 1)`
/// (`// CPP-PARITY: clip.cpp:657-665`).
pub fn clip_find_multicam(clip: &NodeRef) -> Option<NodeRef> {
	let project = clip.project.clone();
	{
		let p = project
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner());
		let mut list = Vec::new();
		find_input_nodes_connected_to_input_internal(
			&p.graph,
			clip.id,
			clip_input::TEXTURE_INPUT,
			-1,
			1,
			&mut list,
		);
		list.first().copied()
	}
	.map(|id| NodeRef::new(project, id))
}

/// One step of the C++ `find_input_nodes_connected_to_input`: the node
/// feeding `node.input[element]`, then a recursive walk of its inputs,
/// stopping after `maximum` matches (`maximum == 0` = unlimited).
fn find_input_nodes_connected_to_input_internal(
	graph: &Graph,
	node: NodeId,
	input: &str,
	element: i32,
	maximum: usize,
	list: &mut Vec<NodeId>,
) {
	let Some(source) = graph.connected_output(node, input, element) else {
		return;
	};
	if is_multicam(graph, source) {
		list.push(source);
		if maximum != 0 && list.len() == maximum {
			return;
		}
	}
	find_input_node_internal(graph, source, maximum, list);
}

/// C++ `find_input_node_internal` — walk `node`'s input connections,
/// checking each source and recursing.
fn find_input_node_internal(graph: &Graph, node: NodeId, maximum: usize, list: &mut Vec<NodeId>) {
	for (from, _input, _element) in graph.input_connections(node) {
		if is_multicam(graph, from) {
			list.push(from);
			if maximum != 0 && list.len() == maximum {
				return;
			}
		}
		find_input_node_internal(graph, from, maximum, list);
		if maximum != 0 && list.len() == maximum {
			return;
		}
	}
}

/// Whether `id` names a `MultiCamNode`.
fn is_multicam(graph: &Graph, id: NodeId) -> bool {
	graph
		.get(id)
		.and_then(|e| e.behavior.as_any())
		.and_then(|a| a.downcast_ref::<MultiCamNode>())
		.is_some()
}

/// Set/clear the `MultiCamNode` sequence state after a `sequence_in` edge
/// edit (the graph arena fires no behavior events, so the command keeps the
/// behavior in sync — C++ `InputConnectedEvent`/`InputDisconnectedEvent`).
fn sync_multicam_sequence(graph: &mut Graph, mc: NodeId, sequence: Option<NodeId>) {
	let Some(entry) = graph.get_mut(mc) else {
		return;
	};
	if let Some(mc_node) = entry
		.behavior
		.as_any_mut()
		.and_then(|a| a.downcast_mut::<MultiCamNode>())
	{
		mc_node.set_sequence(&mut entry.core, sequence);
	}
}

/// C++ `Node::find_ways_node_arrives_here()` — every input slot along
/// `node`'s dependency chain that is fed (directly or transitively) by
/// `output`. Returns `(target, input_id, element)` where the target's
/// input is directly fed by `output`.
fn find_ways_node_arrives_here(
	graph: &Graph,
	output: NodeId,
	node: NodeId,
	v: &mut Vec<(NodeId, String, i32)>,
) {
	for (from, input, element) in graph.input_connections(node) {
		if from == output {
			v.push((node, input, element));
		} else {
			find_ways_node_arrives_here(graph, output, from, v);
		}
	}
}

/// The `current_in` value of a multicam node (its currently selected
/// source; `-1` when the node is stale).
fn multicam_current_source(graph: &Graph, mc: NodeId) -> i32 {
	graph
		.get(mc)
		.map(|e| e.core.standard_value(CURRENT_INPUT, -1).to_double() as i32)
		.unwrap_or(-1)
}

/// Set the `current_in` standard value of a multicam node.
fn multicam_set_current(graph: &mut Graph, mc: NodeId, source: i32) {
	if let Some(entry) = graph.get_mut(mc) {
		entry
			.core
			.set_standard_value(CURRENT_INPUT, -1, NodeValue::Combo(source as i64));
	}
}

/// The clip's track media type (C++ `track_type_of(block_track(c))`);
/// defaults to video when the clip is trackless.
fn clip_track_type(clip: &NodeRef) -> TrackType {
	block_track(clip)
		.and_then(|track| {
			let p = track
				.project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			p.graph
				.get(track.id)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<oak_node::track::TrackBehavior>())
				.map(|t| t.kind)
		})
		.unwrap_or(TrackType::Video)
}

/// Linked block ids of `block` in the project graph (C++
/// `Block::block_links()`).
fn block_links(clip: &NodeRef) -> Vec<NodeRef> {
	let project = clip.project.clone();
	let p = project
		.lock()
		.unwrap_or_else(|poisoned| poisoned.into_inner());
	p.graph
		.links_of(clip.id)
		.into_iter()
		.map(|id| NodeRef::new(project.clone(), id))
		.collect()
}

/// For every clip in `clips`, the `(clip, multicam, old_current)` tuple for
/// each multicam found on the clip or its clip links (C++ Switch: set the
/// source on the new clip's multicam and every linked clip's multicam).
fn collect_switch_targets(clip: &NodeRef) -> Vec<(NodeRef, NodeId, i32)> {
	let project = clip.project.clone();
	let mut targets = Vec::new();
	let mut seen: HashSet<NodeId> = HashSet::new();
	for c in std::iter::once(clip.clone()).chain(block_links(clip)) {
		if !seen.insert(c.id) {
			continue;
		}
		if block_kind(&c) != BlockKind::Clip {
			continue;
		}
		if let Some(mc) = clip_find_multicam(&c) {
			let p = project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			let old = multicam_current_source(&p.graph, mc.id);
			targets.push((c, mc.id, old));
		}
	}
	targets
}

// ---------------------------------------------------------------------------
// MultiCamEnableCommand
// ---------------------------------------------------------------------------

/// Per-clip state of [`MultiCamEnableCommand`].
struct EnableState {
	/// The multicam node created for the clip (valid after the first redo).
	multicam: Option<NodeId>,
	/// Detached arena entry while the node is out of the graph between
	/// `undo` and the next `redo`.
	entry: Option<NodeEntry>,
	/// `sequence_type_in` value (the clip's track type ordinal).
	sequence_type: i32,
	/// Inputs the sequence fed along the clip's chain before the enable
	/// (`(target, input_id, element)`), re-routed through the multicam.
	rerouted: Vec<(NodeId, String, i32)>,
}

/// `MultiCamEnableCommand` — wrap each clip's source through a fresh
/// `MultiCamNode` (C++ `multicam_enabled_triggered(true)`). One undo
/// command covering every clip; push with [`enable_label`].
pub struct MultiCamEnableCommand {
	/// Clips to enable multicam on.
	clips: Vec<NodeRef>,
	/// The sequence whose track list supplies the angles.
	sequence: NodeRef,
	/// Per-clip state, built on the first redo.
	state: Vec<EnableState>,
}

impl MultiCamEnableCommand {
	/// Construct from clips + sequence.
	pub fn new(clips: Vec<NodeRef>, sequence: NodeRef) -> Self {
		Self {
			clips,
			sequence,
			state: Vec::new(),
		}
	}

	/// `prepare`: plan each clip — create its multicam node and record the
	/// sequence-fed inputs to re-route.
	fn prepare(&mut self) {
		if !self.state.is_empty() {
			return;
		}
		let project = self.sequence.project.clone();
		for clip in &self.clips {
			let mc = {
				let (core, behavior) = oak_node::nodes::multicamnode::create();
				let mut p = project
					.lock()
					.unwrap_or_else(|poisoned| poisoned.into_inner());
				p.graph.add_node(core, behavior)
			};
			let sequence_type = clip_track_type(clip).to_c();
			let rerouted = {
				let p = project
					.lock()
					.unwrap_or_else(|poisoned| poisoned.into_inner());
				let mut v = Vec::new();
				find_ways_node_arrives_here(&p.graph, self.sequence.id, clip.id, &mut v);
				v
			};
			self.state.push(EnableState {
				multicam: Some(mc),
				entry: None,
				sequence_type,
				rerouted,
			});
		}
	}

	/// `redo`: (re-)insert the multicam nodes and re-route the sequence
	/// edges through them.
	pub fn redo(&mut self) {
		self.prepare();
		let project = self.sequence.project.clone();
		for st in self.state.iter_mut() {
			let Some(mc) = st.multicam else {
				continue;
			};
			let mut p = project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			// Re-attach the node if a previous undo detached it.
			if let Some(entry) = st.entry.take() {
				p.graph.add_entry(entry, mc);
			}
			// Set the sequence type selector (the clip's track type).
			if let Some(entry) = p.graph.get_mut(mc) {
				entry.core.set_standard_value(
					SEQUENCE_TYPE_INPUT,
					-1,
					NodeValue::Combo(st.sequence_type as i64),
				);
			}
			// Disconnect the sequence from each input it fed and connect
			// the multicam in its place.
			for (target, input, element) in &st.rerouted {
				p.graph
					.disconnect(self.sequence.id, *target, input, *element);
				p.graph.connect(mc, *target, input, *element).ok();
			}
			// Connect the sequence to the multicam's sequence_in.
			p.graph.disconnect(self.sequence.id, mc, SEQUENCE_INPUT, -1);
			p.graph
				.connect(self.sequence.id, mc, SEQUENCE_INPUT, -1)
				.ok();
			// Keep the behavior's cached sequence state in sync.
			sync_multicam_sequence(&mut p.graph, mc, Some(self.sequence.id));
		}
	}

	/// `undo`: disconnect the multicam, re-connect the sequence straight to
	/// each original input, and detach the multicam node.
	pub fn undo(&mut self) {
		let project = self.sequence.project.clone();
		for st in self.state.iter_mut() {
			let Some(mc) = st.multicam else {
				continue;
			};
			let mut p = project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			// The multicam fed every re-routed input; wire the sequence
			// back directly.
			for (target, input, element) in &st.rerouted {
				p.graph.disconnect(mc, *target, input, *element);
				p.graph
					.connect(self.sequence.id, *target, input, *element)
					.ok();
			}
			// Drop the sequence_in edge.
			p.graph.disconnect(self.sequence.id, mc, SEQUENCE_INPUT, -1);
			// Detach the multicam node (identity preserved for the next
			// redo).
			if st.entry.is_none() {
				st.entry = p.graph.take_node(mc);
			}
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		crate::undocommon::box_command(self)
	}
}

impl crate::undocommon::Command for MultiCamEnableCommand {
	fn redo(&mut self) {
		self.redo();
	}

	fn undo(&mut self) {
		self.undo();
	}
}

/// Build the enable command (C++ `multicam_enabled_triggered(true)`).
pub fn multicam_enable(clips: Vec<NodeRef>, sequence: NodeRef) -> UndoCommand {
	MultiCamEnableCommand::new(clips, sequence).to_command()
}

// ---------------------------------------------------------------------------
// MultiCamDisableCommand
// ---------------------------------------------------------------------------

/// Per-clip state of [`MultiCamDisableCommand`].
struct DisableState {
	/// The multicam node currently feeding the clip.
	multicam: NodeId,
	/// The sequence the multicam pulled angles from (from its
	/// `sequence_in` edge).
	sequence: NodeId,
	/// The multicam's output edges `(target, input_id, element)` that
	/// must be re-wired straight to the sequence.
	outputs: Vec<(NodeId, String, i32)>,
	/// Detached arena entry while the node is out of the graph between
	/// `undo` and the next `redo`.
	entry: Option<NodeEntry>,
}

/// `MultiCamDisableCommand` — bypass a clip's `MultiCamNode` and remove it
/// (C++ `multicam_enabled_triggered(false)`). One undo command covering
/// every clip; push with [`disable_label`].
pub struct MultiCamDisableCommand {
	/// Clips to disable multicam on.
	clips: Vec<NodeRef>,
	/// Per-clip state, built on the first redo.
	state: Vec<DisableState>,
}

impl MultiCamDisableCommand {
	/// Construct from clips.
	pub fn new(clips: Vec<NodeRef>) -> Self {
		Self {
			clips,
			state: Vec::new(),
		}
	}

	/// `prepare`: locate each clip's multicam and the outputs to re-wire.
	fn prepare(&mut self) {
		if !self.state.is_empty() {
			return;
		}
		let clips = self.clips.clone();
		for clip in clips {
			let Some(mc) = clip_find_multicam(&clip) else {
				continue;
			};
			let (sequence, outputs) = {
				let p = clip
					.project
					.lock()
					.unwrap_or_else(|poisoned| poisoned.into_inner());
				let sequence = p
					.graph
					.connected_output(mc.id, SEQUENCE_INPUT, -1)
					.unwrap_or(NodeId::INVALID);
				let outputs = p.graph.output_connections(mc.id);
				(sequence, outputs)
			};
			if sequence == NodeId::INVALID {
				// No connected sequence: nothing to bypass back to; leave
				// the node untouched.
				continue;
			}
			self.state.push(DisableState {
				multicam: mc.id,
				sequence,
				outputs,
				entry: None,
			});
		}
	}

	/// `redo`: re-wire the multicam's outputs straight to the sequence and
	/// detach the multicam node.
	pub fn redo(&mut self) {
		self.prepare();
		let mut project: Option<Arc<Mutex<Project>>> = None;
		for st in self.state.iter_mut() {
			if project.is_none() {
				if let Some(clip) = self.clips.first() {
					project = Some(clip.project.clone());
				} else {
					return;
				}
			}
			let project = project.as_ref().expect("set above");
			let mut p = project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			// Re-attach the node if a previous undo detached it.
			if let Some(entry) = st.entry.take() {
				p.graph.add_entry(entry, st.multicam);
			}
			// Re-wire outputs to the sequence.
			for (target, input, element) in &st.outputs {
				p.graph.disconnect(st.multicam, *target, input, *element);
				p.graph.connect(st.sequence, *target, input, *element).ok();
			}
			// Detach the multicam (the sequence_in edge goes with it).
			if st.entry.is_none() {
				st.entry = p.graph.take_node(st.multicam);
			}
		}
	}

	/// `undo`: re-insert the multicam node, re-route the outputs through it
	/// (disconnecting the sequence), and re-connect the sequence_in edge.
	pub fn undo(&mut self) {
		// `undo` only runs after `redo`, which returned early when `clips`
		// was empty, so a project is always available here.
		let project = self
			.clips
			.first()
			.map(|c| c.project.clone())
			.unwrap_or_else(Project::new);
		for st in self.state.iter_mut() {
			let mut p = project
				.lock()
				.unwrap_or_else(|poisoned| poisoned.into_inner());
			if let Some(entry) = st.entry.take() {
				p.graph.add_entry(entry, st.multicam);
			}
			// The sequence now feeds every output directly; re-route them
			// through the multicam and restore the sequence_in edge.
			for (target, input, element) in &st.outputs {
				p.graph.disconnect(st.sequence, *target, input, *element);
				p.graph.connect(st.multicam, *target, input, *element).ok();
			}
			p.graph
				.connect(st.sequence, st.multicam, SEQUENCE_INPUT, -1)
				.ok();
			sync_multicam_sequence(&mut p.graph, st.multicam, Some(st.sequence));
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		crate::undocommon::box_command(self)
	}
}

impl crate::undocommon::Command for MultiCamDisableCommand {
	fn redo(&mut self) {
		self.redo();
	}

	fn undo(&mut self) {
		self.undo();
	}
}

/// Build the disable command (C++ `multicam_enabled_triggered(false)`).
pub fn multicam_disable(clips: Vec<NodeRef>) -> UndoCommand {
	MultiCamDisableCommand::new(clips).to_command()
}

// ---------------------------------------------------------------------------
// MultiCamSwitchCommand
// ---------------------------------------------------------------------------

/// `MultiCamSwitchCommand` — change the multicam source, optionally
/// splitting the clip at the playhead first (C++ `MulticamWidget::Switch`).
/// One undo command for the whole operation; push with [`SWITCH_LABEL`].
pub struct MultiCamSwitchCommand {
	/// The clip whose multicam source changes.
	clip: NodeRef,
	/// The new source index.
	source: i32,
	/// Whether to split the clip at the playhead (source change applies
	/// from the playhead forward).
	split_clip: bool,
	/// The playhead time (split point).
	playhead: Rational,
	/// The split command, when `split_clip` and the playhead lies strictly
	/// inside the clip.
	split: Option<BlockSplitPreservingLinksCommand>,
	/// Whether the clip was actually split on the first redo.
	did_split: bool,
	/// `(clip, multicam, old_source)` targets captured on the first redo;
	/// the ids stay stable across undo/redo (the split re-attaches its
	/// copies identity-preserving).
	targets: Vec<(NodeRef, NodeId, i32)>,
	/// True when the clip has no multicam: the command is a no-op (the C++
	/// `if (!node_) return;` guard).
	noop: bool,
}

impl MultiCamSwitchCommand {
	/// Construct from clip + source + split flag + playhead.
	pub fn new(clip: NodeRef, source: i32, split_clip: bool, playhead: Rational) -> Self {
		Self {
			clip,
			source,
			split_clip,
			playhead,
			split: None,
			did_split: false,
			targets: Vec::new(),
			noop: false,
		}
	}

	/// `redo`: split (if requested and the playhead is strictly inside),
	/// then write `current_in` on the affected multicam copies.
	pub fn redo(&mut self) {
		if self.split.is_none() && !self.noop {
			if clip_find_multicam(&self.clip).is_none() {
				// C++ `if (!node_) return;` — nothing to switch.
				self.noop = true;
				return;
			}
			if self.split_clip {
				let clip_in = block_in(&self.clip);
				let clip_out = block_out(&self.clip);
				if clip_in < self.playhead && self.playhead < clip_out {
					// Split the clip and every linked block, preserving
					// links. Each half keeps its own multicam copy.
					let mut blocks = vec![self.clip.clone()];
					blocks.extend(block_links(&self.clip));
					let times = vec![self.playhead];
					let mut split = BlockSplitPreservingLinksCommand::new(blocks, times);
					split.redo();
					let new_clip = split.get_split(&self.clip, 0);
					self.did_split = true;
					// Targets come from the new (post-playhead) halves.
					if let Some(new_clip) = new_clip {
						self.targets = collect_switch_targets(&new_clip);
					}
					self.split = Some(split);
				}
			}
			if !self.did_split {
				// No split (split disabled or playhead outside the clip):
				// the whole clip (and its links) switch source.
				self.targets = collect_switch_targets(&self.clip);
			}
		} else if self.did_split {
			// Redo after undo: re-run the split (the halves re-attach
			// identity-preserving) and re-apply the stored targets.
			if let Some(split) = self.split.as_mut() {
				split.redo();
			}
		}

		// Write the new source on every captured multicam.
		let project = self.clip.project.clone();
		let mut p = project
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner());
		for (_clip, mc, _old) in &self.targets {
			multicam_set_current(&mut p.graph, *mc, self.source);
		}
	}

	/// `undo`: restore the old sources and undo the split.
	pub fn undo(&mut self) {
		// Restore the previous sources first (the targets are still
		// attached); undoing the split afterwards discards the copies.
		let project = self.clip.project.clone();
		let mut p = project
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner());
		for (_clip, mc, old) in &self.targets {
			multicam_set_current(&mut p.graph, *mc, *old);
		}
		drop(p);
		if self.did_split {
			if let Some(split) = self.split.as_mut() {
				split.undo();
			}
		}
	}

	/// Wrap as an oakundo command value.
	pub fn to_command(self) -> UndoCommand {
		crate::undocommon::box_command(self)
	}
}

impl crate::undocommon::Command for MultiCamSwitchCommand {
	fn redo(&mut self) {
		self.redo();
	}

	fn undo(&mut self) {
		self.undo();
	}
}

/// Build the switch command (C++ `MulticamWidget::Switch`).
pub fn multicam_switch(
	clip: NodeRef,
	source: i32,
	split_clip: bool,
	playhead: Rational,
) -> UndoCommand {
	MultiCamSwitchCommand::new(clip, source, split_clip, playhead).to_command()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use crate::util::{
		block_clip_create, block_in, block_length, block_out, same_block, track_append_block,
		track_block_at, track_block_count, NodeRef,
	};
	use oak_core::TimeRange;
	use oak_node::block::ClipBlockBehavior;
	use oak_node::node::NodeCore;
	use oak_node::sequence::SequenceBehavior;
	use oak_node::track::{TrackBehavior, TrackListBehavior};

	/// Project fixture: a sequence owning one video track list with one
	/// video track.
	struct Fixture {
		project: Arc<Mutex<Project>>,
		seq: NodeRef,
		track: NodeRef,
	}

	fn fixture() -> Fixture {
		let project = Project::new();
		let (seq_id, list_id, track_id) = {
			let mut p = project.lock().unwrap();
			let (core, behavior) = SequenceBehavior::create();
			let seq_id = p.graph.add_node(core, behavior);
			let (core, behavior) = TrackListBehavior::create();
			let list_id = p.graph.add_node(core, behavior);
			let (core, behavior) = (
				NodeCore::new(),
				Box::new(TrackBehavior::new(TrackType::Video)),
			);
			let track_id = p.graph.add_node(core, behavior);
			{
				let seq = p.graph.get_mut(seq_id).unwrap();
				let s = seq
					.behavior
					.as_any_mut()
					.unwrap()
					.downcast_mut::<SequenceBehavior>()
					.unwrap();
				s.track_lists.push(list_id);
			}
			let list = p.graph.get_mut(list_id).unwrap();
			let l = list
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<TrackListBehavior>()
				.unwrap();
			l.sequence = Some(seq_id);
			l.tracks.push(track_id);
			let track = p.graph.get_mut(track_id).unwrap();
			let t = track
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<TrackBehavior>()
				.unwrap();
			t.kind = TrackType::Video;
			t.track_list = Some(list_id);
			(seq_id, list_id, track_id)
		};
		let _ = list_id;
		Fixture {
			project: project.clone(),
			seq: NodeRef::new(project.clone(), seq_id),
			track: NodeRef::new(project, track_id),
		}
	}

	/// Add a clip spanning `[in, out)` on the fixture's video track.
	fn add_clip(fx: &Fixture, in_: Rational, out: Rational) -> NodeRef {
		let clip = block_clip_create(&fx.project);
		{
			let mut p = fx.project.lock().unwrap();
			let e = p.graph.get_mut(clip.id).unwrap();
			let c = e
				.behavior
				.as_any_mut()
				.unwrap()
				.downcast_mut::<ClipBlockBehavior>()
				.unwrap();
			c.core.range = TimeRange::new(in_, out);
		}
		track_append_block(&fx.track, &clip);
		clip
	}

	/// The pre-enable topology: the sequence feeds the clip's effect input.
	fn connect_sequence_to_clip(fx: &Fixture, clip: &NodeRef) {
		let mut p = fx.project.lock().unwrap();
		p.graph
			.connect(fx.seq.id, clip.id, clip_input::TEXTURE_INPUT, -1)
			.unwrap();
	}

	/// The multicam node currently feeding `clip` (via `clip_find_multicam`).
	#[allow(unused_variables)]
	fn multicam_of(_fx: &Fixture, clip: &NodeRef) -> Option<NodeRef> {
		clip_find_multicam(clip)
	}

	/// The multicam node feeding `clip`, asserted present.
	fn multicam_of_expect(fx: &Fixture, clip: &NodeRef) -> NodeRef {
		multicam_of(fx, clip).expect("clip has a multicam")
	}

	/// The `current_in` value of a multicam node.
	fn current_of(fx: &Fixture, mc: &NodeRef) -> i32 {
		let p = fx.project.lock().unwrap();
		multicam_current_source(&p.graph, mc.id)
	}

	/// Set the `current_in` of a multicam node (test setup for non-default
	/// initial source).
	fn set_current(fx: &Fixture, mc: &NodeRef, source: i32) {
		let mut p = fx.project.lock().unwrap();
		multicam_set_current(&mut p.graph, mc.id, source);
	}

	/// The `sequence_type_in` value of a multicam node.
	fn sequence_type_of(fx: &Fixture, mc: &NodeRef) -> i64 {
		let p = fx.project.lock().unwrap();
		let v = p
			.graph
			.get(mc.id)
			.unwrap()
			.core
			.standard_value(SEQUENCE_TYPE_INPUT, -1);
		match v {
			NodeValue::Combo(i) => i,
			other => panic!("sequence_type_in is {other:?}, expected Combo"),
		}
	}

	#[test]
	fn labels_format_count() {
		assert_eq!(enable_label(3), "Multi-Cam Enabled On 3 Clip(s)");
		assert_eq!(disable_label(2), "Multi-Cam Disabled On 2 Clip(s)");
		assert_eq!(SWITCH_LABEL, "Switched Multi-Camera Source");
	}

	/// `clip_find_multicam` is None for a plain clip and Some after the
	/// sequence is routed through a multicam (the C++
	/// `ClipBlock::find_multicam` parity).
	#[test]
	fn find_multicam_positive_and_negative() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));

		// No multicam anywhere: nothing found.
		assert!(multicam_of(&fx, &clip).is_none());

		// A non-multicam source on the effect input is not reported either.
		connect_sequence_to_clip(&fx, &clip);
		assert!(multicam_of(&fx, &clip).is_none());

		// After the enable command, the multicam feeds the clip.
		let mut cmd = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		cmd.redo();
		let mc = multicam_of_expect(&fx, &clip);
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph.get(mc.id).unwrap().behavior.type_id(),
				"org.olivevideoeditor.Olive.multicam"
			);
		}

		// After undo the multicam is gone again.
		cmd.undo();
		assert!(multicam_of(&fx, &clip).is_none());
	}

	/// `multicam_enable` re-routes the sequence→clip edges through a fresh
	/// multicam node and sets `sequence_type_in`; undo restores the exact
	/// pre-enable edges.
	#[test]
	fn enable_reroutes_through_multicam_and_undo_restores() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));
		connect_sequence_to_clip(&fx, &clip);

		let mut cmd = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		cmd.redo();

		let mc = multicam_of_expect(&fx, &clip);
		{
			let p = fx.project.lock().unwrap();
			// The sequence no longer feeds the clip; the multicam does.
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(mc.id)
			);
			// The sequence feeds the multicam's sequence_in.
			assert_eq!(
				p.graph.connected_output(mc.id, SEQUENCE_INPUT, -1),
				Some(fx.seq.id)
			);
		}
		// The type selector mirrors the video track type.
		assert_eq!(sequence_type_of(&fx, &mc), 0);

		cmd.undo();
		// Back to the pre-enable edge; the multicam node is gone.
		assert!(multicam_of(&fx, &clip).is_none());
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(fx.seq.id)
			);
			assert!(!p.graph.is_valid(mc.id));
		}

		// Redo re-creates the multicam (fresh node id) and re-routes again.
		cmd.redo();
		let mc2 = multicam_of_expect(&fx, &clip);
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(mc2.id)
			);
		}
	}

	/// `multicam_disable` re-wires the multicam's outputs back to the
	/// sequence and removes the node; undo restores the enabled state.
	#[test]
	fn disable_round_trip() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));
		connect_sequence_to_clip(&fx, &clip);

		let mut enable = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		enable.redo();
		let mc = multicam_of_expect(&fx, &clip);

		let mut disable = MultiCamDisableCommand::new(vec![clip.clone()]);
		disable.redo();
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(fx.seq.id)
			);
			assert!(!p.graph.is_valid(mc.id));
		}

		disable.undo();
		let restored_mc = multicam_of_expect(&fx, &clip);
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(restored_mc.id)
			);
			assert_eq!(
				p.graph.connected_output(restored_mc.id, SEQUENCE_INPUT, -1),
				Some(fx.seq.id)
			);
		}
	}

	/// `multicam_switch` without splitting sets the source on the clip and
	/// its linked clips' multicams; a single undo restores every source.
	#[test]
	fn switch_no_split_updates_clip_and_links() {
		let fx = fixture();
		let clip_a = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));
		let clip_b = add_clip(&fx, Rational::new(50, 1), Rational::new(100, 1));
		connect_sequence_to_clip(&fx, &clip_a);
		connect_sequence_to_clip(&fx, &clip_b);
		{
			let mut p = fx.project.lock().unwrap();
			p.graph.link(clip_a.id, clip_b.id);
		}
		let mut enable =
			MultiCamEnableCommand::new(vec![clip_a.clone(), clip_b.clone()], fx.seq.clone());
		enable.redo();
		let mc_a = multicam_of_expect(&fx, &clip_a);
		let mc_b = multicam_of_expect(&fx, &clip_b);
		set_current(&fx, &mc_a, 1);
		set_current(&fx, &mc_b, 1);

		// Split disabled: the switch writes the source directly.
		let mut cmd = MultiCamSwitchCommand::new(clip_a.clone(), 2, false, Rational::new(30, 1));
		cmd.redo();
		assert_eq!(current_of(&fx, &mc_a), 2);
		assert_eq!(current_of(&fx, &mc_b), 2);
		// No split happened: still one block per track slot.
		assert_eq!(track_block_count(&fx.track), 2);

		cmd.undo();
		assert_eq!(current_of(&fx, &mc_a), 1);
		assert_eq!(current_of(&fx, &mc_b), 1);
		assert_eq!(track_block_count(&fx.track), 2);
	}

	/// `multicam_switch` with a playhead strictly inside the clip splits it
	/// preserving links: the halves each own an independent multicam copy,
	/// the post-playhead half switches source and its linked half follows,
	/// the pre-playhead half keeps the old source. Undo restores the single
	/// clip and its original source.
	#[test]
	fn switch_splits_and_copies_multicam() {
		let fx = fixture();
		let clip_a = add_clip(&fx, Rational::new(0, 1), Rational::new(100, 1));
		let clip_b = add_clip(&fx, Rational::new(0, 1), Rational::new(100, 1));
		// Same track layout: clip_b on a second track slot is created after;
		// link the two clips so the split is link-preserving.
		{
			let mut p = fx.project.lock().unwrap();
			p.graph.link(clip_a.id, clip_b.id);
		}
		connect_sequence_to_clip(&fx, &clip_a);
		connect_sequence_to_clip(&fx, &clip_b);
		let mut enable =
			MultiCamEnableCommand::new(vec![clip_a.clone(), clip_b.clone()], fx.seq.clone());
		enable.redo();
		let mc_a = multicam_of_expect(&fx, &clip_a);
		let mc_b = multicam_of_expect(&fx, &clip_b);
		set_current(&fx, &mc_a, 1);
		set_current(&fx, &mc_b, 1);

		// Switch at t=40 (strictly inside [0,100)) with split.
		let mut cmd = MultiCamSwitchCommand::new(clip_a.clone(), 3, true, Rational::new(40, 1));
		cmd.redo();

		// Two halves per clip (both linked clips split): 4 blocks total.
		assert_eq!(track_block_count(&fx.track), 4);
		let first = track_block_at(&fx.track, 0).unwrap();
		let second = track_block_at(&fx.track, 1).unwrap();
		assert!(
			same_block(&first, &clip_a),
			"original clip keeps the in half"
		);
		assert_eq!(block_in(&first), Rational::new(0, 1));
		assert_eq!(block_out(&first), Rational::new(40, 1));
		assert_eq!(block_in(&second), Rational::new(40, 1));
		assert_eq!(block_out(&second), Rational::new(100, 1));

		// The pre-playhead half keeps its own multicam and old source; the
		// post-playhead half has a distinct multicam copy with the new
		// source.
		let mc_first = multicam_of_expect(&fx, &first);
		let mc_second = multicam_of_expect(&fx, &second);
		assert_ne!(mc_first.id, mc_second.id, "independent multicam copies");
		assert_eq!(current_of(&fx, &mc_first), 1);
		assert_eq!(current_of(&fx, &mc_second), 3);

		// The linked clip's post-playhead half switched too.
		let linked_second = {
			let p = fx.project.lock().unwrap();
			// The second half of clip_a links to the second half of clip_b.
			let links = p.graph.links_of(second.id);
			links.iter().find(|id| **id != second.id).copied().unwrap()
		};
		assert_ne!(linked_second, clip_a.id);
		let mc_linked = multicam_of_expect(&fx, &NodeRef::new(fx.project.clone(), linked_second));
		assert_eq!(current_of(&fx, &mc_linked), 3);

		// A single undo restores the two original clips and the old sources.
		cmd.undo();
		assert_eq!(track_block_count(&fx.track), 2);
		assert_eq!(block_length(&clip_a), Rational::new(100, 1));
		assert_eq!(current_of(&fx, &multicam_of_expect(&fx, &clip_a)), 1);
		assert_eq!(current_of(&fx, &multicam_of_expect(&fx, &clip_b)), 1);

		// Redo re-splits (the copies re-attach identity-preserving) and
		// re-applies the new source.
		cmd.redo();
		assert_eq!(track_block_count(&fx.track), 4);
		let second_again = track_block_at(&fx.track, 1).unwrap();
		let mc_second_again = multicam_of_expect(&fx, &second_again);
		assert_eq!(current_of(&fx, &mc_second_again), 3);
		let mc_first_again = multicam_of_expect(&fx, &track_block_at(&fx.track, 0).unwrap());
		assert_eq!(current_of(&fx, &mc_first_again), 1);
	}

	/// The split's dependency-graph copy gives the two halves independent
	/// multicam nodes: mutating one half's multicam does not affect the
	/// other's.
	#[test]
	fn split_copies_dependency_graph_independently() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(100, 1));
		connect_sequence_to_clip(&fx, &clip);
		let mut enable = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		enable.redo();
		let mc_orig = multicam_of_expect(&fx, &clip);

		// Split manually through the split command.
		let mut split =
			crate::undosplit::BlockSplitCommand::new(clip.clone(), Rational::new(40, 1));
		split.prepare();
		split.redo();
		let second = split.new_block().unwrap();

		let mc_first = multicam_of_expect(&fx, &clip);
		let mc_second = multicam_of_expect(&fx, &second);
		assert_ne!(mc_first.id, mc_second.id);
		assert_ne!(mc_orig.id, mc_second.id);

		// Both copies kept the original source value.
		assert_eq!(current_of(&fx, &mc_first), 0);
		assert_eq!(current_of(&fx, &mc_second), 0);

		// Changing the second half's source leaves the first half's copy
		// untouched.
		let mut p = fx.project.lock().unwrap();
		multicam_set_current(&mut p.graph, mc_second.id, 5);
		drop(p);
		assert_eq!(current_of(&fx, &mc_first), 0);
		assert_eq!(current_of(&fx, &mc_second), 5);
	}

	/// A switch with `split_clip` but a playhead outside the clip does not
	/// split; it writes the source directly (C++ only splits when the
	/// playhead lies strictly inside).
	#[test]
	fn switch_outside_playhead_does_not_split() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));
		connect_sequence_to_clip(&fx, &clip);
		let mut enable = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		enable.redo();
		let mc = multicam_of_expect(&fx, &clip);

		let mut cmd = MultiCamSwitchCommand::new(
			clip.clone(),
			2,
			true,
			Rational::new(70, 1), // past the out point
		);
		cmd.redo();
		assert_eq!(track_block_count(&fx.track), 1);
		assert_eq!(current_of(&fx, &mc), 2);

		cmd.undo();
		assert_eq!(current_of(&fx, &mc), 0);
	}

	/// A switch on a clip without any multicam is a no-op (the C++
	/// `if (!node_) return;` guard) — no split, no writes.
	#[test]
	fn switch_without_multicam_is_noop() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(100, 1));

		let mut cmd = MultiCamSwitchCommand::new(clip.clone(), 2, true, Rational::new(40, 1));
		cmd.redo();
		// No split happened.
		assert_eq!(track_block_count(&fx.track), 1);
		cmd.undo();
		assert_eq!(track_block_count(&fx.track), 1);
	}

	#[test]
	fn disable_skips_clips_without_sequence_connection() {
		let fx = fixture();
		let clip = add_clip(&fx, Rational::new(0, 1), Rational::new(50, 1));
		connect_sequence_to_clip(&fx, &clip);
		let mut enable = MultiCamEnableCommand::new(vec![clip.clone()], fx.seq.clone());
		enable.redo();
		let mc = multicam_of_expect(&fx, &clip);

		// A second clip without any multicam is a no-op for the disable.
		let bare = add_clip(&fx, Rational::new(50, 1), Rational::new(100, 1));
		let mut disable = MultiCamDisableCommand::new(vec![clip.clone(), bare.clone()]);
		disable.redo();
		{
			let p = fx.project.lock().unwrap();
			assert_eq!(
				p.graph
					.connected_output(clip.id, clip_input::TEXTURE_INPUT, -1),
				Some(fx.seq.id)
			);
			assert!(!p.graph.is_valid(mc.id));
		}
		// The bare clip is untouched.
		assert!(multicam_of(&fx, &bare).is_none());
	}
}

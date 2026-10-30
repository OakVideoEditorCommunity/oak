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

//! M12 P2: the real node-graph surface.
//!
//! Builds the gpui node-graph data (`RealNode` / `RealPort` / `RealEdge`)
//! from the CURRENT SEQUENCE's graph (M14 R3: the direct oaknode graph walk
//! — the app links the module rlibs and reads the project graph itself):
//!
//! - the sequence node becomes the output card (rightmost);
//! - every clip block becomes a card titled with its label (or "Clip");
//! - effects (everything else in the sequence graph) sit between;
//! - footage nodes (the media) feed the clip's `tex_in` from the left;
//! - declared inputs become input ports (id string as the label); every
//!   node exposes one "out" output port (the module declares no outputs;
//!   edges are enumerated from `Graph::output_connections`);
//! - REAL edges connect the source node's main output to the target
//!   node's input port; a synthesized "clip → output" wire per clip
//!   connects the clip's main output to the sequence's `tex_in` (the
//!   module's place-block flow never wires blocks to the sequence node —
//!   the C++ reaches it through track nodes, which do not exist here);
//! - positions come from the sequence node's context position map, with a
//!   deterministic role-grid fallback layout (footage | effects | clips |
//!   output) when a node has no entry yet — the first drag persists
//!   through the undoable position setter.
//!
//! Identity mapping: `NodeId` = the module node identity (stable across
//! frames). `PortId` packs `(node identity, kind, index)` — input port
//! `(id << 4) | (index << 1)`, output port `(id << 4) | 1`. Identities
//! are arena slots (low index bits, zero generation for most nodes), so
//! the shifts are injective.
//!
//! Structural timeline plumbing (track lists, tracks, gaps, transitions)
//! is NOT displayed: those nodes carry no graph edges in the module world
//! and would only add empty cards; the displayed graph is the media chain
//! clip → effects → output the C++ node editor centers on.

use std::collections::HashMap;

use crate::widgets::node_graph::{
	EdgeData, EdgeId, NodeData, NodeId, PortData, PortId, PortDataType, PortKind,
};
use gpui_kit::{hsla, point, px, Hsla, Pixels, Point, SharedString};

use quercus_engine_rs::node::NodeId as DomainNodeId;

use crate::oakui::graphops::{self, ProjectRef};

/// The sequence node type id (the graph's output card).
const TYPE_ID_SEQUENCE: &str = "org.olivevideoeditor.Olive.sequence";
/// Clip block type id (the graph's clip cards).
const TYPE_ID_CLIP_BLOCK: &str = "org.olivevideoeditor.Olive.clipblock";
/// Footage node type id (the graph's media cards).
const TYPE_ID_FOOTAGE: &str = "org.olivevideoeditor.Olive.footage";
/// Structural timeline nodes never shown in the node editor.
const TYPE_ID_TRACK: &str = "org.olivevideoeditor.Olive.track";
const TYPE_ID_TRACK_LIST: &str = "org.olivevideoeditor.Olive.tracklist";
const TYPE_ID_GAP_BLOCK: &str = "org.olivevideoeditor.Olive.gapblock";
const TYPE_ID_TRANSITION_BLOCK: &str = "org.olivevideoeditor.Olive.transitionblock";

/// The real factory's Add-menu entries, converted to app-owned DTOs.
pub fn node_library() -> Vec<super::engine::NodeLibraryEntry> {
	use super::engine::{NodeCategory, NodeLibraryEntry, node_category_key};
	fn category(value: quercus_engine_rs::effect::NodeCategory) -> NodeCategory {
		use quercus_engine_rs::effect::NodeCategory as In;
		match value {
			In::Output => NodeCategory::Output,
			In::Effect => NodeCategory::Effect,
			In::Generator => NodeCategory::Generator,
			In::Input => NodeCategory::Input,
			In::Math => NodeCategory::Math,
			In::Color => NodeCategory::Color,
			In::Distort => NodeCategory::Distort,
			In::Filter => NodeCategory::Filter,
			In::Keying => NodeCategory::Keying,
			In::OpenFx => NodeCategory::OpenFx,
			In::Timeline => NodeCategory::Timeline,
			In::Group => NodeCategory::Group,
		}
	}

	let mut out = Vec::new();
	for meta in quercus_engine_rs::effect::factory_entries() {
		if meta.flags & quercus_engine_rs::effect::NODE_FLAG_DONT_SHOW_IN_CREATE_MENU != 0 {
			continue;
		}
		let Some(category_key) = meta
			.category
			.map(category)
			.and_then(node_category_key)
		else {
			continue;
		};
		out.push(NodeLibraryEntry {
			type_id: meta.type_id.to_string(),
			name: if meta.name.is_empty() {
				meta.type_id.to_string()
			} else {
				meta.name.to_string()
			},
			category_key,
		});
	}
	for meta in quercus_engine_rs::effect::dynamic_factory_entries() {
		let category_key = node_category_key(category(quercus_engine_rs::effect::NodeCategory::OpenFx))
			.expect("the OpenFX category is visible in the node library");
		out.push(NodeLibraryEntry {
			type_id: meta.type_id,
			name: meta.name,
			category_key,
		});
	}
	out
}
/// The "video" wire type used by the real graph (the module exposes no
/// per-input type names; all node ports are treated as video).
fn video_type() -> PortDataType {
	PortDataType::new("video", hsla(0.55, 0.75, 0.6, 1.0))
}
/// The "output" wire type for the module's implicit outputs.
fn out_type() -> PortDataType {
	PortDataType::new("video", hsla(0.08, 0.7, 0.55, 1.0))
}
/// Pack a node identity + kind + index into a global `PortId`.
fn port_id(node: u64, kind: PortKind, index: u32) -> PortId {
	match kind {
		PortKind::Input => PortId((node << 4) | ((index as u64) << 1)),
		PortKind::Output => PortId((node << 4) | 1),
	}
}

/// Unpack a `PortId` into `(node identity, kind, index)`.
fn unpack_port(id: PortId) -> (u64, PortKind, u32) {
	let raw = id.0;
	let node = raw >> 4;
	if raw & 1 == 1 {
		(node, PortKind::Output, 0)
	} else {
		(node, PortKind::Input, ((raw >> 1) & 0x7) as u32)
	}
}

/// A stable REAL edge id from `(from node, to node, input id)` (FNV-1a,
/// high bit masked so it can never collide with the synthesized-wire tag
/// [`is_output_wire`] reserves).
fn real_edge_id(from: u64, to: u64, input_id: &str) -> EdgeId {
	let mut h: u64 = 0xcbf29ce484222325;
	for &b in [from.to_le_bytes(), to.to_le_bytes()].concat().iter() {
		h ^= b as u64;
		h = h.wrapping_mul(0x100000001b3);
	}
	for &b in input_id.as_bytes() {
		h ^= b as u64;
		h = h.wrapping_mul(0x100000001b3);
	}
	EdgeId(h & 0x7fff_ffff_ffff_ffff)
}

/// The synthesized "clip → output" wire's id: tagged with the high bit so
/// the app can tell structural wires apart from real edges (whose ids are
/// masked below the tag — see [`real_edge_id`]).
fn output_wire_id(clip: u64) -> EdgeId {
	EdgeId(0x8000_0000_0000_0000 | real_edge_id(clip, clip, "out").0)
}

/// Whether `id` is a synthesized structural wire (never a real edge).
pub fn is_output_wire(id: EdgeId) -> bool {
	id.0 & 0x8000_0000_0000_0000 != 0
}

/// A node card in the real graph.
#[derive(Debug, Clone)]
pub struct RealNode {
	/// The module node identity.
	pub id: NodeId,
	/// Card title.
	pub title: SharedString,
	/// Graph-space position.
	pub position: Point<Pixels>,
	/// Input ports (top to bottom).
	pub inputs: Vec<RealPort>,
	/// The implicit output port (`out`); empty on the output endpoint,
	/// which is a pure sink.
	pub outputs: Vec<RealPort>,
	/// Category accent.
	pub header_color: Option<Hsla>,
	/// Collapsed state.
	pub collapsed: bool,
	/// Enabled state.
	pub enabled: bool,
	/// The virtual endpoint role, when this card is one of the fixed
	/// endpoint pair (`None` for ordinary effect / media cards).
	pub endpoint: Option<GraphEndpoint>,
}

impl NodeData for RealNode {
	type Port = RealPort;

	fn id(&self) -> NodeId {
		self.id
	}

	fn title(&self) -> SharedString {
		self.title.clone()
	}

	fn position(&self) -> Point<Pixels> {
		self.position
	}

	fn inputs(&self) -> Vec<Self::Port> {
		self.inputs.clone()
	}

	fn outputs(&self) -> Vec<Self::Port> {
		self.outputs.clone()
	}

	fn header_color(&self) -> Option<Hsla> {
		self.header_color
	}

	fn is_collapsed(&self) -> bool {
		self.collapsed
	}

	fn is_enabled(&self) -> bool {
		self.enabled
	}
}

/// A port on a real node.
#[derive(Debug, Clone)]
pub struct RealPort {
	/// Global port id.
	pub id: PortId,
	/// Direction.
	pub kind: PortKind,
	/// The node input id this port fronts ("" for the implicit output) —
	/// edges resolve to ports by this, not by the display label.
	pub input_id: SharedString,
	/// Port label.
	pub label: SharedString,
	/// Wire type.
	pub data_type: PortDataType,
	/// Whether an edge is attached.
	pub connected: bool,
}

impl PortData for RealPort {
	fn id(&self) -> PortId {
		self.id
	}

	fn kind(&self) -> PortKind {
		self.kind
	}

	fn label(&self) -> SharedString {
		self.label.clone()
	}

	fn data_type(&self) -> PortDataType {
		self.data_type.clone()
	}

	fn is_connected(&self) -> bool {
		self.connected
	}
}

/// A wire between two real nodes.
#[derive(Debug, Clone)]
pub struct RealEdge {
	/// Stable edge id.
	pub id: EdgeId,
	/// Source node.
	pub from_node: NodeId,
	/// Source output port.
	pub from_port: PortId,
	/// Target node.
	pub to_node: NodeId,
	/// Target input port.
	pub to_port: PortId,
}

impl EdgeData for RealEdge {
	fn id(&self) -> EdgeId {
		self.id
	}

	fn from_node(&self) -> NodeId {
		self.from_node
	}

	fn from_port(&self) -> PortId {
		self.from_port
	}

	fn to_node(&self) -> NodeId {
		self.to_node
	}

	fn to_port(&self) -> PortId {
		self.to_port
	}
}

/// Deterministic header accents per node identity.
fn node_color(ident: u64) -> Hsla {
	let hues = [0.55f32, 0.6, 0.08, 0.3, 0.78, 0.45, 0.9, 0.15];
	hsla(hues[(ident as usize) % hues.len()], 0.5, 0.35, 1.0)
}

/// Whether a sequence-graph node should be shown in the node editor: the
/// media chain (sequence output, clip blocks, footage, effects) only;
/// structural timeline plumbing (tracks, track lists, gaps, transitions)
/// is hidden.
fn is_displayed(type_id: &str) -> bool {
	!matches!(
		type_id,
		TYPE_ID_TRACK | TYPE_ID_TRACK_LIST | TYPE_ID_GAP_BLOCK | TYPE_ID_TRANSITION_BLOCK
	)
}

/// The virtual endpoint pair every project graph carries: `GraphInput`
/// (the evaluation walk's root) and `GraphOutput` (where the frame is
/// read). Endpoints are shown as fixed cards — the panel hides their edit
/// entries and filters delete requests naming them, and the graph model
/// itself refuses to remove or copy the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEndpoint {
	/// `GraphInput`: data enters through `feed_in`, leaves through `tex_out`.
	Input,
	/// `GraphOutput`: every branch converges on `tex_in`; a pure sink.
	Output,
}

/// Whether `type_id` names one of the virtual endpoints, and which.
pub fn endpoint_of_type_id(type_id: &str) -> Option<GraphEndpoint> {
	if type_id == "org.olivevideoeditor.Olive.graphinput" {
		Some(GraphEndpoint::Input)
	} else if type_id == "org.olivevideoeditor.Olive.graphoutput" {
		Some(GraphEndpoint::Output)
	} else {
		None
	}
}

/// The fixed header accent of an endpoint card: the pair reads as
/// structural furniture, not as one more effect (deliberately outside the
/// [`node_color`] palette — and the demo graph's hand-picked hues).
fn endpoint_color(endpoint: GraphEndpoint) -> Hsla {
	match endpoint {
		GraphEndpoint::Input => hsla(0.40, 0.55, 0.35, 1.0),
		GraphEndpoint::Output => hsla(0.85, 0.65, 0.45, 1.0),
	}
}

/// A displayed node: its domain id plus its type id.
struct TypedNode {
	/// The node's domain id.
	id: DomainNodeId,
	/// The node's identity.
	ident: u64,
	/// The node's type id.
	type_id: String,
}

/// The displayable nodes of ONE clip's context chain — the per-clip node
/// view the editor shows while that clip is selected: the clip block node
/// plus its effect chain in signal order. The chain (from
/// [`effectchain::chain`]) walks the `tex_in`/effect-input link all the way
/// to the media source, so it already contains the footage node feeding the
/// clip (`[footage, effect1, effect2, ...]`); concatenating the clip yields
/// the full `footage → effects → clip` chain. The sequence node is NOT part
/// of a clip's context — the chain is self-contained, so the filtered view
/// shows no output card and no synthesized clip→output wires.
fn clip_context_nodes(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
) -> Vec<TypedNode> {
	snapshot_nodes(snapshot, seq)
}

/// The displayable nodes of the sequence's graph, in identity order (the
/// sequence node itself exactly once).
fn graph_nodes(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
) -> Vec<TypedNode> {
	snapshot_nodes(snapshot, seq)
}

fn snapshot_nodes(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
) -> Vec<TypedNode> {
	let mut out: Vec<TypedNode> = snapshot
		.nodes
		.iter()
		.filter(|node| is_displayed(&node.type_id))
		.map(|node| TypedNode {
			id: graphops::id_of(node.id.raw()).expect("SDK node identity"),
			ident: node.id.raw(),
			type_id: node.type_id.clone(),
		})
		.filter(|node| node.id != seq)
		.collect();
	out.push(TypedNode {
		id: seq,
		ident: seq.raw(),
		type_id: TYPE_ID_SEQUENCE.into(),
	});
	out.sort_by_key(|node| node.ident);
	out
}

/// Converts an owned SDK graph snapshot into the GPUI graph DTOs. Layout,
/// colors and widget ID packing stay in the app; graph traversal stays in engine.
fn present_graph_snapshot(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
) -> (Vec<RealNode>, Vec<RealEdge>) {
	use quercus_engine_rs::editor::{GraphMediaType, GraphNodeRole};
	let seq_ident = seq.raw();
	let mut nodes = Vec::new();
	let mut edges = Vec::new();
	for node in &snapshot.nodes {
		let ident = node.id.raw();
		let endpoint = match node.role {
			GraphNodeRole::InputEndpoint => Some(GraphEndpoint::Input),
			GraphNodeRole::OutputEndpoint => Some(GraphEndpoint::Output),
			_ => None,
		};
		let inputs = node
			.inputs
			.iter()
			.map(|port| RealPort {
				id: port_id(ident, PortKind::Input, port.declaration_index as u32),
				kind: PortKind::Input,
				input_id: port.id.clone().into(),
				label: port.label.clone().into(),
				data_type: match port.media_type {
					Some(GraphMediaType::Samples) => PortDataType::new("samples", hsla(0.12, 0.65, 0.55, 1.0)),
					_ => video_type(),
				},
				connected: port.connected,
			})
			.collect();
		let outputs = node
			.output
			.as_ref()
			.map(|port| {
				vec![RealPort {
					id: port_id(ident, PortKind::Output, 0),
					kind: PortKind::Output,
					input_id: SharedString::new_static(""),
					label: port.label.clone().into(),
					data_type: out_type(),
					connected: port.connected,
				}]
			})
			.unwrap_or_default();
		let position = node
			.position
			.map(|(x, y)| point(px(x as f32), px(y as f32)))
			.unwrap_or_else(|| point(px(0.0), px(0.0)));
		nodes.push(RealNode {
			id: NodeId(ident),
			title: node.label.clone().into(),
			position,
			inputs,
			outputs,
			header_color: Some(match endpoint {
				Some(endpoint) => endpoint_color(endpoint),
				None => node_color(ident),
			}),
			collapsed: false,
			enabled: node.enabled,
			endpoint,
		});
	}
	for edge in &snapshot.edges {
		let to_port = nodes
			.iter()
			.find(|node| node.id.0 == edge.to.raw())
			.and_then(|node| node.inputs.iter().find(|port| port.input_id.as_ref() == edge.input_id))
			.map(|port| port.id)
			.unwrap_or_else(|| port_id(edge.to.raw(), PortKind::Input, 0));
		edges.push(RealEdge {
			id: if edge.synthetic {
				output_wire_id(edge.from.raw())
			} else {
				real_edge_id(edge.from.raw(), edge.to.raw(), &edge.input_id)
			},
			from_node: NodeId(edge.from.raw()),
			from_port: port_id(edge.from.raw(), PortKind::Output, 0),
			to_node: NodeId(edge.to.raw()),
			to_port,
		});
	}
	// Apply the existing deterministic role grid only to nodes without a
	// persisted position; role and row order match the legacy presentation.
	let mut row_at_role: HashMap<u32, u32> = HashMap::new();
	for node in &mut nodes {
		if node.position != point(px(0.0), px(0.0)) {
			continue;
		}
		let snapshot_node = snapshot.nodes.iter().find(|candidate| candidate.id.raw() == node.id.0)
			.expect("presentation nodes come from the same snapshot");
		let role = role_of(&snapshot_node.type_id, node.id.0 == seq_ident);
		let row = row_at_role.entry(role).or_insert(0);
		node.position = point(
			px(40.0 + (role as f32) * 260.0),
			px(40.0 + (*row as f32) * 180.0),
		);
		*row += 1;
	}
	(nodes, edges)
}

/// Builds one session snapshot over the caller's existing project Arc.
fn build_graph_impl(
	project: &ProjectRef,
	seq: DomainNodeId,
	clip: Option<DomainNodeId>,
) -> (Vec<RealNode>, Vec<RealEdge>) {
	let Ok(session) = session_for_graph(project) else {
		return (Vec::new(), Vec::new());
	};
	let sequence = quercus_engine_rs::node::NodeId::from_raw(seq.raw());
	let clip = clip.map(|id| quercus_engine_rs::node::NodeId::from_raw(id.raw()));
	let Some(snapshot) = session.graph_snapshot(sequence, clip) else {
		return (Vec::new(), Vec::new());
	};
	present_graph_snapshot(&snapshot, seq)
}

/// Creates a session over the exact project Arc used by the current app.
fn session_for_graph(
	project: &ProjectRef,
) -> Result<quercus_engine_rs::editor::EditorSession, String> {
	quercus_engine_rs::editor::EditorSession::from_project(project.clone())
		.map_err(|error| format!("failed to adopt the current project: {error}"))
}

/// Build the (nodes, edges) snapshot of the sequence's node graph.
pub fn build_graph(project: &ProjectRef, seq: DomainNodeId) -> (Vec<RealNode>, Vec<RealEdge>) {
	build_graph_impl(project, seq, None)
}

/// Build the (nodes, edges) snapshot of ONE clip's context chain — the
/// per-clip node view the editor shows while that clip is selected. The
/// chain is footage → effects → clip; the sequence output card and the
/// synthesized wires are not part of a clip's context. An invalid clip
/// identity yields an empty graph.
pub fn build_graph_for_clip(
	project: &ProjectRef,
	seq: DomainNodeId,
	clip_ident: u64,
) -> (Vec<RealNode>, Vec<RealEdge>) {
	let Some(clip) = graphops::id_of(clip_ident) else {
		return (Vec::new(), Vec::new());
	};
	build_graph_impl(project, seq, Some(clip))
}

/// The role column of a displayed node (footage | effects | clips |
/// output); drives the fallback grid's x position.
fn role_of(type_id: &str, is_output: bool) -> u32 {
	if is_output {
		3
	} else if type_id == TYPE_ID_FOOTAGE {
		0
	} else if type_id == TYPE_ID_CLIP_BLOCK {
		2
	} else {
		1 // effects
	}
}

/// The provisional position the builder assigns to an unplaced node: the
/// role column and the per-role row among the OTHER unplaced nodes in
/// sorted display order (mirrors the fallback grid in `build_graph`).
/// `apply_edit` uses it so the first drag release writes
/// `(displayed base + delta)` instead of `(origin + delta)`.
fn fallback_base(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
	ident: u64,
) -> (f64, f64) {
	fallback_base_in(snapshot, seq, &graph_nodes(snapshot, seq), ident)
		.expect("the moved node is part of the displayed graph")
}

/// The provisional position an unplaced node displays at within
/// `displayed` (the node editor's current view, in display order): the
/// role-grid base mirroring the fallback layout in `build_graph_impl`.
/// `None` when the node is not part of that view — the caller falls back
/// to the whole-sequence grid.
///
/// The view matters: the per-clip chain shows a subset, so a clip card at
/// the chain's row 0 can sit at row N of the full-sequence grid; taking
/// the base from the wrong grid teleports the card on the first drag.
fn fallback_base_in(
	snapshot: &quercus_engine_rs::editor::GraphSnapshot,
	seq: DomainNodeId,
	displayed: &[TypedNode],
	ident: u64,
) -> Option<(f64, f64)> {
	let seq_ident = seq.raw();
	let target = displayed.iter().find(|n| n.ident == ident)?;
	let target_role = role_of(&target.type_id, ident == seq_ident);
	let mut row: u32 = 0;
	for n in displayed {
		if n.ident == ident {
			break;
		}
		if role_of(&n.type_id, n.ident == seq_ident) != target_role {
			continue;
		}
		// Placed nodes do not consume a row.
		let placed = snapshot
			.nodes
			.iter()
			.find(|node| node.id.raw() == n.ident)
			.and_then(|node| node.position)
			.is_some_and(|(x, y)| x != 0.0 || y != 0.0);
		if !placed {
			row += 1;
		}
	}
	Some((
		40.0 + f64::from(target_role * 260),
		40.0 + f64::from(row * 180),
	))
}

/// Resolve a domain node id from a widget identity, validating it against
/// the graph.
// ---------------------------------------------------------------------------
// Connection rules and edit application
// ---------------------------------------------------------------------------

/// Whether connecting output port `from` to input port `to` is valid in
/// the graph: output → input, distinct nodes, and the target input must
/// exist and be free. The sequence node's inputs are connectable like any
/// other (its `tex_in` starts unconnected; a user wire replaces the
/// synthesized one once the graph grows real edges).
pub fn can_connect(project: &ProjectRef, from: PortId, to: PortId) -> bool {
	let (from_node, from_kind, _) = unpack_port(from);
	let (to_node, to_kind, to_index) = unpack_port(to);
	if from_kind != PortKind::Output || to_kind != PortKind::Input {
		return false;
	}
	if from_node == to_node {
		return false;
	}
	let Ok(session) = session_for_graph(project) else {
		return false;
	};
	let Some(sequence) = session.snapshot().current_sequence else {
		return false;
	};
	let Some(snapshot) = session.graph_snapshot(sequence, None) else {
		return false;
	};
	let Some(id_str) = snapshot
		.nodes
		.iter()
		.find(|node| node.id.raw() == to_node)
		.and_then(|node| node.inputs.get(to_index as usize))
		.map(|input| input.id.clone())
	else {
		return false;
	};
	if id_str.is_empty() {
		return false;
	}
	session.can_connect(
		quercus_engine_rs::node::NodeId::from_raw(from_node),
		quercus_engine_rs::node::NodeId::from_raw(to_node),
		&id_str,
		-1,
	)
}

/// Apply a node-graph edit to the sequence's graph (undoable through the
/// global undo stack).
pub fn apply_edit(
	project: &ProjectRef,
	seq: DomainNodeId,
	edit: &crate::widgets::node_graph::NodeGraphEvent,
) -> Result<(), String> {
	apply_edit_in_view(project, seq, None, edit)
}

/// [`apply_edit`] with the node editor's current view: while one clip is
/// selected the editor shows that clip's context chain (footage → effects
/// → clip), and the node-move fallback base must come from THAT view's
/// grid — taking it from the whole-sequence grid teleports the card on
/// the first drag release. `clip` is the selected clip's identity.
pub fn apply_edit_in_view(
	project: &ProjectRef,
	seq: DomainNodeId,
	clip: Option<u64>,
	edit: &crate::widgets::node_graph::NodeGraphEvent,
) -> Result<(), String> {
	use crate::widgets::node_graph::NodeGraphEvent;
	match edit {
		NodeGraphEvent::ConnectionRequested { from, to } => {
			let (from_node, from_kind, _) = unpack_port(*from);
			let (to_node, to_kind, to_index) = unpack_port(*to);
			if from_kind != PortKind::Output || to_kind != PortKind::Input {
				return Err("connection endpoints must be output → input".into());
			}
			let session = session_for_graph(project)?;
			let sequence = session
				.snapshot()
				.current_sequence
				.ok_or_else(|| "no sequence open".to_string())?;
			let snapshot = session
				.graph_snapshot(
					sequence,
					clip.map(quercus_engine_rs::node::NodeId::from_raw),
				)
				.ok_or_else(|| "node graph unavailable".to_string())?;
			let id_str = snapshot
				.nodes
				.iter()
				.find(|node| node.id.raw() == to_node)
				.and_then(|node| node.inputs.get(to_index as usize))
				.map(|input| input.id.clone())
				.filter(|id| !id.is_empty());
			let Some(id_str) = id_str else {
				return Err("target input missing".into());
			};
			session.execute(quercus_engine_rs::edit::EditCommand::Connect {
				from: quercus_engine_rs::node::NodeId::from_raw(from_node),
				to: quercus_engine_rs::node::NodeId::from_raw(to_node),
				input_id: id_str,
			})
			.map(|_| ())
		}
		NodeGraphEvent::DisconnectionRequested { edge } => {
			if is_output_wire(*edge) {
				// The synthesized clip → output wire is structural; the
				// graph has no such edge to remove.
				return Ok(());
			}
			let session = session_for_graph(project)?;
			let sequence = session
				.snapshot()
				.current_sequence
				.ok_or_else(|| "no sequence open".to_string())?;
			let snapshot = session
				.graph_snapshot(
					sequence,
					clip.map(quercus_engine_rs::node::NodeId::from_raw),
				)
				.ok_or_else(|| "node graph unavailable".to_string())?;
			let (to, conn_id) = snapshot
				.edges
				.iter()
				.find(|connection| {
					real_edge_id(connection.from.raw(), connection.to.raw(), &connection.input_id).0
						== edge.0
				})
				.map(|connection| (connection.to, connection.input_id.clone()))
				.ok_or_else(|| format!("edge {} not found", edge.0))?;
			session.execute(quercus_engine_rs::edit::EditCommand::Disconnect {
				to: quercus_engine_rs::node::NodeId::from_raw(to.raw()),
				input_id: conn_id,
			})
			.map(|_| ())
		}
		NodeGraphEvent::NodeMoveRequested { nodes, delta } => {
			for node in nodes {
				let session = session_for_graph(project)?;
				let snapshot = session
					.graph_snapshot(
						quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
						clip.map(quercus_engine_rs::node::NodeId::from_raw),
					)
					.ok_or_else(|| "node graph unavailable".to_string())?;
				let placed = snapshot
					.nodes
					.iter()
					.find(|candidate| candidate.id.raw() == node.0)
					.and_then(|candidate| candidate.position)
					.filter(|(x, y)| *x != 0.0 || *y != 0.0);
				let base = placed.unwrap_or_else(|| {
					if clip.is_some() {
						fallback_base_in(&snapshot, seq, &clip_context_nodes(&snapshot, seq), node.0)
							.unwrap_or_else(|| fallback_base(&snapshot, seq, node.0))
					} else {
						fallback_base(&snapshot, seq, node.0)
					}
				});
				session.execute(
					quercus_engine_rs::edit::EditCommand::SetContextPosition {
						node: quercus_engine_rs::node::NodeId::from_raw(node.0),
						context: quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
						x: base.0 + f64::from(delta.x.as_f32()),
						y: base.1 + f64::from(delta.y.as_f32()),
					},
				)?;
			}
			Ok(())
		}
		NodeGraphEvent::DeleteRequested { nodes, .. } => {
			for node in nodes {
				// The output node is the graph's context; never deleted.
				if node.0 == seq.raw() {
					continue;
				}
				session_for_graph(project)?.execute(quercus_engine_rs::edit::EditCommand::RemoveNode {
					node: quercus_engine_rs::node::NodeId::from_raw(node.0),
				})?;
			}
			Ok(())
		}
		_ => Ok(()),
	}
}

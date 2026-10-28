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

use oak_node::id::NodeId as DomainNodeId;

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
	fn category(value: oak_engine_rust_sdk::effect::NodeCategory) -> NodeCategory {
		use oak_engine_rust_sdk::effect::NodeCategory as In;
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
	for meta in oak_engine_rust_sdk::effect::factory_entries() {
		if meta.flags & oak_engine_rust_sdk::effect::NODE_FLAG_DONT_SHOW_IN_CREATE_MENU != 0 {
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
	for meta in oak_engine_rust_sdk::effect::dynamic_factory_entries() {
		let category_key = node_category_key(category(oak_engine_rust_sdk::effect::NodeCategory::OpenFx))
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
	use oak_node::nodes::graphendpoints::{GRAPH_INPUT_TYPE_ID, GRAPH_OUTPUT_TYPE_ID};
	if type_id == GRAPH_INPUT_TYPE_ID {
		Some(GraphEndpoint::Input)
	} else if type_id == GRAPH_OUTPUT_TYPE_ID {
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
fn clip_context_nodes(g: &oak_node::graph::Graph, clip: DomainNodeId) -> Vec<TypedNode> {
	let mut out: Vec<TypedNode> = Vec::new();
	// The clip block card (the chain's output end).
	out.push(TypedNode {
		id: clip,
		ident: clip.identity(),
		type_id: TYPE_ID_CLIP_BLOCK.into(),
	});
	// The media → effects chain, closest-to-source first (the same order
	// the effect stack lists cards in).
	for node in crate::oakui::effectchain::chain(g, clip) {
		out.push(TypedNode {
			id: node,
			ident: node.identity(),
			type_id: graphops::node_type_id(g, node),
		});
	}
	out
}

/// The displayable nodes of the sequence's graph, in identity order (the
/// sequence node itself exactly once).
fn graph_nodes(g: &oak_node::graph::Graph, seq: DomainNodeId) -> Vec<TypedNode> {
	let mut out: Vec<TypedNode> = g
		.node_ids()
		.into_iter()
		.filter_map(|id| {
			let type_id = graphops::node_type_id(g, id);
			if !is_displayed(&type_id) {
				return None;
			}
			Some(TypedNode {
				id,
				ident: id.identity(),
				type_id,
			})
		})
		.filter(|n| n.id != seq)
		.collect();
	// The sequence node itself: exactly one output card.
	out.push(TypedNode {
		id: seq,
		ident: seq.identity(),
		type_id: TYPE_ID_SEQUENCE.into(),
	});
	out.sort_by_key(|n| n.ident);
	out
}

/// Converts an owned SDK graph snapshot into the GPUI graph DTOs. Layout,
/// colors and widget ID packing stay in the app; graph traversal stays in engine.
fn present_graph_snapshot(
	snapshot: &oak_engine_rust_sdk::editor::GraphSnapshot,
	seq: DomainNodeId,
) -> (Vec<RealNode>, Vec<RealEdge>) {
	use oak_engine_rust_sdk::editor::{GraphMediaType, GraphNodeRole};
	let seq_ident = seq.identity();
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
	let sequence = oak_engine_rust_sdk::node::NodeId::from_raw(seq.identity());
	let clip = clip.map(|id| oak_engine_rust_sdk::node::NodeId::from_raw(id.identity()));
	let Some(snapshot) = session.graph_snapshot(sequence, clip) else {
		return (Vec::new(), Vec::new());
	};
	present_graph_snapshot(&snapshot, seq)
}

/// Creates a session over the exact project Arc used by the current app.
fn session_for_graph(
	project: &ProjectRef,
) -> Result<oak_engine_rust_sdk::editor::EditorSession, String> {
	let handle = graphops::storage_bind(project);
	oak_engine_rust_sdk::editor::EditorSession::adopt_storage_project_handle(
		oak_engine_rust_sdk::editor::StorageProjectHandle {
			ctx: handle.ctx,
			addref: handle.addref,
			release: handle.release,
			abi_version: handle.abi_version,
		},
	)
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
fn fallback_base(g: &oak_node::graph::Graph, seq: DomainNodeId, ident: u64) -> (f64, f64) {
	fallback_base_in(g, seq, &graph_nodes(g, seq), ident)
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
	g: &oak_node::graph::Graph,
	seq: DomainNodeId,
	displayed: &[TypedNode],
	ident: u64,
) -> Option<(f64, f64)> {
	let seq_ident = seq.identity();
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
		let placed = g
			.get(n.id)
			.and_then(|e| {
				e.core
					.context_positions
					.iter()
					.find(|(c, _, _)| *c == seq)
					.map(|(_, pos, _)| *pos)
			})
			.map(|(x, y)| x != 0.0 || y != 0.0)
			.unwrap_or(false);
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
fn find_node(g: &oak_node::graph::Graph, ident: u64) -> Result<DomainNodeId, String> {
	let Some(id) = graphops::id_of(ident) else {
		return Err(format!("node {ident} not found"));
	};
	if !g.is_valid(id) {
		return Err(format!("node {ident} not found"));
	}
	Ok(id)
}

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
	let guard = graphops::lock(project);
	let g = &guard.graph;
	let Ok(node) = find_node(g, to_node) else {
		return false;
	};
	let Some(entry) = g.get(node) else {
		return false;
	};
	let Some(id_str) = entry.core.inputs.get(to_index as usize).map(|i| i.id.clone()) else {
		return false;
	};
	if id_str.is_empty() {
		return false;
	}
	drop(guard);
	let Some(session) = session_for_graph(project).ok() else {
		return false;
	};
	session.can_connect(
		oak_engine_rust_sdk::node::NodeId::from_raw(from_node),
		oak_engine_rust_sdk::node::NodeId::from_raw(node.identity()),
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
			let (src, dst, id_str) = {
				let guard = graphops::lock(project);
				let g = &guard.graph;
				let src = find_node(g, from_node)?;
				let dst = find_node(g, to_node)?;
				let id_str = g
					.get(dst)
					.and_then(|e| e.core.inputs.get(to_index as usize))
					.map(|i| i.id.clone())
					.filter(|s| !s.is_empty());
				let Some(id_str) = id_str else {
					return Err("target input missing".into());
				};
				(src, dst, id_str)
			};
			session_for_graph(project)?.execute(oak_engine_rust_sdk::edit::EditCommand::Connect {
				from: oak_engine_rust_sdk::node::NodeId::from_raw(src.identity()),
				to: oak_engine_rust_sdk::node::NodeId::from_raw(dst.identity()),
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
			let (dst, conn_id) = {
				let guard = graphops::lock(project);
				let g = &guard.graph;
				let mut found = None;
				'outer: for (typed_from, to, conn_id, _) in g.output_connections_all() {
					if real_edge_id(typed_from.identity(), to.identity(), &conn_id).0 == edge.0 {
						found = Some((to, conn_id));
						break 'outer;
					}
				}
				found.ok_or_else(|| format!("edge {} not found", edge.0))?
			};
			session_for_graph(project)?.execute(oak_engine_rust_sdk::edit::EditCommand::Disconnect {
				to: oak_engine_rust_sdk::node::NodeId::from_raw(dst.identity()),
				input_id: conn_id,
			})
			.map(|_| ())
		}
		NodeGraphEvent::NodeMoveRequested { nodes, delta } => {
			for node in nodes {
				let (id, base) = {
					let guard = graphops::lock(project);
					let g = &guard.graph;
					let id = find_node(g, node.0)?;
					let placed = g
						.get(id)
						.and_then(|e| {
							e.core
								.context_positions
								.iter()
								.find(|(c, _, _)| *c == seq)
								.map(|(_, pos, _)| *pos)
						})
						.filter(|(x, y)| *x != 0.0 || *y != 0.0);
					let base = match placed {
						Some(pos) => pos,
						None => clip
							.and_then(|ident| graphops::id_of(ident))
							.and_then(|clip| fallback_base_in(g, seq, &clip_context_nodes(g, clip), node.0))
							.unwrap_or_else(|| fallback_base(g, seq, node.0)),
					};
					(id, base)
				};
				session_for_graph(project)?.execute(
					oak_engine_rust_sdk::edit::EditCommand::SetContextPosition {
						node: oak_engine_rust_sdk::node::NodeId::from_raw(id.identity()),
						context: oak_engine_rust_sdk::node::NodeId::from_raw(seq.identity()),
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
				if node.0 == seq.identity() {
					continue;
				}
				let id = {
					let guard = graphops::lock(project);
					find_node(&guard.graph, node.0)?
				};
				session_for_graph(project)?.execute(oak_engine_rust_sdk::edit::EditCommand::RemoveNode {
					node: oak_engine_rust_sdk::node::NodeId::from_raw(id.identity()),
				})?;
			}
			Ok(())
		}
		_ => Ok(()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::effectchain;
	use crate::oakui::graphops;
	use crate::widgets::node_graph::NodeGraphEvent;
	use oak_node::track::TrackType;
	use oak_node::value::ValueType;

	/// Serializes with the other app test modules (the process-global undo
	/// stack).
	fn stack_lock() -> std::sync::MutexGuard<'static, ()> {
		crate::oakui::graphops::test_lock()
	}

	/// Places an already-created block node onto the sequence's first video
	/// track through the same undoable command the timeline drag uses (so
	/// the block really gets its outgoing structural edges).
	fn place_on_first_video_track(p: &ProjectRef, seq: DomainNodeId, block: DomainNodeId) {
		let (list, index, in_r) = {
			let g = graphops::lock(p);
			let track = graphops::track_ids(&g.graph, seq, TrackType::Video)
				.first()
				.copied()
				.expect("the sequence has a video track");
			let list = graphops::track_behavior(&g.graph, track)
				.and_then(|t| t.track_list)
				.expect("the track has a list");
			let index = graphops::track_list_behavior(&g.graph, list)
				.and_then(|l| l.tracks.iter().position(|&t| t == track))
				.expect("the track is in its list") as i32;
			let tb = graphops::sequence_time_base(&g.graph, seq).expect("the sequence timebase");
			(list, index, graphops::ts_to_rational(0, tb))
		};
		graphops::push_command(
			oak_timeline::undopointer::TrackPlaceBlockCommand::new(
				graphops::node_ref(p, list),
				index,
				graphops::node_ref(p, block),
				in_r,
			)
			.to_command(),
			"Place Clip",
		)
		.expect("place the block");
	}

	/// The `NodeData` / `PortData` / `EdgeData` accessor impls expose their
	/// fields, and the port-id packing round-trips (including the output
	/// tag).
	#[test]
	fn data_traits_and_port_packing_expose_the_fields() {
		let node = RealNode {
			id: NodeId(7),
			title: "Card".into(),
			position: point(px(1.0), px(2.0)),
			inputs: Vec::new(),
			outputs: Vec::new(),
			header_color: Some(hsla(0.5, 0.5, 0.5, 1.0)),
			collapsed: true,
			enabled: false,
			endpoint: Some(GraphEndpoint::Input),
		};
		assert_eq!(NodeData::id(&node), NodeId(7));
		assert_eq!(NodeData::title(&node).as_ref(), "Card");
		assert_eq!(NodeData::position(&node), point(px(1.0), px(2.0)));
		assert!(NodeData::inputs(&node).is_empty());
		assert!(NodeData::outputs(&node).is_empty());
		assert_eq!(NodeData::header_color(&node), Some(hsla(0.5, 0.5, 0.5, 1.0)));
		assert!(NodeData::is_collapsed(&node));
		assert!(!NodeData::is_enabled(&node));

		let port = RealPort {
			id: PortId(9),
			kind: PortKind::Input,
			input_id: "tex_in".into(),
			label: "Texture".into(),
			data_type: video_type(),
			connected: true,
		};
		assert_eq!(PortData::id(&port), PortId(9));
		assert_eq!(PortData::kind(&port), PortKind::Input);
		assert_eq!(PortData::label(&port).as_ref(), "Texture");
		assert_eq!(PortData::data_type(&port).name.as_ref(), "video");
		assert!(PortData::is_connected(&port));

		let edge = RealEdge {
			id: EdgeId(3),
			from_node: NodeId(1),
			from_port: PortId(5),
			to_node: NodeId(2),
			to_port: PortId(11),
		};
		assert_eq!(EdgeData::id(&edge), EdgeId(3));
		assert_eq!(EdgeData::from_node(&edge), NodeId(1));
		assert_eq!(EdgeData::from_port(&edge), PortId(5));
		assert_eq!(EdgeData::to_node(&edge), NodeId(2));
		assert_eq!(EdgeData::to_port(&edge), PortId(11));

		// Output ports pack `(ident << 4) | 1` (index 0), inputs keep their
		// declaration index.
		assert_eq!(
			unpack_port(port_id(7, PortKind::Output, 3)),
			(7, PortKind::Output, 0)
		);
		assert_eq!(
			unpack_port(port_id(7, PortKind::Input, 3)),
			(7, PortKind::Input, 3)
		);
		assert!(is_output_wire(output_wire_id(7)));
		assert!(!is_output_wire(real_edge_id(1, 2, "tex_in")));

		// Endpoint type ids round-trip; anything else is no endpoint.
		use oak_node::nodes::graphendpoints::{GRAPH_INPUT_TYPE_ID, GRAPH_OUTPUT_TYPE_ID};
		assert_eq!(
			endpoint_of_type_id(GRAPH_INPUT_TYPE_ID),
			Some(GraphEndpoint::Input)
		);
		assert_eq!(
			endpoint_of_type_id(GRAPH_OUTPUT_TYPE_ID),
			Some(GraphEndpoint::Output)
		);
		assert_eq!(endpoint_of_type_id(TYPE_ID_CLIP_BLOCK), None);
		assert_eq!(endpoint_color(GraphEndpoint::Output), hsla(0.85, 0.65, 0.45, 1.0));
		assert_eq!(node_color(9), node_color(1));
		assert!(role_of(TYPE_ID_FOOTAGE, false) == 0);
		assert!(role_of(TYPE_ID_CLIP_BLOCK, false) == 2);
		assert!(role_of("x", true) == 3);
	}

	/// Builder rejection and placement arms: invalid sequence/clip ids
	/// yield empty graphs, persisted context positions win over the role
	/// grid, an empty sequence label falls back to the behavior name and
	/// the per-clip chain skips edges whose target is outside the view.
	#[test]
	fn build_graph_handles_placement_and_invalid_ids() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, clip, _effect, _footage) = project_with_chained_clip();

		// Invalid sequence / clip identities: id_of accepts any non-sentinel
		// identity, the graph validity check rejects it.
		let bogus = graphops::id_of(12_345).expect("a syntactically valid id");
		assert!(build_graph(&project, bogus).0.is_empty());
		assert!(build_graph_for_clip(&project, seq, 12_345).0.is_empty());
		// The invalid sentinel has no domain id at all.
		assert!(build_graph_for_clip(&project, seq, u64::from(u32::MAX)).0.is_empty());

		// Persist a context position: the builder must use it (not the
		// fallback grid), and the fallback skips placed nodes.
		graphops::push_command(
			graphops::set_context_position_command(&project, clip, seq, 120.0, 60.0)
				.expect("the position command"),
			"Set Position",
		)
		.expect("apply the position");
		let (nodes, _) = build_graph(&project, seq);
		let placed = nodes
			.iter()
			.find(|n| n.id == NodeId(clip.identity()))
			.expect("the clip card");
		assert_eq!(placed.position, point(px(120.0), px(60.0)));
		let unplaced = nodes
			.iter()
			.find(|n| n.id == NodeId(_footage.identity()))
			.expect("the footage card");
		assert_ne!(unplaced.position, point(px(0.0), px(0.0)));

		// An empty sequence label falls back to the behavior name.
		{
			let mut g = graphops::lock(&project);
			g.graph.get_mut(seq).expect("the sequence node").core.label.clear();
		}
		let (nodes, _) = build_graph(&project, seq);
		let seq_card = nodes
			.iter()
			.find(|n| n.id == NodeId(seq.identity()))
			.expect("the sequence card");
		assert!(!seq_card.title.is_empty());

		// A placed clip's outgoing wire targets a track that is not part of
		// the per-clip view, so the edge is skipped.
		let placed_clip = {
			let mut g = graphops::lock(&project);
			let (core, behavior) = oak_node::block::clip_create();
			g.graph.add_node(core, behavior)
		};
		place_on_first_video_track(&project, seq, placed_clip);
		let (chain, chain_edges) = build_graph_for_clip(&project, seq, placed_clip.identity());
		assert!(!chain.is_empty(), "the placed clip card exists");
		assert!(
			chain_edges.iter().all(|e| e.to_node != NodeId(seq.identity())),
			"the structural clip -> track edge does not leak into the clip view"
		);

		oak_engine_rust_sdk::node::undo_clear();
	}

	/// Stream-input port filtering: an empty display name falls back to the
	/// input id, an empty input id drops the port entirely.
	#[test]
	fn build_graph_filters_stream_input_ports() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, clip, _, _) = project_with_chained_clip();

		// The clip's texture input (a stream input that is not hidden).
		let input_index = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip)
				.expect("the clip node")
				.core
				.inputs
				.iter()
				.position(|i| {
					matches!(
						i.value_type,
						ValueType::Texture | ValueType::Samples
					) && i.flags & oak_node::input::flags::HIDDEN == 0
				})
				.expect("the clip has a stream input")
		};

		// Empty display name: the label falls back to the input id.
		{
			let mut g = graphops::lock(&project);
			let input = &mut g.graph.get_mut(clip).expect("clip").core.inputs[input_index];
			input.display_name.clear();
			input.id = "tex_in".to_string();
		}
		let (nodes, _) = build_graph(&project, seq);
		let card = nodes
			.iter()
			.find(|n| n.id == NodeId(clip.identity()))
			.expect("the clip card");
		let port = card
			.inputs
			.iter()
			.find(|p| p.input_id.as_ref() == "tex_in")
			.expect("the tex_in port");
		assert_eq!(port.label.as_ref(), "tex_in");

		// Empty input id: the port is dropped.
		{
			let mut g = graphops::lock(&project);
			g.graph.get_mut(clip).expect("clip").core.inputs[input_index]
				.id
				.clear();
		}
		let (nodes, _) = build_graph(&project, seq);
		let card = nodes
			.iter()
			.find(|n| n.id == NodeId(clip.identity()))
			.expect("the clip card");
		assert!(
			card.inputs.iter().all(|p| !p.input_id.is_empty()),
			"an input with an empty id is not exposed"
		);

		oak_engine_rust_sdk::node::undo_clear();
	}

	/// The connection rules reject wrong directions, same-node pairs,
	/// unknown nodes, missing/empty target inputs and already-connected
	/// inputs, and accept a free stream input.
	#[test]
	fn can_connect_rejects_the_invalid_pairs() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, _seq, _clip, effect, footage) = project_with_chained_clip();

		let (effect_input_id, effect_input_index) = {
			let g = graphops::lock(&project);
			let entry = g.graph.get(effect).expect("the effect node");
			let index = entry
				.core
				.inputs
				.iter()
				.position(|i| i.id == entry.core.effect_input)
				.expect("the effect input");
			(entry.core.effect_input.clone(), index)
		};
		let effect_input = port_id(effect.identity(), PortKind::Input, effect_input_index as u32);
		let footage_out = port_id(footage.identity(), PortKind::Output, 0);
		let clip_out = port_id(_clip.identity(), PortKind::Output, 0);
		let clip_input = port_id(_clip.identity(), PortKind::Input, 0);

		// Wrong direction and same-node pairs.
		assert!(!can_connect(&project, clip_input, effect_input));
		assert!(!can_connect(&project, clip_out, clip_input));
		// Unknown target node (a syntactically valid, non-arena identity).
		assert!(!can_connect(
			&project,
			clip_out,
			port_id(u64::from(u32::MAX) - 1, PortKind::Input, 0)
		));
		// Declaration index past the end of the target's inputs (the clip
		// declares far fewer than eight inputs).
		assert!(!can_connect(
			&project,
			clip_out,
			port_id(_clip.identity(), PortKind::Input, 7)
		));
		// The footage -> effect wire already exists: the target is busy.
		assert!(!can_connect(&project, footage_out, effect_input));
		assert!(!effect_input_id.is_empty());
		// A free clip -> sequence input is accepted (the sequence's tex_in).
		let seq_input = {
			let g = graphops::lock(&project);
			g.graph
				.get(_seq)
				.expect("the sequence node")
				.core
				.inputs
				.iter()
				.position(|i| {
					matches!(i.value_type, ValueType::Texture | ValueType::Samples)
						&& !i.id.is_empty()
				})
				.map(|index| port_id(_seq.identity(), PortKind::Input, index as u32))
				.expect("the sequence has a stream input")
		};
		assert!(can_connect(&project, clip_out, seq_input));

		// Empty input id: rejected.
		{
			let mut g = graphops::lock(&project);
			g.graph.get_mut(effect).expect("effect").core.inputs
				[effect_input_index]
				.id
				.clear();
		}
		assert!(!can_connect(&project, clip_out, effect_input));

		oak_engine_rust_sdk::node::undo_clear();
	}

	/// `apply_edit` covers every event: connection rejection/success,
	/// synthesized-wire disconnection, real-edge removal, node moves (both
	/// the fallback base and the persisted position) and node deletion
	/// (including the protected sequence and the no-op preview event).
	#[test]
	fn apply_edit_covers_connect_move_and_delete() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, clip, effect, _footage) = project_with_chained_clip();

		// Wrong endpoint kinds.
		let err = apply_edit(
			&project,
			seq,
			&NodeGraphEvent::ConnectionRequested {
				from: port_id(clip.identity(), PortKind::Input, 0),
				to: port_id(seq.identity(), PortKind::Input, 0),
			},
		)
		.expect_err("input -> input is invalid");
		assert!(err.contains("output"));

		// Unknown source node.
		assert!(apply_edit(
			&project,
			seq,
			&NodeGraphEvent::ConnectionRequested {
				from: port_id(123_456, PortKind::Output, 0),
				to: port_id(seq.identity(), PortKind::Input, 0),
			},
		)
		.is_err());

		// Target input index missing (the clip declares fewer than eight
		// inputs).
		assert!(apply_edit(
			&project,
			seq,
			&NodeGraphEvent::ConnectionRequested {
				from: port_id(seq.identity(), PortKind::Output, 0),
				to: port_id(clip.identity(), PortKind::Input, 7),
			},
		)
		.is_err());

		// A real connection: the clip output into the sequence's tex_in.
		let seq_input = {
			let g = graphops::lock(&project);
			g.graph
				.get(seq)
				.expect("the sequence node")
				.core
				.inputs
				.iter()
				.position(|i| {
					matches!(i.value_type, ValueType::Texture | ValueType::Samples)
						&& !i.id.is_empty()
				})
				.map(|index| port_id(seq.identity(), PortKind::Input, index as u32))
				.expect("the sequence has a stream input")
		};
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::ConnectionRequested {
				from: port_id(clip.identity(), PortKind::Output, 0),
				to: seq_input,
			},
		)
		.expect("the connection applies");

		// Disconnecting the synthesized wire is a no-op.
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DisconnectionRequested {
				edge: output_wire_id(clip.identity()),
			},
		)
		.expect("the structural wire is ignored");
		// An unknown edge id errors.
		assert!(apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DisconnectionRequested {
				edge: EdgeId(999_999),
			},
		)
		.is_err());
		// The real edge disconnects by its stable id.
		let edge_id = real_edge_id(clip.identity(), seq.identity(), "tex_in");
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DisconnectionRequested { edge: edge_id },
		)
		.expect("the real edge disconnects");

		// A move of an unplaced node: the fallback grid base plus delta.
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::NodeMoveRequested {
				nodes: vec![NodeId(clip.identity())],
				delta: point(px(10.0), px(20.0)),
			},
		)
		.expect("the first move applies");
		let moved = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip)
				.expect("the clip")
				.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == seq)
				.map(|(_, pos, _)| *pos)
				.expect("the position was persisted")
		};
		assert_eq!(moved, (570.0, 60.0), "fallback column 2, row 0, plus delta");

		// A move list with an unknown node applies the earlier entries and
		// still reports the error; the next move starts from the persisted
		// position (not the origin).
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::NodeMoveRequested {
				nodes: vec![NodeId(clip.identity()), NodeId(123_456)],
				delta: point(px(5.0), px(5.0)),
			},
		)
		.expect_err("the unknown node rejects the whole move");
		let moved = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip)
				.expect("the clip")
				.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == seq)
				.map(|(_, pos, _)| *pos)
				.expect("the position")
		};
		assert_eq!(moved, (575.0, 65.0), "the valid entry still moved");
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::NodeMoveRequested {
				nodes: vec![NodeId(clip.identity())],
				delta: point(px(5.0), px(5.0)),
			},
		)
		.expect("the persisted position is the next base");
		let moved = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip)
				.expect("the clip")
				.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == seq)
				.map(|(_, pos, _)| *pos)
				.expect("the position")
		};
		assert_eq!(moved, (580.0, 70.0));

		// The preview event is a no-op.
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::NodeMovePreview {
				nodes: vec![NodeId(clip.identity())],
				delta: point(px(1.0), px(1.0)),
			},
		)
		.expect("the preview is inert");

		// Delete: the sequence is protected, a real node is removed and an
		// unknown node errors.
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DeleteRequested {
				nodes: vec![NodeId(seq.identity())],
				edges: Vec::new(),
			},
		)
		.expect("the sequence cannot be deleted");
		{
			let g = graphops::lock(&project);
			assert!(g.graph.is_valid(seq), "the sequence survives");
		}
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DeleteRequested {
				nodes: vec![NodeId(effect.identity())],
				edges: Vec::new(),
			},
		)
		.expect("the effect is removed");
		{
			let g = graphops::lock(&project);
			assert!(!g.graph.is_valid(effect), "the effect is gone");
		}
		assert!(apply_edit(
			&project,
			seq,
			&NodeGraphEvent::DeleteRequested {
				nodes: vec![NodeId(123_456)],
				edges: Vec::new(),
			},
		)
		.is_err());

		oak_engine_rust_sdk::node::undo_clear();
	}

	/// Regression: in the per-clip view the first move of an unplaced node
	/// must start from the base the VIEW displays (the chain-local fallback
	/// grid), not the whole-sequence grid — the second clip's card sits at
	/// row 0 of its own chain but row 1 of the full grid, and the wrong
	/// grid teleported the card down on the first drag release.
	#[test]
	fn move_in_clip_view_uses_the_chain_local_fallback() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, _clip_a, _effect, _footage) = project_with_chained_clip();
		// A second, bare clip node: the full view's fallback grid now has
		// two clip-role rows (A at 0, B at 1), B's own chain view has one.
		let clip_b = {
			let mut g = graphops::lock(&project);
			let (core, behavior) = oak_node::block::clip_create();
			g.graph.add_node(core, behavior)
		};

		apply_edit_in_view(
			&project,
			seq,
			Some(clip_b.identity()),
			&NodeGraphEvent::NodeMoveRequested {
				nodes: vec![NodeId(clip_b.identity())],
				delta: point(px(10.0), px(20.0)),
			},
		)
		.expect("the move applies");
		let moved = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip_b)
				.expect("clip B")
				.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == seq)
				.map(|(_, pos, _)| *pos)
				.expect("the position was persisted")
		};
		// Column 2 (clip role), row 0 — the chain-local base, plus delta.
		// The full-grid base would have been row 1 → y = 240.
		assert_eq!(moved, (570.0, 60.0), "chain-local base, not full-grid row 1");

		// Sanity: the old whole-sequence path would have teleported the
		// card to row 1 (y = 240) — prove the grids actually differ.
		let clip_c = {
			let mut g = graphops::lock(&project);
			let (core, behavior) = oak_node::block::clip_create();
			g.graph.add_node(core, behavior)
		};
		apply_edit(
			&project,
			seq,
			&NodeGraphEvent::NodeMoveRequested {
				nodes: vec![NodeId(clip_c.identity())],
				delta: point(px(10.0), px(20.0)),
			},
		)
		.expect("the move applies");
		let moved_c = {
			let g = graphops::lock(&project);
			g.graph
				.get(clip_c)
				.expect("clip C")
				.core
				.context_positions
				.iter()
				.find(|(c, _, _)| *c == seq)
				.map(|(_, pos, _)| *pos)
				.expect("the position was persisted")
		};
		assert_eq!(moved_c, (570.0, 240.0), "the full grid puts C at row 1");

		oak_engine_rust_sdk::node::undo_clear();
	}


	/// A project with a sequence and a clip whose chain runs footage → one
	/// effect → clip: `(project, seq, clip, effect, footage)`.
	fn project_with_chained_clip() -> (
		ProjectRef,
		DomainNodeId,
		DomainNodeId,
		DomainNodeId,
		DomainNodeId,
	) {
		let project = graphops::create_project();
		let seq = graphops::create_sequence(&project, "Chain");
		let clip = {
			let mut g = graphops::lock(&project);
			let (core, behavior) = oak_node::block::clip_create();
			g.graph.add_node(core, behavior)
		};
		// Insert the effect on the bare clip first (the chain is empty, so
		// the insert rewires nothing and connects the effect to the clip's
		// `tex_in`).
		let ty = effectchain::addable_effects()
			.into_iter()
			.next()
			.expect("the factory registers at least one video effect")
			.type_id;
		let effect = effectchain::insert(&project, clip, 0, &ty).expect("chain the effect");
		// Then feed the effect's media input from a footage node: the chain
		// becomes footage → effect → clip.
		let effect_input = graphops::lock(&project)
			.graph
			.get(effect)
			.map(|e| e.core.effect_input.clone())
			.unwrap_or_default();
		let footage = {
			let mut g = graphops::lock(&project);
			let (core, behavior) = oak_node::footage::FootageBehavior::create();
			g.graph.add_node(core, behavior)
		};
		{
			let mut g = graphops::lock(&project);
			g.graph
				.connect(footage, effect, &effect_input, -1)
				.expect("wire the footage into the effect");
		}
		(project, seq, clip, effect, footage)
	}

	/// The per-clip builder yields exactly the clip's context chain
	/// (footage → effect → clip): no sequence output card, no synthesized
	/// wires, and the chain's real edges connect footage → effect → clip.
	/// The full-sequence builder still shows the output card and wires.
	#[test]
	fn clip_context_build_is_the_clip_chain_only() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, clip, effect, footage) = project_with_chained_clip();

		let (nodes, edges) = build_graph_for_clip(&project, seq, clip.identity());
		let ids: Vec<u64> = nodes.iter().map(|n| n.id.0).collect();
		assert_eq!(ids.len(), 3, "clip + effect + footage (got {ids:?})");
		assert!(ids.contains(&clip.identity()), "the clip node is present");
		assert!(ids.contains(&effect.identity()), "the effect node is present");
		assert!(ids.contains(&footage.identity()), "the footage node is present");
		assert!(
			!ids.contains(&seq.identity()),
			"the sequence output card is not part of a clip's context"
		);

		// A per-clip view carries no synthesized clip→output wires.
		assert!(
			edges.iter().all(|e| !is_output_wire(e.id)),
			"the per-clip view has no synthesized output wires"
		);
		// The chain's real edges: footage feeds the effect, which feeds the
		// clip.
		assert!(
			edges.iter().any(|e| e.from_node == NodeId(footage.identity())
				&& e.to_node == NodeId(effect.identity())),
			"footage feeds the effect"
		);
		assert!(
			edges.iter().any(|e| e.from_node == NodeId(effect.identity())
				&& e.to_node == NodeId(clip.identity())),
			"the effect feeds the clip"
		);

		// The full-sequence view keeps the output card and the synthesized
		// clip→output wire.
		let (full, full_edges) = build_graph(&project, seq);
		assert!(
			full.iter().any(|n| n.id == NodeId(seq.identity())),
			"the full graph shows the sequence output card"
		);
		assert!(
			full_edges.iter().any(|e| is_output_wire(e.id)),
			"the full graph synthesizes the clip→output wire"
		);

		oak_engine_rust_sdk::node::undo_clear();
	}

	/// Every project graph carries the virtual endpoint pair: the full view
	/// shows both as marked cards (the output endpoint as a pure sink), and
	/// a clip's context chain shows neither.
	#[test]
	fn endpoints_build_as_marked_cards() {
		let _g = stack_lock();
		oak_engine_rust_sdk::node::undo_clear();
		let (project, seq, clip, _, _) = project_with_chained_clip();

		let (nodes, _edges) = build_graph(&project, seq);
		assert_eq!(
			nodes.iter().filter(|n| n.endpoint.is_some()).count(),
			2,
			"exactly one endpoint pair is displayed"
		);
		let input = nodes
			.iter()
			.find(|n| n.endpoint == Some(GraphEndpoint::Input))
			.expect("the input endpoint card");
		let output = nodes
			.iter()
			.find(|n| n.endpoint == Some(GraphEndpoint::Output))
			.expect("the output endpoint card");
		assert!(
			!input.outputs.is_empty(),
			"the input endpoint keeps its `tex_out`"
		);
		assert!(
			output.outputs.is_empty(),
			"the output endpoint is a sink and has no output port"
		);
		assert!(
			!input.title.is_empty() && !output.title.is_empty(),
			"endpoint cards carry their behavior name as the title"
		);

		// The per-clip chain view contains no endpoint cards.
		let (chain, _) = build_graph_for_clip(&project, seq, clip.identity());
		assert!(
			chain.iter().all(|n| n.endpoint.is_none()),
			"a clip's context chain contains no endpoints"
		);

		oak_engine_rust_sdk::node::undo_clear();
	}
}

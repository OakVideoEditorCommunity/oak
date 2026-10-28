//! Domain-side construction of detached sequence and clip-context graph snapshots.

use oak_node::graph::Graph;
use oak_node::id::NodeId;
use oak_node::node::ENABLED_INPUT;
use oak_node::nodes::graphendpoints::{GRAPH_INPUT_TYPE_ID, GRAPH_OUTPUT_TYPE_ID};
use oak_node::value::{NodeValue, ValueType};

use crate::editor_commands::ProjectRef;

// Behavior type IDs used to assign graph roles and to reproduce the node
// editor's media-chain filtering without depending on UI types.
const TYPE_SEQUENCE: &str = "org.olivevideoeditor.Olive.sequence";
const TYPE_CLIP: &str = "org.olivevideoeditor.Olive.clipblock";
const TYPE_FOOTAGE: &str = "org.olivevideoeditor.Olive.footage";
const TYPE_TRACK: &str = "org.olivevideoeditor.Olive.track";
const TYPE_TRACK_LIST: &str = "org.olivevideoeditor.Olive.tracklist";
const TYPE_GAP: &str = "org.olivevideoeditor.Olive.gapblock";
const TYPE_TRANSITION: &str = "org.olivevideoeditor.Olive.transitionblock";

/// Semantic role of a graph card.
///
/// This is deliberately a small SDK-facing classification rather than the full
/// behavior-type enum. Unknown effect and generator implementations use
/// `Effect`, while structural objects that need special UI treatment have
/// dedicated roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphRole {
	/// An ordinary effect, generator or other non-structural node.
	Effect,
	/// The sequence's output card. Full views synthesize clip links to it.
	Sequence,
	/// A timeline clip block carrying an effect chain.
	Clip,
	/// A media-producing footage node.
	Footage,
	/// The protected virtual `GraphInput` endpoint.
	InputEndpoint,
	/// The protected virtual `GraphOutput` endpoint and pure sink.
	OutputEndpoint,
}

impl GraphRole {
	/// Returns the stable integer stored in `OakGraphNode::role`.
	/// The SDK maps these values back without depending on Rust enum layout.
	pub(crate) fn code(self) -> i32 {
		match self {
			Self::Effect => 0,
			Self::Sequence => 1,
			Self::Clip => 2,
			Self::Footage => 3,
			Self::InputEndpoint => 4,
			Self::OutputEndpoint => 5,
		}
	}

	/// Decodes an ABI role value. Unknown values fall back to `Effect` so a
	/// newer role cannot make an older consumer treat the node as protected.
	pub(crate) fn from_code(code: i32) -> Self {
		match code {
			1 => Self::Sequence,
			2 => Self::Clip,
			3 => Self::Footage,
			4 => Self::InputEndpoint,
			5 => Self::OutputEndpoint,
			_ => Self::Effect,
		}
	}
}

/// One visible stream input. Parameter inputs are deliberately not graph ports.
#[derive(Clone, Debug)]
pub struct GraphPort {
	/// Stable domain input ID used by connect/disconnect commands.
	pub id: String,
	/// Human-readable label; falls back to `id` when no display name exists.
	pub label: String,
	/// Index in the node's complete declaration list. This can differ from the
	/// port's index in the filtered graph-port list.
	pub declaration_index: usize,
	/// Wire media code: 1 = texture, 2 = samples. Other input types are filtered
	/// out before this DTO is constructed.
	pub media_type: i32,
	/// Whether any source is connected to this input in the captured graph.
	pub connected: bool,
	/// Whether the declaration accepts graph connections at all.
	pub connectable: bool,
}

/// One detached graph card and its implicit output state.
#[derive(Clone, Debug)]
pub struct GraphNode {
	/// Stable generational node identity in the source project.
	pub id: NodeId,
	/// Behavior type ID, preserved separately from the coarse `role`.
	pub type_id: String,
	/// User label, or the behavior's display name when the label is empty.
	pub label: String,
	/// Coarse semantic role used for output and protection rules.
	pub role: GraphRole,
	/// True for the sequence and virtual endpoints, which must not be deleted
	/// or renamed as ordinary effect nodes.
	pub protected: bool,
	/// Value of the standard `enabled_in` input at capture time.
	pub enabled: bool,
	/// Nonzero position in this sequence's context-position map. `(0, 0)` is
	/// treated as absent so callers can apply their fallback layout.
	pub position: Option<(f64, f64)>,
	/// Visible Texture/Samples inputs in declaration order, with hidden and
	/// non-stream parameter inputs removed.
	pub inputs: Vec<GraphPort>,
	/// Whether the node exposes an implicit main output. Only `GraphOutput`
	/// lacks this port because it is a pure sink.
	pub has_output: bool,
	/// Whether at least one real outgoing connection exists at capture time.
	/// Synthetic clip-to-sequence edges do not affect this flag.
	pub output_connected: bool,
}

/// One real graph edge or a full-view synthetic clip-to-sequence wire.
#[derive(Clone, Debug)]
pub struct GraphEdge {
	/// Source node whose main output feeds the edge.
	pub from: NodeId,
	/// Destination node containing the input.
	pub to: NodeId,
	/// Destination input ID. Synthetic wires always use `tex_in`.
	pub input_id: String,
	/// Input element index; `-1` denotes a scalar input.
	pub element: i32,
	/// True only for display-only clip-to-sequence wires in the full view.
	/// Such an edge is not present in the project's graph or in clip views.
	pub synthetic: bool,
}

/// A consistent graph view captured at one undo revision.
#[derive(Clone, Debug)]
pub(crate) struct GraphSnapshot {
	/// Global undo index recorded for this immutable view.
	pub revision: i64,
	/// Cards included in this full or clip-context view.
	pub nodes: Vec<GraphNode>,
	/// Real edges internal to the view plus any full-view synthetic wires.
	pub edges: Vec<GraphEdge>,
}

/// Maps a behavior type ID to its coarse graph role.
fn role(type_id: &str) -> GraphRole {
	match type_id {
		TYPE_SEQUENCE => GraphRole::Sequence,
		TYPE_CLIP => GraphRole::Clip,
		TYPE_FOOTAGE => GraphRole::Footage,
		GRAPH_INPUT_TYPE_ID => GraphRole::InputEndpoint,
		GRAPH_OUTPUT_TYPE_ID => GraphRole::OutputEndpoint,
		_ => GraphRole::Effect,
	}
}

/// Excludes timeline plumbing that has no useful media-chain card.
/// Tracks, track lists, gaps and transitions remain in the project but are not
/// exposed in the node-editor snapshot.
fn displayed(type_id: &str) -> bool {
	!matches!(type_id, TYPE_TRACK | TYPE_TRACK_LIST | TYPE_GAP | TYPE_TRANSITION)
}

/// Reads a node's behavior type ID, returning an empty string for a stale ID.
fn type_id(g: &Graph, id: NodeId) -> String {
	g.get(id)
		.map(|entry| entry.behavior.type_id().to_string())
		.unwrap_or_default()
}

/// Collects the clip and its complete upstream effect/media chain.
/// The sequence and virtual endpoints are intentionally outside this view.
fn clip_context_nodes(g: &Graph, clip: NodeId) -> Vec<NodeId> {
	let mut nodes = vec![clip];
	nodes.extend(crate::effect::chain(g, clip));
	nodes
}

/// Collects all displayable project nodes and appends the sequence exactly once.
/// Identity sorting keeps snapshots deterministic across graph traversal order.
fn full_graph_nodes(g: &Graph, sequence: NodeId) -> Vec<NodeId> {
	let mut nodes: Vec<NodeId> = g
		.node_ids()
		.into_iter()
		.filter(|&id| id != sequence && displayed(&type_id(g, id)))
		.collect();
	nodes.push(sequence);
	nodes.sort_by_key(|id| id.identity());
	nodes
}

/// Copies one graph node and its visible ports without retaining graph borrows.
///
/// The caller supplies IDs from a validated node list, so a missing entry here
/// would indicate a builder invariant violation rather than a user error.
fn node_snapshot(g: &Graph, sequence: NodeId, id: NodeId) -> GraphNode {
	let entry = g.get(id).expect("node IDs are validated by the builder");
	let type_id = entry.behavior.type_id().to_string();
	let role = role(&type_id);
	let label = if entry.core.label.is_empty() {
		entry.behavior.name().to_string()
	} else {
		entry.core.label.clone()
	};
	let inputs = entry
		.core
		.inputs
		.iter()
		.enumerate()
		.filter(|(_, input)| {
			matches!(input.value_type, ValueType::Texture | ValueType::Samples)
				&& input.flags & oak_node::input::flags::HIDDEN == 0
		})
		.map(|(index, input)| GraphPort {
			id: input.id.clone(),
			label: if input.display_name.is_empty() {
				input.id.clone()
			} else {
				input.display_name.clone()
			},
			declaration_index: index,
			media_type: if input.value_type == ValueType::Samples { 2 } else { 1 },
			connected: g.is_input_connected(id, &input.id, -1),
			connectable: input.is_connectable(),
		})
		.collect();
	let position = entry
		.core
		.context_positions
		.iter()
		.find(|(context, _, _)| *context == sequence)
		.map(|(_, position, _)| *position)
		.filter(|(x, y)| *x != 0.0 || *y != 0.0);
	GraphNode {
		id,
		type_id,
		label,
		role,
		protected: matches!(role, GraphRole::Sequence | GraphRole::InputEndpoint | GraphRole::OutputEndpoint),
		enabled: matches!(entry.core.standard_value(ENABLED_INPUT, -1), NodeValue::Boolean(true)),
		position,
		inputs,
		has_output: role != GraphRole::OutputEndpoint,
		output_connected: !g.output_connections(id).is_empty(),
	}
}

/// Builds a full sequence graph or a self-contained clip context graph.
/// Invalid sequence/clip IDs return `None` instead of a partial snapshot.
///
/// The full view includes the sequence and both virtual endpoints. Real edges
/// are retained only when both endpoints belong to the selected view; synthetic
/// clip-to-sequence wires are added only in the full view. All strings, ports,
/// positions and edge arrays are owned by the returned snapshot.
pub(crate) fn build(
	project: &ProjectRef,
	sequence: NodeId,
	clip: Option<NodeId>,
) -> Option<GraphSnapshot> {
	let revision = oak_undo::global::index().unwrap_or(0);
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	let g = &guard.graph;
	if !g.is_valid(sequence) || clip.is_some_and(|id| !g.is_valid(id)) {
		return None;
	}
	let ids = match clip {
		Some(clip) => clip_context_nodes(g, clip),
		None => full_graph_nodes(g, sequence),
	};
	let nodes: Vec<GraphNode> = ids
		.iter()
		.map(|&id| node_snapshot(g, sequence, id))
		.collect();
	let mut edges = Vec::new();
	for &from in &ids {
		for (to, input_id, element) in g.output_connections(from) {
			if ids.contains(&to) && !input_id.is_empty() {
				edges.push(GraphEdge {
					from,
					to,
					input_id,
					element,
					synthetic: false,
				});
			}
		}
		if clip.is_none() && type_id(g, from) == TYPE_CLIP {
			edges.push(GraphEdge {
				from,
				to: sequence,
				input_id: "tex_in".into(),
				element: -1,
				synthetic: true,
			});
		}
	}
	Some(GraphSnapshot {
		revision,
		nodes,
		edges,
	})
}

/// Validates a proposed stream connection without changing the graph.
/// Existing connections, self-links, stale IDs and non-connectable inputs fail.
/// Only Texture/Samples inputs are accepted, matching the visible port set.
/// `element` is checked for occupancy but otherwise left to the write command's
/// domain validation when a connection is actually executed.
pub(crate) fn can_connect(
	project: &ProjectRef,
	from: NodeId,
	to: NodeId,
	input_id: &str,
	element: i32,
) -> bool {
	if from == to || input_id.is_empty() {
		return false;
	}
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	let g = &guard.graph;
	if !g.is_valid(from) || !g.is_valid(to) || g.is_input_connected(to, input_id, element) {
		return false;
	}
	g.get(to)
		.and_then(|entry| entry.core.get_input(input_id))
		.is_some_and(|input| {
			input.is_connectable() && matches!(input.value_type, ValueType::Texture | ValueType::Samples)
		})
}

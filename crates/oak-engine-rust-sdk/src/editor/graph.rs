//! Owned node-graph snapshots and live connection validation.
//!
//! `EditorSession::graph_snapshot` captures a complete view in one engine call,
//! then copies the immutable native snapshot into safe Rust DTOs. Consumers
//! never receive live graph references or per-query partially updated state.

use std::ffi::c_void;

use crate::codec::string_out;
use crate::editor::EditorSession;
use crate::node::NodeId;

#[cfg(test)]
mod tests;

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_editor_graph_snapshot(session: *mut c_void, sequence: u64, clip: u64) -> *mut c_void;
	fn oak_editor_graph_release(graph: *const c_void);
	fn oak_editor_graph_revision(graph: *const c_void) -> i64;
	fn oak_editor_graph_node_count(graph: *const c_void) -> i32;
	fn oak_editor_graph_node_at(graph: *const c_void, index: i32, out: *mut GraphNodeRaw) -> bool;
	fn oak_editor_graph_node_label_at(graph: *const c_void, index: i32, buf: *mut u8, len: usize) -> i32;
	fn oak_editor_graph_node_type_at(graph: *const c_void, index: i32, buf: *mut u8, len: usize) -> i32;
	fn oak_editor_graph_port_at(
		graph: *const c_void,
		node: i32,
		port: i32,
		out: *mut GraphPortRaw,
	) -> bool;
	fn oak_editor_graph_port_id_at(
		graph: *const c_void,
		node: i32,
		port: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
	fn oak_editor_graph_port_label_at(
		graph: *const c_void,
		node: i32,
		port: i32,
		buf: *mut u8,
		len: usize,
	) -> i32;
	fn oak_editor_graph_edge_count(graph: *const c_void) -> i32;
	fn oak_editor_graph_edge_at(graph: *const c_void, index: i32, out: *mut GraphEdgeRaw) -> bool;
	fn oak_editor_graph_edge_input_at(graph: *const c_void, index: i32, buf: *mut u8, len: usize) -> i32;
	fn oak_editor_graph_can_connect(
		session: *mut c_void,
		from: u64,
		to: u64,
		input_id: *const u8,
		input_id_len: usize,
		element: i32,
	) -> bool;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct GraphNodeRaw {
	id: u64,
	role: i32,
	protected: bool,
	enabled: bool,
	has_position: bool,
	x: f64,
	y: f64,
	input_count: i32,
	has_output: bool,
	output_connected: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct GraphPortRaw {
	declaration_index: usize,
	media_type: i32,
	connected: bool,
	connectable: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct GraphEdgeRaw {
	from: u64,
	to: u64,
	element: i32,
	synthetic: bool,
}

/// Semantic node role in the graph view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphNodeRole {
	Effect,
	Sequence,
	Clip,
	Footage,
	InputEndpoint,
	OutputEndpoint,
}

impl GraphNodeRole {
	fn from_raw(raw: i32) -> Self {
		match raw {
			1 => Self::Sequence,
			2 => Self::Clip,
			3 => Self::Footage,
			4 => Self::InputEndpoint,
			5 => Self::OutputEndpoint,
			_ => Self::Effect,
		}
	}
}

/// Stream media accepted by a visible graph port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphMediaType {
	Texture,
	Samples,
}

/// One visible stream input or a node's implicit main output.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphPort {
	/// Empty only for an implicit main output; otherwise the declared input ID.
	pub id: String,
	pub label: String,
	pub declaration_index: usize,
	pub media_type: Option<GraphMediaType>,
	pub connected: bool,
	pub connectable: bool,
}

/// One detached graph node.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
	pub id: NodeId,
	pub type_id: String,
	pub label: String,
	pub role: GraphNodeRole,
	/// Sequence and virtual endpoint nodes must not be deleted or renamed.
	pub protected: bool,
	pub enabled: bool,
	/// Persisted nonzero position in the current sequence context, if present.
	pub position: Option<(f64, f64)>,
	pub inputs: Vec<GraphPort>,
	/// `None` only for the output endpoint, which is a pure sink.
	pub output: Option<GraphPort>,
}

/// One real or synthetic graph connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphEdge {
	pub from: NodeId,
	pub to: NodeId,
	pub input_id: String,
	pub element: i32,
	/// True for the full view's display-only clip-to-sequence output wire.
	pub synthetic: bool,
}

/// A consistent, fully owned graph view at `revision`.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphSnapshot {
	pub revision: i64,
	pub nodes: Vec<GraphNode>,
	pub edges: Vec<GraphEdge>,
}

struct SnapshotHandle(*mut c_void);

impl Drop for SnapshotHandle {
	fn drop(&mut self) {
		unsafe { oak_editor_graph_release(self.0) };
	}
}

impl GraphSnapshot {
	unsafe fn from_handle(handle: SnapshotHandle) -> Self {
		let revision = unsafe { oak_editor_graph_revision(handle.0) };
		let nodes = (0..unsafe { oak_editor_graph_node_count(handle.0) }.max(0))
			.map(|index| {
				let mut raw = GraphNodeRaw::default();
				if !unsafe { oak_editor_graph_node_at(handle.0, index, &mut raw) } {
					return None;
				}
				let inputs = (0..raw.input_count.max(0))
					.filter_map(|port_index| {
						let mut port = GraphPortRaw::default();
						if !unsafe { oak_editor_graph_port_at(handle.0, index, port_index, &mut port) } {
							return None;
						}
						Some(GraphPort {
							id: string_out(|buf, len| unsafe {
								oak_editor_graph_port_id_at(handle.0, index, port_index, buf, len)
							})
							.unwrap_or_default(),
							label: string_out(|buf, len| unsafe {
								oak_editor_graph_port_label_at(handle.0, index, port_index, buf, len)
							})
							.unwrap_or_default(),
							declaration_index: port.declaration_index,
							media_type: match port.media_type {
								1 => Some(GraphMediaType::Texture),
								2 => Some(GraphMediaType::Samples),
								_ => None,
							},
							connected: port.connected,
							connectable: port.connectable,
						})
					})
					.collect();
				let output = raw.has_output.then_some(GraphPort {
					id: String::new(),
					label: "out".into(),
					declaration_index: 0,
					media_type: None,
					connected: raw.output_connected,
					connectable: true,
				});
				Some(GraphNode {
					id: NodeId::from_raw(raw.id),
					type_id: string_out(|buf, len| unsafe {
						oak_editor_graph_node_type_at(handle.0, index, buf, len)
					})
					.unwrap_or_default(),
					label: string_out(|buf, len| unsafe {
						oak_editor_graph_node_label_at(handle.0, index, buf, len)
					})
					.unwrap_or_default(),
					role: GraphNodeRole::from_raw(raw.role),
					protected: raw.protected,
					enabled: raw.enabled,
					position: raw.has_position.then_some((raw.x, raw.y)),
					inputs,
					output,
				})
			})
			.collect::<Option<Vec<_>>>()
			.unwrap_or_default();
		let edges = (0..unsafe { oak_editor_graph_edge_count(handle.0) }.max(0))
			.filter_map(|index| {
				let mut raw = GraphEdgeRaw::default();
				if !unsafe { oak_editor_graph_edge_at(handle.0, index, &mut raw) } {
					return None;
				}
				Some(GraphEdge {
					from: NodeId::from_raw(raw.from),
					to: NodeId::from_raw(raw.to),
					input_id: string_out(|buf, len| unsafe {
						oak_editor_graph_edge_input_at(handle.0, index, buf, len)
					})
					.unwrap_or_default(),
					element: raw.element,
					synthetic: raw.synthetic,
				})
			})
			.collect();
		Self {
			revision,
			nodes,
			edges,
		}
	}
}

impl EditorSession {
	/// Captures a full sequence graph, or only a clip's `footage → effects → clip`
	/// context when `clip` is `Some`. Invalid IDs return `None`.
	pub fn graph_snapshot(&self, sequence: NodeId, clip: Option<NodeId>) -> Option<GraphSnapshot> {
		if !sequence.is_valid() || clip.is_some_and(|id| !id.is_valid()) {
			return None;
		}
		let ptr = unsafe {
			oak_editor_graph_snapshot(
				self.ptr,
				sequence.raw(),
				clip.map(|id| id.raw()).unwrap_or(NodeId::INVALID.raw()),
			)
		};
		if ptr.is_null() {
			return None;
		}
		Some(unsafe { GraphSnapshot::from_handle(SnapshotHandle(ptr)) })
	}

	/// Validates a proposed stream connection against the current live graph.
	/// This never mutates the project or creates an undo entry.
	pub fn can_connect(&self, from: NodeId, to: NodeId, input_id: &str, element: i32) -> bool {
		unsafe {
			oak_editor_graph_can_connect(
				self.ptr,
				from.raw(),
				to.raw(),
				input_id.as_ptr(),
				input_id.len(),
				element,
			)
		}
	}
}

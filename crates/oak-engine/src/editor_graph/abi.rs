//! C ABI for reference-counted graph snapshots and live connection checks.

use oak_node::id::NodeId;

use crate::codec::{copy_str_out, str_arg};
use crate::editor::EditorSessionHandle;
use crate::editor_graph::snapshot::{GraphNode, GraphPort, GraphSnapshot, build, can_connect};
use crate::handle::NativeMirror;
use crate::node::OAK_NODE_ID_INVALID;

/// Opaque, immutable graph snapshot. Queries never re-read the live project.
pub struct EditorGraphHandle {
	snapshot: GraphSnapshot,
}

crate::export_handle!(EditorGraphHandle, oak_editor_graph_add_ref, oak_editor_graph_release);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// POD node fields; labels, types and ports are queried separately.
pub struct OakGraphNode {
	pub id: u64,
	pub role: i32,
	pub protected: bool,
	pub enabled: bool,
	pub has_position: bool,
	pub x: f64,
	pub y: f64,
	pub input_count: i32,
	pub has_output: bool,
	pub output_connected: bool,
}

impl Default for OakGraphNode {
	fn default() -> Self {
		Self {
			id: OAK_NODE_ID_INVALID,
			role: 0,
			protected: false,
			enabled: false,
			has_position: false,
			x: 0.0,
			y: 0.0,
			input_count: 0,
			has_output: false,
			output_connected: false,
		}
	}
}

/// Scalar POD mirror. `to_native` reconstructs those scalar fields only;
/// labels, type IDs and nested input ports travel through separate ABI calls.
impl NativeMirror for OakGraphNode {
	type Native = GraphNode;

	fn from_native(node: &GraphNode) -> Self {
		Self {
			id: node.id.identity(),
			role: node.role.code(),
			protected: node.protected,
			enabled: node.enabled,
			has_position: node.position.is_some(),
			x: node.position.map(|p| p.0).unwrap_or(0.0),
			y: node.position.map(|p| p.1).unwrap_or(0.0),
			input_count: node.inputs.len() as i32,
			has_output: node.has_output,
			output_connected: node.output_connected,
		}
	}

	fn to_native(&self) -> GraphNode {
		GraphNode {
			id: NodeId::from_identity(self.id).unwrap_or(NodeId::INVALID),
			type_id: String::new(),
			label: String::new(),
			role: crate::editor_graph::snapshot::GraphRole::from_code(self.role),
			protected: self.protected,
			enabled: self.enabled,
			position: self.has_position.then_some((self.x, self.y)),
			inputs: Vec::new(),
			has_output: self.has_output,
			output_connected: self.output_connected,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
/// POD stream-port fields; IDs and display labels use bounded strings.
pub struct OakGraphPort {
	pub declaration_index: usize,
	pub media_type: i32,
	pub connected: bool,
	pub connectable: bool,
}

/// Scalar POD mirror. Input IDs and display labels remain out-of-band strings.
impl NativeMirror for OakGraphPort {
	type Native = GraphPort;

	fn from_native(port: &GraphPort) -> Self {
		Self {
			declaration_index: port.declaration_index,
			media_type: port.media_type,
			connected: port.connected,
			connectable: port.connectable,
		}
	}

	fn to_native(&self) -> GraphPort {
		GraphPort {
			id: String::new(),
			label: String::new(),
			declaration_index: self.declaration_index,
			media_type: self.media_type,
			connected: self.connected,
			connectable: self.connectable,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// POD edge fields. `synthetic` distinguishes full-view clip/output wires.
pub struct OakGraphEdge {
	pub from: u64,
	pub to: u64,
	pub element: i32,
	pub synthetic: bool,
}

impl Default for OakGraphEdge {
	fn default() -> Self {
		Self {
			from: OAK_NODE_ID_INVALID,
			to: OAK_NODE_ID_INVALID,
			element: -1,
			synthetic: false,
		}
	}
}

/// Scalar POD mirror. The target input ID is queried separately as UTF-8.
impl NativeMirror for OakGraphEdge {
	type Native = crate::editor_graph::snapshot::GraphEdge;

	fn from_native(edge: &Self::Native) -> Self {
		Self {
			from: edge.from.identity(),
			to: edge.to.identity(),
			element: edge.element,
			synthetic: edge.synthetic,
		}
	}

	fn to_native(&self) -> Self::Native {
		Self::Native {
			from: NodeId::from_identity(self.from).unwrap_or(NodeId::INVALID),
			to: NodeId::from_identity(self.to).unwrap_or(NodeId::INVALID),
			input_id: String::new(),
			element: self.element,
			synthetic: self.synthetic,
		}
	}
}

/// Captures one consistent graph view. `clip = OAK_NODE_ID_INVALID` selects the
/// full sequence view; a valid clip selects its self-contained context chain.
/// Returns null for a null session or invalid sequence/clip ID.
///
/// # Safety
/// A non-null `session` must point to a live editor session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_snapshot(
	session: *const EditorSessionHandle,
	sequence: u64,
	clip: u64,
) -> *mut EditorGraphHandle {
	if session.is_null() {
		return std::ptr::null_mut();
	}
	let Some(sequence) = NodeId::from_identity(sequence) else {
		return std::ptr::null_mut();
	};
	let clip = if clip == OAK_NODE_ID_INVALID {
		None
	} else {
		NodeId::from_identity(clip)
	};
	let project = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.project
		.clone();
	build(&project, sequence, clip)
		.map(|snapshot| crate::handle::into_ffi(EditorGraphHandle { snapshot }) as *mut EditorGraphHandle)
		.unwrap_or(std::ptr::null_mut())
}

/// Returns the undo revision captured by the snapshot, or zero for a null graph.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_graph_revision(graph: *const EditorGraphHandle) -> i64 {
	unsafe { graph.as_ref() }.map(|g| g.snapshot.revision).unwrap_or(0)
}

/// Returns the number of nodes in the immutable snapshot, or zero for null.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_graph_node_count(graph: *const EditorGraphHandle) -> i32 {
	unsafe { graph.as_ref() }.map(|g| g.snapshot.nodes.len() as i32).unwrap_or(0)
}

/// Copies scalar node data at `index` into `out`.
///
/// Returns false for null arguments or an out-of-range index. No output is
/// written on failure. The node remains owned by the immutable snapshot.
///
/// # Safety
/// `graph` must be a live graph handle and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_node_at(
	graph: *const EditorGraphHandle,
	index: i32,
	out: *mut OakGraphNode,
) -> bool {
	if graph.is_null() || out.is_null() || index < 0 {
		return false;
	}
	let graph = unsafe { &*graph };
	let Some(node) = graph.snapshot.nodes.get(index as usize) else {
		return false;
	};
	unsafe {
		*out = OakGraphNode::from_native(node);
	};
	true
}

/// Copies a node's display label using the bounded-string convention.
/// Returns the required byte count, or `-1` for null/invalid indices.
///
/// # Safety
/// `graph` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_node_label_at(
	graph: *const EditorGraphHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if graph.is_null() || index < 0 {
		return -1;
	}
	unsafe { &*graph }
		.snapshot
		.nodes
		.get(index as usize)
		.map(|node| copy_str_out(&node.label, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies a node's behavior type ID using the bounded-string convention.
/// Returns the required byte count, or `-1` for null/invalid indices.
///
/// # Safety
/// `graph` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_node_type_at(
	graph: *const EditorGraphHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if graph.is_null() || index < 0 {
		return -1;
	}
	unsafe { &*graph }
		.snapshot
		.nodes
		.get(index as usize)
		.map(|node| copy_str_out(&node.type_id, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies scalar data for one visible stream input.
///
/// `node_index` addresses the snapshot node and `port_index` its filtered input
/// list, not the original declaration index. The original declaration index is
/// returned inside `OakGraphPort`. Returns false without writing on invalid input.
///
/// # Safety
/// `graph` must be a live graph handle and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_port_at(
	graph: *const EditorGraphHandle,
	node_index: i32,
	port_index: i32,
	out: *mut OakGraphPort,
) -> bool {
	if graph.is_null() || out.is_null() || node_index < 0 || port_index < 0 {
		return false;
	}
	let Some(port) = unsafe { &*graph }
		.snapshot
		.nodes
		.get(node_index as usize)
		.and_then(|node| node.inputs.get(port_index as usize))
	else {
		return false;
	};
	unsafe {
		*out = OakGraphPort::from_native(port);
	};
	true
}

/// Copies a stream input's stable domain ID using the bounded-string convention.
/// Returns the required byte count, or `-1` for null/invalid indices.
///
/// # Safety
/// `graph` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_port_id_at(
	graph: *const EditorGraphHandle,
	node_index: i32,
	port_index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if graph.is_null() || node_index < 0 || port_index < 0 {
		return -1;
	}
	unsafe { &*graph }
		.snapshot
		.nodes
		.get(node_index as usize)
		.and_then(|node| node.inputs.get(port_index as usize))
		.map(|port| copy_str_out(&port.id, buf, buf_len))
		.unwrap_or(-1)
}

/// Copies a stream input's display label using the bounded-string convention.
/// Returns the required byte count, or `-1` for null/invalid indices.
///
/// # Safety
/// `graph` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_port_label_at(
	graph: *const EditorGraphHandle,
	node_index: i32,
	port_index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if graph.is_null() || node_index < 0 || port_index < 0 {
		return -1;
	}
	unsafe { &*graph }
		.snapshot
		.nodes
		.get(node_index as usize)
		.and_then(|node| node.inputs.get(port_index as usize))
		.map(|port| copy_str_out(&port.label, buf, buf_len))
		.unwrap_or(-1)
}

/// Returns the number of real and synthetic edges, or zero for a null graph.
#[unsafe(no_mangle)]
pub extern "C" fn oak_editor_graph_edge_count(graph: *const EditorGraphHandle) -> i32 {
	unsafe { graph.as_ref() }.map(|g| g.snapshot.edges.len() as i32).unwrap_or(0)
}

/// Copies scalar edge data at `index` into `out`.
///
/// Returns false for null arguments or an out-of-range index, and writes nothing
/// on failure. Use `oak_editor_graph_edge_input_at` for the target input ID.
///
/// # Safety
/// `graph` must be a live graph handle and `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_edge_at(
	graph: *const EditorGraphHandle,
	index: i32,
	out: *mut OakGraphEdge,
) -> bool {
	if graph.is_null() || out.is_null() || index < 0 {
		return false;
	}
	let Some(edge) = unsafe { &*graph }.snapshot.edges.get(index as usize) else {
		return false;
	};
	unsafe {
		*out = OakGraphEdge::from_native(edge);
	};
	true
}

/// Copies an edge's target input ID using the bounded-string convention.
/// Returns the required byte count, or `-1` for null/invalid indices.
///
/// # Safety
/// `graph` must be live; a non-null `buf` must be writable for `buf_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_edge_input_at(
	graph: *const EditorGraphHandle,
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if graph.is_null() || index < 0 {
		return -1;
	}
	unsafe { &*graph }
		.snapshot
		.edges
		.get(index as usize)
		.map(|edge| copy_str_out(&edge.input_id, buf, buf_len))
		.unwrap_or(-1)
}

/// Validates a live connection without mutating the project or undo history.
/// Returns false for null/invalid arguments, occupied inputs, self-connections,
/// non-connectable inputs and non-stream input types.
///
/// # Safety
/// A non-null `session` must point to a live editor session. `input_id` must
/// reference `input_id_len` readable bytes when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_editor_graph_can_connect(
	session: *const EditorSessionHandle,
	from: u64,
	to: u64,
	input_id: *const u8,
	input_id_len: usize,
	element: i32,
) -> bool {
	if session.is_null() {
		return false;
	}
	let (Some(from), Some(to), Some(input_id)) = (
		NodeId::from_identity(from),
		NodeId::from_identity(to),
		unsafe { str_arg(input_id, input_id_len) },
	) else {
		return false;
	};
	let project = unsafe { &*session }
		.inner
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.project
		.clone();
	can_connect(&project, from, to, input_id, element)
}

//! The interactive node-graph view and its event type.
//!
//! [`NodeGraphView`] is the top-level widget: a focusable canvas that renders
//! nodes, wires and interaction overlays, and reports every user edit
//! intention as a [`NodeGraphEvent`]. See the
//! [module-level docs](crate::widgets::node_graph) for the overall architecture.

use std::collections::{BTreeSet, HashMap};

use gpui_kit::{
	App, BorderStyle, Bounds, Context, Corners, Edges, Entity, EventEmitter, FocusHandle,
	Focusable, Hsla, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent,
	MouseMoveEvent, PaintQuad, PinchEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
	Size, Window, canvas, colors::DefaultColors, div, fill, hsla, point, prelude::*, px, size,
};

use crate::widgets::node_graph::{
	DEFAULT_NODE_WIDTH, EdgeData, EdgeId, GhostWire, GraphViewState, NodeData, NodeElement,
	NodeGraphDataSource, NodeId, NodeVisualState, PortData, PortDataType, PortId, PortKind,
	SelectionRect, Wire, WireVisualState, paint_ghost,
};

/// Spacing between grid lines, in graph-space pixels.
const GRID_SIZE: f32 = 20.0;

/// How close (in screen pixels) the cursor must be to a port dot for a wire
/// drag to snap to it.
const PORT_GRAB_RADIUS: Pixels = px(12.0);

/// Minimum marquee drag distance (in screen pixels) before a background press
/// is treated as a marquee drag rather than a plain click.
const MARQUEE_DRAG_THRESHOLD: f32 = 3.0;

/// What a mouse press on the canvas hit.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HitTarget {
	/// Empty background.
	Background,
	/// A port dot: start a wire drag.
	Port(PortId),
	/// A collapse or enable toggle in a node header: toggle selection only.
	Toggle(NodeId),
	/// A node body: select (and possibly drag) the node.
	Node(NodeId),
}

/// Transient state of a node move drag.
struct NodeDragState {
	/// The nodes being moved (the full selection at drag start).
	nodes: Vec<NodeId>,
	/// Element-local cursor position where the drag started.
	anchor: Point<Pixels>,
	/// Accumulated graph-space displacement since drag start.
	delta: Point<Pixels>,
}

/// Transient state of a wire drag (a "ghost" connection in progress).
struct WireDragState {
	/// The ghost wire, anchored at the drag source port.
	ghost: GhostWire,
	/// The port the drag started from.
	source_port: PortId,
	/// When the drag picked up an existing edge (from a connected input), the
	/// edge id; dropping in empty space disconnects it.
	picked_edge: Option<EdgeId>,
	/// Ports currently approved as drop targets by
	/// [`NodeGraphDataSource::can_connect`].
	valid_ports: BTreeSet<PortId>,
}

/// Transient state of a pan drag (space-drag or middle-mouse drag).
struct PanDragState {
	/// Window-space cursor position where the pan started.
	start_mouse: Point<Pixels>,
	/// The viewport offset when the pan started.
	start_offset: Point<Pixels>,
}

/// A snapshot of everything the view paints in one frame, computed in the
/// canvas prepaint and consumed by the paint closure.
struct GraphDraw {
	/// Node elements in paint order (bottom-most first), positioned in window
	/// space. Dragged nodes are painted last (on top).
	nodes: Vec<(Point<Pixels>, NodeElement)>,
	/// Edge wires, in graph order, positioned in window space.
	wires: Vec<Wire>,
	/// The in-progress ghost wire, if any, in window space.
	ghost: Option<GhostSnapshot>,
	/// The in-progress marquee rectangle, in element-local space.
	marquee: Option<SelectionRect>,
	/// The pan offset used to compute this frame.
	offset: Point<Pixels>,
	/// The zoom factor used to compute this frame.
	zoom: f32,
}

/// A copy of a [`GhostWire`]'s geometry, stored in the frame snapshot so the
/// paint closure does not need to borrow the view.
struct GhostSnapshot {
	/// Window-space anchor of the fixed end.
	from: Point<Pixels>,
	/// Window-space position of the free (cursor) end.
	to: Point<Pixels>,
	/// Data-type tint of the source port.
	color: Hsla,
	/// Whether the current drop target is valid.
	target_valid: bool,
}

/// Returns whether the two axis-aligned rectangles overlap (touching counts).
///
/// Both rectangles are given by their min/max corners in the same coordinate
/// space (the marquee path passes graph-space corners), so dragged corners must
/// be normalized first, as `SelectionRect::normalized` does.
///
/// # Arguments
///
/// * `min1` / `max1` - first rectangle's top-left and bottom-right corners.
/// * `min2` / `max2` - second rectangle's corners, in the same space.
///
/// # Returns
///
/// `true` when the projections overlap on both axes, exact edge contact
/// included.
fn rects_intersect(
	min1: Point<Pixels>,
	max1: Point<Pixels>,
	min2: Point<Pixels>,
	max2: Point<Pixels>,
) -> bool {
	min1.x <= max2.x && min2.x <= max1.x && min1.y <= max2.y && min2.y <= max1.y
}

/// Events emitted by [`NodeGraphView`].
///
/// **Every variant is a request, not a fact.** The widget never mutates the
/// graph itself; the app receives these events, validates them against its
/// engine and undo stack, applies them (or not), and calls `cx.notify()` on
/// the data-source entity. Variants named `*Requested` correspond to undoable
/// engine operations; the others are view-state notifications the app may
/// ignore.
#[derive(Clone, Debug)]
pub enum NodeGraphEvent {
	/// Continuous preview emitted while the user drags one or more nodes:
	/// reports the *accumulated* graph-space delta since the drag started.
	///
	/// Emitted on every pointer move during a node drag, before the final
	/// [`NodeMoveRequested`](Self::NodeMoveRequested). Apps may use it for
	/// live feedback (e.g. snapping guides) but must not push undo states for
	/// it. The widget draws dragged nodes at their model position plus this
	/// delta, so the app does not need to apply it for the drag to look
	/// right.
	NodeMovePreview {
		/// The nodes being dragged (the full selection at drag start).
		nodes: Vec<NodeId>,
		/// Accumulated graph-space displacement since drag start.
		delta: Point<Pixels>,
	},

	/// Emitted exactly once when a node drag ends (pointer release).
	///
	/// This is the undoable operation: the app should move all listed nodes
	/// by `delta` in graph space as a single undo step. `delta` is the same
	/// accumulated displacement reported by the last
	/// [`NodeMovePreview`](Self::NodeMovePreview) of this drag.
	NodeMoveRequested {
		/// The nodes to move (the full selection at drag start).
		nodes: Vec<NodeId>,
		/// Total graph-space displacement to apply.
		delta: Point<Pixels>,
	},

	/// The user dropped a wire drag on a port and
	/// [`NodeGraphDataSource::can_connect`] approved the pair.
	///
	/// `from` is always the output port, `to` the input port, regardless of
	/// which end the drag started from. The app should still re-validate
	/// before applying — the model may have changed since the drag started.
	ConnectionRequested {
		/// The output port the connection starts at.
		from: PortId,
		/// The input port the connection ends at.
		to: PortId,
	},

	/// The user asked to remove an existing edge (e.g. by clicking a wire
	/// with the disconnect modifier, or dragging a connected input's wire
	/// off into empty space).
	DisconnectionRequested {
		/// The edge to remove.
		edge: EdgeId,
	},

	/// The user pressed the delete/backspace key with a non-empty selection.
	///
	/// Nodes and edges are delivered together so the app can remove them as
	/// one undo step. `edges` contains both explicitly selected edges and
	/// every edge incident to a deleted node (computed by the widget, since
	/// those edges cannot outlive their endpoints).
	DeleteRequested {
		/// The nodes to delete.
		nodes: Vec<NodeId>,
		/// The edges to delete, including edges incident to `nodes`.
		edges: Vec<EdgeId>,
	},

	/// The selection changed. The full new selection is included so listeners
	/// do not need to track deltas. Oak uses this to keep the node graph and
	/// the [`crate::widgets::effect_stack`] selections in sync.
	SelectionChanged {
		/// The complete new selection.
		nodes: BTreeSet<NodeId>,
	},

	/// The viewport (pan offset and/or zoom) changed. Emitted after the
	/// gesture that caused it completes — for a zoom-to-cursor scroll this is
	/// per scroll tick; apps that persist the viewport should debounce.
	ViewChanged {
		/// The new pan offset (screen-space position of the graph origin).
		offset: Point<Pixels>,
		/// The new zoom factor.
		zoom: f32,
	},

	/// The user clicked (or released a cancelled wire drag on) empty
	/// background. `position` is the click position in *graph space*, ready
	/// to be used as the position of a newly created node. Oak opens its
	/// "add node" menu from this event.
	BackgroundClicked {
		/// Click position in graph space.
		position: Point<Pixels>,
	},

	/// The user right-clicked a node. If the node was not part of the
	/// selection, the widget made it the sole selection first (so the
	/// host's menu actions operate on it — the C++ `NodeView` behavior),
	/// emitting [`SelectionChanged`](Self::SelectionChanged) ahead of this
	/// event. `position` is in window coordinates, suitable for placing a
	/// context menu.
	NodeContextMenuRequested {
		/// The right-clicked node.
		node: NodeId,
		/// Mouse position in window coordinates.
		position: Point<Pixels>,
	},
}

use NodeGraphEvent::*;

/// The interactive node-graph editor view.
///
/// Generic over the app's data source `D`. Construct with
/// [`NodeGraphView::new`], place the returned `Entity<NodeGraphView<D>>` in
/// your layout, and [`cx.subscribe`](Context::subscribe) to
/// [`NodeGraphEvent`] to receive edit requests.
///
/// # Interaction summary
///
/// | Gesture | Effect |
/// |---|---|
/// | Space-drag or middle-mouse drag on background | pan ([`NodeGraphEvent::ViewChanged`]) |
/// | Scroll wheel / trackpad pinch | zoom at cursor ([`NodeGraphEvent::ViewChanged`]) |
/// | Left-drag on a node | move the node — and the whole selection if the node was selected ([`NodeGraphEvent::NodeMovePreview`] × N, then [`NodeGraphEvent::NodeMoveRequested`]) |
/// | Left-drag from a port dot | wire drag: compatible target ports highlight live via [`NodeGraphDataSource::can_connect`]; drop on a port emits [`NodeGraphEvent::ConnectionRequested`], drop on empty space cancels and emits [`NodeGraphEvent::BackgroundClicked`] so the app can offer an "add node" menu |
/// | Left-drag on background | marquee selection ([`NodeGraphEvent::SelectionChanged`]) |
/// | Click node | select it; Shift-click toggles it in the selection |
/// | Right-click node | select it (unless already selected) and request its context menu ([`NodeGraphEvent::NodeContextMenuRequested`]) |
/// | Right-click background | request the background context menu ([`NodeGraphEvent::BackgroundClicked`]) |
/// | Delete / Backspace | [`NodeGraphEvent::DeleteRequested`] for the selection |
///
/// All mouse positions in events are in window space; hit testing and painting
/// convert to element-local space by subtracting the viewport origin, which is
/// captured each frame by the canvas prepaint.
pub struct NodeGraphView<D: NodeGraphDataSource> {
	/// The app-supplied graph model. Read every frame; never mutated.
	data: Entity<D>,
	/// Viewport and selection state.
	state: GraphViewState,
	/// Focus handle for keyboard interactions (delete, future shortcuts).
	focus_handle: FocusHandle,
	/// The view's bounds within the window, set every frame by the canvas
	/// prepaint. Its origin converts between window-space and element-local
	/// coordinates.
	viewport: Bounds<Pixels>,
	/// In-progress node move drag, if any.
	node_drag: Option<NodeDragState>,
	/// In-progress wire drag, if any.
	wire_drag: Option<WireDragState>,
	/// In-progress pan drag, if any.
	pan_drag: Option<PanDragState>,
	/// Whether the space key is currently held down (space-drag pans).
	space_down: bool,
}

impl<D: NodeGraphDataSource + 'static> NodeGraphView<D> {
	/// Creates a new node-graph view over the given data-source entity.
	///
	/// The view subscribes to the entity and re-renders whenever the app
	/// calls `cx.notify()` on it after applying (or rejecting) edit requests.
	pub fn new(data: Entity<D>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
		let focus_handle = cx.focus_handle();
		cx.observe(&data, |_, _, cx| cx.notify()).detach();
		Self {
			data,
			state: GraphViewState::new(),
			focus_handle,
			viewport: Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))),
			node_drag: None,
			wire_drag: None,
			pan_drag: None,
			space_down: false,
		}
	}

	/// Returns the current viewport/selection state.
	pub fn state(&self) -> &GraphViewState {
		&self.state
	}

	/// The graph-space position of a window-space point (e.g. the pointer
	/// position of a drop landing on the canvas). Uses the viewport origin
	/// captured at the last prepaint.
	pub fn graph_position_at(&self, window_position: Point<Pixels>) -> Point<Pixels> {
		self.state
			.screen_to_graph(window_position - self.viewport.origin)
	}

	/// Returns the size of the canvas the graph was last painted into, or a
	/// zero size before the first frame. Hosts use this to fit the viewport
	/// to the graph (see [`GraphViewState::fit_to_rect`]).
	pub fn viewport_size(&self) -> Size<Pixels> {
		self.viewport.size
	}

	/// Returns a mutable reference to the viewport/selection state, e.g. to
	/// restore a persisted viewport or to sync selection with
	/// [`crate::widgets::effect_stack`]. Does not emit events; call `cx.notify()` on
	/// the view entity afterwards if you changed anything.
	pub fn state_mut(&mut self) -> &mut GraphViewState {
		&mut self.state
	}

	/// Programmatically replaces the selection (e.g. to sync the graph with
	/// a timeline selection or an inspector card click). Emits
	/// [`NodeGraphEvent::SelectionChanged`] when the set changed, so hosts
	/// keep their engine-side selection mirror consistent. Callers should
	/// not call `cx.notify()` on the view themselves — this method does.
	pub fn set_selection(&mut self, nodes: BTreeSet<NodeId>, cx: &mut Context<Self>) {
		self.set_selection_and_emit(nodes, cx);
		cx.notify();
	}

	/// Returns the data-source entity this view renders.
	pub fn data(&self) -> &Entity<D> {
		&self.data
	}

	/// Returns what is under `position` (in window space), or
	/// [`HitTarget::Background`]. Nodes are tested in reverse paint order so
	/// the topmost (last-painted) node wins.
	fn hit_test(&self, position: Point<Pixels>, cx: &App) -> HitTarget {
		let anchor = position - self.viewport.origin;
		let zoom = self.state.zoom();
		let data = self.data.read(cx);
		for node in data.nodes().into_iter().rev() {
			let element = NodeElement::from_node(&node, NodeVisualState::default());
			let screen_pos = self.state.graph_to_screen(node.position());
			// Cards are painted scaled by zoom, so their screen-space bounds
			// are the graph-space card size times `zoom`.
			let bounds = Bounds::new(
				screen_pos,
				size(DEFAULT_NODE_WIDTH * zoom, element.height() * zoom),
			);
			if bounds.contains(&anchor) {
				// Back to graph-space card-local coordinates for the element's
				// (unzoomed) hit helpers.
				let local = point(
					(anchor.x - screen_pos.x) / zoom,
					(anchor.y - screen_pos.y) / zoom,
				);
				if let Some(port) = element.port_at(local) {
					return HitTarget::Port(port);
				}
				if element.collapse_toggle_hit(local) || element.enable_toggle_hit(local) {
					return HitTarget::Toggle(node.id());
				}
				return HitTarget::Node(node.id());
			}
		}
		HitTarget::Background
	}

	/// Handles a press on a node's body: updates the selection according to
	/// modifier keys (plain click selects exclusively, Shift toggles) and
	/// begins a potential node drag. Emits
	/// [`NodeGraphEvent::SelectionChanged`] when the selection changed.
	fn on_node_mouse_down(
		&mut self,
		node: NodeId,
		position: Point<Pixels>,
		toggle: bool,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if toggle {
			let mut new_selection = self.state.selection().clone();
			if !new_selection.remove(&node) {
				new_selection.insert(node);
			}
			self.set_selection_and_emit(new_selection, cx);
		} else if !self.state.is_selected(node) {
			self.set_selection_and_emit(BTreeSet::from([node]), cx);
		}
		self.node_drag = Some(NodeDragState {
			nodes: self.state.selection().iter().copied().collect(),
			anchor: position - self.viewport.origin,
			delta: point(px(0.0), px(0.0)),
		});
		cx.notify();
	}

	/// Handles pointer movement during a node drag: updates the accumulated
	/// drag delta in graph space and emits [`NodeGraphEvent::NodeMovePreview`].
	///
	/// The delta is recomputed from the drag anchor on every move (never added
	/// incrementally), so a zoom change mid-drag cannot accumulate error; the
	/// widget draws dragged nodes at model position + delta. Panics (`expect`) if
	/// no node drag is in progress.
	///
	/// # Arguments
	///
	/// * `window` - source of the current cursor position.
	/// * `cx` - emit/notify target.
	fn on_node_drag_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let drag = self.node_drag.as_mut().expect("node drag in progress");
		let cursor = window.mouse_position() - self.viewport.origin;
		drag.delta = self.state.screen_to_graph(cursor) - self.state.screen_to_graph(drag.anchor);
		let (nodes, delta) = (drag.nodes.clone(), drag.delta);
		cx.emit(NodeMovePreview { nodes, delta });
		cx.notify();
	}

	/// Handles pointer release at the end of a node drag: emits the final
	/// [`NodeGraphEvent::NodeMoveRequested`] with the accumulated delta and
	/// clears the transient drag state.
	///
	/// Nothing is emitted when the delta is zero (a click, not a drag), but the
	/// drag state is always cleared and the view repaints, so a later release
	/// cannot replay a stale drag. The event is the undoable operation; the
	/// preceding `NodeMovePreview` events are only live feedback.
	///
	/// # Arguments
	///
	/// * `cx` - emit/notify target; the window is unused.
	fn on_node_drag_end(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
		if let Some(drag) = self.node_drag.take() {
			if drag.delta != point(px(0.0), px(0.0)) {
				cx.emit(NodeMoveRequested {
					nodes: drag.nodes,
					delta: drag.delta,
				});
			}
			cx.notify();
		}
	}

	/// Begins a wire drag from the given port. If the port is a connected
	/// input, the existing edge is "picked up" instead: its other end becomes
	/// the drag source and a [`NodeGraphEvent::DisconnectionRequested`] is
	/// emitted only if the drag ends without a new connection.
	fn begin_wire_drag(&mut self, port: PortId, _window: &mut Window, cx: &mut Context<Self>) {
		let data = self.data.read(cx);
		let mut found: Option<(Point<Pixels>, Point<Pixels>, PortKind, Option<PortDataType>)> =
			None;
		for node in data.nodes() {
			let element = NodeElement::from_node(&node, NodeVisualState::default());
			if let Some(anchor) = element.port_anchor(port) {
				let kind = if node.inputs().into_iter().any(|p| p.id() == port) {
					PortKind::Input
				} else {
					PortKind::Output
				};
				let data_type = node
					.inputs()
					.into_iter()
					.chain(node.outputs())
					.find(|p| p.id() == port)
					.map(|p| p.data_type());
				found = Some((node.position(), anchor, kind, data_type));
				break;
			}
		}
		let (node_pos, anchor, kind, data_type) = match found {
			Some((node_pos, anchor, kind, Some(data_type))) => (node_pos, anchor, kind, data_type),
			_ => return,
		};
		let screen_anchor = self.viewport.origin + self.state.graph_to_screen(node_pos + anchor);

		// Picking up an existing edge: only a connected input drag re-roots the
		// ghost at the far (output) end; an output drag always starts fresh.
		if kind == PortKind::Input {
			if let Some(edge) = data.edges().into_iter().find(|e| e.to_port() == port) {
				if let Some(from_node) = data
					.nodes()
					.into_iter()
					.find(|n| n.id() == edge.from_node())
				{
					let from_element =
						NodeElement::from_node(&from_node, NodeVisualState::default());
					if let Some(far_anchor) = from_element.port_anchor(edge.from_port()) {
						let far_screen = self.viewport.origin
							+ self
								.state
								.graph_to_screen(from_node.position() + far_anchor);
						if let Some(far_type) = from_node
							.outputs()
							.into_iter()
							.find(|p| p.id() == edge.from_port())
							.map(|p| p.data_type())
						{
							self.wire_drag = Some(WireDragState {
								ghost: GhostWire::new(far_screen, &far_type, true),
								source_port: port,
								picked_edge: Some(edge.id()),
								valid_ports: BTreeSet::new(),
							});
							cx.notify();
							return;
						}
					}
				}
			}
		}

		self.wire_drag = Some(WireDragState {
			ghost: GhostWire::new(screen_anchor, &data_type, kind == PortKind::Output),
			source_port: port,
			picked_edge: None,
			valid_ports: BTreeSet::new(),
		});
		cx.notify();
	}

	/// Updates the wire drag: moves the ghost wire's free end to the cursor
	/// and recomputes which ports are valid drop targets by calling
	/// [`NodeGraphDataSource::can_connect`] for each port of the opposite
	/// kind. Ports that pass are highlighted; the ghost wire is drawn in its
	/// invalid state while hovering a port that fails.
	///
	/// The free end snaps to a port anchor when the cursor is within
	/// `PORT_GRAB_RADIUS` of it, and `target_valid` reflects only that hovered
	/// port. The drag direction (which end is the output) comes from the ghost
	/// wire; for a picked-up edge the output is recovered from the edge itself.
	/// Panics (`expect`) if no wire drag is in progress.
	///
	/// # Arguments
	///
	/// * `window` - source of the current cursor position.
	/// * `cx` - data source read plus notify target.
	fn update_wire_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let data = self.data.read(cx);
		let drag = self.wire_drag.as_mut().expect("wire drag in progress");
		let from_output = drag.ghost.is_from_output();
		let source_port = drag.source_port;
		let picked_edge = drag.picked_edge;
		let output_id = if from_output {
			picked_edge
				.and_then(|edge_id| data.edges().into_iter().find(|e| e.id() == edge_id))
				.map(|edge| edge.from_port())
				.unwrap_or(source_port)
		} else {
			source_port
		};
		let cursor = window.mouse_position();
		let mut valid: BTreeSet<PortId> = BTreeSet::new();
		let mut snapped: Option<Point<Pixels>> = None;
		let mut target_valid = false;
		for node in data.nodes() {
			let element = NodeElement::from_node(&node, NodeVisualState::default());
			for port in node.inputs().into_iter().chain(node.outputs()) {
				let port_id = port.id();
				let candidate = if from_output {
					port.kind() == PortKind::Input && data.can_connect(output_id, port_id)
				} else {
					port.kind() == PortKind::Output && data.can_connect(port_id, source_port)
				};
				if candidate {
					valid.insert(port_id);
				}
				if let Some(anchor) = element.port_anchor(port_id) {
					let screen =
						self.viewport.origin + self.state.graph_to_screen(node.position() + anchor);
					let dx = screen.x.as_f32() - cursor.x.as_f32();
					let dy = screen.y.as_f32() - cursor.y.as_f32();
					if dx * dx + dy * dy <= PORT_GRAB_RADIUS.as_f32() * PORT_GRAB_RADIUS.as_f32() {
						snapped = Some(screen);
						target_valid = candidate;
					}
				}
			}
		}
		drag.valid_ports = valid;
		drag.ghost.update(cursor, snapped, target_valid);
		cx.notify();
	}

	/// Ends the wire drag. On a compatible port: emits
	/// [`NodeGraphEvent::ConnectionRequested`]. On empty space: cancels and
	/// emits [`NodeGraphEvent::BackgroundClicked`] at the drop position so
	/// the app may open an "add node" menu pre-wired to the dragged port. On
	/// an incompatible port (or back on the source port): cancels silently.
	///
	/// The hit test re-runs `can_connect` at the drop point and uses
	/// `PORT_GRAB_RADIUS`, matching `update_wire_drag`; `ConnectionRequested.from`
	/// is always the output port, even when the drag started at an input. A drag
	/// that picked up an existing edge (`picked_edge`) and ends in empty space
	/// emits [`NodeGraphEvent::DisconnectionRequested`] for that edge instead of
	/// `BackgroundClicked`. Returns immediately when no wire drag is in progress.
	///
	/// # Arguments
	///
	/// * `window` - source of the current cursor position.
	/// * `cx` - data source read for ports/edges, plus emit/notify target.
	fn end_wire_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let drag = match self.wire_drag.take() {
			Some(drag) => drag,
			None => return,
		};
		let data = self.data.read(cx);
		let from_output = drag.ghost.is_from_output();
		let source_port = drag.source_port;
		let picked_edge = drag.picked_edge;
		let output_id = if from_output {
			picked_edge
				.and_then(|edge_id| data.edges().into_iter().find(|e| e.id() == edge_id))
				.map(|edge| edge.from_port())
				.unwrap_or(source_port)
		} else {
			source_port
		};
		let cursor = window.mouse_position();
		let mut hit: Option<(PortId, bool)> = None;
		'ports: for node in data.nodes() {
			let element = NodeElement::from_node(&node, NodeVisualState::default());
			for port in node.inputs().into_iter().chain(node.outputs()) {
				let port_id = port.id();
				let candidate = if from_output {
					port.kind() == PortKind::Input && data.can_connect(output_id, port_id)
				} else {
					port.kind() == PortKind::Output && data.can_connect(port_id, source_port)
				};
				if let Some(anchor) = element.port_anchor(port_id) {
					let screen =
						self.viewport.origin + self.state.graph_to_screen(node.position() + anchor);
					let dx = screen.x.as_f32() - cursor.x.as_f32();
					let dy = screen.y.as_f32() - cursor.y.as_f32();
					if dx * dx + dy * dy <= PORT_GRAB_RADIUS.as_f32() * PORT_GRAB_RADIUS.as_f32() {
						hit = Some((port_id, candidate));
						break 'ports;
					}
				}
			}
		}
		match hit {
			Some((target, true)) if target != source_port => {
				cx.emit(ConnectionRequested {
					from: output_id,
					to: target,
				});
			}
			// An incompatible port or the port the drag started from: cancel.
			Some(_) => {}
			None => {
				if let Some(edge) = picked_edge {
					cx.emit(DisconnectionRequested { edge });
				} else {
					cx.emit(BackgroundClicked {
						position: self.state.screen_to_graph(cursor - self.viewport.origin),
					});
				}
			}
		}
		cx.notify();
	}

	/// Handles background presses: begins panning (space/middle button) or a
	/// marquee selection (left button), or emits
	/// [`NodeGraphEvent::BackgroundClicked`] on a right click.
	fn on_background_mouse_down(
		&mut self,
		position: Point<Pixels>,
		button: MouseButton,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let anchor = position - self.viewport.origin;
		if self.space_down || button == MouseButton::Middle {
			self.pan_drag = Some(PanDragState {
				start_mouse: position,
				start_offset: self.state.offset(),
			});
		}
		if button == MouseButton::Left {
			self.state.begin_marquee(anchor);
		}
		if button == MouseButton::Right {
			cx.emit(BackgroundClicked {
				position: self.state.screen_to_graph(anchor),
			});
		}
		cx.notify();
	}

	/// Handles pointer movement during a pan drag: repositions the viewport
	/// offset and emits [`NodeGraphEvent::ViewChanged`].
	///
	/// The offset becomes `start_offset + (position - start_mouse)`, so the view
	/// follows the cursor exactly; zoom is untouched. Panics (`expect`) if no pan
	/// drag is in progress, which only happens if the mouse-down that started it
	/// was missed.
	///
	/// # Arguments
	///
	/// * `position` - current window-space cursor position.
	/// * `cx` - emit/notify target.
	fn on_pan_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
		let pan = self.pan_drag.as_ref().expect("pan drag in progress");
		let (start_mouse, start_offset) = (pan.start_mouse, pan.start_offset);
		self.state
			.set_offset(start_offset + (position - start_mouse));
		cx.emit(ViewChanged {
			offset: self.state.offset(),
			zoom: self.state.zoom(),
		});
		cx.notify();
	}

	/// Handles scroll-wheel and pinch gestures: zooms at the cursor via
	/// [`GraphViewState::zoom_at`] and emits [`NodeGraphEvent::ViewChanged`].
	///
	/// Zoom is clamped to `MIN_ZOOM..=MAX_ZOOM` inside `zoom_at`, and the graph
	/// point under the cursor stays put. `ViewChanged` is emitted even when the
	/// clamp left the zoom unchanged, so the app sees every gesture.
	///
	/// # Arguments
	///
	/// * `position` - window-space cursor position; converted to element-local
	///   space before the anchor is applied.
	/// * `factor` - multiplicative zoom step (> 1 zooms in, < 1 zooms out).
	/// * `cx` - emit/notify target.
	fn on_scroll_or_pinch(&mut self, position: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
		self.state.zoom_at(position - self.viewport.origin, factor);
		cx.emit(ViewChanged {
			offset: self.state.offset(),
			zoom: self.state.zoom(),
		});
		cx.notify();
	}

	/// Handles the delete/backspace key: collects the selected nodes plus all
	/// edges incident to them and emits [`NodeGraphEvent::DeleteRequested`].
	/// Does nothing with an empty selection.
	///
	/// The view tracks a node selection only, so `edges` is derived from it: every
	/// edge whose `from_node` or `to_node` is selected. Delivering both in one
	/// event lets the app remove them as a single undo step; the widget itself
	/// only emits the request.
	///
	/// # Arguments
	///
	/// * `cx` - data source read for the edge list, plus emit/notify target; the
	///   window is unused.
	fn on_delete_key(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
		let nodes = self.state.selection().iter().copied().collect::<Vec<_>>();
		if nodes.is_empty() {
			return;
		}
		let data = self.data.read(cx);
		let edges = data
			.edges()
			.into_iter()
			.filter(|edge| {
				nodes
					.iter()
					.any(|node| *node == edge.from_node() || *node == edge.to_node())
			})
			.map(|edge| edge.id())
			.collect::<Vec<_>>();
		cx.emit(DeleteRequested { nodes, edges });
		cx.notify();
	}

	/// Emits [`NodeGraphEvent::SelectionChanged`] if `new` differs from the
	/// current selection, and stores `new`.
	fn set_selection_and_emit(&mut self, new: BTreeSet<NodeId>, cx: &mut Context<Self>) {
		if self.state.selection() == &new {
			return;
		}
		self.state.set_selection(new.clone());
		cx.emit(SelectionChanged { nodes: new });
	}

	/// Ends a marquee drag: selects all nodes intersecting the rectangle, or
	/// treats the press as a plain background click (clear selection + emit
	/// [`NodeGraphEvent::BackgroundClicked`]) when the drag was too small to
	/// count.
	///
	/// "Too small" means the normalized rectangle moved less than
	/// `MARQUEE_DRAG_THRESHOLD` on both axes. Intersection is tested in graph
	/// space against each node's `DEFAULT_NODE_WIDTH` × measured-height box, with
	/// touching counting as inside (`rects_intersect`). Returns quietly when no
	/// marquee is in progress; selection changes are emitted through
	/// `set_selection_and_emit` (no event when the selection is unchanged).
	///
	/// # Arguments
	///
	/// * `cx` - data source read for the node list, plus emit/notify target; the
	///   window is unused.
	fn end_marquee_or_click(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
		let rect = match self.state.end_marquee() {
			Some(rect) => rect,
			None => return,
		};
		let (min, max) = rect.normalized();
		let dragged = (max.x - min.x).as_f32() >= MARQUEE_DRAG_THRESHOLD
			|| (max.y - min.y).as_f32() >= MARQUEE_DRAG_THRESHOLD;
		if dragged {
			let g_min = self.state.screen_to_graph(min);
			let g_max = self.state.screen_to_graph(max);
			let data = self.data.read(cx);
			let mut new_selection = BTreeSet::new();
			for node in data.nodes() {
				let element = NodeElement::from_node(&node, NodeVisualState::default());
				let pos = node.position();
				if rects_intersect(
					g_min,
					g_max,
					pos,
					pos + point(DEFAULT_NODE_WIDTH, element.height()),
				) {
					new_selection.insert(node.id());
				}
			}
			self.set_selection_and_emit(new_selection, cx);
		} else {
			self.set_selection_and_emit(BTreeSet::new(), cx);
			cx.emit(BackgroundClicked {
				position: self.state.screen_to_graph(min),
			});
		}
		cx.notify();
	}

	/// Snapshot of the frame the canvas is about to paint: nodes (in paint
	/// order, dragged nodes last), wires, ghost wire and marquee, all in
	/// window space where applicable.
	fn build_draw(&self, cx: &mut Context<Self>) -> GraphDraw {
		let data = self.data.read(cx);
		let selection = self.state.selection().clone();
		let drag = self.node_drag.as_ref();
		let wire = self.wire_drag.as_ref();
		let viewport_origin = self.viewport.origin;
		let fallback = PortDataType::new("", hsla(0.0, 0.0, 0.5, 1.0));

		let mut port_types: HashMap<PortId, PortDataType> = HashMap::new();
		let mut elements: HashMap<NodeId, (Point<Pixels>, NodeElement)> = HashMap::new();
		let mut order: Vec<NodeId> = Vec::new();
		let mut top: Vec<NodeId> = Vec::new();

		for node in data.nodes() {
			let node_id = node.id();
			for port in node.inputs().into_iter().chain(node.outputs()) {
				port_types.insert(port.id(), port.data_type());
			}
			let has_compatible_port = wire.map_or(false, |w| {
				node.inputs()
					.into_iter()
					.chain(node.outputs())
					.any(|port| w.valid_ports.contains(&port.id()))
			});
			let element = NodeElement::from_node(
				&node,
				NodeVisualState {
					selected: selection.contains(&node_id),
					has_compatible_port,
				},
			);
			let mut pos = node.position();
			if let Some(d) = drag {
				if d.nodes.contains(&node_id) {
					pos = pos + d.delta;
					top.push(node_id);
				} else {
					order.push(node_id);
				}
			} else {
				order.push(node_id);
			}
			elements.insert(
				node_id,
				(viewport_origin + self.state.graph_to_screen(pos), element),
			);
		}
		order.extend(top);

		let mut wires = Vec::new();
		let zoom = self.state.zoom();
		for edge in data.edges() {
			let (from_pos, from_element) = match elements.get(&edge.from_node()) {
				Some(entry) => entry,
				None => continue,
			};
			let (to_pos, to_element) = match elements.get(&edge.to_node()) {
				Some(entry) => entry,
				None => continue,
			};
			let from_anchor = match from_element.port_anchor(edge.from_port()) {
				Some(anchor) => anchor,
				None => continue,
			};
			let to_anchor = match to_element.port_anchor(edge.to_port()) {
				Some(anchor) => anchor,
				None => continue,
			};
			let data_type = port_types
				.get(&edge.from_port())
				.or_else(|| port_types.get(&edge.to_port()))
				.unwrap_or(&fallback);
			let selected =
				selection.contains(&edge.from_node()) || selection.contains(&edge.to_node());
			let wire_state = if selected {
				WireVisualState::Selected
			} else {
				WireVisualState::Normal
			};
			// Anchors are graph-space card-local offsets; scale them into
			// screen space before adding them to the node's screen origin.
			wires.push(Wire::new(
				edge.id(),
				*from_pos + point(from_anchor.x * zoom, from_anchor.y * zoom),
				*to_pos + point(to_anchor.x * zoom, to_anchor.y * zoom),
				data_type,
				wire_state,
			));
		}

		let nodes = order
			.into_iter()
			.map(|id| {
				elements
					.remove(&id)
					.expect("every painted node must have an element")
			})
			.collect();

		let ghost = wire.map(|d| GhostSnapshot {
			from: d.ghost.source(),
			to: d.ghost.free_end(),
			color: d.ghost.color(),
			target_valid: d.ghost.is_target_valid(),
		});

		GraphDraw {
			nodes,
			wires,
			ghost,
			marquee: self.state.marquee().copied(),
			offset: self.state.offset(),
			zoom: self.state.zoom(),
		}
	}

	/// Paints a [`GraphDraw`] snapshot: background, grid, wires, nodes, ghost
	/// wire and marquee overlay.
	///
	/// Static because the paint closure cannot borrow the view: everything it
	/// needs is in `draw`. The order is bottom-to-top and matters: background
	/// quad, grid lines (spaced `GRID_SIZE` graph pixels, scaled by `draw.zoom`
	/// and shifted by `draw.offset`), wires, nodes, ghost wire, marquee.
	///
	/// # Arguments
	///
	/// * `draw` - the frame snapshot built by the canvas prepaint; node/wire
	///   geometry is already in window space, the marquee stays element-local.
	/// * `bounds` - element-local paint area: the background fills it and the
	///   grid lines are generated to cover it.
	/// * `window` - paint target for quads and wires.
	/// * `cx` - only consulted for the theme's default `background`, `border`
	///   and `selected` colors.
	fn paint_draw(draw: &GraphDraw, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
		let colors = cx.default_colors().clone();
		window.paint_quad(fill(bounds, Hsla::from(colors.background)));

		// Grid lines. Lines are spaced GRID_SIZE graph pixels apart; a line
		// with graph coordinate k lands at screen x = offset.x + k*GRID_SIZE*zoom.
		let zoom = draw.zoom;
		let x0 = ((-draw.offset.x.as_f32()) / (GRID_SIZE * zoom)).floor() as i64;
		let x1 = ((bounds.size.width.as_f32() - draw.offset.x.as_f32()) / (GRID_SIZE * zoom)).ceil() as i64;
		for k in x0..=x1 {
			let x = bounds.left() + px(k as f32 * GRID_SIZE * zoom + draw.offset.x.as_f32());
			window.paint_quad(fill(
				Bounds::new(point(x, bounds.top()), size(px(1.0), bounds.size.height)),
				Hsla::from(colors.border).opacity(0.5),
			));
		}
		let y0 = ((-draw.offset.y.as_f32()) / (GRID_SIZE * zoom)).floor() as i64;
		let y1 = ((bounds.size.height.as_f32() - draw.offset.y.as_f32()) / (GRID_SIZE * zoom)).ceil() as i64;
		for k in y0..=y1 {
			let y = bounds.top() + px(k as f32 * GRID_SIZE * zoom + draw.offset.y.as_f32());
			window.paint_quad(fill(
				Bounds::new(point(bounds.left(), y), size(bounds.size.width, px(1.0))),
				Hsla::from(colors.border).opacity(0.5),
			));
		}

		for wire in &draw.wires {
			wire.paint(window, zoom);
		}
		for (origin, element) in &draw.nodes {
			element.paint(*origin, zoom, window, cx);
		}
		if let Some(ghost) = &draw.ghost {
			paint_ghost(
				window,
				ghost.from,
				ghost.to,
				ghost.color,
				ghost.target_valid,
				zoom,
			);
		}
		if let Some(marquee) = &draw.marquee {
			let (min, max) = marquee.normalized();
			let marquee_bounds = Bounds::from_corners(bounds.origin + min, bounds.origin + max);
			window.paint_quad(fill(
				marquee_bounds,
				Hsla::from(colors.selected).opacity(0.15),
			));
			window.paint_quad(PaintQuad {
				bounds: marquee_bounds,
				corner_radii: Corners::all(px(0.0)),
				background: hsla(0.0, 0.0, 0.0, 0.0).into(),
				border_widths: Edges::all(px(1.0)),
				border_color: Hsla::from(colors.selected),
				border_style: BorderStyle::Solid,
			});
		}
	}
}

impl<D: NodeGraphDataSource + 'static> EventEmitter<NodeGraphEvent> for NodeGraphView<D> {}

impl<D: NodeGraphDataSource + 'static> Focusable for NodeGraphView<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: NodeGraphDataSource + 'static> Render for NodeGraphView<D> {
	/// Renders the graph: a full-size background layer (grid + pan/zoom
	/// handlers), then wires below nodes in graph-space order, then the
	/// marquee rectangle and the ghost wire as overlays.
	///
	/// Layout/painting is done in screen space; node and wire geometry is
	/// computed by mapping graph-space model coordinates through
	/// [`GraphViewState::graph_to_screen`]. Wire anchors come from
	/// [`NodeElement::port_anchor`](crate::widgets::node_graph::NodeElement::port_anchor)
	/// so wires always land on port dots.
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let focus_handle = self.focus_handle.clone();
		let entity = cx.entity();

		div()
			.relative()
			.size_full()
			.track_focus(&focus_handle)
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
				if event.keystroke.key == "space" {
					this.space_down = true;
				} else if event.keystroke.key == "delete" || event.keystroke.key == "backspace" {
					this.on_delete_key(window, cx);
				}
			}))
			.on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, _cx| {
				if event.keystroke.key == "space" {
					this.space_down = false;
				}
			}))
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, event: &MouseDownEvent, window, cx| {
					window.focus(&this.focus_handle, cx);
					match this.hit_test(event.position, cx) {
						HitTarget::Port(port) => this.begin_wire_drag(port, window, cx),
						HitTarget::Toggle(node) => {
							this.set_selection_and_emit(BTreeSet::from([node]), cx);
							cx.notify();
						}
						HitTarget::Node(node) => {
							this.on_node_mouse_down(
								node,
								event.position,
								event.modifiers.shift,
								window,
								cx,
							);
						}
						HitTarget::Background => {
							this.on_background_mouse_down(
								event.position,
								MouseButton::Left,
								window,
								cx,
							);
						}
					}
				}),
			)
			.on_mouse_down(
				MouseButton::Middle,
				cx.listener(|this, event: &MouseDownEvent, window, cx| {
					window.focus(&this.focus_handle, cx);
					this.on_background_mouse_down(event.position, MouseButton::Middle, window, cx);
				}),
			)
			.on_mouse_down(
				MouseButton::Right,
				cx.listener(|this, event: &MouseDownEvent, window, cx| {
					window.focus(&this.focus_handle, cx);
					match this.hit_test(event.position, cx) {
						HitTarget::Background => {
							this.on_background_mouse_down(
								event.position,
								MouseButton::Right,
								window,
								cx,
							);
						}
						// A right-clicked node becomes the sole selection
						// when it is not part of it, then its context menu
						// is requested (the C++ `NodeView` behavior).
						HitTarget::Node(node) => {
							if !this.state.selection().contains(&node) {
								this.set_selection_and_emit(BTreeSet::from([node]), cx);
							}
							cx.emit(NodeContextMenuRequested {
								node,
								position: event.position,
							});
							cx.notify();
						}
						_ => {}
					}
				}),
			)
			.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
				if this.node_drag.is_some() {
					this.on_node_drag_move(window, cx);
				} else if this.wire_drag.is_some() {
					this.update_wire_drag(window, cx);
				} else if this.pan_drag.is_some() {
					this.on_pan_drag_move(event.position, cx);
				} else if this.state.marquee().is_some() {
					this.state
						.update_marquee(event.position - this.viewport.origin);
					cx.notify();
				}
			}))
			.capture_any_mouse_up(cx.listener(|this, _event, window, cx| {
				if this.node_drag.is_some() {
					this.on_node_drag_end(window, cx);
				} else if this.wire_drag.is_some() {
					this.end_wire_drag(window, cx);
				} else if this.pan_drag.is_some() {
					this.pan_drag = None;
					cx.notify();
				} else {
					this.end_marquee_or_click(window, cx);
				}
			}))
			.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
				let factor = match event.delta {
					ScrollDelta::Pixels(delta) => 1.0 + delta.y.as_f32() * 0.002,
					ScrollDelta::Lines(lines) => 1.0 + lines.y * 0.1,
				};
				this.on_scroll_or_pinch(event.position, factor, cx);
			}))
			.on_pinch(cx.listener(|this, event: &PinchEvent, _window, cx| {
				this.on_scroll_or_pinch(event.position, 1.0 + event.delta, cx);
			}))
			.child(
				canvas(
					move |bounds, _window, cx| {
						entity.update(cx, |this, cx| {
							this.viewport = bounds;
							this.build_draw(cx)
						})
					},
					move |bounds, draw: GraphDraw, window, cx| {
						NodeGraphView::<D>::paint_draw(&draw, bounds, window, cx);
					},
				)
				.size_full(),
			)
	}
}

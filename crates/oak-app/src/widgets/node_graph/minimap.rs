//! Optional minimap overlay for the node graph.
//!
//! The minimap is a small corner overlay showing a scaled-down viewport
//! indicator: a translucent backdrop plus a rectangle marking the region of
//! the graph currently visible, derived from the view's pan offset and zoom.
//!
//! **Scope note.** The minimap is intentionally minimal: it draws no node
//! rectangles and supports no click/drag navigation, because [`render`]
//! receives neither the data source nor a mutable view state — it only
//! reflects the viewport. [`graph_bounds`] reserves the bounding-box
//! computation a future content-aware minimap would need; it is not yet wired
//! in.

// `graph_bounds` reserves the bounding-box computation for a future
// content-aware minimap; it has no caller yet.
#![allow(dead_code)]
// The `D` parameter is part of the render contract (the data source the
// minimap would read for node rectangles); it is unused until that feature
// lands.
#![allow(clippy::extra_unused_type_parameters)]

use gpui_kit::{
	Bounds, Empty, IntoElement, Pixels, Point, Window, canvas, deferred, fill, hsla, point, px,
	size,
};

use crate::widgets::node_graph::{
	DEFAULT_NODE_WIDTH, GraphViewState, NodeData, NodeElement, NodeGraphDataSource, NodeVisualState,
};

/// Scale factor from graph-space coordinates to minimap coordinates.
pub const MINIMAP_CONTENT_SCALE: f32 = 0.15;

/// A small overview map of the entire graph, drawn as a corner overlay.
///
/// Renders a translucent backdrop and a highlight rectangle indicating the
/// currently visible region (derived from the view's offset and zoom). The
/// minimap draws no node rectangles and does not react to clicks; it is a
/// passive viewport indicator.
pub struct GraphMinimap {
	/// Whether the minimap is shown. Toggled by the app's view menu; the
	/// minimap renders nothing and ignores input when `false`.
	visible: bool,
}

impl Default for GraphMinimap {
	fn default() -> Self {
		Self { visible: true }
	}
}

impl GraphMinimap {
	/// Creates a visible minimap overlay.
	pub fn new() -> Self {
		Self::default()
	}

	/// Shows or hides the minimap overlay.
	///
	/// `visible = false` makes the minimap render nothing and ignore input;
	/// `true` restores the passive viewport indicator. The flag is plain
	/// state (defaults to visible via [`Self::default`]) — callers must
	/// notify/redraw the node-graph view themselves after flipping it, as the
	/// view menu does. Paired with [`Self::is_visible`].
	pub fn set_visible(&mut self, visible: bool) {
		self.visible = visible;
	}

	/// Returns whether the minimap is currently shown.
	pub fn is_visible(&self) -> bool {
		self.visible
	}

	/// Computes the axis-aligned bounding box of all nodes in graph space,
	/// used as the minimap's content rect. Returns `None` for an empty
	/// graph (the minimap then renders only its backdrop).
	///
	/// Node extents are derived from each node's position plus its rendered
	/// card size ([`DEFAULT_NODE_WIDTH`] × [`NodeElement::height`]).
	fn graph_bounds<D: NodeGraphDataSource>(data: &D) -> Option<(Point<Pixels>, Point<Pixels>)> {
		let mut nodes = data.nodes().into_iter();
		let first = nodes.next()?;
		let extent = |node: &D::Node| {
			let pos = node.position();
			(
				pos,
				pos + point(
					DEFAULT_NODE_WIDTH,
					NodeElement::from_node(node, NodeVisualState::default()).height(),
				),
			)
		};
		let (mut min, mut max) = extent(&first);
		for node in nodes {
			let (node_min, node_max) = extent(&node);
			min = min.min(&node_min);
			max = max.max(&node_max);
		}
		Some((min, max))
	}

	/// Renders the minimap: a translucent backdrop with a viewport indicator
	/// rectangle, laid out over `viewport_bounds` (the main view's
	/// screen-space bounds).
	///
	/// The viewport rectangle is derived from `state` (offset + zoom): the
	/// screen-space viewport is mapped back into graph space (`-offset / zoom`
	/// plus `viewport size / zoom`) and then down to minimap scale.
	pub fn render<D: NodeGraphDataSource>(
		&mut self,
		state: &GraphViewState,
		viewport_bounds: Bounds<Pixels>,
		_window: &mut Window,
	) -> impl IntoElement {
		if !self.visible {
			return deferred(Empty);
		}
		let offset = state.offset();
		let zoom = state.zoom();
		deferred(canvas(
			move |_bounds, _window, _cx| MinimapDraw {
				offset,
				zoom,
				viewport_bounds,
			},
			move |bounds, draw, window, _cx| {
				// Backdrop.
				window.paint_quad(fill(bounds, hsla(0.0, 0.0, 0.0, 0.6)));

				// Viewport indicator: the graph-space viewport rect (screen
				// size scaled back through `zoom`) mapped down to minimap
				// scale, positioned at `-offset / zoom`.
				let scale = MINIMAP_CONTENT_SCALE;
				let origin = bounds.origin
					+ point(
						px(-(draw.offset.x.as_f32() / draw.zoom) * scale),
						px(-(draw.offset.y.as_f32() / draw.zoom) * scale),
					);
				let vp_size = size(
					px(draw.viewport_bounds.size.width.as_f32() / draw.zoom * scale),
					px(draw.viewport_bounds.size.height.as_f32() / draw.zoom * scale),
				);
				window.paint_quad(fill(
					Bounds::new(origin, vp_size),
					hsla(0.63, 0.55, 0.55, 0.5),
				));
			},
		))
	}
}

/// Per-frame snapshot passed from the canvas prepaint to its paint closure.
#[derive(Clone, Copy)]
struct MinimapDraw {
	/// Pan offset (screen-space position of the graph origin).
	offset: Point<Pixels>,
	/// Zoom factor.
	zoom: f32,
	/// The main view's screen-space bounds.
	viewport_bounds: Bounds<Pixels>,
}

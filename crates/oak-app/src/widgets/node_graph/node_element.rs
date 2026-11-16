//! Rendering of a single node card.
//!
//! [`NodeElement`] draws one node of the graph: header, port columns, status
//! styling. It is used internally by
//! [`NodeGraphView`](crate::widgets::node_graph::NodeGraphView) but is public so apps
//! can customize or reuse the node chrome.
//!
//! # Sizing and port anchors
//!
//! The node is a fixed-width column (`DEFAULT_NODE_WIDTH`) laid out as:
//!
//! ```text
//! ┌──────────────────────────┐
//! │ ▶ Title            (on)  │  header — colored, carries collapse & enable toggles
//! │ ● in               out ● │  one row per max(inputs, outputs) index
//! │ ● mask                   │
//! └──────────────────────────┘
//! ```
//!
//! Input ports form a left-aligned column, output ports a right-aligned
//! column, and row *i* of each column shares the same y coordinate, so the
//! port dots of opposite sides on the same row are horizontally aligned.
//!
//! Wires attach at **port dot centers**. The single source of truth for a
//! port's anchor point is [`NodeElement::port_anchor`], computed as:
//!
//! ```text
//! anchor.x = node_bounds.left()  + PORT_DOT_RADIUS + PORT_INSET        (inputs)
//! anchor.x = node_bounds.right() - PORT_DOT_RADIUS - PORT_INSET        (outputs)
//! anchor.y = node_bounds.top()   + HEADER_HEIGHT + row * PORT_ROW_HEIGHT
//!            + PORT_ROW_HEIGHT / 2
//! ```
//!
//! All coordinates are in the node's local space; the view adds the node's
//! screen-space origin. Wire rendering ([`crate::widgets::node_graph::wire`]) uses the
//! same function, so anchors and dots can never drift apart.

use gpui_kit::{
	App, BorderStyle, Bounds, Corners, Edges, Font, Hsla, PaintQuad, Pixels, Point, SharedString,
	TextAlign, TextRun, Window, colors::DefaultColors, fill, hsla, point, px, size,
};

use crate::widgets::node_graph::{NodeData, NodeId, PortId, data::PortData};

/// The default width of a node card. Node width is fixed; only the height
/// grows with the port count.
pub const DEFAULT_NODE_WIDTH: Pixels = px(180.0);

/// Height of the node header bar.
pub const HEADER_HEIGHT: Pixels = px(28.0);

/// Height of a single port row; both port columns share this row pitch.
pub const PORT_ROW_HEIGHT: Pixels = px(22.0);

/// Radius of a port dot.
pub const PORT_DOT_RADIUS: Pixels = px(5.0);

/// Horizontal distance between the node's edge and the port dot center.
pub const PORT_INSET: Pixels = px(8.0);

/// Visual state of a node card, supplied by the view at render time.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeVisualState {
	/// Whether the node is part of the current selection (drawn with a
	/// selection outline).
	pub selected: bool,
	/// Whether a wire drag is in progress and this node contains at least
	/// one port that [`NodeGraphDataSource::can_connect`] approved as a drop
	/// target (drawn with a subtle glow).
	///
	/// [`NodeGraphDataSource::can_connect`]: crate::widgets::node_graph::NodeGraphDataSource::can_connect
	pub has_compatible_port: bool,
}

/// A single rendered node card.
///
/// Constructed per frame by the view from a [`NodeData`] snapshot plus a
/// [`NodeVisualState`]. Carries no interaction state of its own; mouse
/// handling for drags and wire pulls is installed by
/// [`NodeGraphView`](crate::widgets::node_graph::NodeGraphView), which owns the
/// gesture state machine.
pub struct NodeElement {
	node: NodeId,
	title: SharedString,
	header_color: Option<Hsla>,
	collapsed: bool,
	enabled: bool,
	visual: NodeVisualState,
	inputs: Vec<PortRow>,
	outputs: Vec<PortRow>,
}

/// One rendered port row: everything needed to draw a port dot and label
/// without re-querying the data source.
#[derive(Clone, Debug)]
struct PortRow {
	id: PortId,
	label: SharedString,
	color: Hsla,
	connected: bool,
}

impl NodeElement {
	/// Builds the element from a node snapshot and its visual state.
	///
	/// Reads title, header color, collapse/enable flags and both port columns
	/// off `node`. Port rows are taken in the order returned by
	/// [`NodeData::inputs`] / [`NodeData::outputs`], which defines their
	/// top-to-bottom draw order.
	pub fn from_node<N: NodeData>(node: &N, visual: NodeVisualState) -> Self {
		let inputs = node
			.inputs()
			.into_iter()
			.map(|port| PortRow {
				id: port.id(),
				label: port.label(),
				color: port.data_type().color,
				connected: port.is_connected(),
			})
			.collect();
		let outputs = node
			.outputs()
			.into_iter()
			.map(|port| PortRow {
				id: port.id(),
				label: port.label(),
				color: port.data_type().color,
				connected: port.is_connected(),
			})
			.collect();
		Self {
			node: node.id(),
			title: node.title(),
			header_color: node.header_color(),
			collapsed: node.is_collapsed(),
			enabled: node.is_enabled(),
			visual,
			inputs,
			outputs,
		}
	}

	/// Returns the id of the node this element renders.
	pub fn node_id(&self) -> NodeId {
		self.node
	}

	/// Returns the total height of the node card: the header plus
	/// `max(inputs, outputs)` port rows (zero rows when collapsed).
	pub fn height(&self) -> Pixels {
		if self.collapsed {
			HEADER_HEIGHT
		} else {
			HEADER_HEIGHT + PORT_ROW_HEIGHT * self.inputs.len().max(self.outputs.len()) as f32
		}
	}

	/// Computes the node-local anchor point (port dot center) of the given
	/// port, per the formula in the [module docs](crate::widgets::node_graph::node_element).
	/// Wires attach here.
	///
	/// Returns `None` when the port is not part of this node or the node is
	/// collapsed (collapsed nodes expose no anchors and cannot be
	/// connection targets).
	///
	/// # Panics
	///
	/// Never panics; unknown ports yield `None`.
	pub fn port_anchor(&self, port: PortId) -> Option<Point<Pixels>> {
		if self.collapsed {
			return None;
		}
		let row_y =
			|row: usize| HEADER_HEIGHT + PORT_ROW_HEIGHT * row as f32 + PORT_ROW_HEIGHT * 0.5;
		if let Some(row) = self.inputs.iter().position(|p| p.id == port) {
			return Some(point(PORT_DOT_RADIUS + PORT_INSET, row_y(row)));
		}
		if let Some(row) = self.outputs.iter().position(|p| p.id == port) {
			return Some(point(
				DEFAULT_NODE_WIDTH - PORT_DOT_RADIUS - PORT_INSET,
				row_y(row),
			));
		}
		None
	}

	/// Hit-tests a node-local point against port dots and returns the id of
	/// the port whose dot (inflated by a small grab margin) contains it.
	/// Used to start wire drags. Header and body hits return `None`.
	pub fn port_at(&self, position: Point<Pixels>) -> Option<PortId> {
		let hit_radius = PORT_DOT_RADIUS + px(4.0);
		for port in self.inputs.iter().chain(self.outputs.iter()) {
			if let Some(anchor) = self.port_anchor(port.id) {
				let dx = (position.x - anchor.x).as_f32();
				let dy = (position.y - anchor.y).as_f32();
				if dx * dx + dy * dy <= hit_radius.as_f32() * hit_radius.as_f32() {
					return Some(port.id);
				}
			}
		}
		None
	}

	/// Returns whether a node-local point lands on the collapse toggle in the
	/// header. The view uses this to distinguish "toggle collapse" clicks
	/// from drag starts.
	pub fn collapse_toggle_hit(&self, position: Point<Pixels>) -> bool {
		position.x.as_f32() >= 0.0
			&& position.x.as_f32() <= HEADER_HEIGHT.as_f32()
			&& position.y.as_f32() >= 0.0
			&& position.y.as_f32() <= HEADER_HEIGHT.as_f32()
	}

	/// Returns whether a node-local point lands on the enable/bypass toggle
	/// in the header. Toggling emits no dedicated event — it is handled like
	/// any other edit: the view emits a request and the app flips the flag in
	/// its model.
	pub fn enable_toggle_hit(&self, position: Point<Pixels>) -> bool {
		position.x.as_f32() >= DEFAULT_NODE_WIDTH.as_f32() - HEADER_HEIGHT.as_f32()
			&& position.x.as_f32() <= DEFAULT_NODE_WIDTH.as_f32()
			&& position.y.as_f32() >= 0.0
			&& position.y.as_f32() <= HEADER_HEIGHT.as_f32()
	}

	/// Paints the node card into the current window layer: header with title
	/// and toggles, port dots tinted by data type (filled when connected,
	/// hollow otherwise) with labels, selection outline, disabled dimming and
	/// the compatible-port glow. `origin` is the card's screen-space top-left
	/// corner; all card geometry is defined in graph space and scaled by
	/// `zoom` here, so cards shrink/grow consistently with node positions.
	pub(crate) fn paint(
		&self,
		origin: Point<Pixels>,
		zoom: f32,
		window: &mut Window,
		cx: &mut App,
	) {
		let colors = cx.default_colors().clone();
		let width = DEFAULT_NODE_WIDTH * zoom;
		let header_h = HEADER_HEIGHT * zoom;
		let dot_r = PORT_DOT_RADIUS * zoom;
		let bounds = Bounds::new(origin, size(width, self.height() * zoom));

		// Compatible-port glow: a slightly inflated rect behind the card while
		// a wire drag offers at least one valid drop target on this node.
		if self.visual.has_compatible_port {
			let glow_pad = px(2.0) * zoom;
			let glow = Bounds::new(
				point(origin.x - glow_pad, origin.y - glow_pad),
				size(width + glow_pad * 2.0, bounds.size.height + glow_pad * 2.0),
			);
			window.paint_quad(fill(glow, Hsla::from(colors.selected).opacity(0.2)));
		}

		// Card body.
		window.paint_quad(fill(bounds, colors.background));

		// Border quad: transparent fill, themed border (accent when selected).
		// The 6px corner matches the design's softly rounded node cards.
		window.paint_quad(PaintQuad {
			bounds,
			corner_radii: Corners::all(px(6.0) * zoom),
			background: hsla(0.0, 0.0, 0.0, 0.0).into(),
			border_widths: Edges::all(if self.visual.selected {
				px(1.5)
			} else {
				px(1.0)
			}),
			border_color: if self.visual.selected {
				Hsla::from(colors.selected)
			} else {
				Hsla::from(colors.border)
			},
			border_style: BorderStyle::Solid,
		});

		// Header bar with the node's accent color (or the theme container
		// color), containing the title and the collapse/enable toggles.
		let header_bounds = Bounds::new(origin, size(width, header_h));
		window.paint_quad(fill(
			header_bounds,
			self.header_color.unwrap_or(Hsla::from(colors.container)),
		));

		let text_y = bounds.top() + (header_h - px(12.0) * zoom) * 0.5;
		paint_text(
			window,
			cx,
			&self.title,
			px(12.0) * zoom,
			point(bounds.left() + px(28.0) * zoom, text_y),
			px(12.0) * zoom,
			Hsla::from(colors.text),
			TextAlign::Left,
			None,
		);

		// Collapse toggle: "▶" when collapsed (click to expand), "▼" when
		// expanded (click to collapse).
		paint_text(
			window,
			cx,
			if self.collapsed { "▶" } else { "▼" },
			px(10.0) * zoom,
			point(bounds.left() + px(10.0) * zoom, text_y),
			px(12.0) * zoom,
			Hsla::from(colors.text),
			TextAlign::Left,
			None,
		);

		// Enable toggle glyph (power symbol) on the right edge of the header.
		paint_text(
			window,
			cx,
			"⏻",
			px(12.0) * zoom,
			point(bounds.right() - px(20.0) * zoom, text_y),
			px(12.0) * zoom,
			Hsla::from(colors.text),
			TextAlign::Left,
			None,
		);

		// Port dots and labels, only when the node is expanded.
		if !self.collapsed {
			let label_font_size = px(11.0) * zoom;
			let label_height = px(12.0) * zoom;
			for port in self.inputs.iter().chain(self.outputs.iter()) {
				let Some(anchor) = self.port_anchor(port.id) else {
					continue;
				};
				// The anchor is graph-space; scale it into the card's
				// screen-space rectangle.
				let center = point(origin.x + anchor.x * zoom, origin.y + anchor.y * zoom);
				let dot_bounds = Bounds::new(
					point(center.x - dot_r, center.y - dot_r),
					size(dot_r * 2.0, dot_r * 2.0),
				);
				if port.connected {
					// Connected dots are solid tinted circles.
					window.paint_quad(PaintQuad {
						bounds: dot_bounds,
						corner_radii: Corners::all(dot_r),
						background: port.color.into(),
						border_widths: Edges::all(px(0.0)),
						border_color: hsla(0.0, 0.0, 0.0, 0.0),
						border_style: BorderStyle::Solid,
					});
				} else {
					// Unconnected dots are hollow: a tinted ring around the
					// card's background color.
					window.paint_quad(fill(dot_bounds, port.color));
					let inset = px(2.0) * zoom;
					let inner = Bounds::new(
						point(dot_bounds.origin.x + inset, dot_bounds.origin.y + inset),
						size(
							dot_bounds.size.width - inset * 2.0,
							dot_bounds.size.height - inset * 2.0,
						),
					);
					window.paint_quad(fill(inner, colors.background));
				}

				if !port.label.is_empty() {
					if self.inputs.iter().any(|p| p.id == port.id) {
						// Input labels: left-aligned, starting right of the dot.
						paint_text(
							window,
							cx,
							&port.label,
							label_font_size,
							point(center.x + dot_r + px(6.0) * zoom, center.y - px(6.0) * zoom),
							label_height,
							Hsla::from(colors.text),
							TextAlign::Left,
							None,
						);
					} else {
						// Output labels: right-aligned so they end just left of
						// the dot. The box origin sits `align_width` left of the
						// dot; the label's right edge lands at the box right.
						let align_width = px(100.0) * zoom;
						paint_text(
							window,
							cx,
							&port.label,
							label_font_size,
							point(
								center.x - dot_r - px(6.0) * zoom - align_width,
								center.y - px(6.0) * zoom,
							),
							label_height,
							Hsla::from(colors.text),
							TextAlign::Right,
							Some(align_width),
						);
					}
				}
			}
		}

		// Disabled nodes are dimmed with a dark overlay.
		if !self.enabled {
			window.paint_quad(fill(bounds, hsla(0.0, 0.0, 0.0, 0.5)));
		}
	}
}

/// Shapes and paints a single text line at `origin` (the top-left of the
/// line box) with the given font size, line height, alignment and color.
fn paint_text(
	window: &mut Window,
	cx: &mut App,
	text: &str,
	font_size: Pixels,
	origin: Point<Pixels>,
	line_height: Pixels,
	color: Hsla,
	align: TextAlign,
	align_width: Option<Pixels>,
) {
	let line = window.text_system().shape_line(
		SharedString::from(text),
		font_size,
		&[TextRun {
			len: text.len(),
			font: Font::default(),
			color,
			background_color: None,
			underline: None,
			strikethrough: None,
		}],
		None,
	);
	let _ = line.paint(origin, line_height, align, align_width, window, cx);
}

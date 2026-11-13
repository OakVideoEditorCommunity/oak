//! Wire (edge) rendering for the node graph.
//!
//! Wires are cubic bezier curves drawn with [`PathBuilder`](gpui_kit::PathBuilder), anchored at port
//! dot centers (see [`NodeElement::port_anchor`]). This module also covers
//! the transient "ghost" wire shown while the user drags a connection.
//!
//! [`NodeElement::port_anchor`]: crate::widgets::node_graph::NodeElement::port_anchor

use gpui_kit::{Hsla, Path, PathBuilder, Pixels, Point, Window, hsla, point, px};

use crate::widgets::node_graph::{EdgeId, PortDataType};

/// Horizontal distance the bezier control points are pushed out from the
/// endpoints. Larger values make wires leave ports more "horizontally" and
/// sag less. Scaled by zoom so screen-space curvature stays constant.
pub const WIRE_CURVATURE: Pixels = px(60.0);

/// The visual state of a wire, chosen by the view per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WireVisualState {
	/// A regular, idle wire.
	#[default]
	Normal,
	/// The wire is hovered (slightly brightened; click targets become
	/// discoverable).
	Hovered,
	/// The wire is part of the selection (accent color, thicker stroke).
	Selected,
	/// The ghost wire of an in-progress drag whose current hover target (if
	/// any) was rejected by
	/// [`NodeGraphDataSource::can_connect`](crate::widgets::node_graph::NodeGraphDataSource::can_connect).
	/// Drawn dashed/red to signal "dropping here will not connect".
	InvalidDrag,
}

/// A fully-resolved wire ready to paint: both endpoints are already computed
/// in screen space.
///
/// Built per frame by [`NodeGraphView`](crate::widgets::node_graph::NodeGraphView)
/// from an [`EdgeData`](crate::widgets::node_graph::EdgeData) plus the port anchors of
/// the two endpoint nodes.
pub struct Wire {
	edge: EdgeId,
	from: Point<Pixels>,
	to: Point<Pixels>,
	color: Hsla,
	state: WireVisualState,
}

impl Wire {
	/// Creates a wire between two screen-space anchor points, tinted with the
	/// connection's data-type color.
	///
	/// `edge` is the stable edge id (returned by [`Self::edge`]); `from` and
	/// `to` are the already-resolved screen anchors of the source output and
	/// target input (the view computes them per frame); `data_type` is the
	/// connection's wire type — only its `color` is stored; `state` selects the
	/// stroke styling in [`Self::paint`]. No validation of the endpoints or of
	/// the connection itself happens here.
	pub fn new(
		edge: EdgeId,
		from: Point<Pixels>,
		to: Point<Pixels>,
		data_type: &PortDataType,
		state: WireVisualState,
	) -> Self {
		Self {
			edge,
			from,
			to,
			color: data_type.color,
			state,
		}
	}

	/// Returns the edge this wire represents.
	pub fn edge(&self) -> EdgeId {
		self.edge
	}

	/// Builds the cubic bezier [`gpui_kit::Path`] for a wire from `from` to
	/// `to`, leaving both endpoints horizontally: the control points are
	/// placed `WIRE_CURVATURE * zoom` to the right of `from` and to the left
	/// of `to`. Shared by regular wires and the ghost wire so both have
	/// identical curvature behavior.
	///
	/// Returns `None` when the path cannot be built (degenerate input); the
	/// caller simply skips painting that frame.
	pub fn build_path(from: Point<Pixels>, to: Point<Pixels>, zoom: f32) -> Option<Path<Pixels>> {
		wire_path(from, to, zoom, px(2.0), None)
	}

	/// Paints the wire with [`Window::paint_path`], applying the stroke width
	/// and color adjustments implied by its [`WireVisualState`].
	///
	/// `window` is the paint target and `zoom` is the node graph's zoom factor,
	/// which scales the bezier curvature (not the stroke width). Style per
	/// state: `Normal` draws the data-type colour at 60% opacity and 2px,
	/// `Hovered` the full colour at 2.5px, `Selected` the full colour at 3px,
	/// and `InvalidDrag` a red 6/4 dashed 2px stroke. Nothing is painted when
	/// the path cannot be built for this frame.
	pub fn paint(&self, window: &mut Window, zoom: f32) {
		let (color, width, dash) = match self.state {
			WireVisualState::Normal => (self.color.opacity(0.6), px(2.0), None),
			WireVisualState::Hovered => (self.color, px(2.5), None),
			WireVisualState::Selected => (self.color, px(3.0), None),
			WireVisualState::InvalidDrag => (
				hsla(0.0, 0.85, 0.55, 1.0),
				px(2.0),
				Some([px(6.0), px(4.0)]),
			),
		};
		if let Some(path) = wire_path(
			self.from,
			self.to,
			zoom,
			width,
			dash.as_ref().map(|dash| &dash[..]),
		) {
			window.paint_path(path, color);
		}
	}
}

/// Builds the cubic bezier path for a wire stroke with the given width and
/// optional dash array. This is the single place the wire geometry lives;
/// [`Wire::build_path`] and the ghost wire both delegate to it so every wire
/// shares the same curvature behavior.
pub(crate) fn wire_path(
	from: Point<Pixels>,
	to: Point<Pixels>,
	zoom: f32,
	width: Pixels,
	dash: Option<&[Pixels]>,
) -> Option<Path<Pixels>> {
	let mut builder = PathBuilder::stroke(width);
	if let Some(dash) = dash {
		builder = builder.dash_array(dash);
	}
	let curvature = WIRE_CURVATURE * zoom;
	builder.move_to(from);
	builder.cubic_bezier_to(
		to,
		point(from.x + curvature, from.y),
		point(to.x - curvature, to.y),
	);
	builder.build().ok()
}

/// The transient "ghost" wire shown while the user drags a connection from a
/// port.
///
/// One end stays fixed at the source port's anchor; the other follows the
/// cursor. When the cursor hovers a port, the free end snaps to that port's
/// anchor and the ghost switches between [`WireVisualState::Hovered`] and
/// [`WireVisualState::InvalidDrag`] depending on
/// [`NodeGraphDataSource::can_connect`](crate::widgets::node_graph::NodeGraphDataSource::can_connect).
pub struct GhostWire {
	/// The screen-space anchor of the port the drag started from.
	source: Point<Pixels>,
	/// The current screen-space position of the free end (cursor, or a
	/// snapped hover-target anchor).
	free_end: Point<Pixels>,
	/// Data type of the source port; tints the ghost.
	color: Hsla,
	/// Whether the current hover target is a valid drop (drives the
	/// [`WireVisualState::InvalidDrag`] styling).
	target_valid: bool,
	/// Whether the drag started from an output port. When `false` (drag
	/// started from an input), `source`/`free_end` are swapped when building
	/// the path so the bezier tangents still point the right way.
	from_output: bool,
}

impl GhostWire {
	/// Creates a ghost wire anchored at `source` (screen space), tinted with
	/// the source port's data type. `from_output` records the drag direction;
	/// see the field docs.
	pub fn new(source: Point<Pixels>, data_type: &PortDataType, from_output: bool) -> Self {
		Self {
			source,
			free_end: source,
			color: data_type.color,
			target_valid: false,
			from_output,
		}
	}

	/// Returns the screen-space anchor of the port the drag started from.
	pub(crate) fn source(&self) -> Point<Pixels> {
		self.source
	}

	/// Returns the current screen-space position of the free end.
	pub(crate) fn free_end(&self) -> Point<Pixels> {
		self.free_end
	}

	/// Returns the data-type color tinting the ghost.
	pub(crate) fn color(&self) -> Hsla {
		self.color
	}

	/// Returns whether the currently hovered port is a valid drop target.
	pub(crate) fn is_target_valid(&self) -> bool {
		self.target_valid
	}

	/// Returns whether the drag started from an output port.
	pub(crate) fn is_from_output(&self) -> bool {
		self.from_output
	}

	/// Moves the free end to `cursor` (screen space) and records whether the
	/// currently hovered port — if any — is a valid drop target. Pass
	/// `snapped = Some(anchor)` instead of the raw cursor when the cursor is
	/// inside a port's grab radius, so the ghost visually snaps onto it.
	pub fn update(
		&mut self,
		cursor: Point<Pixels>,
		snapped: Option<Point<Pixels>>,
		target_valid: bool,
	) {
		self.free_end = snapped.unwrap_or(cursor);
		self.target_valid = target_valid;
	}

	/// Paints the ghost wire using the same bezier shape as [`Wire`], with
	/// its state styling.
	///
	/// `window` is the paint target and `zoom` scales the bezier curvature.
	/// The endpoints are ordered by `from_output`, so the tangents always leave
	/// the source horizontally even when the drag started at an input; the
	/// stroke itself comes from [`paint_ghost`] (solid data-type tint for a
	/// valid drop, red dashed otherwise).
	pub fn paint(&self, window: &mut Window, zoom: f32) {
		let (from, to) = if self.from_output {
			(self.source, self.free_end)
		} else {
			(self.free_end, self.source)
		};
		paint_ghost(window, from, to, self.color, self.target_valid, zoom);
	}
}

/// Paints the ghost wire between two screen-space anchors. Valid drops are
/// drawn solid with the data-type tint; invalid drops (hovering an
/// incompatible port) are drawn dashed/red to signal that dropping will not
/// connect. Used both by [`GhostWire::paint`] and by the view's frame
/// snapshot.
pub(crate) fn paint_ghost(
	window: &mut Window,
	from: Point<Pixels>,
	to: Point<Pixels>,
	color: Hsla,
	target_valid: bool,
	zoom: f32,
) {
	if target_valid {
		if let Some(path) = wire_path(from, to, zoom, px(2.5), None) {
			window.paint_path(path, color);
		}
	} else if let Some(path) = wire_path(from, to, zoom, px(2.0), Some(&[px(6.0), px(4.0)])) {
		window.paint_path(path, hsla(0.0, 0.85, 0.55, 1.0));
	}
}

/// Pixels per second the [`FlowAnimation`] dash phase advances while active.
pub const FLOW_SPEED: f32 = 60.0;

/// Optional signal-flow animation hook.
///
/// A subtle animated dash offset travelling along each wire from output to
/// input while playback is running, to visualize which connections are
/// "live". The view calls [`FlowAnimation::advance`] each frame during
/// playback and passes the resulting offset to the wire stroke's dash phase.
///
/// The phase advances at [`FLOW_SPEED`] pixels per second; wires fall back to
/// their static style while inactive.
#[derive(Clone, Debug, Default)]
pub struct FlowAnimation {
	/// Current dash phase in pixels, monotonically increasing while active.
	phase: Pixels,
	/// Whether the animation is currently running (e.g. during playback).
	active: bool,
}

impl FlowAnimation {
	/// Starts the flow animation (e.g. when playback begins), resetting the
	/// phase to zero.
	pub fn start(&mut self) {
		self.active = true;
		self.phase = px(0.0);
	}

	/// Stops the flow animation; wires fall back to their static style.
	pub fn stop(&mut self) {
		self.active = false;
	}

	/// Advances the phase by one frame. `dt` is the elapsed frame time in
	/// seconds; flow speed is a fixed px/s constant. No-op while inactive.
	pub fn advance(&mut self, dt: f32) {
		if self.active {
			self.phase += px(FLOW_SPEED * dt);
		}
	}

	/// Returns the current dash phase to apply to wire strokes, or `None`
	/// while inactive.
	pub fn phase(&self) -> Option<Pixels> {
		self.active.then_some(self.phase)
	}
}

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

//! The node editor panel (节点编辑器): the real `gpui_kit::node_graph` canvas over
//! the engine's graph (the mock's demo graph, or the real engine's current
//! sequence graph), with the design's zoom controls (+ / − / 适配).
//!
//! The graph is a full [`NodeGraphView`] fed by the engine's
//! [`NodeGraphDataSource`] implementation. Every gesture the view emits
//! (move, connect, disconnect, delete, selection) is forwarded to the engine
//! as a request; the engine applies it to its model and notifies, so the
//! view re-reads on the next frame. The toolbar buttons drive the viewport
//! directly: zoom in/out at the canvas center, or fit the whole graph.

use std::collections::BTreeSet;

use crate::oakui::component::menu::{Menu, MenuItem};
use crate::panels::PanelFocused;
use crate::widgets::node_graph::{
	EdgeData, EdgeId, MAX_ZOOM, MIN_ZOOM, NodeData, NodeElement, NodeGraphEvent, NodeGraphView,
	NodeId, NodeVisualState,
};
use gpui_kit::colors::DefaultColors;
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::{
	App, Bounds, ClickEvent, Context, Entity, EventEmitter, MouseButton, Pixels, Point, Render,
	Window, div, point, prelude::*, px,
};
use gpui_kit::{FocusHandle, Focusable};

use crate::oakui::component::menu;
use crate::oakui::component::menu::{ContextMenuHandle, ContextMenuTriggered};
use crate::oakui::{AppEngine, NodeLibraryEntry};
use crate::panels::commands::PanelCommandHandler;

/// The node editor panel.
pub struct NodeEditorPanel<E: AppEngine> {
	/// The panel's focus handle (the dock's zoom/focus and the
	/// shell's focused-panel routing need it).
	focus_handle: FocusHandle,
	/// The node-graph canvas over the engine's graph data.
	graph: Entity<NodeGraphView<E>>,
	engine: Entity<E>,
	/// Whether the initial fit-to-window has been applied (the canvas size is
	/// only known after the first layout).
	fitted: bool,
	/// The right-click context menu.
	context_menu: ContextMenuHandle,
	/// Window position of the last right-click: `BackgroundClicked` only
	/// carries a graph-space position, so the background menu is placed at
	/// the recorded pointer position (the right-click bubbles up to the
	/// panel).
	last_right_click: Option<Point<Pixels>>,
	/// The graph-space position of the last background click — the spot a
	/// node added through the Add menu lands on.
	add_node_position: Option<Point<Pixels>>,
	/// The Add-menu item ids currently on offer, mapped to their factory
	/// type ids (rebuilt whenever the menu opens).
	add_menu_ids: Vec<(usize, String)>,
	/// The engine's node-graph selection mirror the widget was last synced
	/// to (see [`Self::sync_graph_selection`]): the panel only pushes into
	/// the widget when the engine's authoritative selection changes, so a
	/// marquee or click selection inside the graph is never overwritten.
	last_graph_selection: Option<u64>,
	/// Node identities in the last fitted graph. Opening a project or changing
	/// the selected clip can replace the whole canvas after the initial fit.
	last_graph_nodes: BTreeSet<u64>,
}

impl<E: AppEngine> NodeEditorPanel<E> {
	/// Builds the graph canvas over `engine` and routes its edit requests back
	/// to the engine.
	///
	/// `engine` is the shared engine entity (borrowed, not owned); `window`
	/// registers the context menu and builds the graph widget; `cx` supplies
	/// the panel's focus handle. The constructor wires the "edits are
	/// requests" loop: every graph gesture is forwarded through
	/// `apply_node_graph_event` — except delete requests naming a protected
	/// endpoint, which are filtered first — graph context-menu events open
	/// the panel's menus, and engine notifies re-sync the highlight. A clip or
	/// node already selected when the panel is built is pushed into the graph
	/// immediately. All subscriptions are detached, so they live as long as
	/// the entity.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let graph = cx.new(|cx| NodeGraphView::new(engine.clone(), window, cx));
		// The "edits are requests" loop: every graph gesture goes to the
		// engine, which applies it to its model and notifies. The one
		// exception is the fixed endpoint pair, which the panel filters out
		// of delete requests before the engine can see them.
		cx.subscribe(&graph, |this, _graph, event: &NodeGraphEvent, cx| {
			let Some(event) = this.filter_delete_request(event, cx) else {
				return;
			};
			this.engine
				.update(cx, |engine, cx| engine.apply_node_graph_event(&event, cx));
		})
		.detach();
		// The panel-side half of the graph events: the context menus.
		cx.subscribe(
			&graph,
			|this, _graph, event: &NodeGraphEvent, cx| match event {
				NodeGraphEvent::BackgroundClicked { position } => {
					this.add_node_position = Some(*position);
					if let Some(window_position) = this.last_right_click {
						let menu = background_menu(
							this.engine.read(cx).node_library(),
							&mut this.add_menu_ids,
						);
						this.context_menu.show(window_position, menu, cx);
					}
				}
				NodeGraphEvent::NodeContextMenuRequested { node, position } => {
					let protected = this.engine.read(cx).protected_graph_nodes().contains(node);
					this.context_menu.show(*position, node_menu(protected), cx);
				}
				_ => {}
			},
		)
		.detach();

		// The engine's selection mirror is the single source of truth for
		// what the graph highlights: a timeline clip selection (req: the
		// selected clip's block node) and an inspector card click both land
		// in `selected_graph_node`, and this pushes it into the widget.
		cx.observe(&engine, |this, _engine, cx| {
			this.sync_graph_state(cx);
		})
		.detach();

		let context_menu = ContextMenuHandle::new(Self::on_local_menu_item, window, cx);

		let mut panel = Self {
			graph,
			focus_handle: cx.focus_handle(),
			engine,
			fitted: false,
			context_menu,
			last_right_click: None,
			add_node_position: None,
			add_menu_ids: Vec::new(),
			last_graph_selection: None,
			last_graph_nodes: BTreeSet::new(),
		};
		// If a clip (or graph node) is already selected when the panel is
		// built, push the highlight immediately (the observe only fires on
		// the next engine notify).
		panel.sync_graph_state(cx);
		panel
	}

	/// The UI-layer guard for the fixed endpoint pair: a graph gesture that
	/// names a protected node (the `GraphInput` / `GraphOutput` cards the
	/// engine reports) never reaches the engine's model. Only delete requests
	/// can name nodes today, so every other event passes through untouched; a
	/// delete request that names an endpoint is narrowed to the rest of the
	/// selection — the endpoints stay, and so do the wires hanging off them.
	/// A request left with nothing to delete is dropped whole (`None`).
	fn filter_delete_request(&self, event: &NodeGraphEvent, cx: &App) -> Option<NodeGraphEvent> {
		let NodeGraphEvent::DeleteRequested { nodes, edges } = event else {
			return Some(event.clone());
		};
		let protected = self.engine.read(cx).protected_graph_nodes();
		if protected.is_empty() || !nodes.iter().any(|node| protected.contains(node)) {
			return Some(event.clone());
		}
		let kept_nodes: Vec<NodeId> = nodes
			.iter()
			.filter(|node| !protected.contains(node))
			.copied()
			.collect();
		// The widget packs every edge incident to a deleted node into the
		// request; an edge that touches a surviving endpoint would be
		// stranded, so it stays with its node.
		let stranded: Vec<EdgeId> = self
			.engine
			.read(cx)
			.edges()
			.iter()
			.filter(|edge| {
				protected.contains(&edge.from_node()) || protected.contains(&edge.to_node())
			})
			.map(|edge| edge.id())
			.collect();
		let kept_edges: Vec<EdgeId> = edges
			.iter()
			.filter(|edge| !stranded.contains(edge))
			.copied()
			.collect();
		if kept_nodes.is_empty() && kept_edges.is_empty() {
			return None;
		}
		Some(NodeGraphEvent::DeleteRequested {
			nodes: kept_nodes,
			edges: kept_edges,
		})
	}

	/// Pushes the engine's selection mirror into the graph widget: the
	/// single selected node becomes the widget selection, so a timeline
	/// clip selection highlights that clip's block node and an inspector
	/// card click highlights the effect's node. `None` is never pushed —
	/// the widget keeps its live selection (e.g. a marquee) until the
	/// engine names a new authoritative node.
	fn sync_graph_state(&mut self, cx: &mut Context<Self>) {
		let nodes = self.engine.read(cx).nodes();
		let identities: BTreeSet<u64> = nodes.into_iter().map(|node| node.id().0).collect();
		if identities != self.last_graph_nodes {
			self.last_graph_nodes = identities;
			self.fitted = false;
		}
		let node = self.engine.read(cx).selected_graph_node();
		if node == self.last_graph_selection {
			return;
		}
		self.last_graph_selection = node;
		let Some(node) = node else {
			return;
		};
		self.graph.update(cx, |graph, cx| {
			graph.set_selection(BTreeSet::from([NodeId(node)]), cx)
		});
	}

	/// Handles the node editor's local (non-registry) context-menu items.
	///
	/// `item` is the numeric id the menu emitted on `cx`. Color-label entries
	/// (see `menu::color_label_index`) are currently only logged — the panel
	/// keeps no per-node label state yet. Ids at or above
	/// `LOCAL_ADD_NODE_BASE` are Add-menu entries: the type id is looked up in
	/// `add_menu_ids` and the node is inserted at the last background-click
	/// position through the engine, logging instead of panicking when the
	/// engine rejects it. Unknown ids are logged and ignored.
	fn on_local_menu_item(&mut self, item: usize, cx: &mut Context<Self>) {
		if let Some(color) = menu::color_label_index(item) {
			println!("[node editor] set node color label to {color}");
			return;
		}
		if item >= LOCAL_ADD_NODE_BASE {
			let type_id = self
				.add_menu_ids
				.iter()
				.find(|entry| entry.0 == item)
				.map(|entry| entry.1.clone());
			if let Some(type_id) = type_id {
				let position = self.add_node_position.unwrap_or_default();
				if let Err(err) = self
					.engine
					.update(cx, |engine, cx| engine.add_node_at(&type_id, position, cx))
				{
					println!("[node editor] add node failed: {err}");
				}
			}
			return;
		}
		println!("[node editor] context-menu item {item} (not implemented yet)");
	}

	/// The union of every node's bounds in graph space, if the graph is
	/// non-empty.
	fn graph_bounds(&self, cx: &App) -> Option<Bounds<Pixels>> {
		let nodes = self.engine.read(cx).nodes();
		if nodes.is_empty() {
			return None;
		}
		let mut min_x = f32::MAX;
		let mut min_y = f32::MAX;
		let mut max_x = f32::MIN;
		let mut max_y = f32::MIN;
		for node in &nodes {
			let element = NodeElement::from_node(node, NodeVisualState::default());
			let position = node.position();
			let width = crate::widgets::node_graph::DEFAULT_NODE_WIDTH;
			let height = element.height();
			let (x, y) = (f32::from(position.x), f32::from(position.y));
			min_x = min_x.min(x);
			min_y = min_y.min(y);
			max_x = max_x.max(x + f32::from(width));
			max_y = max_y.max(y + f32::from(height));
		}
		Some(Bounds::from_corners(
			point(px(min_x), px(min_y)),
			point(px(max_x), px(max_y)),
		))
	}

	/// The canvas size to fit against: the graph view's own painted size once
	/// known, otherwise the window (before the first layout).
	fn fit_viewport(&self, window: &Window, cx: &App) -> gpui_kit::Size<Pixels> {
		let viewport = self.graph.read(cx).viewport_size();
		if viewport.width > px(0.0) && viewport.height > px(0.0) {
			viewport
		} else {
			window.viewport_size()
		}
	}

	/// Fits the whole graph into the canvas (the 适配 button).
	///
	/// The fitted rectangle is the union of every node's position and its
	/// default element size, so it is the visible extent of the graph, not
	/// just the node origins. `window` supplies the fallback viewport before
	/// the canvas has been laid out (see [`Self::fit_viewport`]); `cx` is
	/// used to read the engine's node list and to request a repaint after
	/// the zoom/pan state changes. An empty graph returns without touching
	/// the transform.
	fn fit_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let Some(rect) = self.graph_bounds(cx) else {
			return;
		};
		let viewport = self.fit_viewport(window, cx);
		self.graph.update(cx, |graph, cx| {
			graph.state_mut().fit_to_rect(rect, viewport);
			cx.notify();
		});
	}

	/// Zooms the canvas by `factor` at its center (`+` / `−` buttons).
	fn zoom(&mut self, factor: f32, window: &mut Window, cx: &mut Context<Self>) {
		let viewport = self.fit_viewport(window, cx);
		let anchor = point(viewport.width * 0.5, viewport.height * 0.5);
		self.graph.update(cx, |graph, cx| {
			graph.state_mut().zoom_at(anchor, factor);
			cx.notify();
		});
	}
}

/// The node editor implements no focused-panel commands: everything falls
/// through to the shell's global handler.
impl<E: AppEngine> PanelCommandHandler for NodeEditorPanel<E> {}

impl<E: AppEngine> Render for NodeEditorPanel<E> {
	fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		// Fit the graph once the canvas size is known (first layout). Before
		// that the viewport is zero-sized, so ask for another frame instead.
		if !self.fitted {
			if self.graph.read(cx).viewport_size() != Default::default() {
				self.fitted = true;
				self.fit_graph(window, cx);
			} else {
				cx.notify();
			}
		}

		let colors = cx.default_colors().clone();
		div()
			.size_full()
			.flex()
			.flex_col()
			// Any click inside the panel makes it the focused panel (the
			// dock re-emits this as `DockEvent::PanelFocused`, which the
			// shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &gpui_kit::MouseDownEvent, _window, cx| {
					cx.emit(PanelFocused);
				})
			})
			// The graph's `BackgroundClicked` only carries a graph-space
			// position; record the pointer here (right-clicks bubble up) so
			// the background menu can open at the window position.
			.on_mouse_down(MouseButton::Right, {
				cx.listener(|this, event: &gpui_kit::MouseDownEvent, _window, _cx| {
					this.last_right_click = Some(event.position);
				})
			})
			.child(
				div()
					.flex()
					.items_center()
					.gap_1()
					.px_2()
					.py_1()
					.border_b_1()
					.border_color(colors.border)
					.child(zoom_button(
						cx,
						"node-zoom-in",
						Some(crate::oakui::icons::ICON_ZOOM_IN),
						"+",
						"timeline.zoom_in",
						|this, window, cx| {
							this.zoom(1.25, window, cx);
						},
					))
					.child(zoom_button(
						cx,
						"node-zoom-out",
						Some(crate::oakui::icons::ICON_ZOOM_OUT),
						"−",
						"timeline.zoom_out",
						|this, window, cx| {
							this.zoom(1.0 / 1.25, window, cx);
						},
					))
					.child(zoom_button(
						cx,
						"node-zoom-fit",
						None,
						crate::i18n::tr("node.fit"),
						"node.fit",
						|this, window, cx| this.fit_graph(window, cx),
					))
					.child(div().flex_1())
					.child(div().text_color(colors.disabled).child(format!(
						"{}% · {}–{}",
						(self.graph.read(cx).state().zoom() * 100.0).round(),
						MIN_ZOOM,
						MAX_ZOOM,
					))),
			)
			.child(
				div()
					.id("node-editor-canvas")
					.debug_selector(|| "node-editor-canvas".into())
					.flex_1()
					.min_h_0()
					// An effect dragged out of the effect library drops onto
					// the canvas as a new node at the drop position (the
					// background "add node" menu path).
					.on_drop::<crate::widgets::effect_stack::LibraryEffectDrag>(cx.listener(
						|this,
						 payload: &crate::widgets::effect_stack::LibraryEffectDrag,
						 window,
						 cx| {
							let graph_position = this
								.graph
								.read(cx)
								.graph_position_at(window.mouse_position());
							let type_id = payload.type_id.to_string();
							this.engine.update(cx, |engine, cx| {
								if let Err(err) = engine.add_node_at(&type_id, graph_position, cx) {
									println!("[node editor] add node failed: {err}");
								}
							});
						},
					))
					.can_drop(|payload, _window, _cx| {
						payload.is::<crate::widgets::effect_stack::LibraryEffectDrag>()
					})
					.child(self.graph.clone()),
			)
			// The right-click popup renders anchored above the panel.
			.child(self.context_menu.widget())
	}
}

/// A small toolbar button driving the graph viewport. With `icon_name`, the
/// button shows the 16px icon on a 24px hit target; otherwise the `label`
/// text. Both get a localized `tooltip`.
fn zoom_button<E: AppEngine>(
	cx: &mut Context<NodeEditorPanel<E>>,
	id: &'static str,
	icon_name: Option<&'static str>,
	label: impl IntoElement,
	tooltip: &'static str,
	action: impl Fn(&mut NodeEditorPanel<E>, &mut Window, &mut Context<NodeEditorPanel<E>>) + 'static,
) -> impl gpui_kit::IntoElement {
	let colors = cx.default_colors().clone();
	let container = colors.container;
	let tooltip_label = crate::i18n::tr(tooltip);
	let mut el = div()
		.id(id)
		.size(px(24.0))
		.flex()
		.items_center()
		.justify_center()
		.rounded_md()
		.border_1()
		.border_color(colors.border)
		.text_color(colors.text)
		.cursor_pointer()
		.hover(move |style| style.bg(container))
		.tooltip(move |window, cx| {
			crate::widgets::tooltip::tooltip_view(tooltip_label.into(), window, cx)
		})
		.on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
			action(this, window, cx);
		}));
	if let Some(name) = icon_name {
		el = el.child(gpui_kit::img(crate::oakui::icons::icon_path(name, cx)).size(px(16.0)));
	} else {
		el = el.child(label);
	}
	el
}

impl<E: AppEngine> EventEmitter<PanelEvent> for NodeEditorPanel<E> {}

impl<E: AppEngine> NodeEditorPanel<E> {
	/// Test access to the graph canvas and engine (dock-level regression
	/// tests live in the shell's test module).
	#[cfg(test)]
	pub(crate) fn test_handles(&self) -> (Entity<NodeGraphView<E>>, Entity<E>) {
		(self.graph.clone(), self.engine.clone())
	}
}

impl<E: AppEngine> EventEmitter<ContextMenuTriggered> for NodeEditorPanel<E> {}

impl<E: AppEngine> EventEmitter<PanelFocused> for NodeEditorPanel<E> {}

impl<E: AppEngine> Focusable for NodeEditorPanel<E> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<E: AppEngine> BasePanel for NodeEditorPanel<E> {
	fn panel_name(&self) -> &'static str {
		crate::panels::ids::NODE_EDITOR
	}
}

impl<E: AppEngine> Panel for NodeEditorPanel<E> {
	fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		crate::i18n::tr("panel.node_editor")
	}
}

// ---------------------------------------------------------------------------
// Context menus — the Rust counterpart of the C++ `NodeView::
// show_context_menu` (`app/widget/nodeview/nodeview.cpp`).
// ---------------------------------------------------------------------------

/// Local (non-registry) item ids of the node editor's context menus.
const LOCAL_SMOOTH_EDGES: usize = 2401;
const LOCAL_DIR_TOP_BOTTOM: usize = 2402;
const LOCAL_DIR_BOTTOM_TOP: usize = 2403;
const LOCAL_DIR_LEFT_RIGHT: usize = 2404;
const LOCAL_DIR_RIGHT_LEFT: usize = 2405;
const LOCAL_GROUP: usize = 2406;
const LOCAL_UNGROUP: usize = 2407;
const LOCAL_OPEN_IN_VIEWER: usize = 2408;
const LOCAL_SHOW_IN_PARAM_EDITOR: usize = 2409;
const LOCAL_NODE_PROPERTIES: usize = 2410;
/// The Add-menu items occupy `LOCAL_ADD_NODE_BASE..` (one id per library
/// entry; the panel maps them back to factory type ids).
const LOCAL_ADD_NODE_BASE: usize = 2420;

/// The node context menu: the shared edit section, grouping, color labels,
/// viewer/parameter-editor reveals and properties (the C++ node branch).
/// With `protected` (the fixed endpoint pair) the whole edit section is left
/// out: the endpoints cannot be cut, copied, pasted over, duplicated,
/// renamed or deleted, so none of those entries are offered.
pub(crate) fn node_menu(protected: bool) -> Menu {
	use crate::i18n::tr;
	let mut items = Vec::new();
	if !protected {
		items = menu::edit_section(false);
		if let Some(last) = items.last_mut() {
			last.separator_after = true;
		}
	}
	items.push(MenuItem::new(LOCAL_GROUP, tr("node.context.group")));
	items.push(MenuItem::new(LOCAL_UNGROUP, tr("node.context.ungroup")));
	items.push(menu::color_label_item(None).separated());
	items.push(MenuItem::new(
		LOCAL_OPEN_IN_VIEWER,
		tr("node.context.open_in_viewer"),
	));
	items.push(MenuItem::new(
		LOCAL_SHOW_IN_PARAM_EDITOR,
		tr("node.context.show_in_param_editor"),
	));
	items.push(MenuItem::new(
		LOCAL_NODE_PROPERTIES,
		tr("menu.context.properties"),
	));
	Menu::new(items)
}

/// The background context menu: edge smoothing, flow direction and the Add
/// submenu built from the engine's node library (grouped by category,
/// alphabetical inside each group — the C++ `create_add_menu` order).
/// `add_menu_ids` is rewritten to map the fresh item ids to type ids.
pub(crate) fn background_menu(
	library: Vec<NodeLibraryEntry>,
	add_menu_ids: &mut Vec<(usize, String)>,
) -> Menu {
	use crate::i18n::tr;
	add_menu_ids.clear();

	// Group the library by category key (BTreeMap = alphabetical category
	// order), then sort each group's entries by name.
	let mut groups: std::collections::BTreeMap<&'static str, Vec<NodeLibraryEntry>> =
		std::collections::BTreeMap::new();
	for entry in library {
		groups.entry(entry.category_key).or_default().push(entry);
	}
	let mut add_items: Vec<MenuItem> = Vec::new();
	let mut next_id = LOCAL_ADD_NODE_BASE;
	for (category_key, mut entries) in groups {
		entries.sort_by_key(|a| a.name.to_lowercase());
		let mut submenu = Vec::with_capacity(entries.len());
		for entry in entries {
			submenu.push(MenuItem::new(next_id, entry.name.clone()));
			add_menu_ids.push((next_id, entry.type_id));
			next_id += 1;
		}
		add_items.push(MenuItem::new(0, tr(category_key)).with_submenu(Menu::new(submenu)));
	}

	let direction_menu = Menu::new(vec![
		MenuItem::new(LOCAL_DIR_TOP_BOTTOM, tr("node.context.dir_top_bottom")).with_checked(true),
		MenuItem::new(LOCAL_DIR_BOTTOM_TOP, tr("node.context.dir_bottom_top")).with_checked(false),
		MenuItem::new(LOCAL_DIR_LEFT_RIGHT, tr("node.context.dir_left_right")).with_checked(false),
		MenuItem::new(LOCAL_DIR_RIGHT_LEFT, tr("node.context.dir_right_left")).with_checked(false),
	]);

	Menu::new(vec![
		MenuItem::new(LOCAL_SMOOTH_EDGES, tr("node.context.smooth_edges"))
			.with_checked(false)
			.separated(),
		MenuItem::new(0, tr("node.context.direction"))
			.with_submenu(direction_menu)
			.separated(),
		MenuItem::new(0, tr("node.context.add")).with_submenu(Menu::new(add_items)),
	])
}

#[cfg(test)]
mod tests;

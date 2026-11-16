use super::*;
use crate::actions::ActionId;
use crate::oakui::MockEngine;
use crate::oakui::nodegraph::GraphEndpoint;
use crate::widgets::effect_stack::{EffectId, EffectStackEvent};
use crate::widgets::node_graph::{NodeGraphDataSource, PortData, PortId};
use gpui_kit::{TestAppContext, VisualTestContext, size};

/// Builds the panel in a window and returns a `VisualTestContext` for
/// bounds assertions.
fn panel_window(
	cx: &mut TestAppContext,
) -> (
	&'static mut VisualTestContext,
	Entity<NodeEditorPanel<MockEngine>>,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(640.0), px(480.0)), |window, cx| {
		let engine = cx.new(crate::oakui::MockEngine::demo);
		NodeEditorPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	// The graph canvas reports its size only after the first paint, so the
	// initial fit applies on the following frame: draw a few more.
	for _ in 0..3 {
		cx.update_window(window.into(), |_root, window, app| {
			let _ = window.draw(app);
		})
		.expect("window still open");
		cx.run_until_parked();
	}
	let panel = window.root(cx).expect("node editor panel root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	(cx, panel)
}

/// The panel lays out a graph canvas below the zoom toolbar, and the
/// initial fit centers the graph so every demo node is on screen.
/// Regression: dragging a node moves it by exactly the pointer
/// displacement in graph units — the first mouse-move must not
/// teleport the card (a canvas origin change between press and move
/// would add the whole panel offset to the drag delta).
#[gpui_kit::test]
async fn node_drag_moves_by_the_pointer_delta(cx: &mut TestAppContext) {
	use crate::widgets::node_graph::{NodeData as _, NodeElement, NodeVisualState};

	let (cx, panel) = panel_window(cx);
	let canvas = cx
		.debug_bounds("node-editor-canvas")
		.expect("graph canvas rendered");

	// Pick a node whose card center is covered by no other card (so the
	// press hits it on top), then aim at the card body center (clear of
	// the header toggles and the edge ports).
	let (engine, target_id, press, zoom) = cx.read(|app| {
		let panel = panel.read(app);
		let graph = panel.graph.read(app);
		let state = graph.state().clone();
		let zoom = state.zoom();
		let nodes = graph.data().read(app).nodes();
		let card = |n: &<MockEngine as NodeGraphDataSource>::Node| {
			let element = NodeElement::from_node(n, NodeVisualState::default());
			gpui_kit::Bounds::new(
				state.graph_to_screen(n.position()),
				gpui_kit::size(
					crate::widgets::node_graph::DEFAULT_NODE_WIDTH * zoom,
					element.height() * zoom,
				),
			)
		};
		let (node, bounds) = nodes
			.iter()
			.map(|n| (n, card(n)))
			.find(|(n, bounds)| {
				let center = bounds.center();
				nodes
					.iter()
					.filter(|other| other.id() != n.id())
					.all(|other| !card(other).contains(&center))
			})
			.expect("a demo node with an uncovered card center");
		(
			panel.engine.clone(),
			node.id(),
			canvas.origin + bounds.center(),
			zoom,
		)
	});

	let position_of = |app: &gpui_kit::App| {
		engine
			.read(app)
			.nodes()
			.into_iter()
			.find(|n| n.id() == target_id)
			.expect("the aimed node")
			.position()
	};
	let before = cx.read(|app| position_of(app));

	cx.simulate_mouse_down(
		press,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	let target = press + gpui_kit::point(gpui_kit::px(10.0), gpui_kit::px(6.0));
	cx.simulate_mouse_move(
		target,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	cx.simulate_mouse_up(
		target,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	cx.run_until_parked();

	let after = cx.read(|app| position_of(app));
	let dx = after.x.as_f32() - before.x.as_f32();
	let dy = after.y.as_f32() - before.y.as_f32();
	assert!(
		(dx - 10.0 / zoom).abs() < 1.0,
		"x tracks the pointer (dx {dx}, expected {})",
		10.0 / zoom
	);
	assert!(
		(dy - 6.0 / zoom).abs() < 1.0,
		"y tracks the pointer (dy {dy}, expected {})",
		6.0 / zoom
	);
}

/// The panel lays out a graph canvas below the zoom toolbar, and the
/// initial fit centers the graph so every demo node is on screen.
#[gpui_kit::test]
async fn canvas_fills_the_panel_below_the_toolbar(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);

	let canvas = cx
		.debug_bounds("node-editor-canvas")
		.expect("graph canvas rendered");
	assert!(canvas.size.width > px(0.0));
	assert!(canvas.size.height > px(0.0));

	// The initial fit moved the viewport off the default origin, so the
	// graph is framed rather than clipped at the corner.
	let state = cx.read(|app| panel.read(app).graph.read(app).state().clone());
	assert_ne!(
		state.offset(),
		crate::widgets::node_graph::GraphViewState::new().offset()
	);
	assert!(state.zoom() > 0.0);
}

/// The node menu wraps the plain edit section with grouping, color
/// labels, the reveal entries and properties.
#[test]
fn node_menu_carries_grouping_and_reveals() {
	// The color-label lookup below matches on a localized label: pin
	// en-US under the shared language lock.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let menu = node_menu(false);
	let ids: Vec<usize> = menu.items.iter().map(|item| item.id).collect();
	assert!(ids.contains(&LOCAL_GROUP));
	assert!(ids.contains(&LOCAL_UNGROUP));
	assert!(ids.contains(&LOCAL_OPEN_IN_VIEWER));
	assert!(ids.contains(&LOCAL_SHOW_IN_PARAM_EDITOR));
	assert!(ids.contains(&LOCAL_NODE_PROPERTIES));

	let color = menu
		.items
		.iter()
		.find(|item| item.label == crate::i18n::tr("menu.color.label"))
		.expect("color label item");
	assert!(color.separator_after);
}

fn entry(type_id: &str, name: &str, category_key: &'static str) -> NodeLibraryEntry {
	NodeLibraryEntry {
		type_id: type_id.to_string(),
		name: name.to_string(),
		category_key,
	}
}

/// The background menu groups the library alphabetically by category,
/// sorts entries inside each group, and records the id → type-id map
/// starting at `LOCAL_ADD_NODE_BASE`.
#[test]
fn background_menu_groups_the_library() {
	let library = vec![
		entry("video.solid", "Solid", "node.category.generator"),
		entry("math.add", "Add", "node.category.math"),
		entry("video.bars", "Color Bars", "node.category.generator"),
		entry("math.multiply", "Multiply", "node.category.math"),
	];
	let mut add_menu_ids = Vec::new();
	let menu = background_menu(library, &mut add_menu_ids);

	// Top level: smooth edges, direction, add.
	assert_eq!(menu.items.len(), 3);
	assert_eq!(menu.items[0].id, LOCAL_SMOOTH_EDGES);
	let add = &menu.items[2];
	assert_eq!(add.label, crate::i18n::tr("node.context.add"));

	// Categories sort alphabetically: generator before math, entries
	// sorted case-insensitively inside each group.
	let categories = add.submenu.as_ref().unwrap();
	let labels: Vec<_> = categories
		.items
		.iter()
		.map(|item| item.label.clone())
		.collect();
	assert_eq!(
		labels,
		vec![
			crate::i18n::tr("node.category.generator"),
			crate::i18n::tr("node.category.math"),
		]
	);
	let generator = &categories.items[0].submenu.as_ref().unwrap().items;
	let names: Vec<_> = generator.iter().map(|item| item.label.clone()).collect();
	assert_eq!(names, vec!["Color Bars", "Solid"]);

	// The id map starts at the base and matches submenu order.
	assert_eq!(add_menu_ids[0].0, LOCAL_ADD_NODE_BASE);
	let mapped: std::collections::HashMap<usize, String> = add_menu_ids.iter().cloned().collect();
	assert_eq!(
		mapped.get(&generator[0].id),
		Some(&"video.bars".to_string())
	);
	assert_eq!(mapped.len(), 4);
}

/// An empty library still yields the smoothing/direction entries, with
/// an empty Add submenu.
#[test]
fn background_menu_survives_an_empty_library() {
	let mut add_menu_ids = Vec::new();
	let menu = background_menu(Vec::new(), &mut add_menu_ids);
	assert_eq!(menu.items.len(), 3);
	assert!(add_menu_ids.is_empty());
	assert!(menu.items[2].submenu.as_ref().unwrap().items.is_empty());
}

/// The engine's selection mirror is pushed into the graph widget: a node
/// selection (a node click, or an inspector card click) lands in the
/// engine through the panel's event loop, and the panel's observe then
/// sets the widget selection — so the graph, the inspector and the
/// timeline share one highlight.
#[gpui_kit::test]
async fn engine_selection_syncs_into_the_graph_widget(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);

	// A node click round trip: the engine applies the SelectionChanged
	// (as the panel subscription would forward it), and the observe
	// pushes the mirrored selection into the widget.
	cx.update(|_window, app| {
		let engine = panel.read(app).engine.clone();
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::SelectionChanged {
					nodes: BTreeSet::from([NodeId(2)]),
				},
				cx,
			);
		});
	});
	cx.run_until_parked();

	let selection = cx.read(|app| panel.read(app).graph.read(app).state().selection().clone());
	assert!(
		selection.contains(&NodeId(2)),
		"the widget highlights the mirrored node (got {selection:?})"
	);

	// An inspector card click (the "变换" card) selects the same-named
	// node through the same mirror.
	cx.update(|_window, app| {
		let engine = panel.read(app).engine.clone();
		engine.update(app, |engine, cx| {
			engine.apply_effect_event(
				&EffectStackEvent::CardSelected {
					effect: EffectId(1),
				},
				cx,
			);
		});
	});
	cx.run_until_parked();

	let selection = cx.read(|app| panel.read(app).graph.read(app).state().selection().clone());
	assert!(
		selection.contains(&NodeId(2)),
		"the card click highlights the matching node (got {selection:?})"
	);
}

/// The demo graph carries the fixed endpoint pair as real graph data: two
/// endpoint-marked cards, the input one with the output port the walk
/// pulls from, the output one a pure sink. Both are reported as protected,
/// and the connection rules follow the ports — the input's `feed_in` takes
/// a wire, its output starts one, and the sink can never source a wire
/// (it has no output port to drag from).
#[gpui_kit::test]
async fn demo_endpoints_are_protected_and_marked(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);

	let protected = cx.read(|app| panel.read(app).engine.read(app).protected_graph_nodes());
	assert_eq!(protected.len(), 2, "the endpoint pair is protected");

	let nodes = cx.read(|app| panel.read(app).engine.read(app).nodes());
	assert_eq!(
		nodes
			.iter()
			.filter(|node| node.endpoint().is_some())
			.count(),
		2,
		"exactly the fixed pair is marked as endpoints"
	);
	let input = nodes
		.iter()
		.find(|node| node.endpoint() == Some(GraphEndpoint::Input))
		.expect("the demo graph has a graph-input endpoint");
	let output = nodes
		.iter()
		.find(|node| node.endpoint() == Some(GraphEndpoint::Output))
		.expect("the demo graph has a graph-output endpoint");
	assert!(protected.contains(&input.id()));
	assert!(protected.contains(&output.id()));
	assert_eq!((input.inputs().len(), input.outputs().len()), (1, 1));
	assert_eq!((output.inputs().len(), output.outputs().len()), (1, 0));

	let can_connect = |from: PortId, to: PortId| {
		cx.read(|app| panel.read(app).engine.read(app).can_connect(from, to))
	};
	let feed_in = input.inputs()[0].id();
	let tex_out = input.outputs()[0].id();
	let tex_in = output.inputs()[0].id();
	assert!(can_connect(PortId(42), feed_in), "feed_in accepts a wire");
	assert!(
		can_connect(tex_out, PortId(22)),
		"the input endpoint sources a wire"
	);
	assert!(
		!can_connect(tex_in, PortId(42)),
		"the sink has no output port, so no wire can leave it"
	);
}

/// The UI-layer guard over the endpoint pair: a delete request that names
/// an endpoint is narrowed by the panel before the engine sees it — the
/// endpoints (and the wires hanging off them) survive, the rest of the
/// request still lands, and a request left with nothing to delete is
/// dropped whole. The last step goes behind the panel to show the model
/// itself has no protection: the panel is the only thing standing between
/// a Delete/Backspace gesture and the pair.
#[gpui_kit::test]
async fn delete_requests_cannot_remove_endpoints(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);

	// The widget's Delete/Backspace path: the request is emitted by the
	// graph entity (a node selection plus the incident edges it packs),
	// exactly as the widget emits it.
	let emit_delete = |cx: &mut VisualTestContext, nodes: Vec<NodeId>, edges: Vec<EdgeId>| {
		let graph = cx.read(|app| panel.read(app).graph.clone());
		graph.update(cx, |_graph, cx| {
			cx.emit(NodeGraphEvent::DeleteRequested { nodes, edges })
		});
		cx.run_until_parked();
	};
	let counts = |cx: &VisualTestContext| {
		cx.read(|app| {
			let engine = panel.read(app).engine.read(app);
			(engine.nodes().len(), engine.edges().len())
		})
	};
	assert_eq!(counts(cx), (8, 6));

	// A mixed request: the ordinary node goes, the endpoint stays — and so
	// does the wire it carries, even though the request named it.
	emit_delete(cx, vec![NodeId(2), NodeId(6)], vec![EdgeId(6)]);
	assert_eq!(counts(cx), (7, 4));
	let input_survived = cx.read(|app| {
		panel
			.read(app)
			.engine
			.read(app)
			.nodes()
			.iter()
			.any(|node| node.endpoint() == Some(GraphEndpoint::Input))
	});
	assert!(input_survived, "the endpoint survives the mixed request");

	// A request naming only the pair is dropped whole: nothing changes.
	emit_delete(cx, vec![NodeId(6), NodeId(7)], Vec::new());
	assert_eq!(counts(cx), (7, 4));

	// A plain edge deletion names no node and passes through untouched.
	emit_delete(cx, Vec::new(), vec![EdgeId(4)]);
	assert_eq!(counts(cx), (7, 3));

	// Behind the panel, the mock model deletes an endpoint happily, so the
	// survival above is the panel's doing.
	cx.update(|_window, app| {
		let engine = panel.read(app).engine.clone();
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::DeleteRequested {
					nodes: vec![NodeId(6)],
					edges: Vec::new(),
				},
				cx,
			);
		});
	});
	cx.run_until_parked();
	assert_eq!(counts(cx), (6, 2));
}

/// The endpoint cards get the protected node menu: the whole edit section
/// (undo/redo, cut/copy/paste, duplicate, rename, delete) is left out,
/// while the grouping, reveal and properties entries stay. The ordinary
/// node menu keeps its edit entries.
#[test]
fn protected_node_menu_has_no_edit_section() {
	let ids = |menu: &Menu| -> Vec<usize> { menu.items.iter().map(|item| item.id).collect() };

	let protected = node_menu(true);
	let protected_ids = ids(&protected);
	let edit_items = [
		ActionId::Undo,
		ActionId::Redo,
		ActionId::Cut,
		ActionId::Copy,
		ActionId::Paste,
		ActionId::PasteInsert,
		ActionId::Duplicate,
		ActionId::Rename,
		ActionId::Delete,
	];
	for action in edit_items {
		assert!(
			!protected_ids.contains(&action.menu_id()),
			"the endpoint menu hides {action:?}"
		);
	}
	assert!(protected_ids.contains(&LOCAL_GROUP));
	assert!(protected_ids.contains(&LOCAL_UNGROUP));
	assert!(protected_ids.contains(&LOCAL_OPEN_IN_VIEWER));
	assert!(protected_ids.contains(&LOCAL_SHOW_IN_PARAM_EDITOR));
	assert!(protected_ids.contains(&LOCAL_NODE_PROPERTIES));

	let ordinary_ids = ids(&node_menu(false));
	for action in edit_items {
		assert!(
			ordinary_ids.contains(&action.menu_id()),
			"an ordinary node menu offers {action:?}"
		);
	}
}

/// Local menu routing (color labels, add-node entries, unknown ids),
/// the fit/zoom viewport helpers and the dock metadata.
#[gpui_kit::test]
async fn menu_fit_zoom_and_dock_paths(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);

	cx.update(|window, app| {
		panel.update(app, |panel, cx| {
			// A color-label id takes the early return.
			let color_item = (0..4096)
				.find(|item| menu::color_label_index(*item).is_some())
				.expect("a color label id exists");
			panel.on_local_menu_item(color_item, cx);

			// An add-node entry (the mock engine rejects the add) and an
			// unknown id both return without panicking.
			panel
				.add_menu_ids
				.push((LOCAL_ADD_NODE_BASE + 41, "oak:test-node".to_string()));
			panel.on_local_menu_item(LOCAL_ADD_NODE_BASE + 41, cx);
			panel.on_local_menu_item(LOCAL_ADD_NODE_BASE + 42, cx);
			panel.on_local_menu_item(0, cx);

			// Fit/zoom/viewport helpers.
			assert!(panel.graph_bounds(cx).is_some());
			let _ = panel.fit_viewport(window, cx);
			panel.fit_graph(window, cx);
			panel.zoom(1.25, window, cx);
			panel.zoom(0.5, window, cx);
		});
	});

	cx.update(|_window, app| {
		assert_eq!(
			panel.read(app).panel_name(),
			crate::panels::ids::NODE_EDITOR
		);
	});
}

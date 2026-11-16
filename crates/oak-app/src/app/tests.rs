use super::*;
use crate::oakui::EngineGateway as _;
use crate::widgets::timeline::TimelineDataSource as _;
use gpui_kit::component::dock::BasePanel as _;
use gpui_kit::{
	AnyWindowHandle, ExternalPaths, FileDropEvent, TestAppContext, VisualTestContext, px, size,
};

/// The 视图/View menu carries a 语言/Language submenu whose items are
/// labeled in their own language and whose checkmark follows the active
/// language — and the whole menu bar flips language with `i18n`.
#[test]
fn language_menu_tracks_the_active_language() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let view_entry = |dark: bool| -> MenuBarEntry {
		make_menus(MenuState::new(dark))
			.into_iter()
			.find(|entry| entry.title == crate::i18n::tr("menu.view"))
			.expect("视图/View menu exists")
	};

	// The language submenu sits under 视图/View.
	let language_item = |entry: &MenuBarEntry| -> Menu {
		entry
			.menu
			.items
			.iter()
			.find(|item| item.label == crate::i18n::tr("menu.view.language"))
			.map(|item| item.submenu.clone().map(|m| *m).unwrap_or_default())
			.expect("语言/Language submenu exists")
	};

	crate::i18n::set_language_code("en-US");
	let languages = crate::i18n::available_languages();
	let en_index = languages.iter().position(|c| c == "en-US").unwrap();
	let zh_index = languages.iter().position(|c| c == "zh-CN").unwrap();
	let submenu = language_item(&view_entry(true));
	assert_eq!(
		submenu.items.len(),
		languages.len(),
		"one menu item per discovered language pack"
	);
	let zh = submenu
		.items
		.iter()
		.find(|i| i.id == menu::LANG_ITEM_BASE + zh_index)
		.expect("zh item");
	let en = submenu
		.items
		.iter()
		.find(|i| i.id == menu::LANG_ITEM_BASE + en_index)
		.expect("en item");
	assert_eq!(zh.label, "简体中文 (zh-CN)");
	assert_eq!(en.label, "English (en-US)");
	assert_eq!(zh.checked, Some(false));
	assert_eq!(en.checked, Some(true), "en-US is active → checked");

	crate::i18n::set_language_code("zh-CN");
	let submenu = language_item(&view_entry(true));
	let zh = submenu
		.items
		.iter()
		.find(|i| i.id == menu::LANG_ITEM_BASE + zh_index)
		.expect("zh item");
	let en = submenu
		.items
		.iter()
		.find(|i| i.id == menu::LANG_ITEM_BASE + en_index)
		.expect("en item");
	assert_eq!(zh.checked, Some(true), "zh-CN is active → checked");
	assert_eq!(en.checked, Some(false));

	// The menu titles themselves are localized.
	assert_eq!(view_entry(true).title, "视图(V)");
	crate::i18n::set_language_code("en-US");
	assert_eq!(view_entry(true).title, "View(V)");
}

/// The theme submenu's checkmark follows the `dark` flag.
#[test]
fn theme_menu_checkmark_follows_dark_flag() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let dark_item = |dark: bool| -> menu::MenuItem {
		let entries = make_menus(MenuState::new(dark));
		let view = entries
			.iter()
			.find(|entry| entry.title == crate::i18n::tr("menu.view"))
			.expect("视图/View menu");
		let theme = view
			.menu
			.items
			.iter()
			.find(|i| i.id == menu_ids::THEME_DARK)
			.and_then(|i| i.submenu.clone())
			.expect("theme submenu");
		theme
			.items
			.into_iter()
			.find(|i| i.id == menu_ids::THEME_DARK)
			.expect("Olive Dark item")
	};
	assert_eq!(dark_item(true).checked, Some(true));
	assert_eq!(dark_item(false).checked, Some(false));
}

/// The 窗口 menu lists every dockable panel, and each item's checkmark
/// mirrors the `open_panels` bitmask carried in [`MenuState`] (which the
/// shell reads from the dock's live visible-panel set).
#[test]
fn window_menu_lists_all_panels_and_checks_the_open_ones() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	let mask = panel_bit_for(INSPECTOR) | panel_bit_for(TIMELINE);
	let mut state = MenuState::new(true);
	state.open_panels = mask;
	let window = make_menus(state)
		.into_iter()
		.find(|entry| entry.title == crate::i18n::tr("menu.window"))
		.expect("Window menu exists")
		.menu;

	for (panel, action) in WINDOW_PANELS {
		let item = window
			.items
			.iter()
			.find(|item| item.id == action.menu_id())
			.unwrap_or_else(|| panic!("Window menu is missing panel {}", panel));
		assert_eq!(
			item.checked,
			Some(mask & panel_bit_for(panel) != 0),
			"panel {} checkmark follows open_panels",
			panel
		);
	}
}

/// Every docked panel carries its gpui-component presentation handle:
/// the dock skin recovers the localized tab title, the toolbar buttons
/// and the zoom affordance through it — a panel added as a bare entity
/// (`add_panel`) renders its raw `panel_name` in the tab and loses the
/// trailing controls.
#[gpui_kit::test]
async fn docked_panels_carry_their_presentation_handle(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);
	cx.read(|app| {
		let root = root.read(app);
		let dock = root.dock.read(app);
		for id in [
			PROJECT,
			SOURCE_VIEWER,
			PROGRAM_VIEWER,
			NODE_EDITOR,
			INSPECTOR,
			HISTORY,
			EFFECT_LIBRARY,
			MULTICAM,
		] {
			let entity = root
				.panels
				.entity_id(id)
				.unwrap_or_else(|| panic!("{id} has a panel entity"));
			let view = dock
				.panel(DockPanelId::from(entity))
				.unwrap_or_else(|| panic!("{id} is docked by the default layout"));
			assert!(
				gpui_kit::component::dock::PanelHandle::of(view).is_some(),
				"{id} must be added through `panel_handle` so the tab draws its title"
			);
		}
	});
}

/// Regression: a node drag inside the docked node editor moves the node
/// by exactly the pointer displacement (in graph units). A canvas-origin
/// change between the press and the first move (e.g. a dock relayout on
/// focus) would add the whole panel offset to the drag delta and
/// teleport the card.
#[gpui_kit::test]
async fn docked_node_drag_moves_by_the_pointer_delta(cx: &mut TestAppContext) {
	use crate::widgets::node_graph::{
		NodeData as _, NodeElement, NodeGraphDataSource as _, NodeVisualState,
	};

	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	// Activate the node editor's tab so its canvas paints.
	let node_editor_entity = cx.read(|app| root.read(app).panels.node_editor.entity_id());
	cx.update_window(window.into(), |_, window, app| {
		root.update(app, |root, cx| {
			root.dock.update(cx, |dock, cx| {
				dock.select_panel(DockPanelId::from(node_editor_entity), window, cx);
			});
		});
	})
	.expect("window is open");
	cx.run_until_parked();

	let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
	for _ in 0..4 {
		visual.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
	}
	let canvas = visual
		.debug_bounds("node-editor-canvas")
		.expect("node editor canvas painted");

	let (engine, target_id, press, zoom) = visual.read(|app| {
		let (graph, engine) = root.read(app).panels.node_editor.read(app).test_handles();
		let graph = graph.read(app);
		let state = graph.state().clone();
		let zoom = state.zoom();
		let nodes = graph.data().read(app).nodes();
		let card = |n: &<MockEngine as crate::widgets::node_graph::NodeGraphDataSource>::Node| {
			let element = NodeElement::from_node(n, NodeVisualState::default());
			Bounds::new(
				state.graph_to_screen(n.position()),
				size(
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
		(engine, node.id(), canvas.origin + bounds.center(), zoom)
	});

	let position_of = |app: &App| {
		engine
			.read(app)
			.nodes()
			.into_iter()
			.find(|n| n.id() == target_id)
			.expect("the aimed node")
			.position()
	};
	let before = visual.read(|app| position_of(app));

	visual.simulate_mouse_down(
		press,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	let target = press + gpui_kit::point(px(10.0), px(6.0));
	visual.simulate_mouse_move(
		target,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	visual.simulate_mouse_up(
		target,
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::default(),
	);
	cx.run_until_parked();

	let after = visual.read(|app| position_of(app));
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

/// 窗口 → 检查器 closes the inspector through the dock's remove flow (the
/// same path the tab ✕ takes) and re-opens it from the closed state — the
/// round trip that used to leave a dismissed inspector unrecoverable.
#[gpui_kit::test]
async fn window_menu_toggle_closes_and_reopens_the_inspector(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// The checkmark shown in the 窗口 menu for the inspector, derived from
	// the dock's current visible-panel set exactly as `rebuild_menu_bar`
	// computes it.
	let inspector_checked = |app: &App| {
		let dock = root.read(app).dock.read(app);
		let mut state = MenuState::new(true);
		state.open_panels = open_panels_mask(
			dock,
			&root.read(app).panels,
			root.read(app).timeline_visible,
		);
		make_menus(state)
			.into_iter()
			.find(|e| e.title == crate::i18n::tr("menu.window"))
			.expect("Window menu exists")
			.menu
			.items
			.iter()
			.find(|item| item.id == ActionId::FocusInspector.menu_id())
			.expect("inspector item exists")
			.checked
	};

	// The inspector starts docked and checked.
	assert!(cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));
	assert_eq!(cx.read(|app| inspector_checked(app)), Some(true));

	// Toggle closed via the menu action: removed from the dock and
	// unchecked.
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_INSPECTOR, cx)));
	cx.run_until_parked();
	assert!(!cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));
	assert!(
		!cx.read(|app| dock_panel_open(app, &root, INSPECTOR)),
		"the inspector is fully closed, not floating"
	);
	assert_eq!(cx.read(|app| inspector_checked(app)), Some(false));

	// Toggle open again: the inspector comes back (its entity is reused,
	// so its state survives the close).
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_INSPECTOR, cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));
	assert_eq!(cx.read(|app| inspector_checked(app)), Some(true));
}

/// 窗口 → 重置布局 tears the current dock arrangement down and rebuilds
/// every panel at its design position with the default ratios: panels
/// dismissed via the 窗口 menu come back, floating (tear-off) panels'
/// windows are closed for good, and tilted splits are restored.
#[gpui_kit::test]
async fn reset_layout_restores_the_default_workspace(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	// Distort the workspace: dismiss two panels and shrink the bottom
	// dock. (The fork's tear-off/floating panels are gone with the
	// gpui-kit dock.)
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_INSPECTOR, cx)));
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_HISTORY, cx)));
	cx.run_until_parked();
	cx.update_window(window.clone().into(), |_, window, app| {
		let dock = root.read(app).dock.clone();
		dock.update(app, |dock, cx| {
			dock.set_dock_size(DockPlacement::Bottom, px(200.0), window, cx);
		});
	})
	.expect("window open");
	cx.run_until_parked();

	// The distorted state is in place.
	assert_eq!(
		cx.read(|app| {
			(
				dock_panel_open(app, &root, INSPECTOR),
				dock_panel_open(app, &root, HISTORY),
				root.read(app)
					.dock
					.read(app)
					.dock_size(DockPlacement::Bottom),
			)
		})
		.0,
		false
	);
	assert!(!cx.read(|app| dock_panel_open(app, &root, HISTORY)));

	// 窗口 → 重置布局 restores every panel to its design position.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(menu_ids::RESET_DEFAULT_LAYOUT, cx)
		});
	});
	cx.run_until_parked();

	assert!(
		cx.read(|app| {
			WINDOW_PANELS
				.iter()
				.all(|(panel, _)| dock_panel_open(app, &root, *panel))
		}),
		"every panel is back in the dock after the reset"
	);

	// The 窗口 menu's checkmarks follow the restored visible-panel set.
	let mask = cx.read(|app| {
		open_panels_mask(
			root.read(app).dock.read(app),
			&root.read(app).panels,
			root.read(app).timeline_visible,
		)
	});
	let mut state = MenuState::new(true);
	state.open_panels = mask;
	let window_menu = make_menus(state)
		.into_iter()
		.find(|e| e.title == crate::i18n::tr("menu.window"))
		.expect("Window menu exists")
		.menu;
	for (panel, action) in WINDOW_PANELS {
		let item = window_menu
			.items
			.iter()
			.find(|item| item.id == action.menu_id())
			.unwrap_or_else(|| panic!("Window menu is missing panel {}", panel));
		assert_eq!(
			item.checked,
			Some(true),
			"{} is re-checked after the reset",
			panel
		);
	}
}

#[gpui_kit::test]
async fn closing_a_floating_panel_restores_it_to_the_dock(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1200.0), px(800.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, None, cx)
	});
	let root = window.root(cx).expect("app root");
	root.update(cx, |app, cx| app.open_floating_panel(PROGRAM_VIEWER, cx));
	assert_eq!(cx.read(|cx| root.read(cx).floating_panels.len()), 1);
	let entity = cx.read(|cx| root.read(cx).panels.program_viewer.clone());
	assert!(cx.read(|cx| {
		root.read(cx)
			.dock
			.read(cx)
			.panel(DockPanelId::from(entity.entity_id()))
			.is_none()
	}));

	root.update(cx, |app, cx| app.close_floating_panel(PROGRAM_VIEWER, cx));
	cx.run_until_parked();
	assert!(cx.read(|cx| root.read(cx).floating_panels.is_empty()));
	assert!(cx.read(|cx| {
		root.read(cx)
			.dock
			.read(cx)
			.panel(DockPanelId::from(entity.entity_id()))
			.is_some()
	}));
}

/// Closing a panel via its tab ✕ — the dock's own close flow — also
/// refreshes the 窗口 menu: the dock's structural event rebuilds the menu
/// bar, so the dismissed panel loses its checkmark without waiting for any
/// other rebuild trigger.
#[gpui_kit::test]
async fn closing_a_panel_via_its_tab_unchecks_it_in_the_window_menu(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let menu_bar_before = cx.read(|app| root.read(app).menu_bar.entity_id());
	assert!(cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));

	// Close the inspector through the dock's own remove flow (what the
	// tab's ✕ button drives; gpui-kit's close-button click itself is
	// covered by gpui-component's tests).
	cx.update_window(window.clone().into(), |_, window, app| {
		root.update(app, |app, cx| {
			let inspector = app.panels.inspector.clone();
			let dock = app.dock.clone();
			dock.update(cx, |dock, cx| dock.remove_panel(inspector, window, cx));
		});
	})
	.expect("window open");
	cx.run_until_parked();

	// The inspector is gone and the menu bar was rebuilt (new entity) from
	// the dock's shrunken visible-panel set.
	assert!(!cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));
	let menu_bar_after = cx.read(|app| root.read(app).menu_bar.entity_id());
	assert_ne!(
		menu_bar_after, menu_bar_before,
		"the dock's structural event rebuilds the menu bar"
	);
}

/// The File menu exposes the full project lifecycle actions (new /
/// open-from-library / open-file / manager / export-project / close /
/// export) and the Edit menu the undo stack plus the delete variants,
/// across both languages.
#[test]
fn file_and_edit_menus_cover_the_project_lifecycle() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let entry = |title: &str| -> MenuBarEntry {
		make_menus_for_test()
			.into_iter()
			.find(|entry| entry.title == title)
			.expect("menu exists")
	};

	crate::i18n::set_language_code("en-US");
	let file = entry("File(F)");
	for id in [
		menu_ids::NEW_PROJECT,
		menu_ids::OPEN_FROM_LIBRARY,
		menu_ids::OPEN_PROJECT,
		menu_ids::PROJECT_MANAGER,
		menu_ids::EXPORT_PROJECT,
		menu_ids::CLOSE,
		menu_ids::EXPORT,
		menu_ids::QUIT,
	] {
		assert!(
			file.menu.items.iter().any(|item| item.id == id),
			"File menu is missing item {id}"
		);
	}
	let edit = entry("Edit(E)");
	for id in [
		menu_ids::UNDO,
		menu_ids::REDO,
		menu_ids::DELETE,
		menu_ids::RIPPLE_DELETE,
	] {
		assert!(
			edit.menu.items.iter().any(|item| item.id == id),
			"Edit menu is missing item {id}"
		);
	}

	// The same ids exist in the zh-CN menu bar.
	crate::i18n::set_language_code("zh-CN");
	let file = entry("文件(F)");
	assert!(
		file.menu
			.items
			.iter()
			.any(|item| item.id == menu_ids::EXPORT_PROJECT)
	);
	// Restore the default language for the next (lock-free) test.
	crate::i18n::set_language_code("en-US");
}

/// Opening 视图 → Preferences… must not crash: the dialog content and the
/// modal are built on the main window and the shell state swaps over the
/// entity's weak handle (regression test for the Preferences crash).
#[gpui_kit::test]
async fn preferences_dialog_opens_without_crashing(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1600.0), px(900.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, None, cx)
	});
	cx.run_until_parked();
	let root = window.root(cx).expect("app root");

	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::PREFERENCES, cx)));
	cx.run_until_parked();
	// Force a draw so render-time panics in the dialog content surface.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	let has_modal = cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. }));
	assert!(
		has_modal,
		"preferences modal should be shown after the menu action"
	);
}

/// The General tab's new rows are live end to end: the cache-ahead spin
/// box writes `PlaybackPreRenderFrames` (the playback pre-render window
/// reads it every tick) and the storage backend combo writes
/// `Storage/Backend` — PostgreSQL reveals the `Storage/PgUrl` field the
/// write-through library consumes.
#[gpui_kit::test]
async fn preferences_general_tab_writes_cache_ahead_and_storage(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	// The config store is process-wide: pin the keys to a known state and
	// restore them afterwards so no other test inherits these writes.
	struct Restore(&'static str, String);
	impl Drop for Restore {
		fn drop(&mut self) {
			crate::oakui::real::config_set_string(self.0, &self.1);
		}
	}
	let _ahead = Restore(
		"PlaybackPreRenderFrames",
		crate::oakui::real::config_get_string("PlaybackPreRenderFrames"),
	);
	let _backend = Restore(
		"Storage/Backend",
		crate::oakui::real::config_get_string("Storage/Backend"),
	);
	let _pg_url = Restore(
		"Storage/PgUrl",
		crate::oakui::real::config_get_string("Storage/PgUrl"),
	);
	crate::oakui::real::config_set_int("PlaybackPreRenderFrames", 120);
	crate::oakui::real::config_set_string("Storage/Backend", "sqlite");

	let (window, root) = mock_shell(cx);
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::PREFERENCES, cx)));
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.run_until_parked();
	// Clear the app keymap: the global up/down/home/end bindings
	// (GoToPrevCut / GoToNextCut / GoToStart / GoToEnd) dispatch before
	// the controls' element key handlers and would shadow the spin
	// box's arrow keys and the combo's selection keys.
	cx.update(|cx| cx.clear_key_bindings());

	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	// The connection-string row only exists for the PostgreSQL backend.
	assert!(
		cx.debug_bounds("preferences-storage-pg-url").is_none(),
		"the PostgreSQL connection-string row is hidden for SQLite"
	);

	// Step the cache-ahead spin box once (8 frames): 120 → 128 lands in
	// the config the playback pre-render window reads. The spin box sits
	// at the row's right edge; clicking the row center can land on the
	// (non-focusable) label, so target the control itself and allow a
	// bounded re-focus retry — under a loaded parallel suite the first
	// stroke can be dropped before the focus hand-off lands (observed as
	// the config staying at 120). A stroke that *does* land always steps
	// exactly 8; an overshoot (136) fails loudly instead of passing.
	let ahead = cx
		.debug_bounds("preferences-cache-ahead")
		.expect("the cache-ahead row is rendered");
	let spin_point = gpui_kit::Point::new(
		ahead.origin.x + ahead.size.width - px(24.0),
		ahead.origin.y + ahead.size.height * 0.5,
	);
	let mut attempts = 0usize;
	loop {
		attempts += 1;
		cx.simulate_click(spin_point, gpui_kit::Modifiers::none());
		// Flush the focus effect and draw one frame before the key
		// dispatch.
		cx.run_until_parked();
		cx.update_window(window.into(), |_root, window, cx| {
			window.draw(cx).clear(cx);
		})
		.expect("window is still open");
		cx.run_until_parked();
		cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("up").unwrap());
		cx.run_until_parked();
		let value = crate::oakui::real::config_get_int("PlaybackPreRenderFrames", 120);
		if value == 128 {
			break;
		}
		assert_eq!(
			value, 120,
			"a delivered stroke steps exactly once (attempt {attempts})"
		);
		assert!(attempts < 5, "the spin box never received the key");
	}

	// Re-select the storage backend: PostgreSQL (the combo starts on
	// SQLite; with the popup closed the arrow keys drive the selection).
	let backend = cx
		.debug_bounds("preferences-storage-backend")
		.expect("the storage backend row is rendered");
	cx.simulate_click(backend.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("escape").unwrap());
	cx.run_until_parked();
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("down").unwrap());
	cx.run_until_parked();
	assert_eq!(
		crate::oakui::real::config_get_string("Storage/Backend"),
		"pg",
		"selecting PostgreSQL writes Storage/Backend to the config"
	);
	// A fresh draw reveals the connection-string row.
	cx.update(|window, cx| window.draw(cx).clear(cx));
	cx.run_until_parked();
	cx.debug_bounds("preferences-storage-pg-url")
		.expect("the PostgreSQL connection-string row appears after the switch");
}

/// Opening File → Project Properties… must not crash: the content is
/// built with the engine and the modal is layered onto the shell (the
/// same deferred-build path as Preferences).
#[gpui_kit::test]
async fn project_properties_dialog_opens_without_crashing(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1600.0), px(900.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, None, cx)
	});
	cx.run_until_parked();
	let root = window.root(cx).expect("app root");

	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(ActionId::ProjectProperties.menu_id(), cx)
		})
	});
	cx.run_until_parked();
	// Force a draw so render-time panics in the dialog content surface.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	let has_modal =
		cx.read(|app| matches!(root.read(app).modal, ModalState::ProjectProperties { .. }));
	assert!(
		has_modal,
		"project properties modal should be shown after the menu action"
	);
}

/// The dialog's OK button applies the chosen cache location to the
/// engine: choosing 自定义位置 + a path, then clicking OK, lands in
/// `project_cache_location()` as `(2, path)` and closes the dialog.
#[gpui_kit::test]
async fn project_properties_commit_applies_cache_location(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(ActionId::ProjectProperties.menu_id(), cx)
		})
	});
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::ProjectProperties { content, .. } => content.clone(),
		_ => panic!("project properties modal should be open"),
	});

	// Choose 自定义位置 and type a path.
	let _ = cx.update_window(window.clone().into(), |_, window, app| {
		content.update(app, |dialog, cx| {
			dialog.select_cache_setting(2, cx);
			dialog.set_custom_cache_path("/tmp/oak-cache", window, cx);
		});
	});
	cx.run_until_parked();

	// Click the OK button (the modal's primary button).
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.run_until_parked();
	let mut vcx = VisualTestContext::from_window(window.into(), cx);
	let ok = vcx
		.debug_bounds("dialog-button-0")
		.expect("OK button rendered");
	vcx.simulate_click(ok.center(), gpui_kit::Modifiers::none());
	drop(vcx);
	cx.run_until_parked();

	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::None)),
		"OK closes the dialog"
	);
	let location = cx.read(|app| root.read(app).engine.read(app).project_cache_location());
	assert_eq!(location, (2, "/tmp/oak-cache".to_string()));
}

/// A bogus OCIO config keeps the dialog open: clicking OK runs the
/// engine's validation, which rejects the path, shows the error label
/// under the OCIO row and leaves the modal on screen.
#[gpui_kit::test]
async fn project_properties_invalid_ocio_keeps_the_dialog_open(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(ActionId::ProjectProperties.menu_id(), cx)
		})
	});
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::ProjectProperties { content, .. } => content.clone(),
		_ => panic!("project properties modal should be open"),
	});

	// Type a path that cannot load as an OCIO config.
	let _ = cx.update_window(window.clone().into(), |_, window, app| {
		content.update(app, |dialog, cx| {
			dialog.set_ocio_config_path("/nonexistent/ocio/config.ocio", window, cx);
		});
	});
	cx.run_until_parked();

	// Click OK: the commit fails, the dialog stays open, the error shows.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.run_until_parked();
	let mut vcx = VisualTestContext::from_window(window.into(), cx);
	let ok = vcx
		.debug_bounds("dialog-button-0")
		.expect("OK button rendered");
	vcx.simulate_click(ok.center(), gpui_kit::Modifiers::none());
	drop(vcx);
	cx.run_until_parked();

	let still_open =
		cx.read(|app| matches!(root.read(app).modal, ModalState::ProjectProperties { .. }));
	assert!(still_open, "an invalid OCIO config keeps the dialog open");
	let error = cx.read(|app| match &root.read(app).modal {
		ModalState::ProjectProperties { content, .. } => content.read(app).error().cloned(),
		_ => None,
	});
	let error = error.expect("the commit error is recorded");
	assert!(
		error.to_lowercase().contains("ocio"),
		"the error mentions the config: {error}"
	);

	// The error label renders under the OCIO row.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	let mut vcx = VisualTestContext::from_window(window.into(), cx);
	assert!(
		vcx.debug_bounds("projprops-error").is_some(),
		"the OCIO error label renders"
	);
	drop(vcx);
}

// -------------------------------------------------------------------
// Keyboard shortcuts (M12 P5c)
// -------------------------------------------------------------------

/// Every registry action with a default key exists as a menu item
/// (recursing into submenus), so a key press can never dispatch a dead
/// action.
#[test]
fn every_shortcut_maps_to_a_menu_item() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	fn collect(menu: &Menu, out: &mut Vec<usize>) {
		for item in &menu.items {
			out.push(item.id);
			if let Some(sub) = &item.submenu {
				collect(sub, out);
			}
		}
	}
	let mut ids = Vec::new();
	for entry in make_menus_for_test() {
		collect(&entry.menu, &mut ids);
	}
	// Context-menu-only actions (the timeline clip menu's synchronize
	// group) appear in the clip menu instead of the menu bar.
	collect(
		&crate::panels::timeline::clip_menu(
			crate::oakui::engine::SyncEligibility::default(),
			&[],
			None,
		),
		&mut ids,
	);
	for entry in crate::actions::REGISTRY {
		if entry.default_keys.is_empty() {
			continue;
		}
		assert!(
			ids.contains(&entry.menu_id()),
			"shortcut {} → action {} has no menu item",
			entry.default_keys[0],
			entry.cpp_id
		);
	}
}

/// The menu's shortcut labels mirror the registry's display strings (a
/// drift between them would show the user the wrong key). Both sides
/// come from [`crate::actions::display_shortcut`], so this holds on
/// every platform.
#[test]
fn menu_shortcut_labels_match_the_table() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	fn walk(menu: &Menu) -> Vec<(usize, Option<gpui_kit::SharedString>)> {
		let mut out = Vec::new();
		for item in &menu.items {
			out.push((item.id, item.shortcut.clone()));
			if let Some(sub) = &item.submenu {
				out.extend(walk(sub));
			}
		}
		out
	}
	for entry in make_menus_for_test() {
		for (id, label) in walk(&entry.menu) {
			let Some(label) = label else { continue };
			let Some(registry_entry) = crate::actions::entry_for_menu_id(id) else {
				panic!("menu item {id} shows {label} but is not a registry action");
			};
			let expected = crate::actions::display_shortcut(registry_entry.action)
				.unwrap_or_else(|| panic!("menu item {id} shows {label} but has no shortcut"));
			assert_eq!(
				label.as_ref(),
				expected,
				"shortcut label drift on menu item {id}"
			);
		}
	}
}

/// Pressing space on the shell toggles program playback (the keystroke
/// bubbles to the shell's key listener and dispatches 回放 → 播放/暂停).
#[gpui_kit::test]
async fn space_toggles_program_playback(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let playing = cx.read(|app| root.read(app).program_clock.read(app).is_playing());
	assert!(!playing, "the shell starts paused");

	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("space").unwrap());
	cx.run_until_parked();
	let playing = cx.read(|app| root.read(app).program_clock.read(app).is_playing());
	assert!(playing, "space starts program playback");

	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("space").unwrap());
	cx.run_until_parked();
	let playing = cx.read(|app| root.read(app).program_clock.read(app).is_playing());
	assert!(!playing, "space again pauses program playback");
}

/// The registry's default keys dispatch their actions end to end:
/// shuttle (j/k/l), in/out points (i/o), marker (m), snapping (s),
/// timeline zoom (=/-), track height (⌘=) and — with the timeline
/// focused — select-all/deselect-all (⌘A/⌘⇧A) plus the fall-through of
/// the still-unwired ripple-to-in/out (q/w).
#[gpui_kit::test]
async fn keymap_defaults_dispatch_their_actions(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let key = |keystroke: &str| gpui_kit::Keystroke::parse(keystroke).unwrap();

	// --- shuttle: l plays, k stops, j steps back ----------------------
	settle_key(cx, window.into(), "l");
	assert!(
		cx.read(|app| root.read(app).program_clock.read(app).is_playing()),
		"l starts playback"
	);
	settle_key(cx, window.into(), "k");
	assert!(
		!cx.read(|app| root.read(app).program_clock.read(app).is_playing()),
		"k stops playback"
	);
	for _ in 0..10 {
		settle_key(cx, window.into(), "right");
	}
	settle_key(cx, window.into(), "j");
	assert_eq!(
		cx.read(|app| root.read(app).program_clock.read(app).current_frame()),
		Frame(9),
		"j steps one frame back from frame 10"
	);

	// --- in/out points: i marks the in at 9, o the out at 15 ----------
	cx.dispatch_keystroke(window.into(), key("i"));
	for _ in 0..6 {
		settle_key(cx, window.into(), "right");
	}
	settle_key(cx, window.into(), "o");
	let workarea = cx.read(|app| root.read(app).engine.read(app).workarea());
	assert_eq!(
		workarea,
		Some((Frame(9), Frame(15))),
		"i/o set the work area"
	);

	// --- marker: m adds one at the playhead ---------------------------
	settle_key(cx, window.into(), "m");
	let markers = cx.read(|app| root.read(app).engine.read(app).markers());
	assert!(
		markers.iter().any(|marker| marker.frame == Frame(15)),
		"m adds a marker at the playhead"
	);

	// --- snapping: s toggles the timeline's snap flag ------------------
	settle_key(cx, window.into(), "s");
	assert!(
		!cx.read(|app| root.read(app).timeline.read(app).state.snap_enabled),
		"s toggles snapping off"
	);

	// --- zoom: = zooms in, - zooms out ---------------------------------
	let zoom_before = cx.read(|app| root.read(app).timeline.read(app).state.zoom);
	settle_key(cx, window.into(), "=");
	let zoom_in = cx.read(|app| root.read(app).timeline.read(app).state.zoom);
	assert!(zoom_in > zoom_before, "= zooms the timeline in");
	settle_key(cx, window.into(), "-");
	let zoom_out = cx.read(|app| root.read(app).timeline.read(app).state.zoom);
	assert!(zoom_out < zoom_in, "- zooms the timeline out");

	// --- track height: ⌘= grows the tracks by 8px (24–160px clamp) ----
	let height = |cx: &mut TestAppContext| -> f32 {
		cx.read(|app| {
			root.read(app)
				.engine
				.read(app)
				.track(0)
				.map(|track| f32::from(track.height()))
				.unwrap_or_default()
		})
	};
	let height_before = height(cx);
	settle_key(cx, window.into(), "secondary-=");
	assert_eq!(height(cx), height_before + 8.0, "⌘= grows the tracks");

	// --- focused-panel routing: ⌘A / ⌘⇧A hit the timeline panel --------
	cx.update(|app| root.update(app, |app, _cx| app.focused_panel = Some(TIMELINE)));
	settle_key(cx, window.into(), "secondary-a");
	let selected = cx.read(|app| root.read(app).timeline.read(app).selection().len());
	assert!(selected > 0, "⌘A selects every clip via the timeline panel");
	settle_key(cx, window.into(), "secondary-shift-a");
	let selected = cx.read(|app| root.read(app).timeline.read(app).selection().len());
	assert_eq!(selected, 0, "⌘⇧A deselects via the timeline panel");

	// q/w (ripple-to-in/out) reach the panel but are not wired yet —
	// they fall through to the shell's placeholder, state untouched.
	cx.dispatch_keystroke(window.into(), key("q"));
	settle_key(cx, window.into(), "w");
	let workarea = cx.read(|app| root.read(app).engine.read(app).workarea());
	assert_eq!(
		workarea,
		Some((Frame(9), Frame(15))),
		"q/w are inert placeholders for now"
	);
}

/// ⌘Z on the shell dispatches 编辑 → 撤销 to the engine, and ⌘K splits
/// at the playhead.
#[gpui_kit::test]
async fn edit_shortcuts_dispatch_to_the_engine(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	// Move the playhead inside the first clip (the → shortcut steps one
	// frame), then split with ⌘K: the mock sequence grows by one clip.
	for _ in 0..10 {
		settle_key(cx, window.into(), "right");
	}
	let playhead = cx.read(|app| root.read(app).program_clock.read(app).current_frame());
	assert_eq!(playhead, Frame(10), "→ steps the playhead");

	let clips_before: usize = cx.read(|app| {
		let engine = root.read(app).engine.read(app);
		(0..engine.track_count())
			.filter_map(|i| engine.track(i))
			.map(|t| t.clips().len())
			.sum()
	});
	settle_key(cx, window.into(), "secondary-k");
	let clips_after: usize = cx.read(|app| {
		let engine = root.read(app).engine.read(app);
		(0..engine.track_count())
			.filter_map(|i| engine.track(i))
			.map(|t| t.clips().len())
			.sum()
	});
	assert!(
		clips_after > clips_before,
		"split at the playhead adds a clip ({clips_before} → {clips_after})"
	);

	// ⌘Z / ⌘⇧Z reach the engine's undo/redo (the mock counts the
	// calls). settle_key flushes the deferred binding dispatch that
	// intermittently landed after the assert on Windows CI.
	settle_key(cx, window.into(), "secondary-z");
	settle_key(cx, window.into(), "secondary-shift-z");
	let (undo, redo) = cx.read(|app| root.read(app).engine.read(app).undo_redo_calls());
	assert!(
		undo >= 1 && redo >= 1,
		"⌘Z/⌘⇧Z dispatch undo/redo (undo {undo}, redo {redo})"
	);
}

/// While a modal dialog is open the shell's shortcuts are inert (the
/// dialog's text fields must never trigger editing actions).
#[gpui_kit::test]
async fn shortcuts_are_suppressed_while_a_modal_is_open(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());

	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::PREFERENCES, cx)));
	cx.run_until_parked();

	// ⌘K would split at the playhead, "s" would toggle snapping and
	// space would start playback without the modal guard.
	let clips_before: usize = cx.read(|app| {
		let engine = root.read(app).engine.read(app);
		(0..engine.track_count())
			.filter_map(|i| engine.track(i))
			.map(|t| t.clips().len())
			.sum()
	});
	let snapping_before = cx.read(|app| root.read(app).timeline.read(app).state.snap_enabled);
	cx.dispatch_keystroke(
		window.into(),
		gpui_kit::Keystroke::parse("secondary-k").unwrap(),
	);
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("s").unwrap());
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("space").unwrap());
	cx.run_until_parked();

	let (clips_after, playing, snapping_after) = cx.read(|app| {
		let root = root.read(app);
		let clips: usize = (0..root.engine.read(app).track_count())
			.filter_map(|i| root.engine.read(app).track(i))
			.map(|t| t.clips().len())
			.sum();
		(
			clips,
			root.program_clock.read(app).is_playing(),
			root.timeline.read(app).state.snap_enabled,
		)
	});
	assert_eq!(
		clips_after, clips_before,
		"no split while the modal is open"
	);
	assert!(!playing, "no playback toggle while the modal is open");
	assert_eq!(
		snapping_after, snapping_before,
		"no snapping toggle while the modal is open"
	);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. })),
		"the modal is still open"
	);
}

// -------------------------------------------------------------------
// Action search + custom shortcuts (stage 7)
// -------------------------------------------------------------------

/// The `/` key opens the action search dialog (the registry's ActionSearch
/// default key), and Escape dismisses it.
#[gpui_kit::test]
async fn action_search_opens_from_the_slash_key(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	// A leftover real shortcuts file must not shadow the default `/`
	// binding under test.
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|app| root.update(app, |app, cx| app.rebind_keys(cx)));
	cx.run_until_parked();

	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("/").unwrap());
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::ActionSearch { .. })),
		"the / key should open the action search dialog"
	);

	// Escape closes it again (the modal's own handler).
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("escape").unwrap());
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::None)),
		"escape closes the action search dialog"
	);
}

/// The 窗口/Window → 多机位向导 menu item opens the wizard modal with
/// the engine's wizard footage rows (the mock offers a demo list), and
/// The multicam wizard action still opens the modal via the direct
/// dispatch path (the 窗口 → 多机位向导 MENU ITEM was cut from v0.5 —
/// the feature is out of the shipped surface, but the action code path
/// keeps its guard).
#[gpui_kit::test]
async fn multicam_wizard_menu_opens_the_modal(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	cx.update(|app| {
		root.update(app, |app, cx| {
			app.dispatch_action_id(crate::actions::ActionId::MulticamWizard, cx)
		})
	});
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	let modal_rows = cx.read(|app| match &root.read(app).modal {
		ModalState::MulticamWizard { content, .. } => content.read(app).selection().len(),
		_ => panic!("the multicam wizard modal should be open"),
	});
	// The mock offers four demo footage rows.
	let footage = cx.read(|app| {
		let engine = root.read(app).engine.read(app);
		engine.multicam_wizard_footage().unwrap_or_default()
	});
	assert_eq!(footage.len(), 4, "the mock exposes the demo footage list");
	assert_eq!(modal_rows, 0, "no angles are pre-checked");
}

/// Arrow keys move the search selection and Enter runs the selected action
/// through the same dispatch path the menu clicks take (here the dialog
/// closes because the action dispatched successfully).
#[gpui_kit::test]
async fn action_search_arrows_and_enter_dispatch_the_selection(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|app| root.update(app, |app, cx| app.rebind_keys(cx)));
	cx.run_until_parked();

	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("/").unwrap());
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::ActionSearch { .. })),
		"the search dialog is open"
	);

	// Down selects the first listed action, Enter runs it. The empty query
	// lists every menu-bar action, so the first one is New Project — the
	// v0.5 flow opens the new-project dialog (name + format), so the
	// search dialog hands off to it.
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("down").unwrap());
	cx.run_until_parked();
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("enter").unwrap());
	cx.run_until_parked();

	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::NewProject { .. })),
		"the search dialog hands off to the new-project dialog"
	);
}

/// The preferences dialog opens from its keyboard shortcut too (⌘,), and
/// its Keyboard tab enumerates the menu-bar actions — the deferred-modal
/// fix matters here: a shortcut dispatches inside a window update where
/// `spawn_modal` would otherwise silently fail.
#[gpui_kit::test]
async fn preferences_opens_from_its_shortcut_with_the_keyboard_tab(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|app| root.update(app, |app, cx| app.rebind_keys(cx)));
	cx.run_until_parked();

	cx.dispatch_keystroke(
		window.into(),
		gpui_kit::Keystroke::parse("secondary-,").unwrap(),
	);
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. })),
		"⌘, opens the preferences dialog"
	);

	// The tabbed content carries the Keyboard tab with a non-empty action
	// list (the general tab stays the default active tab).
	let rows = cx.read(|app| match &root.read(app).modal {
		ModalState::Preferences { content, .. } => content.read(app).keyboard_tab_row_count(app),
		_ => 0,
	});
	assert!(rows > 0, "the keyboard tab lists the menu-bar actions");
}

/// End to end through the Keyboard tab: switching to it, clicking the first
/// action's capture field and pressing a key assigns the new binding (the
/// override layer + interceptor + save path in one flow).
#[gpui_kit::test]
async fn keyboard_tab_capture_assigns_a_shortcut(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::PREFERENCES, cx)));
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.run_until_parked();

	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	// Switch to the Keyboard tab.
	let tab = cx
		.debug_bounds("prefs-tab-keyboard")
		.expect("keyboard tab button rendered");
	cx.simulate_click(tab.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	// Enter capture on the first row (New Project).
	let field = cx
		.debug_bounds("keyboard-capture-0")
		.expect("first capture field rendered");
	cx.simulate_click(field.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	// Press a key → it becomes the new binding.
	cx.dispatch_keystroke(
		window.into(),
		gpui_kit::Keystroke::parse("secondary-x").unwrap(),
	);
	cx.run_until_parked();

	let expected = gpui_kit::Keystroke::parse("secondary-x").unwrap().unparse();
	assert_eq!(
		crate::actions::effective_keys(ActionId::NewProject.entry()),
		vec![expected],
		"capture assigns the pressed key to the row's action"
	);
	// The change is live: the shortcut display (used by the menus and the
	// row label) follows the override.
	assert_ne!(
		crate::actions::display_shortcut(ActionId::NewProject),
		Some("⌘N".to_string()),
		"the label no longer shows the default key"
	);
	assert!(
		crate::actions::display_shortcut(ActionId::NewProject).is_some(),
		"the assigned key still shows a label"
	);
}

/// Escape during a capture cancels the capture but keeps the dialog open
/// (the interceptor stops the key before it can bubble to the modal's own
/// Escape handler).
#[gpui_kit::test]
async fn keyboard_tab_capture_escape_cancels_without_closing(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::PREFERENCES, cx)));
	cx.run_until_parked();
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.run_until_parked();

	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	let tab = cx
		.debug_bounds("prefs-tab-keyboard")
		.expect("keyboard tab button rendered");
	cx.simulate_click(tab.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	let field = cx
		.debug_bounds("keyboard-capture-0")
		.expect("first capture field rendered");
	cx.simulate_click(field.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("escape").unwrap());
	cx.run_until_parked();

	let modal_still_open =
		cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. }));
	let override_keys = crate::actions::effective_keys(ActionId::NewProject.entry());
	assert!(
		modal_still_open,
		"escape cancels the capture, not the dialog"
	);
	assert_eq!(
		override_keys,
		vec!["secondary-n".to_string()],
		"no override was written by the cancelled capture"
	);
}

/// A shortcut override re-binds the global key map immediately: the new
/// key drives the action, the displaced default key no longer does.
#[gpui_kit::test]
async fn custom_shortcut_overrides_are_live_after_rebind(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();

	// Move Snapping from its default `s` to `f5` and apply the new map.
	crate::actions::set_custom_shortcut("snapping", vec!["f5".to_string()]);
	cx.update(|app| root.update(app, |app, cx| app.rebind_keys(cx)));
	cx.run_until_parked();

	let snap =
		|cx: &TestAppContext| cx.read(|app| root.read(app).timeline.read(app).state.snap_enabled);
	let before = snap(cx);
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("f5").unwrap());
	cx.run_until_parked();
	let after = snap(cx);
	assert_ne!(before, after, "f5 toggles snapping after the override");

	// The displaced default key is inert now.
	let steady = snap(cx);
	cx.dispatch_keystroke(window.into(), gpui_kit::Keystroke::parse("s").unwrap());
	cx.run_until_parked();
	assert_eq!(
		snap(cx),
		steady,
		"the displaced default s no longer toggles snapping"
	);
}

/// 文件 → 导入素材… opens the *platform* path picker (not the in-window
/// file dialog) and routes the picked path to the engine's import; the
/// mock engine records it, so the async round trip is observable.
#[gpui_kit::test]
async fn import_footage_prompts_and_routes_the_picked_path(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1600.0), px(900.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, None, cx)
	});
	cx.run_until_parked();
	let root = window.root(cx).expect("app root");

	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::IMPORT_FOOTAGE, cx)));
	cx.run_until_parked();
	assert!(
		cx.did_prompt_for_paths(),
		"the platform path picker should be shown"
	);

	// Answer the picker with one file: the async continuation must land it
	// in the engine's import.
	let picked = PathBuf::from("/media/raw/interview.mov");
	cx.simulate_path_prompt_response({
		let picked = picked.clone();
		move |options| {
			assert!(options.multiple, "import allows multiple selection");
			Some(vec![picked])
		}
	});
	cx.run_until_parked();

	let imported = cx.read(|app| root.read(app).engine.read(app).imported_footage().to_vec());
	assert_eq!(imported, vec![picked]);

	// Cancelling the picker imports nothing.
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::IMPORT_FOOTAGE, cx)));
	cx.run_until_parked();
	cx.simulate_path_prompt_response(|_options| None);
	cx.run_until_parked();
	let imported = cx.read(|app| root.read(app).engine.read(app).imported_footage().len());
	assert_eq!(imported, 1, "a cancelled picker imports nothing");
}

/// Dragging files onto the project explorer routes them to the engine's
/// import: the explorer emits `FileDropRequested` on a real drop and the
/// panel's subscription calls `AppEngine::import_footage` per path (the
/// mock engine records them). The drop is simulated as the platform
/// delivers it — the OS drag entering the window, then the release.
#[gpui_kit::test]
async fn dropping_files_onto_the_project_browser_imports_them(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	let mut cx = VisualTestContext::from_window(window.into(), cx);

	// The mock shell's parked frame is live: dispatch the drop against it
	// (an extra draw would repaint the cached element tree and consume the
	// one-shot interactive listeners).
	let bounds = cx
		.debug_bounds("gpui-widgets-explorer-entry-1")
		.expect("the explorer's first row is rendered");
	let dropped = PathBuf::from("/media/raw/drag-drop.mov");
	let paths = ExternalPaths(std::iter::once(dropped.clone()).collect());
	// Deliver the whole drag sequence against the same rendered frame:
	// gpui's interactive listeners are registered per render, so a repaint
	// between the events would consume them before the drop lands.
	cx.update(|window, cx| {
		window.dispatch_event(
			gpui_kit::PlatformInput::FileDrop(FileDropEvent::Entered {
				position: bounds.center(),
				paths,
			}),
			cx,
		);
		window.dispatch_event(
			gpui_kit::PlatformInput::FileDrop(FileDropEvent::Pending {
				position: bounds.center(),
			}),
			cx,
		);
		window.dispatch_event(
			gpui_kit::PlatformInput::FileDrop(FileDropEvent::Submit {
				position: bounds.center(),
			}),
			cx,
		);
	});
	cx.run_until_parked();

	let imported = cx.read(|app| root.read(app).engine.read(app).imported_footage().to_vec());
	assert_eq!(imported, vec![dropped]);
}

/// Dragging a media entry from the project explorer onto the timeline
/// places a clip there: the row starts a [`FootageDrag`] carrying the
/// entry id, the timeline panel resolves the cursor to a display track +
/// frame, and the engine records the `drop_footage` request with the
/// correct parameters.
#[gpui_kit::test]
async fn dragging_footage_onto_the_timeline_places_a_clip(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	let mut cx = VisualTestContext::from_window(window.into(), cx);

	// The mock project's root-level footage entry ("第一稿.mp4", id 3) is
	// the drag source; the timeline's clip area is the drop target.
	let row = cx
		.debug_bounds("gpui-widgets-explorer-entry-3")
		.expect("root-level footage row rendered");
	let canvas = cx
		.debug_bounds("timeline-canvas")
		.expect("timeline body rendered");
	// A point inside the clip area: 100 px into the ruler's content
	// (frame 50 at the shell's default zoom of 2 px/frame) on the first
	// track row (V2, 64 px tall).
	let drop = gpui_kit::point(
		canvas.left() + px(crate::widgets::timeline::HEADER_WIDTH + 100.0),
		canvas.top() + px(crate::widgets::timeline::RULER_HEIGHT + 20.0),
	);

	// Dispatch the whole gesture against the same rendered frame: gpui's
	// interactive listeners are registered per render, so a repaint
	// between the events would consume them before the drop lands (same
	// caveat as the file-drop test above).
	cx.update(|window, cx| {
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
				position: row.center(),
				modifiers: gpui_kit::Modifiers::none(),
				button: gpui_kit::MouseButton::Left,
				click_count: 1,
				first_mouse: false,
			}),
			cx,
		);
		// Move past the drag threshold to start the drag.
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
				position: row.center() + gpui_kit::point(px(6.0), px(0.0)),
				modifiers: gpui_kit::Modifiers::none(),
				pressed_button: Some(gpui_kit::MouseButton::Left),
			}),
			cx,
		);
		// Hover the drop point (the panel resolves the track + frame).
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
				position: drop,
				modifiers: gpui_kit::Modifiers::none(),
				pressed_button: Some(gpui_kit::MouseButton::Left),
			}),
			cx,
		);
		// Release over the drop point.
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
				position: drop,
				modifiers: gpui_kit::Modifiers::none(),
				button: gpui_kit::MouseButton::Left,
				click_count: 1,
			}),
			cx,
		);
	});
	cx.run_until_parked();

	let drops = cx.read(|app| root.read(app).engine.read(app).footage_drops().to_vec());
	assert_eq!(
		drops.len(),
		1,
		"the drop must reach the engine exactly once"
	);
	assert_eq!(drops[0].id, 3);
	assert_eq!(
		drops[0].track_kind,
		crate::widgets::timeline::TrackKind::Video
	);
	assert_eq!(drops[0].track_index, 0);
	assert_eq!(drops[0].time, crate::widgets::timeline::Frame(50));
}

/// The project explorer's 添加文本素材 button creates a text generator
/// bin entry: the click emits `NewTextFootageRequested`, the shell's
/// subscription calls `AppEngine::create_text_footage`, and the bin
/// lists the new entry (labelled 文本, the shared node label) beside
/// the demo rows.
#[gpui_kit::test]
async fn clicking_add_text_footage_creates_a_text_entry(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	let mut vcx = VisualTestContext::from_window(window.into(), cx);

	let button = vcx
		.debug_bounds("project-add-text-footage")
		.expect("the explorer's 添加文本素材 button is rendered");
	vcx.simulate_click(button.center(), gpui_kit::Modifiers::none());
	drop(vcx);
	cx.run_until_parked();

	let created = cx.read(|app| root.read(app).engine.read(app).text_footages().to_vec());
	assert_eq!(
		created,
		vec![100],
		"the click reached the engine exactly once"
	);
	let listed = cx.read(|app| {
		use crate::widgets::project_explorer::ProjectDataSource as _;
		root.read(app)
			.engine
			.read(app)
			.roots()
			.iter()
			.any(|e| e.id == 100 && e.name == crate::oakui::graphops::TEXT_FOOTAGE_LABEL)
	});
	assert!(listed, "the created text entry is listed in the bin");
}

/// The project explorer lists root-level imported footage in BOTH views:
/// the tree shows the entry as a row, and after switching to the icon
/// grid the same entry (plus folder children) appears as icons.
#[gpui_kit::test]
async fn explorer_views_list_root_level_footage(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, _root) = mock_shell(cx);
	let mut cx = VisualTestContext::from_window(window.into(), cx);

	// Tree view: the root-level footage entry (id 3 "第一稿.mp4") renders
	// as a row.
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-entry-3").is_some(),
		"tree view shows the root-level footage row"
	);

	// Switch to the icon grid; root-level footage and folder children
	// both appear as icons, the folder root itself does not.
	let toggle = cx
		.debug_bounds("gpui-widgets-explorer-icons")
		.expect("icons toggle rendered");
	cx.simulate_click(toggle.center(), gpui_kit::Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-3").is_some(),
		"root-level footage shows as an icon"
	);
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-10").is_some(),
		"a folder child shows as an icon"
	);
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-1").is_none(),
		"the footage folder root itself has no icon"
	);
}

/// The command-line parser understands the project path and the mock
/// flag, and the `OAK_ENGINE` env var forces the mock.
#[test]
fn app_args_parse_path_and_mock_flag() {
	// Drive the real parser with synthetic argv (the env override is
	// a parameter, no process-global env is touched).
	let parse = |argv: &[&str], env: Option<&str>| {
		AppArgs::parse_args(argv.iter().map(std::ffi::OsString::from), env)
	};
	let a = parse(&["/tmp/a.ove"], None);
	assert_eq!(a.project, Some(PathBuf::from("/tmp/a.ove")));
	assert!(!a.mock);

	let b = parse(&["/tmp/a.ove", "--mock"], None);
	assert!(b.mock);

	let c = parse(&["/tmp/a.ove"], Some("MOCK"));
	assert!(c.mock);

	let d = parse(&[], None);
	assert!(d.project.is_none());
	assert!(!d.mock);

	// The first positional wins; later positionals are ignored.
	let e = parse(&["/tmp/a.ove", "/tmp/b.ove"], None);
	assert_eq!(e.project, Some(PathBuf::from("/tmp/a.ove")));
}

// -------------------------------------------------------------------
// Project manager (M13 D4)
// -------------------------------------------------------------------

/// 文件 → 导出序列 opens the dialog with the project's sequences (the
/// current one preselected); OK exports the picked sequence and swaps
/// in the progress dialog.
#[gpui_kit::test]
async fn export_sequence_dialog_picks_and_starts(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	cx.update(|app| root.update(app, |app, cx| app.on_menu(ActionId::Export.menu_id(), cx)));
	cx.run_until_parked();

	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Export { content, .. } => content.clone(),
		_ => panic!("the export dialog should be open"),
	});
	// The mock bin's one sequence (entry 4) is preselected; the
	// default output path is a real file name.
	assert_eq!(
		cx.read(|app| content.read(app).selected_sequence(app)),
		Some(4),
		"the current sequence is preselected"
	);
	let path = cx.read(|app| content.read(app).path(app).to_string());
	assert!(path.ends_with(".mp4"), "default export path: {path}");

	// OK starts the export of the picked sequence: the progress dialog
	// replaces the settings dialog.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::EXPORT,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })),
		"OK starts the export and opens the progress dialog"
	);
}

/// Dispatch a synthetic keystroke and settle with a double park.
/// gpui's window key handling may defer the binding dispatch (its
/// pending/repeat machinery); parking twice drains the executor
/// across two idle cycles so a deferred delivery lands before the
/// assertion. Deliberately does NOT advance the simulated clock —
/// the mock engine's playback ticks with executor time, so a clock
/// advance would move the playhead out from under the assertions.
fn settle_key(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
	cx.dispatch_keystroke(window, gpui_kit::Keystroke::parse(key).unwrap());
	cx.run_until_parked();
	cx.run_until_parked();
}

/// A running app shell on the mock engine (en-US), plus its root. The
/// caller holds the language lock (the tests flip the process-global
/// language).
fn mock_shell(
	cx: &mut TestAppContext,
) -> (
	gpui_kit::WindowHandle<OakApp<MockEngine>>,
	Entity<OakApp<MockEngine>>,
) {
	crate::i18n::set_language_code("en-US");
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1600.0), px(900.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, None, cx)
	});
	cx.run_until_parked();
	let root = window.root(cx).expect("app root");
	(window, root)
}

/// Whether `id`'s panel is open (in the dock; the timeline strip reads
/// its shell flag).
fn dock_panel_open(app: &App, root: &Entity<OakApp<MockEngine>>, id: PanelId) -> bool {
	let root = root.read(app);
	if id == TIMELINE {
		return root.timeline_visible;
	}
	root.panels
		.entity_id(id)
		.map(|entity| {
			root.dock
				.read(app)
				.panel(DockPanelId::from(entity))
				.is_some()
		})
		.unwrap_or(false)
}

/// Opens the manager and returns its content entity.
fn open_manager(
	cx: &mut TestAppContext,
	root: &Entity<OakApp<MockEngine>>,
) -> Entity<crate::manager::ProjectManager<MockEngine>> {
	cx.update(|app| root.update(app, |app, cx| app.show_project_manager(cx)));
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Manager { content, .. } => content.clone(),
		_ => panic!("the manager modal should be open"),
	});
	content
}

/// The manager's listed rows.
fn manager_rows(
	cx: &mut TestAppContext,
	content: &Entity<crate::manager::ProjectManager<MockEngine>>,
) -> Vec<crate::oakui::LibraryProject> {
	cx.read(|app| content.read(app).rows().to_vec())
}

/// The manager lists the mock library; opening a row (the double-click
/// / 打开 route) drives the engine's library open and closes the dialog.
#[gpui_kit::test]
async fn manager_lists_and_opens(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);
	let content = open_manager(cx, &root);
	let rows = manager_rows(cx, &content);
	assert_eq!(rows.len(), 3, "the mock library seeds three rows");
	assert_eq!(rows[0].name, "第一稿", "most recently modified first");
	assert!(rows[0].track_count > 0 && rows[0].footage_count > 0);

	// Select the second row and open it.
	let uuid = rows[1].uuid.clone();
	cx.update(|app| content.update(app, |m, cx| m.select(&uuid, cx)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&crate::manager::ManagerEvent::Open(uuid.clone()), cx)
		})
	});
	cx.run_until_parked();

	let opened = cx.read(|app| root.read(app).engine.read(app).library_opened().to_vec());
	assert_eq!(opened, vec![uuid], "the engine opened the selected row");
	let name = cx.read(|app| {
		root.read(app)
			.engine
			.read(app)
			.project()
			.unwrap()
			.name
			.clone()
	});
	assert_eq!(name, "宣传片 v3");
	let modal_none = cx.read(|app| matches!(root.read(app).modal, ModalState::None));
	assert!(modal_none, "a successful open closes the manager");
}

/// Create / rename / duplicate / delete round-trip through the manager
/// and its sub-dialogs.
#[gpui_kit::test]
async fn manager_create_rename_duplicate_delete(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	let _content = open_manager(cx, &root);

	// Create: a new row appears and the project opens (dialog closes).
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&crate::manager::ManagerEvent::Create, cx)
		})
	});
	cx.run_until_parked();
	let rows = cx.read(|app| root.read(app).engine.read(app).library_projects().unwrap());
	assert_eq!(rows.len(), 4);
	let created = rows
		.iter()
		.find(|row| row.name == "Untitled Project")
		.expect("the created row")
		.clone();
	let project_name = cx.read(|app| {
		root.read(app)
			.engine
			.read(app)
			.project()
			.unwrap()
			.name
			.clone()
	});
	assert_eq!(
		project_name, "Untitled Project",
		"create opens the new project"
	);

	// Reopen the manager, select the created row, rename it.
	let content = open_manager(cx, &root);
	cx.update(|app| content.update(app, |m, cx| m.select(&created.uuid, cx)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(
				&crate::manager::ManagerEvent::Rename(created.uuid.clone()),
				cx,
			)
		})
	});
	cx.run_until_parked();
	let prompt = cx.read(|app| match &root.read(app).modal {
		ModalState::ManagerRename { content, uuid, .. } => {
			assert_eq!(uuid, &created.uuid);
			content.clone()
		}
		_ => panic!("the rename prompt should be open"),
	});
	cx.update_window(window.into(), |_, window, app| {
		prompt.update(app, |p, cx| p.set_value("改名为正稿", window, cx));
	})
	.expect("window open");
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_RENAME,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	// The confirmation swaps in a FRESH manager (the captured content is
	// stale from here on); assert against the engine's library instead.
	let rows = cx.read(|app| root.read(app).engine.read(app).library_projects().unwrap());
	let renamed = rows.iter().find(|row| row.uuid == created.uuid).unwrap();
	assert_eq!(renamed.name, "改名为正稿");
	let back = cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. }));
	assert!(back, "a confirmed rename returns to the manager");

	// Duplicate the renamed row.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(
				&crate::manager::ManagerEvent::Duplicate(created.uuid.clone()),
				cx,
			)
		})
	});
	cx.run_until_parked();
	let rows = cx.read(|app| root.read(app).engine.read(app).library_projects().unwrap());
	assert_eq!(rows.len(), 5);
	assert!(
		rows.iter().any(|row| row.name == "改名为正稿 (copy)"),
		"the copy is named '<name> (copy)': {rows:?}"
	);

	// Delete the original through the confirmation dialog.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(
				&crate::manager::ManagerEvent::Delete(created.uuid.clone()),
				cx,
			)
		})
	});
	cx.run_until_parked();
	let confirming =
		cx.read(|app| matches!(root.read(app).modal, ModalState::ManagerDelete { .. }));
	assert!(confirming, "the delete confirmation should be open");
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_DELETE,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	let rows = cx.read(|app| root.read(app).engine.read(app).library_projects().unwrap());
	assert_eq!(rows.len(), 4);
	assert!(!rows.iter().any(|row| row.uuid == created.uuid));
	let back = cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. }));
	assert!(back, "a confirmed delete returns to the manager");
}

/// The manager's 导入 opens the platform path picker and lands the file
/// as a new row; 导出 asks for a new path and routes it to the engine.
#[gpui_kit::test]
async fn manager_import_and_export_route_through_the_platform_dialogs(cx: &mut TestAppContext) {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);
	let content = open_manager(cx, &root);

	// Import.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&crate::manager::ManagerEvent::Import, cx)
		})
	});
	cx.run_until_parked();
	assert!(cx.did_prompt_for_paths(), "import shows the path picker");
	cx.simulate_path_prompt_response(|options| {
		assert!(!options.multiple, "project import is single-file");
		Some(vec![PathBuf::from("/library/先导片.ove")])
	});
	cx.run_until_parked();
	let rows = manager_rows(cx, &content);
	assert_eq!(rows.len(), 4);
	assert!(
		rows.iter().any(|row| row.name == "先导片"),
		"the imported file becomes a row named by its stem: {rows:?}"
	);

	// Export the imported row.
	let uuid = rows
		.iter()
		.find(|row| row.name == "先导片")
		.unwrap()
		.uuid
		.clone();
	cx.update(|app| content.update(app, |m, cx| m.select(&uuid, cx)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&crate::manager::ManagerEvent::Export(uuid.clone()), cx)
		})
	});
	cx.run_until_parked();
	assert!(cx.did_prompt_for_new_path(), "export shows the save dialog");
	cx.simulate_new_path_selection(|_dir| Some(PathBuf::from("/library/先导片.otio")));
	cx.run_until_parked();
	let exported = cx.read(|app| root.read(app).engine.read(app).library_exported().to_vec());
	assert_eq!(
		exported,
		vec![(uuid, PathBuf::from("/library/先导片.otio"))],
		"the picked path routes to the engine's library export"
	);
}

// -------------------------------------------------------------------
// Coverage sweep: menus, modals, registry and shell plumbing
// -------------------------------------------------------------------

/// Fetches one menu item's checkmark from a built menu bar (recursing
/// into submenus; a submenu's item wins over a parent with the same id,
/// which is how the theme / add-tool groups are built).
fn menu_checked(entries: &[MenuBarEntry], id: usize) -> Option<bool> {
	fn walk(menu: &Menu, id: usize) -> Option<bool> {
		for item in &menu.items {
			if let Some(submenu) = &item.submenu {
				if let Some(checked) = walk(submenu, id) {
					return Some(checked);
				}
			}
			if item.id == id && item.checked.is_some() {
				return item.checked;
			}
		}
		None
	}
	entries.iter().find_map(|entry| walk(&entry.menu, id))
}

/// The shell-method signature the modal sweeps call through.
type AppFn = fn(&mut OakApp<MockEngine>, &mut Context<OakApp<MockEngine>>);

/// Pins a process-global config key to its current value, restoring it
/// on drop (config writes leak across tests otherwise).
struct ConfigGuard(&'static str, String);

impl ConfigGuard {
	fn pin(key: &'static str) -> Self {
		Self(key, crate::oakui::real::config_get_string(key))
	}
}

impl Drop for ConfigGuard {
	fn drop(&mut self) {
		crate::oakui::real::config_set_string(self.0, &self.1);
	}
}

/// Every dynamic flag in [`MenuState`] drives exactly the menu item the
/// shell's rebuild path toggles: theme (both directions), the 13 tools,
/// snapping, loop playback, show-all, full-screen, proxy media, and the
/// 窗口 panel checkmarks.
#[test]
fn menu_checkmarks_follow_every_dynamic_state_flag() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	// Theme: dark and light are two checked-exclusive items in the
	// 视图 submenu.
	let mut state = MenuState::new(true);
	assert_eq!(
		menu_checked(&make_menus(state), ActionId::ThemeDark.menu_id()),
		Some(true)
	);
	assert_eq!(
		menu_checked(&make_menus(state), ActionId::ThemeLight.menu_id()),
		Some(false)
	);
	state.dark = false;
	let menus = make_menus(state);
	assert_eq!(
		menu_checked(&menus, ActionId::ThemeDark.menu_id()),
		Some(false)
	);
	assert_eq!(
		menu_checked(&menus, ActionId::ThemeLight.menu_id()),
		Some(true)
	);

	// The mutually exclusive tool group: exactly the active tool is
	// checked.
	let tools = [
		Tool::Pointer,
		Tool::TrackSelect,
		Tool::Edit,
		Tool::Ripple,
		Tool::Rolling,
		Tool::Razor,
		Tool::Slip,
		Tool::Slide,
		Tool::Hand,
		Tool::Zoom,
		Tool::Transition,
		Tool::Add,
		Tool::Record,
	];
	for active in tools {
		let mut state = MenuState::new(true);
		state.active_tool = active;
		let menus = make_menus(state);
		for tool in tools {
			assert_eq!(
				menu_checked(&menus, tool.action().menu_id()),
				Some(tool == active),
				"tool checkmark follows the active tool"
			);
		}
	}

	// Boolean placeholders / registry state, both directions.
	let mut state = MenuState::new(true);
	state.snapping = false;
	state.loop_playback = true;
	state.full_screen = true;
	state.use_proxy_media = false;
	let menus = make_menus(state);
	assert_eq!(
		menu_checked(&menus, ActionId::Snapping.menu_id()),
		Some(false)
	);
	assert_eq!(menu_checked(&menus, ActionId::Loop.menu_id()), Some(true));
	assert_eq!(
		menu_checked(&menus, ActionId::FullScreen.menu_id()),
		Some(true)
	);
	assert_eq!(
		menu_checked(&menus, ActionId::UseProxyMedia.menu_id()),
		Some(false)
	);
	state.snapping = true;
	state.loop_playback = false;
	state.full_screen = false;
	state.use_proxy_media = true;
	let menus = make_menus(state);
	assert_eq!(
		menu_checked(&menus, ActionId::Snapping.menu_id()),
		Some(true)
	);
	assert_eq!(menu_checked(&menus, ActionId::Loop.menu_id()), Some(false));
	assert_eq!(
		menu_checked(&menus, ActionId::FullScreen.menu_id()),
		Some(false)
	);
	assert_eq!(
		menu_checked(&menus, ActionId::UseProxyMedia.menu_id()),
		Some(true)
	);

	// 窗口: no visible panel → every panel unchecked; all bits → every
	// panel checked.
	let mut state = MenuState::new(true);
	state.open_panels = 0;
	let menus = make_menus(state);
	for (_, action) in WINDOW_PANELS {
		assert_eq!(menu_checked(&menus, action.menu_id()), Some(false));
	}
	state.open_panels = u16::MAX;
	let menus = make_menus(state);
	for (_, action) in WINDOW_PANELS {
		assert_eq!(menu_checked(&menus, action.menu_id()), Some(true));
	}
}

/// entry point and reports itself through `ModalState::modal_entity`
/// (which `Render` layers).
#[gpui_kit::test]
async fn every_shell_modal_opens_and_reports_its_entity(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let has_modal = |cx: &TestAppContext| -> bool {
		cx.read(|app| root.read(app).modal.modal_entity().is_some())
	};
	let close = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
		cx.run_until_parked();
		assert!(
			!cx.read(|app| root.read(app).modal.modal_entity().is_some()),
			"close_modal clears the layering entity"
		);
	};
	let run = |cx: &mut TestAppContext, f: AppFn| {
		cx.update(|app| root.update(app, f));
		cx.run_until_parked();
	};

	// Preferences and the action search build through the deferred path.
	run(cx, |app, cx| app.open_preferences(cx));
	assert!(has_modal(cx), "preferences modal");
	close(cx);

	run(cx, |app, cx| app.open_action_search(cx));
	assert!(has_modal(cx), "action search modal");
	close(cx);

	// File menu:
	run(cx, |app, cx| app.open_new_project(cx));
	assert!(has_modal(cx), "new project modal");
	close(cx);
	run(cx, |app, cx| app.open_new_sequence(cx));
	assert!(has_modal(cx), "new sequence modal");
	close(cx);
	run(cx, |app, cx| app.open_export_project_dfd(cx));
	assert!(has_modal(cx), "export project modal");
	close(cx);
	run(cx, |app, cx| app.open_save_as(cx));
	assert!(has_modal(cx), "save-as modal");
	close(cx);
	run(cx, |app, cx| app.open_entry_rename(3, cx));
	assert!(has_modal(cx), "entry rename modal");
	close(cx);
	run(cx, |app, cx| app.open_sequence_properties(4, cx));
	assert!(has_modal(cx), "sequence properties modal");
	close(cx);
	run(cx, |app, cx| app.open_export_dialog(None, cx));
	assert!(has_modal(cx), "export dialog");
	close(cx);
	run(cx, |app, cx| app.open_project_properties(cx));
	assert!(has_modal(cx), "project properties modal");
	close(cx);

	// Tools / Help:
	run(cx, |app, cx| app.open_proxy_dialog(cx));
	assert!(has_modal(cx), "proxy modal");
	close(cx);
	run(cx, |app, cx| app.open_multicam_wizard(cx));
	assert!(has_modal(cx), "multicam wizard modal");
	close(cx);
	run(cx, |app, cx| app.open_about(cx));
	assert!(has_modal(cx), "about modal");
	close(cx);

	// Project manager and its sub-dialogs.
	run(cx, |app, cx| app.show_project_manager(cx));
	assert!(has_modal(cx), "manager modal");
	run(cx, |app, cx| app.open_manager_rename("mock-1".into(), cx));
	assert!(has_modal(cx), "manager rename modal");
	close(cx);
	run(cx, |app, cx| app.open_manager_delete("mock-1".into(), cx));
	assert!(has_modal(cx), "manager delete modal");
	close(cx);

	// The drop-onto-empty-timeline choice, then the progress dialog
	// (started from the export dialog's OK).
	run(cx, |app, cx| {
		app.on_footage_drop_needs_sequence(
			FootageDropNeedsSequence {
				footage_id: 3,
				footage_ids: vec![3],
				track_kind: crate::widgets::timeline::TrackKind::Video,
				track_index: 0,
				time: Frame(0),
			},
			cx,
		)
	});
	assert!(has_modal(cx), "drop-sequence choice modal");
	close(cx);

	run(cx, |app, cx| app.open_export_dialog(None, cx));
	run(cx, |app, cx| {
		app.on_modal(
			&ModalEvent::ButtonClicked {
				control: modal_ids::EXPORT,
				button: 0,
			},
			cx,
		)
	});
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })),
		"OK starts the export and swaps in the progress modal"
	);
	close(cx);

	// The shell still renders with a modal layered.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The OFX plugin-progress channel drives the shell's progress modal:
/// the first event opens it, later events move the bar, a completion
/// fraction dismisses it, and events that arrive while another modal is
/// up never hijack the screen.
#[gpui_kit::test]
async fn plugin_progress_channel_drives_the_progress_modal(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// The shell's tick loop drains its receiver: swap in a channel the
	// test drives deterministically.
	let (tx, rx) = mpsc::channel::<crate::oakui::ofx::PluginProgressEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.plugin_progress_rx = Mutex::new(rx);
		})
	});

	let tick = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
		cx.run_until_parked();
	};
	let event = |fraction: f64| crate::oakui::ofx::PluginProgressEvent {
		label: "OakOFX Render".into(),
		message: format!("frame {:.2}", fraction * 10.0),
		fraction,
	};

	// An empty drain is a no-op.
	tick(cx);

	// The first event opens the progress dialog.
	tx.send(event(0.25)).unwrap();
	tick(cx);
	assert!(cx.read(|app| root.read(app).plugin_progress_open));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })));

	// A later event only updates the bar (the dialog stays up).
	tx.send(event(0.75)).unwrap();
	tick(cx);
	assert!(cx.read(|app| root.read(app).plugin_progress_open));

	// Cancel through the dialog's secondary button: the flag drops and
	// the modal closes.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::PLUGIN_PROGRESS,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	assert!(!cx.read(|app| root.read(app).plugin_progress_open));

	// While another modal is up, plugin events never replace it.
	cx.update(|app| root.update(app, |app, cx| app.open_preferences(cx)));
	cx.run_until_parked();
	tx.send(event(0.5)).unwrap();
	tick(cx);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. })));
	assert!(!cx.read(|app| root.read(app).plugin_progress_open));
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// A completion fraction (1.0) closes a dialog the channel opened.
	tx.send(event(0.1)).unwrap();
	tick(cx);
	assert!(cx.read(|app| root.read(app).plugin_progress_open));
	tx.send(event(1.0)).unwrap();
	tick(cx);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	assert!(!cx.read(|app| root.read(app).plugin_progress_open));
}

/// The export session's event channel drives the progress dialog end to
/// end: `Started` is ignored, `Progress` moves the bar, `Finished`
/// closes the dialog (both outcomes), and Cancel / Escape run the
/// session's cancel handle. `begin_export`'s guards (empty path, no
/// sequence) are covered too.
#[gpui_kit::test]
async fn export_progress_events_drive_the_progress_modal(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let open_export = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.open_export_dialog(None, cx)));
		cx.run_until_parked();
	};
	let click_export_ok = |cx: &mut TestAppContext| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.on_modal(
					&ModalEvent::ButtonClicked {
						control: modal_ids::EXPORT,
						button: 0,
					},
					cx,
				)
			})
		});
		cx.run_until_parked();
	};
	// Guards first: an export dialog with an empty path does not start.
	open_export(cx);
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Export { content, .. } => content.clone(),
		_ => panic!("the export dialog should be open"),
	});
	cx.update_window(window.clone().into(), |_, window, app| {
		content.update(app, |content, cx| content.set_path("", window, cx));
	})
	.expect("window open");
	click_export_ok(cx);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Export { .. })),
		"an empty output path keeps the settings dialog open"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// A path without an extension gets the format's extension appended
	// before the export starts.
	open_export(cx);
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Export { content, .. } => content.clone(),
		_ => panic!("the export dialog should be open"),
	});
	cx.update_window(window.clone().into(), |_, window, app| {
		content.update(app, |content, cx| {
			content.set_path("/tmp/oak-export", window, cx)
		});
	})
	.expect("window open");
	click_export_ok(cx);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })),
		"an extension-less path starts the export too"
	);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::EXPORT_PROGRESS,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// The real flow: OK starts the mock export and swaps in progress.
	open_export(cx);
	click_export_ok(cx);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })));
	assert!(cx.read(|app| root.read(app).export.is_some()));

	// Replace the live session with a synthetic, test-driven one.
	let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
	let (tx, rx) = mpsc::channel::<crate::oakui::ExportEvent>();
	let cancelled_for_session = cancelled.clone();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.export = Some(ExportRun {
				session: ExportSession {
					events: rx,
					cancel: Box::new(move || {
						cancelled_for_session.store(true, std::sync::atomic::Ordering::SeqCst)
					}),
				},
			});
		})
	});

	let tick = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
		cx.run_until_parked();
	};

	// Empty drain is a no-op; Started is ignored; Progress moves the
	// bar without closing.
	tick(cx);
	tx.send(crate::oakui::ExportEvent::Started).unwrap();
	tx.send(crate::oakui::ExportEvent::Progress(0.4)).unwrap();
	tick(cx);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Progress { .. })),
		"progress keeps the dialog open"
	);
	assert!(cx.read(|app| root.read(app).export.is_some()));

	// The Cancel button runs the session's cancel handle.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::EXPORT_PROGRESS,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(
		cancelled.load(std::sync::atomic::Ordering::SeqCst),
		"the progress dialog's Cancel runs the session cancel"
	);

	// A failed finish closes the dialog and drops the session.
	tx.send(crate::oakui::ExportEvent::Finished(false, "boom".into()))
		.unwrap();
	tick(cx);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	assert!(!cx.read(|app| root.read(app).export.is_some()));

	// The success path: reopen, replace the session, finish ok.
	open_export(cx);
	click_export_ok(cx);
	let (tx_ok, rx_ok) = mpsc::channel::<crate::oakui::ExportEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.export = Some(ExportRun {
				session: ExportSession {
					events: rx_ok,
					cancel: Box::new(|| {}),
				},
			});
		})
	});
	tx_ok
		.send(crate::oakui::ExportEvent::Finished(true, String::new()))
		.unwrap();
	tick(cx);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	assert!(!cx.read(|app| root.read(app).export.is_some()));

	// Escape on the progress dialog cancels and closes.
	open_export(cx);
	click_export_ok(cx);
	let escaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
	let escaped_for_session = escaped.clone();
	let (_tx_escape, rx_escape) = mpsc::channel::<crate::oakui::ExportEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.export = Some(ExportRun {
				session: ExportSession {
					events: rx_escape,
					cancel: Box::new(move || {
						escaped_for_session.store(true, std::sync::atomic::Ordering::SeqCst)
					}),
				},
			});
		})
	});
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::EXPORT_PROGRESS,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(
		escaped.load(std::sync::atomic::Ordering::SeqCst),
		"Escape runs the session cancel"
	);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// The settings dialog's Cancel button just closes; Escape (a
	// dismissed modal) falls through the catch-all close.
	open_export(cx);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::EXPORT,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// A project with no sequences reports instead of opening the
	// dialog: empty the selection's sequence list.
	open_export(cx);
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Export { content, .. } => content.clone(),
		_ => panic!("the export dialog should be open"),
	});
	cx.update(|app| {
		content.update(app, |content, cx| {
			content.set_sequences(Vec::new(), None, cx)
		})
	});
	click_export_ok(cx);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Export { .. })),
		"a dialog without a selected sequence does not start an export"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
}

/// The shell's global action handler: every wired arm runs, the state
/// each handler touches actually moves, and the placeholders / unknown
/// actions fall through harmlessly.
#[gpui_kit::test]
async fn global_action_dispatch_runs_every_handler_arm(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	let dispatch = |cx: &mut TestAppContext, action: ActionId| {
		cx.update(|app| root.update(app, |app, cx| app.dispatch_action_id(action, cx)));
		cx.run_until_parked();
	};
	let close_modal = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
		cx.run_until_parked();
	};
	let playing =
		|cx: &TestAppContext| cx.read(|app| root.read(app).program_clock.read(app).is_playing());
	let frame =
		|cx: &TestAppContext| cx.read(|app| root.read(app).program_clock.read(app).current_frame());
	let workarea = |cx: &TestAppContext| cx.read(|app| root.read(app).engine.read(app).workarea());
	let track_count =
		|cx: &TestAppContext| cx.read(|app| root.read(app).engine.read(app).track_count());

	// --- edit / history -----------------------------------------------
	dispatch(cx, ActionId::Undo);
	dispatch(cx, ActionId::Redo);
	let (undo, redo) = cx.read(|app| root.read(app).engine.read(app).undo_redo_calls());
	assert!(undo >= 1 && redo >= 1, "undo/redo reached the engine");
	dispatch(cx, ActionId::Delete);
	dispatch(cx, ActionId::RippleDelete);
	dispatch(cx, ActionId::SplitAtPlayhead);
	dispatch(cx, ActionId::SelectAll);
	// SetWorkArea over a non-empty selection takes the selected clips'
	// bounding range.
	dispatch(cx, ActionId::SetWorkArea);
	assert!(workarea(cx).is_some(), "a selection sets the work area");
	// Delete with a selection reaches the engine per clip.
	dispatch(cx, ActionId::Delete);
	dispatch(cx, ActionId::DeselectAll);

	// --- transport -----------------------------------------------------
	dispatch(cx, ActionId::NextFrame);
	assert_eq!(frame(cx), Frame(1));
	dispatch(cx, ActionId::PrevFrame);
	assert_eq!(frame(cx), Frame(0));
	dispatch(cx, ActionId::ShuttleRight);
	assert!(playing(cx), "shuttle right plays");
	dispatch(cx, ActionId::ShuttleStop);
	assert!(!playing(cx), "shuttle stop pauses");
	dispatch(cx, ActionId::ShuttleLeft);
	dispatch(cx, ActionId::PlayPause);
	assert!(playing(cx));
	dispatch(cx, ActionId::PlayPause);
	assert!(!playing(cx));
	dispatch(cx, ActionId::GoToEnd);
	let end = frame(cx);
	assert!(end.0 > 0, "goto end seeks past zero");
	dispatch(cx, ActionId::GoToStart);
	assert_eq!(frame(cx), Frame(0));

	// --- in / out points ------------------------------------------------
	for _ in 0..5 {
		dispatch(cx, ActionId::NextFrame);
	}
	dispatch(cx, ActionId::SetInPoint);
	assert_eq!(workarea(cx).map(|(start, _)| start), Some(Frame(5)));
	dispatch(cx, ActionId::GoToIn);
	assert_eq!(frame(cx), Frame(5));
	for _ in 0..3 {
		dispatch(cx, ActionId::NextFrame);
	}
	dispatch(cx, ActionId::SetOutPoint);
	let area = workarea(cx).expect("out point set");
	assert_eq!(area.1, Frame(8), "out point lands at the playhead");
	dispatch(cx, ActionId::GoToOut);
	assert_eq!(frame(cx), Frame(8));
	dispatch(cx, ActionId::PlayInToOut);
	assert_eq!(
		frame(cx),
		Frame(5),
		"play-in-to-out seeks the in point first"
	);
	assert!(playing(cx));
	dispatch(cx, ActionId::ShuttleStop);

	// An empty range is ignored (the guard), and the no-workarea
	// fallbacks are covered by the second pass.
	dispatch(cx, ActionId::ClearInOut);
	assert!(workarea(cx).is_none());
	dispatch(cx, ActionId::GoToStart);
	dispatch(cx, ActionId::SetOutPoint);
	assert!(workarea(cx).is_none(), "an empty out range is ignored");
	dispatch(cx, ActionId::GoToIn);
	dispatch(cx, ActionId::GoToOut);
	dispatch(cx, ActionId::PlayInToOut);
	dispatch(cx, ActionId::ShuttleStop);

	// --- markers --------------------------------------------------------
	dispatch(cx, ActionId::GoToStart);
	dispatch(cx, ActionId::Marker);
	assert!(
		cx.read(|app| root
			.read(app)
			.engine
			.read(app)
			.markers()
			.iter()
			.any(|marker| marker.frame == Frame(0))),
		"marker added at the playhead"
	);
	dispatch(cx, ActionId::RemoveMarker);
	assert!(
		cx.read(|app| root.read(app).engine.read(app).markers().is_empty()),
		"marker removed at the playhead"
	);

	// --- sequence -------------------------------------------------------
	let before = track_count(cx);
	dispatch(cx, ActionId::AddVideoTrack);
	dispatch(cx, ActionId::AddAudioTrack);
	assert_eq!(track_count(cx), before + 2);
	dispatch(cx, ActionId::RemoveTrack);
	assert_eq!(
		track_count(cx),
		before + 2,
		"remove with no selected track is a no-op"
	);
	dispatch(cx, ActionId::SetWorkArea);
	assert!(
		workarea(cx).is_some(),
		"set work area commits at the playhead"
	);

	// --- view -----------------------------------------------------------
	let zoom = |cx: &TestAppContext| cx.read(|app| root.read(app).timeline.read(app).state.zoom);
	let zoom_before = zoom(cx);
	dispatch(cx, ActionId::ZoomIn);
	assert!(zoom(cx) > zoom_before);
	dispatch(cx, ActionId::ZoomOut);
	let height = |cx: &TestAppContext| {
		cx.read(|app| {
			root.read(app)
				.engine
				.read(app)
				.track(0)
				.map(|track| f32::from(track.height()))
				.unwrap_or_default()
		})
	};
	let height_before = height(cx);
	dispatch(cx, ActionId::IncreaseTrackHeight);
	assert_eq!(height(cx), height_before + 8.0);
	dispatch(cx, ActionId::DecreaseTrackHeight);
	assert_eq!(height(cx), height_before);
	// 显示全部 zooms the timeline strip to fit the sequence.
	let timeline_zoom = |cx: &mut TestAppContext| {
		cx.read(|app| {
			root.read(app)
				.panels
				.timeline
				.read(app)
				.test_timeline()
				.read(app)
				.state
				.zoom
		})
	};
	let zoom_before = timeline_zoom(cx);
	dispatch(cx, ActionId::ToggleShowAll);
	let zoom_after = timeline_zoom(cx);
	assert_ne!(
		zoom_before, zoom_after,
		"show-all fits the sequence (zoom changes)"
	);
	dispatch(cx, ActionId::Loop);
	assert!(cx.read(|app| root.read(app).loop_playback));
	dispatch(cx, ActionId::Loop);
	assert!(!cx.read(|app| root.read(app).loop_playback));

	// The theme handlers persist through the config store: pin and
	// restore the key.
	let _theme = ConfigGuard::pin(crate::oakui::real::CONFIG_KEY_THEME);
	dispatch(cx, ActionId::ThemeLight);
	assert!(!cx.read(|app| root.read(app).dark));
	dispatch(cx, ActionId::ThemeDark);
	assert!(cx.read(|app| root.read(app).dark));

	// --- tools ----------------------------------------------------------
	dispatch(cx, ActionId::Snapping);
	dispatch(cx, ActionId::Snapping);
	for tool in [
		Tool::Pointer,
		Tool::TrackSelect,
		Tool::Edit,
		Tool::Ripple,
		Tool::Rolling,
		Tool::Razor,
		Tool::Slip,
		Tool::Slide,
		Tool::Hand,
		Tool::Zoom,
		Tool::Transition,
		Tool::Add,
		Tool::Record,
	] {
		dispatch(cx, tool.action());
		assert_eq!(cx.read(|app| root.read(app).active_tool), tool);
	}
	let proxy_before = cx.read(|app| root.read(app).engine.read(app).use_proxy_media());
	dispatch(cx, ActionId::UseProxyMedia);
	assert_ne!(
		cx.read(|app| root.read(app).engine.read(app).use_proxy_media()),
		proxy_before
	);
	dispatch(cx, ActionId::UseProxyMedia);

	// --- window panels --------------------------------------------------
	for (panel, action) in WINDOW_PANELS {
		let visible = cx.read(|app| dock_panel_open(app, &root, panel));
		dispatch(cx, action);
		assert_ne!(
			cx.read(|app| dock_panel_open(app, &root, panel)),
			visible,
			"the window action toggles the panel"
		);
		dispatch(cx, action);
		assert_eq!(
			cx.read(|app| dock_panel_open(app, &root, panel)),
			visible,
			"a second click restores the panel"
		);
	}
	dispatch(cx, ActionId::ResetDefaultLayout);

	// --- placeholders / uninplemented fall-throughs ---------------------
	for action in [
		ActionId::Cut,
		ActionId::Copy,
		ActionId::Paste,
		ActionId::NudgeLeft,
		ActionId::GoToPrevCut,
		ActionId::GoToNextCut,
		ActionId::MaximizePanel,
		ActionId::SequenceSettings,
		ActionId::FullScreenViewer,
		ActionId::ClearOpenRecent,
		ActionId::Revert,
		ActionId::MulticamSwitch1,
		ActionId::MulticamSwitchNoSplit9,
		ActionId::AddEmpty,
	] {
		dispatch(cx, action);
	}
	// The mock has no project folder support: the engine error is
	// logged, the shell keeps running.
	dispatch(cx, ActionId::NewFolder);
	dispatch(cx, ActionId::CloseProject);

	// --- modal-opening actions ------------------------------------------
	for action in [
		ActionId::ProjectProperties,
		ActionId::ProxySettings,
		ActionId::MulticamWizard,
		ActionId::Preferences,
		ActionId::ActionSearch,
		ActionId::About,
		ActionId::NewProject,
		ActionId::NewSequence,
		ActionId::Export,
		ActionId::SaveProject,
		ActionId::SaveProjectAs,
	] {
		dispatch(cx, action);
		assert!(
			cx.read(|app| root.read(app).modal.modal_entity().is_some()),
			"{action:?} opens a modal"
		);
		close_modal(cx);
	}

	// The path pickers: Import allows multiple files, Open is
	// single-file. (gpui-pre's PathPromptOptions has no extension
	// filter — the fork's `allowed_extensions` is gone.)
	dispatch(cx, ActionId::Import);
	assert!(cx.did_prompt_for_paths());
	cx.simulate_path_prompt_response(|options| {
		assert!(options.multiple, "import allows multiple files");
		None
	});
	cx.run_until_parked();
	dispatch(cx, ActionId::OpenProject);
	assert!(cx.did_prompt_for_paths());
	cx.simulate_path_prompt_response(|options| {
		assert!(!options.multiple, "open is single-file");
		None
	});
	cx.run_until_parked();

	// The manager entries share the manager modal.
	for action in [ActionId::OpenFromLibrary, ActionId::ProjectManager] {
		dispatch(cx, action);
		assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));
		close_modal(cx);
	}

	// Full screen: toggles the real window state and the checkmark.
	dispatch(cx, ActionId::FullScreen);
	assert!(cx.read(|app| root.read(app).full_screen));
	dispatch(cx, ActionId::FullScreen);
	assert!(!cx.read(|app| root.read(app).full_screen));

	// A draw with the shell still live must not panic.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// Focused-panel routing: the panel the dock reports last gets the
/// command first, the transport panels handle their commands, every
/// other panel declines (the shell's global handler runs), and an id
/// outside the nine declines straight away.
#[gpui_kit::test]
async fn focused_panel_routing_reaches_every_panel(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let focus = |cx: &mut TestAppContext, id: PanelId| {
		cx.update(|app| root.update(app, |app, _cx| app.focused_panel = Some(id)));
	};
	let handles = |cx: &mut TestAppContext, action: ActionId| -> bool {
		cx.update(|app| root.update(app, |app, cx| app.dispatch_to_focused_panel(action, cx)))
	};
	let pause = |cx: &mut TestAppContext| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.engine
					.update(cx, |engine, cx| engine.pause(Monitor::Program, cx));
			})
		});
	};

	// No focused panel yet: every command falls through.
	assert!(!handles(cx, ActionId::PlayPause));
	assert!(!handles(cx, ActionId::MulticamSwitch1));

	for (id, handles_transport) in [
		(PROJECT, false),
		(SOURCE_VIEWER, true),
		(PROGRAM_VIEWER, true),
		(NODE_EDITOR, false),
		(INSPECTOR, false),
		(HISTORY, false),
		(TIMELINE, true),
		(EFFECT_LIBRARY, false),
		(MULTICAM, false),
	] {
		focus(cx, id);
		assert_eq!(cx.read(|app| root.read(app).focused_panel), Some(id));
		assert_eq!(
			handles(cx, ActionId::PlayPause),
			handles_transport,
			"PlayPause handling for panel {}",
			id
		);
		pause(cx);
	}

	// The timeline handles the editing commands the others decline.
	focus(cx, TIMELINE);
	assert!(handles(cx, ActionId::SelectAll));
	assert!(handles(cx, ActionId::DeselectAll));
	focus(cx, PROJECT);
	assert!(!handles(cx, ActionId::SelectAll));

	// The multicam panel handles the source-switch hotkeys; the
	// timeline declines them (the shell's fall-through no-op).
	focus(cx, MULTICAM);
	assert!(handles(cx, ActionId::MulticamSwitch1));
	assert!(handles(cx, ActionId::MulticamSwitchNoSplit9));
	focus(cx, TIMELINE);
	assert!(!handles(cx, ActionId::MulticamSwitch1));

	// An id outside the nine panel ids declines.
	focus(cx, "not-a-panel");
	assert!(!handles(cx, ActionId::PlayPause));
}

/// The shell's subscriptions: a menu-bar trigger routes through
/// `on_menu`, the timeline's selection event reaches the engine, the
/// project explorer's requests open their dialogs / call the engine,
/// the viewer panels' events drive the work area and full screen, and a
/// project panel's context-menu trigger points the focused-panel routing
/// at that panel before dispatching.
#[gpui_kit::test]
async fn shell_subscriptions_route_panel_events(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let close_modal = |cx: &mut TestAppContext| {
		cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
		cx.run_until_parked();
	};
	let step_to = |cx: &mut TestAppContext, frame: i64| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.engine.update(cx, |engine, cx| {
					engine.request_frame(Monitor::Program, Frame(frame), cx)
				});
			})
		});
		cx.run_until_parked();
	};

	// The menu bar's own Triggered event routes through the shell's
	// `on_menu` (the same path a click takes).
	cx.update(|app| {
		root.update(app, |app, cx| {
			let menu_bar = app.menu_bar.clone();
			menu_bar.update(cx, |_bar, cx| {
				cx.emit(MenuBarEvent::Triggered {
					control: 1,
					item: menu_ids::FOCUS_HISTORY,
					label: "窗口".into(),
				});
			});
		});
	});
	cx.run_until_parked();
	assert!(
		!cx.read(|app| dock_panel_open(app, &root, HISTORY)),
		"the menu-bar trigger toggled the history panel"
	);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(menu_ids::FOCUS_HISTORY, cx);
		})
	});
	cx.run_until_parked();

	// The timeline's SelectionChanged forwards the widget selection to
	// the engine and applies the event.
	cx.update(|app| {
		root.update(app, |app, cx| {
			let timeline = app.timeline.clone();
			timeline.update(cx, |view, cx| {
				view.state.select_range([ClipId(1)]);
				cx.emit(TimelineEvent::SelectionChanged);
			});
		});
	});
	cx.run_until_parked();
	assert!(
		cx.read(|app| root
			.read(app)
			.timeline
			.read(app)
			.state
			.is_selected(ClipId(1))),
		"the selection survives the event round trip"
	);

	// Project explorer requests.
	let project = cx.read(|app| root.read(app).panels.project.clone());
	// Delete goes straight to the engine.
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::DeleteRequested(3))
		});
	});
	cx.run_until_parked();
	// Rename opens the rename prompt for the entry.
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::RenameRequested(3))
		});
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(
		&root.read(app).modal,
		ModalState::EntryRename { entry_id: 3, .. }
	)));
	close_modal(cx);
	// A sequence's properties dialog.
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::SequencePropertiesRequested(4))
		});
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::SequenceProperties { .. })));
	close_modal(cx);
	// Export-sequence opens the export dialog with that entry picked.
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::ExportSequenceRequested(4))
		});
	});
	cx.run_until_parked();
	let selected = cx.read(|app| match &root.read(app).modal {
		ModalState::Export { content, .. } => content.read(app).selected_sequence(app),
		_ => None,
	});
	assert_eq!(selected, Some(4));
	close_modal(cx);
	// New sequence opens the seeded dialog.
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::NewSequenceRequested)
		});
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::NewSequence { .. })));
	close_modal(cx);
	// Add text footage calls the engine.
	let text_before = cx.read(|app| root.read(app).engine.read(app).text_footages().len());
	cx.update(|app| {
		project.update(app, |_panel, cx| {
			cx.emit(crate::panels::project_explorer::NewTextFootageRequested)
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| root.read(app).engine.read(app).text_footages().len()),
		text_before + 1
	);

	// The viewer panels' shell-level events act on the shared work area
	// (both monitors) and the window's full screen.
	let workarea = |cx: &TestAppContext| cx.read(|app| root.read(app).engine.read(app).workarea());
	{
		let source_viewer = cx.read(|app| root.read(app).panels.source_viewer.clone());
		step_to(cx, 5);
		cx.update(|app| {
			source_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::SetInPoint)
			})
		});
		cx.run_until_parked();
		assert_eq!(workarea(cx).map(|(start, _)| start), Some(Frame(5)));
		step_to(cx, 9);
		cx.update(|app| {
			source_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::SetOutPoint)
			})
		});
		cx.run_until_parked();
		assert_eq!(workarea(cx).map(|(_, end)| end), Some(Frame(9)));
		cx.update(|app| {
			source_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::ClearRange)
			})
		});
		cx.run_until_parked();
		assert!(workarea(cx).is_none(), "ClearRange clears the work area");
		cx.update(|app| {
			source_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::FullScreenRequested)
			})
		});
		cx.run_until_parked();
		// Toggle back so the window state stays neutral.
		cx.update(|app| root.update(app, |app, cx| app.toggle_full_screen(cx)));
		cx.run_until_parked();
	}
	{
		let program_viewer = cx.read(|app| root.read(app).panels.program_viewer.clone());
		step_to(cx, 7);
		cx.update(|app| {
			program_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::SetInPoint)
			})
		});
		cx.run_until_parked();
		assert_eq!(workarea(cx).map(|(start, _)| start), Some(Frame(7)));
		cx.update(|app| {
			program_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::ClearRange)
			})
		});
		cx.run_until_parked();
		cx.update(|app| {
			program_viewer.update(app, |_panel, cx| {
				cx.emit(menu::ViewerPanelEvent::FullScreenRequested)
			})
		});
		cx.run_until_parked();
		cx.update(|app| root.update(app, |app, cx| app.toggle_full_screen(cx)));
		cx.run_until_parked();
	}

	// A context-menu trigger points the focused-panel target at the
	// panel that was right-clicked, then dispatches like a menu click.
	assert!(cx.read(|app| dock_panel_open(app, &root, INSPECTOR)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			let project = app.panels.project.clone();
			project.update(cx, |_panel, cx| {
				cx.emit(menu::ContextMenuTriggered {
					item: menu_ids::FOCUS_INSPECTOR,
				});
			});
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| root.read(app).focused_panel),
		Some(PROJECT),
		"the right-clicked panel becomes the routing target"
	);
	assert!(
		!cx.read(|app| dock_panel_open(app, &root, INSPECTOR)),
		"the triggered item dispatched through the menu path"
	);
}

/// `on_menu` special-cases the dynamic language items (index-based,
/// switching the UI language live) and logs unknown ids without
/// dispatching anything.
#[gpui_kit::test]
async fn on_menu_switches_language_and_ignores_unknown_ids(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let languages = crate::i18n::available_languages();
	assert!(languages.len() >= 2, "the test ships at least two packs");
	let first = languages[0].clone();

	// A valid language item index switches the language and rebuilds
	// the menu bar in the new language.
	let menu_before = cx.read(|app| root.read(app).menu_bar.entity_id());
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu::LANG_ITEM_BASE, cx)));
	cx.run_until_parked();
	assert_eq!(crate::i18n::language_code(), first);
	assert_ne!(
		cx.read(|app| root.read(app).menu_bar.entity_id()),
		menu_before,
		"the language switch rebuilds the menu bar"
	);

	// An index past the discovered packs returns early without
	// switching (still inside the language range).
	let current = crate::i18n::language_code();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_menu(menu::LANG_ITEM_BASE + languages.len() + 4, cx)
		})
	});
	cx.run_until_parked();
	assert_eq!(crate::i18n::language_code(), current);

	// Unknown ids (outside the language range, and the hidden menu id
	// the multicam hotkeys use) are logged and ignored.
	let menu_before = cx.read(|app| root.read(app).menu_bar.entity_id());
	for id in [usize::MAX, crate::actions::HIDDEN_MENU_ID, 5_000] {
		cx.update(|app| root.update(app, |app, cx| app.on_menu(id, cx)));
		cx.run_until_parked();
	}
	assert_eq!(
		cx.read(|app| root.read(app).menu_bar.entity_id()),
		menu_before,
		"unknown menu ids do not rebuild the menu bar"
	);

	crate::i18n::set_language_code("en-US");
}

/// `on_file_paths` routes every [`FileAction`] through the engine: open
/// / import (multiple, empty, and manager import) / project export /
/// export-to-file, including the early-return guards and the manager
/// error path.
#[gpui_kit::test]
async fn on_file_paths_routes_every_file_action(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let paths = |cx: &mut TestAppContext, action: FileAction, picked: Vec<PathBuf>| {
		cx.update(|app| root.update(app, |app, cx| app.on_file_paths(action, picked, cx)));
		cx.run_until_parked();
	};
	let imported = |cx: &TestAppContext| {
		cx.read(|app| root.read(app).engine.read(app).imported_footage().to_vec())
	};

	// Import: every picked path lands in the engine; an empty list is a
	// silent no-op.
	paths(
		cx,
		FileAction::ImportFootage,
		vec![PathBuf::from("/media/a.mov"), PathBuf::from("/media/b.wav")],
	);
	assert_eq!(imported(cx).len(), 2);
	paths(cx, FileAction::ImportFootage, Vec::new());
	assert_eq!(imported(cx).len(), 2);

	// Open: the first path becomes the project; an empty list no-ops.
	paths(
		cx,
		FileAction::Open,
		vec![PathBuf::from("/projects/opened.ove")],
	);
	let name = cx.read(|app| {
		root.read(app)
			.engine
			.read(app)
			.project()
			.map(|project| project.name.clone())
			.unwrap_or_default()
	});
	assert_eq!(name, "opened");
	paths(cx, FileAction::Open, Vec::new());

	// Export to file: routed to the engine (the mock logs it).
	paths(
		cx,
		FileAction::ExportProjectFile,
		vec![PathBuf::from("/projects/out.ove")],
	);
	paths(cx, FileAction::ExportProjectFile, Vec::new());

	// Manager import: a valid path reloads the manager list; a path
	// with no file stem fails and stays in the manager; an empty list
	// returns early.
	let content = open_manager(cx, &root);
	let before = manager_rows(cx, &content).len();
	paths(
		cx,
		FileAction::ImportProject,
		vec![PathBuf::from("/library/新片.ove")],
	);
	assert_eq!(manager_rows(cx, &content).len(), before + 1);
	paths(cx, FileAction::ImportProject, vec![PathBuf::from("/")]);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })),
		"a failed import keeps the manager open"
	);
	paths(cx, FileAction::ImportProject, Vec::new());

	// Manager export: missing pending row / missing path are early
	// returns; a known row records the export; an unknown row reports
	// in the manager's status (the manager stays open).
	paths(
		cx,
		FileAction::ExportProject,
		vec![PathBuf::from("/library/row.ove")],
	);
	cx.update(|app| root.update(app, |app, _cx| app.pending_export = Some("missing".into())));
	paths(cx, FileAction::ExportProject, Vec::new());
	cx.update(|app| root.update(app, |app, _cx| app.pending_export = Some("missing".into())));
	paths(
		cx,
		FileAction::ExportProject,
		vec![PathBuf::from("/library/bad.ove")],
	);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));
	let row_uuid = manager_rows(cx, &content)[0].uuid.clone();
	cx.update(|app| root.update(app, |app, _cx| app.pending_export = Some(row_uuid.clone())));
	paths(
		cx,
		FileAction::ExportProject,
		vec![PathBuf::from("/library/good.ove")],
	);
	let exported = cx.read(|app| root.read(app).engine.read(app).library_exported().to_vec());
	assert_eq!(
		exported,
		vec![(row_uuid, PathBuf::from("/library/good.ove"))]
	);

	// The generic file dialog ignores the manager's export action (its
	// prompt path lives in `open_manager_export`).
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.open_file_dialog(FileAction::ExportProject, cx)
		})
	});
	cx.run_until_parked();
	assert!(!cx.did_prompt_for_new_path());
}

/// The manager's error / back-out paths: an invalid uuid keeps the
/// manager open with a status line (open / duplicate), the rename and
/// delete confirmations return to the manager on both confirm and
/// cancel, and the manager helpers are no-ops without a manager.
#[gpui_kit::test]
async fn manager_error_paths_and_back_navigation(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	use crate::manager::ManagerEvent;

	let content = open_manager(cx, &root);

	// Re-entrant open just reloads the list.
	cx.update(|app| root.update(app, |app, cx| app.show_project_manager(cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Opening an unknown row fails and keeps the manager.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Open("no-such-row".into()), cx)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Duplicating an unknown row fails the same way.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Duplicate("no-such-row".into()), cx)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Rename: the prompt is prefilled with the selected row's name;
	// an empty value backs out without an engine call.
	let uuid = manager_rows(cx, &content)[0].uuid.clone();
	cx.update(|app| content.update(app, |manager, cx| manager.select(&uuid, cx)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Rename(uuid.clone()), cx)
		})
	});
	cx.run_until_parked();
	let prompt = cx.read(|app| match &root.read(app).modal {
		ModalState::ManagerRename {
			content,
			uuid: prompt_uuid,
			..
		} => {
			assert_eq!(prompt_uuid, &uuid);
			content.clone()
		}
		_ => panic!("the rename prompt should be open"),
	});
	assert_eq!(
		cx.read(|app| prompt.read(app).value(app).to_string()),
		manager_rows(cx, &content)[0].name,
		"the prompt is prefilled with the selected name"
	);
	cx.update_window(window.into(), |_, window, app| {
		prompt.update(app, |p, cx| p.set_value("", window, cx));
	})
	.expect("window open");
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_RENAME,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Rename of an unknown uuid: the error backs out to the manager.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Rename("no-such-row".into()), cx)
		})
	});
	cx.run_until_parked();
	let prompt = cx.read(|app| match &root.read(app).modal {
		ModalState::ManagerRename { content, .. } => content.clone(),
		_ => panic!("the rename prompt should be open"),
	});
	cx.update_window(window.into(), |_, window, app| {
		prompt.update(app, |p, cx| p.set_value("改", window, cx));
	})
	.expect("window open");
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_RENAME,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Cancelling the rename prompt returns to the manager.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Rename(uuid.clone()), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_RENAME,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Delete of an unknown uuid reports and backs out.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Delete("no-such-row".into()), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_DELETE,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// Cancelling the delete confirmation also returns to the manager.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_manager_event(&ManagerEvent::Delete(uuid.clone()), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::MANAGER_DELETE,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));

	// With no manager, the helpers are silent no-ops.
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
	cx.update(|app| root.update(app, |app, cx| app.manager_status("orphan".into(), cx)));
	cx.update(|app| root.update(app, |app, cx| app.reload_manager(cx)));
	assert!(
		cx.read(|app| root.read(app).manager_selected_name(app))
			.is_none()
	);
}

/// The preferences dialog's events reach the shell: theme changes apply,
/// language / shortcut changes rebuild the menu bar, display-color
/// changes re-tag the windows, and closing commits the dialog's text
/// fields.
#[gpui_kit::test]
async fn preferences_events_reach_the_shell(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _theme = ConfigGuard::pin(crate::oakui::real::CONFIG_KEY_THEME);
	// Deterministic language for the tab-title refresh assertion below
	// (the panels snapshot their localized titles at registration).
	crate::i18n::set_language_code("en-US");
	let (_window, root) = mock_shell(cx);
	crate::actions::reset_all_custom_shortcuts();

	cx.update(|app| root.update(app, |app, cx| app.open_preferences(cx)));
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::Preferences { content, .. } => content.clone(),
		_ => panic!("the preferences modal should be open"),
	});

	// ThemeChanged applies the theme immediately.
	cx.update(|app| {
		content.update(app, |_content, cx| {
			cx.emit(crate::dialogs::PreferencesEvent::ThemeChanged(false))
		})
	});
	cx.run_until_parked();
	assert!(!cx.read(|app| root.read(app).dark));
	cx.update(|app| {
		content.update(app, |_content, cx| {
			cx.emit(crate::dialogs::PreferencesEvent::ThemeChanged(true))
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| root.read(app).dark));

	// LanguageChanged rebuilds the menu bar; the dock's tab titles
	// render live from i18n under gpui-kit (no snapshot to refresh).
	let menu_before = cx.read(|app| root.read(app).menu_bar.entity_id());
	crate::i18n::set_language_code("zh-CN");
	cx.update(|app| {
		content.update(app, |_content, cx| {
			cx.emit(crate::dialogs::PreferencesEvent::LanguageChanged)
		})
	});
	cx.run_until_parked();
	assert_ne!(
		cx.read(|app| root.read(app).menu_bar.entity_id()),
		menu_before
	);
	let project_tab = cx.read(|app| root.read(app).panels.project.read(app).panel_name());
	assert_eq!(project_tab, "project");
	crate::i18n::set_language_code("en-US");

	// ShortcutsChanged re-binds the key map and rebuilds the menu bar.
	crate::actions::set_custom_shortcut("snapping", vec!["f5".to_string()]);
	let menu_before = cx.read(|app| root.read(app).menu_bar.entity_id());
	cx.update(|app| {
		content.update(app, |_content, cx| {
			cx.emit(crate::dialogs::PreferencesEvent::ShortcutsChanged)
		})
	});
	cx.run_until_parked();
	assert_ne!(
		cx.read(|app| root.read(app).menu_bar.entity_id()),
		menu_before
	);
	assert_eq!(
		crate::actions::effective_keys(ActionId::Snapping.entry()),
		vec!["f5".to_string()],
		"the override stays live through the rebind"
	);
	crate::actions::reset_all_custom_shortcuts();

	// DisplayColorChanged re-declares the policy on every window.
	cx.update(|app| {
		content.update(app, |_content, cx| {
			cx.emit(crate::dialogs::PreferencesEvent::DisplayColorChanged)
		})
	});
	cx.run_until_parked();

	// The primary button commits + closes; the dismissed event does the
	// same through the other route.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::PREFERENCES,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	cx.update(|app| root.update(app, |app, cx| app.open_preferences(cx)));
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::PREFERENCES,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
}

/// The drop-onto-empty-timeline flow: the choice dialog pauses the drop,
/// "use footage params" resumes it through the engine, the manual path
/// re-stashes it behind the new-sequence dialog, and cancel / Escape
/// abandon it. A drop arriving while another modal is up goes straight
/// to the engine's probe path.
#[gpui_kit::test]
async fn drop_sequence_choice_routes_the_paused_drop(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);
	let drop = FootageDropNeedsSequence {
		footage_id: 3,
		footage_ids: vec![3],
		track_kind: crate::widgets::timeline::TrackKind::Video,
		track_index: 0,
		time: Frame(10),
	};
	let drops =
		|cx: &TestAppContext| cx.read(|app| root.read(app).engine.read(app).footage_drops().len());
	let pending = |cx: &TestAppContext| cx.read(|app| root.read(app).pending_drop.is_some());

	// A modal is already up: the drop resumes through the probe path
	// without a choice dialog.
	cx.update(|app| root.update(app, |app, cx| app.open_preferences(cx)));
	cx.run_until_parked();
	let before = drops(cx);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_footage_drop_needs_sequence(drop.clone(), cx)
		})
	});
	cx.run_until_parked();
	assert_eq!(drops(cx), before + 1);
	assert!(!pending(cx));
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// No modal: the choice dialog pauses the drop.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_footage_drop_needs_sequence(drop.clone(), cx)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::DropSequenceChoice { .. })));
	assert!(pending(cx));

	// "Use footage params": the drop reaches the engine and closes.
	let before = drops(cx);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::DROP_SEQUENCE_CHOICE,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert_eq!(drops(cx), before + 1);
	assert!(!pending(cx));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// "Specify manually": the new-sequence dialog opens with the drop
	// still paused.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_footage_drop_needs_sequence(drop.clone(), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::DROP_SEQUENCE_CHOICE,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::NewSequence { .. })));
	assert!(pending(cx));

	// Cancelling the manual dialog abandons the drop.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::NEW_SEQUENCE,
					button: 1,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(!pending(cx));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// The choice dialog's cancel button abandons the drop too.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_footage_drop_needs_sequence(drop.clone(), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::DROP_SEQUENCE_CHOICE,
					button: 2,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(!pending(cx));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// Escape on the choice dialog abandons the paused drop as well.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_footage_drop_needs_sequence(drop.clone(), cx)
		})
	});
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::DROP_SEQUENCE_CHOICE,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(!pending(cx));

	// Resolving with no paused drop just closes; resuming with none is
	// a no-op.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.resolve_drop_sequence_choice(DropSequenceChoice::UseFootageParams, cx)
		})
	});
	cx.update(|app| root.update(app, |app, cx| app.resume_pending_drop(cx)));

	// Escape from the manual new-sequence dialog (or the choice dialog)
	// abandons a paused drop.
	cx.update(|app| root.update(app, |app, _cx| app.pending_drop = Some(drop.clone())));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::NEW_SEQUENCE,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(!pending(cx));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
}

/// Every guarded dialog opener declines while another modal is up: the
/// preferences modal stays the only modal through the whole sweep.
#[gpui_kit::test]
async fn modal_openers_are_guarded_while_a_modal_is_up(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	cx.update(|app| root.update(app, |app, cx| app.open_preferences(cx)));
	cx.run_until_parked();
	let preferences_still_up = |cx: &TestAppContext| {
		cx.read(|app| matches!(root.read(app).modal, ModalState::Preferences { .. }))
	};
	assert!(preferences_still_up(cx));

	let openers: [AppFn; 12] = [
		|app, cx| app.open_new_project(cx),
		|app, cx| app.open_new_sequence(cx),
		|app, cx| app.open_about(cx),
		|app, cx| app.open_proxy_dialog(cx),
		|app, cx| app.open_project_properties(cx),
		|app, cx| app.open_multicam_wizard(cx),
		|app, cx| app.open_export_project_dfd(cx),
		|app, cx| app.open_save_as(cx),
		|app, cx| app.open_action_search(cx),
		|app, cx| app.open_sequence_properties(4, cx),
		|app, cx| app.open_entry_rename(3, cx),
		|app, cx| app.open_new_sequence(cx),
	];
	for opener in openers {
		cx.update(|app| root.update(app, opener));
		cx.run_until_parked();
		assert!(
			preferences_still_up(cx),
			"a guarded opener must not replace the open modal"
		);
	}

	// Escape closes preferences and the guards lift.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::Dismissed {
					control: modal_ids::PREFERENCES,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
}

/// 窗口 → panel toggles a docked panel closed and back open (its
/// entity — and its state — survive the round trip); unknown panel ids
/// are silent no-ops.
#[gpui_kit::test]
async fn window_toggle_closes_and_reopens_panels(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	assert!(
		cx.read(|app| dock_panel_open(app, &root, TIMELINE)),
		"the timeline starts docked"
	);

	// The 窗口 item removes the panel from the dock.
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_TIMELINE, cx)));
	cx.run_until_parked();
	assert!(
		!cx.read(|app| dock_panel_open(app, &root, TIMELINE)),
		"the timeline is closed"
	);

	// A second click re-opens it (at its design placement — gpui-kit's
	// dock keeps no last-position memory).
	cx.update(|app| root.update(app, |app, cx| app.on_menu(menu_ids::FOCUS_TIMELINE, cx)));
	cx.run_until_parked();
	assert!(
		cx.read(|app| dock_panel_open(app, &root, TIMELINE)),
		"the timeline comes back"
	);

	// An unknown id is a silent no-op through the toggle and dispatch
	// paths.
	cx.update(|app| root.update(app, |app, cx| app.toggle_panel("not-a-panel", cx)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.dispatch_action_id(ActionId::FocusMulticam, cx)
		})
	});
	cx.run_until_parked();
}

/// The modal event router's remaining button / dismissed arms: proxy
/// generate/delete/close, the multicam wizard's create (rejected and
/// accepted), about / project-properties / rename cancellations, the
/// new-project and save-as commits, and the project-file export prompt.
#[gpui_kit::test]
async fn modal_events_route_buttons_and_dismissals(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let click = |cx: &mut TestAppContext, control: usize, button: usize| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.on_modal(&ModalEvent::ButtonClicked { control, button }, cx)
			})
		});
		cx.run_until_parked();
	};
	let dismiss = |cx: &mut TestAppContext, control: usize| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.on_modal(&ModalEvent::Dismissed { control }, cx)
			})
		});
		cx.run_until_parked();
	};
	let is_modal = |cx: &TestAppContext, f: fn(&ModalState<MockEngine>) -> bool| {
		cx.read(|app| f(&root.read(app).modal))
	};

	// --- proxy dialog: generate, delete, then the Close apply path -----
	cx.update(|app| root.update(app, |app, cx| app.open_proxy_dialog(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::PROXY, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::Proxy { .. })));
	click(cx, modal_ids::PROXY, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::Proxy { .. })));
	click(cx, modal_ids::PROXY, 2);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	// And Escape closes without applying.
	cx.update(|app| root.update(app, |app, cx| app.open_proxy_dialog(cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::PROXY);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- about: OK and the catch-all dismissal --------------------------
	cx.update(|app| root.update(app, |app, cx| app.open_about(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::ABOUT, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_about(cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::ABOUT);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- multicam wizard -------------------------------------------------
	cx.update(|app| root.update(app, |app, cx| app.open_multicam_wizard(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::MULTICAM_WIZARD, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	// One angle is below the two-angle minimum: the create is rejected
	// and the dialog stays up.
	cx.update(|app| root.update(app, |app, cx| app.open_multicam_wizard(cx)));
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::MulticamWizard { content, .. } => content.clone(),
		_ => panic!("the wizard should be open"),
	});
	cx.update(|app| content.update(app, |content, cx| content.toggle_row(0, cx)));
	click(cx, modal_ids::MULTICAM_WIZARD, 0);
	assert!(
		is_modal(cx, |m| matches!(m, ModalState::MulticamWizard { .. })),
		"a single-angle create keeps the wizard open"
	);
	// Two angles: the sync + create succeed and the wizard closes.
	cx.update(|app| content.update(app, |content, cx| content.toggle_row(1, cx)));
	click(cx, modal_ids::MULTICAM_WIZARD, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- project properties: Cancel and Escape leave without applying ---
	cx.update(|app| root.update(app, |app, cx| app.open_project_properties(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::PROJECT_PROPERTIES, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_project_properties(cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::PROJECT_PROPERTIES);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- entry rename: OK applies, Cancel and Escape close --------------
	cx.update(|app| root.update(app, |app, cx| app.open_entry_rename(3, cx)));
	cx.run_until_parked();
	// (RenameContent's field has no public setter; OK applies the
	// prefilled name.)
	click(cx, modal_ids::RENAME, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_entry_rename(3, cx)));
	cx.run_until_parked();
	click(cx, modal_ids::RENAME, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_entry_rename(3, cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::RENAME);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- sequence properties: a rejected commit keeps the dialog open ---
	cx.update(|app| root.update(app, |app, cx| app.open_sequence_properties(4, cx)));
	cx.run_until_parked();
	click(cx, modal_ids::SEQUENCE_PROPERTIES, 0);
	assert!(
		is_modal(cx, |m| matches!(m, ModalState::SequenceProperties { .. })),
		"the mock engine rejects the commit, so the dialog stays"
	);
	click(cx, modal_ids::SEQUENCE_PROPERTIES, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_sequence_properties(4, cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::SEQUENCE_PROPERTIES);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- new project: Cancel leaves the library alone, OK creates ------
	let rows_before = cx.read(|app| {
		root.read(app)
			.engine
			.read(app)
			.library_projects()
			.unwrap()
			.len()
	});
	cx.update(|app| root.update(app, |app, cx| app.open_new_project(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::NEW_PROJECT, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	assert_eq!(
		cx.read(|app| root
			.read(app)
			.engine
			.read(app)
			.library_projects()
			.unwrap()
			.len()),
		rows_before
	);
	cx.update(|app| root.update(app, |app, cx| app.open_new_project(cx)));
	cx.run_until_parked();
	let content = cx.read(|app| match &root.read(app).modal {
		ModalState::NewProject { content, .. } => content.clone(),
		_ => panic!("the new-project dialog should be open"),
	});
	let name = cx.read(|app| content.read(app).name(app).to_string());
	click(cx, modal_ids::NEW_PROJECT, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	assert_eq!(
		cx.read(|app| root
			.read(app)
			.engine
			.read(app)
			.library_projects()
			.unwrap()
			.len()),
		rows_before + 1,
		"OK creates the library row"
	);
	assert_eq!(
		cx.read(|app| root
			.read(app)
			.engine
			.read(app)
			.project()
			.map(|project| project.name.clone())
			.unwrap_or_default()),
		name,
		"the created project is open"
	);

	// --- save as: OK commits (the mock rejects but the dialog closes) ---
	cx.update(|app| root.update(app, |app, cx| app.open_save_as(cx)));
	cx.run_until_parked();
	// (RenameContent's field has no public setter; OK applies the
	// prefilled "<name> Copy" value.)
	click(cx, modal_ids::SAVE_AS, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_save_as(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::SAVE_AS, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- export project file: Cancel, Escape, and the system save path -
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_file_paths(
				FileAction::Open,
				vec![PathBuf::from("/projects/seq.ove")],
				cx,
			)
		})
	});
	cx.run_until_parked();
	cx.update(|app| root.update(app, |app, cx| app.open_export_project_dfd(cx)));
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| root.read(app).export_suggested_name.clone()),
		"seq"
	);
	click(cx, modal_ids::EXPORT_PROJECT_DFD, 1);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_export_project_dfd(cx)));
	cx.run_until_parked();
	dismiss(cx, modal_ids::EXPORT_PROJECT_DFD);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_export_project_dfd(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::EXPORT_PROJECT_DFD, 0);
	assert!(
		cx.did_prompt_for_new_path(),
		"OK asks the system for the output path"
	);
	cx.simulate_new_path_selection(|_directory| Some(PathBuf::from("/projects/export.otio")));
	cx.run_until_parked();
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));

	// --- manager close button / unknown control id ----------------------
	cx.update(|app| root.update(app, |app, cx| app.show_project_manager(cx)));
	cx.run_until_parked();
	click(cx, modal_ids::MANAGER, 0);
	assert!(is_modal(cx, |m| matches!(m, ModalState::None)));
	cx.update(|app| root.update(app, |app, cx| app.open_about(cx)));
	cx.run_until_parked();
	click(cx, 9_999, 0);
	assert!(
		is_modal(cx, |m| matches!(m, ModalState::About { .. })),
		"an unknown control id is ignored"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
}

/// A shell started with a CLI project path and the persisted light theme:
/// `OakApp::new` takes the light branch, opens the path, and one tick
/// drives the whole animation-frame body.
#[gpui_kit::test]
async fn app_starts_with_an_initial_path_and_the_light_theme(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _theme = ConfigGuard::pin(crate::oakui::real::CONFIG_KEY_THEME);
	crate::oakui::real::set_theme_dark(false);

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1600.0), px(900.0)), |window, cx| {
		OakApp::<MockEngine>::new(window, Some(PathBuf::from("/projects/oak-cli.ove")), cx)
	});
	cx.run_until_parked();
	let root = window.root(cx).expect("app root");

	assert!(
		!cx.read(|app| root.read(app).dark),
		"the persisted light theme wins"
	);
	let name = cx.read(|app| {
		root.read(app)
			.engine
			.read(app)
			.project()
			.map(|project| project.name.clone())
			.unwrap_or_default()
	});
	assert_eq!(name, "oak-cli", "the CLI-provided path was opened");

	// One tick: engine advance, playhead sync, work-area mirror, meter,
	// export + plugin drains, repaint.
	let before = cx.read(|app| root.read(app).program_clock.read(app).current_frame());
	cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
	cx.run_until_parked();
	let after = cx.read(|app| root.read(app).program_clock.read(app).current_frame());
	assert_eq!(before, after, "a paused clock does not move on tick");

	// Let the spawned 60Hz loop fire once: the timer body (monitor poll
	// + tick) runs on the simulated clock.
	cx.executor().advance_clock(Duration::from_millis(40));
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| root.read(app).program_clock.read(app).current_frame()),
		after,
		"the tick-loop body ran without moving a paused clock"
	);

	// Restore the process-global theme colors for the other tests.
	cx.update(|app| root.update(app, |app, cx| app.apply_dark(true, cx)));
	cx.run_until_parked();
}

/// Clicking a track header selects the track, so 序列 → 删除轨道 removes
/// exactly that track through the engine.
#[gpui_kit::test]
async fn removing_a_selected_track_deletes_it(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);
	let mut vcx = VisualTestContext::from_window(window.into(), cx);

	// The track headers column starts at the timeline body's left edge;
	// the first track row sits below the ruler (same geometry the
	// footage-drag test uses).
	let canvas = vcx
		.debug_bounds("timeline-canvas")
		.expect("timeline body rendered");
	let header = gpui_kit::point(
		canvas.left() + px(crate::widgets::timeline::HEADER_WIDTH / 2.0),
		canvas.top() + px(crate::widgets::timeline::RULER_HEIGHT + 20.0),
	);
	vcx.simulate_click(header, gpui_kit::Modifiers::none());
	drop(vcx);
	cx.run_until_parked();

	assert!(
		cx.read(|app| !root
			.read(app)
			.timeline
			.read(app)
			.selected_tracks()
			.is_empty()),
		"the click selected the track"
	);
	let before = cx.read(|app| root.read(app).engine.read(app).track_count());
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.dispatch_action_id(ActionId::RemoveTrack, cx)
		})
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| root.read(app).engine.read(app).track_count()),
		before - 1,
		"the selected track is removed"
	);
}

/// `build_root` constructs the shell entity for the selected backend and
/// opens a CLI-provided path.
#[gpui_kit::test]
async fn build_root_constructs_the_shell(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, _shell) = mock_shell(cx);
	let built = cx
		.update_window(_window.into(), |_root, window, app| {
			build_root::<MockEngine>(window, Some(PathBuf::from("/projects/built.ove")), app)
		})
		.expect("window is still open");
	cx.run_until_parked();
	let name = cx.read(|app| {
		built
			.read(app)
			.engine
			.read(app)
			.project()
			.map(|project| project.name.clone())
			.unwrap_or_default()
	});
	assert_eq!(name, "built", "the built shell opened its initial path");
}

/// The argument parser's help / unknown-flag handling: `--help` / `-h`
/// are skipped, later positionals are logged and ignored, and the
/// `OAK_ENGINE` override matches case-insensitively.
#[test]
fn app_args_help_and_unknown_flags() {
	let parse = |argv: &[&str], env: Option<&str>| {
		AppArgs::parse_args(argv.iter().map(std::ffi::OsString::from), env)
	};

	let a = parse(&["--help", "/tmp/h.ove", "--mock", "--bogus"], None);
	assert_eq!(a.project, Some(PathBuf::from("/tmp/h.ove")));
	assert!(a.mock, "--mock wins even after --help");
	assert!(
		a.project
			.as_ref()
			.unwrap()
			.to_string_lossy()
			.contains("h.ove")
	);

	let b = parse(&["-h", "/tmp/h.ove"], None);
	assert_eq!(b.project, Some(PathBuf::from("/tmp/h.ove")));
	assert!(!b.mock);

	let c = parse(&[], Some("MoCk"));
	assert!(c.mock, "the env override is case-insensitive");
	assert!(c.project.is_none());
}

/// `from_env` reads the process arguments (the test binary's own argv
/// carries no `--help`, or the harness would not run this test).
#[test]
fn app_args_from_env_is_safe_without_flags() {
	let args = AppArgs::from_env();
	// `--mock` may or may not be present depending on the harness — the
	// call must simply parse without exiting.
	let _ = args.project;
}

/// The subscriptions that only exist after a rebuild / on a panel
/// entity: the rebuilt menu bar's trigger, the timeline's
/// footage-drop-needs-sequence request, and the manager content's
/// request events.
#[gpui_kit::test]
async fn rebuilt_subscriptions_route_their_events(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// Rebuild the menu bar (a fresh entity with a fresh subscription)
	// and fire its Triggered event: the item routes through on_menu.
	cx.update(|app| root.update(app, |app, cx| app.rebuild_menu_bar(cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| dock_panel_open(app, &root, HISTORY)));
	cx.update(|app| {
		root.update(app, |app, cx| {
			let menu_bar = app.menu_bar.clone();
			menu_bar.update(cx, |_bar, cx| {
				cx.emit(MenuBarEvent::Triggered {
					control: 1,
					item: menu_ids::FOCUS_HISTORY,
					label: "窗口".into(),
				});
			});
		});
	});
	cx.run_until_parked();
	assert!(
		!cx.read(|app| dock_panel_open(app, &root, HISTORY)),
		"the rebuilt menu bar's trigger reaches on_menu"
	);

	// The timeline panel's footage-drop request pauses the drop behind
	// the choice dialog.
	cx.update(|app| {
		root.update(app, |app, cx| {
			let timeline = app.panels.timeline.clone();
			timeline.update(cx, |_panel, cx| {
				cx.emit(FootageDropNeedsSequence {
					footage_id: 3,
					footage_ids: vec![3],
					track_kind: crate::widgets::timeline::TrackKind::Video,
					track_index: 0,
					time: Frame(0),
				});
			});
		});
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::DropSequenceChoice { .. })));
	assert!(cx.read(|app| root.read(app).pending_drop.is_some()));
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// The manager content emits its requests: Create routes through the
	// shell subscription and closes the dialog.
	let content = open_manager(cx, &root);
	let before = manager_rows(cx, &content).len();
	cx.update(|app| {
		content.update(app, |_manager, cx| {
			cx.emit(crate::manager::ManagerEvent::Create)
		});
	});
	cx.run_until_parked();
	let rows = cx.read(|app| root.read(app).engine.read(app).library_projects().unwrap());
	assert_eq!(
		rows.len(),
		before + 1,
		"the manager's Create reached the engine"
	);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
}

/// Every `on_modal` arm whose modal state does not match is inert (a
/// click on a control with no dialog, a direct confirmation call, an
/// unknown control id), and a dismissed manager sub-dialog with no
/// manager falls back to opening it.
#[gpui_kit::test]
/// Regression: typing into the effect library's search box must not
/// trigger the single-key global hotkeys (s = split, q = ripple-to-in).
/// The shell's bindings are gated on the input exclusion predicate; a
/// focused text field's `Input` key context must keep them out.
#[gpui_kit::test]
async fn typing_in_the_search_box_beats_the_global_hotkeys(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	// Activate the effect library's tab (it starts behind the project
	// explorer), so its search field is rendered and focusable.
	let effect_library_entity = cx.read(|app| root.read(app).panels.effect_library.entity_id());
	cx.update_window(window.into(), |_, window, app| {
		root.update(app, |root, cx| {
			root.dock.update(cx, |dock, cx| {
				dock.select_panel(DockPanelId::from(effect_library_entity), window, cx);
			});
		});
	})
	.expect("window open");
	cx.run_until_parked();
	cx.update_window(window.into(), |_, window, app| {
		let _ = window.draw(app);
	})
	.expect("window open");

	// Focus the effect library's search field and type "sq".
	let search = cx.read(|app| root.read(app).panels.effect_library.read(app).test_search());
	let focus = cx.read(|app| {
		use gpui_kit::Focusable as _;
		search.read(app).focus_handle(app).clone()
	});
	cx.update_window(window.into(), |_, window, app| {
		window.focus(&focus, app);
	})
	.expect("window open");
	cx.run_until_parked();
	let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.simulate_keystrokes("s q");
	cx.run_until_parked();

	// Both letters landed in the field (neither fired its action: the
	// demo engine would log a split; the text is the assertion).
	let text = visual.read(|app| search.read(app).value().to_string());
	assert_eq!(text, "sq", "the letters type into the search box");
}

/// 编辑 → 重命名 on the timeline opens the shell's rename modal; OK
/// applies the name through the engine (one real edit, not a
/// placeholder).
#[gpui_kit::test]
async fn clip_rename_dialog_renames_the_selection(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// Select the first demo clip on the timeline and request the rename.
	let clip = cx.read(|app| {
		let engine = root.read(app).engine.clone();
		let track = engine.read(app).track(0).expect("first track");
		track.clips()[0].id()
	});
	cx.update(|app| {
		root.update(app, |app, cx| {
			let timeline = app.panels.timeline.clone();
			timeline.update(cx, |panel, cx| {
				let view = panel.test_timeline();
				view.update(cx, |t, cx| {
					t.state.clear_selection();
					t.state.select(clip);
					cx.notify();
				});
				crate::panels::commands::PanelCommandHandler::rename_selected(panel, cx);
			});
		});
	});
	cx.run_until_parked();

	// The rename modal is open; type the new name and confirm.
	let prompt = cx.read(|app| match &root.read(app).modal {
		ModalState::ClipRename { content, .. } => content.clone(),
		_ => panic!("the clip rename prompt should be open"),
	});
	cx.update_window(_window.into(), |_, window, app| {
		prompt.update(app, |p, cx| p.set_value("新片段名", window, cx));
	})
	.expect("window open");
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::CLIP_RENAME,
					button: 0,
				},
				cx,
			);
		});
	});
	cx.run_until_parked();

	// The clip carries the new label and the modal is gone.
	let label = cx.read(|app| {
		let engine = root.read(app).engine.clone();
		let track = engine.read(app).track(0).expect("first track");
		track.clips()[0].label().to_string()
	});
	assert_eq!(label, "新片段名");
	assert!(cx.read(|app| root.read(app).modal.modal_entity().is_none()));
}

async fn on_modal_state_mismatches_are_inert(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let click = |cx: &mut TestAppContext, control: usize, button: usize| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.on_modal(&ModalEvent::ButtonClicked { control, button }, cx)
			})
		});
		cx.run_until_parked();
	};
	let dismiss = |cx: &mut TestAppContext, control: usize| {
		cx.update(|app| {
			root.update(app, |app, cx| {
				app.on_modal(&ModalEvent::Dismissed { control }, cx)
			})
		});
		cx.run_until_parked();
	};
	let none =
		|cx: &TestAppContext| cx.read(|app| matches!(root.read(app).modal, ModalState::None));

	for (control, button) in [
		(modal_ids::EXPORT, 0),
		(modal_ids::EXPORT_PROGRESS, 1),
		(modal_ids::PLUGIN_PROGRESS, 1),
		(modal_ids::PREFERENCES, 0),
		(modal_ids::MANAGER, 0),
		(modal_ids::MANAGER_RENAME, 0),
		(modal_ids::MANAGER_DELETE, 0),
		(modal_ids::PROXY, 0),
		(modal_ids::PROJECT_PROPERTIES, 0),
		(modal_ids::NEW_SEQUENCE, 0),
		(modal_ids::MULTICAM_WIZARD, 0),
		(modal_ids::RENAME, 0),
		(modal_ids::EXPORT_PROJECT_DFD, 1),
		(modal_ids::NEW_PROJECT, 0),
		(modal_ids::SAVE_AS, 0),
		(modal_ids::SEQUENCE_PROPERTIES, 0),
		(9_999, 0),
	] {
		click(cx, control, button);
		assert!(none(cx), "control {control} is inert without its modal");
	}

	// The direct confirmation helpers guard on the modal state too.
	cx.update(|app| root.update(app, |app, cx| app.confirm_manager_rename(cx)));
	cx.update(|app| root.update(app, |app, cx| app.confirm_manager_delete(cx)));
	assert!(none(cx));

	// Dismissing a manager sub-dialog with no manager opens it.
	dismiss(cx, modal_ids::MANAGER_RENAME);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));
	dismiss(cx, modal_ids::MANAGER_DELETE);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::Manager { .. })));
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
}

/// The export and plugin progress drains when their dialogs are not the
/// current modal: progress events update nothing and no dialog opens,
/// completion still clears the state.
#[gpui_kit::test]
async fn progress_drains_without_their_dialogs(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// A synthetic export session whose Progress arrives with no
	// progress dialog: the event is drained, the session stays.
	let (tx, rx) = mpsc::channel::<crate::oakui::ExportEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.export = Some(ExportRun {
				session: ExportSession {
					events: rx,
					cancel: Box::new(|| {}),
				},
			});
		});
	});
	tx.send(crate::oakui::ExportEvent::Progress(0.5)).unwrap();
	cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| root.read(app).export.is_some()));
	tx.send(crate::oakui::ExportEvent::Finished(true, String::new()))
		.unwrap();
	cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| root.read(app).export.is_none()));

	// Plugin progress with the open flag set but no progress modal: the
	// fraction updates nothing, no dialog is opened; completion clears
	// the orphaned flag.
	let (ptx, prx) = mpsc::channel::<crate::oakui::ofx::PluginProgressEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.plugin_progress_rx = Mutex::new(prx);
			app.plugin_progress_open = true;
		});
	});
	let event = |fraction: f64| crate::oakui::ofx::PluginProgressEvent {
		label: "OakOFX Render".into(),
		message: "frame".into(),
		fraction,
	};
	ptx.send(event(0.4)).unwrap();
	cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
	cx.run_until_parked();
	assert!(cx.read(|app| root.read(app).plugin_progress_open));
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	ptx.send(event(1.0)).unwrap();
	cx.update(|app| root.update(app, |app, cx| app.tick(cx)));
	cx.run_until_parked();
	assert!(!cx.read(|app| root.read(app).plugin_progress_open));
}

/// Selection-derived work area bounds, the unknown panel's default dock
/// target, the save-as name's extension strip and the export path
/// fallback for a nameless sequence.
#[gpui_kit::test]
async fn selection_bounds_save_as_and_export_defaults(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	// Select every clip: the work-area range is their bounding span.
	cx.update(|app| root.update(app, |app, cx| app.select_all_clips(cx)));
	cx.run_until_parked();
	let range = cx.read(|app| root.read(app).selection_workarea_range(app));
	assert_eq!(range.0, Frame(0));
	assert_eq!(range.1, Frame(600), "the longest demo clip ends at 600");
	// Nothing selected: one frame at the playhead.
	cx.update(|app| root.update(app, |app, cx| app.deselect_all_clips(cx)));
	cx.run_until_parked();
	let range = cx.read(|app| root.read(app).selection_workarea_range(app));
	assert_eq!(range, (Frame(0), Frame(1)));

	// An unknown panel id falls back to the center placement.
	let (placement, size) = default_dock_placement("not-a-panel");
	assert_eq!(placement, DockPlacement::Center);
	assert!(size.is_none());

	// Save-as strips a dotted project name before appending Copy.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.engine.update(cx, |engine, cx| {
				let _ = engine.library_create_project("dotted.name", cx);
			});
		});
	});
	cx.update(|app| root.update(app, |app, cx| app.open_save_as(cx)));
	cx.run_until_parked();
	let name = cx.read(|app| match &root.read(app).modal {
		ModalState::SaveAs { content, .. } => content.read(app).value(app).to_string(),
		_ => panic!("the save-as modal should be open"),
	});
	assert_eq!(name, "dotted Copy");
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// A nameless sequence entry falls back to the project name for the
	// default output path (the created project has no path, so the
	// directory is ".").
	let path = cx.read(|app| {
		root.read(app)
			.default_export_path(app, &[(99, SharedString::from(""))], Some(99))
	});
	assert!(
		path.ends_with(".mp4"),
		"the default path carries the format: {path}"
	);
	assert!(
		path.contains("dotted.name"),
		"the project name is the fallback: {path}"
	);
}

/// The project-file export prompt inside `open_file_dialog` (its own
/// branch, separate from the export-project dialog): the system save
/// path resolves into `on_file_paths` → the engine's export.
#[gpui_kit::test]
async fn open_file_dialog_exports_the_project_file(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	cx.update(|app| {
		root.update(app, |app, cx| {
			app.open_file_dialog(FileAction::ExportProjectFile, cx)
		});
	});
	cx.run_until_parked();
	assert!(
		cx.did_prompt_for_new_path(),
		"the export-project-file action asks for a path"
	);
	cx.simulate_new_path_selection(|directory| Some(directory.join("oak-export.ove")));
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
}

/// A modal opener called from inside a window update defers its build
/// (the nested `update_window` would fail); the deferred half installs
/// the dialog on the next update.
#[gpui_kit::test]
async fn nested_modal_open_defers_to_the_app_update(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (window, root) = mock_shell(cx);

	cx.update_window(window.into(), |_view, _window, app| {
		root.update(app, |app, cx| app.open_about(cx));
	})
	.expect("window is still open");
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::About { .. })),
		"the nested open deferred and then landed"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
}

/// The startup update check end to end: a scripted transport returning a
/// newer release opens the prompt and the primary button opens the
/// downloads page; an equal release and a transport failure stay silent;
/// the preferences toggle skips the fetch entirely; a release found while
/// another modal is up waits for the layer; and the Bug Report action
/// opens the report site.
#[gpui_kit::test]
async fn update_check_prompts_and_respects_the_preferences(cx: &mut TestAppContext) {
	use std::sync::atomic::{AtomicU32, Ordering};

	// Lock order: language -> config (see the preferences tests).
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::oakui::graphops::test_lock();
	crate::i18n::set_language_code("en-US");
	let (_window, root) = mock_shell(cx);

	/// A scripted transport: every call records itself and returns the
	/// canned body/error.
	struct Scripted {
		body: Result<String, String>,
		calls: AtomicU32,
	}
	impl crate::update::UpdateTransport for Scripted {
		fn get(&self, url: &str) -> Result<String, String> {
			assert_eq!(url, crate::update::UPDATE_ENDPOINT);
			self.calls.fetch_add(1, Ordering::SeqCst);
			self.body.clone()
		}
	}
	let set_transport = |cx: &mut TestAppContext, transport: Arc<Scripted>| {
		cx.update(|app| {
			root.update(app, |app, _cx| app.set_update_transport(transport));
		});
	};

	// A newer release prompts; the primary button opens the downloads
	// page and dismisses the dialog.
	let newer = Arc::new(Scripted {
		body: Ok(r#"{"version":"v9.9.9","notes":"New things"}"#.to_string()),
		calls: AtomicU32::new(0),
	});
	set_transport(cx, newer.clone());
	cx.update(|app| root.update(app, |app, cx| app.start_update_check(cx)));
	cx.run_until_parked();
	assert_eq!(newer.calls.load(Ordering::SeqCst), 1, "one fetch");
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::UpdateAvailable { .. })),
		"a newer release opens the prompt"
	);
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::UPDATE_AVAILABLE,
					button: 0,
				},
				cx,
			)
		})
	});
	cx.run_until_parked();
	assert_eq!(
		cx.opened_url().as_deref(),
		Some(crate::update::DOWNLOAD_URL),
		"the primary button sends the user to the downloads page"
	);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// The running version (and a failing transport) stay silent.
	let current = Arc::new(Scripted {
		body: Ok(format!(
			r#"{{"version":"{}"}}"#,
			crate::update::current_version()
		)),
		calls: AtomicU32::new(0),
	});
	set_transport(cx, current.clone());
	cx.update(|app| root.update(app, |app, cx| app.start_update_check(cx)));
	cx.run_until_parked();
	assert_eq!(current.calls.load(Ordering::SeqCst), 1);
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::None)),
		"the current release does not prompt"
	);

	let failing = Arc::new(Scripted {
		body: Err("offline".to_string()),
		calls: AtomicU32::new(0),
	});
	set_transport(cx, failing.clone());
	cx.update(|app| root.update(app, |app, cx| app.start_update_check(cx)));
	cx.run_until_parked();
	assert_eq!(failing.calls.load(Ordering::SeqCst), 1);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));

	// Preferences off: the startup path does not even fetch.
	let previous = crate::oakui::real::config_get_string(crate::update::CONFIG_KEY_CHECK_UPDATES);
	crate::oakui::real::config_set_string(crate::update::CONFIG_KEY_CHECK_UPDATES, "false");
	let disabled = Arc::new(Scripted {
		body: Ok(r#"{"version":"v9.9.9"}"#.to_string()),
		calls: AtomicU32::new(0),
	});
	set_transport(cx, disabled.clone());
	cx.update(|app| root.update(app, |app, cx| app.start_update_check(cx)));
	cx.run_until_parked();
	assert_eq!(
		disabled.calls.load(Ordering::SeqCst),
		0,
		"the toggle off skips the request"
	);
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	crate::oakui::real::config_set_string(crate::update::CONFIG_KEY_CHECK_UPDATES, &previous);

	// A release checked while another modal is up is deferred until the
	// modal layer frees.
	cx.update(|app| root.update(app, |app, cx| app.open_about(cx)));
	cx.run_until_parked();
	let release = crate::update::LatestRelease {
		version: "v9.9.9".to_string(),
		tag_name: "v9.9.9".to_string(),
		notes: "New things".to_string(),
		is_prerelease: false,
		published_at: "2026-09-20T08:00:00+00:00".to_string(),
		download_url: "/api/v1/releases/7/download?asset_id=12".to_string(),
	};
	cx.update(|app| root.update(app, |app, cx| app.on_update_checked(Ok(release), cx)));
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::About { .. })),
		"the open modal stays on top"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::UpdateAvailable { .. })),
		"the deferred prompt opens once the layer is free"
	);
	cx.update(|app| root.update(app, |app, cx| app.close_modal(cx)));
	cx.run_until_parked();

	// Help → Report a Bug opens the report form.
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_action_dispatched(ActionId::BugReport, cx)
		})
	});
	assert_eq!(
		cx.opened_url().as_deref(),
		Some(crate::update::BUG_REPORT_URL)
	);
}

/// The new-sequence dialog's OK path (the mock has no create command,
/// so the error keeps the dialog open) and its Cancel path (which also
/// abandons a paused drop).
#[gpui_kit::test]
async fn new_sequence_modal_commit_and_cancel(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	cx.update(|app| root.update(app, |app, cx| app.open_new_sequence(cx)));
	cx.run_until_parked();
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::NEW_SEQUENCE,
					button: 0,
				},
				cx,
			)
		});
	});
	cx.run_until_parked();
	assert!(
		cx.read(|app| matches!(root.read(app).modal, ModalState::NewSequence { .. })),
		"the mock rejects the create, so the dialog stays"
	);

	// Cancel abandons a paused drop and closes.
	cx.update(|app| {
		root.update(app, |app, _cx| {
			app.pending_drop = Some(FootageDropNeedsSequence {
				footage_id: 3,
				footage_ids: vec![3],
				track_kind: crate::widgets::timeline::TrackKind::Video,
				track_index: 0,
				time: Frame(0),
			});
		});
	});
	cx.update(|app| {
		root.update(app, |app, cx| {
			app.on_modal(
				&ModalEvent::ButtonClicked {
					control: modal_ids::NEW_SEQUENCE,
					button: 1,
				},
				cx,
			)
		});
	});
	cx.run_until_parked();
	assert!(cx.read(|app| matches!(root.read(app).modal, ModalState::None)));
	assert!(cx.read(|app| root.read(app).pending_drop.is_none()));
}

/// The app's tick loop runs on the simulated clock: advancing it lets
/// the spawned 60 Hz body drain the plugin-progress channel.
#[gpui_kit::test]
async fn tick_loop_body_drains_the_plugin_channel(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);

	let (tx, rx) = mpsc::channel::<crate::oakui::ofx::PluginProgressEvent>();
	cx.update(|app| {
		root.update(app, |app, _cx| app.plugin_progress_rx = Mutex::new(rx));
	});
	tx.send(crate::oakui::ofx::PluginProgressEvent {
		label: "OakOFX Render".into(),
		message: "frame".into(),
		fraction: 0.25,
	})
	.unwrap();
	cx.executor().advance_clock(Duration::from_millis(40));
	cx.run_until_parked();
	assert!(
		cx.read(|app| root.read(app).plugin_progress_open),
		"the timer body ran the shell's tick"
	);
}

/// The Exit action reaches the platform's quit hook without panicking.
#[gpui_kit::test]
async fn exit_action_quits(cx: &mut TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let (_window, root) = mock_shell(cx);
	cx.update(|app| root.update(app, |app, cx| app.dispatch_action_id(ActionId::Exit, cx)));
	cx.run_until_parked();
}

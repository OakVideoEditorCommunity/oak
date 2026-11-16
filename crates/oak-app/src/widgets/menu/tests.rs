use super::*;
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px, size};

#[test]
fn menu_bar_open_close_round_trip() {
	// Pure state checks are in model tests; here we just ensure the
	// model types are wired through the view API.
	let menu = Menu::new(vec![MenuItem::new(1, "Save").with_shortcut("⌘S")]);
	let entry = MenuBarEntry::new("File", menu);
	assert_eq!(entry.title, "File");
	assert_eq!(entry.menu.items[0].id, 1);
}

#[test]
fn context_menu_event_carries_item() {
	let event = ContextMenuEvent {
		item: 7,
		label: "Paste".into(),
	};
	assert_eq!(event.item, 7);
	assert_eq!(event.label, "Paste");
}

// --- interaction tests for the menu views ---

fn demo_entries() -> Vec<MenuBarEntry> {
	vec![MenuBarEntry::new(
		"File",
		Menu::new(vec![
			MenuItem::new(10, "Open…").with_shortcut("⌘O"),
			MenuItem::new(11, "Save").with_shortcut("⌘S"),
			MenuItem::new(12, "Quit").separated(),
		]),
	)]
}

/// An entry whose first item carries a nested submenu (like the app's
/// 视图 → 语言 / 主题).
fn demo_entries_with_submenu() -> Vec<MenuBarEntry> {
	let sub = Menu::new(vec![
		MenuItem::new(21, "简体中文"),
		MenuItem::new(22, "English"),
	]);
	vec![MenuBarEntry::new(
		"View",
		Menu::new(vec![
			MenuItem::new(20, "Language").with_submenu(sub),
			MenuItem::new(23, "Preferences…"),
		]),
	)]
}

/// Two top-level menus ("File" then "View"), for scrubbing between titles.
fn demo_entries_two() -> Vec<MenuBarEntry> {
	vec![
		MenuBarEntry::new(
			"File",
			Menu::new(vec![
				MenuItem::new(10, "Open…").with_shortcut("⌘O"),
				MenuItem::new(11, "Save").with_shortcut("⌘S"),
				MenuItem::new(12, "Quit").separated(),
			]),
		),
		MenuBarEntry::new(
			"View",
			Menu::new(vec![
				MenuItem::new(20, "Language"),
				MenuItem::new(23, "Preferences…"),
			]),
		),
	]
}

struct Host {
	menu_bar: Entity<MenuBar>,
	events: Vec<MenuBarEvent>,
}
impl Render for Host {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.menu_bar.clone())
	}
}

fn make_bar(cx: &mut TestAppContext) -> (&'static mut VisualTestContext, Entity<Host>) {
	make_bar_with(cx, demo_entries())
}

fn make_bar_with(
	cx: &mut TestAppContext,
	entries: Vec<MenuBarEntry>,
) -> (&'static mut VisualTestContext, Entity<Host>) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(400.0), px(120.0)), |window, cx| {
		let menu_bar = cx.new(|cx| MenuBar::new(1, entries, window, cx));
		let host = Host {
			menu_bar,
			events: Vec::new(),
		};
		cx.subscribe(
			&host.menu_bar,
			|host: &mut Host,
			 _m: Entity<MenuBar>,
			 event: &MenuBarEvent,
			 _cx: &mut Context<Host>| {
				host.events.push(event.clone());
			},
		)
		.detach();
		host
	});
	cx.run_until_parked();
	let host = window.root(cx).unwrap();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	(cx, host)
}

#[gpui_kit::test]
async fn clicking_a_menu_title_opens_the_popup(cx: &mut TestAppContext) {
	let (cx, _host) = make_bar(cx);
	// Click the "File" title (top-left of the bar).
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx.debug_bounds("menu-popup").expect("menu popup rendered");
	assert!(popup.size.height > px(60.0), "popup should list the items");
}

#[gpui_kit::test]
async fn clicking_a_menu_item_emits_triggered(cx: &mut TestAppContext) {
	let (cx, host) = make_bar(cx);
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx.debug_bounds("menu-popup").expect("menu popup rendered");
	// The first item row is the first ~26px of the popup.
	cx.simulate_click(
		point(popup.left() + px(40.0), popup.top() + px(16.0)),
		Modifiers::none(),
	);
	cx.run_until_parked();

	let triggered = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, MenuBarEvent::Triggered { item: 10, .. }))
	});
	assert!(triggered, "expected Triggered for the first item");
}

#[gpui_kit::test]
async fn keyboard_navigation_triggers_the_hovered_item(cx: &mut TestAppContext) {
	let (cx, host) = make_bar(cx);
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Down from no selection lands on the first item; the second down
	// moves to Save. Enter triggers it.
	cx.simulate_keystrokes("down");
	cx.run_until_parked();
	cx.simulate_keystrokes("down");
	cx.run_until_parked();
	cx.simulate_keystrokes("enter");
	cx.run_until_parked();

	let triggered = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, MenuBarEvent::Triggered { item: 11, .. }))
	});
	assert!(
		triggered,
		"expected Triggered for the second item via keyboard"
	);
}

#[gpui_kit::test]
async fn escape_closes_the_menu(cx: &mut TestAppContext) {
	let (cx, host) = make_bar(cx);
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(cx.debug_bounds("menu-popup").is_some());

	cx.simulate_keystrokes("escape");
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	assert!(
		cx.debug_bounds("menu-popup").is_none(),
		"menu should close on escape"
	);
	let closed = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, MenuBarEvent::MenuClosed { .. }))
	});
	assert!(closed);
}

/// Menu scrubbing: while a menu is open, moving over another top-level
/// title switches the open menu to it (and re-anchors the popup under that
/// title). With nothing open, hovering does not open a menu.
#[gpui_kit::test]
async fn hovering_another_title_switches_the_open_menu(cx: &mut TestAppContext) {
	let (cx, host) = make_bar_with(cx, demo_entries_two());
	// Open the first menu ("File").
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let popup = cx.debug_bounds("menu-popup").expect("first menu open");

	// Move over the second title ("View"): the open menu must switch to it.
	let second_title = cx
		.debug_bounds("menu-title-1")
		.expect("second title rendered");
	cx.simulate_mouse_move(second_title.center(), None, Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let switched = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, MenuBarEvent::MenuOpened { index: 1, .. }))
	});
	assert!(
		switched,
		"hovering the second title should switch the open menu"
	);
	let moved = cx.debug_bounds("menu-popup").expect("popup stays open");
	assert!(
		moved.left() > popup.left() + px(10.0),
		"the popup follows the hovered title"
	);

	// Hovering back over the first title switches back.
	let first_title = cx
		.debug_bounds("menu-title-0")
		.expect("first title rendered");
	cx.simulate_mouse_move(first_title.center(), None, Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let switched_back = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.filter(|e| matches!(e, MenuBarEvent::MenuOpened { index: 0, .. }))
			.count() >= 2
	});
	assert!(switched_back, "hovering the first title switches back");
}

#[gpui_kit::test]
async fn runtime_set_item_checked_flips_the_checkmark(cx: &mut TestAppContext) {
	let (cx, host) = make_bar(cx);
	// Toggle item 11 ("Save") at runtime, by id.
	let changed = cx.update(|_window, app| {
		let bar = host.read(app).menu_bar.clone();
		bar.update(app, |bar, _cx| bar.set_item_checked(11, true))
	});
	assert!(changed, "item 11 exists and should be updated");
	let checked = cx
		.update(|_window, app| host.read(app).menu_bar.read(app).entries[0].menu.items[1].checked);
	assert_eq!(checked, Some(true));

	// Unknown ids are reported as not found and change nothing.
	let changed = cx.update(|_window, app| {
		let bar = host.read(app).menu_bar.clone();
		bar.update(app, |bar, _cx| bar.set_item_checked(12345, true))
	});
	assert!(!changed);
	let checked = cx
		.update(|_window, app| host.read(app).menu_bar.read(app).entries[0].menu.items[0].checked);
	assert_eq!(checked, None);
}

/// Hovering a menu item that carries a nested menu must open the second
/// level to the right of the popup (the app's 视图 → 语言 / 主题 flow).
#[gpui_kit::test]
async fn hovering_a_submenu_item_opens_the_submenu(cx: &mut TestAppContext) {
	let (cx, host) = make_bar_with(cx, demo_entries_with_submenu());
	// Click the "View" title to open the menu.
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx.debug_bounds("menu-popup").expect("menu popup rendered");
	// Hover the first row ("Language", which has the submenu).
	cx.simulate_mouse_move(
		point(popup.left() + px(40.0), popup.top() + px(13.0)),
		None,
		Modifiers::none(),
	);
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let submenu = cx
		.debug_bounds("menu-submenu-popup")
		.expect("submenu popup opens next to the hovered item");
	assert!(
		submenu.left() > popup.left(),
		"the submenu opens to the right of the parent popup"
	);

	// Clicking a submenu row triggers it (and closes the menus).
	cx.simulate_click(
		point(submenu.left() + px(40.0), submenu.top() + px(13.0)),
		Modifiers::none(),
	);
	cx.run_until_parked();
	let triggered = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, MenuBarEvent::Triggered { item: 21, .. }))
	});
	assert!(triggered, "expected Triggered for the submenu item");
	assert!(
		cx.debug_bounds("menu-popup").is_none(),
		"menu closed after triggering"
	);
}

/// A host for the hover-occlusion test: a menu bar plus a full-width probe
/// strip underneath the bar that reports whether it is hovered. The open
/// popup drops over the strip, so the strip must stop being hovered while
/// the pointer is on the popup.
struct HoverHost {
	menu_bar: Entity<MenuBar>,
	probe_hovered: bool,
}
impl Render for HoverHost {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		div()
			.size_full()
			.child(
				div()
					.id("hover-probe")
					.absolute()
					.left_0()
					.top(px(30.0))
					.w_full()
					.h(px(90.0))
					.on_hover(cx.listener(|this, hovered: &bool, _window, _cx| {
						this.probe_hovered = *hovered;
					})),
			)
			.child(self.menu_bar.clone())
	}
}

/// The open menu drops from the open title's bottom-left corner: its left
/// edge lines up with the title's left edge and its top edge with the
/// title's bottom edge, no matter where the pointer was when the title was
/// clicked.
#[gpui_kit::test]
async fn menu_bar_popup_drops_from_the_open_titles_bottom_left(cx: &mut TestAppContext) {
	let (cx, _host) = make_bar(cx);
	// Click the right half of the "File" title; the popup must still line up
	// with the title's left edge, not the click position.
	cx.simulate_click(point(px(40.0), px(14.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx.debug_bounds("menu-popup").expect("menu popup rendered");
	let title = cx
		.debug_bounds("menu-title-0")
		.expect("menu title rendered");
	assert!(
		(f32::from(popup.left()) - f32::from(title.left())).abs() < 1.0,
		"popup left edge ({:?}) should align with the title's left edge ({:?})",
		popup.left(),
		title.left()
	);
	assert!(
		(f32::from(popup.top()) - f32::from(title.bottom())).abs() < 1.0,
		"popup top ({:?}) should start at the title's bottom edge ({:?})",
		popup.top(),
		title.bottom()
	);
}

/// The context menu's top level opens with its top-left corner exactly at
/// the requested window position (the right-click point).
#[gpui_kit::test]
async fn context_menu_popup_aligns_with_the_click_point(cx: &mut TestAppContext) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(400.0), px(120.0)), |window, cx| {
		ContextMenu::new(0, window, cx)
	});
	cx.run_until_parked();
	let context_menu = window.root(cx).unwrap();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	let position = point(px(200.0), px(50.0));
	let menu = Menu::new(vec![MenuItem::new(1, "Copy"), MenuItem::new(2, "Paste")]);
	cx.update(|_window, app| {
		context_menu.update(app, |menu_view, cx| menu_view.show(position, menu, cx));
	});
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx
		.debug_bounds("menu-popup")
		.expect("context menu popup rendered");
	assert!(
		(f32::from(popup.left()) - f32::from(position.x)).abs() < 1.0,
		"popup left ({:?}) should align with the click point x ({:?})",
		popup.left(),
		position.x
	);
	assert!(
		(f32::from(popup.top()) - f32::from(position.y)).abs() < 1.0,
		"popup top ({:?}) should align with the click point y ({:?})",
		popup.top(),
		position.y
	);
}

/// While a menu popup is open, elements of the underlying UI inside the
/// popup's bounds must not be hovered: the popup occludes them. This is the
/// structural regression test for the "hover bleed-through" bug where, for
/// example, an effect-library list item kept its highlight while the pointer
/// was over the open menu.
#[gpui_kit::test]
async fn open_menu_popup_occludes_hover_below_it(cx: &mut TestAppContext) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(400.0), px(120.0)), |window, cx| {
		let menu_bar = cx.new(|cx| MenuBar::new(1, demo_entries(), window, cx));
		HoverHost {
			menu_bar,
			probe_hovered: false,
		}
	});
	cx.run_until_parked();
	let host = window.root(cx).unwrap();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	// Open the "File" menu (the popup drops over the probe strip).
	cx.simulate_click(point(px(20.0), px(10.0)), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let popup = cx.debug_bounds("menu-popup").expect("menu popup rendered");

	// Over the probe but to the right of the popup: the probe is hovered.
	cx.simulate_mouse_move(point(px(300.0), px(60.0)), None, Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		cx.read(|app| host.read(app).probe_hovered),
		"the probe should be hovered when the pointer is not over the popup"
	);

	// Over the popup: the popup's hitbox occludes the probe, so the probe
	// must no longer be hovered (its highlight must be gone).
	cx.simulate_mouse_move(
		point(popup.left() + px(40.0), popup.top() + px(20.0)),
		None,
		Modifiers::none(),
	);
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		!cx.read(|app| host.read(app).probe_hovered),
		"the probe must not be hovered while the pointer is over the popup"
	);
}

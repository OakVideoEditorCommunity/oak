use super::*;
use crate::oakui::MockEngine;
use gpui_kit::{Modifiers, Point, TestAppContext, VisualTestContext, px, size};

/// Serializes the tests that read or write the process-global
/// collapsed-group key, and starts them from a clean value (a
/// developer's own persisted choice would otherwise hide rows from
/// the render tests).
fn collapsed_config_lock() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	store_collapsed(&HashSet::new());
	guard
}

/// A one-entry table row for the pure collapse logic.
fn entry(group: Option<&str>) -> EffectEntry {
	EffectEntry {
		type_id: "oak:testeffect".to_string(),
		name: "Test Effect".to_string(),
		group: group.map(str::to_string),
	}
}

/// The row selector `debug_bounds` looks up (leaked: it needs a
/// `&'static str` and tests are process-lifetime).
fn row_selector(type_id: &str) -> &'static str {
	Box::leak(format!("effect-library-row-{type_id}").into_boxed_str())
}

/// The collapse toggle and the collapsed-row predicate — a collapsed
/// group hides its rows, but a search still looks inside it.
#[test]
fn collapsed_rows_toggle_and_stay_searchable() {
	let color = entry(Some("color"));
	let mut collapsed = HashSet::new();

	assert!(!hidden_by_collapse(&color, "", &collapsed));
	assert!(
		toggle_collapsed(&mut collapsed, "color"),
		"first toggle collapses"
	);
	assert!(hidden_by_collapse(&color, "", &collapsed));
	assert!(
		!hidden_by_collapse(&color, "blur", &collapsed),
		"a search looks through collapsed groups"
	);
	assert!(
		!hidden_by_collapse(&color, "", &HashSet::new()),
		"an unlisted group stays open"
	);
	assert!(
		!toggle_collapsed(&mut collapsed, "color"),
		"second toggle expands"
	);
	assert!(collapsed.is_empty());
}

/// The search predicate: name or type id, case-folded substring.
#[test]
fn query_matches_name_and_type_id() {
	let color = entry(Some("color"));
	assert!(matches_query(&color, ""));
	assert!(matches_query(&color, "test"));
	assert!(matches_query(&color, "oak:test"));
	assert!(!matches_query(&color, "blur"));
}

/// The persisted form round-trips: sorted comma separated keys, with
/// padding and blank entries dropped.
#[test]
fn collapsed_list_round_trips() {
	let collapsed: HashSet<String> = ["keying", "color"].iter().map(|k| k.to_string()).collect();
	let raw = format_collapsed(&collapsed);
	assert_eq!(raw, "color,keying");
	assert_eq!(parse_collapsed(&raw), collapsed);
	assert_eq!(parse_collapsed(&format!(" {raw} , ")), collapsed);
	assert!(parse_collapsed("").is_empty());
	assert!(parse_collapsed(" , ").is_empty());
}

/// The panel's storage path: the collapsed set survives a store/load
/// cycle through the process config store.
#[test]
fn collapsed_state_persists_in_the_config() {
	let _guard = collapsed_config_lock();
	assert!(
		load_collapsed().is_empty(),
		"the lock starts from a clear key"
	);
	let collapsed: HashSet<String> = ["distort", "general"]
		.iter()
		.map(|k| k.to_string())
		.collect();
	store_collapsed(&collapsed);
	assert_eq!(load_collapsed(), collapsed);
	store_collapsed(&HashSet::new());
	assert!(load_collapsed().is_empty());
}

/// The panel renders one row per addable effect of the engine (the
/// mock exposes a deterministic app-local effect table).
#[gpui_kit::test]
async fn lists_every_addable_effect(cx: &mut TestAppContext) {
	let _guard = collapsed_config_lock();
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(400.0), px(600.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		EffectLibraryPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	let expected = crate::oakui::MockEngine::demo_effect_library();
	assert!(!expected.is_empty());
	for entry in &expected {
		let type_id = &entry.type_id;
		assert!(
			cx.debug_bounds(row_selector(type_id)).is_some(),
			"effect row {type_id} rendered"
		);
	}
}

/// A collapsed group keeps its header (so it can be reopened) but
/// renders none of its rows; the groups below it are unaffected.
#[gpui_kit::test]
async fn collapsing_a_group_hides_its_rows(cx: &mut TestAppContext) {
	let _guard = collapsed_config_lock();

	let entries = crate::oakui::MockEngine::demo_effect_library();
	let group = entries
		.first()
		.and_then(|entry| entry.group.clone())
		.expect("the engine's table is grouped");
	let other = entries
		.iter()
		.find(|entry| entry.group.as_deref() != Some(group.as_str()))
		.expect("the table spreads over more than one group")
		.clone();

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(400.0), px(600.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		let mut panel = EffectLibraryPanel::new(engine, window, cx);
		// The state the toggle writes (and the config reloads on the
		// next start), without touching the process-global key.
		panel.collapsed.insert(group.clone());
		panel
	});
	cx.run_until_parked();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	assert!(
		cx.debug_bounds(Box::leak(
			format!("effect-library-group-row-{group}").into_boxed_str()
		))
		.is_some(),
		"the collapsed group keeps its header"
	);
	for entry in entries
		.iter()
		.filter(|entry| entry.group.as_deref() == Some(group.as_str()))
	{
		assert!(
			cx.debug_bounds(row_selector(&entry.type_id)).is_none(),
			"collapsed row {} is hidden",
			entry.type_id
		);
	}
	assert!(
		cx.debug_bounds(row_selector(&other.type_id)).is_some(),
		"the next group still renders its rows"
	);
}

/// Clicking a group header is what writes the state: the group's rows
/// leave the render and the collapsed key lands in the persisted
/// config.
#[gpui_kit::test]
async fn clicking_a_group_header_collapses_and_persists_it(cx: &mut TestAppContext) {
	let _guard = collapsed_config_lock();

	let entries = crate::oakui::MockEngine::demo_effect_library();
	let group = entries
		.first()
		.and_then(|entry| entry.group.clone())
		.expect("the engine's table is grouped");
	let row = entries
		.iter()
		.find(|entry| entry.group.as_deref() == Some(group.as_str()))
		.expect("the group has rows")
		.type_id
		.clone();

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(400.0), px(600.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		EffectLibraryPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let header = visual
		.debug_bounds(Box::leak(
			format!("effect-library-group-row-{group}").into_boxed_str(),
		))
		.expect("the group header is painted");
	let center = Point::new(
		header.origin.x + header.size.width * 0.5,
		header.origin.y + header.size.height * 0.5,
	);
	assert!(
		visual.debug_bounds(row_selector(&row)).is_some(),
		"the row starts visible"
	);

	visual.simulate_click(center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	assert!(
		visual.debug_bounds(row_selector(&row)).is_none(),
		"the clicked group collapsed"
	);
	assert!(
		load_collapsed().contains(&group),
		"the collapse was persisted under {COLLAPSED_CONFIG_KEY}"
	);
}

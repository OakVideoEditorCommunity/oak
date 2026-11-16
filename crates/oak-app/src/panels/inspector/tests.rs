use super::*;

/// The enable entry flips its label with the card state, and `removable`
/// gates only the remove entry.
#[test]
fn stack_card_menu_flips_label_and_gates_remove() {
	for enabled in [true, false] {
		for removable in [true, false] {
			let menu = stack_card_menu(enabled, removable);
			assert_eq!(menu.items.len(), 4);

			let enable = &menu.items[0];
			assert_eq!(enable.id, LOCAL_ENABLE);
			let expected = if enabled {
				crate::i18n::tr("inspector.context.disable")
			} else {
				crate::i18n::tr("inspector.context.enable")
			};
			assert_eq!(enable.label, expected);

			let remove = &menu.items[1];
			assert_eq!(remove.id, LOCAL_REMOVE);
			assert_eq!(remove.enabled, removable);

			assert_eq!(menu.items[2].id, LOCAL_RENAME);
			assert_eq!(menu.items[3].id, LOCAL_PROPERTIES);
		}
	}
}

/// The add menu flattens the engine's table into one header per group,
/// labelled through the same resolution the effect library uses, with
/// the entries keeping the engine's (grouped, sorted) order.
#[test]
fn add_menu_rows_emit_one_header_per_group() {
	let entries = vec![
		EffectEntry {
			type_id: "oak:first".to_string(),
			name: "First".to_string(),
			group: Some("color".to_string()),
		},
		EffectEntry {
			type_id: "oak:second".to_string(),
			name: "Second".to_string(),
			group: Some("color".to_string()),
		},
		EffectEntry {
			type_id: "ofx:plugin".to_string(),
			name: "Plugin".to_string(),
			group: Some("Filter".to_string()),
		},
		EffectEntry {
			type_id: "oak:ungrouped".to_string(),
			name: "Ungrouped".to_string(),
			group: None,
		},
	];
	let shape: Vec<String> = add_menu_rows(&entries)
		.iter()
		.map(|row| match row {
			AddMenuRow::Header { key, label } => {
				format!("header:{}:{label}", key.unwrap_or("-"))
			}
			AddMenuRow::Effect(entry) => format!("effect:{}", entry.type_id),
		})
		.collect();
	assert_eq!(
		shape,
		vec![
			format!("header:color:{}", group_label("color")),
			"effect:oak:first".to_string(),
			"effect:oak:second".to_string(),
			format!("header:Filter:{}", group_label("Filter")),
			"effect:ofx:plugin".to_string(),
			format!(
				"header:-:{}",
				crate::i18n::tr("effect_library.group.builtin")
			),
			"effect:oak:ungrouped".to_string(),
		]
	);
}

use crate::oakui::MockEngine;
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, px, size};

/// The panel's context-menu routing (card menu target, every local
/// item, no-target guard), the pending add-menu render and the dock
/// metadata.
#[gpui_kit::test]
async fn panel_context_and_add_menu_paths(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(420.0), px(500.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		InspectorPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("inspector panel root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	// Dock metadata.
	cx.update(|_window, app| {
		assert_eq!(panel.read(app).panel_name(), crate::panels::ids::INSPECTOR);
	});

	// Right click targets the first effect card and opens the card menu.
	cx.simulate_mouse_down(
		gpui_kit::point(px(200.0), px(200.0)),
		MouseButton::Right,
		Modifiers::none(),
	);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		cx.debug_bounds("menu-popup").is_some(),
		"the card context menu opens"
	);

	// Every local item routes; a missing target is a no-op.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(LOCAL_ENABLE, cx);
			panel.on_local_menu_item(LOCAL_RENAME, cx);
			panel.on_local_menu_item(LOCAL_PROPERTIES, cx);
			panel.on_local_menu_item(9999, cx);
			panel.context_effect = None;
			panel.on_local_menu_item(LOCAL_ENABLE, cx);
			panel.context_effect = Some((EffectId(1), true, true));
			panel.on_local_menu_item(LOCAL_REMOVE, cx);
		});
	});

	// The add-effect menu renders while an add is pending (the mock
	// always reports a stack target).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.pending_add = Some(0);
			cx.notify();
		});
	});
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	// The add-menu list itself carries an element id but no debug
	// selector; its group headers do, so the menu's presence is
	// asserted through the built-in group header it renders.
	let menu_rendered = [
		"add-effect-group-row-color",
		"add-effect-group-row-filter",
		"add-effect-group-row-distort",
		"add-effect-group-row-keying",
		"add-effect-group-row-generator",
		"add-effect-group-row-math",
	]
	.into_iter()
	.any(|selector| cx.debug_bounds(selector).is_some());
	assert!(
		menu_rendered,
		"the add menu renders while an add is pending"
	);
	assert_eq!(
		cx.update(|_window, app| panel.read(app).pending_add),
		Some(0),
		"the mock engine has a stack target, so the pending add stays open"
	);
}

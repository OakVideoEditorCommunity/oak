use super::*;
use crate::oakui::MockEngine;
use crate::widgets::project_explorer::ProjectDataSource;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};

/// Serializes the tests that touch process-global state (the undo stack,
/// the media/codec library and the panel's HOME override).
fn stack_lock() -> std::sync::MutexGuard<'static, ()> {
	crate::oakui::graphops::test_lock()
}

/// Builds a `ProjectExplorerPanel` over the demo mock engine in a test
/// window.
fn mock_panel_window(
	cx: &mut TestAppContext,
) -> (
	&'static mut VisualTestContext,
	Entity<ProjectExplorerPanel<MockEngine>>,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(640.0), px(480.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		ProjectExplorerPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("project explorer panel root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	(cx, panel)
}

/// The widget's requests reach the panel's engine: open selects/opens
/// the entry, dropped files import, and a right-click request opens the
/// context-menu popup.
#[gpui_kit::test]
async fn explorer_events_route_to_the_engine_and_open_the_menu(cx: &mut TestAppContext) {
	let (cx, panel) = mock_panel_window(cx);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let explorer = cx.update(|_, cx| panel.read(cx).explorer.clone());
	let engine = cx.update(|_, cx| panel.read(cx).engine.clone());

	// OpenRequested on a non-sequence entry selects it into the shop
	// window (the mock reports no sequences).
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::OpenRequested {
			control: 1,
			id: 3,
			name: "第一稿.mp4".into(),
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| engine.read(app).selected_item()),
		Some(3),
		"a footage open request selects the entry"
	);

	// FileDropRequested imports every dropped path.
	let dropped = std::env::temp_dir().join("oakapp_explorer_drop.mp4");
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::FileDropRequested {
			control: 1,
			paths: vec![dropped.clone()],
		});
	});
	cx.run_until_parked();
	assert!(
		cx.read(|app| engine.read(app).imported_footage().contains(&dropped)),
		"the dropped path reaches the engine's import"
	);

	// ContextMenuRequested opens the popup for the requested entry.
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::ContextMenuRequested {
			control: 1,
			id: Some(3),
			position: gpui_kit::point(px(20.0), px(20.0)),
		});
	});
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		cx.debug_bounds("menu-popup").is_some(),
		"the requested context menu renders"
	);
}

/// The full context-menu selection: an empty-area click gets the blank
/// menu, footage gets the footage menu (with its proxy state), a
/// sequence gets the sequence menu and anything else the entry menu.
#[gpui_kit::test]
async fn context_menu_selection_follows_the_engine(cx: &mut TestAppContext) {
	use crate::oakui::real::RealEngine;
	let _lock = stack_lock();
	quercus_engine_rs::node::undo_clear();
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let media = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/demo.mp4");
	let window = cx.open_window(size(px(640.0), px(480.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		engine.update(cx, |engine, cx| {
			engine.new_project(cx);
			engine
				.create_sequence_with_params(
					"Context".into(),
					crate::oakui::VideoFormat::hd_1080p25(),
					false,
					cx,
				)
				.expect("create the sequence");
			engine
				.import_footage(media.clone(), cx)
				.expect("import the fixture media");
		});
		ProjectExplorerPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("project explorer panel root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	let engine = cx.update(|_, cx| panel.read(cx).engine.clone());

	let (seq_id, footage_id) = cx.read(|app| {
		let engine = engine.read(app);
		let seq = engine
			.sequence_entries()
			.first()
			.expect("a sequence entry")
			.0;
		let footage = engine
			.roots()
			.into_iter()
			.chain(
				engine
					.roots()
					.into_iter()
					.flat_map(|root| engine.children(root.id)),
			)
			.find(|entry| entry.name.as_ref() == "demo.mp4")
			.expect("the imported footage entry")
			.id;
		(seq, footage)
	});
	assert!(
		cx.read(|app| engine.read(app).entry_is_sequence(seq_id)),
		"the created sequence is an explorer sequence entry"
	);
	assert!(
		cx.read(|app| engine.read(app).entry_path(footage_id).is_some()),
		"the imported footage has an on-disk path"
	);

	// A second sequence makes the open-request routing observable: the
	// engine's current sequence must follow a sequence OpenRequested.
	let second_seq = cx.update(|_, cx| {
		engine.update(cx, |engine, cx| {
			engine
				.create_sequence_with_params(
					"Second".into(),
					crate::oakui::VideoFormat::hd_1080p25(),
					false,
					cx,
				)
				.expect("create the second sequence")
		})
	});
	assert_eq!(
		cx.read(|app| engine.read(app).current_sequence_id()),
		Some(second_seq),
		"creating a sequence opens it"
	);

	// The widget's open / drop requests: a sequence opens, footage
	// selects, a bogus dropped path logs the first error and a real one
	// imports.
	let explorer = cx.update(|_, cx| panel.read(cx).explorer.clone());
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::OpenRequested {
			control: 1,
			id: seq_id,
			name: "Context".into(),
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| engine.read(app).current_sequence_id()),
		Some(seq_id),
		"a sequence open request opens that sequence"
	);
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::OpenRequested {
			control: 1,
			id: footage_id,
			name: "demo.mp4".into(),
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|app| engine.read(app).current_sequence_id()),
		Some(seq_id),
		"a footage open request selects instead of switching the sequence"
	);

	// The import path counts what is actually in the project: a bogus
	// dropped path adds nothing (the engine rejects it before any node
	// lands), a real one adds exactly one entry.
	let entry_count = |app: &gpui_kit::App| -> usize {
		let engine = engine.read(app);
		let roots = engine.roots();
		let children: usize = roots
			.iter()
			.map(|root| engine.children(root.id).len())
			.sum();
		roots.len() + children
	};
	let entries_before = cx.update(|_, app| entry_count(app));
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::FileDropRequested {
			control: 1,
			paths: vec![std::path::PathBuf::from("/nonexistent/oakapp-drop.mp4")],
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.update(|_, app| entry_count(app)),
		entries_before,
		"a missing dropped path imports nothing"
	);
	explorer.update(cx, |_explorer, cx| {
		cx.emit(ProjectExplorerEvent::FileDropRequested {
			control: 1,
			paths: vec![media.clone()],
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.update(|_, app| entry_count(app)),
		entries_before + 1,
		"the real dropped path imports exactly one entry"
	);
	let demo_entries = cx.read(|app| {
		let engine = engine.read(app);
		let roots = engine.roots();
		let mut names: Vec<String> = roots.iter().map(|e| e.name.to_string()).collect();
		for root in &roots {
			names.extend(
				engine
					.children(root.id)
					.into_iter()
					.map(|e| e.name.to_string()),
			);
		}
		names.into_iter().filter(|name| name == "demo.mp4").count()
	});
	assert_eq!(
		demo_entries, 2,
		"the dropped media is listed a second time under its file name"
	);

	// Empty area -> the blank menu; footage -> footage menu (with proxy
	// state); sequence -> sequence menu; anything else -> entry menu.
	// Pin the engine data each choice keys off, then assert the open
	// state and the recorded context entry for every choice.
	assert!(cx.read(|app| engine.read(app).entry_path(footage_id).is_some()));
	assert!(!cx.read(|app| engine.read(app).entry_is_sequence(footage_id)));
	assert!(cx.read(|app| engine.read(app).entry_path(seq_id)).is_none());
	assert!(cx.read(|app| engine.read(app).entry_is_sequence(seq_id)));
	assert!(cx.read(|app| engine.read(app).entry_path(1)).is_none());
	assert!(!cx.read(|app| engine.read(app).entry_is_sequence(1)));
	for id in [None, Some(footage_id), Some(seq_id), Some(1)] {
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.open_context_menu(id, gpui_kit::point(px(20.0), px(20.0)), cx);
			});
		});
		cx.run_until_parked();
		assert_eq!(
			cx.update(|_, cx| panel.read(cx).context_entry),
			id,
			"the opened menu records its context entry"
		);
		let menu = cx.update(|_, cx| panel.read(cx).context_menu.widget());
		assert!(
			cx.read(|app| menu.read(app).is_open()),
			"the requested menu is open for {id:?}"
		);
	}

	// The properties item on a sequence emits the properties request
	// while the same item on a non-sequence is a logged no-op.
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(seq_id);
			panel.on_local_menu_item(LOCAL_PROPERTIES, cx);
			panel.context_entry = Some(footage_id);
			panel.on_local_menu_item(LOCAL_PROPERTIES, cx);
		});
	});
	cx.run_until_parked();

	quercus_engine_rs::node::undo_clear();
}

/// Every local menu item path: the guard returns without a context
/// entry, the emissions fire with one, the prompt-driven replace runs
/// its async body (both a selection and a cancellation) and the proxy
/// items apply to the mock's footage.
#[gpui_kit::test]
async fn local_menu_items_apply_and_guard(cx: &mut TestAppContext) {
	let (cx, panel) = mock_panel_window(cx);
	let engine = cx.update(|_, cx| panel.read(cx).engine.clone());

	// No context entry: every item returns early (reveal has no path
	// either, so its inner branch is skipped too).
	for item in [
		LOCAL_REVEAL_IN_FINDER,
		LOCAL_REPLACE_FOOTAGE,
		LOCAL_RENAME,
		LOCAL_DELETE,
		LOCAL_PROPERTIES,
		LOCAL_EXPORT_SEQUENCE,
		LOCAL_PROXY_GENERATE,
	] {
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.context_entry = None;
				panel.on_local_menu_item(item, cx);
			});
		});
	}

	// With a video footage entry: reveal (the mock has no path),
	// rename/delete/export emissions, the unimplemented tab entry, the
	// non-sequence properties log and the proxy actions.
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(10);
			panel.on_local_menu_item(LOCAL_REVEAL_IN_FINDER, cx);
			panel.on_local_menu_item(LOCAL_RENAME, cx);
			panel.on_local_menu_item(LOCAL_DELETE, cx);
			panel.on_local_menu_item(LOCAL_OPEN_IN_NEW_TAB, cx);
			panel.on_local_menu_item(LOCAL_PROPERTIES, cx);
			panel.on_local_menu_item(LOCAL_EXPORT_SEQUENCE, cx);
			panel.on_local_menu_item(LOCAL_PROXY_USE, cx);
		});
	});
	assert!(
		cx.read(|app| engine.read(app).proxy_row(10).expect("row").enabled),
		"the Use Proxy item toggles the footprint's proxy flag on"
	);
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(10);
			panel.on_local_menu_item(LOCAL_PROXY_USE, cx);
			panel.on_local_menu_item(LOCAL_PROXY_REVEAL, cx);
			panel.on_local_menu_item(LOCAL_PROXY_DELETE, cx);
		});
	});
	assert!(
		cx.read(|app| engine
			.read(app)
			.proxy_row(10)
			.is_some_and(|row| !row.enabled)),
		"the second Use Proxy toggles it back off"
	);

	// Generate succeeds for video, fails for audio-only footage.
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(10);
			panel.on_local_menu_item(LOCAL_PROXY_GENERATE, cx);
			panel.context_entry = Some(20);
			panel.on_local_menu_item(LOCAL_PROXY_GENERATE, cx);
		});
	});
	assert!(
		cx.read(|app| engine.read(app).proxy_state(10).is_some()),
		"generating a video proxy lands a state"
	);

	// The unknown item logs through the fallback arm.
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.on_local_menu_item(999_999, cx);
		});
	});

	// Replace footage: cancel the prompt (the async body takes the
	// non-selection path), then answer it (the mock returns Err, the
	// logged failure path).
	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(10);
			panel.on_local_menu_item(LOCAL_REPLACE_FOOTAGE, cx);
		});
	});
	assert!(cx.did_prompt_for_paths(), "the replace item prompts");
	cx.simulate_path_prompt_response(|_| None);
	cx.run_until_parked();

	cx.update(|_, cx| {
		panel.update(cx, |panel, cx| {
			panel.context_entry = Some(10);
			panel.on_local_menu_item(LOCAL_REPLACE_FOOTAGE, cx);
		});
	});
	cx.simulate_path_prompt_response(|_| {
		Some(vec![
			std::env::temp_dir().join("oakapp_explorer_replace.mp4"),
		])
	});
	cx.run_until_parked();
}

/// The panel's dock metadata and the platform reveal helper.
#[gpui_kit::test]
async fn panel_metadata_and_reveal(cx: &mut TestAppContext) {
	let (_cx, _panel) = mock_panel_window(cx);
	reveal_in_finder(std::path::Path::new("/tmp"));
}

/// The blank-area menu is New ▸ (the shared new section) + Import.
#[test]
fn blank_menu_is_new_and_import() {
	let menu = blank_menu();
	assert_eq!(menu.items.len(), 2);
	let new = &menu.items[0];
	assert_eq!(new.label, crate::i18n::tr("project.context.new"));
	assert_eq!(new.submenu.as_ref().unwrap().items.len(), 3);
	assert_eq!(menu.items[1].id, ActionId::Import.entry().menu_id());
}

/// The footage menu gates only the reveal entry on `reveal_enabled`;
/// without proxy state every proxy entry but the settings action stays
/// disabled.
#[test]
fn footage_menu_gates_the_reveal_entry() {
	for reveal_enabled in [true, false] {
		let menu = footage_menu(reveal_enabled, None);
		let reveal = menu
			.items
			.iter()
			.find(|item| item.id == LOCAL_REVEAL_IN_FINDER)
			.expect("reveal entry");
		assert_eq!(reveal.enabled, reveal_enabled);

		let ids: Vec<usize> = menu.items.iter().map(|item| item.id).collect();
		assert_eq!(
			ids,
			vec![
				LOCAL_REVEAL_IN_FINDER,
				LOCAL_REPLACE_FOOTAGE,
				0, // proxy submenu header
				LOCAL_RENAME,
				LOCAL_DELETE,
				LOCAL_PROPERTIES,
			]
		);
		let proxy = &menu.items[2].submenu.as_ref().unwrap().items;
		assert_eq!(proxy.len(), 5);
		assert!(proxy[..4].iter().all(|item| !item.enabled));
		assert!(proxy[4].enabled);
	}
}

/// The non-footage entry menu offers the open-in-new-tab/window pair
/// before rename/delete/properties.
#[test]
fn entry_menu_offers_open_in_new_tab_first() {
	let ids: Vec<usize> = entry_menu().items.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			LOCAL_OPEN_IN_NEW_TAB,
			LOCAL_OPEN_IN_NEW_WINDOW,
			LOCAL_RENAME,
			LOCAL_DELETE,
			LOCAL_PROPERTIES,
		]
	);
}

/// The sequence menu appends 导出序列 to the generic entry items.
#[test]
fn sequence_menu_appends_export() {
	let menu = sequence_menu();
	let ids: Vec<usize> = menu.items.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			LOCAL_OPEN_IN_NEW_TAB,
			LOCAL_OPEN_IN_NEW_WINDOW,
			LOCAL_RENAME,
			LOCAL_DELETE,
			LOCAL_PROPERTIES,
			LOCAL_EXPORT_SEQUENCE,
		]
	);
	let export = menu.items.last().expect("export item");
	assert_eq!(
		export.label,
		crate::i18n::tr("project.context.export_sequence")
	);
	assert!(export.enabled);
}

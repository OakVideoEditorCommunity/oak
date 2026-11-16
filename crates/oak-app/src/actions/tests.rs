use super::*;

/// Every stable id is unique (the shortcut-file format keys on it).
#[test]
fn registry_ids_are_unique() {
	let mut seen = std::collections::HashSet::new();
	for entry in REGISTRY {
		assert!(
			seen.insert(entry.cpp_id),
			"duplicate cpp id {}",
			entry.cpp_id
		);
	}
}

/// Every menu id is unique (the menu bar reports plain ids; a duplicate
/// would make two items dispatch the same action). Panel-context
/// hotkeys share [`HIDDEN_MENU_ID`] (no menu item) and are skipped.
#[test]
fn registry_menu_ids_are_unique() {
	let mut seen = std::collections::HashSet::new();
	for entry in REGISTRY {
		if entry.menu_id() == HIDDEN_MENU_ID {
			continue;
		}
		assert!(
			seen.insert(entry.menu_id()),
			"duplicate menu id {} ({})",
			entry.menu_id(),
			entry.cpp_id
		);
	}
}

/// Every default key parses as a gpui keystroke (the table is static
/// data, so a typo would otherwise surface only as a dead shortcut at
/// runtime — `key_bindings` would panic at startup).
#[test]
fn every_default_key_parses() {
	for entry in REGISTRY {
		for key in entry.default_keys {
			assert!(
				gpui_kit::Keystroke::parse(key).is_ok(),
				"invalid keystroke {key:?} on {}",
				entry.cpp_id
			);
		}
	}
}

/// No two actions claim the same keystroke (a conflict would make the
/// keymap dispatch whichever binding was registered last).
#[test]
fn default_keys_are_conflict_free() {
	let mut seen = std::collections::HashMap::new();
	for entry in REGISTRY {
		for key in entry.default_keys {
			let parsed = gpui_kit::Keystroke::parse(key).unwrap();
			let canon = parsed.unparse();
			let previous = seen.insert(canon.clone(), entry.cpp_id);
			assert!(
				previous.is_none(),
				"keystroke {canon} bound to both {} and {}",
				previous.unwrap(),
				entry.cpp_id
			);
		}
	}
}

/// The multicam source-switch hotkeys are panel-context: their
/// bindings carry the `MulticamPanel` key context so the digit keys
/// stay free for text entry outside the panel, and every other binding
/// stays global. (Regression: bound globally, `1..9` matched in every
/// focused panel and swallowed the digits before any value box / hex
/// field / text input could type them.)
#[test]
fn multicam_bindings_are_scoped_to_the_panel_context() {
	let panel_context = Some(Rc::new(
		KeyBindingContextPredicate::parse("MulticamPanel").unwrap(),
	));
	let multicam_actions: &[&dyn Action] = &[
		&MulticamSwitch1,
		&MulticamSwitch2,
		&MulticamSwitch3,
		&MulticamSwitch4,
		&MulticamSwitch5,
		&MulticamSwitch6,
		&MulticamSwitch7,
		&MulticamSwitch8,
		&MulticamSwitch9,
		&MulticamSwitchNoSplit1,
		&MulticamSwitchNoSplit2,
		&MulticamSwitchNoSplit3,
		&MulticamSwitchNoSplit4,
		&MulticamSwitchNoSplit5,
		&MulticamSwitchNoSplit6,
		&MulticamSwitchNoSplit7,
		&MulticamSwitchNoSplit8,
		&MulticamSwitchNoSplit9,
	];
	let mut scoped = 0usize;
	for binding in key_bindings() {
		let is_multicam = multicam_actions
			.iter()
			.any(|known| binding.action().partial_eq(*known));
		if is_multicam {
			scoped += 1;
			assert_eq!(
				binding.predicate(),
				panel_context,
				"the source-switch binding must be gated on the MulticamPanel key context"
			);
		} else {
			assert_eq!(
				binding.predicate(),
				Some(input_exclusion_predicate()),
				"non-multicam bindings must stay out of text fields"
			);
		}
	}
	assert_eq!(scoped, 18, "all 18 source-switch bindings are panel-scoped");
}

/// The input-exclusion predicate matches everywhere EXCEPT inside an
/// editable-text field, so single-key shuttles (j/k/l, i/o, c/v, …)
/// fire on the timeline but never swallow a letter typed into a value
/// box / hex field / text input (the "cannot type into the text field"
/// report).
#[test]
fn input_exclusion_predicate_stays_out_of_text_fields() {
	let predicate = input_exclusion_predicate();
	let text_context = gpui_kit::KeyContext::parse("Input").unwrap();
	let panel_context = gpui_kit::KeyContext::parse("Timeline").unwrap();
	assert_eq!(
		predicate.depth_of(&[text_context]),
		None,
		"no app hotkey matches while a text field is focused"
	);
	assert!(
		predicate.depth_of(&[panel_context]).is_some(),
		"hotkeys still match outside text fields"
	);
}

/// Every registry action appears somewhere in the menu tree (an action
/// without a menu entry is unreachable with the mouse, and the registry
/// is meant to drive the menus).
#[test]
fn every_action_appears_in_the_menu_tree() {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");

	use crate::oakui::component::menu;
	fn collect(menu: &menu::Menu, out: &mut Vec<usize>) {
		for item in &menu.items {
			out.push(item.id);
			if let Some(sub) = &item.submenu {
				collect(sub, out);
			}
		}
	}
	let mut ids = Vec::new();
	for entry in crate::app::make_menus_for_test() {
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
	for entry in REGISTRY {
		// The panel-context hotkeys (multicam source switches) are bound
		// to the focused panel, not to any menu — the C++ attaches them
		// straight to the MulticamWidget.
		if entry.menu_id() == HIDDEN_MENU_ID {
			continue;
		}
		assert!(
			ids.contains(&entry.menu_id()),
			"action {} (menu id {}) has no menu item",
			entry.cpp_id,
			entry.menu_id()
		);
	}
}

/// Every registry i18n key exists in both language tables with a
/// non-empty value (the menu labels come straight from `tr`).
#[test]
fn every_i18n_key_exists_in_both_languages() {
	for entry in REGISTRY {
		for code in ["en-US", "zh-CN"] {
			let _guard = crate::i18n::lang_test_lock()
				.lock()
				.unwrap_or_else(|e| e.into_inner());
			crate::i18n::set_language_code(code);
			let value = crate::i18n::tr(entry.i18n_key);
			assert_ne!(
				value, entry.i18n_key,
				"i18n key {} ({code}) is missing for action {}",
				entry.i18n_key, entry.cpp_id
			);
			assert!(!value.is_empty());
		}
	}
	// The loop leaves zh-CN active; restore the default so lock-free
	// tests building English menus are not fooled mid-assert.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
}

/// The shortcut display formatter renders the documented labels for a
/// sample of shapes (modifier stacks, arrows, named keys, punctuation).
/// The glyph form is the macOS rendering; other platforms spell the
/// modifiers out (covered by the parser tests instead).
#[test]
#[cfg(target_os = "macos")]
fn display_shortcut_formats_labels() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	assert_eq!(display_shortcut(ActionId::Redo).as_deref(), Some("⇧⌘Z"));
	assert_eq!(
		display_shortcut(ActionId::SplitAtPlayhead).as_deref(),
		Some("⌘K")
	);
	assert_eq!(display_shortcut(ActionId::NudgeLeft).as_deref(), Some("⌥←"));
	assert_eq!(
		display_shortcut(ActionId::FullScreen).as_deref(),
		Some("F11")
	);
	assert_eq!(display_shortcut(ActionId::Insert).as_deref(), Some(","));
	assert!(display_shortcut(ActionId::About).is_none());
}

// -------------------------------------------------------------------
// Custom shortcut overrides (stage 7)
// -------------------------------------------------------------------

/// A unique temporary directory for one test (the `shortcuts` file
/// round-trip), so parallel tests never collide on the same path.
fn temp_dir(label: &str) -> String {
	static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
	let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let path = std::env::temp_dir().join(format!(
		"oak-shortcuts-test-{label}-{}-{n}",
		std::process::id()
	));
	let _ = std::fs::remove_dir_all(&path);
	std::fs::create_dir_all(&path).unwrap();
	path.to_string_lossy().into_owned()
}

/// Parsing skips blank lines, unknown ids and unparseable keystrokes; an
/// empty value after the tab means "unbound".
#[test]
fn parse_shortcut_file_skips_malformed_lines() {
	let entries = parse_shortcut_file(
		"newproj\tsecondary-n\n\nbogusaction\tsecondary-x\nundo\t\nsaveproj\tnot-a-key\ncut\tsecondary-c\n",
	);
	assert_eq!(
		entries,
		vec![
			("newproj".to_string(), vec!["secondary-n".to_string()]),
			("undo".to_string(), vec![]), // unbound
			("cut".to_string(), vec!["secondary-c".to_string()]),
		]
	);
}

/// Write → clear → reload round-trips the effective keys, and the file
/// only carries the entries that differ from the registry defaults.
#[test]
fn shortcuts_file_round_trips_through_the_override_table() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	reset_all_custom_shortcuts();
	let dir = temp_dir("roundtrip");
	let path = format!("{dir}/shortcuts");

	// `newproj` = its own default (spelled in registry syntax) → not a
	// diff; the other two differ (`undo` gets unbound).
	set_custom_shortcut("newproj", vec!["secondary-n".to_string()]);
	set_custom_shortcut("saveproj", vec!["cmd-alt-s".to_string()]);
	set_custom_shortcut("undo", Vec::new()); // unbound
	assert_eq!(save_custom_shortcuts_to(&path).unwrap(), 2);

	let contents = std::fs::read_to_string(&path).unwrap();
	assert!(contents.contains("saveproj\t"), "file: {contents}");
	assert!(contents.contains("undo\t"), "file: {contents}");
	assert!(!contents.contains("newproj"), "file: {contents}");

	// Reload into a fresh table.
	reset_all_custom_shortcuts();
	assert!(!has_custom_shortcuts());
	assert_eq!(load_custom_shortcuts_from(&path).unwrap(), 2);
	assert_eq!(
		effective_keys(ActionId::SaveProject.entry()),
		vec!["cmd-alt-s".to_string()]
	);
	assert_eq!(effective_keys(ActionId::Undo.entry()), Vec::<String>::new());
	// The un-overridden action keeps its registry default.
	assert_eq!(
		effective_keys(ActionId::NewProject.entry()),
		vec!["secondary-n".to_string()]
	);
}

/// A default-equal override is dropped on save (canonical comparison), and
/// an all-default table removes the file entirely (the C++ behavior).
#[test]
fn save_writes_only_entries_that_differ_from_default() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	reset_all_custom_shortcuts();
	let dir = temp_dir("diff");
	let path = format!("{dir}/shortcuts");

	set_custom_shortcut("newproj", vec!["secondary-n".to_string()]);
	set_custom_shortcut("undo", vec!["cmd-alt-z".to_string()]);
	assert_eq!(save_custom_shortcuts_to(&path).unwrap(), 1);
	let contents = std::fs::read_to_string(&path).unwrap();
	assert!(contents.contains("undo\tcmd-alt-z"), "file: {contents}");
	assert!(!contents.contains("newproj"), "file: {contents}");

	// All-default → the file disappears.
	reset_all_custom_shortcuts();
	set_custom_shortcut("newproj", vec!["secondary-n".to_string()]);
	assert_eq!(save_custom_shortcuts_to(&path).unwrap(), 0);
	assert!(!std::path::Path::new(&path).exists());
}

/// Effective keys fall back to the registry defaults and the multi-key
/// defaults stay intact until overridden.
#[test]
fn effective_keys_fall_back_to_defaults() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	reset_all_custom_shortcuts();
	// Delete defaults to ["delete", "backspace"].
	assert_eq!(
		effective_keys(ActionId::Delete.entry()),
		vec!["delete".to_string(), "backspace".to_string()]
	);
	set_custom_shortcut("delete", vec!["secondary-x".to_string()]);
	assert_eq!(
		effective_keys(ActionId::Delete.entry()),
		vec!["secondary-x".to_string()]
	);
	reset_custom_shortcut("delete");
	assert_eq!(
		effective_keys(ActionId::Delete.entry()),
		vec!["delete".to_string(), "backspace".to_string()]
	);
}

/// Conflict resolution: taking a key away from another action moves the
/// binding (the displaced action falls back to its remaining keys, or to
/// none).
#[test]
fn stealing_a_key_moves_the_binding_away() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	reset_all_custom_shortcuts();
	let canon = canonical_key("secondary-c").expect("copy's key parses");
	assert_eq!(owner_of_shortcut(&canon), Some(ActionId::Copy));

	// The capture flow: steal the key away from its current owner, then
	// assign it to the target.
	let stolen = steal_shortcut_for(ActionId::Paste.entry(), &canon);
	assert_eq!(stolen, Some(ActionId::Copy));
	set_custom_shortcut(ActionId::Paste.entry().cpp_id, vec![canon.clone()]);
	assert_eq!(owner_of_shortcut(&canon), Some(ActionId::Paste));
	assert!(
		effective_keys(ActionId::Copy.entry()).is_empty(),
		"the displaced Copy must be unbound"
	);

	// A key nobody owns steals nothing.
	reset_all_custom_shortcuts();
	assert_eq!(steal_shortcut_for(ActionId::Paste.entry(), "f24"), None);
}

/// Overriding a default with the same key keeps `display_shortcut` stable
/// (the canonical comparison treats `secondary-` and `cmd-` as equal).
#[test]
#[cfg(target_os = "macos")]
fn display_shortcut_uses_the_effective_key() {
	let _guard = shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	set_custom_shortcut("saveproj", vec!["cmd-alt-s".to_string()]);
	assert_eq!(
		display_shortcut(ActionId::SaveProject).as_deref(),
		Some("⌥⌘S")
	);
	set_custom_shortcut("saveproj", Vec::new()); // unbound → no label
	assert!(display_shortcut(ActionId::SaveProject).is_none());
	reset_all_custom_shortcuts();
}

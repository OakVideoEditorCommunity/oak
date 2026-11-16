use super::*;

/// The color-label ids occupy the first local-id block.
#[test]
fn color_label_ids_are_the_first_local_ids() {
	assert_eq!(COLOR_LABEL_BASE, LOCAL_ID_BASE);
	for index in 0..COLOR_LABEL_COUNT {
		assert_eq!(color_label_index(COLOR_LABEL_BASE + index), Some(index));
	}
	assert_eq!(color_label_index(LOCAL_ID_BASE - 1), None);
	assert_eq!(color_label_index(LOCAL_ID_BASE + COLOR_LABEL_COUNT), None);
}

/// The color submenu carries all 16 labels with registry-free ids and
/// checks only the selected one.
#[test]
fn color_label_menu_marks_the_selection() {
	let menu = color_label_menu(Some(5));
	assert_eq!(menu.items.len(), COLOR_LABEL_COUNT);
	for (index, item) in menu.items.iter().enumerate() {
		assert_eq!(item.id, COLOR_LABEL_BASE + index);
		assert!(crate::actions::entry_for_menu_id(item.id).is_none());
		assert_eq!(item.checked, (index == 5).then_some(true));
	}
}

/// The edit segment mirrors `add_items_for_edit_menu`: the plain form
/// ends at delete, the clip form appends the clip-only tail.
#[test]
fn edit_section_matches_the_cpp_layout() {
	use ActionId as A;
	let plain = edit_section(false);
	let ids: Vec<usize> = plain.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			A::Undo.menu_id(),
			A::Redo.menu_id(),
			A::Cut.menu_id(),
			A::Copy.menu_id(),
			A::Paste.menu_id(),
			A::PasteInsert.menu_id(),
			A::Duplicate.menu_id(),
			A::Rename.menu_id(),
			A::Delete.menu_id(),
		]
	);
	// The separator sits after redo.
	assert!(plain[1].separator_after);

	let clips = edit_section(true);
	let tail: Vec<usize> = clips.iter().skip(9).map(|item| item.id).collect();
	assert_eq!(
		tail,
		vec![
			A::RippleDelete.menu_id(),
			A::SplitAtPlayhead.menu_id(),
			A::SpeedDuration.menu_id(),
			A::DefaultTransition.menu_id(),
			A::LinkUnlink.menu_id(),
			A::EnableDisable.menu_id(),
			A::Nest.menu_id(),
		]
	);
	// The separator between the clip-only editing and clip-edit groups
	// sits after speed/duration.
	assert!(clips[11].separator_after);
}

/// The in/out and new segments keep the C++ separator placement.
#[test]
fn in_out_and_new_sections_match_the_cpp_layout() {
	use ActionId as A;
	let in_out = in_out_section();
	assert_eq!(
		in_out.iter().map(|item| item.id).collect::<Vec<_>>(),
		vec![
			A::SetInPoint.menu_id(),
			A::SetOutPoint.menu_id(),
			A::ResetIn.menu_id(),
			A::ResetOut.menu_id(),
			A::ClearInOut.menu_id(),
		]
	);
	assert!(in_out[1].separator_after);

	let new_menu = new_section();
	assert_eq!(
		new_menu.iter().map(|item| item.id).collect::<Vec<_>>(),
		vec![
			A::NewProject.menu_id(),
			A::NewSequence.menu_id(),
			A::NewFolder.menu_id(),
		]
	);
	assert!(new_menu[0].separator_after);
}

/// Every registry item the segments build resolves back to its action.
#[test]
fn segment_items_resolve_to_registry_entries() {
	for item in edit_section(true)
		.into_iter()
		.chain(in_out_section())
		.chain(new_section())
	{
		assert!(
			crate::actions::entry_for_menu_id(item.id).is_some(),
			"segment item {} is not a registry id",
			item.id
		);
	}
}

/// The viewer menu carries the zoom levels with percentage labels and
/// checks Fit / the default radio entries.
#[test]
fn viewer_menu_offers_every_zoom_level() {
	// The label lookups below race with tests that flip the process
	// language: pin en-US under the shared lock.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let menu = viewer_menu(&ViewerMenuState::default());
	let zoom = menu
		.items
		.iter()
		.find(|item| item.label == crate::i18n::tr("viewer.context.zoom"))
		.expect("zoom submenu");
	let zoom_items = &zoom.submenu.as_ref().unwrap().items;
	assert_eq!(zoom_items.len(), 1 + VIEWER_ZOOM_LEVELS.len());
	assert_eq!(zoom_items[0].id, LOCAL_VIEWER_ZOOM_FIT);
	assert_eq!(zoom_items[0].checked, Some(true), "Fit checked by default");
	for (index, level) in VIEWER_ZOOM_LEVELS.iter().enumerate() {
		assert_eq!(zoom_items[index + 1].id, viewer_zoom_level_id(index));
		assert_eq!(
			zoom_items[index + 1].label,
			format!("{:.0}%", level * 100.0)
		);
		assert_eq!(
			zoom_items[index + 1].checked,
			Some(false),
			"zoom level {index} unchecked by default"
		);
	}
}

/// The resolution / safe-margin / waveform submenus check their first
/// (default) entry only.
#[test]
fn viewer_menu_radio_groups_default_to_the_first_entry() {
	// The label lookups below race with tests that flip the process
	// language: pin en-US under the shared lock.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let menu = viewer_menu(&ViewerMenuState::default());
	for label_key in [
		"viewer.context.playback_resolution",
		"viewer.context.safe_margins",
		"viewer.context.audio_waveform",
	] {
		let item = menu
			.items
			.iter()
			.find(|item| item.label == crate::i18n::tr(label_key))
			.unwrap_or_else(|| panic!("viewer menu missing {label_key}"));
		let sub = item.submenu.as_ref().unwrap();
		assert_eq!(sub.items[0].checked, Some(true), "{label_key} default");
		assert!(
			sub.items[1..]
				.iter()
				.all(|item| item.checked == Some(false)),
			"{label_key} non-defaults unchecked"
		);
	}
}

/// The resolution radio follows the current playback divider.
#[test]
fn viewer_menu_resolution_radio_follows_the_divider() {
	// Label lookup: pin en-US under the shared language lock.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let menu = viewer_menu(&ViewerMenuState {
		playback_divider: 4,
		..ViewerMenuState::default()
	});
	let item = menu
		.items
		.iter()
		.find(|item| item.label == crate::i18n::tr("viewer.context.playback_resolution"))
		.expect("resolution submenu");
	let sub = &item.submenu.as_ref().unwrap().items;
	assert_eq!(sub[0].checked, Some(false), "full unchecked at /4");
	assert_eq!(sub[1].checked, Some(false), "half unchecked at /4");
	assert_eq!(sub[2].checked, Some(true), "quarter checked at /4");
	assert_eq!(sub[3].checked, Some(false), "eighth unchecked at /4");
}

/// Every checked entry reflects the live state the panels build.
#[test]
fn viewer_menu_reflects_live_state() {
	// Label lookup: pin en-US under the shared language lock.
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let menu = viewer_menu(&ViewerMenuState {
		playback_divider: 2,
		zoom: ViewerZoom::Level(4),
		safe: SafeMargins::On,
		stop_on_last: true,
		waveform: WaveformMode::Both,
		show_fps: true,
	});
	let find_sub = |label: &'static str| {
		menu.items
			.iter()
			.find(|item| item.label == crate::i18n::tr(label))
			.unwrap_or_else(|| panic!("viewer menu missing {label}"))
			.submenu
			.as_ref()
			.unwrap()
			.items
			.clone()
	};
	let zoom = find_sub("viewer.context.zoom");
	assert_eq!(zoom[0].checked, Some(false), "Fit unchecked at 100%");
	assert_eq!(zoom[5].checked, Some(true), "100% checked");
	let resolution = find_sub("viewer.context.playback_resolution");
	assert_eq!(resolution[1].checked, Some(true), "half checked");
	let safe = find_sub("viewer.context.safe_margins");
	assert_eq!(safe[1].checked, Some(true), "safe On checked");
	let waveform = find_sub("viewer.context.audio_waveform");
	assert_eq!(waveform[2].checked, Some(true), "waveform Both checked");
	let stop = menu
		.items
		.iter()
		.find(|item| item.label == crate::i18n::tr("viewer.context.stop_on_last"))
		.expect("stop-on-last item");
	assert_eq!(stop.checked, Some(true));
	let fps = menu
		.items
		.iter()
		.find(|item| item.label == crate::i18n::tr("viewer.context.show_fps"))
		.expect("show-fps item");
	assert_eq!(fps.checked, Some(true));
}

/// Every local viewer id resolves to its action (and non-viewer ids do
/// not).
#[test]
fn viewer_menu_action_parses_every_id() {
	use ViewerMenuAction as V;
	assert_eq!(viewer_menu_action(LOCAL_VIEWER_ZOOM_FIT), Some(V::ZoomFit));
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_ZOOM_LEVELS_BASE + 4),
		Some(V::ZoomLevel(4))
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_ZOOM_LEVELS_BASE + VIEWER_ZOOM_LEVELS.len()),
		None,
		"zoom id past the last level"
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_FULL_SCREEN),
		Some(V::FullScreen)
	);
	for (id, divider) in [
		(LOCAL_VIEWER_RES_FULL, 1),
		(LOCAL_VIEWER_RES_HALF, 2),
		(LOCAL_VIEWER_RES_QUARTER, 4),
		(LOCAL_VIEWER_RES_EIGHTH, 8),
	] {
		assert_eq!(viewer_menu_action(id), Some(V::Resolution(divider)));
	}
	assert_eq!(viewer_menu_action(LOCAL_VIEWER_SAFE_OFF), Some(V::SafeOff));
	assert_eq!(viewer_menu_action(LOCAL_VIEWER_SAFE_ON), Some(V::SafeOn));
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_SAFE_CUSTOM),
		Some(V::SafeCustom)
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_STOP_ON_LAST),
		Some(V::StopOnLast)
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_WF_AUTOMATIC),
		Some(V::Waveform(WaveformMode::Automatic))
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_WF_ONLY),
		Some(V::Waveform(WaveformMode::Only))
	);
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_WF_BOTH),
		Some(V::Waveform(WaveformMode::Both))
	);
	assert_eq!(viewer_menu_action(LOCAL_VIEWER_SHOW_FPS), Some(V::ShowFps));
	assert_eq!(
		viewer_menu_action(LOCAL_VIEWER_SAVE_FRAME),
		Some(V::SaveFrame)
	);
	assert_eq!(
		viewer_menu_action(0),
		None,
		"registry-range id is not local"
	);
	assert_eq!(viewer_menu_action(LOCAL_ID_BASE), None, "color-label id");
}

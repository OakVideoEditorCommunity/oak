use super::*;

/// The dynamic menu-bar state the registry-driven menu tree reads its
/// checkmarks from (theme, active tool, snapping, loop, show-all,
/// full-screen, and which dock panels are open).
#[derive(Clone, Copy)]
pub(crate) struct MenuState {
	pub(crate) dark: bool,
	pub(crate) active_tool: Tool,
	pub(crate) snapping: bool,
	pub(crate) loop_playback: bool,
	pub(crate) full_screen: bool,
	pub(crate) use_proxy_media: bool,
	/// Bitmask of the dock panels currently visible (docked or floating), one
	/// bit per [`PanelId`] via [`panel_bit`]. Read from the live dock area at
	/// menu-bar build time.
	pub(crate) open_panels: u16,
}

impl MenuState {
	/// The shell's startup state (the timeline widget defaults to snapping
	/// on, the pointer tool is active; every panel starts docked, so all bits
	/// are set).
	pub(crate) fn new(dark: bool) -> Self {
		Self {
			dark,
			active_tool: Tool::Pointer,
			snapping: true,
			loop_playback: false,
			full_screen: false,
			use_proxy_media: use_proxy_media(),
			open_panels: WINDOW_PANELS
				.iter()
				.enumerate()
				.map(|(i, _)| panel_bit(i))
				.fold(0, |a, b| a | b),
		}
	}
}

/// One menu item straight from the registry — delegates to
/// [`menu::action_item`](menu::action_item) so the
/// menu bar and the context menus build items the same way.
fn menu_item(action: ActionId) -> MenuItem {
	menu::action_item(action)
}

/// Builds the menu bar entries (文件/编辑/视图/回放/序列/窗口/工具/帮助) from
/// the action registry (`src/actions.rs`): this function decides placement,
/// grouping and separators, while ids, labels and shortcut annotations all
/// come from the registry — so a menu click and a key press can never
/// diverge. All labels come from the [`crate::i18n`] tables, so rebuilding
/// the menu bar after a language switch repaints it in the new language;
/// `state` drives the dynamic checkmarks (theme, tool, snapping, loop, …).
pub(crate) fn make_menus(state: MenuState) -> Vec<MenuBarEntry> {
	use crate::actions::ActionId as A;
	use crate::i18n::tr;

	let theme_submenu = Menu::new(vec![
		menu_item(A::ThemeDark).with_checked(state.dark),
		menu_item(A::ThemeLight).with_checked(!state.dark),
	]);

	// 工具: the mutually exclusive tool group in the registry's order, the
	// add tool growing its addable-items submenu, then snapping + proxy.
	let mut tools: Vec<MenuItem> = Vec::new();
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
		let mut item = menu_item(tool.action()).with_checked(tool == state.active_tool);
		if tool == Tool::Add {
			item = item.with_submenu(Menu::new(vec![
				menu_item(A::AddEmpty),
				menu_item(A::AddBars),
				menu_item(A::AddShape),
				menu_item(A::AddSolid),
				menu_item(A::AddTitle),
				menu_item(A::AddTone),
				menu_item(A::AddSubtitle),
			]));
		}
		tools.push(item);
	}
	tools.push(
		menu_item(A::Snapping)
			.with_checked(state.snapping)
			.separated(),
	);
	tools.push(menu_item(A::UseProxyMedia).with_checked(state.use_proxy_media));
	tools.push(menu_item(A::ProxySettings));
	// 首选项 belongs to the Tools menu (the C++ layout); on macOS it ALSO
	// stays here (the Qt menu-role relocation does not exist in gpui).
	tools.push(menu_item(A::Preferences));

	vec![
		MenuBarEntry::new(
			tr("menu.file"),
			Menu::new(vec![
				// 新建 ▸ group: the parent carries the first child's id (the
				// same shape the theme/language submenus use).
				MenuItem::new(A::NewProject.menu_id(), tr("menu.file.new")).with_submenu(
					Menu::new(vec![
						menu_item(A::NewProject),
						menu_item(A::NewSequence),
						menu_item(A::NewFolder),
					]),
				),
				menu_item(A::OpenFromLibrary),
				menu_item(A::OpenProject),
				// 最近打开 ▸ (no recent-files backend yet: only the clear item).
				MenuItem::new(A::ClearOpenRecent.menu_id(), tr("menu.file.open_recent"))
					.with_submenu(Menu::new(vec![menu_item(A::ClearOpenRecent)])),
				menu_item(A::ProjectManager).separated(),
				menu_item(A::Import),
				menu_item(A::ImportSubtitles),
				menu_item(A::ExportSubtitles),
				menu_item(A::SaveProject),
				menu_item(A::SaveProjectAs),
				menu_item(A::Revert),
				menu_item(A::Export).separated(),
				menu_item(A::ProjectProperties),
				menu_item(A::CloseProject).separated(),
				menu_item(A::Exit),
			]),
		),
		MenuBarEntry::new(
			tr("menu.edit"),
			Menu::new(vec![
				menu_item(A::Undo),
				menu_item(A::Redo).separated(),
				menu_item(A::Cut),
				menu_item(A::Copy),
				menu_item(A::Paste),
				menu_item(A::PasteInsert),
				menu_item(A::Duplicate).separated(),
				menu_item(A::Rename),
				menu_item(A::Delete),
				menu_item(A::RippleDelete).separated(),
				menu_item(A::SplitAtPlayhead),
				menu_item(A::SpeedDuration),
				menu_item(A::DefaultTransition),
				menu_item(A::LinkUnlink),
				menu_item(A::EnableDisable),
				menu_item(A::Nest).separated(),
				menu_item(A::SelectAll),
				menu_item(A::DeselectAll).separated(),
				menu_item(A::Insert),
				menu_item(A::Overwrite).separated(),
				menu_item(A::RippleToIn),
				menu_item(A::RippleToOut),
				menu_item(A::EditToIn),
				menu_item(A::EditToOut).separated(),
				menu_item(A::NudgeLeft),
				menu_item(A::NudgeRight).separated(),
				menu_item(A::MoveInToPlayhead),
				menu_item(A::MoveOutToPlayhead).separated(),
				menu_item(A::SetInPoint),
				menu_item(A::SetOutPoint),
				menu_item(A::ResetIn),
				menu_item(A::ResetOut),
				menu_item(A::ClearInOut).separated(),
				menu_item(A::DeleteInOut),
				menu_item(A::RippleDeleteInOut).separated(),
				menu_item(A::Marker),
			]),
		),
		MenuBarEntry::new(
			tr("menu.view"),
			Menu::new(vec![
				menu_item(A::ThemeDark).with_submenu(theme_submenu),
				menu::language_menu(),
				menu_item(A::ZoomIn).separated(),
				menu_item(A::ZoomOut),
				menu_item(A::IncreaseTrackHeight),
				menu_item(A::DecreaseTrackHeight).separated(),
				menu_item(A::ToggleShowAll),
				menu_item(A::FullScreen).with_checked(state.full_screen),
				menu_item(A::FullScreenViewer),
			]),
		),
		MenuBarEntry::new(
			tr("menu.playback"),
			Menu::new(vec![
				menu_item(A::PlayPause),
				menu_item(A::PlayInToOut),
				menu_item(A::Loop)
					.with_checked(state.loop_playback)
					.separated(),
				menu_item(A::ShuttleLeft),
				menu_item(A::ShuttleStop),
				menu_item(A::ShuttleRight).separated(),
				menu_item(A::PrevFrame),
				menu_item(A::NextFrame).separated(),
				menu_item(A::GoToStart),
				menu_item(A::GoToEnd),
				menu_item(A::GoToPrevCut),
				menu_item(A::GoToNextCut).separated(),
				menu_item(A::GoToIn),
				menu_item(A::GoToOut),
			]),
		),
		MenuBarEntry::new(
			tr("menu.sequence"),
			Menu::new(vec![
				menu_item(A::AddVideoTrack),
				menu_item(A::AddAudioTrack),
				menu_item(A::RemoveTrack).separated(),
				menu_item(A::SetWorkArea),
				menu_item(A::ClearWorkArea).separated(),
				menu_item(A::RemoveMarker),
				menu_item(A::MulticamWizard),
				menu_item(A::SubtitleProperties).separated(),
				MenuItem::new(A::AudioGain50.menu_id(), tr("menu.sequence.audio")).with_submenu(
					Menu::new(vec![
						menu_item(A::AudioGain50),
						menu_item(A::AudioGain100),
						menu_item(A::AudioGain200),
						menu_item(A::AudioPanLeft).separated(),
						menu_item(A::AudioPanCenter),
						menu_item(A::AudioPanRight).separated(),
						menu_item(A::AudioFadeIn),
						menu_item(A::AudioFadeOut).separated(),
						menu_item(A::AudioAddVolume),
						menu_item(A::AudioAddPan),
						menu_item(A::AudioProperties).separated(),
					]),
				),
				menu_item(A::SeqCache),
				menu_item(A::SeqCacheInOut),
				menu_item(A::SeqCacheClear).separated(),
				menu_item(A::SequenceSettings).disabled(),
			]),
		),
		MenuBarEntry::new(
			tr("menu.window"),
			Menu::new({
				// Every panel is listed, checked when it is currently visible
				// in the dock (docked or floating); clicking toggles it open
				// or closed.
				let mut window_items: Vec<MenuItem> = WINDOW_PANELS
					.iter()
					.enumerate()
					.map(|(i, (id, action))| {
						let _ = id;
						menu_item(*action).with_checked(state.open_panels & panel_bit(i) != 0)
					})
					.collect();
				if let Some(last) = window_items.last_mut() {
					last.separator_after = true;
				}
				window_items.push(menu_item(A::MaximizePanel));
				window_items.push(menu_item(A::ResetDefaultLayout).separated());
				window_items
			}),
		),
		MenuBarEntry::new(tr("menu.tools"), Menu::new(tools)),
		MenuBarEntry::new(
			tr("menu.help"),
			Menu::new(vec![
				menu_item(A::ActionSearch),
				menu_item(A::BugReport),
				menu_item(A::Feedback).separated(),
				menu_item(A::About),
			]),
		),
	]
}

/// Builds the menu bar with the startup [`MenuState`] (tests).
#[cfg(test)]
pub(crate) fn make_menus_for_test() -> Vec<MenuBarEntry> {
	make_menus(MenuState::new(true))
}

/// The menu-bar *leaf* actions in hierarchy order, each with its localized
/// "Menu > Submenu > …" path. The Keyboard preferences tab and the action
/// search dialog both enumerate the menu bar like the C++ does
/// (`PreferencesKeyboardTab::setup_kbd_shortcuts` /
/// `ActionSearch::search_update`), so the two share one walk. Panel-context
/// hotkeys (the [`HIDDEN_MENU_ID`](crate::actions::HIDDEN_MENU_ID) multicam
/// switches) are excluded by construction — they have no menu item.
pub(crate) fn menu_action_paths() -> Vec<(ActionId, String)> {
	let mut out = Vec::new();
	for entry in make_menus(MenuState::new(true)) {
		let top = entry.title.to_string();
		walk_menu_for_actions(&entry.menu, &top, &mut out);
	}
	out
}

/// Recurses a menu, emitting leaf items (submenu headers extend the path and
/// are not emitted themselves — their id is the first child's id).
fn walk_menu_for_actions(menu: &Menu, path: &str, out: &mut Vec<(ActionId, String)>) {
	for item in &menu.items {
		if let Some(submenu) = &item.submenu {
			let label = item.label.to_string();
			walk_menu_for_actions(submenu, &format!("{path} > {label}"), out);
		} else if let Some(entry) = crate::actions::entry_for_menu_id(item.id) {
			out.push((entry.action, path.to_string()));
		}
	}
}

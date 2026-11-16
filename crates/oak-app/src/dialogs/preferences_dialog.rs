use super::*;

/// The preferences dialog tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreferencesTab {
	/// The grouped settings (language / theme / backend / cache / …).
	General,
	/// The custom-shortcuts editor.
	Keyboard,
}

/// A fully transparent color (for un-selected rows / tabs).
pub(crate) fn transparent() -> gpui_kit::Rgba {
	gpui_kit::Rgba {
		r: 0.0,
		g: 0.0,
		b: 0.0,
		a: 0.0,
	}
}

/// The tabbed preferences dialog content: the existing grouped settings plus
/// the Keyboard tab — the Rust counterpart of the C++ `PreferencesDialog`
/// hosting the `PreferencesKeyboardTab` (`preferenceskeyboardtab.cpp`).
///
/// The host re-emits the general tab's [`PreferencesEvent`]s (theme /
/// language) and turns the keyboard tab's [`KeyboardEvent::Changed`] into
/// [`PreferencesEvent::ShortcutsChanged`], so the app shell only subscribes to
/// this one content entity.
pub struct PreferencesDialogContent {
	pub(crate) active: PreferencesTab,
	pub(crate) general: Entity<PreferencesContent>,
	pub(crate) keyboard: Entity<KeyboardTabContent>,
}

impl EventEmitter<PreferencesEvent> for PreferencesDialogContent {}

impl PreferencesDialogContent {
	/// Builds the tabbed preferences dialog with both tabs.
	///
	/// Creates the general tab ([`PreferencesContent::new`]) and the keyboard tab
	/// ([`KeyboardTabContent::new`]), then forwards their events so the shell can
	/// watch this single entity: theme / language / display-color changes from the
	/// general tab are re-emitted unchanged, and [`KeyboardEvent::Changed`] from
	/// the keyboard tab becomes [`PreferencesEvent::ShortcutsChanged`]. The general
	/// tab is active initially. `window` is passed through to both tab
	/// constructors; `cx` creates the entities and registers the subscriptions.
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let general = cx.new(|cx| PreferencesContent::new(window, cx));
		let keyboard = cx.new(|cx| KeyboardTabContent::new(window, cx));
		cx.subscribe(
			&general,
			|_this, _general, event: &PreferencesEvent, cx| match event {
				PreferencesEvent::ThemeChanged(dark) => {
					cx.emit(PreferencesEvent::ThemeChanged(*dark));
				}
				PreferencesEvent::LanguageChanged => cx.emit(PreferencesEvent::LanguageChanged),
				PreferencesEvent::ShortcutsChanged => {}
				PreferencesEvent::DisplayColorChanged => {
					cx.emit(PreferencesEvent::DisplayColorChanged);
				}
			},
		)
		.detach();
		cx.subscribe(&keyboard, |_this, _keyboard, event: &KeyboardEvent, cx| {
			if matches!(event, KeyboardEvent::Changed) {
				cx.emit(PreferencesEvent::ShortcutsChanged);
			}
		})
		.detach();
		Self {
			active: PreferencesTab::General,
			general,
			keyboard,
		}
	}

	/// Commits the general tab's free-text fields (the cache directory, the
	/// custom ICC path, the PostgreSQL connection string), for the host when
	/// the dialog closes.
	pub fn commit_cache_dir(&self, cx: &App) {
		let general = self.general.read(cx);
		general.commit_cache_dir(cx);
		general.commit_display_icc_path(cx);
		general.commit_storage_pg_url(cx);
	}

	/// Returns how many action rows the keyboard tab lists (tests).
	///
	/// The count is the number of menu-bar actions enumerated when the tab was
	/// built and does not follow the current search filter, so it stays constant
	/// while the user types; the host uses it to assert the tab was populated.
	pub fn keyboard_tab_row_count(&self, cx: &App) -> usize {
		self.keyboard.read(cx).row_count()
	}
}

impl Render for PreferencesDialogContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let content = match self.active {
			PreferencesTab::General => self.general.clone().into_any_element(),
			PreferencesTab::Keyboard => self.keyboard.clone().into_any_element(),
		};
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(
				div()
					.flex()
					.gap_1()
					.child(tab_button(
						PreferencesTab::General,
						self.active,
						&colors,
						cx,
					))
					.child(tab_button(
						PreferencesTab::Keyboard,
						self.active,
						&colors,
						cx,
					)),
			)
			.child(content)
	}
}

/// Renders one tab switcher button of the preferences dialog.
///
/// `tab` is both the tab the button selects and the source of its localized
/// label, and `active` is the tab currently shown — the button is highlighted
/// only when the two match. `colors` supplies the theme colors for the
/// selected, normal and hover backgrounds. Clicking sets the dialog's active tab
/// and notifies. The returned element carries the stable id
/// `prefs-tab-general` / `prefs-tab-keyboard` (also its debug selector) for
/// tests, and `cx` is the dialog's context for the click listener.
fn tab_button(
	tab: PreferencesTab,
	active: PreferencesTab,
	colors: &gpui_kit::colors::Colors,
	cx: &mut Context<PreferencesDialogContent>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
	let selected = tab == active;
	let label = match tab {
		PreferencesTab::General => i18n::tr("preferences.tab.general"),
		PreferencesTab::Keyboard => i18n::tr("preferences.tab.keyboard"),
	};
	div()
		.id(match tab {
			PreferencesTab::General => "prefs-tab-general",
			PreferencesTab::Keyboard => "prefs-tab-keyboard",
		})
		.debug_selector(move || match tab {
			PreferencesTab::General => "prefs-tab-general".into(),
			PreferencesTab::Keyboard => "prefs-tab-keyboard".into(),
		})
		.px_3()
		.py_1()
		.rounded_md()
		.cursor_pointer()
		.bg(if selected {
			colors.selected
		} else {
			transparent()
		})
		.hover(|style| {
			if selected {
				style.bg(crate::widgets::theme::accent_hover(&colors))
			} else {
				style.bg(crate::widgets::theme::neutral_hover(&colors))
			}
		})
		.text_color(if selected {
			colors.selected_text
		} else {
			colors.text
		})
		.on_click(cx.listener(move |this, _event, _window, cx| {
			this.active = tab;
			cx.notify();
		}))
		.child(label)
}

/// A small pill-shaped text button (the keyboard tab's footer buttons). The
/// caller chains the click handler on the returned element.
pub(crate) fn pill_button(
	id: &'static str,
	label: impl Into<SharedString>,
	bg: gpui_kit::Rgba,
	hover_bg: gpui_kit::Rgba,
	fg: gpui_kit::Rgba,
) -> gpui_kit::Stateful<gpui_kit::Div> {
	let label: SharedString = label.into();
	div()
		.id(id)
		.px_3()
		.py_1()
		.rounded_md()
		.cursor_pointer()
		.bg(bg)
		.hover(move |style| style.bg(hover_bg))
		.text_color(fg)
		.child(label)
}

// ---------------------------------------------------------------------------
// Preferences → Keyboard tab
// ---------------------------------------------------------------------------

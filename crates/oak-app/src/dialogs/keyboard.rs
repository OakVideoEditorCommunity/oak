use super::*;

/// A request the keyboard tab emits; the tabbed host re-emits it as
/// [`PreferencesEvent::ShortcutsChanged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardEvent {
	/// The shortcut overrides changed (a key was assigned / cleared / reset /
	/// imported). The host must re-bind the key map and rebuild the menu bar.
	Changed,
}

impl EventEmitter<KeyboardEvent> for KeyboardTabContent {}

/// One row of the keyboard tab: a menu-bar action with its hierarchy.
pub(crate) struct KeyboardRow {
	pub(crate) action: ActionId,
	/// The top-level menu title (the section header), localized.
	section: String,
	/// The full "Menu > Submenu > …" path, localized.
	path: String,
	/// The capture field's focus handle.
	focus: FocusHandle,
}

/// The Preferences → Keyboard tab: a searchable, section-grouped list of every
/// menu-bar action with a click-to-capture shortcut editor, plus Reset
/// Selected / Reset All and Import / Export — the Rust counterpart of the C++
/// `PreferencesKeyboardTab`.
///
/// # Capture
///
/// Clicking a row's shortcut field enters capture mode: a process-wide
/// [`gpui_kit::App::intercept_keystrokes`] subscription suppresses the global key
/// map for as long as the capture is active, so the field's `on_key_down`
/// sees *every* key — including the letters, digits and arrows that the
/// registry binds (the shell's action listeners would otherwise swallow them
/// before the widget-level handlers run, the same problem Qt solves with
/// `QKeySequenceEdit`'s `ShortcutOverride`). The capture field then decides:
///
/// * any real key (plus modifiers) becomes the action's new binding;
/// * Backspace / Delete unbind the action (back to "None");
/// * Escape cancels the capture.
///
/// Every commit writes the override diff to `<config>/shortcuts` and emits
/// [`KeyboardEvent::Changed`], so the change is live immediately (no restart).
///
/// # Conflicts
///
/// Assigning a key that another action already binds *moves* the binding: the
/// displaced action loses the key (falling back to its remaining keys, or to
/// none) and the status line says so. Simple and explicit — the alternative
/// (refusing the assignment) would leave the user stuck when two popular
/// defaults collide.
pub struct KeyboardTabContent {
	pub(crate) query: Entity<InputState>,
	pub(crate) rows: Vec<KeyboardRow>,
	pub(crate) filter: String,
	pub(crate) capturing: Option<usize>,
	pub(crate) selected: Option<usize>,
	pub(crate) confirm_reset_all: bool,
	pub(crate) status: Option<String>,
	/// The keystroke interceptor that routes every key to the capture logic
	/// while a row is capturing (see the capture notes above); it lives for
	/// the tab's whole lifetime and only acts while `capturing` is set.
	#[allow(dead_code)] // kept alive: dropping it unregisters the keystroke interceptor
	interceptor: Option<gpui_kit::Subscription>,
}

impl KeyboardTabContent {
	/// Builds the keyboard tab with one row per menu-bar action.
	///
	/// Rows come from [`crate::app::menu_action_paths`] in that order; each keeps
	/// its localized section (the first `" > "` segment of the path) and its full
	/// path, and starts with no captured key and no selection. A subscription on
	/// the search field re-applies the filter and clears the selection on every
	/// change. A keystroke interceptor is installed too: it forwards keys to
	/// [`Self::handle_capture_key`] only while a row is capturing, and it lives as
	/// long as the returned content (dropping it unregisters the interceptor).
	/// `window` is needed to build the search input and `cx` to create the
	/// entities.
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let query = cx.new(|cx| InputState::new(window, cx));
		cx.subscribe(&query, |this, _query, event: &InputEvent, cx| {
			if !matches!(event, InputEvent::Change) {
				return;
			}
			this.filter = this.query.read(cx).value().to_string();
			this.selected = None;
			cx.notify();
		})
		.detach();
		let rows = crate::app::menu_action_paths()
			.into_iter()
			.map(|(action, path)| {
				let section = path.split(" > ").next().unwrap_or_default().to_string();
				KeyboardRow {
					action,
					section,
					path,
					focus: cx.focus_handle(),
				}
			})
			.collect();
		let weak = cx.weak_entity();
		let interceptor = cx.intercept_keystrokes(move |event, _window, app| {
			// While any row is capturing, handle the key here — before the
			// shell's global key bindings (which would otherwise swallow it) —
			// and stop the event so it can neither reach an app action nor
			// bubble to the modal (Escape must cancel the capture, not close
			// the dialog).
			let Some(this) = weak.upgrade() else {
				return;
			};
			if this.read(app).capturing.is_some() {
				this.update(app, |this, cx| {
					this.handle_capture_key(&event.keystroke, cx);
				});
			}
		});
		Self {
			query,
			rows,
			filter: String::new(),
			capturing: None,
			selected: None,
			confirm_reset_all: false,
			status: None,
			interceptor: Some(interceptor),
		}
	}

	/// The number of menu-bar actions listed (tests).
	pub fn row_count(&self) -> usize {
		self.rows.len()
	}

	/// Enters capture mode for `index`: remembers the row and highlights it.
	/// The keystroke interceptor installed at construction does the rest — it
	/// sees every key before the shell's global bindings do, so the capture
	/// works regardless of which element currently has focus.
	pub(crate) fn begin_capture(&mut self, index: usize, cx: &mut Context<Self>) {
		self.capturing = Some(index);
		self.selected = Some(index);
		self.confirm_reset_all = false;
		cx.notify();
	}

	/// Leaves capture mode without changing any binding.
	///
	/// Clears the capturing row index and notifies so that field stops showing the
	/// "press a key" label. Escape and every completed assign/clear path call
	/// this; the current row selection is left untouched.
	fn end_capture(&mut self, cx: &mut Context<Self>) {
		self.capturing = None;
		cx.notify();
	}

	/// Applies one keystroke to the row currently being captured.
	///
	/// Called from the keystroke interceptor (which runs before the shell's global
	/// bindings) for every key while a row is capturing; it returns immediately
	/// when no row is capturing. [`capture_decision`] classifies the key:
	/// modifier-only keys (a bare Shift/Ctrl/…) are ignored, Escape cancels,
	/// Backspace/Delete clears the action's override, and any other key becomes
	/// the new single binding after [`crate::actions::steal_shortcut_for`] removes
	/// it from whichever action held it (the status line then names the displaced
	/// action). Every non-ignored key ends the capture, persists the overrides,
	/// emits [`KeyboardEvent::Changed`], and stops propagation so the key cannot
	/// also trigger an app action or reach the modal (Escape must cancel, not
	/// close the dialog). A failed save is ignored; the in-memory map stays
	/// updated either way.
	pub(crate) fn handle_capture_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
		let Some(index) = self.capturing else {
			return;
		};
		match capture_decision(keystroke) {
			CaptureDecision::Ignore => cx.stop_propagation(),
			CaptureDecision::Cancel => {
				self.end_capture(cx);
				cx.stop_propagation();
			}
			CaptureDecision::Clear => {
				let action = self.rows[index].action;
				crate::actions::set_custom_shortcut(action.entry().cpp_id, Vec::new());
				let _ = crate::actions::save_custom_shortcuts();
				self.status = Some(i18n::tr("preferences.keyboard.cleared").to_string());
				self.end_capture(cx);
				cx.emit(KeyboardEvent::Changed);
				cx.stop_propagation();
			}
			CaptureDecision::Assign(canon) => {
				let action = self.rows[index].action;
				let stolen = crate::actions::steal_shortcut_for(action.entry(), &canon);
				crate::actions::set_custom_shortcut(action.entry().cpp_id, vec![canon]);
				let _ = crate::actions::save_custom_shortcuts();
				self.status = stolen.map(|previous| {
					i18n::tr("preferences.keyboard.conflict")
						.replace("{action}", i18n::tr(previous.entry().i18n_key))
				});
				self.end_capture(cx);
				cx.emit(KeyboardEvent::Changed);
				cx.stop_propagation();
			}
		}
	}

	/// Reset Selected: the selected row's action falls back to its registry
	/// default keys.
	pub(crate) fn reset_selected(&mut self, cx: &mut Context<Self>) {
		let Some(index) = self.selected else {
			return;
		};
		let action = self.rows[index].action;
		crate::actions::reset_custom_shortcut(action.entry().cpp_id);
		let _ = crate::actions::save_custom_shortcuts();
		self.status = Some(i18n::tr("preferences.keyboard.reset").to_string());
		cx.emit(KeyboardEvent::Changed);
		cx.notify();
	}

	/// Reset All: the first click arms an inline confirmation (the C++
	/// `QMessageBox` equivalent, kept inside the tab so the host modal
	/// machinery stays untouched); the second applies it.
	pub(crate) fn reset_all(&mut self, cx: &mut Context<Self>) {
		if !self.confirm_reset_all {
			self.confirm_reset_all = true;
			self.capturing = None;
			cx.notify();
			return;
		}
		crate::actions::reset_all_custom_shortcuts();
		let _ = crate::actions::save_custom_shortcuts();
		self.confirm_reset_all = false;
		self.status = Some(i18n::tr("preferences.keyboard.reset_all_done").to_string());
		cx.emit(KeyboardEvent::Changed);
		cx.notify();
	}

	/// Import: pick a `shortcuts` file, replace the overrides with its
	/// contents (anything unlisted falls back to default, like the C++ field
	/// walk), then save the effective state back to the configured location.
	pub(crate) fn import_shortcuts(&mut self, cx: &mut Context<Self>) {
		let receiver = cx.prompt_for_paths(PathPromptOptions {
			files: true,
			directories: false,
			multiple: false,
			prompt: Some(i18n::tr("preferences.keyboard.import").into()),
		});
		cx.spawn(async move |this, cx| {
			let Ok(Ok(Some(paths))) = receiver.await else {
				return;
			};
			let Some(path) = paths.first() else {
				return;
			};
			let result = crate::actions::load_custom_shortcuts_from(&path.to_string_lossy());
			let _ = this.update(cx, |this, cx| {
				match result {
					Ok(_) => {
						this.status = Some(i18n::tr("preferences.keyboard.imported").to_string());
						let _ = crate::actions::save_custom_shortcuts();
					}
					Err(_) => {
						this.status =
							Some(i18n::tr("preferences.keyboard.import_failed").to_string())
					}
				}
				this.capturing = None;
				this.confirm_reset_all = false;
				cx.emit(KeyboardEvent::Changed);
				cx.notify();
			});
		})
		.detach();
	}

	/// Exports the custom shortcut overrides to a user-picked file.
	///
	/// Opens the platform save dialog with the suggested name `shortcuts`
	/// (starting at `.`), then writes the current override diff there through
	/// [`crate::actions::save_custom_shortcuts_to`]. Cancelling the dialog does
	/// nothing. The outcome is reported in the tab's status line (localized
	/// "exported" / "export failed"); the I/O runs in a spawned task, and `cx` is
	/// notified when the status text is updated.
	pub(crate) fn export_shortcuts(&mut self, cx: &mut Context<Self>) {
		let receiver = cx.prompt_for_new_path(&std::path::PathBuf::from("."), Some("shortcuts"));
		cx.spawn(async move |this, cx| {
			let Ok(Ok(Some(path))) = receiver.await else {
				return;
			};
			let result = crate::actions::save_custom_shortcuts_to(&path.to_string_lossy());
			let _ = this.update(cx, |this, cx| {
				this.status = Some(match result {
					Ok(_) => i18n::tr("preferences.keyboard.exported").to_string(),
					Err(_) => i18n::tr("preferences.keyboard.export_failed").to_string(),
				});
				cx.notify();
			});
		})
		.detach();
	}
}

impl Render for KeyboardTabContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let weak = self.query.downgrade();
		let capturing = self.capturing;
		let selected = self.selected;

		// The search box.
		let search = div()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.px_2()
			.py_1()
			.child(
				text_input("keyboard-search-input", cx)
					.state(weak)
					.accepts_input(true),
			);

		// The grouped, filtered action list.
		let mut list = div()
			.id("keyboard-shortcut-list")
			.flex()
			.flex_col()
			.max_h(px(340.0))
			.overflow_y_scroll();
		let mut shown_section: Option<String> = None;
		for (index, row) in self.rows.iter().enumerate() {
			let label = i18n::tr(row.action.entry().i18n_key);
			let shortcut = crate::actions::display_shortcut(row.action);
			if !keyboard_filter_matches(label, &row.path, shortcut.as_deref(), &self.filter) {
				continue;
			}
			if shown_section.as_deref() != Some(row.section.as_str()) {
				list = list.child(section_header(&colors, row.section.clone().into()));
				shown_section = Some(row.section.clone());
			}
			let row_selected = selected == Some(index);
			let is_capturing = capturing == Some(index);
			let field_label: SharedString = if is_capturing {
				i18n::tr("preferences.keyboard.capturing").into()
			} else {
				shortcut
					.map(SharedString::from)
					.unwrap_or_else(|| i18n::tr("preferences.keyboard.unbound").into())
			};
			let row_path = row.path.clone();
			let focus = row.focus.clone();
			list = list.child(
				div()
					.id(ElementId::named_usize("keyboard-shortcut-row", index))
					.flex()
					.items_center()
					.gap_2()
					.px_1()
					.py_0p5()
					.rounded_md()
					.bg(if row_selected {
						colors.selected
					} else {
						transparent()
					})
					.on_click(cx.listener(move |this, _event, _window, cx| {
						this.selected = Some(index);
						cx.notify();
					}))
					.child(
						div()
							.flex_1()
							.flex_col()
							.child(div().text_color(colors.text).child(label))
							.child(div().text_color(colors.disabled).text_xs().child(row_path)),
					)
					.child(
						div()
							.id(ElementId::named_usize("keyboard-shortcut-capture", index))
							.debug_selector(move || format!("keyboard-capture-{index}"))
							.min_w(px(150.0))
							.px_2()
							.py_0p5()
							.rounded_md()
							.border_1()
							.border_color(if is_capturing {
								colors.selected
							} else {
								colors.border
							})
							.bg(colors.background)
							.hover(|style| style.border_color(colors.selected))
							.text_color(colors.text)
							.cursor_pointer()
							.track_focus(&focus)
							.on_click(cx.listener(move |this, _event, _window, cx| {
								if this.capturing != Some(index) {
									this.begin_capture(index, cx);
								}
								cx.stop_propagation();
							}))
							.child(field_label),
					),
			);
		}

		// The footer: Import/Export on the left, Reset Selected/All on the
		// right (the inline Reset-All confirmation replaces them when armed).
		let footer = if self.confirm_reset_all {
			div()
				.flex()
				.items_center()
				.gap_2()
				.child(
					div()
						.flex_1()
						.text_color(colors.text)
						.child(i18n::tr("preferences.keyboard.reset_all.confirm")),
				)
				.child(
					pill_button(
						"prefs-keyboard-confirm-reset",
						i18n::tr("preferences.keyboard.reset_all"),
						colors.selected,
						crate::widgets::theme::accent_hover(&colors),
						colors.selected_text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| this.reset_all(cx))),
				)
				.child(
					pill_button(
						"prefs-keyboard-cancel-reset",
						i18n::tr("dialog.cancel"),
						colors.background,
						crate::widgets::theme::neutral_hover(&colors),
						colors.text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| {
						this.confirm_reset_all = false;
						cx.notify();
					})),
				)
		} else {
			div()
				.flex()
				.items_center()
				.gap_2()
				.child(
					pill_button(
						"prefs-keyboard-import",
						i18n::tr("preferences.keyboard.import"),
						colors.background,
						crate::widgets::theme::neutral_hover(&colors),
						colors.text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| this.import_shortcuts(cx))),
				)
				.child(
					pill_button(
						"prefs-keyboard-export",
						i18n::tr("preferences.keyboard.export"),
						colors.background,
						crate::widgets::theme::neutral_hover(&colors),
						colors.text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| this.export_shortcuts(cx))),
				)
				.child(div().flex_1())
				.child(
					pill_button(
						"prefs-keyboard-reset-selected",
						i18n::tr("preferences.keyboard.reset_selected"),
						colors.background,
						crate::widgets::theme::neutral_hover(&colors),
						colors.text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| this.reset_selected(cx))),
				)
				.child(
					pill_button(
						"prefs-keyboard-reset-all",
						i18n::tr("preferences.keyboard.reset_all"),
						colors.background,
						crate::widgets::theme::neutral_hover(&colors),
						colors.text,
					)
					.on_click(cx.listener(|this, _event, _window, cx| this.reset_all(cx))),
				)
		};

		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(search)
			.child(
				div()
					.flex()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("preferences.keyboard.action"))
					.child(div().flex_1())
					.child(i18n::tr("preferences.keyboard.shortcut")),
			)
			.child(list)
			.child(footer)
			.child(if let Some(status) = &self.status {
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(status.clone())
			} else {
				div()
			})
	}
}

/// The outcome of one captured keystroke.
#[derive(Debug)]
pub(crate) enum CaptureDecision {
	/// A modifier-only key — keep capturing, ignore it.
	Ignore,
	/// Escape — cancel the capture without changing anything.
	Cancel,
	/// Backspace / Delete — clear the binding (unbind).
	Clear,
	/// A real key — the new binding, in canonical gpui keystroke form.
	Assign(String),
}

/// Decides what a capture field should do with a keystroke. Modifier-only
/// keys (a bare Shift / Ctrl / …) never bind; Backspace and Delete clear;
/// Escape cancels; anything else (with or without modifiers) becomes the new
/// binding.
pub(crate) fn capture_decision(keystroke: &Keystroke) -> CaptureDecision {
	match keystroke.key.as_str() {
		"escape" => CaptureDecision::Cancel,
		"backspace" | "delete" => CaptureDecision::Clear,
		// Modifier-only key presses (the parser represents a bare modifier as
		// the modifier's own key name).
		"shift" | "control" | "alt" | "cmd" | "super" | "win" | "fn" | "function" | "secondary"
		| "platform" => CaptureDecision::Ignore,
		"" => CaptureDecision::Ignore,
		_ => CaptureDecision::Assign(keystroke.unparse()),
	}
}

/// Whether an action row survives the keyboard tab's search query:
/// case-insensitive match against the action label, the localized menu path,
/// or the effective shortcut label.
pub(crate) fn keyboard_filter_matches(
	label: &str,
	path: &str,
	shortcut: Option<&str>,
	query: &str,
) -> bool {
	let query = query.trim();
	if query.is_empty() {
		return true;
	}
	let query = query.to_lowercase();
	let label_lower = label.to_lowercase();
	let full_path = format!("{path} > {label}").to_lowercase();
	label_lower.contains(&query)
		|| full_path.contains(&query)
		|| shortcut.is_some_and(|s| s.to_lowercase().contains(&query))
}

// ---------------------------------------------------------------------------
// Action search (Help > Search Actions…, the `/` key)
// ---------------------------------------------------------------------------

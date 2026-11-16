use super::*;

/// One item of the action search list.
pub(crate) struct ActionSearchItem {
	pub(crate) action: ActionId,
	/// The localized "Menu > Submenu > …" path.
	pub(crate) path: String,
}

/// The action search dialog content (the C++ `ActionSearch`): a search field
/// over every menu-bar action, live filtering, arrow-key selection and
/// Enter / double-click execution through the same dispatch path the menu
/// clicks take. Panel-context hotkeys (the `HIDDEN_MENU_ID` items) never
/// appear — they have no menu item, exactly like the C++ "only the menu bar"
/// enumeration.
pub struct ActionSearchContent {
	pub(crate) query: Entity<InputState>,
	pub(crate) items: Vec<ActionSearchItem>,
	pub(crate) filter: String,
	pub(crate) selection: Option<usize>,
	/// The keystroke interceptor that routes Up/Down/Enter to the list while
	/// the dialog is open (see [`ActionSearchContent::new`]).
	#[allow(dead_code)] // kept alive: dropping it unregisters the keystroke interceptor
	interceptor: Option<gpui_kit::Subscription>,
}

/// A request the action search dialog emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSearchEvent {
	/// The user activated `action`; the host dispatches it (the same path the
	/// menu clicks take) and closes the dialog.
	Execute(ActionId),
}

impl EventEmitter<ActionSearchEvent> for ActionSearchContent {}

impl ActionSearchContent {
	/// Builds the dialog with every menu-bar action, subscribes to the search
	/// field so filtering re-runs on every keystroke, and installs a keystroke
	/// interceptor that routes Up / Down / Enter to the list.
	///
	/// The interceptor is needed because the shell's global key bindings run
	/// *before* the widget-level key handlers and would swallow Up/Down (they
	/// are the GoToPrevCut/GoToNextCut keys) and Enter — the same reason the
	/// Keyboard tab captures its keys through an interceptor. The dialog is
	/// modal, so the only text input alive while the interceptor is active is
	/// the search field; every other keystroke passes through untouched (in
	/// the real app the IME delivers text to the focused input independently
	/// of the key map, so typing keeps working).
	///
	/// `window` is required to build the search field and to install the
	/// interceptor, and `cx` creates the entities and registers the subscription.
	/// The item list comes from [`crate::app::menu_action_paths`]; the filter
	/// starts empty and nothing is selected until the first keystroke or arrow
	/// press. The returned content owns the interceptor; dropping it removes the
	/// interceptor.
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let query = cx.new(|cx| InputState::new(window, cx));
		cx.subscribe(&query, |this, _query, event: &InputEvent, cx| {
			if !matches!(event, InputEvent::Change) {
				return;
			}
			this.filter = this.query.read(cx).value().to_string();
			let visible: Vec<usize> = this
				.items
				.iter()
				.enumerate()
				.filter(|(_, item)| search_filter_matches(item.action, &item.path, &this.filter))
				.map(|(index, _)| index)
				.collect();
			this.selection = selection_step(&visible, None, 1);
			cx.notify();
		})
		.detach();
		let items = crate::app::menu_action_paths()
			.into_iter()
			.map(|(action, path)| ActionSearchItem { action, path })
			.collect();
		let weak = cx.weak_entity();
		let interceptor = cx.intercept_keystrokes(move |event, _window, app| {
			// Only act on the keys the dialog owns; everything else (text
			// for the search field, Escape for the modal, …) passes through.
			if !matches!(event.keystroke.key.as_str(), "up" | "down" | "enter") {
				return;
			}
			let Some(this) = weak.upgrade() else {
				return;
			};
			let mut stop = false;
			this.update(app, |this, cx| match event.keystroke.key.as_str() {
				"up" => {
					this.move_selection(-1, cx);
					stop = true;
				}
				"down" => {
					this.move_selection(1, cx);
					stop = true;
				}
				"enter" => {
					this.execute(cx);
					stop = true;
				}
				// Escape deliberately falls through to the modal's own handler
				// (which closes the dialog); every other key passes to the
				// search field.
				_ => {}
			});
			if stop {
				app.stop_propagation();
			}
		});
		Self {
			query,
			items,
			filter: String::new(),
			selection: None,
			interceptor: Some(interceptor),
		}
	}

	/// The search field's focus handle (the host focuses it when the dialog
	/// opens, so the search is keyboard-first from the start).
	pub fn search_focus(&self, cx: &App) -> FocusHandle {
		self.query.read(cx).focus_handle(cx)
	}

	/// Moves the selection by `delta` visible rows, wrapping around.
	///
	/// Recomputes the visible (filter-matching) item indices and advances the
	/// selection through [`selection_step`]: positive `delta` moves down, negative
	/// up, and the position wraps modulo the number of visible rows. With no
	/// current selection a positive `delta` selects the first row and a negative
	/// one the last. When the filter matches nothing the selection is cleared.
	/// `cx` is notified so the list repaints; the Up/Down interceptor calls this.
	pub(crate) fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
		let visible: Vec<usize> = self
			.items
			.iter()
			.enumerate()
			.filter(|(_, item)| search_filter_matches(item.action, &item.path, &self.filter))
			.map(|(index, _)| index)
			.collect();
		self.selection = selection_step(&visible, self.selection, delta);
		cx.notify();
	}

	/// The currently selected action (tests).
	pub fn selected_action(&self) -> Option<ActionId> {
		self.selection
			.and_then(|index| self.items.get(index))
			.map(|item| item.action)
	}

	/// The current search filter (tests).
	pub fn filter(&self) -> &str {
		&self.filter
	}

	pub(crate) fn execute(&mut self, cx: &mut Context<Self>) {
		if let Some(index) = self.selection {
			if let Some(item) = self.items.get(index) {
				cx.emit(ActionSearchEvent::Execute(item.action));
			}
		}
	}
}

impl Render for ActionSearchContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let weak = self.query.downgrade();
		let selection = self.selection;
		let visible: Vec<usize> = self
			.items
			.iter()
			.enumerate()
			.filter(|(_, item)| search_filter_matches(item.action, &item.path, &self.filter))
			.map(|(index, _)| index)
			.collect();

		let list = if visible.is_empty() {
			div()
				.id("action-search-list")
				.flex()
				.flex_col()
				.max_h(px(360.0))
				.overflow_y_scroll()
				.child(div().text_color(colors.disabled).text_xs().child(
					if self.items.is_empty() {
						i18n::tr("actionsearch.no_actions")
					} else {
						i18n::tr("actionsearch.empty")
					},
				))
		} else {
			div()
				.id("action-search-list")
				.flex()
				.flex_col()
				.max_h(px(360.0))
				.overflow_y_scroll()
				.children(visible.iter().map(|&index| {
					let item = &self.items[index];
					let label = i18n::tr(item.action.entry().i18n_key);
					let path = item.path.clone();
					let row_selected = selection == Some(index);
					div()
						.id(ElementId::named_usize("action-search-item", index))
						.flex()
						.items_center()
						.gap_2()
						.px_2()
						.py_0p5()
						.rounded_md()
						.bg(if row_selected {
							colors.selected
						} else {
							transparent()
						})
						.hover(|style| {
							if row_selected {
								style
							} else {
								style.bg(crate::widgets::theme::row_hover(&colors))
							}
						})
						.text_color(if row_selected {
							colors.selected_text
						} else {
							colors.text
						})
						.cursor_pointer()
						.on_click(cx.listener(move |this, _event, _window, cx| {
							this.selection = Some(index);
							cx.notify();
						}))
						.on_mouse_down(
							gpui_kit::MouseButton::Left,
							cx.listener(
								move |this, event: &gpui_kit::MouseDownEvent, _window, cx| {
									// Double-click executes.
									if event.click_count >= 2 {
										this.selection = Some(index);
										this.execute(cx);
									}
								},
							),
						)
						.child(
							div().flex_1().flex_col().child(div().child(label)).child(
								div()
									.text_xs()
									.text_color(colors.disabled)
									.child(format!("({path})")),
							),
						)
				}))
		};

		div()
			.id("action-search-root")
			.flex()
			.flex_col()
			.gap_2()
			.w_full()
			.child(
				div()
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.bg(colors.background)
					.px_2()
					.py_1()
					.child(
						text_input("action-search-input", cx)
							.state(weak)
							.accepts_input(true),
					),
			)
			.child(list)
	}
}

/// Whether an action-search item survives the query: case-insensitive match
/// against the action label or the full "Menu > Submenu > action" path.
pub(crate) fn search_filter_matches(action: ActionId, path: &str, query: &str) -> bool {
	let query = query.trim();
	if query.is_empty() {
		return true;
	}
	let query = query.to_lowercase();
	let label = i18n::tr(action.entry().i18n_key);
	label.to_lowercase().contains(&query)
		|| format!("{path} > {label}").to_lowercase().contains(&query)
}

/// The next selected index when moving `delta` (±1) through `visible` (the
/// indices of the currently visible rows), wrapping around. `None` returns
/// the first (for a downward move) / last (for an upward move).
pub(crate) fn selection_step(
	visible: &[usize],
	current: Option<usize>,
	delta: i32,
) -> Option<usize> {
	if visible.is_empty() {
		return None;
	}
	let position = current.and_then(|c| visible.iter().position(|&v| v == c));
	let next = match position {
		Some(pos) => (pos as i64 + delta as i64).rem_euclid(visible.len() as i64) as usize,
		None if delta > 0 => 0,
		None => visible.len() - 1,
	};
	Some(visible[next])
}

// ---------------------------------------------------------------------------
// About (帮助 → 关于 Oak)
// ---------------------------------------------------------------------------

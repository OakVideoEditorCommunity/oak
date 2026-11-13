// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! The project manager (M13 D4): the DaVinci-style library browser the app
//! shows at startup (no `--project` argument) and from 文件 → 项目管理器.
//!
//! The view is a modal content view ([`ProjectManager`]) hosted by
//! `crate::app::OakApp` inside the standard `Modal` card: a toolbar
//! (新建 / 导入), a column list of the library rows (name, modified time,
//! duration, tracks, clips, footage — the backend-derived stats), and an
//! action row (打开 / 重命名 / 复制 / 删除 / 导出). The view emits
//! [`ManagerEvent`] requests; the app routes them through the engine
//! ([`AppEngine`]'s library surface) and swaps in the rename / delete
//! confirmation modals.
//!
//! Pure formatting helpers ([`format_modified`], [`format_duration_ms`])
//! are unit tested here; the app-level flows are covered by the
//! `OakApp<MockEngine>` tests in `crate::app`.

use crate::oakui::component::text_input;
use gpui_kit::colors::DefaultColors;
use gpui_kit::prelude::*;
use gpui_kit::{
	div, App, ClickEvent, Context, ElementId, Entity, EventEmitter, Hsla, Render, SharedString,
	Window,
};
use gpui_kit::base::input::InputState;

use crate::i18n;
use crate::oakui::{AppEngine, LibraryProject};

/// A request the manager view emits for the host (the app shell) to route
/// through the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
	/// Open the project (double-click / the 打开 button).
	Open(String),
	/// Create a new blank project and open it.
	Create,
	/// Rename the project (the host prompts for the new name).
	Rename(String),
	/// Duplicate the project (history included).
	Duplicate(String),
	/// Delete the project (the host confirms first).
	Delete(String),
	/// Import a `.ove` / `.otio` / `.fcpxml` file as a new library row.
	Import,
	/// Export the project to a file.
	Export(String),
}

/// The project manager content view: the library list plus its toolbars.
pub struct ProjectManager<E: AppEngine> {
	engine: Entity<E>,
	/// The listed rows (most recently modified first, as the engine
	/// reports them).
	rows: Vec<LibraryProject>,
	/// The selected row index.
	selected: Option<usize>,
	/// The last operation error (shown under the list).
	status: Option<String>,
}

impl<E: AppEngine> ProjectManager<E> {
	/// Builds the manager view and performs the initial library load.
	///
	/// `engine` is the shared engine entity the app also mutates, so the list
	/// reflects library changes after a [`Self::reload`]; `_window` is unused by
	/// the view but kept for the GPUI constructor signature; `cx` is the new
	/// view's context. The initial load goes through [`Self::reload`], which
	/// records an engine error in `status` instead of panicking.
	pub fn new(engine: Entity<E>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
		let mut this = Self {
			engine,
			rows: Vec::new(),
			selected: None,
			status: None,
		};
		this.reload(cx);
		this
	}

	/// Reloads the library from the engine, keeping the selection on the
	/// same row (by uuid) when it still exists.
	pub fn reload(&mut self, cx: &mut Context<Self>) {
		let selected_uuid = self.selected_uuid();
		match self.engine.read(cx).library_projects() {
			Ok(rows) => {
				self.rows = rows;
				self.selected = selected_uuid
					.and_then(|uuid| self.rows.iter().position(|row| row.uuid == uuid));
				self.status = None;
			}
			Err(err) => {
				self.rows = Vec::new();
				self.selected = None;
				self.status = Some(err);
			}
		}
		cx.notify();
	}

	/// The uuid of the selected row, cloned.
	///
	/// Returns `None` when nothing is selected or the stored index no longer
	/// matches a row. [`Self::reload`] uses it to keep the selection on the same
	/// project across a library refresh, and the app uses it to re-select a row
	/// after an action.
	pub fn selected_uuid(&self) -> Option<String> {
		self.selected
			.and_then(|index| self.rows.get(index))
			.map(|row| row.uuid.clone())
	}

	/// The selected row's display name, if any.
	pub fn selected_name(&self) -> Option<String> {
		self.selected
			.and_then(|index| self.rows.get(index))
			.map(|row| row.name.clone())
	}

	/// Shows, replaces or clears the error line under the list.
	///
	/// `status` is `Some(message)` for a failed engine operation (the app's
	/// `manager_status` forwards the engine error here so the failure stays
	/// visible inside the dialog) and `None` to clear it. The view repaints
	/// immediately so the text appears or disappears without another event.
	pub fn set_status(&mut self, status: Option<String>, cx: &mut Context<Self>) {
		self.status = status;
		cx.notify();
	}

	/// The listed rows (tests).
	#[cfg(test)]
	pub fn rows(&self) -> &[LibraryProject] {
		&self.rows
	}

	/// Selects the row with `uuid` (tests and the host's post-action
	/// reselection).
	pub fn select(&mut self, uuid: &str, cx: &mut Context<Self>) {
		self.selected = self.rows.iter().position(|row| row.uuid == uuid);
		cx.notify();
	}

	/// Emits a [`ManagerEvent`] to the view's subscribers and repaints.
	///
	/// The app shell is the subscriber and routes the request through the
	/// engine. Ownership of `event` moves to `cx.emit`; the view keeps no copy,
	/// so emit before mutating state that the handler may rely on.
	fn emit(&mut self, event: ManagerEvent, cx: &mut Context<Self>) {
		cx.emit(event);
		cx.notify();
	}

	/// Handles a click on the library row at `index`.
	///
	/// `clicks` is the click count reported by the UI: one selects the row, two
	/// or more also emits [`ManagerEvent::Open`] for it. Out-of-range indices
	/// are ignored, so a stale row element cannot select or open the wrong
	/// project.
	fn row_clicked(&mut self, index: usize, clicks: usize, cx: &mut Context<Self>) {
		if index >= self.rows.len() {
			return;
		}
		self.selected = Some(index);
		if clicks >= 2 {
			let uuid = self.rows[index].uuid.clone();
			self.emit(ManagerEvent::Open(uuid), cx);
		} else {
			cx.notify();
		}
	}

	/// Emits the action event for the currently selected row.
	///
	/// `action` is a constructor such as `ManagerEvent::Rename` or
	/// `ManagerEvent::Delete`; it is only called when a row is selected, and its
	/// result is emitted through [`Self::emit`]. With no selection the call is a
	/// silent no-op (no event, no repaint).
	fn selected_action(
		&mut self,
		action: impl FnOnce(String) -> ManagerEvent,
		cx: &mut Context<Self>,
	) {
		if let Some(uuid) = self.selected_uuid() {
			self.emit(action(uuid), cx);
		}
	}
}

impl<E: AppEngine> EventEmitter<ManagerEvent> for ProjectManager<E> {}

impl<E: AppEngine> Render for ProjectManager<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();

		// Toolbar buttons (新建 / 导入) and the selection-targeting action
		// row (打开 / 重命名 / 复制 / 删除 / 导出).
		let tool = |id: &'static str, label: &'static str| -> gpui_kit::Stateful<gpui_kit::Div> {
			div()
				.id(id)
				.px_3()
				.py_1()
				.rounded_md()
				.bg(colors.background)
				.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
				.border_1()
				.border_color(colors.border)
				.text_color(colors.text)
				.cursor_pointer()
				.child(label)
		};
		let has_selection = self.selected.is_some();
		let action = |id: &'static str, label: &'static str| -> gpui_kit::Stateful<gpui_kit::Div> {
			let mut b = div().id(id).px_3().py_1().rounded_md();
			if has_selection {
				b = b
					.bg(colors.background)
					.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
					.border_1()
					.border_color(colors.border)
					.text_color(colors.text)
					.cursor_pointer();
			} else {
				b = b.text_color(colors.disabled);
			}
			b.child(label)
		};

		// The column header.
		let header_cell = |label: &'static str, width: f32| -> gpui_kit::Div {
			let mut cell = div()
				.px_2()
				.text_xs()
				.text_color(colors.disabled)
				.whitespace_nowrap()
				.child(label);
			if width <= 0.0 {
				cell = cell.flex_1();
			} else {
				cell = cell.w(gpui_kit::px(width)).text_right();
			}
			cell
		};

		let list = div()
			.h(gpui_kit::px(340.0))
			.flex()
			.flex_col()
			.border_1()
			.border_color(colors.border)
			.rounded_md()
			.bg(colors.background)
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.py_1()
					.border_b_1()
					.border_color(colors.border)
					.child(header_cell(i18n::tr("manager.col.name"), 0.0))
					.child(header_cell(i18n::tr("manager.col.modified"), 128.0))
					.child(header_cell(i18n::tr("manager.col.duration"), 72.0))
					.child(header_cell(i18n::tr("manager.col.tracks"), 56.0))
					.child(header_cell(i18n::tr("manager.col.clips"), 56.0))
					.child(header_cell(i18n::tr("manager.col.footage"), 56.0)),
			)
			.child(
				div()
					.id("manager-list")
					.flex_1()
					.min_h_0()
					.overflow_y_scroll()
					.children(if self.rows.is_empty() {
						vec![div()
							.p_4()
							.text_color(colors.disabled)
							.child(i18n::tr("manager.empty"))
							.into_any_element()]
					} else {
						self.rows
							.iter()
							.enumerate()
							.map(|(index, row)| {
								let selected = self.selected == Some(index);
								let mut line = div()
									.id(ElementId::Name(format!("manager-row-{index}").into()))
									.flex()
									.items_center()
									.gap_2()
									.py_1()
									.cursor_pointer()
									.on_click(cx.listener(
										move |this, event: &ClickEvent, _w, cx| {
											this.row_clicked(index, event.click_count(), cx);
										},
									));
								if selected {
									line =
										line.bg(colors.selected).text_color(colors.selected_text);
								} else {
									line = line.text_color(colors.text).hover(|style| {
										style.bg(crate::widgets::theme::row_hover(&colors))
									});
								}
								let cell = |text: String, width: f32| -> gpui_kit::Div {
									let mut c = div().px_2().whitespace_nowrap().child(text);
									if width <= 0.0 {
										c = c.flex_1();
									} else {
										c = c.w(gpui_kit::px(width)).text_right();
									}
									c
								};
								line.child(cell(row.name.clone(), 0.0))
									.child(cell(format_modified(row.modified_at), 128.0))
									.child(cell(format_duration_ms(row.duration_ms), 72.0))
									.child(cell(row.track_count.to_string(), 56.0))
									.child(cell(row.clip_count.to_string(), 56.0))
									.child(cell(row.footage_count.to_string(), 56.0))
									.into_any_element()
							})
							.collect()
					}),
			);

		let mut root =
			div()
				.flex()
				.flex_col()
				.gap_3()
				.w_full()
				.child(
					div()
						.flex()
						.gap_2()
						.child(
							tool("manager-new", i18n::tr("manager.new")).on_click(cx.listener(
								|this, _e: &ClickEvent, _w, cx| {
									this.emit(ManagerEvent::Create, cx);
								},
							)),
						)
						.child(tool("manager-import", i18n::tr("manager.import")).on_click(
							cx.listener(|this, _e: &ClickEvent, _w, cx| {
								this.emit(ManagerEvent::Import, cx);
							}),
						)),
				)
				.child(list)
				.child(
					div()
						.flex()
						.justify_end()
						.gap_2()
						.child(action("manager-open", i18n::tr("manager.open")).on_click(
							cx.listener(|this, _e: &ClickEvent, _w, cx| {
								this.selected_action(ManagerEvent::Open, cx);
							}),
						))
						.child(
							action("manager-rename", i18n::tr("manager.rename")).on_click(
								cx.listener(|this, _e: &ClickEvent, _w, cx| {
									this.selected_action(ManagerEvent::Rename, cx);
								}),
							),
						)
						.child(
							action("manager-duplicate", i18n::tr("manager.duplicate")).on_click(
								cx.listener(|this, _e: &ClickEvent, _w, cx| {
									this.selected_action(ManagerEvent::Duplicate, cx);
								}),
							),
						)
						.child(
							action("manager-delete", i18n::tr("manager.delete")).on_click(
								cx.listener(|this, _e: &ClickEvent, _w, cx| {
									this.selected_action(ManagerEvent::Delete, cx);
								}),
							),
						)
						.child(
							action("manager-export", i18n::tr("manager.export")).on_click(
								cx.listener(|this, _e: &ClickEvent, _w, cx| {
									this.selected_action(ManagerEvent::Export, cx);
								}),
							),
						),
				);
		if let Some(status) = &self.status {
			root = root.child(
				div()
					.text_xs()
					.text_color(Hsla {
						h: 0.0,
						s: 0.6,
						l: 0.55,
						a: 1.0,
					})
					.child(status.clone()),
			);
		}
		root
	}
}

// ---------------------------------------------------------------------------
// Rename prompt / delete confirmation contents
// ---------------------------------------------------------------------------

/// The rename prompt's content: one text field with the new name.
pub struct NamePrompt {
	editor: Entity<InputState>,
}

impl NamePrompt {
	/// Builds the prompt with its single text field.
	///
	/// `initial` seeds the field (the project's current name when renaming, an
	/// empty string or a default factor for the other prompts); `window` and
	/// `cx` are the usual GPUI construction context. Read the edited text back
	/// with [`Self::value`].
	pub fn new(initial: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let editor = cx.new(|cx| InputState::new(window, cx));
		editor.update(cx, |editor, cx| editor.set_value(initial, window, cx));
		Self { editor }
	}

	/// The text currently entered, with leading/trailing whitespace trimmed.
	///
	/// `app` is only needed to read the inner input entity, so this can be
	/// called without the view's own context. Returns an empty string when the
	/// field is empty or whitespace-only; the app treats that as "cancel" rather
	/// than an invalid name.
	pub fn value(&self, app: &App) -> String {
		self.editor.read(app).value().trim().to_string()
	}

	/// Replaces the text currently in the prompt's field.
	///
	/// `value` overwrites the whole field (it is not appended); `window` and
	/// `cx` are forwarded to the inner input state, and the view repaints so the
	/// new text is visible. Used by tests to fill the prompt; the normal host
	/// flow only seeds the initial value through [`Self::new`].
	pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
		self.editor
			.update(cx, |editor, cx| editor.set_value(value, window, cx));
		cx.notify();
	}
}

impl Render for NamePrompt {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let weak = self.editor.downgrade();
		div()
			.flex()
			.flex_col()
			.gap_1()
			.w_full()
			.child(
				div()
					.text_color(colors.text)
					.child(i18n::tr("manager.rename.label")),
			)
			.child(
				div()
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.bg(colors.background)
					.px_2()
					.py_1()
					.child(
						text_input("gpui-widgets-rename-field", cx)
							.state(weak)
							.accepts_input(true),
					),
			)
	}
}

/// The delete confirmation's content: the warning text.
pub struct ConfirmContent {
	text: SharedString,
}

impl ConfirmContent {
	/// Builds the delete-confirmation body around `text`.
	///
	/// `text` is the warning shown in the modal (the app passes the already
	/// localized "delete project {name}?" message); anything convertible into a
	/// `SharedString` is accepted and rendered verbatim by this type's `Render`
	/// impl.
	pub fn new(text: impl Into<SharedString>) -> Self {
		Self { text: text.into() }
	}
}

impl Render for ConfirmContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div().text_color(colors.text).child(self.text.clone())
	}
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Formats a unix timestamp (UTC) as `YYYY-MM-DD HH:MM` for the modified
/// column.
pub fn format_modified(unix_secs: i64) -> String {
	if unix_secs <= 0 {
		return "—".to_string();
	}
	let days = unix_secs.div_euclid(86_400);
	let secs = unix_secs.rem_euclid(86_400);
	let (year, month, day) = civil_from_days(days);
	format!(
		"{year:04}-{month:02}-{day:02} {:02}:{:02}",
		secs / 3600,
		(secs % 3600) / 60
	)
}

/// Days since the unix epoch → (year, month, day), UTC (Howard Hinnant's
/// civil-from-days algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
	let z = days + 719_468;
	let era = z.div_euclid(146_097);
	let doe = z.rem_euclid(146_097);
	let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
	let year = yoe + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
	let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
	let year = if month <= 2 { year + 1 } else { year };
	(year, month, day)
}

/// Formats a duration in milliseconds as `H:MM:SS` for the duration
/// column.
pub fn format_duration_ms(ms: i64) -> String {
	if ms <= 0 {
		return "—".to_string();
	}
	let secs = ms / 1000;
	format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The civil-date conversion matches known dates.
	#[test]
	fn format_modified_known_dates() {
		assert_eq!(format_modified(0), "—");
		assert_eq!(format_modified(-5), "—");
		// 2026-08-16 00:54:01 UTC.
		assert_eq!(format_modified(1_786_841_641), "2026-08-16 00:54");
		// 1970-01-01 00:00 UTC.
		assert_eq!(format_modified(1), "1970-01-01 00:00");
		// 2000-02-29 (a leap day) 12:34 UTC.
		assert_eq!(format_modified(951_827_640), "2000-02-29 12:34");
	}

	/// Durations format as H:MM:SS with a dash for empty projects.
	#[test]
	fn format_duration_ms_shapes() {
		assert_eq!(format_duration_ms(0), "—");
		assert_eq!(format_duration_ms(61_500), "0:01:01");
		assert_eq!(format_duration_ms(3_725_000), "1:02:05");
	}
}

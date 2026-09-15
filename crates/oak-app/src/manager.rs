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
use gpui::colors::DefaultColors;
use gpui::prelude::*;
use gpui::{
	div, App, ClickEvent, Context, ElementId, Entity, EventEmitter, Hsla, Render, SharedString,
	Window,
};
use gpui_elements::editable_text::{EditableTextState, StringStorage};

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
	/// Builds the view and loads the library.
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

	/// The selected row's uuid, if any.
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

	/// Shows an operation error under the list (the host reports engine
	/// failures here so a failed action is visible in the dialog).
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

	/// Emits the event through the view's subscribers.
	fn emit(&mut self, event: ManagerEvent, cx: &mut Context<Self>) {
		cx.emit(event);
		cx.notify();
	}

	/// A click on row `index`: select, or open on a double click.
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

	/// An action-row button targeting the selection.
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
		let tool = |id: &'static str, label: &'static str| -> gpui::Stateful<gpui::Div> {
			div()
				.id(id)
				.px_3()
				.py_1()
				.rounded_md()
				.bg(colors.background)
				.border_1()
				.border_color(colors.border)
				.text_color(colors.text)
				.cursor_pointer()
				.child(label)
		};
		let has_selection = self.selected.is_some();
		let action = |id: &'static str, label: &'static str| -> gpui::Stateful<gpui::Div> {
			let mut b = div().id(id).px_3().py_1().rounded_md();
			if has_selection {
				b = b
					.bg(colors.background)
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
		let header_cell = |label: &'static str, width: f32| -> gpui::Div {
			let mut cell = div()
				.px_2()
				.text_xs()
				.text_color(colors.disabled)
				.whitespace_nowrap()
				.child(label);
			if width <= 0.0 {
				cell = cell.flex_1();
			} else {
				cell = cell.w(gpui::px(width)).text_right();
			}
			cell
		};

		let list = div()
			.h(gpui::px(340.0))
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
									line = line.text_color(colors.text);
								}
								let cell = |text: String, width: f32| -> gpui::Div {
									let mut c = div().px_2().whitespace_nowrap().child(text);
									if width <= 0.0 {
										c = c.flex_1();
									} else {
										c = c.w(gpui::px(width)).text_right();
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
	editor: Entity<EditableTextState>,
}

impl NamePrompt {
	/// Builds the prompt seeded with `initial`.
	pub fn new(initial: &str, cx: &mut Context<Self>) -> Self {
		let editor = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
		editor.update(cx, |editor, cx| editor.emplace(initial, cx));
		Self { editor }
	}

	/// The name currently entered (trimmed).
	pub fn value(&self, app: &App) -> String {
		self.editor.read(app).as_str().trim().to_string()
	}

	/// Replaces the entered name (tests / prefill).
	pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
		self.editor
			.update(cx, |editor, cx| editor.emplace(value, cx));
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
	/// Builds the confirmation with `text`.
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

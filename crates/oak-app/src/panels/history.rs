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

//! The history panel (历史记录): the real undo stack as a list, mirroring
//! the C++ `HistoryWidget` — two columns (number + action), every command
//! on the stack (done first, then the redoable tail), undone rows gray,
//! the row under the stack pointer selected. Clicking a row jumps the
//! stack to it (`row + 1`); a right-click opens a context menu with
//! undo/redo and a jump-to-row action.

use gpui::colors::DefaultColors;
use gpui::dock::{DockPanel, PanelEvent};
use gpui::{
	div, prelude::*, px, AnyElement, App, Context, ElementId, Entity, EventEmitter, MouseButton,
	MouseDownEvent, Render, SharedString, Window,
};
use crate::oakui::component::menu::{ContextMenu, ContextMenuEvent, Menu, MenuItem};

use crate::oakui::AppEngine;
use crate::panels::commands::PanelCommandHandler;
use crate::panels::ids::HISTORY;

/// Context-menu item ids.
const MENU_UNDO: usize = 1;
const MENU_REDO: usize = 2;
const MENU_JUMP_HERE: usize = 3;

/// The history panel over the engine's undo stack.
pub struct HistoryPanel<E: AppEngine> {
	engine: Entity<E>,
	/// The right-click menu (hidden until a row is right-clicked).
	menu: Entity<ContextMenu>,
	/// The row under the last right-click (the "jump here" target).
	menu_row: Option<usize>,
}

impl<E: AppEngine> HistoryPanel<E> {
	/// Builds the panel over `engine`'s undo stack.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		// The stack changes whenever the engine notifies (every edit,
		// undo, redo and jump runs through the engine), so re-reading on
		// observe keeps the rows, the graying and the selection live —
		// the C++ model reset itself on Core::undo_index_changed.
		cx.observe(&engine, |_this, _engine, cx| cx.notify()).detach();

		let menu = cx.new(|cx| ContextMenu::new(0, window, cx));
		cx.subscribe(&menu, |this, _menu, event: &ContextMenuEvent, cx| {
			this.on_menu(event.item, cx);
		})
		.detach();

		Self {
			engine,
			menu,
			menu_row: None,
		}
	}

	/// Routes a context-menu action.
	fn on_menu(&mut self, item: usize, cx: &mut Context<Self>) {
		match item {
			MENU_UNDO => self.engine.update(cx, |engine, cx| engine.undo(cx)),
			MENU_REDO => self.engine.update(cx, |engine, cx| engine.redo(cx)),
			MENU_JUMP_HERE => {
				if let Some(row) = self.menu_row {
					self.engine
						.update(cx, |engine, cx| engine.jump_history(row as i64 + 1, cx));
				}
			}
			_ => {}
		}
	}

	/// Shows the right-click menu at `position` (window coordinates).
	fn show_menu(
		&mut self,
		row: Option<usize>,
		position: gpui::Point<gpui::Pixels>,
		cx: &mut Context<Self>,
	) {
		let can_undo = self.engine.read(cx).can_undo();
		let can_redo = self.engine.read(cx).can_redo();
		let mut items = vec![
			MenuItem::new(MENU_UNDO, crate::i18n::tr("menu.edit.undo")).with_shortcut("⌘Z"),
			MenuItem::new(MENU_REDO, crate::i18n::tr("menu.edit.redo")).with_shortcut("⇧⌘Z"),
		];
		if !can_undo {
			items[0] = items[0].clone().disabled();
		}
		if !can_redo {
			items[1] = items[1].clone().disabled();
		}
		if let Some(row) = row {
			self.menu_row = Some(row);
			items.push(MenuItem::new(MENU_JUMP_HERE, crate::i18n::tr("history.jump_here")).separated());
		} else {
			self.menu_row = None;
		}
		self.menu.update(cx, |menu, cx| {
			menu.show(position, Menu::new(items), cx);
		});
	}
}

/// The history panel implements no focused-panel commands: everything falls
/// through to the shell's global handler.
impl<E: AppEngine> PanelCommandHandler for HistoryPanel<E> {}

impl<E: AppEngine> Render for HistoryPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let entries = self.engine.read(cx).history_entries();
		let index = self.engine.read(cx).history_index();
		// The C++ widget selects row `index - 1` (the newest done
		// command); the bottom empty command keeps the index >= 1.
		let selected = (index - 1).max(0) as usize;

		let mut list = div()
			.id("history-list")
			.flex_1()
			.flex()
			.flex_col()
			.py_1()
			.overflow_y_scroll();

		for (row, entry) in entries.iter().enumerate() {
			let undone = !entry.done;
			let is_selected = row == selected;
			// The C++ model falls back to tr("Command") for rows without
			// a label (the bottom "New/Open Project" command).
			let label: SharedString = if entry.name.is_empty() {
				crate::i18n::tr("history.command").into()
			} else {
				entry.name.clone().into()
			};
			let row_color = if undone { colors.disabled } else { colors.text };

			list = list.child(
				div()
					.id(ElementId::named_usize("history-row", row))
					.flex()
					.items_center()
					.gap_2()
					.px_3()
					.py_1()
					.cursor_pointer()
					.when(is_selected, |el| el.bg(colors.selected))
					.on_click(cx.listener(move |this, _event: &gpui::ClickEvent, _window, cx| {
						// A left click jumps to this row (the C++
						// currentRowChanged → oakengine_undo_jump(row+1)).
						this.engine
							.update(cx, |engine, cx| engine.jump_history(row as i64 + 1, cx));
						cx.stop_propagation();
					}))
					.on_mouse_down(
						MouseButton::Right,
						cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
							this.show_menu(Some(row), event.position, cx);
							cx.stop_propagation();
						}),
					)
					.child(
						// The number column (C++ column 0: row + 1).
						div()
							.w(px(28.0))
							.flex_shrink_0()
							.text_right()
							.text_color(colors.disabled)
							.child(format!("{}", row + 1)),
					)
					.child(
						div()
							.flex_1()
							.min_w_0()
							.overflow_hidden()
							.whitespace_nowrap()
							.text_ellipsis()
							.text_color(if is_selected { colors.selected_text } else { row_color })
							.child(label),
					),
			);
		}

		// An empty stack (no project open) gets a quiet placeholder so the
		// panel does not read as broken.
		let body: AnyElement = if entries.is_empty() {
			div()
				.size_full()
				.flex()
				.items_center()
				.justify_center()
				.text_color(colors.disabled)
				.child(crate::i18n::tr("history.empty"))
				.into_any_element()
		} else {
			list.into_any_element()
		};

		div()
			.size_full()
			.flex()
			.flex_col()
			// Any left click inside the panel makes it the focused panel
			// (the dock re-emits this as `DockEvent::PanelFocused`, which
			// the shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
					cx.emit(PanelEvent::Focused);
				})
			})
			// Right-clicking the blank area below the rows still opens
			// the menu (without the row-specific "jump here" action).
			.on_mouse_down(
				MouseButton::Right,
				cx.listener(|this, event: &MouseDownEvent, _window, cx| {
					this.show_menu(None, event.position, cx);
					cx.stop_propagation();
				}),
			)
			.child(body)
			.child(self.menu.clone())
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for HistoryPanel<E> {}

impl<E: AppEngine> DockPanel for HistoryPanel<E> {
	fn panel_id(&self) -> gpui::dock::PanelId {
		HISTORY
	}

	fn title(&self, _cx: &App) -> SharedString {
		crate::i18n::tr("panel.history").into()
	}

	fn tab_content(&self, _cx: &App) -> AnyElement {
		div()
			.child(crate::i18n::tr("panel.history"))
			.into_any_element()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::MockEngine;
	use gpui::{size, Modifiers, TestAppContext, VisualTestContext};

	/// Builds the panel in a window and returns a `VisualTestContext`.
	fn panel_window(
		cx: &mut TestAppContext,
	) -> (
		&'static mut VisualTestContext,
		Entity<HistoryPanel<MockEngine>>,
	) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(320.0), px(400.0)), |window, cx| {
			let engine = cx.new(MockEngine::demo);
			HistoryPanel::new(engine, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("history panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, panel)
	}

	/// The mock engine keeps no undo stack, so the panel renders the empty
	/// placeholder rather than stale demo rows.
	#[gpui::test]
	async fn empty_stack_shows_the_placeholder(cx: &mut TestAppContext) {
		let (cx, _panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		// No rows rendered; the placeholder branch owns the body.
		assert!(
			cx.debug_bounds("history-row-0").is_none(),
			"no history rows without an undo stack"
		);
	}

	/// Right-clicking the blank panel body opens the context menu with the
	/// undo/redo entries (both disabled on an empty stack).
	#[gpui::test]
	async fn right_click_opens_the_context_menu(cx: &mut TestAppContext) {
		let (cx, _panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(cx.debug_bounds("menu-popup").is_none(), "menu starts hidden");

		cx.simulate_mouse_down(
			gpui::point(px(160.0), px(200.0)),
			MouseButton::Right,
			Modifiers::none(),
		);
		cx.run_until_parked();
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});

		let popup = cx
			.debug_bounds("menu-popup")
			.expect("context menu opened on right-click");
		assert!(popup.size.height > px(20.0), "popup lists the items");
	}

	/// A real engine's undo stack renders as rows (the row-list branch of
	/// `render`); the menu actions route to the engine, the blank-area
	/// right-click opens the menu, and the dock metadata is populated.
	#[gpui::test]
	async fn rows_render_and_interact(cx: &mut TestAppContext) {
		let _lock = crate::oakui::graphops::test_lock();
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(320.0), px(400.0)), |window, cx| {
			let engine = cx.new(crate::oakui::RealEngine::create);
			HistoryPanel::new(engine, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("history panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

		// Seed a project and one undoable edit through the panel's engine:
		// the panel now renders the row list instead of the placeholder.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.engine.update(cx, |engine, cx| {
					engine.new_project(cx);
					engine.add_track(gpui::timeline::TrackKind::Video, cx);
				});
			});
		});
		cx.run_until_parked();
		let entries = cx.read(|app| panel.read(app).engine.read(app).history_entries().len());
		assert!(entries >= 2, "new project + add track: {entries}");
		let total = entries as i64;
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).history_index()),
			total,
			"the pointer starts at the top of the seeded stack"
		);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Dock metadata.
		cx.update(|_window, app| {
			assert!(!panel.read(app).title(app).is_empty());
			assert_eq!(panel.read(app).panel_id(), HISTORY, "the dock routes by id");
			// `tab_content` only builds an element with no mounted bounds to
			// inspect in this test, so the call is a no-panic smoke check.
			let _ = panel.read(app).tab_content(app);
		});

		// Left click focuses the panel (the dock re-emits the event).
		cx.simulate_mouse_down(
			gpui::point(px(160.0), px(380.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		// Right-clicking the blank body opens the menu without a row.
		cx.simulate_mouse_down(
			gpui::point(px(160.0), px(380.0)),
			MouseButton::Right,
			Modifiers::none(),
		);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(cx.debug_bounds("menu-popup").is_some(), "blank-area menu opens");

		// Every menu action routes to the engine; unknown ids are ignored,
		// and show_menu toggles between row and blank-area targets.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				// `menu_row` is still None: jump-here is a no-op and the
				// pointer stays at the top.
				panel.on_menu(MENU_JUMP_HERE, cx);
				assert_eq!(panel.engine.read(cx).history_index(), total);

				// Undo/redo route to the engine stack: the pointer moves,
				// the row count stays, and the top row joins/leaves the
				// redoable tail.
				panel.on_menu(MENU_UNDO, cx);
				assert_eq!(panel.engine.read(cx).history_index(), total - 1);
				let entries = panel.engine.read(cx).history_entries();
				assert_eq!(entries.len(), total as usize, "undo keeps rows listed");
				assert!(!entries.last().expect("rows").done, "the undone row is gray");
				panel.on_menu(MENU_REDO, cx);
				assert_eq!(panel.engine.read(cx).history_index(), total);
				assert!(
					panel
						.engine
						.read(cx)
						.history_entries()
						.last()
						.expect("rows")
						.done,
					"redo restores the top row"
				);

				// Unknown ids are ignored (the pointer does not move).
				panel.on_menu(9999, cx);
				assert_eq!(panel.engine.read(cx).history_index(), total);

				panel.show_menu(None, gpui::point(px(5.0), px(5.0)), cx);
				assert_eq!(panel.menu_row, None);
				panel.show_menu(Some(0), gpui::point(px(5.0), px(5.0)), cx);
				assert_eq!(panel.menu_row, Some(0));

				// Jump-here targets the last right-clicked row: row 0 maps
				// to stack index 0 + 1 and undoes everything above it.
				panel.on_menu(MENU_JUMP_HERE, cx);
				assert_eq!(panel.engine.read(cx).history_index(), 1);
			});
		});
	}
}

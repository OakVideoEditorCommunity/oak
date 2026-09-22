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

//! The material bin panel (项目): the `ProjectExplorer` widget over the
//! engine's project data.

use std::process::Command;

use gpui::colors::DefaultColors;
use gpui::dock::{DockPanel, PanelEvent};
use gpui::{
	div, px, prelude::*, AnyElement, App, ClickEvent, Context, Entity, EventEmitter, MouseButton,
	PathPromptOptions, Pixels, Point, Render, SharedString, Window,
};
use crate::oakui::component::menu::{Menu, MenuItem};
use gpui_widgets::project_explorer::{ProjectExplorer, ProjectExplorerEvent};
use gpui_widgets::tooltip::tooltip_view;

use crate::actions::ActionId;
use crate::oakui::component::menu::{ContextMenuHandle, ContextMenuTriggered};
use crate::oakui::component::menu;
use crate::oakui::AppEngine;
use crate::panels::commands::PanelCommandHandler;
use crate::panels::ids::PROJECT;

/// The material bin panel.
pub struct ProjectExplorerPanel<E: AppEngine> {
	explorer: Entity<ProjectExplorer<E>>,
	engine: Entity<E>,
	/// The right-click context menu.
	context_menu: ContextMenuHandle,
	/// The entry under the currently open context menu (`None` = the menu
	/// was opened on the empty area).
	context_entry: Option<u64>,
}

impl<E: AppEngine> ProjectExplorerPanel<E> {
	/// Builds the explorer over `engine`'s project data.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let explorer = cx.new(|cx| ProjectExplorer::new(1, engine.clone(), window, cx));
		let context_menu = ContextMenuHandle::new(Self::on_local_menu_item, window, cx);
		cx.subscribe(
			&explorer,
			|this, _explorer, event: &ProjectExplorerEvent, cx| match event {
				ProjectExplorerEvent::OpenRequested { id, .. } => {
					// A sequence entry opens in the timeline (the
					// double-click on a sequence acts like opening it);
					// footage entries select into the source viewer.
					if this.engine.read(cx).entry_is_sequence(*id) {
						this.engine
							.update(cx, |engine, cx| engine.open_sequence_id(*id, cx));
					} else {
						this.engine
							.update(cx, |engine, cx| engine.select_item(*id, cx));
					}
				}
				ProjectExplorerEvent::FileDropRequested { paths, .. } => {
					// Drag-and-drop import: probe and add each dropped file
					// (the first failure is logged after the rest run).
					let mut first_error = None;
					for path in paths {
						if let Err(err) =
							this.engine
								.update(cx, |engine, cx| engine.import_footage(path.clone(), cx))
						{
							first_error.get_or_insert(err);
						}
					}
					if let Some(err) = first_error {
						println!("[project explorer] import failed: {err}");
					}
				}
				ProjectExplorerEvent::ContextMenuRequested { id, position, .. } => {
					this.open_context_menu(*id, *position, cx);
				}
				other => println!("[project explorer] request: {other:?}"),
			},
		)
		.detach();

		Self {
			explorer,
			engine,
			context_menu,
			context_entry: None,
		}
	}

	/// Opens the context menu for `id` (`None` = the empty area) at
	/// `position`.
	fn open_context_menu(
		&mut self,
		id: Option<u64>,
		position: Point<Pixels>,
		cx: &mut Context<Self>,
	) {
		self.context_entry = id;
		let menu = match id {
			None => blank_menu(),
			Some(id) => {
				if self.engine.read(cx).entry_path(id).is_some() {
					let proxy = self.engine.read(cx).proxy_row(id);
					footage_menu(true, proxy.as_ref())
				} else if self.engine.read(cx).entry_is_sequence(id) {
					sequence_menu()
				} else {
					entry_menu()
				}
			}
		};
		self.context_menu.show(position, menu, cx);
	}

	/// Handles the panel's local (non-registry) context-menu items.
	fn on_local_menu_item(&mut self, item: usize, cx: &mut Context<Self>) {
		match item {
			LOCAL_REVEAL_IN_FINDER => {
				let path = self
					.context_entry
					.and_then(|id| self.engine.read(cx).entry_path(id));
				if let Some(path) = path {
					reveal_in_finder(&path);
				}
			}
			LOCAL_REPLACE_FOOTAGE => {
				let Some(id) = self.context_entry else {
					return;
				};
				let receiver = cx.prompt_for_paths(PathPromptOptions {
					files: true,
					directories: false,
					multiple: false,
					prompt: Some(crate::i18n::tr("project.context.replace_footage").into()),
				allowed_extensions: Vec::new(),
				});
				cx.spawn(async move |this, cx| {
					if let Ok(Ok(Some(paths))) = receiver.await {
						if let Some(path) = paths.into_iter().next() {
							let _ = this.update(cx, |this, cx| {
								if let Err(err) = this.engine.update(cx, |engine, cx| {
									engine.replace_footage(id, path.clone(), cx)
								}) {
									println!("[project explorer] replace failed: {err}");
								}
							});
						}
					}
				})
				.detach();
			}
			LOCAL_RENAME => {
				let Some(id) = self.context_entry else {
					return;
				};
				cx.emit(RenameRequested(id));
			}
			LOCAL_DELETE => {
				let Some(id) = self.context_entry else {
					return;
				};
				cx.emit(DeleteRequested(id));
			}
			LOCAL_OPEN_IN_NEW_TAB => {
				println!("[project explorer] menu action {item} (not implemented yet)");
			}
			LOCAL_PROPERTIES => {
				let Some(id) = self.context_entry else {
					return;
				};
				if self.engine.read(cx).entry_is_sequence(id) {
					cx.emit(SequencePropertiesRequested(id));
				} else {
					println!("[project explorer] properties for non-sequence entry {id} (not implemented yet)");
				}
			}
			LOCAL_EXPORT_SEQUENCE => {
				let Some(id) = self.context_entry else {
					return;
				};
				cx.emit(ExportSequenceRequested(id));
			}
			LOCAL_PROXY_GENERATE | LOCAL_PROXY_USE | LOCAL_PROXY_REVEAL | LOCAL_PROXY_DELETE => {
				let Some(id) = self.context_entry else {
					return;
				};
				match item {
					LOCAL_PROXY_GENERATE => {
						if let Err(err) = self.engine.update(cx, |engine, cx| {
							engine.proxy_generate(id, cx)
						}) {
							println!("[project explorer] proxy generate failed: {err}");
						}
					}
					LOCAL_PROXY_USE => {
						let enabled = self
							.engine
							.read(cx)
							.proxy_row(id)
							.is_some_and(|row| row.enabled);
						self.engine.update(cx, |engine, cx| {
							engine.proxy_set_enabled(id, !enabled, cx)
						});
					}
					LOCAL_PROXY_REVEAL => {
						self.engine.read(cx).proxy_reveal(id);
					}
					LOCAL_PROXY_DELETE => {
						self.engine.update(cx, |engine, cx| engine.proxy_delete(id, cx));
					}
					_ => {}
				}
			}
			_ => {
				println!("[project explorer] unhandled local menu item {item}");
			}
		}
	}
}

/// The project bin implements no focused-panel commands: everything falls
/// through to the shell's global handler.
impl<E: AppEngine> PanelCommandHandler for ProjectExplorerPanel<E> {}

impl<E: AppEngine> Render for ProjectExplorerPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();

		// The panel title row, per the design's panel headers: the
		// widget below only shows the bare tree/icon view toggles, so
		// without this row the panel reads as anonymous. The trailing 新建
		// 序列 button mirrors the blank-area context menu's new-sequence
		// item (and the File > New > Sequence… action).
		let new_sequence_button = div()
			.id("project-new-sequence")
			.debug_selector(|| "project-new-sequence".into())
			.px_2()
			.py_0p5()
			.rounded_sm()
			.flex()
			.items_center()
			.cursor_pointer()
			.text_color(colors.text)
			.text_xs()
			.hover(|style| style.bg(colors.selected))
			.tooltip(move |window, cx| {
				tooltip_view(crate::i18n::tr("project.new_sequence").into(), window, cx)
			})
			.on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
				cx.emit(NewSequenceRequested);
			}))
			.child(crate::i18n::tr("project.new_sequence"));
		// 添加文本素材: creates a text generator (a bin entry that drops on
		// the timeline as a generator clip).
		let add_text_button = div()
			.id("project-add-text-footage")
			.debug_selector(|| "project-add-text-footage".into())
			.px_2()
			.py_0p5()
			.rounded_sm()
			.flex()
			.items_center()
			.cursor_pointer()
			.text_color(colors.text)
			.text_xs()
			.hover(|style| style.bg(colors.selected))
			.tooltip(move |window, cx| {
				tooltip_view(crate::i18n::tr("project.add_text_footage").into(), window, cx)
			})
			.on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
				cx.emit(NewTextFootageRequested);
			}))
			.child(crate::i18n::tr("project.add_text_footage"));
		let header = div()
			.flex()
			.items_center()
			.h(px(28.0))
			.flex_shrink_0()
			.px_2()
			.border_b_1()
			.border_color(colors.border)
			.bg(colors.container)
			.text_sm()
			.text_color(colors.text)
			.child(div().flex_1().child(crate::i18n::tr("panel.project")))
			.child(add_text_button)
			.child(new_sequence_button);
		div()
			.size_full()
			.flex()
			.flex_col()
			// Any click inside the panel makes it the focused panel (the
			// dock re-emits this as `DockEvent::PanelFocused`, which the
			// shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &gpui::MouseDownEvent, _window, cx| {
					cx.emit(PanelEvent::Focused);
				})
			})
			.child(header)
			.child(
				div()
					.flex_1()
					.min_h_0()
					.child(self.explorer.clone()),
			)
			// The right-click popup renders anchored above the panel.
			.child(self.context_menu.widget())
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for ProjectExplorerPanel<E> {}

impl<E: AppEngine> EventEmitter<ContextMenuTriggered> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to open the sequence properties
/// dialog for the given sequence entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequencePropertiesRequested(pub u64);

impl<E: AppEngine> EventEmitter<SequencePropertiesRequested> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to open the new-sequence dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewSequenceRequested;

impl<E: AppEngine> EventEmitter<NewSequenceRequested> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to create a text generator bin
/// entry (the header's 添加文本素材 action).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewTextFootageRequested;

impl<E: AppEngine> EventEmitter<NewTextFootageRequested> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to rename entry `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenameRequested(pub u64);

impl<E: AppEngine> EventEmitter<RenameRequested> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to delete entry `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteRequested(pub u64);

impl<E: AppEngine> EventEmitter<DeleteRequested> for ProjectExplorerPanel<E> {}

/// The project explorer asked the shell to open the 导出序列 dialog for
/// the given sequence entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportSequenceRequested(pub u64);

impl<E: AppEngine> EventEmitter<ExportSequenceRequested> for ProjectExplorerPanel<E> {}

impl<E: AppEngine> DockPanel for ProjectExplorerPanel<E> {
	fn panel_id(&self) -> gpui::dock::PanelId {
		PROJECT
	}

	fn title(&self, _cx: &App) -> SharedString {
		crate::i18n::tr("panel.project").into()
	}

	fn tab_content(&self, _cx: &App) -> AnyElement {
		div()
			.child(crate::i18n::tr("panel.project"))
			.into_any_element()
	}
}

// ---------------------------------------------------------------------------
// Context menus — the Rust counterpart of the C++
// `ProjectExplorer::show_context_menu` (`app/widget/projectexplorer/`).
// ---------------------------------------------------------------------------

/// Local (non-registry) item ids of the project explorer's context menus.
const LOCAL_OPEN_IN_NEW_TAB: usize = 2201;
const LOCAL_OPEN_IN_NEW_WINDOW: usize = 2202;
const LOCAL_REVEAL_IN_FINDER: usize = 2203;
const LOCAL_REPLACE_FOOTAGE: usize = 2204;
const LOCAL_PROXY_GENERATE: usize = 2205;
const LOCAL_PROXY_USE: usize = 2206;
const LOCAL_PROXY_REVEAL: usize = 2207;
const LOCAL_PROXY_DELETE: usize = 2208;
const LOCAL_RENAME: usize = 2209;
const LOCAL_DELETE: usize = 2210;
const LOCAL_PROPERTIES: usize = 2211;
const LOCAL_EXPORT_SEQUENCE: usize = 2212;

/// The proxy submenu (shared shape with the timeline's): enable state
/// follows the footage's proxy fields (the C++ project explorer gates
/// Generate on a video stream, Use/Reveal/Delete on the proxy path); the
/// settings entry is the real registry action. `row` is `None` when the
/// entry is not footage — every entry but the settings stays disabled.
fn proxy_submenu(row: Option<&crate::oakui::engine::ProxyFootageRow>) -> Menu {
	let can_generate = row.is_some_and(|row| row.can_generate);
	let enabled = row.is_some_and(|row| row.enabled);
	let has_proxy = row.is_some_and(|row| row.has_proxy);
	let mut generate =
		MenuItem::new(LOCAL_PROXY_GENERATE, crate::i18n::tr("timeline.context.generate_proxy"));
	if !can_generate {
		generate = generate.disabled();
	}
	let mut use_proxy = MenuItem::new(LOCAL_PROXY_USE, crate::i18n::tr("timeline.context.use_proxy"))
		.with_checked(enabled);
	if row.is_none() {
		use_proxy = use_proxy.disabled();
	}
	let mut reveal =
		MenuItem::new(LOCAL_PROXY_REVEAL, crate::i18n::tr("timeline.context.reveal_proxy"));
	if !has_proxy {
		reveal = reveal.disabled();
	}
	let mut delete =
		MenuItem::new(LOCAL_PROXY_DELETE, crate::i18n::tr("timeline.context.delete_proxy"));
	if !has_proxy {
		delete = delete.disabled();
	}
	Menu::new(vec![
		generate,
		use_proxy,
		reveal,
		delete,
		menu::action_item(ActionId::ProxySettings).separated(),
	])
}

/// The empty-area context menu: New + Import.
pub(crate) fn blank_menu() -> Menu {
	Menu::new(vec![
		MenuItem::new(0, crate::i18n::tr("project.context.new"))
			.with_submenu(Menu::new(menu::new_section())),
		menu::action_item(ActionId::Import),
	])
}

/// A footage entry's context menu: reveal + replace, the proxy submenu,
/// then rename / delete / properties. `proxy` carries the entry's proxy
/// state so the submenu enables Generate/Use/Reveal/Delete correctly.
pub(crate) fn footage_menu(
	reveal_enabled: bool,
	proxy: Option<&crate::oakui::engine::ProxyFootageRow>,
) -> Menu {
	let mut reveal =
		MenuItem::new(LOCAL_REVEAL_IN_FINDER, crate::i18n::tr("project.context.reveal_in_finder"));
	if !reveal_enabled {
		reveal = reveal.disabled();
	}
	Menu::new(vec![
		reveal,
		MenuItem::new(LOCAL_REPLACE_FOOTAGE, crate::i18n::tr("project.context.replace_footage"))
			.separated(),
		MenuItem::new(0, crate::i18n::tr("timeline.context.proxy"))
			.with_submenu(proxy_submenu(proxy)),
		MenuItem::new(LOCAL_RENAME, crate::i18n::tr("project.context.rename")).separated(),
		MenuItem::new(LOCAL_DELETE, crate::i18n::tr("project.context.delete")),
		MenuItem::new(LOCAL_PROPERTIES, crate::i18n::tr("menu.context.properties")).separated(),
	])
}

/// A non-footage entry's context menu (folder / sequence): open-in-new-tab,
/// then rename / delete / properties. Open-in-new-tab and the window
/// variant are pending the sequence tabs.
pub(crate) fn entry_menu() -> Menu {
	let open_tab = MenuItem::new(
		LOCAL_OPEN_IN_NEW_TAB,
		crate::i18n::tr("project.context.open_in_new_tab"),
	)
	.disabled();
	let open_window = MenuItem::new(
		LOCAL_OPEN_IN_NEW_WINDOW,
		crate::i18n::tr("project.context.open_in_new_window"),
	)
	.disabled();
	Menu::new(vec![
		open_tab,
		open_window.separated(),
		MenuItem::new(LOCAL_RENAME, crate::i18n::tr("project.context.rename")).separated(),
		MenuItem::new(LOCAL_DELETE, crate::i18n::tr("project.context.delete")),
		MenuItem::new(LOCAL_PROPERTIES, crate::i18n::tr("menu.context.properties")).separated(),
	])
}

/// A sequence entry's context menu: the generic entry items plus 导出序列
/// (the 文件 → 导出序列 dialog preselected to this sequence).
pub(crate) fn sequence_menu() -> Menu {
	let mut menu = entry_menu();
	menu.items.push(
		MenuItem::new(
			LOCAL_EXPORT_SEQUENCE,
			crate::i18n::tr("project.context.export_sequence"),
		)
		.separated(),
	);
	menu
}

/// Reveals `path` in the platform file manager (Finder on macOS, Explorer
/// on Windows, `xdg-open` on the parent directory elsewhere).
fn reveal_in_finder(path: &std::path::Path) {
	let result = if cfg!(target_os = "macos") {
		Command::new("open").arg("-R").arg(path).spawn()
	} else if cfg!(target_os = "windows") {
		Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
	} else {
		let dir = path.parent().unwrap_or(path);
		Command::new("xdg-open").arg(dir).spawn()
	};
	if let Err(err) = result {
		println!("[project explorer] reveal failed: {err}");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::MockEngine;
	use gpui::{px, size, TestAppContext, VisualTestContext};
	use gpui_widgets::project_explorer::ProjectDataSource;

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
		cx.update(|cx| cx.init_colors());
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
	#[gpui::test]
	async fn explorer_events_route_to_the_engine_and_open_the_menu(cx: &mut TestAppContext) {
		let (cx, panel) = mock_panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
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
				position: gpui::point(px(20.0), px(20.0)),
			});
		});
		cx.run_until_parked();
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(
			cx.debug_bounds("menu-popup").is_some(),
			"the requested context menu renders"
		);
	}

	/// The full context-menu selection: an empty-area click gets the blank
	/// menu, footage gets the footage menu (with its proxy state), a
	/// sequence gets the sequence menu and anything else the entry menu.
	#[gpui::test]
	async fn context_menu_selection_follows_the_engine(cx: &mut TestAppContext) {
		use crate::oakui::real::RealEngine;
		let _lock = stack_lock();
		oak_undo::global::clear().unwrap();
		cx.update(|cx| cx.init_colors());
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
			let seq = engine.sequence_entries().first().expect("a sequence entry").0;
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
		let entry_count = |app: &gpui::App| -> usize {
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
			names
				.into_iter()
				.filter(|name| name == "demo.mp4")
				.count()
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
					panel.open_context_menu(id, gpui::point(px(20.0), px(20.0)), cx);
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

		oak_undo::global::clear().unwrap();
	}

	/// Every local menu item path: the guard returns without a context
	/// entry, the emissions fire with one, the prompt-driven replace runs
	/// its async body (both a selection and a cancellation) and the proxy
	/// items apply to the mock's footage.
	#[gpui::test]
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
			cx.read(|app| engine.read(app).proxy_row(10).is_some_and(|row| !row.enabled)),
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
			Some(vec![std::env::temp_dir().join("oakapp_explorer_replace.mp4")])
		});
		cx.run_until_parked();
	}

	/// The panel's dock metadata and the platform reveal helper.
	#[gpui::test]
	async fn panel_metadata_and_reveal(cx: &mut TestAppContext) {
		let (cx, panel) = mock_panel_window(cx);
		let _ = cx.update(|_, cx| panel.read(cx).tab_content(cx));
		let _ = cx.update(|_, cx| panel.read(cx).title(cx));
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
		assert_eq!(
			menu.items[1].id,
			ActionId::Import.entry().menu_id()
		);
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
}

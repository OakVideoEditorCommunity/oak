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

//! The effect library panel (效果库): every effect type the engine can add
//! to a clip's chain, as a grouped list. Double-clicking an entry appends
//! the effect to the selected clip's effect chain (the undoable
//! [`AppEngine::add_effect`]; the insertion index is clamped to the chain
//! end by the backend). Every group header collapses and expands its
//! entries; the collapsed set is persisted across sessions.

use std::collections::HashSet;

use crate::oakui::component::text_input;
use crate::panels::PanelFocused;
use gpui_kit::base::input::InputState;
use gpui_kit::colors::DefaultColors;
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::input::InputEvent;
use gpui_kit::{
	App, ClickEvent, Context, Entity, EventEmitter, MouseButton, Render, SharedString, Window, div,
	prelude::*,
};
use gpui_kit::{FocusHandle, Focusable};
use quercus_engine_rs::state::Config;

use crate::i18n;
use crate::oakui::AppEngine;
use crate::oakui::effectchain::group_label;
use crate::oakui::engine::EffectEntry;
use crate::panels::commands::PanelCommandHandler;

/// The config key the collapsed groups are persisted under (a comma
/// separated list of group keys — the keys [`EffectEntry::group`] holds,
/// not the display labels).
const COLLAPSED_CONFIG_KEY: &str = "EffectLibraryCollapsed";

/// The effect library panel.
pub struct EffectLibraryPanel<E: AppEngine> {
	/// The panel's focus handle (the dock's zoom/focus and the
	/// shell's focused-panel routing need it).
	focus_handle: FocusHandle,
	engine: Entity<E>,
	/// The search box state: live-filters the list by name / type id
	/// (case-insensitive substring).
	search: Entity<InputState>,
	/// The group keys whose entries are collapsed away. The headers stay
	/// rendered (and clickable) so a collapsed group can be reopened.
	collapsed: HashSet<String>,
}

impl<E: AppEngine> EffectLibraryPanel<E> {
	/// Test access to the search box state (the shell-level shortcut
	/// conflict regression types into it).
	#[cfg(test)]
	pub(crate) fn test_search(&self) -> Entity<InputState> {
		self.search.clone()
	}

	/// Builds the panel over `engine`'s addable-effect table.
	///
	/// `engine` is the shared engine entity (borrowed, not owned); its
	/// `addable_effects` table drives the list and its selection hint decides
	/// which row is highlighted, so the panel re-renders on every engine
	/// notify.
	/// `window` is needed to create the search box input, and `cx` supplies
	/// the panel's focus handle. The collapsed-group set is read from the
	/// config once here (see `COLLAPSED_CONFIG_KEY`), not on every render.
	/// The engine and search subscriptions are detached, so they live as long
	/// as the entity.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		// Re-read the effect table whenever the engine notifies (the table
		// itself is static, but the selection hint depends on the target).
		cx.observe(&engine, |_this, _engine, cx| cx.notify())
			.detach();
		let search = cx.new(|cx| InputState::new(window, cx));
		cx.subscribe(&search, |_this, _state, event: &InputEvent, cx| {
			if matches!(event, InputEvent::Change) {
				cx.notify();
			}
		})
		.detach();
		Self {
			focus_handle: cx.focus_handle(),
			engine,
			search,
			collapsed: load_collapsed(),
		}
	}
}

/// Reads the persisted collapsed groups (a missing or malformed value
/// simply starts with every group expanded).
fn load_collapsed() -> HashSet<String> {
	parse_collapsed(
		&Config::global()
			.get(COLLAPSED_CONFIG_KEY)
			.unwrap_or_default(),
	)
}

/// Parses the persisted comma separated key list; blanks and padding
/// drop, so a hand-edited value cannot produce a phantom group.
fn parse_collapsed(raw: &str) -> HashSet<String> {
	raw.split(',')
		.map(str::trim)
		.filter(|key| !key.is_empty())
		.map(str::to_string)
		.collect()
}

/// Serializes the collapsed set for the config: sorted and comma
/// separated, so an unchanged set persists an unchanged string.
fn format_collapsed(collapsed: &HashSet<String>) -> String {
	let mut keys: Vec<&str> = collapsed.iter().map(String::as_str).collect();
	keys.sort_unstable();
	keys.join(",")
}

/// Writes the collapsed set back to the config (the `UseProxyMedia`
/// pattern: the config store is the source of truth, read once at panel
/// construction).
fn store_collapsed(collapsed: &HashSet<String>) {
	let _ = Config::global().set(COLLAPSED_CONFIG_KEY, &format_collapsed(collapsed));
}

/// Flips the collapsed state of `group_key`, returning the new state
/// (`true` = collapsed).
fn toggle_collapsed(collapsed: &mut HashSet<String>, group_key: &str) -> bool {
	if collapsed.remove(group_key) {
		false
	} else {
		collapsed.insert(group_key.to_string());
		true
	}
}

/// Whether `entry` matches the trimmed, lowercased search query.
fn matches_query(entry: &EffectEntry, query: &str) -> bool {
	query.is_empty()
		|| entry.name.to_lowercase().contains(query)
		|| entry.type_id.to_lowercase().contains(query)
}

/// Whether `entry`'s row is collapsed away. The collapse only narrows the
/// unfiltered list: a search looks through every group, collapsed or not.
fn hidden_by_collapse(entry: &EffectEntry, query: &str, collapsed: &HashSet<String>) -> bool {
	query.is_empty()
		&& entry
			.group
			.as_deref()
			.is_some_and(|group| collapsed.contains(group))
}

/// The effect library implements no focused-panel commands: everything
/// falls through to the shell's global handler.
impl<E: AppEngine> PanelCommandHandler for EffectLibraryPanel<E> {}

impl<E: AppEngine> Render for EffectLibraryPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let effects = self.engine.read(cx).addable_effects();
		let query = self.search.read(cx).value().trim().to_lowercase();

		let mut list = div()
			.id("effect-library-list")
			.flex_1()
			.min_h_0()
			.flex()
			.flex_col()
			.gap_1()
			.p_2()
			.overflow_y_scroll();

		// Built-in effects group by their category (color / filter /
		// distort / keying / generator / math / general, see
		// `effectchain::category_group_key`); OpenFX plugin entries group
		// by their sub-category (Filter / Generator / Transition /
		// General — the C++ `factorymenu` OpenFX branch). The engine table
		// arrives sorted (built-ins first, then groups and names
		// alphabetically); the search box live-filters by name / type id
		// and always searches collapsed groups too.
		let mut last_group: Option<Option<String>> = None;
		for entry in &effects {
			if !matches_query(entry, &query) {
				continue;
			}
			let group_key = entry.group.clone();
			if last_group.as_ref() != Some(&group_key) {
				last_group = Some(group_key.clone());
				let key = group_key.clone().unwrap_or_default();
				let label = match &entry.group {
					Some(group) => group_label(group),
					None => i18n::tr("effect_library.group.builtin").to_string(),
				};
				let collapsed = self.collapsed.contains(&key);
				list = list.child(group_header(
					&colors,
					&key,
					&label,
					collapsed,
					cx.listener({
						let key = key.clone();
						move |this, _event: &ClickEvent, _window, cx| {
							toggle_collapsed(&mut this.collapsed, &key);
							store_collapsed(&this.collapsed);
							cx.notify();
						}
					}),
				));
			}
			if hidden_by_collapse(entry, &query, &self.collapsed) {
				continue;
			}
			let engine = self.engine.clone();
			let row_id = entry.type_id.clone();
			let name = entry.name.clone();
			let type_id = entry.type_id.clone();
			let drag_payload = crate::widgets::effect_stack::LibraryEffectDrag {
				type_id: SharedString::from(entry.type_id.clone()),
				name: SharedString::from(entry.name.clone()),
			};
			list = list.child(
				div()
					.id(SharedString::from(format!("effect-library-{type_id}")))
					.debug_selector(move || format!("effect-library-row-{row_id}"))
					.cursor_pointer()
					.px_2()
					.py_1()
					.rounded_sm()
					.text_sm()
					.text_color(colors.text)
					.hover(|style| style.bg(colors.selected))
					.child(name)
					// Drag the effect onto the inspector's effect stack (or
					// the node editor) to add it there; double-click adds it
					// to the selected clip's chain end.
					.on_drag(drag_payload, |payload, _origin, _window, cx| {
						cx.new(|_cx| EffectDragGhost {
							name: payload.name.clone(),
						})
					})
					.on_click(move |event: &ClickEvent, _window, cx| {
						// Double-click appends the effect to the selected
						// clip's chain; the backend clamps the index to the
						// chain end.
						if event.click_count() == 2 {
							let type_id = type_id.clone();
							engine.update(cx, |engine, cx| {
								if let Err(err) = engine.add_effect(usize::MAX, &type_id, cx) {
									println!("[effect library] add effect failed: {err}");
								}
							});
						}
					}),
			);
		}

		div()
			.size_full()
			.flex()
			.flex_col()
			// Any click inside the panel makes it the focused panel (the
			// dock re-emits this as `DockEvent::PanelFocused`, which the
			// shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &gpui_kit::MouseDownEvent, _window, cx| {
					cx.emit(PanelFocused);
				})
			})
			.child(
				div()
					.flex_shrink_0()
					.p_2()
					.border_b_1()
					.border_color(colors.border)
					.child(
						text_input("effect-library-search", cx)
							.state(self.search.downgrade())
							.accepts_input(true),
					),
			)
			.child(list)
			.child(
				div()
					.flex_shrink_0()
					.px_2()
					.py_1()
					.border_t_1()
					.border_color(colors.border)
					.text_xs()
					.text_color(colors.disabled)
					.child(i18n::tr("effect_library.hint")),
			)
	}
}

/// The header row of a group: a muted semibold line with a ▶ / ▼ marker
/// that collapses and expands the group's entries. `group_key` is the raw
/// key ([`EffectEntry::group`], also the element id suffix); `label` is
/// its display string.
fn group_header(
	colors: &gpui_kit::colors::Colors,
	group_key: &str,
	label: &str,
	collapsed: bool,
	on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
	div()
		.id(SharedString::from(format!(
			"effect-library-group-{group_key}"
		)))
		.debug_selector({
			let key = group_key.to_string();
			move || format!("effect-library-group-row-{key}")
		})
		.cursor_pointer()
		.pt_2()
		.pb_1()
		.px_2()
		.flex()
		.items_center()
		.gap_1()
		.text_xs()
		.font_weight(gpui_kit::FontWeight(600.0))
		.text_color(colors.disabled)
		.hover(|style| style.text_color(colors.text))
		.child(crate::widgets::glyphs::icon(
			if collapsed {
				crate::widgets::glyphs::Glyph::ChevronRight
			} else {
				crate::widgets::glyphs::Glyph::ChevronDown
			},
			colors.disabled,
		))
		.child(label.to_string())
		.on_click(on_toggle)
}

/// The drag ghost shown under the pointer while an effect is dragged out
/// of the library (a small floating label with the effect name).
struct EffectDragGhost {
	name: SharedString,
}

impl Render for EffectDragGhost {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.px_2()
			.py_1()
			.rounded_sm()
			.border_1()
			.border_color(colors.border)
			.bg(colors.container)
			.text_sm()
			.text_color(colors.text)
			.child(self.name.clone())
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for EffectLibraryPanel<E> {}

impl<E: AppEngine> EventEmitter<PanelFocused> for EffectLibraryPanel<E> {}

impl<E: AppEngine> Focusable for EffectLibraryPanel<E> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<E: AppEngine> BasePanel for EffectLibraryPanel<E> {
	fn panel_name(&self) -> &'static str {
		crate::panels::ids::EFFECT_LIBRARY
	}
}

impl<E: AppEngine> Panel for EffectLibraryPanel<E> {
	fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		i18n::tr("panel.effect_library")
	}
}

#[cfg(test)]
mod tests;

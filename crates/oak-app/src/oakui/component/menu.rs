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

//! The app's menu component: the menu bar and the context menus.
//!
//! All app menu code lives here instead of reaching into gpui_widgets
//! directly: the rendering-engine types are re-exported
//! ([`Menu`], [`MenuItem`], [`MenuBar`], …), the shared context-menu
//! plumbing ([`ContextMenuHandle`], [`ContextMenuTriggered`]) and the
//! shared menu segments (edit / clip-edit / in-out / color label / new,
//! plus the viewer context menu) are built from the action registry
//! ([`crate::actions`]) exactly like the menu bar, so ids, labels and
//! shortcut annotations can never diverge.
//!
//! Local (non-registry) items — the color labels and the dynamic
//! language items — live at [`LOCAL_ID_BASE`] and above;
//! [`crate::actions::entry_for_menu_id`] splits the two worlds at
//! dispatch time.

pub use crate::widgets::menu::{
	ContextMenu, ContextMenuEvent, Menu, MenuBar, MenuBarEntry, MenuBarEvent, MenuItem,
};
use crate::widgets::viewer::{SafeMargins, VIEWER_ZOOM_LEVELS, ViewerZoom, WaveformMode};

// ---------------------------------------------------------------------------
// Context-menu plumbing
// ---------------------------------------------------------------------------

use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Pixels, Point, Window};
/// A registry-backed context-menu item was triggered: the panel re-emits it
/// so the app shell dispatches it like a menu-bar click (after pointing
/// `focused_panel` at the panel, since a right-click does not emit
/// `PanelEvent::Focused`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMenuTriggered {
	/// The triggered item's id (an action registry menu id).
	pub item: usize,
}

/// A panel's context menu: owns the popup entity, re-emits registry items as
/// [`ContextMenuTriggered`] and hands local items to the panel.
pub struct ContextMenuHandle {
	menu: Entity<ContextMenu>,
}

impl ContextMenuHandle {
	/// Create the popup entity and subscribe to it. `on_local_item` handles
	/// every triggered item that is not in the action registry (color
	/// labels, panel-specific placeholders, …).
	pub fn new<P, F>(on_local_item: F, window: &mut Window, cx: &mut Context<P>) -> Self
	where
		P: EventEmitter<ContextMenuTriggered>,
		F: Fn(&mut P, usize, &mut Context<P>) + 'static,
	{
		let menu = cx.new(|cx| ContextMenu::new(0, window, cx));
		cx.subscribe(
			&menu,
			move |panel: &mut P, _menu, event: &ContextMenuEvent, cx| {
				if crate::actions::entry_for_menu_id(event.item).is_some() {
					cx.emit(ContextMenuTriggered { item: event.item });
				} else {
					on_local_item(panel, event.item, cx);
				}
			},
		)
		.detach();
		Self { menu }
	}

	/// Opens the panel's context menu at `position`.
	///
	/// `position` is the popup's top-left anchor in window coordinates (usually
	/// the right-click location) and `menu` is the fully built menu model, moved
	/// into the popup. Forwards to [`ContextMenu::show`], which replaces any menu
	/// already open and requests a repaint. The popup is only visible when
	/// `self.widget()` is rendered as a child of the panel, so it paints above
	/// the panel content.
	///
	/// Activation of an item is routed by [`ContextMenuHandle::new`]: registry
	/// items are re-emitted as [`ContextMenuTriggered`], everything else goes to
	/// the panel's `on_local_item` callback.
	pub fn show(&self, position: Point<Pixels>, menu: Menu, cx: &mut App) {
		self.menu
			.update(cx, |menu_view, cx| menu_view.show(position, menu, cx));
	}

	/// The popup entity, to be rendered as a child of the panel so the
	/// anchored popup can paint above it.
	pub fn widget(&self) -> Entity<ContextMenu> {
		self.menu.clone()
	}
}

// ---------------------------------------------------------------------------
// Shared menu segments
// ---------------------------------------------------------------------------

use crate::actions::ActionId;

/// The first id reserved for local (non-registry) context-menu items. Every
/// registry action's menu id sits below this; the panels' panel-specific
/// items and the color labels sit at or above it.
pub const LOCAL_ID_BASE: usize = 2001;

/// The first of the 16 color-label items:
/// `COLOR_LABEL_BASE..COLOR_LABEL_BASE + COLOR_LABEL_COUNT`.
pub const COLOR_LABEL_BASE: usize = LOCAL_ID_BASE;

/// The number of standard color labels (the C++ `ColorCoding` enum: red …
/// gray).
pub const COLOR_LABEL_COUNT: usize = 16;

/// The first id reserved for the dynamic language items: one per
/// discovered language pack, `LANG_ITEM_BASE + index` into
/// [`crate::i18n::available_languages`]. Language switching is
/// data-driven (a new pack is a new YAML file, not a new registry
/// action), so these ids live above the registry range like the color
/// labels.
pub const LANG_ITEM_BASE: usize = COLOR_LABEL_BASE + COLOR_LABEL_COUNT;

/// The reserved language-item id range (far more than any plausible
/// pack count).
pub const LANG_ITEM_COUNT: usize = 32;

/// Maps a menu item id in the language range back to its index into
/// [`crate::i18n::available_languages`].
pub fn language_item_index(item: usize) -> Option<usize> {
	(LANG_ITEM_BASE..LANG_ITEM_BASE + LANG_ITEM_COUNT)
		.contains(&item)
		.then(|| item - LANG_ITEM_BASE)
}

/// Builds the language submenu: one item per discovered pack, labelled
/// with the pack's own `language.name` endonym plus its code, the active
/// pack checkmarked.
pub fn language_menu() -> MenuItem {
	let current = crate::i18n::language_code();
	let items = crate::i18n::available_languages()
		.iter()
		.enumerate()
		.map(|(index, code)| {
			let label = format!("{} ({code})", crate::i18n::pack_native_name(code));
			MenuItem::new(LANG_ITEM_BASE + index, label).with_checked(*code == current)
		})
		.collect();
	MenuItem::new(0, crate::i18n::tr("menu.view.language")).with_submenu(Menu::new(items))
}

/// One menu item straight from the registry: id and label come from the
/// entry, the shortcut annotation from
/// [`display_shortcut`](crate::actions::display_shortcut) — the same recipe
/// the menu bar uses.
pub fn action_item(action: ActionId) -> MenuItem {
	let entry = action.entry();
	let mut item = MenuItem::new(entry.menu_id(), crate::i18n::tr(entry.i18n_key));
	if let Some(shortcut) = crate::actions::display_shortcut(action) {
		item = item.with_shortcut(shortcut);
	}
	item
}

/// The shared "edit" segment (`add_items_for_edit_menu`): undo/redo, the
/// clipboard group and delete; with `for_clips` the clip-only tail follows
/// (ripple delete, split, speed/duration, then the clip-edit group).
pub fn edit_section(for_clips: bool) -> Vec<MenuItem> {
	use ActionId as A;
	let mut items = vec![
		action_item(A::Undo),
		action_item(A::Redo).separated(),
		action_item(A::Cut),
		action_item(A::Copy),
		action_item(A::Paste),
		action_item(A::PasteInsert),
		action_item(A::Duplicate),
		action_item(A::Rename),
		action_item(A::Delete),
	];
	if for_clips {
		items.push(action_item(A::RippleDelete));
		items.push(action_item(A::SplitAtPlayhead));
		items.push(action_item(A::SpeedDuration).separated());
		items.extend(clip_edit_section());
	}
	items
}

/// The shared "clip edit" segment (`add_items_for_clip_edit_menu`): default
/// transition, link/unlink, enable/disable, nest.
pub fn clip_edit_section() -> Vec<MenuItem> {
	use ActionId as A;
	vec![
		action_item(A::DefaultTransition),
		action_item(A::LinkUnlink),
		action_item(A::EnableDisable),
		action_item(A::Nest),
	]
}

/// The shared "in/out" segment (`add_items_for_in_out_menu`).
pub fn in_out_section() -> Vec<MenuItem> {
	use ActionId as A;
	vec![
		action_item(A::SetInPoint),
		action_item(A::SetOutPoint).separated(),
		action_item(A::ResetIn),
		action_item(A::ResetOut),
		action_item(A::ClearInOut),
	]
}

/// The shared "new" segment (`add_items_for_new_menu`).
pub fn new_section() -> Vec<MenuItem> {
	use ActionId as A;
	vec![
		action_item(A::NewProject).separated(),
		action_item(A::NewSequence),
		action_item(A::NewFolder),
	]
}

/// The 16 color labels as a "Color" submenu (the C++ `ColorLabelMenu`),
/// with `selected` checked when it is the item's index
/// (`0..COLOR_LABEL_COUNT`).
pub fn color_label_menu(selected: Option<usize>) -> Menu {
	const KEYS: [&str; COLOR_LABEL_COUNT] = [
		"menu.color.red",
		"menu.color.maroon",
		"menu.color.orange",
		"menu.color.brown",
		"menu.color.yellow",
		"menu.color.oak",
		"menu.color.lime",
		"menu.color.green",
		"menu.color.cyan",
		"menu.color.teal",
		"menu.color.blue",
		"menu.color.navy",
		"menu.color.pink",
		"menu.color.purple",
		"menu.color.silver",
		"menu.color.gray",
	];
	let items = KEYS
		.iter()
		.enumerate()
		.map(|(index, key)| {
			let mut item = MenuItem::new(COLOR_LABEL_BASE + index, crate::i18n::tr(key));
			if selected == Some(index) {
				item = item.with_checked(true);
			}
			item
		})
		.collect();
	Menu::new(items)
}

/// The "Color" submenu header item (label localized, submenu attached).
pub fn color_label_item(selected: Option<usize>) -> MenuItem {
	MenuItem::new(0, crate::i18n::tr("menu.color.label")).with_submenu(color_label_menu(selected))
}

/// The color index (`0..COLOR_LABEL_COUNT`) behind a triggered menu item id,
/// when `item` is one of the color-label items.
pub fn color_label_index(item: usize) -> Option<usize> {
	(COLOR_LABEL_BASE..COLOR_LABEL_BASE + COLOR_LABEL_COUNT)
		.contains(&item)
		.then(|| item - COLOR_LABEL_BASE)
}

// ---------------------------------------------------------------------------
// Viewer context menu (shared by the source and program monitors)
// ---------------------------------------------------------------------------

/// Local (non-registry) item ids of the viewer context menu.
pub const LOCAL_VIEWER_ZOOM_FIT: usize = 2301;
/// The zoom-level items occupy `LOCAL_VIEWER_ZOOM_LEVELS_BASE + i`, aligned
/// with [`VIEWER_ZOOM_LEVELS`].
pub const LOCAL_VIEWER_ZOOM_LEVELS_BASE: usize = 2302;
pub const LOCAL_VIEWER_FULL_SCREEN: usize = 2320;
pub const LOCAL_VIEWER_RES_FULL: usize = 2321;
pub const LOCAL_VIEWER_RES_HALF: usize = 2322;
pub const LOCAL_VIEWER_RES_QUARTER: usize = 2323;
pub const LOCAL_VIEWER_RES_EIGHTH: usize = 2324;
pub const LOCAL_VIEWER_SAFE_OFF: usize = 2325;
pub const LOCAL_VIEWER_SAFE_ON: usize = 2326;
pub const LOCAL_VIEWER_SAFE_CUSTOM: usize = 2327;
pub const LOCAL_VIEWER_STOP_ON_LAST: usize = 2328;
pub const LOCAL_VIEWER_WF_AUTOMATIC: usize = 2329;
pub const LOCAL_VIEWER_WF_ONLY: usize = 2330;
pub const LOCAL_VIEWER_WF_BOTH: usize = 2331;
pub const LOCAL_VIEWER_SHOW_FPS: usize = 2332;
pub const LOCAL_VIEWER_SAVE_FRAME: usize = 2333;

/// The zoom submenu item id for `level` (one of
/// [`crate::widgets::viewer::VIEWER_ZOOM_LEVELS`]).
pub fn viewer_zoom_level_id(index: usize) -> usize {
	LOCAL_VIEWER_ZOOM_LEVELS_BASE + index
}

/// The live state the viewer context menu reflects. The panels build one per
/// right-click from the engine config and the viewer widget, so every checked
/// entry is real: the radio groups mark the current zoom / resolution / safe
/// margins / waveform, and the toggles mark `StopOnLastFrame` and `ShowFPS`.
pub struct ViewerMenuState {
	/// The current playback resolution divider (1/2/4/8).
	pub playback_divider: i64,
	/// The viewer widget's zoom state.
	pub zoom: ViewerZoom,
	/// The viewer widget's safe-margin overlay.
	pub safe: SafeMargins,
	/// The `StopOnLastFrame` config.
	pub stop_on_last: bool,
	/// The `ViewerWaveformMode` config.
	pub waveform: WaveformMode,
	/// Whether the frame-rate overlay is shown.
	pub show_fps: bool,
}

impl Default for ViewerMenuState {
	fn default() -> Self {
		Self {
			playback_divider: 1,
			zoom: ViewerZoom::Fit,
			safe: SafeMargins::Off,
			stop_on_last: false,
			waveform: WaveformMode::Automatic,
			show_fps: false,
		}
	}
}

/// The context menu both viewer monitors show (the C++
/// `ViewerWidget::show_context_menu`, minus the OCIO color menus and the
/// subtitle block the engine does not surface yet).
pub fn viewer_menu(state: &ViewerMenuState) -> Menu {
	use crate::i18n::tr;
	// Zoom: Fit + one entry per zoom level, checked against the live state.
	let mut zoom_items = vec![
		MenuItem::new(LOCAL_VIEWER_ZOOM_FIT, tr("viewer.context.zoom_fit"))
			.with_checked(state.zoom == ViewerZoom::Fit),
	];
	for (index, level) in VIEWER_ZOOM_LEVELS.iter().enumerate() {
		zoom_items.push(
			MenuItem::new(
				viewer_zoom_level_id(index),
				format!("{:.0}%", level * 100.0),
			)
			.with_checked(state.zoom == ViewerZoom::Level(index)),
		);
	}
	// Playback Resolution radio group (the C++ `PlaybackDivider` config):
	// the checked entry reflects the current divider.
	let resolution_menu = Menu::new(vec![
		MenuItem::new(LOCAL_VIEWER_RES_FULL, tr("viewer.context.res_full"))
			.with_checked(state.playback_divider <= 1),
		MenuItem::new(LOCAL_VIEWER_RES_HALF, tr("viewer.context.res_half"))
			.with_checked(state.playback_divider == 2),
		MenuItem::new(LOCAL_VIEWER_RES_QUARTER, tr("viewer.context.res_quarter"))
			.with_checked(state.playback_divider == 4),
		MenuItem::new(LOCAL_VIEWER_RES_EIGHTH, tr("viewer.context.res_eighth"))
			.with_checked(state.playback_divider >= 8),
	]);
	// Safe margins radio group.
	let safe_menu = Menu::new(vec![
		MenuItem::new(LOCAL_VIEWER_SAFE_OFF, tr("viewer.context.safe_off"))
			.with_checked(state.safe == SafeMargins::Off),
		MenuItem::new(LOCAL_VIEWER_SAFE_ON, tr("viewer.context.safe_on"))
			.with_checked(state.safe == SafeMargins::On),
		MenuItem::new(LOCAL_VIEWER_SAFE_CUSTOM, tr("viewer.context.safe_custom"))
			.with_checked(matches!(state.safe, SafeMargins::Custom(_, _))),
	]);
	// Audio waveform radio group.
	let waveform_menu = Menu::new(vec![
		MenuItem::new(LOCAL_VIEWER_WF_AUTOMATIC, tr("viewer.context.wf_automatic"))
			.with_checked(state.waveform == WaveformMode::Automatic),
		MenuItem::new(LOCAL_VIEWER_WF_ONLY, tr("viewer.context.wf_only"))
			.with_checked(state.waveform == WaveformMode::Only),
		MenuItem::new(LOCAL_VIEWER_WF_BOTH, tr("viewer.context.wf_both"))
			.with_checked(state.waveform == WaveformMode::Both),
	]);

	Menu::new(vec![
		MenuItem::new(0, tr("viewer.context.zoom")).with_submenu(Menu::new(zoom_items)),
		MenuItem::new(LOCAL_VIEWER_FULL_SCREEN, tr("viewer.context.full_screen")),
		MenuItem::new(0, tr("viewer.context.playback_resolution")).with_submenu(resolution_menu),
		MenuItem::new(0, tr("viewer.context.safe_margins"))
			.with_submenu(safe_menu)
			.separated(),
		MenuItem::new(LOCAL_VIEWER_STOP_ON_LAST, tr("viewer.context.stop_on_last"))
			.with_checked(state.stop_on_last)
			.separated(),
		MenuItem::new(0, tr("viewer.context.audio_waveform")).with_submenu(waveform_menu),
		MenuItem::new(LOCAL_VIEWER_SHOW_FPS, tr("viewer.context.show_fps"))
			.with_checked(state.show_fps),
		MenuItem::new(LOCAL_VIEWER_SAVE_FRAME, tr("viewer.context.save_frame")).separated(),
	])
}

/// A viewer context-menu item the panels apply directly on the viewer widget
/// or the engine (unlike the registry items, which re-emit as
/// [`ContextMenuTriggered`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerMenuAction {
	/// Zoom out to fit the whole frame.
	ZoomFit,
	/// Zoom to `index` of [`VIEWER_ZOOM_LEVELS`].
	ZoomLevel(usize),
	/// Request full-screen mode.
	FullScreen,
	/// Set the playback resolution divider (1/2/4/8).
	Resolution(i64),
	/// Turn the safe-margin overlay off.
	SafeOff,
	/// Show the standard safe margins.
	SafeOn,
	/// Show custom safe margins (the app uses the standard 0.9 × 0.8).
	SafeCustom,
	/// Toggle the `StopOnLastFrame` config.
	StopOnLast,
	/// Set the audio-waveform overlay mode.
	Waveform(WaveformMode),
	/// Toggle the frame-rate overlay.
	ShowFps,
	/// Save the current frame to a PNG.
	SaveFrame,
}

/// Resolve a triggered viewer context-menu item id into the action the
/// panels apply, `None` when the id belongs to another menu.
pub fn viewer_menu_action(item: usize) -> Option<ViewerMenuAction> {
	if item == LOCAL_VIEWER_ZOOM_FIT {
		Some(ViewerMenuAction::ZoomFit)
	} else if item >= LOCAL_VIEWER_ZOOM_LEVELS_BASE
		&& item < LOCAL_VIEWER_ZOOM_LEVELS_BASE + VIEWER_ZOOM_LEVELS.len()
	{
		Some(ViewerMenuAction::ZoomLevel(
			item - LOCAL_VIEWER_ZOOM_LEVELS_BASE,
		))
	} else {
		match item {
			LOCAL_VIEWER_FULL_SCREEN => Some(ViewerMenuAction::FullScreen),
			LOCAL_VIEWER_RES_FULL => Some(ViewerMenuAction::Resolution(1)),
			LOCAL_VIEWER_RES_HALF => Some(ViewerMenuAction::Resolution(2)),
			LOCAL_VIEWER_RES_QUARTER => Some(ViewerMenuAction::Resolution(4)),
			LOCAL_VIEWER_RES_EIGHTH => Some(ViewerMenuAction::Resolution(8)),
			LOCAL_VIEWER_SAFE_OFF => Some(ViewerMenuAction::SafeOff),
			LOCAL_VIEWER_SAFE_ON => Some(ViewerMenuAction::SafeOn),
			LOCAL_VIEWER_SAFE_CUSTOM => Some(ViewerMenuAction::SafeCustom),
			LOCAL_VIEWER_STOP_ON_LAST => Some(ViewerMenuAction::StopOnLast),
			LOCAL_VIEWER_WF_AUTOMATIC => Some(ViewerMenuAction::Waveform(WaveformMode::Automatic)),
			LOCAL_VIEWER_WF_ONLY => Some(ViewerMenuAction::Waveform(WaveformMode::Only)),
			LOCAL_VIEWER_WF_BOTH => Some(ViewerMenuAction::Waveform(WaveformMode::Both)),
			LOCAL_VIEWER_SHOW_FPS => Some(ViewerMenuAction::ShowFps),
			LOCAL_VIEWER_SAVE_FRAME => Some(ViewerMenuAction::SaveFrame),
			_ => None,
		}
	}
}

/// A viewer panel request the app shell applies: full-screen goes to the
/// window, the in/out/clear requests go to the program workarea (both
/// monitors share one workarea, so the source monitor's requests act on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerPanelEvent {
	/// Toggle the window's full-screen state.
	FullScreenRequested,
	/// Set the program workarea's in point at the playhead.
	SetInPoint,
	/// Set the program workarea's out point at the playhead.
	SetOutPoint,
	/// Clear the program workarea's in/out range.
	ClearRange,
}

#[cfg(test)]
mod tests;

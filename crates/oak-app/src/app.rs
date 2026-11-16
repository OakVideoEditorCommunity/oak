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

//! The application shell: menu bar, dock layout, status bar, modal dialogs
//! and the tick loop that drives playback, playhead sync, the audio meter
//! and the export progress.
//!
//! The shell is generic over the engine backend ([`AppEngine`]); [`run`]
//! picks the backend at startup: the real engine by default, the mock when
//! the `--mock` flag / `OAK_ENGINE=mock` env var is given or the
//! `mock-engine` cargo feature is enabled.
//!
//! Layout per the design (`design/Oak-UI设计图-主界面-标注版.png`):
//!
//! ```text
//! ┌ menu bar (文件 编辑 视图 回放 序列 窗口 工具 帮助)
//! ├─────────────────────────────────────────────────────
//! │ dock: 项目 | 素材查看器 | 序列查看器+节点编辑器 | 检查器+历史记录
//! │       (vertical split) 时间线 (full width, 31px toolbar on top)
//! ├─────────────────────────────────────────────────────
//! └ status bar: 就绪 | 缓存 | 代理 | 库写入状态 || 时间码/时长 | 帧率 | 分辨率 | 引擎
//! ```

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::oakui::component::menu::{self, Menu, MenuBar, MenuBarEntry, MenuBarEvent, MenuItem};
use crate::oakui::component::text_input::install_text_input_bindings;
use crate::oakui::settings::use_proxy_media;
use crate::panels::PanelId;
use crate::widgets::audio_meter::{AudioLevelMeter, MeterOrientation};
use crate::widgets::dialog::progress::{ProgressContent, progress_dialog};
use crate::widgets::dialog::{DialogButton, Modal, ModalEvent, ModalOptions};
use crate::widgets::theme::{OakTheme, apply_theme};
use crate::widgets::timeline::{
	ClipData, ClipId, Frame, FrameRange, TimelineEvent, TimelineView, TrackData,
};
use crate::widgets::viewer::PlaybackClock;
use gpui_kit::component::dock::{
	DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin, DragPanel,
	PanelId as DockPanelId, PanelStyle, panel_handle, register_panel,
};
use gpui_kit::{
	AnyView, App, AsyncWindowContext, Bounds, Context, Entity, PathPromptOptions, Render,
	SharedString, Subscription, TitlebarOptions, Window, WindowBounds, WindowId, WindowOptions,
	colors::DefaultColors, div, prelude::*, px, size,
};

use crate::actions::{ActionId, TimelineToolExt, Tool};
use crate::dialogs::{
	DropSequenceChoice, ExportDialogContent, PreferencesDialogContent, SequenceFormatSeed,
};
use crate::oakui::{AppEngine, ExportSession, MockEngine, Monitor, RealEngine};
use crate::panels::ai_assistant::AiAssistantPanel;
use crate::panels::commands as panel_commands;
use crate::panels::effect_library::EffectLibraryPanel;
use crate::panels::history::HistoryPanel;
use crate::panels::ids::*;
use crate::panels::inspector::InspectorPanel;
use crate::panels::multicam::MulticamPanel;
use crate::panels::node_editor::NodeEditorPanel;
use crate::panels::program_viewer::ProgramViewerPanel;
use crate::panels::project_explorer::ProjectExplorerPanel;
use crate::panels::source_viewer::SourceViewerPanel;
use crate::panels::status_bar::StatusBar;
use crate::panels::timeline::{FootageDropNeedsSequence, TimelinePanel};

// Menu item ids: thin aliases over the action registry's menu ids (the
// registry is the single source; these keep the test call sites readable).
#[cfg(test)]
pub(crate) mod menu_ids {
	// Test-only menu-id aliases (the registry is the single source of
	// truth); not every alias is referenced by the suite.
	#![allow(dead_code)]

	use crate::actions::ActionId;

	pub const NEW_PROJECT: usize = ActionId::NewProject.menu_id();
	pub const OPEN_PROJECT: usize = ActionId::OpenProject.menu_id();
	pub const EXPORT_PROJECT: usize = ActionId::SaveProject.menu_id();
	pub const CLOSE: usize = ActionId::CloseProject.menu_id();
	pub const EXPORT: usize = ActionId::Export.menu_id();
	pub const QUIT: usize = ActionId::Exit.menu_id();
	pub const IMPORT_FOOTAGE: usize = ActionId::Import.menu_id();
	pub const PROJECT_MANAGER: usize = ActionId::ProjectManager.menu_id();
	pub const OPEN_FROM_LIBRARY: usize = ActionId::OpenFromLibrary.menu_id();

	pub const UNDO: usize = ActionId::Undo.menu_id();
	pub const REDO: usize = ActionId::Redo.menu_id();
	pub const DELETE: usize = ActionId::Delete.menu_id();
	pub const RIPPLE_DELETE: usize = ActionId::RippleDelete.menu_id();

	pub const THEME_DARK: usize = ActionId::ThemeDark.menu_id();
	pub const PREFERENCES: usize = ActionId::Preferences.menu_id();

	pub const FOCUS_PROJECT: usize = ActionId::FocusProject.menu_id();
	pub const FOCUS_SOURCE_VIEWER: usize = ActionId::FocusSourceViewer.menu_id();
	pub const FOCUS_PROGRAM_VIEWER: usize = ActionId::FocusProgramViewer.menu_id();
	pub const FOCUS_NODE_EDITOR: usize = ActionId::FocusNodeEditor.menu_id();
	pub const FOCUS_INSPECTOR: usize = ActionId::FocusInspector.menu_id();
	pub const FOCUS_HISTORY: usize = ActionId::FocusHistory.menu_id();
	pub const FOCUS_TIMELINE: usize = ActionId::FocusTimeline.menu_id();
	pub const FOCUS_EFFECT_LIBRARY: usize = ActionId::FocusEffectLibrary.menu_id();
	pub const FOCUS_MULTICAM: usize = ActionId::FocusMulticam.menu_id();
	pub const FOCUS_AI_ASSISTANT: usize = ActionId::FocusAiAssistant.menu_id();
	pub const RESET_DEFAULT_LAYOUT: usize = ActionId::ResetDefaultLayout.menu_id();
}

/// Modal-dialog control ids (see [`ModalEvent::control`]).
mod shell;
pub(crate) use shell::{
	AudioEdit, CONFIG_KEY_DOCK_LAYOUT, CONFIG_KEY_TIMELINE_HEIGHT, CONFIG_KEY_TIMELINE_VISIBLE,
	FileAction, FloatingPanel, FloatingWindow, ModalState, ShellPanels, WORKSPACE_SAVE_DELAY,
	modal_ids,
};

struct ExportRun {
	session: ExportSession,
}

pub struct OakApp<E: AppEngine> {
	engine: Entity<E>,
	program_clock: Entity<E::Clock>,
	timeline: Entity<TimelineView<E>>,
	meter: Entity<AudioLevelMeter<E>>,
	menu_bar: Entity<MenuBar>,
	dock: Entity<DockArea>,
	status_bar: Entity<StatusBar<E>>,
	/// Whether the dark theme is active (toggles via 视图 → 主题).
	dark: bool,
	/// The modal currently shown on top of the shell, if any.
	modal: ModalState<E>,
	/// The shell's own focus handle: the root element tracks it so the
	/// action dispatch layer (the root's `on_action` listeners) sits on the
	/// dispatch path even before any panel takes focus.
	shell_focus: gpui_kit::FocusHandle,
	/// The running export session, if any.
	export: Option<ExportRun>,
	/// A footage drop paused on the empty-timeline drop choice: kept while
	/// its modal (the choice dialog, or the new-sequence dialog it spawns)
	/// is up; performed when a sequence exists. `None` outside the flow.
	pending_drop: Option<FootageDropNeedsSequence>,
	/// Whether the progress modal currently on screen is the OFX plugin
	/// progress dialog (as opposed to the export progress). Guards
	/// [`poll_plugin_progress`] from hijacking the export's bar.
	plugin_progress_open: bool,
	/// The receiving half of the OFX plugin-progress channel (the sending
	/// half is registered into the oakplugin progress suite by
	/// [`crate::oakui::ofx::set_progress_tx`]). Drained in the tick loop to
	/// drive the progress dialog.
	plugin_progress_rx: Mutex<mpsc::Receiver<crate::oakui::ofx::PluginProgressEvent>>,
	/// The library row pending an export save dialog (manager 导出).
	pending_export: Option<String>,
	/// The suggested file stem for the export-project system save dialog.
	export_suggested_name: String,
	/// The panel that most recently took focus (the target of
	/// [`Route::FocusedPanel`](crate::actions::Route::FocusedPanel)
	/// commands; `None` before any panel is focused).
	focused_panel: Option<PanelId>,
	/// The timeline lives OUTSIDE the dock as a full-width bottom strip
	/// (the design: 时间线顶着窗口左右两边缘). gpui-kit's dock composes
	/// its bottom dock between the side docks, so the strip is
	/// shell-managed: visibility toggled by 窗口 → 时间线, height by the
	/// drag handle on its top edge.
	timeline_visible: bool,
	/// The timeline strip's height in px.
	timeline_height: f32,
	/// The eight dock panels, kept so focused-panel commands can be routed
	/// to the right entity (the dock only hands back type-erased handles).
	panels: ShellPanels<E>,
	/// The active timeline tool (the Tools menu's exclusive group; the
	/// timeline toolbar keeps its own visual selection for now).
	active_tool: Tool,
	/// The tool the timeline widget last reported; app tool follows it via
	/// the observe hook on [`Self::timeline`] (toolbar ↔ menu stay in sync).
	last_timeline_tool: crate::widgets::timeline::TimelineTool,
	/// 回放 → 循环播放 is on (the C++ `Loop` config flag; playback looping
	/// itself is a transport gap, so this only drives the checkmark).
	loop_playback: bool,
	/// 视图 → Full Screen is on (placeholder state for the checkmark).
	full_screen: bool,
	/// A checked-but-unshown newer release: the startup check can finish
	/// while another modal (the project manager) is up, so the prompt is
	/// stashed and opens when the modal layer frees up.
	pending_update: Option<crate::update::LatestRelease>,
	/// The transport the startup update check fetches through (tests inject
	/// a scripted one; production uses the real HTTPS client).
	update_transport: Arc<dyn crate::update::UpdateTransport>,
	floating_panels: std::collections::HashMap<PanelId, FloatingWindow>,
	workspace_save_at: Option<Instant>,
	workspace_persistence_suspended: bool,
}

impl<E: AppEngine> OakApp<E> {
	/// Build the whole editor shell, including engine, panels, menus, dock
	/// layout, tick loop and subscriptions.
	///
	/// `window` is the freshly opened main window: the theme, icons, dock
	/// skin, every panel, the tick-loop `spawn` and the initial focus all need
	/// it. `initial_path` is the project path from the command line; it is
	/// opened only after the shell exists, so a bad path logs
	/// `[app] failed to open …` instead of failing construction. `cx` is the
	/// entity context; the returned `Self` is not yet an `Entity` — gpui's
	/// `cx.new` wraps it ([`build_root`]).
	///
	/// Construction order: apply the persisted theme (`Theme` config, dark
	/// when unset) and icons; create the `E` engine plus its source/program
	/// clocks and the timeline view (zoom 2.0), installing the waveform
	/// decorator when the engine exposes a waveform cache; build the menu bar
	/// from the action registry and subscribe to its triggers; load custom
	/// shortcut overrides, bind the action keys and the text-input editing
	/// keys; create the dock with the tab-bar skin and the nine panel
	/// entities; wire panel context menus, the project explorer's requests
	/// (sequence properties/export/rename/delete/new sequence/add text
	/// footage), timeline dialog requests and the viewers' panel events; then
	/// restore the dock layout from `CONFIG_KEY_DOCK_LAYOUT`, falling back to
	/// [`Self::seed_default_layout`] when it is absent or fails to load. It
	/// adds the status bar, repaints on engine notifications, tracks the
	/// focused panel, persists/refreshes on dock layout changes, forwards
	/// timeline events to the engine, starts the ~60 Hz tick loop (playback
	/// clocks, audio meter, export/plugin progress, monitor tracking), focuses
	/// the shell, installs the OFX progress channel, and finally restores
	/// `timeline_visible` / `timeline_height` (clamped to 120–800px) from the
	/// config.
	///
	/// Side effects beyond the widget tree: config reads/writes, a spawned
	/// timer task, global OFX progress registration, and opening
	/// `initial_path`.
	///
	/// # Panics
	///
	/// Does not panic for user data; a malformed saved layout is treated as
	/// "no layout" and the persisted config values are read with defaults.
	pub fn new(window: &mut Window, initial_path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
		let restore_workspace = !cfg!(test);
		// The persisted theme (config `Theme`) wins; the app defaults to dark.
		let dark = crate::oakui::real::theme_is_dark();
		if dark {
			apply_theme(cx, &OakTheme::olive_dark());
		} else {
			apply_theme(cx, &OakTheme::olive_light());
		}
		crate::oakui::icons::init(cx);

		// --- engine and shared state ---------------------------------------
		let engine = cx.new(|cx| E::create(cx));
		let source_clock = engine.read(cx).source_clock().clone();
		let program_clock = engine.read(cx).program_clock().clone();
		let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
		// The toolbar and the Tools menu both drive the widget's tool; the
		// widget is the single source of truth, and any change (from either
		// side) is mirrored into the app tool + menu checkmark here.
		// NOTE: the returned subscription is dropped immediately, so this
		// observer never fires; the drop is kept to preserve behavior.
		let _ = cx.observe(&timeline, |this, timeline, cx| {
			let tool = timeline.read(cx).tool();
			if tool != this.last_timeline_tool {
				this.last_timeline_tool = tool;
				this.active_tool = tool.app_tool();
				this.rebuild_menu_bar(cx);
			}
		});
		// M12 P4: install the waveform decorator when the engine provides
		// a waveform cache.
		if let Some(cache) = engine.read(cx).waveform_cache() {
			let decorator: std::sync::Arc<
				std::sync::RwLock<dyn crate::widgets::timeline::clip::ClipDecorator>,
			> = std::sync::Arc::new(std::sync::RwLock::new(
				crate::oakui::waveform::OakClipDecorator { cache },
			));
			timeline.update(cx, |view, _| view.set_clip_decorator(decorator));
		}
		let meter = cx.new(|cx| {
			AudioLevelMeter::new(3, engine.clone(), window, cx)
				.with_orientation(MeterOrientation::Vertical)
		});

		// --- menu bar ------------------------------------------------------
		// The menus are generated from the action registry; the dynamic
		// checkmarks (theme, tool, snapping, loop, …) come from this state.
		let menu_bar = cx.new(|cx| MenuBar::new(1, make_menus(MenuState::new(dark)), window, cx));
		cx.subscribe(
			&menu_bar,
			|this, _menu: Entity<MenuBar>, event: &MenuBarEvent, cx| {
				if let MenuBarEvent::Triggered { item, .. } = event {
					this.on_menu(*item, cx);
				}
			},
		)
		.detach();

		// --- keyboard map ---------------------------------------------------
		// Load the persisted custom shortcut overrides (`<config>/shortcuts`)
		// *before* binding, so a user's shortcuts file wins over the registry
		// defaults. The bindings below are built from the effective keys.
		crate::actions::load_custom_shortcuts();
		// Register every action's effective key as a global binding (context
		// `None`): the keystrokes dispatch the gpui actions, which bubble to
		// the shell's `on_action` listeners — the same path the menu clicks
		// take through `on_menu`.
		cx.bind_keys(crate::actions::key_bindings());
		// The text-input editing keys (Backspace/Delete/arrows/Home/End/
		// select-all …) are scoped to the `EditableText` key context; without
		// them every field accepts IME text but ignores its editing keys.
		install_text_input_bindings(cx);

		// --- dock ----------------------------------------------------------
		// The dock: gpui-kit's built-in dock area wearing the component
		// skin (tab bars, drop indicators, zoom). Panels are seeded below
		// in the design's default layout; there is no persisted layout yet
		// (the fork's registry was wired but never saved — when layout
		// persistence lands, it goes through `DockArea::dump`/`load`).
		let (dock, dock_skin) = DockSkin::dock_area("oak-main", Some(1), window, cx);
		dock_skin.set_panel_style(PanelStyle::TabBar, cx);
		// Tabs carry their close (✕) button (the fork dock's parity).
		dock_skin.set_close_button_visible(true, cx);

		let project = cx.new(|cx| ProjectExplorerPanel::new(engine.clone(), window, cx));
		let effect_library = cx.new(|cx| EffectLibraryPanel::new(engine.clone(), window, cx));
		let source_viewer =
			cx.new(|cx| SourceViewerPanel::new(engine.clone(), source_clock.clone(), window, cx));
		let program_viewer = cx.new(|cx| {
			ProgramViewerPanel::new(
				engine.clone(),
				program_clock.clone(),
				meter.clone(),
				window,
				cx,
			)
		});
		let node_editor = cx.new(|cx| NodeEditorPanel::new(engine.clone(), window, cx));
		let inspector = cx.new(|cx| InspectorPanel::new(engine.clone(), window, cx));
		let history = cx.new(|cx| HistoryPanel::new(engine.clone(), window, cx));
		let timeline_panel =
			cx.new(|cx| TimelinePanel::new(engine.clone(), timeline.clone(), window, cx));
		let multicam_panel =
			cx.new(|cx| MulticamPanel::new(engine.clone(), program_clock.clone(), window, cx));
		let ai_assistant_panel = cx.new(|cx| AiAssistantPanel::new(engine.clone(), window, cx));

		// Keep the panel entities for focused-panel command routing (the dock
		// only hands back type-erased handles).
		let panels = ShellPanels {
			project: project.clone(),
			source_viewer: source_viewer.clone(),
			program_viewer: program_viewer.clone(),
			node_editor: node_editor.clone(),
			inspector: inspector.clone(),
			history: history.clone(),
			timeline: timeline_panel.clone(),
			effect_library: effect_library.clone(),
			multicam: multicam_panel.clone(),
			ai_assistant: ai_assistant_panel.clone(),
		};

		// Wire each panel's right-click menu: registry-backed items come
		// back through `ContextMenuTriggered` and are dispatched exactly
		// like menu-bar clicks. A right-click does not emit
		// `PanelEvent::Focused`, so the subscription points `focused_panel`
		// at the panel before dispatching (the C++
		// `PanelManager::currently_focused` target is the clicked panel).
		Self::wire_panel_context_menu(cx, &panels.project, PROJECT);
		Self::wire_panel_context_menu(cx, &panels.source_viewer, SOURCE_VIEWER);
		Self::wire_panel_context_menu(cx, &panels.program_viewer, PROGRAM_VIEWER);
		Self::wire_panel_context_menu(cx, &panels.node_editor, NODE_EDITOR);
		Self::wire_panel_context_menu(cx, &panels.inspector, INSPECTOR);
		Self::wire_panel_context_menu(cx, &panels.timeline, TIMELINE);

		// The project explorer's 序列属性 context item opens the sequence
		// properties dialog.
		cx.subscribe(
			&panels.project,
			|this,
			 _panel,
			 event: &crate::panels::project_explorer::SequencePropertiesRequested,
			 cx| {
				this.open_sequence_properties(event.0, cx);
			},
		)
		.detach();

		// The project explorer's 导出序列 context item opens the export
		// dialog with that sequence preselected.
		cx.subscribe(
			&panels.project,
			|this, _panel, event: &crate::panels::project_explorer::ExportSequenceRequested, cx| {
				this.open_export_dialog(Some(event.0), cx);
			},
		)
		.detach();

		// The project explorer's 重命名 / 删除 context items operate on
		// the project's entry (rename prompts; delete removes the node).
		cx.subscribe(
			&panels.project,
			|this, _panel, event: &crate::panels::project_explorer::RenameRequested, cx| {
				this.open_entry_rename(event.0, cx);
			},
		)
		.detach();
		cx.subscribe(
			&panels.project,
			|this, _panel, event: &crate::panels::project_explorer::DeleteRequested, cx| {
				this.engine
					.update(cx, |engine, cx| engine.delete_entry(event.0, cx));
			},
		)
		.detach();

		// The project explorer's 新建序列 button opens the new-sequence
		// dialog (the same one File > New > Sequence… uses).
		cx.subscribe(
			&panels.project,
			|this, _panel, _event: &crate::panels::project_explorer::NewSequenceRequested, cx| {
				this.open_new_sequence(cx);
			},
		)
		.detach();

		// The project explorer's 添加文本素材 button creates a text
		// generator bin entry in the open project.
		cx.subscribe(
			&panels.project,
			|this,
			 _panel,
			 _event: &crate::panels::project_explorer::NewTextFootageRequested,
			 cx| {
				this.engine.update(cx, |engine, cx| {
					if let Err(e) = engine.create_text_footage(cx) {
						println!("[shell] add text footage: {e}");
					}
				});
			},
		)
		.detach();

		// A footage drop onto a timeline with no open sequence goes through
		// the probe-vs-manual choice before any sequence is created.
		cx.subscribe(
			&panels.timeline,
			|this, _panel, event: &FootageDropNeedsSequence, cx| {
				this.on_footage_drop_needs_sequence(event.clone(), cx);
			},
		)
		.detach();

		// The timeline's dialog requests (rename / speed prompts) open as
		// shell modals; the result lands through the engine.
		cx.subscribe(
			&panels.timeline,
			|this, _panel, event: &crate::panels::timeline::TimelineDialogRequest, cx| match event {
				crate::panels::timeline::TimelineDialogRequest::Rename { clips } => {
					this.open_clip_rename(clips.clone(), cx);
				}
				crate::panels::timeline::TimelineDialogRequest::Speed { clips } => {
					this.open_clip_speed(clips.clone(), cx);
				}
			},
		)
		.detach();

		// The viewer panels re-emit their monitor-level requests (full-screen
		// and the loop in/out range) for the shell to apply: full-screen
		// toggles the window, and the in/out/clear requests act on the shared
		// program workarea (both monitors use it).
		cx.subscribe(
			&panels.source_viewer,
			|this, _panel, event: &menu::ViewerPanelEvent, cx| {
				this.apply_viewer_panel_event(Monitor::Source, *event, cx);
			},
		)
		.detach();
		cx.subscribe(
			&panels.program_viewer,
			|this, _panel, event: &menu::ViewerPanelEvent, cx| {
				this.apply_viewer_panel_event(Monitor::Program, *event, cx);
			},
		)
		.detach();

		panels.register_for_layout_restore(cx);
		let saved_layout = restore_workspace
			.then(|| {
				serde_json::from_str::<DockAreaState>(&crate::oakui::real::config_get_string(
					CONFIG_KEY_DOCK_LAYOUT,
				))
				.ok()
			})
			.flatten();
		dock.update(cx, |dock, cx| {
			let restored = saved_layout
				.map(|state| dock.load(state, window, cx).is_ok())
				.unwrap_or(false);
			if !restored {
				Self::seed_default_layout(dock, &panels, window, cx);
			}
		});

		// --- status bar ----------------------------------------------------
		let status_bar = cx.new(|cx| StatusBar::new(engine.clone(), program_clock.clone(), cx));

		// Repaint the shell whenever the engine notifies.
		cx.observe(&engine, |this, _engine, cx| {
			cx.notify();
			let _ = this;
		})
		.detach();

		// Track the focused panel: every panel emits `PanelFocused` on a
		// click inside it; the shell subscribes per panel and routes
		// focused-panel commands to the last one heard from.
		Self::wire_panel_focus(cx, &panels.project, PROJECT);
		Self::wire_panel_focus(cx, &panels.source_viewer, SOURCE_VIEWER);
		Self::wire_panel_focus(cx, &panels.program_viewer, PROGRAM_VIEWER);
		Self::wire_panel_focus(cx, &panels.node_editor, NODE_EDITOR);
		Self::wire_panel_focus(cx, &panels.inspector, INSPECTOR);
		Self::wire_panel_focus(cx, &panels.history, HISTORY);
		Self::wire_panel_focus(cx, &panels.timeline, TIMELINE);
		Self::wire_panel_focus(cx, &panels.effect_library, EFFECT_LIBRARY);
		Self::wire_panel_focus(cx, &panels.multicam, MULTICAM);

		// Structural changes (a panel closed via its tab ✕ or re-docked)
		// refresh the 窗口 menu's checkmarks, which read the dock's live
		// visible-panel set.
		cx.subscribe(&dock, |this, _dock, event: &DockEvent, cx| {
			if matches!(event, DockEvent::LayoutChanged) {
				this.persist_workspace(cx);
				this.rebuild_menu_bar(cx);
				cx.notify();
			}
		})
		.detach();

		// --- timeline events -----------------------------------------------
		// Every timeline widget request (playhead seek, trim, move, track
		// height) is applied by the engine through its backend's edit
		// commands; the playhead is routed to the program monitor.
		// `SelectionChanged` carries no payload — the selection is read from
		// the view and forwarded to the engine so the inspector's effect
		// stack can target the selected clip.
		cx.subscribe(&timeline, |this, timeline, event: &TimelineEvent, cx| {
			if matches!(event, TimelineEvent::SelectionChanged) {
				let clips: Vec<ClipId> = timeline.read(cx).selection().iter().copied().collect();
				this.engine
					.update(cx, |engine, cx| engine.set_selected_clips(clips, cx));
			}
			this.engine
				.update(cx, |engine, cx| engine.apply_timeline_event(event, cx));
		})
		.detach();

		// --- tick loop -----------------------------------------------------
		// Drives playback clocks, playhead sync, the audio meter and the
		// export progress at ~60Hz.
		let this = cx.weak_entity();
		window
			.spawn(cx, async move |cx: &mut AsyncWindowContext| {
				loop {
					cx.background_executor()
						.timer(Duration::from_millis(16))
						.await;
					let _ = cx.update(|window, app| {
						// Multi-monitor: track which physical display the window
						// sits on so the self-managed display transform follows
						// window moves (throttled inside `poll_monitor`).
						crate::oakui::displaycolor::poll_monitor(window, app);
						if let Some(this) = this.upgrade() {
							this.update(app, |this, cx| this.tick(cx));
						}
					});
				}
			})
			.detach();

		let shell_focus = cx.focus_handle();
		// The shell starts focused so the action dispatch layer works before
		// any panel grabs focus.
		window.focus(&shell_focus, cx);
		// OFX plugin-progress channel: the oakplugin progress suite pushes
		// (label, message, fraction) events here; the tick loop drives the
		// progress dialog.
		let (plugin_progress_tx, plugin_progress_rx) =
			mpsc::channel::<crate::oakui::ofx::PluginProgressEvent>();
		crate::oakui::ofx::set_progress_tx(plugin_progress_tx);
		let shell = Self {
			engine,
			program_clock,
			timeline,
			meter,
			menu_bar,
			dock,
			status_bar,
			dark,
			modal: ModalState::None,
			shell_focus,
			export: None,
			pending_drop: None,
			pending_export: None,
			export_suggested_name: String::new(),
			plugin_progress_open: false,
			plugin_progress_rx: Mutex::new(plugin_progress_rx),
			focused_panel: None,
			timeline_visible: if restore_workspace {
				crate::oakui::real::config_get_bool(CONFIG_KEY_TIMELINE_VISIBLE, true)
			} else {
				true
			},
			timeline_height: if restore_workspace {
				(crate::oakui::real::config_get_int(CONFIG_KEY_TIMELINE_HEIGHT, 320) as f32)
					.clamp(120.0, 800.0)
			} else {
				320.0
			},
			panels,
			active_tool: Tool::Pointer,
			last_timeline_tool: crate::widgets::timeline::TimelineTool::Select,
			loop_playback: false,
			full_screen: false,
			pending_update: None,
			update_transport: Arc::new(crate::update::HttpTransport),
			floating_panels: std::collections::HashMap::new(),
			workspace_save_at: None,
			workspace_persistence_suspended: false,
		};

		// Open the CLI-provided project once the shell is up.
		if let Some(path) = initial_path {
			shell.engine.update(cx, |engine, cx| {
				if let Err(err) = engine.open_project_path(path.clone(), cx) {
					println!("[app] failed to open {}: {err}", path.display());
				}
			});
		}

		shell
	}

	pub(crate) fn persist_workspace(&mut self, cx: &App) {
		if cfg!(test) {
			return;
		}
		crate::oakui::real::config_set_bool(CONFIG_KEY_TIMELINE_VISIBLE, self.timeline_visible);
		crate::oakui::real::config_set_int(
			CONFIG_KEY_TIMELINE_HEIGHT,
			self.timeline_height.round() as i64,
		);
		if self.workspace_persistence_suspended || !self.floating_panels.is_empty() {
			return;
		}
		if let Ok(layout) = serde_json::to_string(&self.dock.read(cx).dump(cx)) {
			crate::oakui::real::config_set_string(CONFIG_KEY_DOCK_LAYOUT, &layout);
		}
		self.workspace_save_at = Some(Instant::now() + WORKSPACE_SAVE_DELAY);
	}

	/// One animation-frame tick: advance the engine, sync the timeline
	/// playhead to the program clock, refresh the audio meter and drain the
	/// export progress events.
	pub(crate) fn tick(&mut self, cx: &mut Context<Self>) {
		if self
			.workspace_save_at
			.is_some_and(|deadline| Instant::now() >= deadline)
		{
			self.workspace_save_at = None;
			crate::oakui::real::config_save();
		}
		self.engine.update(cx, |engine, cx| engine.tick(cx));
		let frame = self.program_clock.read(cx).current_frame();
		self.timeline
			.update(cx, |timeline, cx| timeline.seek(frame, cx));
		// Mirror the engine's work area into the ruler's view state (M12 P4).
		// Read every tick so undo/redo and the ruler-drag commit land on the
		// band promptly; the read is a cheap engine getter.
		let work_area = self.engine.read(cx).workarea();
		self.timeline.update(cx, |timeline, _| {
			timeline.state.work_area = work_area.map(|(s, e)| FrameRange::new(s, e))
		});
		self.meter.update(cx, |meter, cx| meter.update(cx));
		self.poll_export(cx);
		self.poll_plugin_progress(cx);
		cx.notify();
	}

	/// Drains the OFX plugin-progress channel: the first event of a render
	/// opens the progress dialog, subsequent events update the bar, and the
	/// dialog closes when the fraction reaches 1.0 (the plugin's
	/// progressEnd is not surfaced by the reporter, so completion is
	/// inferred from the fraction).
	pub(crate) fn poll_plugin_progress(&mut self, cx: &mut Context<Self>) {
		let mut events = Vec::new();
		while let Ok(event) = self.plugin_progress_rx.lock().unwrap().try_recv() {
			events.push(event);
		}
		if events.is_empty() {
			return;
		}
		let last = events.last().cloned().unwrap_or_default();
		// Ensure the plugin progress dialog is on screen. If another modal
		// is already up (e.g. export progress), leave it alone.
		if !self.plugin_progress_open && matches!(self.modal, ModalState::None) {
			let title = crate::i18n::tr("ofx.progress.title");
			let label = last.label.clone();
			let message = last.message.clone();
			self.spawn_modal(cx, move |window, app| {
				let (modal, content) = progress_dialog(
					modal_ids::PLUGIN_PROGRESS,
					title,
					format!("{label}\n{message}"),
					window,
					app,
				);
				ModalState::Progress { modal, content }
			});
			self.plugin_progress_open = true;
		}
		if self.plugin_progress_open {
			if let ModalState::Progress { content, .. } = &self.modal {
				let fraction = last.fraction as f32;
				content.update(cx, |content, cx| content.set_progress(fraction, cx));
			}
			// The reporter answers false after the user cancels, and the
			// fraction reaches 1.0 when the render completes; either way the
			// dialog is dismissed.
			if last.fraction >= 1.0 {
				self.modal = ModalState::None;
				self.plugin_progress_open = false;
				cx.notify();
			}
		}
	}

	/// Route one menu item id to its effect.
	///
	/// `item` is the stable numeric id a menu entry (or a panel context menu)
	/// reports; `cx` is the entity context. Dynamic language items live above
	/// the registry id range: if [`menu::language_item_index`] recognises the
	/// id, the corresponding entry of
	/// [`crate::i18n::available_languages`] is selected and
	/// [`Self::switch_language`] runs — an index that no longer exists is
	/// ignored. Otherwise the id is resolved through
	/// [`crate::actions::entry_for_menu_id`] and dispatched with
	/// [`Self::dispatch_action_id`], the same path keyboard shortcuts use, so
	/// a menu click and its shortcut can never diverge.
	///
	/// An unknown id only prints `[menu] unknown item <id>`; nothing else
	/// happens, which makes stale menu ids from older builds harmless.
	pub(crate) fn on_menu(&mut self, item: usize, cx: &mut Context<Self>) {
		if let Some(index) = menu::language_item_index(item) {
			let languages = crate::i18n::available_languages();
			if let Some(code) = languages.get(index) {
				let code = code.clone();
				self.switch_language(&code, cx);
			}
			return;
		}
		let Some(entry) = crate::actions::entry_for_menu_id(item) else {
			println!("[menu] unknown item {item}");
			return;
		};
		self.dispatch_action_id(entry.action, cx);
	}

	/// Subscribes the shell to a panel's right-click menu: a triggered
	/// registry item is dispatched like a menu-bar click, with the panel set
	/// as the focused panel first (a right-click does not emit
	/// `PanelEvent::Focused`).
	pub(crate) fn wire_panel_context_menu<P>(cx: &mut Context<Self>, panel: &Entity<P>, id: PanelId)
	where
		P: gpui_kit::EventEmitter<menu::ContextMenuTriggered>,
	{
		cx.subscribe(
			panel,
			move |this, _panel, event: &menu::ContextMenuTriggered, cx| {
				this.focused_panel = Some(id);
				this.on_menu(event.item, cx);
			},
		)
		.detach();
	}

	/// The full-width timeline strip (window bottom, above the status bar):
	/// a 4px drag handle on its top edge resizes it, the panel fills the
	/// rest.
	pub(crate) fn render_timeline_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
		let height = self.timeline_height;
		let handle = div()
			.id("timeline-resize-handle")
			.h(px(4.0))
			.w_full()
			.cursor(gpui_kit::CursorStyle::ResizeRow)
			.on_drag(
				std::sync::Arc::new(std::sync::RwLock::new(TimelineStripDrag {
					start_height: height,
					start_y: None,
				})),
				|_drag, _origin, _window, cx| cx.new(|_| TimelineStripGhost),
			);
		div()
			.flex_shrink_0()
			.h(px(height))
			.w_full()
			.flex()
			.flex_col()
			.on_drag_move(cx.listener(
				|this,
				 event: &gpui_kit::DragMoveEvent<
					std::sync::Arc<std::sync::RwLock<TimelineStripDrag>>,
				>,
				 _window,
				 cx| {
					let drag = event.drag(cx).clone();
					let mut drag = drag.write().unwrap_or_else(|e| e.into_inner());
					let now = event.event.position.y;
					let base = *drag.start_y.get_or_insert(now);
					// The strip is bottom-anchored: dragging up grows it.
					this.timeline_height =
						(drag.start_height + f32::from(base - now)).clamp(120.0, 800.0);
					this.persist_workspace(cx);
					cx.notify();
				},
			))
			.child(handle)
			.child(div().flex_1().min_h_0().child(self.panels.timeline.clone()))
	}
}

mod actions;
mod dialogs;
mod layout;
mod manager;
mod modal;
mod preferences;

impl<E: AppEngine> Render for OakApp<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let mut root = div()
			.size_full()
			.flex()
			.flex_col()
			// The shell paints the design's near-black base behind the dock;
			// panels and chrome layer their own fills on top.
			.bg(cx.default_colors().background)
			// Keep the dispatch context stack non-empty everywhere in the
			// shell: predicate-gated key bindings (`!EditableText`) never match
			// on an empty stack, so without a root context every global
			// shortcut goes dead outside the panels that set their own.
			.key_context("OakApp")
			.track_focus(&self.shell_focus);
		// The shell's keyboard dispatch layer: the global key bindings
		// (registered in `new` from the action registry) dispatch gpui
		// actions, which bubble up from the focused widget and land here —
		// one listener per registry action, all routing through the same
		// `dispatch_action_id` the menu clicks use.
		for entry in crate::actions::REGISTRY {
			let action = entry.action;
			root = root.on_boxed_action(
				&*(entry.build)(),
				cx.listener(move |this, _action: &dyn gpui_kit::Action, _window, cx| {
					cx.stop_propagation();
					this.on_action_dispatched(action, cx);
				}),
			);
		}
		root = root.on_drop(cx.listener(|this, drag: &DragPanel, _window, cx| {
			if let Some(id) = this.panels.id_of_raw(drag.panel().as_u64()) {
				this.open_floating_panel(id, cx);
				cx.stop_propagation();
			}
		}));
		let mut root = root
			.child(self.menu_bar.clone())
			.child(div().flex_1().min_h_0().child(self.dock.clone()))
			.children(if self.timeline_visible {
				Some(self.render_timeline_strip(cx))
			} else {
				None
			})
			.child(self.status_bar.clone());
		if let Some(modal) = self.modal.modal_entity() {
			root = root.child(modal);
		}
		root
	}
}

/// The timeline strip's resize drag payload.
struct TimelineStripDrag {
	/// The strip height at drag start.
	start_height: f32,
	/// The pointer y of the first drag move (the delta baseline).
	start_y: Option<gpui_kit::Pixels>,
}

/// The resize drag ghost (invisible; the strip resizes live).
struct TimelineStripGhost;

impl Render for TimelineStripGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_0()
	}
}

/// The ten dockable panels in the 窗口 menu's order, each with the registry
/// action that toggles it.
const WINDOW_PANELS: [(PanelId, crate::actions::ActionId); 10] = [
	(PROJECT, crate::actions::ActionId::FocusProject),
	(SOURCE_VIEWER, crate::actions::ActionId::FocusSourceViewer),
	(PROGRAM_VIEWER, crate::actions::ActionId::FocusProgramViewer),
	(NODE_EDITOR, crate::actions::ActionId::FocusNodeEditor),
	(INSPECTOR, crate::actions::ActionId::FocusInspector),
	(HISTORY, crate::actions::ActionId::FocusHistory),
	(TIMELINE, crate::actions::ActionId::FocusTimeline),
	(EFFECT_LIBRARY, crate::actions::ActionId::FocusEffectLibrary),
	(MULTICAM, crate::actions::ActionId::FocusMulticam),
	(AI_ASSISTANT, crate::actions::ActionId::FocusAiAssistant),
];

/// The bit for panel `index` (the [`WINDOW_PANELS`] order) in
/// [`MenuState::open_panels`].
fn panel_bit(index: usize) -> u16 {
	1u16 << index
}

/// The bit for `id` (tests).
#[cfg(test)]
fn panel_bit_for(id: PanelId) -> u16 {
	WINDOW_PANELS
		.iter()
		.position(|(panel, _)| *panel == id)
		.map(panel_bit)
		.unwrap_or(0)
}

/// The design's default dock placement for a panel (used when a closed
/// panel reopens): 项目 | 效果库 left, 检查器 | 历史记录 right, 时间线
/// bottom, the viewers/editors center.
fn default_dock_placement(id: PanelId) -> (DockPlacement, Option<gpui_kit::Pixels>) {
	match id {
		PROJECT | EFFECT_LIBRARY => (DockPlacement::Left, Some(px(280.0))),
		INSPECTOR | HISTORY | AI_ASSISTANT => (DockPlacement::Right, Some(px(340.0))),
		TIMELINE => (DockPlacement::Bottom, Some(px(320.0))),
		_ => (DockPlacement::Center, None),
	}
}

/// The 窗口 menu's open-panel bitmask for `dock`'s current visible panel set
/// (docked or floating). Shared by [`OakApp::rebuild_menu_bar`] and tests.
fn open_panels_mask(
	dock: &DockArea,
	panels: &ShellPanels<impl AppEngine>,
	timeline_visible: bool,
) -> u16 {
	WINDOW_PANELS
		.iter()
		.enumerate()
		.map(|(i, (id, _))| {
			// The timeline strip is shell-owned, not docked.
			let open = if *id == TIMELINE {
				timeline_visible
			} else {
				panels
					.entity_id(id)
					.map(|entity| dock.panel(DockPanelId::from(entity)).is_some())
					.unwrap_or(false)
			};
			if open { panel_bit(i) } else { 0 }
		})
		.fold(0, |a, b| a | b)
}

mod menu_bar;
#[cfg(test)]
pub(crate) use menu_bar::make_menus_for_test;
pub(crate) use menu_bar::{MenuState, make_menus, menu_action_paths};

mod app_args;
pub use app_args::run;
#[cfg(test)]
pub(crate) use app_args::{AppArgs, build_root};

#[cfg(test)]
mod tests;

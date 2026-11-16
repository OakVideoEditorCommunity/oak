use super::*;

impl<E: AppEngine> OakApp<E> {
	/// 窗口 menu toggle: clicking a listed panel opens it if it is closed and
	/// closes it if it is open. Docked panels are removed through the dock's
	/// remove flow — the same path the tab ✕ button takes (see
	/// [`DockArea::remove_panel`]) — so the close is symmetrical and the
	/// position is recorded for a later reopen. Floating (tear-off) panels
	/// have their window closed for good. Rebuilds the menu bar so the
	/// checkmarks follow.
	/// Points the shell's focused-panel routing at the panel that emitted
	/// [`PanelFocused`](crate::panels::PanelFocused).
	pub(crate) fn wire_panel_focus<P>(cx: &mut Context<Self>, panel: &Entity<P>, id: PanelId)
	where
		P: gpui_kit::EventEmitter<crate::panels::PanelFocused>,
	{
		cx.subscribe(
			panel,
			move |this, _panel, _event: &crate::panels::PanelFocused, _cx| {
				this.focused_panel = Some(id);
			},
		)
		.detach();
	}

	/// Runs `f` against the dock with the main window (gpui-kit's panel
	/// adds/removes need both). No window (tests) skips the edit.
	pub(crate) fn update_dock(
		&mut self,
		cx: &mut Context<Self>,
		f: impl FnOnce(&mut DockArea, &mut Window, &mut Context<DockArea>),
	) {
		let dock = self.dock.clone();
		let Some(handle) = cx.windows().first().copied() else {
			return;
		};
		let _ = cx.update_window(handle, |_, window, cx| {
			dock.update(cx, |dock, cx| f(dock, window, cx));
		});
	}

	pub(crate) fn open_floating_panel(&mut self, id: PanelId, cx: &mut Context<Self>) {
		if self.floating_panels.contains_key(id) {
			return;
		}
		let Some(view) = self.panels.floating_view(id) else {
			return;
		};
		self.workspace_persistence_suspended = true;
		self.toggle_panel(id, cx);
		let Ok(handle) = cx.open_window(
			WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds {
					origin: gpui_kit::point(px(120.0), px(120.0)),
					size: size(px(960.0), px(640.0)),
				})),
				titlebar: Some(TitlebarOptions {
					title: Some(id.into()),
					appears_transparent: false,
					traffic_light_position: None,
				}),
				focus: true,
				show: true,
				..Default::default()
			},
			move |_window, cx| cx.new(|_| FloatingPanel { view }),
		) else {
			self.workspace_persistence_suspended = false;
			return;
		};
		let window_id = gpui_kit::AnyWindowHandle::from(handle).window_id();
		let this = cx.weak_entity();
		let subscription = cx.on_window_closed(move |cx, closed_id| {
			if closed_id != window_id {
				return;
			}
			let this = this.clone();
			cx.defer(move |cx| {
				let _ = this.update(cx, |this, cx| {
					this.floating_panels.remove(&id);
					match id {
						SOURCE_VIEWER => this.panels.source_viewer.update(cx, |panel, cx| {
							panel.set_video_fullscreen(false, cx);
						}),
						PROGRAM_VIEWER => this.panels.program_viewer.update(cx, |panel, cx| {
							panel.set_video_fullscreen(false, cx);
						}),
						_ => {}
					}
					this.toggle_panel(id, cx);
				});
			});
		});
		self.floating_panels.insert(
			id,
			FloatingWindow {
				window_id,
				_subscription: subscription,
			},
		);
		self.workspace_persistence_suspended = false;
		self.rebuild_menu_bar(cx);
	}

	pub(crate) fn close_floating_panel(&mut self, id: PanelId, cx: &mut Context<Self>) {
		let Some(floating) = self.floating_panels.remove(&id) else {
			return;
		};
		let _ = gpui_kit::WindowHandle::<FloatingPanel>::new(floating.window_id).update(
			cx,
			|_, window, _| {
				window.remove_window();
			},
		);
	}

	pub(crate) fn close_all_floating_panels(&mut self, cx: &mut Context<Self>) {
		let ids: Vec<PanelId> = self.floating_panels.keys().copied().collect();
		for id in ids {
			self.close_floating_panel(id, cx);
		}
	}

	/// Toggle one panel open/closed (窗口 → Focus X menu entries and the
	/// viewer's panel switches).
	///
	/// `id` selects the panel; `cx` is the entity context. Three cases:
	/// - the panel currently lives in a floating window → the floating window
	///   is removed ([`Self::close_floating_panel`]) and the call ends;
	/// - `id == TIMELINE` → the shell-owned bottom strip is shown/hidden by
	///   flipping `timeline_visible`, then the workspace is persisted;
	/// - any other panel → present in the dock means remove it, absent means
	///   re-add it at its design placement/size from
	///   [`default_dock_placement`]. The panel entity is never dropped, so all
	///   of its state (selection, scroll, ...) survives the round trip.
	///
	/// All paths finish by rebuilding the menu bar (so the checkmark matches)
	/// and notifying the shell. gpui-kit's dock has no floating-panel support,
	/// so panels always come back docked; the fork's tear-off handling is gone.
	pub(crate) fn toggle_panel(&mut self, id: PanelId, cx: &mut Context<Self>) {
		if self.floating_panels.contains_key(id) {
			self.close_floating_panel(id, cx);
			return;
		}
		// The timeline is shell-owned (the full-width strip), not docked.
		if id == TIMELINE {
			self.timeline_visible = !self.timeline_visible;
			self.persist_workspace(cx);
			self.rebuild_menu_bar(cx);
			cx.notify();
			return;
		}
		let panels = self.panels.clone();
		self.update_dock(cx, move |dock, window, cx| {
			macro_rules! toggle {
				($panel:expr) => {{
					let panel = $panel.clone();
					if dock.panel(DockPanelId::from(panel.entity_id())).is_some() {
						dock.remove_panel(panel, window, cx);
					} else {
						let (placement, size) = default_dock_placement(id);
						// `add_panel_view` + `panel_handle`: the dock skin
						// recovers the title / toolbar buttons / zoom control
						// from the presentation handle; a bare `add_panel`
						// entity falls back to drawing the raw panel name.
						dock.add_panel_view(panel_handle(panel), placement, size, window, cx);
					}
				}};
			}
			match id {
				PROJECT => toggle!(panels.project),
				SOURCE_VIEWER => toggle!(panels.source_viewer),
				PROGRAM_VIEWER => toggle!(panels.program_viewer),
				NODE_EDITOR => toggle!(panels.node_editor),
				INSPECTOR => toggle!(panels.inspector),
				HISTORY => toggle!(panels.history),
				EFFECT_LIBRARY => toggle!(panels.effect_library),
				MULTICAM => toggle!(panels.multicam),
				AI_ASSISTANT => toggle!(panels.ai_assistant),
				TIMELINE => unreachable!("handled above"),
				_ => {}
			}
		});
		self.rebuild_menu_bar(cx);
		cx.notify();
	}

	/// 窗口 → 重置布局: clears the dock and re-seeds every panel at its
	/// design position.
	pub(crate) fn reset_default_layout(&mut self, cx: &mut Context<Self>) {
		self.close_all_floating_panels(cx);
		let panels = self.panels.clone();
		self.update_dock(cx, move |dock, window, cx| {
			macro_rules! remove {
				($panel:expr) => {{
					let panel = $panel.clone();
					if dock.panel(DockPanelId::from(panel.entity_id())).is_some() {
						dock.remove_panel(panel, window, cx);
					}
				}};
			}
			remove!(panels.project);
			remove!(panels.source_viewer);
			remove!(panels.program_viewer);
			remove!(panels.node_editor);
			remove!(panels.inspector);
			remove!(panels.history);
			remove!(panels.timeline);
			remove!(panels.effect_library);
			remove!(panels.multicam);
			remove!(panels.ai_assistant);
			Self::seed_default_layout(dock, &panels, window, cx);
		});
		self.timeline_visible = true;
		self.timeline_height = 320.0;
		self.persist_workspace(cx);
		self.rebuild_menu_bar(cx);
		cx.notify();
	}

	/// Seed every panel of `panels` at its design position in `dock`.
	///
	/// Shared by [`OakApp::new`] and [`Self::reset_default_layout`], so a first
	/// launch and a layout reset produce the same arrangement: a center
	/// h-split with 素材查看器 (source viewer, 430px) beside a tab group of
	/// 序列查看器 (program viewer) and the node editor; 项目 (280px) and
	/// 效果库 tabbed on the left; 检查器 (340px) and 历史记录 tabbed on the
	/// right; the multicam page added to the center, which merges it into the
	/// source viewer's tab group; then the active tabs are selected — project,
	/// source viewer, program viewer, inspector. The timeline is **not**
	/// docked: it is the shell's full-width bottom strip, rendered outside the
	/// dock.
	///
	/// `window` and `cx` (the dock area's context) are needed by the dock
	/// calls; every panel goes in through `panel_handle` so the skin can
	/// recover its localized title, toolbar and zoom affordance. Panels must
	/// already exist and be un-docked — the function only inserts, it never
	/// removes, so calling it twice would duplicate entries.
	pub(crate) fn seed_default_layout(
		dock: &mut DockArea,
		panels: &ShellPanels<E>,
		window: &mut Window,
		cx: &mut Context<DockArea>,
	) {
		// Every panel goes in through `panel_handle` so the dock skin can
		// recover its presentation (localized tab title, toolbar buttons,
		// zoom affordance); a bare entity renders as its raw panel name.
		dock.set_center(
			DockLayout::h_split()
				.child(
					DockLayout::tabs().panel_view(panel_handle(panels.source_viewer.clone()), cx),
					Some(px(430.0)),
				)
				.child(
					DockLayout::tabs()
						.panel_view(panel_handle(panels.program_viewer.clone()), cx)
						.panel_view(panel_handle(panels.node_editor.clone()), cx)
						.active_index(0),
					None,
				),
			window,
			cx,
		);
		dock.add_panel_view(
			panel_handle(panels.project.clone()),
			DockPlacement::Left,
			Some(px(280.0)),
			window,
			cx,
		);
		dock.add_panel_view(
			panel_handle(panels.effect_library.clone()),
			DockPlacement::Left,
			None,
			window,
			cx,
		);
		dock.add_panel_view(
			panel_handle(panels.inspector.clone()),
			DockPlacement::Right,
			Some(px(340.0)),
			window,
			cx,
		);
		dock.add_panel_view(
			panel_handle(panels.history.clone()),
			DockPlacement::Right,
			None,
			window,
			cx,
		);
		// (The timeline is not docked: it is the shell's full-width bottom
		// strip, see the render.)
		// Center merges into the first tab group — the source viewer's.
		dock.add_panel_view(
			panel_handle(panels.multicam.clone()),
			DockPlacement::Center,
			None,
			window,
			cx,
		);
		dock.add_panel_view(
			panel_handle(panels.ai_assistant.clone()),
			DockPlacement::Right,
			Some(px(340.0)),
			window,
			cx,
		);

		// The design's active tabs (a tab group paints only its active
		// panel): project bin left, source + program viewers center,
		// inspector right.
		for id in [PROJECT, SOURCE_VIEWER, PROGRAM_VIEWER, INSPECTOR] {
			if let Some(entity) = panels.entity_id(id) {
				dock.select_panel(DockPanelId::from(entity), window, cx);
			}
		}
	}

	/// Switch the UI language live, without a restart.
	///
	/// `code` is an i18n language code (e.g. `en-US`, `zh-CN`); `cx` is the
	/// entity context. Sets the [`crate::i18n`] global, rebuilds the menu bar
	/// so its labels and the language checkmark follow immediately
	/// ([`Self::rebuild_menu_bar`]), and notifies the shell to repaint. The
	/// dock tab titles need no explicit refresh: the skin re-evaluates each
	/// panel's `title()` every frame through its presentation handle.
	///
	/// Unknown codes are handled by [`crate::i18n::set_language_code`] (the
	/// caller's checkmark simply won't move); no error is returned.
	pub(crate) fn switch_language(&mut self, code: &str, cx: &mut Context<Self>) {
		crate::i18n::set_language_code(code);
		self.rebuild_menu_bar(cx);
		cx.notify();
	}

	/// Toggles the window's full-screen state (the 视图 → 全屏 menu entry and
	/// the viewer context menu's `FullScreenRequested`), then refreshes the
	/// menu bar so the checkmark follows the real window state.
	pub(crate) fn toggle_full_screen(&mut self, cx: &mut Context<Self>) {
		let windows = cx.windows();
		let Some(window) = windows.first() else {
			return;
		};
		let Ok(full_screen) = cx.update_window(*window, |_root, window, _app| {
			window.toggle_fullscreen();
			window.is_fullscreen()
		}) else {
			return;
		};
		self.full_screen = full_screen;
		self.rebuild_menu_bar(cx);
	}

	/// Applies a viewer panel's shell-level request: full-screen toggles the
	/// window, and the loop-range requests move the shared program workarea
	/// (both monitors act on it through the same undoable path as the menu).
	pub(crate) fn toggle_viewer_fullscreen(&mut self, monitor: Monitor, cx: &mut Context<Self>) {
		let id = match monitor {
			Monitor::Source => SOURCE_VIEWER,
			Monitor::Program => PROGRAM_VIEWER,
		};
		self.open_floating_panel(id, cx);
		let Some(floating) = self.floating_panels.get(&id) else {
			return;
		};
		let window_id = floating.window_id;
		let was_fullscreen = gpui_kit::WindowHandle::<FloatingPanel>::new(window_id)
			.update(cx, |_, window, _| window.is_fullscreen())
			.unwrap_or(false);
		match monitor {
			Monitor::Source => self.panels.source_viewer.update(cx, |panel, cx| {
				panel.set_video_fullscreen(!was_fullscreen, cx);
			}),
			Monitor::Program => self.panels.program_viewer.update(cx, |panel, cx| {
				panel.set_video_fullscreen(!was_fullscreen, cx);
			}),
		}
		let _ =
			gpui_kit::WindowHandle::<FloatingPanel>::new(window_id).update(cx, |_, window, _| {
				window.toggle_fullscreen();
			});
	}

	/// Apply a shell-level request emitted by a viewer panel (source or
	/// program monitor).
	///
	/// `monitor` says which viewer sent it — only `FullScreenRequested` uses
	/// it, since that toggles the panel's own floating window; `cx` is the
	/// entity context. `FullScreenRequested` delegates to
	/// [`Self::toggle_viewer_fullscreen`] (open/floating window + video
	/// fullscreen toggle). `SetInPoint` / `SetOutPoint` call
	/// [`Self::set_point_at_playhead`] with `true`/`false`, marking the shared
	/// program work-area in/out at the playhead. `ClearRange` calls the
	/// engine's `clear_workarea` — the same undoable path the menu uses, so
	/// both monitors and the menu edit one shared workarea.
	///
	/// The three range actions ignore `monitor` (there is one workarea, not
	/// one per viewer); an engine error from `clear_workarea` is discarded.
	pub(crate) fn apply_viewer_panel_event(
		&mut self,
		monitor: Monitor,
		event: menu::ViewerPanelEvent,
		cx: &mut Context<Self>,
	) {
		match event {
			menu::ViewerPanelEvent::FullScreenRequested => {
				self.toggle_viewer_fullscreen(monitor, cx)
			}
			menu::ViewerPanelEvent::SetInPoint => self.set_point_at_playhead(true, cx),
			menu::ViewerPanelEvent::SetOutPoint => self.set_point_at_playhead(false, cx),
			menu::ViewerPanelEvent::ClearRange => {
				self.engine
					.update(cx, |engine, cx| engine.clear_workarea(cx));
			}
		}
	}

	/// Replaces the `MenuBar` entity with one built from the current language
	/// and the dynamic checkmark state, re-subscribing to its trigger events.
	pub(crate) fn rebuild_menu_bar(&mut self, cx: &mut Context<Self>) {
		let windows = cx.windows();
		let Some(handle) = windows.first() else {
			return;
		};
		let state = MenuState {
			dark: self.dark,
			active_tool: self.active_tool,
			snapping: self.timeline.read(cx).state.snap_enabled,
			loop_playback: self.loop_playback,
			full_screen: self.full_screen,
			use_proxy_media: self.engine.read(cx).use_proxy_media(),
			// The 窗口 menu's checkmarks mirror the dock's live visible panel
			// set (docked or floating), so a panel closed via its tab ✕ (or
			// torn off) loses its checkmark on the next rebuild.
			open_panels: open_panels_mask(self.dock.read(cx), &self.panels, self.timeline_visible),
		};
		let Ok(menu_bar) = cx.update_window(*handle, |_root, window, app| {
			app.new(|cx| MenuBar::new(1, make_menus(state), window, cx))
		}) else {
			return;
		};
		self.menu_bar = menu_bar;
		let menu_bar = self.menu_bar.clone();
		cx.subscribe(
			&menu_bar,
			|this, _menu: Entity<MenuBar>, event: &MenuBarEvent, cx| {
				if let MenuBarEvent::Triggered { item, .. } = event {
					this.on_menu(*item, cx);
				}
			},
		)
		.detach();
	}

	// -----------------------------------------------------------------------
	// Project manager (M13 D4)
	// -----------------------------------------------------------------------
}

use super::*;

impl<E: AppEngine> OakApp<E> {
	/// The central action dispatch (menu clicks and keyboard shortcuts both
	/// end up here). [`Route::FocusedPanel`](crate::actions::Route::FocusedPanel)
	/// actions go to the focused panel first; whatever the panel does not
	/// implement falls through to the shell's global handler.
	pub(crate) fn dispatch_action_id(&mut self, action: ActionId, cx: &mut Context<Self>) {
		let entry = action.entry();
		if entry.route == crate::actions::Route::FocusedPanel
			&& self.dispatch_to_focused_panel(action, cx)
		{
			return;
		}
		self.handle_global_action(action, cx);
	}

	/// Hands `action` to the focused panel's
	/// [`PanelCommandHandler`](panel_commands::PanelCommandHandler); whether
	/// it was handled. No focused panel (or the panel declines) means the
	/// shell's global handler runs instead.
	pub(crate) fn dispatch_to_focused_panel(
		&mut self,
		action: ActionId,
		cx: &mut Context<Self>,
	) -> bool {
		let Some(id) = self.focused_panel else {
			return false;
		};
		match id {
			PROJECT => self.panels.project.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			SOURCE_VIEWER => self.panels.source_viewer.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			PROGRAM_VIEWER => self.panels.program_viewer.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			NODE_EDITOR => self.panels.node_editor.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			INSPECTOR => self.panels.inspector.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			HISTORY => self.panels.history.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			TIMELINE => self.panels.timeline.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			EFFECT_LIBRARY => self.panels.effect_library.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			MULTICAM => self.panels.multicam.update(cx, |panel, cx| {
				panel_commands::dispatch_to(panel, action, cx)
			}),
			_ => false,
		}
	}

	/// The shell's global action handler: file dialogs, undo, view
	/// preferences, tools, transport fallbacks — and the placeholder
	/// `println!` for the actions not wired yet.
	pub(crate) fn handle_global_action(&mut self, action: ActionId, cx: &mut Context<Self>) {
		use crate::actions::ActionId as A;
		match action {
			// --- File ------------------------------------------------------
			A::NewProject => self.open_new_project(cx),
			A::OpenProject => self.open_file_dialog(FileAction::Open, cx),
			A::OpenFromLibrary | A::ProjectManager => self.show_project_manager(cx),
			A::Import => self.open_file_dialog(FileAction::ImportFootage, cx),
			A::ImportSubtitles => self.open_file_dialog(FileAction::ImportSubtitles, cx),
			A::ExportSubtitles => self.open_file_dialog(FileAction::ExportSubtitles, cx),
			A::AudioGain50 => self.edit_selected_audio(AudioEdit::Gain(0.5), cx),
			A::AudioGain100 => self.edit_selected_audio(AudioEdit::Gain(1.0), cx),
			A::AudioGain200 => self.edit_selected_audio(AudioEdit::Gain(2.0), cx),
			A::AudioPanLeft => self.edit_selected_audio(AudioEdit::Pan(-1.0), cx),
			A::AudioPanCenter => self.edit_selected_audio(AudioEdit::Pan(0.0), cx),
			A::AudioPanRight => self.edit_selected_audio(AudioEdit::Pan(1.0), cx),
			A::AudioFadeIn => self.edit_selected_audio(AudioEdit::FadeIn, cx),
			A::AudioFadeOut => self.edit_selected_audio(AudioEdit::FadeOut, cx),
			A::AudioAddVolume => self.edit_selected_audio(AudioEdit::AddVolume, cx),
			A::AudioAddPan => self.edit_selected_audio(AudioEdit::AddPan, cx),
			// The 导出工程文件… dialog: choose OTIO / OVE / FCPXML + path.
			A::SaveProject => self.open_export_project_dfd(cx),
			A::SaveProjectAs => self.open_save_as(cx),
			A::CloseProject => self
				.engine
				.update(cx, |engine, cx| engine.close_project(cx)),
			A::Export => self.open_export_dialog(None, cx),
			A::Exit => {
				crate::oakui::ofx::shutdown();
				cx.quit();
			}
			// --- Edit ------------------------------------------------------
			A::Undo => self.engine.update(cx, |engine, cx| engine.undo(cx)),
			A::Redo => self.engine.update(cx, |engine, cx| engine.redo(cx)),
			A::Delete => self.delete_timeline_selection(false, cx),
			A::RippleDelete => self.delete_timeline_selection(true, cx),
			A::SelectAll => self.select_all_clips(cx),
			A::DeselectAll => self.deselect_all_clips(cx),
			A::SplitAtPlayhead => self
				.engine
				.update(cx, |engine, cx| engine.split_at_playhead(cx)),
			A::SetInPoint => self.set_point_at_playhead(true, cx),
			A::SetOutPoint => self.set_point_at_playhead(false, cx),
			A::ClearInOut | A::ClearWorkArea => self
				.engine
				.update(cx, |engine, cx| engine.clear_workarea(cx)),
			A::Marker => self
				.engine
				.update(cx, |engine, cx| engine.add_marker_at_playhead(cx)),
			// --- View ------------------------------------------------------
			A::ThemeDark => {
				crate::oakui::real::set_theme_dark(true);
				self.apply_dark(true, cx);
			}
			A::ThemeLight => {
				crate::oakui::real::set_theme_dark(false);
				self.apply_dark(false, cx);
			}
			A::ZoomIn => self.zoom_timeline(1.25, cx),
			A::ZoomOut => self.zoom_timeline(0.8, cx),
			A::IncreaseTrackHeight => self.nudge_track_height(8.0, cx),
			A::DecreaseTrackHeight => self.nudge_track_height(-8.0, cx),
			A::ToggleShowAll => {
				// 视图 → 显示全部: zoom the timeline strip so the whole
				// sequence fits (the widget tracks its clip-area width).
				let timeline = self.panels.timeline.clone();
				timeline.update(cx, |panel, cx| {
					crate::panels::commands::PanelCommandHandler::toggle_show_all(panel, cx)
				});
			}
			A::FullScreen => self.toggle_full_screen(cx),
			A::Preferences => self.open_preferences(cx),
			// --- Playback (the program monitor) ----------------------------
			A::PlayPause => {
				let playing = self.program_clock.read(cx).is_playing();
				let monitor = Monitor::Program;
				self.engine.update(cx, |engine, cx| {
					if playing {
						engine.pause(monitor, cx);
					} else {
						engine.play(monitor, cx);
					}
				});
			}
			A::PrevFrame => {
				let monitor = Monitor::Program;
				self.engine
					.update(cx, |engine, cx| engine.step(monitor, -1, cx));
			}
			A::NextFrame => {
				let monitor = Monitor::Program;
				self.engine
					.update(cx, |engine, cx| engine.step(monitor, 1, cx));
			}
			A::ShuttleLeft => {
				// J steps back — true reverse playback is an engine transport
				// gap (the old shortcut table did the same).
				let monitor = Monitor::Program;
				self.engine
					.update(cx, |engine, cx| engine.step(monitor, -1, cx));
			}
			A::ShuttleStop => {
				let monitor = Monitor::Program;
				self.engine
					.update(cx, |engine, cx| engine.pause(monitor, cx));
			}
			A::ShuttleRight => {
				let monitor = Monitor::Program;
				self.engine
					.update(cx, |engine, cx| engine.play(monitor, cx));
			}
			A::GoToStart => {
				let monitor = Monitor::Program;
				self.engine.update(cx, |engine, cx| {
					engine.request_frame(monitor, Frame::ZERO, cx)
				});
			}
			A::GoToEnd => {
				let monitor = Monitor::Program;
				let length = self.engine.read(cx).sequence_length();
				self.engine
					.update(cx, |engine, cx| engine.request_frame(monitor, length, cx));
			}
			A::GoToIn => {
				if let Some((start, _)) = self.engine.read(cx).workarea() {
					let monitor = Monitor::Program;
					self.engine
						.update(cx, |engine, cx| engine.request_frame(monitor, start, cx));
				}
			}
			A::GoToOut => {
				if let Some((_, end)) = self.engine.read(cx).workarea() {
					let monitor = Monitor::Program;
					self.engine
						.update(cx, |engine, cx| engine.request_frame(monitor, end, cx));
				}
			}
			A::PlayInToOut => {
				// Seek to the work area's start, then play (out-point
				// stopping is a transport gap).
				let monitor = Monitor::Program;
				if let Some((start, _)) = self.engine.read(cx).workarea() {
					self.engine
						.update(cx, |engine, cx| engine.request_frame(monitor, start, cx));
				}
				self.engine
					.update(cx, |engine, cx| engine.play(monitor, cx));
			}
			A::Loop => {
				self.loop_playback = !self.loop_playback;
				println!("[playback] loop: {} (placeholder)", self.loop_playback);
				self.rebuild_menu_bar(cx);
			}
			// --- Sequence --------------------------------------------------
			A::AddVideoTrack => {
				let kind = crate::widgets::timeline::TrackKind::Video;
				self.engine
					.update(cx, |engine, cx| engine.add_track(kind, cx));
			}
			A::AddAudioTrack => {
				let kind = crate::widgets::timeline::TrackKind::Audio;
				self.engine
					.update(cx, |engine, cx| engine.add_track(kind, cx));
			}
			A::RemoveTrack => self.remove_selected_track(cx),
			A::RemoveMarker => self
				.engine
				.update(cx, |engine, cx| engine.remove_marker_at_playhead(cx)),
			A::SetWorkArea => self.set_workarea_from_selection(cx),
			// --- Window ----------------------------------------------------
			// The 窗口 menu toggles each panel: clicking an open panel closes
			// it (through the dock's remove flow, the same path the tab ✕
			// takes), clicking a closed one re-opens it.
			A::FocusProject => self.toggle_panel(PROJECT, cx),
			A::FocusSourceViewer => self.toggle_panel(SOURCE_VIEWER, cx),
			A::FocusProgramViewer => self.toggle_panel(PROGRAM_VIEWER, cx),
			A::FocusNodeEditor => self.toggle_panel(NODE_EDITOR, cx),
			A::FocusInspector => self.toggle_panel(INSPECTOR, cx),
			A::FocusHistory => self.toggle_panel(HISTORY, cx),
			A::FocusTimeline => self.toggle_panel(TIMELINE, cx),
			A::FocusEffectLibrary => self.toggle_panel(EFFECT_LIBRARY, cx),
			A::FocusMulticam => self.toggle_panel(MULTICAM, cx),
			A::FocusAiAssistant => self.toggle_panel(AI_ASSISTANT, cx),
			A::ResetDefaultLayout => self.reset_default_layout(cx),
			// --- Tools -----------------------------------------------------
			A::Snapping => {
				let enabled = !self.timeline.read(cx).state.snap_enabled;
				self.timeline.update(cx, |timeline, cx| {
					timeline.state.snap_enabled = enabled;
					cx.notify();
				});
				println!("[tools] snapping: {enabled}");
				self.rebuild_menu_bar(cx);
			}
			tool_action if Tool::from_action(tool_action).is_some() => {
				let tool = Tool::from_action(tool_action).unwrap();
				self.active_tool = tool;
				println!("[tools] selected: {tool:?}");
				// Push the app tool into the timeline widget when it models
				// one; unmodeled tools (edit/transition/add/record/hand) keep
				// the widget's current tool.
				if let Some(tt) = tool.timeline_tool() {
					self.timeline.update(cx, |timeline, cx| {
						timeline.set_tool(tt, cx);
					});
				}
				self.rebuild_menu_bar(cx);
			}
			// --- Proxy (Tools) ---------------------------------------------
			A::UseProxyMedia => {
				let enabled = !self.engine.read(cx).use_proxy_media();
				self.engine
					.update(cx, |engine, cx| engine.set_use_proxy_media(enabled, cx));
				self.rebuild_menu_bar(cx);
			}
			A::ProxySettings => self.open_proxy_dialog(cx),
			A::ProjectProperties => self.open_project_properties(cx),
			A::MulticamWizard => self.open_multicam_wizard(cx),
			A::SubtitleProperties => self.open_subtitle_properties(cx),
			A::AudioProperties => self.open_audio_properties(cx),
			A::NewSequence => self.open_new_sequence(cx),
			A::NewFolder => {
				if let Err(err) = self
					.engine
					.update(cx, |engine, cx| engine.create_folder(String::new(), cx))
				{
					println!("[sequence] new folder: {err}");
				}
			}
			// The multicam source-switch hotkeys are scoped to the Multicam
			// panel (the focused-panel route handles them there); a fall-through
			// from any other focused panel is a silent no-op.
			A::MulticamSwitch1
			| A::MulticamSwitch2
			| A::MulticamSwitch3
			| A::MulticamSwitch4
			| A::MulticamSwitch5
			| A::MulticamSwitch6
			| A::MulticamSwitch7
			| A::MulticamSwitch8
			| A::MulticamSwitch9
			| A::MulticamSwitchNoSplit1
			| A::MulticamSwitchNoSplit2
			| A::MulticamSwitchNoSplit3
			| A::MulticamSwitchNoSplit4
			| A::MulticamSwitchNoSplit5
			| A::MulticamSwitchNoSplit6
			| A::MulticamSwitchNoSplit7
			| A::MulticamSwitchNoSplit8
			| A::MulticamSwitchNoSplit9 => {}
			// --- Help --------------------------------------------------------
			A::ActionSearch => self.open_action_search(cx),
			A::BugReport => cx.open_url(crate::update::BUG_REPORT_URL),
			A::About => self.open_about(cx),
			// --- everything else is a placeholder --------------------------
			other => println!(
				"[action] {} not wired yet (placeholder)",
				other.entry().cpp_id
			),
		}
	}

	/// 新建项目: creates a blank project in the library and opens it (the
	/// write-through persists it from the first edit). Falls back to the
	/// engine's plain new-project path when the library is unavailable.
	pub(crate) fn open_new_project(&mut self, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::dialogs::NewProjectContent::new(window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::NEW_PROJECT,
					ModalOptions::new(crate::i18n::tr("menu.file.new_project"), px(480.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::new(
							crate::i18n::tr("dialog.cancel"),
							crate::widgets::dialog::DialogButtonRole::Secondary,
						)),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::NewProject { modal, content }
		});
	}

	/// Create the project described by the new-project dialog, then seed its
	/// first sequence.
	///
	/// `content` is the dialog state: it supplies the project `name`, the
	/// `format` (sequence preset) and the `interlaced` flag. `cx` is the
	/// entity context. Two paths:
	/// - `library_create_project(&name)` succeeds → the dialog closes and a
	///   first sequence is created via `create_sequence_with_params` using the
	///   localized default name `manager.new.default_name` (not the project
	///   name), the chosen format and the interlace flag; that call's error is
	///   discarded.
	/// - the library create fails → the error is printed
	///   (`[file] library create failed (…); plain new project`), the dialog
	///   closes anyway and the engine's plain `new_project` runs as the
	///   fallback path (a project with no sequence from this method).
	///
	/// Either way the dialog never stays open: there is no in-dialog error
	/// reporting for a failed create.
	pub(crate) fn commit_new_project(
		&mut self,
		content: &Entity<crate::dialogs::NewProjectContent>,
		cx: &mut Context<Self>,
	) {
		let name = content.read(cx).name(cx).to_string();
		let format = content.read(cx).format(cx);
		let interlaced = content.read(cx).interlaced(cx);
		let engine = self.engine.clone();
		let result = engine.update(cx, |engine, cx| engine.library_create_project(&name, cx));
		match result {
			Ok(()) => {
				self.close_modal(cx);
				let engine = self.engine.clone();
				engine.update(cx, |engine, cx| {
					let _ = engine.create_sequence_with_params(
						crate::i18n::tr("manager.new.default_name").to_string(),
						format,
						interlaced,
						cx,
					);
				});
			}
			Err(err) => {
				println!("[file] library create failed ({err}); plain new project");
				self.close_modal(cx);
				self.engine.update(cx, |engine, cx| engine.new_project(cx));
			}
		}
	}

	/// Deletes the timeline's selected clips (ripple or gap) through the
	/// engine's edit commands.
	pub(crate) fn delete_timeline_selection(&mut self, ripple: bool, cx: &mut Context<Self>) {
		let ids: Vec<crate::widgets::timeline::ClipId> =
			self.timeline.read(cx).selection().iter().copied().collect();
		if ids.is_empty() {
			println!("[timeline] delete: nothing selected");
			return;
		}
		for id in ids {
			self.engine
				.update(cx, |engine, cx| engine.delete_clip(id, ripple, cx));
		}
	}

	/// 编辑 → 全选: selects every timeline clip and forwards the selection
	/// to the engine (the effect stack's target), mirroring what the
	/// timeline's own `SelectionChanged` subscription does.
	pub(crate) fn select_all_clips(&mut self, cx: &mut Context<Self>) {
		let ids: Vec<ClipId> = {
			let engine = self.engine.read(cx);
			let mut ids = Vec::new();
			for index in 0..engine.track_count() {
				if let Some(track) = engine.track(index) {
					ids.extend(track.clips().iter().map(|clip| clip.id()));
				}
			}
			ids
		};
		self.timeline.update(cx, |view, cx| {
			view.state.select_range(ids.iter().copied());
			cx.notify();
		});
		self.engine
			.update(cx, |engine, cx| engine.set_selected_clips(ids, cx));
	}

	/// 编辑 → 取消全选: clears the timeline selection and tells the engine
	/// (the effect stack's target follows).
	pub(crate) fn deselect_all_clips(&mut self, cx: &mut Context<Self>) {
		self.timeline.update(cx, |view, cx| {
			view.state.select_range(std::iter::empty::<ClipId>());
			cx.notify();
		});
		self.engine
			.update(cx, |engine, cx| engine.set_selected_clips(Vec::new(), cx));
	}

	/// 视图 → 放大/缩小: scale the timeline zoom by `factor` around its left
	/// edge.
	///
	/// `factor` is a multiplier applied to the current zoom (`> 1` zooms in,
	/// `< 1` zooms out; the menu uses fixed steps), and `cx` is the entity
	/// context. The widget clamps the result to its zoom range
	/// (`TimelineState::set_zoom`), so extreme factors simply saturate at the
	/// limit; the anchor `px(0.)` keeps the view's left edge fixed while the
	/// content scales. Repaints the timeline view. A non-finite or negative
	/// factor is passed through to the clamp rather than being rejected here.
	pub(crate) fn zoom_timeline(&mut self, factor: f32, cx: &mut Context<Self>) {
		self.timeline.update(cx, |view, cx| {
			let zoom = view.state.zoom * factor;
			view.state.set_zoom(zoom, px(0.));
			cx.notify();
		});
	}

	/// 视图 → 轨道高度 ±: steps every track's height by `delta` pixels,
	/// clamped to the track-height slider's range (24–160px).
	pub(crate) fn nudge_track_height(&mut self, delta: f32, cx: &mut Context<Self>) {
		let current = self
			.engine
			.read(cx)
			.track(0)
			.map(|track| f32::from(track.height()))
			.unwrap_or(64.0);
		let next = (current + delta).clamp(24.0, 160.0);
		self.engine
			.update(cx, |engine, cx| engine.set_track_height(px(next), cx));
	}

	/// 回放 → 设置入点/出点: moves the work area's start (`in_point`) or end
	/// to the program playhead as ONE undoable entry (the same commit the
	/// ruler drag and 序列 → 设置工作区 use). Without an existing work area
	/// the far edge falls back to the sequence bounds.
	pub(crate) fn set_point_at_playhead(&mut self, in_point: bool, cx: &mut Context<Self>) {
		let playhead = self.program_clock.read(cx).current_frame();
		let seq_len = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|s| s.length)
			.unwrap_or(Frame(playhead.0 + 1));
		let (old_start, old_end) = self
			.engine
			.read(cx)
			.workarea()
			.unwrap_or((Frame::ZERO, seq_len));
		let (start, end) = if in_point {
			(playhead, old_end.max(Frame(playhead.0 + 1)))
		} else {
			(old_start.min(Frame((playhead.0 - 1).max(0))), playhead)
		};
		if end.0 <= start.0 {
			println!("[playback] set in/out point: empty range, ignored");
			return;
		}
		self.engine.update(cx, |engine, cx| {
			engine.commit_workarea(old_start, old_end, start, end, cx);
		});
	}

	/// Applies the dark/light theme and rebuilds the menu bar (the theme
	/// checkmark moves). The caller persists the choice through
	/// [`crate::oakui::real::set_theme_dark`].
	pub(crate) fn apply_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
		self.dark = dark;
		if dark {
			apply_theme(cx, &OakTheme::olive_dark());
		} else {
			apply_theme(cx, &OakTheme::olive_light());
		}
		self.rebuild_menu_bar(cx);
		cx.notify();
	}

	/// The shell's keyboard entry point: the global key bindings dispatch
	/// gpui actions, which bubble up to the root's action listeners and land
	/// here — the same [`Self::dispatch_action_id`] path the menu clicks
	/// take. While a modal dialog is open the shell stays keyboard-quiet, so
	/// the dialogs' text fields never trigger editing actions.
	pub(crate) fn on_action_dispatched(&mut self, action: ActionId, cx: &mut Context<Self>) {
		if !matches!(self.modal, ModalState::None) {
			return;
		}
		self.dispatch_action_id(action, cx);
	}

	/// Remove the first track selected in the timeline header.
	///
	/// `cx` is the entity context. Reads the timeline's selected track indices,
	/// takes the lowest one and calls the engine's `remove_track(index)`;
	/// any further selected tracks are left alone (one track per invocation —
	/// the menu action is the repeated unit). With no selected track it prints
	/// `[timeline] remove track: nothing selected` and does nothing. The
	/// engine decides what happens to the clips on the removed track (and
	/// whether the removal is undoable); this method ignores its result.
	pub(crate) fn remove_selected_track(&mut self, cx: &mut Context<Self>) {
		let Some(&index) = self.timeline.read(cx).selected_tracks().iter().next() else {
			println!("[timeline] remove track: nothing selected");
			return;
		};
		self.engine
			.update(cx, |engine, cx| engine.remove_track(index, cx));
	}

	/// 序列 → 设置工作区: sets the work area to the bounding range of the
	/// selected clips, or one frame at the program playhead when nothing is
	/// selected. Committed as ONE undoable entry whose old side is the
	/// engine's current work area (mirrors the ruler drag's commit).
	pub(crate) fn set_workarea_from_selection(&mut self, cx: &mut Context<Self>) {
		let (old_start, old_end) = self
			.engine
			.read(cx)
			.workarea()
			.unwrap_or((Frame::ZERO, Frame::ZERO));
		let (start, end) = self.selection_workarea_range(cx);
		self.engine.update(cx, |engine, cx| {
			engine.commit_workarea(old_start, old_end, start, end, cx);
		});
	}

	/// The bounding range of the timeline's selected clips; `[playhead,
	/// playhead + 1)` when nothing is selected (the menu's fallback).
	pub(crate) fn selection_workarea_range(&self, cx: &App) -> (Frame, Frame) {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		let engine = self.engine.read(cx);
		let mut start: Option<i64> = None;
		let mut end: i64 = 0;
		for index in 0..engine.track_count() {
			if let Some(track) = engine.track(index) {
				for clip in track.clips() {
					if ids.contains(&clip.id()) {
						let range = clip.range();
						start = Some(start.map_or(range.start.0, |s| s.min(range.start.0)));
						end = end.max(range.end.0);
					}
				}
			}
		}
		match start {
			Some(s) if end > s => (Frame(s), Frame(end)),
			_ => {
				let playhead = self.program_clock.read(cx).current_frame();
				(playhead, Frame(playhead.0 + 1))
			}
		}
	}
}

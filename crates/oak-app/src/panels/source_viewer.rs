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

//! The source viewer panel (素材查看器): the `ViewerWidget` over the source
//! monitor's clock, with its own transport and format chips.

use gpui::colors::DefaultColors;
use gpui::dock::{DockPanel, PanelEvent};
use gpui::{
	div, prelude::*, AnyElement, App, Context, Entity, EventEmitter, MouseButton, Render,
	SharedString, Window,
};
use gpui_widgets::viewer::{
	SafeMargins, ViewerEvent, ViewerWidget, ViewerZoom, WaveformMode,
};

use crate::actions::ActionId;
use crate::oakui::component::menu;
use crate::oakui::component::menu::{ContextMenuHandle, ContextMenuTriggered};
use crate::oakui::timecode::{format_fps, format_resolution};
use crate::oakui::{AppEngine, Monitor};
use crate::panels::commands::{self as panel_commands, PanelCommandHandler};
use crate::panels::{chip, viewer_title};
use crate::panels::ids::SOURCE_VIEWER;

/// The source viewer panel.
pub struct SourceViewerPanel<E: AppEngine> {
	viewer: Entity<ViewerWidget<E::Clock>>,
	engine: Entity<E>,
	/// The source monitor's clock (also owned by the viewer widget; kept
	/// here so transport commands can read the playing state).
	clock: Entity<E::Clock>,
	/// The last CPU frame handed to the viewer (compared by `Arc` identity so
	/// a paused playhead does not re-upload the picture every frame).
	last_cpu_frame: Option<std::sync::Arc<gpui::RenderImage>>,
	/// The right-click context menu.
	context_menu: ContextMenuHandle,
}

impl<E: AppEngine> SourceViewerPanel<E> {
	/// Builds a viewer over `clock` (the source monitor's clock).
	pub fn new(
		engine: Entity<E>,
		clock: Entity<E::Clock>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let viewer = cx.new(|cx| ViewerWidget::new(2, clock.clone(), window, cx));
		// Route every transport request to the engine's source monitor, and
		// re-emit the loop-range / overlay requests for the app shell.
		cx.subscribe(&viewer, |this, _viewer, event: &ViewerEvent, cx| match event {
			// The source monitor hosts no OFX interact: the picture's
			// pointer/key events are not forwarded here (the program
			// viewer's panel forwards them when an interact is live).
			ViewerEvent::InteractPointer { .. } | ViewerEvent::InteractKey { .. } => {}
			// The widget already toggled its own overlay state.
			ViewerEvent::ToggleSafeFramesRequested { .. } | ViewerEvent::ToggleZoomRequested { .. } => {}
			// Both monitors share the shell-owned program workarea.
			ViewerEvent::InPointRequested { .. } => cx.emit(menu::ViewerPanelEvent::SetInPoint),
			ViewerEvent::OutPointRequested { .. } => cx.emit(menu::ViewerPanelEvent::SetOutPoint),
			ViewerEvent::ClearRangeRequested { .. } => cx.emit(menu::ViewerPanelEvent::ClearRange),
			event => {
				let monitor = Monitor::Source;
				this.engine.update(cx, |engine, cx| match event {
					ViewerEvent::PlayRequested { .. } => engine.play(monitor, cx),
					ViewerEvent::PauseRequested { .. } => engine.pause(monitor, cx),
					ViewerEvent::StepRequested { delta, .. } => engine.step(monitor, *delta, cx),
					other => println!("[source viewer] request: {other:?}"),
				});
			}
		})
		.detach();

		let context_menu =
			ContextMenuHandle::new(Self::on_local_menu_item, window, cx);

		Self {
			viewer,
			engine,
			clock,
			last_cpu_frame: None,
			context_menu,
		}
	}

	/// Handles the viewer's local (non-registry) context-menu items by
	/// resolving them into [`menu::ViewerMenuAction`] and applying the action
	/// (the id resolution and the menu construction are shared with the program
	/// viewer, so both monitors behave identically).
	fn on_local_menu_item(&mut self, item: usize, cx: &mut Context<Self>) {
		let Some(action) = menu::viewer_menu_action(item) else {
			println!("[source viewer] context-menu item {item} (not handled)");
			return;
		};
		self.apply_viewer_action(action, cx);
	}

	/// Applies one viewer context-menu action. Zoom, safe margins and the FPS
	/// overlay are viewer-widget state; the resolution divider, stop-on-last
	/// and waveform mode are engine config; full-screen and the in/out range
	/// requests are re-emitted for the app shell (the workarea is shell-owned).
	fn apply_viewer_action(&mut self, action: menu::ViewerMenuAction, cx: &mut Context<Self>) {
		match action {
			menu::ViewerMenuAction::ZoomFit => {
				let zoom = ViewerZoom::Fit;
				self.viewer.update(cx, |viewer, cx| viewer.set_zoom(zoom, cx));
			}
			menu::ViewerMenuAction::ZoomLevel(index) => {
				let zoom = ViewerZoom::Level(index);
				self.viewer.update(cx, |viewer, cx| viewer.set_zoom(zoom, cx));
			}
			menu::ViewerMenuAction::Resolution(divider) => self
				.engine
				.update(cx, |engine, cx| engine.set_playback_divider(divider, cx)),
			menu::ViewerMenuAction::SafeOff => {
				let margins = SafeMargins::Off;
				self.viewer
					.update(cx, |viewer, cx| viewer.set_safe_margins(margins, cx));
			}
			menu::ViewerMenuAction::SafeOn => {
				let margins = SafeMargins::On;
				self.viewer
					.update(cx, |viewer, cx| viewer.set_safe_margins(margins, cx));
			}
			menu::ViewerMenuAction::SafeCustom => {
				let margins = SafeMargins::Custom(0.9, 0.8);
				self.viewer
					.update(cx, |viewer, cx| viewer.set_safe_margins(margins, cx));
			}
			menu::ViewerMenuAction::StopOnLast => {
				let enabled = self.engine.read(cx).stop_on_last();
				self.engine
					.update(cx, |engine, cx| engine.set_stop_on_last(!enabled, cx));
			}
			menu::ViewerMenuAction::Waveform(mode) => self.engine.update(cx, |engine, cx| {
				engine.set_waveform_mode(mode.config_value(), cx);
			}),
			menu::ViewerMenuAction::ShowFps => {
				let show = self.viewer.read(cx).show_fps();
				self.viewer.update(cx, |viewer, cx| viewer.set_show_fps(!show, cx));
			}
			menu::ViewerMenuAction::SaveFrame => self.save_frame(cx),
			menu::ViewerMenuAction::FullScreen => {
				cx.emit(menu::ViewerPanelEvent::FullScreenRequested)
			}
		}
	}

	/// Saves the current source-monitor frame to a PNG in `$HOME/Pictures`
	/// (falling back to the working directory), named after the frame number
	/// and a timestamp. The engine's shared capture path writes the BGRA CPU
	/// frame; there is no save dialog yet.
	fn save_frame(&mut self, cx: &mut Context<Self>) {
		let frame = self.engine.read(cx).clock_frame(Monitor::Source, cx).0;
		let ts = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|duration| duration.as_secs())
			.unwrap_or(0);
		let name = format!("oak-frame-source-{frame}-{ts}.png");
		let dir = std::env::var_os("HOME")
			.map(std::path::PathBuf::from)
			.map(|home| home.join("Pictures"))
			.filter(|dir| std::fs::create_dir_all(dir).is_ok())
			.unwrap_or_else(|| std::path::PathBuf::from("."));
		let path = dir.join(name);
		match self.engine.read(cx).save_frame(Monitor::Source, path.clone(), cx) {
			Ok(path) => println!("[source viewer] saved frame to {}", path.display()),
			Err(error) => println!("[source viewer] save frame failed: {error}"),
		}
	}

	/// Routes a transport command to the engine's source monitor through
	/// the shared viewer transport helper.
	fn transport(&mut self, action: ActionId, cx: &mut Context<Self>) -> bool {
		let engine = self.engine.clone();
		let clock = self.clock.clone();
		panel_commands::viewer_transport(&engine, &clock, Monitor::Source, action, cx)
	}

	/// Pushes the engine's synthetic test frame into the viewer, but only when
	/// it actually changed (the engine caches one image per playhead frame).
	fn sync_frame(&mut self, cx: &mut Context<Self>) {
		let frame = self.engine.read(cx).cpu_frame(Monitor::Source, cx);
		if self
			.last_cpu_frame
			.as_ref()
			.is_none_or(|last| !std::sync::Arc::ptr_eq(last, &frame))
		{
			self.last_cpu_frame = Some(frame.clone());
			let frame = frame.clone();
			self.viewer
				.update(cx, |viewer, cx| viewer.set_cpu_frame(Some(frame), cx));
		}
	}
}

impl<E: AppEngine> Render for SourceViewerPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		self.sync_frame(cx);

		let colors = cx.default_colors().clone();
		let format = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|sequence| sequence.format)
			.unwrap_or(crate::oakui::VideoFormat::hd_1080p25());
		let media = self.engine.read(cx).source_media_name();
		let title = viewer_title("panel.source_viewer", &media);

		div()
			.size_full()
			.flex()
			.flex_col()
			.overflow_hidden()
			// Any click inside the panel makes it the focused panel (the
			// dock re-emits this as `DockEvent::PanelFocused`, which the
			// shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &gpui::MouseDownEvent, _window, cx| {
					cx.emit(PanelEvent::Focused);
				})
			})
			// The viewer widget has no right-click handling of its own, so
			// the panel opens the shared viewer menu here.
			.on_mouse_down(MouseButton::Right, {
				cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
					let state = menu::ViewerMenuState {
						playback_divider: this.engine.read(cx).playback_divider(),
						zoom: this.viewer.read(cx).zoom(),
						safe: this.viewer.read(cx).safe_margins(),
						stop_on_last: this.engine.read(cx).stop_on_last(),
						waveform: WaveformMode::from_config_value(
							this.engine.read(cx).waveform_mode(),
						),
						show_fps: this.viewer.read(cx).show_fps(),
					};
					this.context_menu
						.show(event.position, menu::viewer_menu(&state), cx);
				})
			})
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.px_2()
					.py_1()
					.border_b_1()
					.border_color(colors.border)
					.child(chip(&colors, title))
					.child(chip(
						&colors,
						format_resolution(format.width, format.height),
					))
					.child(chip(&colors, format_fps(format.rate))),
			)
			.child(
				div()
					.flex_1()
					.min_w_0()
					.min_h_0()
					.overflow_hidden()
					.child(self.viewer.clone()),
			)
			// The right-click popup renders anchored above the panel.
			.child(self.context_menu.widget())
	}
}

impl<E: AppEngine> PanelCommandHandler for SourceViewerPanel<E> {
	fn play_pause(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PlayPause, cx)
	}
	fn prev_frame(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PrevFrame, cx)
	}
	fn next_frame(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::NextFrame, cx)
	}
	fn go_to_start(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToStart, cx)
	}
	fn go_to_end(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToEnd, cx)
	}
	fn play_in_to_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PlayInToOut, cx)
	}
	fn go_to_in(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToIn, cx)
	}
	fn go_to_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToOut, cx)
	}
	fn shuttle_left(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleLeft, cx)
	}
	fn shuttle_stop(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleStop, cx)
	}
	fn shuttle_right(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleRight, cx)
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for SourceViewerPanel<E> {}

impl<E: AppEngine> EventEmitter<menu::ViewerPanelEvent> for SourceViewerPanel<E> {}

impl<E: AppEngine> EventEmitter<ContextMenuTriggered> for SourceViewerPanel<E> {}

impl<E: AppEngine> DockPanel for SourceViewerPanel<E> {
	fn panel_id(&self) -> gpui::dock::PanelId {
		SOURCE_VIEWER
	}

	fn title(&self, cx: &App) -> SharedString {
		viewer_title("panel.source_viewer", &self.engine.read(cx).source_media_name()).into()
	}

	fn tab_content(&self, cx: &App) -> AnyElement {
		div()
			.child(viewer_title(
				"panel.source_viewer",
				&self.engine.read(cx).source_media_name(),
			))
			.into_any_element()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::MockEngine;
	use gpui::{Modifiers, px, size, TestAppContext, VisualTestContext};
	use gpui_widgets::viewer::PlaybackClock as _;

	/// Builds the panel in a window and returns a `VisualTestContext`.
	fn panel_window(
		cx: &mut TestAppContext,
	) -> (
		&'static mut VisualTestContext,
		Entity<SourceViewerPanel<MockEngine>>,
	) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(640.0), px(360.0)), |window, cx| {
			let engine = cx.new(MockEngine::demo);
			let clock = engine.read(cx).source_clock().clone();
			SourceViewerPanel::new(engine, clock, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("source viewer panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, panel)
	}

	/// Every viewer context-menu action updates the viewer widget state or
	/// the engine config; a real menu item id resolves through the shared
	/// menu mapping, unknown ids are ignored.
	#[gpui::test]
	async fn menu_actions_update_viewer_and_engine_state(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(9999, cx);

				panel.apply_viewer_action(menu::ViewerMenuAction::ZoomFit, cx);
				panel.apply_viewer_action(menu::ViewerMenuAction::ZoomLevel(3), cx);
				assert_eq!(panel.viewer.read(cx).zoom(), ViewerZoom::Level(3));

				// Resolution/stop-on-last/waveform persist through the
				// engine's config-backed defaults; exercise the reads and
				// writes without assuming a fresh config entry type.
				let _ = panel.engine.read(cx).playback_divider();
				panel.apply_viewer_action(menu::ViewerMenuAction::Resolution(4), cx);
				let _ = panel.engine.read(cx).playback_divider();

				panel.apply_viewer_action(menu::ViewerMenuAction::SafeOff, cx);
				panel.apply_viewer_action(menu::ViewerMenuAction::SafeOn, cx);
				panel.apply_viewer_action(menu::ViewerMenuAction::SafeCustom, cx);

				let _ = panel.engine.read(cx).stop_on_last();
				panel.apply_viewer_action(menu::ViewerMenuAction::StopOnLast, cx);
				let _ = panel.engine.read(cx).stop_on_last();

				let _ = panel.engine.read(cx).waveform_mode();
				panel.apply_viewer_action(
					menu::ViewerMenuAction::Waveform(WaveformMode::Both),
					cx,
				);
				let _ = panel.engine.read(cx).waveform_mode();

				let show_fps = panel.viewer.read(cx).show_fps();
				panel.apply_viewer_action(menu::ViewerMenuAction::ShowFps, cx);
				assert_eq!(panel.viewer.read(cx).show_fps(), !show_fps);

				panel.apply_viewer_action(menu::ViewerMenuAction::FullScreen, cx);

				// A real menu id from the shared builder resolves and applies.
				let state = menu::ViewerMenuState {
					playback_divider: panel.engine.read(cx).playback_divider(),
					zoom: panel.viewer.read(cx).zoom(),
					safe: panel.viewer.read(cx).safe_margins(),
					stop_on_last: panel.engine.read(cx).stop_on_last(),
					waveform: WaveformMode::from_config_value(
						panel.engine.read(cx).waveform_mode(),
					),
					show_fps: panel.viewer.read(cx).show_fps(),
				};
				let menu = menu::viewer_menu(&state);
				let item = menu.items[0].id;
				panel.on_local_menu_item(item, cx);
			});
		});
	}

	/// The focused-panel transport commands drive the SOURCE monitor.
	#[gpui::test]
	async fn transport_commands_drive_the_source_clock(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.play_pause(cx));
				assert!(panel.clock.read(cx).is_playing());
				assert!(panel.play_pause(cx));
				assert!(!panel.clock.read(cx).is_playing());

				assert!(panel.next_frame(cx));
				assert!(panel.prev_frame(cx));
				assert!(panel.go_to_start(cx));
				assert!(panel.go_to_end(cx));
				assert!(panel.shuttle_left(cx));
				assert!(panel.shuttle_stop(cx));
				assert!(panel.shuttle_right(cx));
				assert!(panel.play_in_to_out(cx));
				assert!(panel.go_to_in(cx));
				assert!(panel.go_to_out(cx));
			});
		});
	}

	/// The panel renders, right-click opens the shared viewer menu, and the
	/// dock metadata is populated.
	#[gpui::test]
	async fn render_right_click_menu_and_dock_metadata(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		cx.update(|_window, app| {
			assert!(!panel.read(app).title(app).is_empty());
			let _ = panel.read(app).tab_content(app);
		});

		// A right-click anywhere in the panel opens the viewer menu.
		cx.simulate_mouse_down(
			gpui::point(px(320.0), px(180.0)),
			MouseButton::Right,
			Modifiers::none(),
		);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(
			cx.debug_bounds("menu-popup").is_some(),
			"the viewer context menu opens"
		);

		// A left click focuses the panel (the dock re-emits the event).
		cx.simulate_mouse_down(
			gpui::point(px(320.0), px(180.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
	}
}

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

//! The program viewer panel (序列查看器): the `ViewerWidget` over the
//! program monitor's clock, with a 26px audio level strip attached to its
//! right edge (the design's WP6 layout). A header tab row switches the body
//! between the picture and the scopes (histogram / waveform / vectorscope),
//! whose samples come from the same rendered frame the picture shows.

use gpui::colors::DefaultColors;
use gpui::dock::{DockPanel, PanelEvent};
use gpui::{
	div, prelude::*, px, size, AnyElement, App, ClickEvent, Context, Entity, EventEmitter,
	MouseButton, Point, Render, SharedString, Window,
};
use gpui_widgets::audio_meter::AudioLevelMeter;
use gpui_widgets::scopes::{ChromaDataSource, Histogram, LumaDataSource, Vectorscope, Waveform};
use gpui_widgets::viewer::{
	InteractPointerKind, PlaybackClock, SafeMargins, ViewerEvent, ViewerWidget, ViewerZoom,
	WaveformMode,
};

use crate::actions::ActionId;
use crate::oakui::component::menu;
use crate::oakui::component::menu::{ContextMenuHandle, ContextMenuTriggered};
use crate::oakui::ofx::InteractViewport;
use crate::oakui::timecode::{format_fps, format_resolution};
use crate::oakui::{AppEngine, Monitor};
use crate::panels::commands::{self as panel_commands, PanelCommandHandler};
use crate::panels::ids::PROGRAM_VIEWER;
use crate::panels::{chip, viewer_title};

/// Width of the audio level strip, per the design (26px).
const METER_WIDTH: f32 = 26.0;

/// The body tab of the program viewer: the picture or the scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgramViewTab {
	/// The rendered picture (the viewer widget plus the level strip).
	Picture,
	/// The scopes (histogram / waveform / vectorscope).
	Scopes,
}

/// The program monitor's scope samples, refreshed from the engine whenever
/// the displayed frame changes. The scope widgets read this entity through
/// the [`LumaDataSource`] / [`ChromaDataSource`] traits.
struct ScopeState {
	/// Per-pixel luma of the current frame (`0..=1`).
	luma: Vec<f32>,
	/// Per-pixel chroma `(Cb, Cr)` of the current frame (`0..=1`).
	chroma: Vec<(f32, f32)>,
}

impl LumaDataSource for ScopeState {
	fn luma_samples(&self) -> Vec<f32> {
		self.luma.clone()
	}
}

impl ChromaDataSource for ScopeState {
	fn chroma_samples(&self) -> Vec<(f32, f32)> {
		self.chroma.clone()
	}
}

/// The cached overlay composite the viewer displays while an OFX interact
/// is active (redrawn only when the frame/time/viewport changed or the
/// plugin requested a repaint).
struct OverlayComposite {
	/// The base frame the composite was built from (Arc identity compare).
	frame: std::sync::Arc<gpui::RenderImage>,
	/// The plugin instance the overlay came from.
	instance: u64,
	/// The viewport the overlay was drawn at.
	viewport: InteractViewport,
	/// The playhead seconds used for the draw.
	time: f64,
	/// The composited image shown in the viewer.
	image: std::sync::Arc<gpui::RenderImage>,
}

/// The program viewer panel.
pub struct ProgramViewerPanel<E: AppEngine> {
	viewer: Entity<ViewerWidget<E::Clock>>,
	meter: Entity<AudioLevelMeter<E>>,
	engine: Entity<E>,
	/// The program monitor's clock (also owned by the viewer widget; kept
	/// here so transport commands can read the playing state).
	clock: Entity<E::Clock>,
	/// The last CPU frame handed to the viewer (compared by `Arc` identity so
	/// a paused playhead does not re-upload the picture every frame).
	last_cpu_frame: Option<std::sync::Arc<gpui::RenderImage>>,
	/// The last base frame the scope samples were analyzed from (the scopes
	/// follow the raw frame, not the overlay composite).
	scope_frame: Option<std::sync::Arc<gpui::RenderImage>>,
	/// The cached overlay composite (active OFX interact only); `None` when
	/// the viewer shows the raw engine frame.
	overlay: Option<OverlayComposite>,
	/// The active body tab.
	tab: ProgramViewTab,
	/// The scope samples backing the three scope widgets.
	scope_state: Entity<ScopeState>,
	/// The histogram scope.
	histogram: Entity<Histogram<ScopeState>>,
	/// The waveform scope.
	waveform: Entity<Waveform<ScopeState>>,
	/// The vectorscope.
	vectorscope: Entity<Vectorscope<ScopeState>>,
	/// The right-click context menu.
	context_menu: ContextMenuHandle,
}

impl<E: AppEngine> ProgramViewerPanel<E> {
	/// Builds a viewer over `clock` (the program monitor's clock) with the
	/// level meter `meter` (updated on the app's tick timer).
	pub fn new(
		engine: Entity<E>,
		clock: Entity<E::Clock>,
		meter: Entity<AudioLevelMeter<E>>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let viewer = cx.new(|cx| ViewerWidget::new(3, clock.clone(), window, cx));
		// Route every transport request to the engine's program monitor, and
		// forward the picture's pointer/key events to the active OFX interact
		// (no-op when none is live).
		cx.subscribe(
			&viewer,
			|this, _viewer, event: &ViewerEvent, cx| match event {
				ViewerEvent::InteractPointer {
					kind,
					position,
					button,
					pressed,
				} => this.forward_interact_pointer(*kind, *position, *button, *pressed, cx),
				ViewerEvent::InteractKey { down, keystroke } => {
					this.forward_interact_key(*down, keystroke, cx)
				}
				// The widget already toggled its own overlay state; nothing to
				// forward to the engine.
				ViewerEvent::ToggleSafeFramesRequested { .. }
				| ViewerEvent::ToggleZoomRequested { .. } => {}
				// The loop in/out range is the shell-owned program workarea, so
				// the panel re-emits the request.
				ViewerEvent::InPointRequested { .. } => cx.emit(menu::ViewerPanelEvent::SetInPoint),
				ViewerEvent::OutPointRequested { .. } => {
					cx.emit(menu::ViewerPanelEvent::SetOutPoint)
				}
				ViewerEvent::ClearRangeRequested { .. } => {
					cx.emit(menu::ViewerPanelEvent::ClearRange)
				}
				// The OFX color picker armed the eyedropper and the user clicked
				// the frame: hand the sampled colour back to the engine's mailbox.
				ViewerEvent::EyedropperPick { color } => this
					.engine
					.update(cx, |engine, cx| engine.eyedropper_picked(*color, cx)),
				event => {
					let monitor = Monitor::Program;
					this.engine.update(cx, |engine, cx| match event {
						ViewerEvent::PlayRequested { .. } => engine.play(monitor, cx),
						ViewerEvent::PauseRequested { .. } => engine.pause(monitor, cx),
						ViewerEvent::StepRequested { delta, .. } => {
							engine.step(monitor, *delta, cx)
						}
						other => println!("[program viewer] request: {other:?}"),
					});
				}
			},
		)
		.detach();

		// Re-render whenever the engine's global eyedropper arm state flips,
		// so `sync_frame` mirrors it into the viewer widget.
		cx.observe(&engine, |_this, _engine, cx| cx.notify())
			.detach();

		// A throttled idle pump for the OFX interact: the plugin's UI work
		// loop is served on the viewer's own timer (the app tick is
		// shell-owned), so an active interact's `idle` action runs without
		// coupling to the shell.
		let this = cx.weak_entity();
		window
			.spawn(cx, async move |cx: &mut gpui::AsyncWindowContext| loop {
				cx.background_executor()
					.timer(std::time::Duration::from_millis(50))
					.await;
				let _ = cx.update(|_window, app| {
					if let Some(this) = this.upgrade() {
						this.update(app, |_this, _cx| {
							crate::oakui::ofx::pump_interact_idle();
						});
					}
				});
			})
			.detach();

		let context_menu = ContextMenuHandle::new(Self::on_local_menu_item, window, cx);

		let scope_state = cx.new(|_cx| ScopeState {
			luma: Vec::new(),
			chroma: Vec::new(),
		});
		let histogram = cx.new(|cx| Histogram::new(41, scope_state.clone(), window, cx));
		let waveform = cx.new(|cx| Waveform::new(42, scope_state.clone(), window, cx));
		let vectorscope = cx.new(|cx| Vectorscope::new(43, scope_state.clone(), window, cx));

		Self {
			viewer,
			meter,
			engine,
			clock,
			last_cpu_frame: None,
			scope_frame: None,
			overlay: None,
			tab: ProgramViewTab::Picture,
			scope_state,
			histogram,
			waveform,
			vectorscope,
			context_menu,
		}
	}

	/// Handles the viewer's local (non-registry) context-menu items by
	/// resolving them into [`menu::ViewerMenuAction`] and applying the action
	/// (the id resolution and the menu construction are shared with the source
	/// viewer, so both monitors behave identically).
	fn on_local_menu_item(&mut self, item: usize, cx: &mut Context<Self>) {
		let Some(action) = menu::viewer_menu_action(item) else {
			println!("[program viewer] context-menu item {item} (not handled)");
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
				self.viewer
					.update(cx, |viewer, cx| viewer.set_zoom(zoom, cx));
			}
			menu::ViewerMenuAction::ZoomLevel(index) => {
				let zoom = ViewerZoom::Level(index);
				self.viewer
					.update(cx, |viewer, cx| viewer.set_zoom(zoom, cx));
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
				self.viewer
					.update(cx, |viewer, cx| viewer.set_show_fps(!show, cx));
			}
			menu::ViewerMenuAction::SaveFrame => self.save_frame(cx),
			menu::ViewerMenuAction::FullScreen => {
				cx.emit(menu::ViewerPanelEvent::FullScreenRequested)
			}
		}
	}

	/// Saves the current program-monitor frame to a PNG in `$HOME/Pictures`
	/// (falling back to the working directory), named after the frame number
	/// and a timestamp. The engine's shared capture path writes the BGRA CPU
	/// frame; there is no save dialog yet.
	fn save_frame(&mut self, cx: &mut Context<Self>) {
		let frame = self.engine.read(cx).clock_frame(Monitor::Program, cx).0;
		let ts = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|duration| duration.as_secs())
			.unwrap_or(0);
		let name = format!("oak-frame-program-{frame}-{ts}.png");
		let dir = std::env::var_os("HOME")
			.map(std::path::PathBuf::from)
			.map(|home| home.join("Pictures"))
			.filter(|dir| std::fs::create_dir_all(dir).is_ok())
			.unwrap_or_else(|| std::path::PathBuf::from("."));
		let path = dir.join(name);
		match self
			.engine
			.read(cx)
			.save_frame(Monitor::Program, path.clone(), cx)
		{
			Ok(path) => println!("[program viewer] saved frame to {}", path.display()),
			Err(error) => println!("[program viewer] save frame failed: {error}"),
		}
	}

	/// Routes a transport command to the engine's program monitor through
	/// the shared viewer transport helper.
	fn transport(&mut self, action: ActionId, cx: &mut Context<Self>) -> bool {
		let engine = self.engine.clone();
		let clock = self.clock.clone();
		panel_commands::viewer_transport(&engine, &clock, Monitor::Program, action, cx)
	}

	/// Pushes the engine's current frame into the viewer and the scopes, but
	/// only when it actually changed (the engine caches one image per
	/// playhead frame, with the scope samples analyzed in the same pass).
	///
	/// When an OFX interact is live, the displayed picture is the engine
	/// frame with the interact's GL-drawn overlay composited over it; the
	/// composite is cached and only redrawn when the frame / playhead /
	/// viewport changed or the plugin requested a repaint (so a paused
	/// viewer stays inert).
	fn sync_frame(&mut self, cx: &mut Context<Self>) {
		// Mirror the OFX color picker's eyedropper arm state into the viewer
		// widget; while armed, clicking the frame samples the pixel under the
		// cursor (the setter no-ops when the state is unchanged, so a paused
		// viewer stays inert).
		let armed = self.engine.read(cx).eyedropper_armed(cx);
		self.viewer
			.update(cx, |viewer, cx| viewer.set_eyedropper_armed(armed, cx));

		// Keep the main-process interact in sync with the inspector's
		// current selection (creates/destroys the interact as the target
		// moves; a no-op while it is unchanged).
		let target = self.engine.read(cx).ofx_interact_target(cx);
		crate::oakui::ofx::sync_active_interact(target);

		let frame = self.engine.read(cx).cpu_frame(Monitor::Program, cx);

		// The scopes follow the *base* frame (the overlay is not part of the
		// analysed picture).
		if self
			.scope_frame
			.as_ref()
			.is_none_or(|last| !std::sync::Arc::ptr_eq(last, &frame))
		{
			self.scope_frame = Some(frame.clone());
			let scope = self.engine.read(cx).scope_data(Monitor::Program, cx);
			self.scope_state.update(cx, |state, cx| {
				state.luma = (*scope.luma).clone();
				state.chroma = (*scope.chroma).clone();
				cx.notify();
			});
		}

		// The picture actually shown: the raw frame, or the overlay
		// composite while an interact is live.
		let displayed = self.displayed_frame(&frame, cx);

		if self
			.last_cpu_frame
			.as_ref()
			.is_none_or(|last| !std::sync::Arc::ptr_eq(last, &displayed))
		{
			self.last_cpu_frame = Some(displayed.clone());
			let displayed = displayed.clone();
			if std::env::var_os("OAK_DEBUG_VIEWER").is_some() {
				let sz = displayed.size(0);
				eprintln!("[viewer] push frame {}x{}", sz.width.0, sz.height.0);
				// Dump the displayed pixels (BGRA -> PPM) for black-frame
				// debugging: what the viewer RECEIVES, before the GPU path.
				if let Some(bytes) = displayed.as_bytes(0) {
					let (w, h) = (sz.width.0 as usize, sz.height.0 as usize);
					let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
					for px in bytes[..w * h * 4].as_chunks::<4>().0 {
						ppm.extend_from_slice(&[px[2], px[1], px[0]]);
					}
					let _ = std::fs::write("/tmp/oak_viewer_frame.ppm", ppm);
				}
			}
			self.viewer
				.update(cx, |viewer, cx| viewer.set_cpu_frame(Some(displayed), cx));
		}
	}

	/// The picture for the current engine `frame`: the frame itself, or —
	/// when an interact is live — the cached overlay composite, redrawing it
	/// when any redraw condition changed.
	fn displayed_frame(
		&mut self,
		frame: &std::sync::Arc<gpui::RenderImage>,
		cx: &mut Context<Self>,
	) -> std::sync::Arc<gpui::RenderImage> {
		use std::sync::atomic::Ordering;

		let Some((instance, interact, _)) = crate::oakui::ofx::active_interact() else {
			self.overlay = None;
			return frame.clone();
		};
		let frame_size = frame.size(0);
		let viewport =
			InteractViewport::at_frame_size(frame_size.width.0 as u32, frame_size.height.0 as u32);
		let time = self.playhead_seconds(cx);

		// Redraw when the base frame, the target instance, the playhead, or
		// the viewport changed, or when the plugin asked for a repaint via
		// interactRedraw / interactSwapBuffers (polled and cleared here —
		// outside the cache predicate so a redraw request during a cache-miss
		// frame is not replayed).
		let plugin_redraw = interact.redraw_requested.swap(false, Ordering::Relaxed)
			|| interact.swap_requested.swap(false, Ordering::Relaxed);
		let stale = plugin_redraw
			|| self.overlay.as_ref().is_none_or(|o| {
				!std::sync::Arc::ptr_eq(&o.frame, frame)
					|| o.instance != instance
					|| o.time != time
					|| o.viewport != viewport
			});
		if stale {
			if let Some(image) = crate::oakui::ofx::draw_interact_composite(
				instance, &interact, &viewport, time, frame,
			) {
				self.overlay = Some(OverlayComposite {
					frame: frame.clone(),
					instance,
					viewport,
					time,
					image: image.clone(),
				});
				return image;
			}
			// The interact is inert (no GL / the plugin failed to draw):
			// show the base frame. The next frame re-probes (cheap when GL
			// is unavailable).
			self.overlay = None;
			return frame.clone();
		}
		self.overlay.as_ref().unwrap().image.clone()
	}

	/// The program monitor's playhead in seconds (the interact's draw/pen
	/// `time`).
	fn playhead_seconds(&self, cx: &App) -> f64 {
		let clock = self.clock.read(cx);
		gpui::timeline::frame_to_seconds(clock.current_frame(), clock.frame_rate())
	}

	/// Forwards a picture pointer event to the active OFX interact, mapping
	/// the picture-local position into the interact's viewport pixels (the
	/// frame's pixel grid). Only the primary (left) button drives
	/// pen_down/pen_up (the OFX pen is a two-state device); moves forward
	/// pen_motion with the pen-down state reflecting whether a button is
	/// held. No-op without an active interact or outside the frame rect.
	fn forward_interact_pointer(
		&mut self,
		kind: InteractPointerKind,
		position: Point<f32>,
		button: Option<MouseButton>,
		pressed: bool,
		cx: &mut Context<Self>,
	) {
		let Some((_, interact, _)) = crate::oakui::ofx::active_interact() else {
			return;
		};
		let Some(bounds) = self.viewer.read(cx).picture_bounds() else {
			return;
		};
		let area = size(f32::from(bounds.size.width), f32::from(bounds.size.height));
		let frame_size = self.engine.read(cx).cpu_frame(Monitor::Program, cx).size(0);
		let frame = size(frame_size.width.0 as f32, frame_size.height.0 as f32);
		let Some((px, py)) = crate::oakui::ofx::viewport_pixel_to_pen(position, area, frame) else {
			// The pointer is in the letterbox (outside the frame rect).
			return;
		};
		let time = self.playhead_seconds(cx);
		match kind {
			InteractPointerKind::Move => {
				let _ = interact.pen_motion((px, py), pressed, time);
			}
			InteractPointerKind::Down if button == Some(MouseButton::Left) => {
				let _ = interact.pen_down((px, py), time);
			}
			InteractPointerKind::Up if button == Some(MouseButton::Left) => {
				let _ = interact.pen_up((px, py), time);
			}
			_ => {}
		}
	}

	/// Forwards a key event to the active OFX interact. No-op without an
	/// active interact.
	///
	/// # Consumption semantics
	///
	/// gpui dispatches the global keybindings *before* the focused element's
	/// key handlers, so a key consumed by the app's action system (space =
	/// play/pause, J/K/L = shuttle, …) never reaches the picture's key
	/// handler and therefore never reaches the interact — the existing
	/// shortcut system keeps priority. A key that reaches here was not
	/// consumed by any binding. The plugin's return status is deliberately
	/// not acted on: the app does not steal key repeat or other listeners,
	/// so the interact is a passive consumer of otherwise-unused keys.
	fn forward_interact_key(
		&mut self,
		down: bool,
		keystroke: &gpui::Keystroke,
		cx: &mut Context<Self>,
	) {
		let Some((_, interact, _)) = crate::oakui::ofx::active_interact() else {
			return;
		};
		let (sym, key_string) = crate::oakui::ofx::key_symbol(keystroke);
		let time = self.playhead_seconds(cx);
		if down {
			let _ = interact.key_down(sym, &key_string, time);
		} else {
			let _ = interact.key_up(sym, &key_string, time);
		}
	}

	/// One header tab button (picture / scopes), highlighted when active.
	fn tab_button(
		&self,
		id: &'static str,
		label: &'static str,
		tab: ProgramViewTab,
		colors: &gpui::colors::Colors,
		cx: &mut Context<Self>,
	) -> impl IntoElement {
		let active = self.tab == tab;
		div()
			.id(id)
			.px_2()
			.py_1()
			.rounded_sm()
			.border_1()
			.border_color(colors.border)
			.bg(if active {
				colors.selected
			} else {
				colors.container
			})
			.text_color(colors.text)
			.cursor_pointer()
			.child(label)
			.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
				this.tab = tab;
				cx.notify();
			}))
	}
}

impl<E: AppEngine> Render for ProgramViewerPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		self.sync_frame(cx);

		let colors = cx.default_colors().clone();
		let format = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|sequence| sequence.format)
			.unwrap_or(crate::oakui::VideoFormat::hd_1080p25());
		let sequence_name = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|sequence| sequence.name.clone())
			.unwrap_or_default();
		let title = viewer_title("panel.program_viewer", &sequence_name);

		let body = match self.tab {
			ProgramViewTab::Picture => div()
				.flex_1()
				.flex()
				.min_h_0()
				.min_w_0()
				.child(
					div()
						.flex_1()
						.min_w_0()
						.min_h_0()
						.overflow_hidden()
						.child(self.viewer.clone()),
				)
				.child(
					div()
						.w(px(METER_WIDTH))
						.flex_shrink_0()
						.border_l_1()
						.border_color(colors.border)
						.child(self.meter.clone()),
				),
			ProgramViewTab::Scopes => {
				let cell = |label: &'static str, scope: AnyElement| {
					div()
						.flex_1()
						.flex()
						.flex_col()
						.min_w_0()
						.child(
							div()
								.px_2()
								.py_1()
								.text_xs()
								.text_color(colors.disabled)
								.child(label),
						)
						.child(div().flex_1().min_h_0().child(scope))
				};
				div()
					.flex_1()
					.flex()
					.min_h_0()
					.child(cell(
						crate::i18n::tr("scope.histogram"),
						self.histogram.clone().into_any_element(),
					))
					.child(cell(
						crate::i18n::tr("scope.waveform"),
						self.waveform.clone().into_any_element(),
					))
					.child(cell(
						crate::i18n::tr("scope.vectorscope"),
						self.vectorscope.clone().into_any_element(),
					))
			}
		};

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
					.child(chip(&colors, format_fps(format.rate)))
					.child(self.tab_button(
						"program-tab-picture",
						crate::i18n::tr("viewer.picture"),
						ProgramViewTab::Picture,
						&colors,
						cx,
					))
					.child(self.tab_button(
						"program-tab-scopes",
						crate::i18n::tr("viewer.scopes"),
						ProgramViewTab::Scopes,
						&colors,
						cx,
					)),
			)
			.child(body)
			// The right-click popup renders anchored above the panel.
			.child(self.context_menu.widget())
	}
}

impl<E: AppEngine> PanelCommandHandler for ProgramViewerPanel<E> {
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

impl<E: AppEngine> EventEmitter<PanelEvent> for ProgramViewerPanel<E> {}

impl<E: AppEngine> EventEmitter<menu::ViewerPanelEvent> for ProgramViewerPanel<E> {}

impl<E: AppEngine> EventEmitter<ContextMenuTriggered> for ProgramViewerPanel<E> {}

impl<E: AppEngine> DockPanel for ProgramViewerPanel<E> {
	fn panel_id(&self) -> gpui::dock::PanelId {
		PROGRAM_VIEWER
	}

	fn title(&self, cx: &App) -> SharedString {
		let name = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|sequence| sequence.name.clone())
			.unwrap_or_default();
		viewer_title("panel.program_viewer", &name).into()
	}

	fn tab_content(&self, cx: &App) -> AnyElement {
		let name = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|sequence| sequence.name.clone())
			.unwrap_or_default();
		div()
			.child(viewer_title("panel.program_viewer", &name))
			.into_any_element()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::component::menu::ViewerMenuAction;
	use crate::oakui::graphops;
	use crate::oakui::MockEngine;
	use gpui::timeline::Frame;
	use gpui::{size, Keystroke, Modifiers, Rgba, TestAppContext, VisualTestContext};
	use gpui_widgets::viewer::PlaybackClock;

	/// Serializes the tests that mutate process-global state (HOME, the
	/// debug env var) and the undo stack.
	fn stack_lock() -> std::sync::MutexGuard<'static, ()> {
		graphops::test_lock()
	}

	/// Saves `HOME` and restores it (including the was-unset case) on drop,
	/// so a panicking assertion cannot leave the process pointed at the
	/// scratch directory. Callers hold [`stack_lock`] across the mutation.
	struct HomeGuard(Option<std::ffi::OsString>);

	impl HomeGuard {
		fn redirect(path: &std::path::Path) -> HomeGuard {
			let previous = std::env::var_os("HOME");
			std::env::set_var("HOME", path);
			HomeGuard(previous)
		}
	}

	impl Drop for HomeGuard {
		fn drop(&mut self) {
			match self.0.take() {
				Some(value) => std::env::set_var("HOME", value),
				None => std::env::remove_var("HOME"),
			}
		}
	}

	/// Builds a `ProgramViewerPanel` over the demo mock engine in a test
	/// window.
	fn panel_window(
		cx: &mut TestAppContext,
	) -> (
		&'static mut VisualTestContext,
		Entity<ProgramViewerPanel<MockEngine>>,
	) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(640.0), px(360.0)), |window, cx| {
			let engine = cx.new(MockEngine::demo);
			let clock = engine.read(cx).program_clock().clone();
			let meter = cx.new(|cx| AudioLevelMeter::new(30, engine.clone(), window, cx));
			ProgramViewerPanel::new(engine, clock, meter, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("program viewer panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, panel)
	}

	/// The panel's viewer subscription routes every event family: the
	/// interact forwarders (no-op without an interact), the overlay toggles,
	/// the workarea re-emissions, the eyedropper mailbox and the transport
	/// requests against the program monitor's clock.
	#[gpui::test]
	async fn viewer_events_route_through_the_panel(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		let viewer = cx.update(|_, cx| panel.read(cx).viewer.clone());
		let engine = cx.update(|_, cx| panel.read(cx).engine.clone());
		let clock = cx.update(|_, cx| panel.read(cx).clock.clone());

		for event in [
			ViewerEvent::InteractPointer {
				kind: InteractPointerKind::Move,
				position: Point { x: 1.0, y: 1.0 },
				button: Some(MouseButton::Left),
				pressed: false,
			},
			ViewerEvent::InteractKey {
				down: true,
				keystroke: Keystroke::parse("a").expect("parse"),
			},
			ViewerEvent::ToggleSafeFramesRequested { control: 3 },
			ViewerEvent::ToggleZoomRequested { control: 3 },
			ViewerEvent::InPointRequested { control: 3 },
			ViewerEvent::OutPointRequested { control: 3 },
			ViewerEvent::ClearRangeRequested { control: 3 },
			ViewerEvent::EyedropperPick {
				color: Rgba {
					r: 1.0,
					g: 0.0,
					b: 0.0,
					a: 1.0,
				},
			},
		] {
			cx.update(|_, cx| viewer.update(cx, |_viewer, cx| cx.emit(event.clone())));
			cx.run_until_parked();
		}
		let picked =
			cx.update(|_, cx| engine.update(cx, |engine, cx| engine.take_eyedropper_result(cx)));
		assert_eq!(
			picked,
			Some(Rgba {
				r: 1.0,
				g: 0.0,
				b: 0.0,
				a: 1.0,
			}),
			"the eyedropper pick lands in the engine's mailbox"
		);

		// Transport requests reach the program clock.
		cx.update(|_, cx| {
			viewer.update(cx, |_viewer, cx| {
				cx.emit(ViewerEvent::PlayRequested { control: 3 })
			})
		});
		cx.run_until_parked();
		assert!(cx.read(|app| clock.read(app).is_playing()));
		cx.update(|_, cx| {
			viewer.update(cx, |_viewer, cx| {
				cx.emit(ViewerEvent::PauseRequested { control: 3 })
			})
		});
		cx.run_until_parked();
		assert!(!cx.read(|app| clock.read(app).is_playing()));
		cx.update(|_, cx| {
			viewer.update(cx, |_viewer, cx| {
				cx.emit(ViewerEvent::StepRequested {
					control: 3,
					delta: 5,
				})
			})
		});
		cx.run_until_parked();
		assert_eq!(cx.read(|app| clock.read(app).current_frame()), Frame(5));
	}

	/// The context-menu resolver and every viewer menu action, including the
	/// Save Frame arm (which writes through the engine's capture path).
	#[gpui::test]
	async fn menu_actions_apply_to_viewer_and_engine(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		let engine = cx.update(|_, cx| panel.read(cx).engine.clone());

		// A valid local item id resolves through the shared table; an
		// unknown one logs and returns.
		let valid = (0..4096)
			.find(|&item| crate::oakui::component::menu::viewer_menu_action(item).is_some())
			.expect("the viewer menu has at least one item");
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.on_local_menu_item(999_999, cx);
				panel.on_local_menu_item(valid, cx);
			});
		});

		// Every action variant.
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.apply_viewer_action(ViewerMenuAction::ZoomFit, cx);
				panel.apply_viewer_action(ViewerMenuAction::ZoomLevel(4), cx);
				panel.apply_viewer_action(ViewerMenuAction::Resolution(2), cx);
				panel.apply_viewer_action(ViewerMenuAction::SafeOff, cx);
				panel.apply_viewer_action(ViewerMenuAction::SafeOn, cx);
				panel.apply_viewer_action(ViewerMenuAction::SafeCustom, cx);
				panel.apply_viewer_action(ViewerMenuAction::StopOnLast, cx);
				panel.apply_viewer_action(ViewerMenuAction::Waveform(WaveformMode::Only), cx);
				panel.apply_viewer_action(ViewerMenuAction::Waveform(WaveformMode::Both), cx);
				panel.apply_viewer_action(ViewerMenuAction::ShowFps, cx);
				panel.apply_viewer_action(ViewerMenuAction::FullScreen, cx);
			});
		});
		cx.run_until_parked();
		// The viewer-local actions are observable; the divider/stop-on-last/
		// waveform actions write the process-global config store (asserted
		// by the engine's own tests, not here).
		cx.update(|_, cx| {
			let panel = panel.read(cx);
			assert_eq!(panel.viewer.read(cx).zoom(), ViewerZoom::Level(4));
			assert_eq!(
				panel.viewer.read(cx).safe_margins(),
				SafeMargins::Custom(0.9, 0.8)
			);
			assert!(panel.viewer.read(cx).show_fps());
		});
		let _ = engine;

		// Save Frame: HOME points at a scratch directory so the capture
		// lands in `$HOME/Pictures` instead of the developer's home.
		let _lock = stack_lock();
		let home = std::env::temp_dir().join(format!("oakapp_viewer_home_{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&home);
		std::fs::create_dir_all(&home).expect("create the scratch home");
		let _home_guard = HomeGuard::redirect(&home);
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.apply_viewer_action(ViewerMenuAction::SaveFrame, cx)
			});
		});
		let saved = std::fs::read_dir(home.join("Pictures"))
			.map(|entries| {
				entries
					.filter_map(|e| e.ok())
					.any(|e| e.file_name().to_string_lossy().starts_with("oak-frame-program-"))
			})
			.unwrap_or(false);
		assert!(saved, "Save Frame writes a PNG into $HOME/Pictures");

		// Non-writable target: the engine's error arm logs instead of
		// panicking. (Only meaningful when the test user owns the mode
		// bits; a root run writes through and still passes.)
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let pictures = home.join("Pictures");
			std::fs::set_permissions(&pictures, std::fs::Permissions::from_mode(0o555))
				.expect("make Pictures read-only");
			cx.update(|_, cx| {
				panel.update(cx, |panel, cx| {
					panel.apply_viewer_action(ViewerMenuAction::SaveFrame, cx)
				});
			});
			std::fs::set_permissions(&pictures, std::fs::Permissions::from_mode(0o755))
				.expect("restore the mode");
		}
		let _ = std::fs::remove_dir_all(&home);
	}

	/// The panel's own mouse handlers: a left click focuses the panel, a
	/// right click builds the viewer menu state and opens the popup.
	#[gpui::test]
	async fn mouse_downs_focus_and_open_the_context_menu(cx: &mut TestAppContext) {
		let (cx, _panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		let center = gpui::point(px(320.0), px(180.0));
		cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
		cx.run_until_parked();
		cx.simulate_mouse_down(center, MouseButton::Right, Modifiers::none());
		cx.run_until_parked();
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(
			cx.debug_bounds("menu-popup").is_some(),
			"the viewer context menu renders"
		);
	}

	/// Every focused-panel transport hook, the tab button click and the
	/// dock tab content.
	#[gpui::test]
	async fn transport_hooks_and_dock_content(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				assert!(panel.play_pause(cx));
				assert!(panel.prev_frame(cx));
				assert!(panel.next_frame(cx));
				assert!(panel.go_to_start(cx));
				assert!(panel.go_to_end(cx));
				assert!(panel.play_in_to_out(cx));
				assert!(panel.go_to_in(cx));
				assert!(panel.go_to_out(cx));
				assert!(panel.shuttle_left(cx));
				assert!(panel.shuttle_stop(cx));
				assert!(panel.shuttle_right(cx));
			});
		});
		let _ = cx.update(|_, cx| panel.read(cx).tab_content(cx));
		assert_eq!(
			cx.update(|_, cx| panel.read(cx).panel_id()),
			crate::panels::ids::PROGRAM_VIEWER
		);

		// The header's tab buttons switch the body. They are the rightmost
		// row elements; sweep from the right edge to find them.
		let mut switched = false;
		for step in 0..320 {
			let x = 639.0 - f64::from(step) * 2.0;
			cx.simulate_click(
				gpui::point(px(x as f32), px(14.0)),
				Modifiers::none(),
			);
			cx.run_until_parked();
			if cx.read(|app| panel.read(app).tab == ProgramViewTab::Scopes) {
				switched = true;
				break;
			}
		}
		assert!(switched, "a click on the scopes tab switches the body");
	}

	/// The frame-sync debug dump, the interact forwarders without an active
	/// interact, the playhead helper and the idle pump timer.
	#[gpui::test]
	async fn sync_frame_debug_and_interact_noops(cx: &mut TestAppContext) {
		let _lock = stack_lock();
		let (cx, panel) = panel_window(cx);

		let prev_debug = std::env::var_os("OAK_DEBUG_VIEWER");
		std::env::set_var("OAK_DEBUG_VIEWER", "1");
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		cx.run_until_parked();
		std::fs::remove_file("/tmp/oak_viewer_frame.ppm").ok();
		match prev_debug {
			Some(value) => std::env::set_var("OAK_DEBUG_VIEWER", value),
			None => std::env::remove_var("OAK_DEBUG_VIEWER"),
		}

		// The forwarders bail out before touching the picture when no OFX
		// interact is live.
		cx.update(|_, cx| {
			panel.update(cx, |panel, cx| {
				panel.forward_interact_pointer(
					InteractPointerKind::Down,
					Point { x: 1.0, y: 1.0 },
					Some(MouseButton::Left),
					true,
					cx,
				);
				let keystroke = Keystroke::parse("a").expect("parse");
				panel.forward_interact_key(true, &keystroke, cx);
				panel.forward_interact_key(false, &keystroke, cx);
				let _seconds = panel.playhead_seconds(cx);
				// `displayed_frame` returns the raw frame without an
				// overlay.
				let frame = panel.engine.read(cx).cpu_frame(Monitor::Program, cx);
				let displayed = panel.displayed_frame(&frame, cx);
				assert!(std::sync::Arc::ptr_eq(&displayed, &frame));
			});
		});

		// The idle pump's 50 ms timer runs on the test clock.
		cx.executor()
			.advance_clock(std::time::Duration::from_millis(120));
		cx.run_until_parked();
	}

	/// The scopes tab renders from the mock engine's synthetic frame without
	/// crashing, and the scope state carries that frame's samples.
	#[gpui::test]
	async fn scopes_tab_renders_from_the_current_frame(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(640.0), px(360.0)), |window, cx| {
			let engine = cx.new(MockEngine::demo);
			let clock = engine.read(cx).program_clock().clone();
			let meter = cx.new(|cx| AudioLevelMeter::new(30, engine.clone(), window, cx));
			ProgramViewerPanel::new(engine, clock, meter, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("program viewer panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

		// Draw the picture tab once (fills the scope state from frame 0),
		// then switch to the scopes tab and draw it.
		cx.update(|window, cx| {
			window.draw(cx).clear();
			panel.update(cx, |panel, cx| {
				panel.tab = ProgramViewTab::Scopes;
				cx.notify();
			});
		});
		cx.run_until_parked();
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});

		let (luma_len, chroma_len, bins, envelope) = cx.read(|app| {
			let panel = panel.read(app);
			(
				panel.scope_state.read(app).luma.len(),
				panel.scope_state.read(app).chroma.len(),
				panel.histogram.read(app).bins(app),
				panel.waveform.read(app).envelope(app),
			)
		});
		let pixels = (crate::oakui::frames::SYNTH_FRAME_WIDTH
			* crate::oakui::frames::SYNTH_FRAME_HEIGHT) as usize;
		assert_eq!(luma_len, pixels);
		assert_eq!(chroma_len, pixels);
		assert_eq!(bins.iter().sum::<u32>() as usize, pixels);
		assert_eq!(envelope.len(), 128);
	}
}

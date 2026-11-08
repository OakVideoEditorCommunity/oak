//! The slider family: float, integer, rational (fraction) and angle sliders.
//!
//! A [`Slider`] is a thin view over the pure [`SliderModel`] state machine.
//! Gestures:
//!
//! * **Drag vertically** (up increases) to change the value; hold `shift` for
//!   fine adjustment. A drag starts once the cursor moves past a small
//!   threshold, so plain clicks are still clicks.
//! * **Double-click** to type a value directly. `enter` commits, `escape`
//!   cancels, and losing focus commits what was typed. Invalid text is
//!   rejected and the previous value is kept.
//! * **Mouse wheel** steps the value (coarse, or fine with `shift`).
//! * **Middle-click** resets to the default.
//! * **Arrow keys** step when the slider is focused.
//!
//! Every change is emitted as [`SliderEvent::ValueChanged`]; the widget never
//! mutates engine state. Formatting/parsing is injectable via
//! [`ValueFormatter`](crate::widgets::value::ValueFormatter).

mod model;

use std::sync::{Arc, RwLock};

use gpui_kit::{
	App, BorderStyle, Bounds, ClickEvent, Context, Corners, DragMoveEvent, Edges, ElementId,
	Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
	Pixels, Point, Render, ScrollWheelEvent, Subscription, Window, canvas, colors::DefaultColors,
	div, fill, point, prelude::*, px, quad, size,
};
use gpui_kit::base::input::InputState;
use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::Sizable as _;

pub use model::SliderModel;

use crate::widgets::keyable::{KeyingRequest, KeyingState, keying_diamond, keying_request};
use crate::widgets::value::{SliderValue, ValueFormatter};

/// How far (in pixels) the cursor must move vertically to sweep the whole
/// value range. Larger values make the slider less sensitive.
const VERTICAL_DRAG_RANGE_PX: f32 = 100.0;

/// A request emitted by a slider.
#[derive(Debug, Clone, PartialEq)]
pub enum SliderEvent {
	/// The value changed (drag, wheel, reset, typed commit).
	ValueChanged {
		/// The control's stable id.
		control: usize,
		/// The new value.
		value: SliderValue,
	},
	/// A drag gesture started.
	DragStarted {
		/// The control's stable id.
		control: usize,
	},
	/// A drag gesture ended.
	DragFinished {
		/// The control's stable id.
		control: usize,
	},
	/// Direct text entry was committed (or the editor lost focus).
	EditCommitted {
		/// The control's stable id.
		control: usize,
		/// The accepted value.
		value: SliderValue,
	},
	/// Direct text entry was cancelled (escape or rejected input).
	EditCancelled {
		/// The control's stable id.
		control: usize,
	},
	/// The keying diamond was clicked.
	Keying(KeyingRequest),
}

/// Transient payload carried by an in-flight drag gesture.
#[derive(Clone, Copy, Debug)]
struct SliderDrag {
	/// Cursor Y at drag start (window coords, px).
	start_y: f32,
}

/// Invisible ghost view that follows the cursor during a drag.
#[derive(Clone, Copy, Debug)]
struct SliderDragGhost;

impl Render for SliderDragGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().w(px(0.0)).h(px(0.0))
	}
}

/// A single form-control slider.
pub struct Slider {
	/// Stable id used for element ids, events and keying requests.
	control: usize,
	/// The pure state machine.
	model: SliderModel,
	/// Injectable formatting/parsing.
	formatter: Box<dyn ValueFormatter>,
	/// The keying state reported at the current frame.
	keying: KeyingState,
	/// For arrow-key navigation.
	focus_handle: FocusHandle,
	/// Active direct-entry editor, if any.
	editing: Option<Entity<InputState>>,
	/// Focus-out subscription of the active editor, kept alive while editing.
	edit_subscription: Option<Subscription>,
	/// Enter-key (commit) subscription of the active editor.
	edit_enter_subscription: Option<Subscription>,
	/// Set by the escape path so a later focus-lost does not commit.
	edit_cancelled: bool,
	/// True while the user is dragging (so we can report drag start/finish).
	dragging: bool,
}

impl Slider {
	/// Create a slider for `control` over `model`.
	///
	/// Starts with the default `DefaultFormatter`, `KeyingState::NoKey`, no
	/// active direct-entry editor and not dragging. The edited value comes from
	/// `model`, so range, step and default are configured there; chain
	/// `with_formatter`/`with_keying` to customize the rest.
	///
	/// # Arguments
	///
	/// * `control` - stable control id used in element ids and in every
	///   `SliderEvent`; the host maps it to its own parameter.
	/// * `model` - the pure range/step state, copied into the view.
	/// * `_window` - unused; accepted so the constructor matches the other widget
	///   constructors.
	/// * `cx` - used for the focus handle that arrow-key navigation needs.
	pub fn new(
		control: usize,
		model: SliderModel,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			model,
			formatter: Box::new(crate::widgets::value::DefaultFormatter),
			keying: KeyingState::NoKey,
			focus_handle: cx.focus_handle(),
			editing: None,
			edit_subscription: None,
			edit_enter_subscription: None,
			edit_cancelled: false,
			dragging: false,
		}
	}

	/// Inject a custom formatter/parser for display and direct entry.
	pub fn with_formatter(mut self, formatter: impl ValueFormatter) -> Self {
		self.formatter = Box::new(formatter);
		self
	}

	/// Report the control's keying state for the current frame (builder form).
	///
	/// The slider does not derive keying itself: the host recomputes the state
	/// (animated / keyframed / no key) every frame and passes it in. The diamond
	/// renders that state and emits `SliderEvent::Keying` when clicked, without
	/// mutating the state.
	///
	/// # Arguments
	///
	/// * `state` - the state to display; replaces any previously set state.
	pub fn with_keying(mut self, state: KeyingState) -> Self {
		self.keying = state;
		self
	}

	/// The current value.
	pub fn value(&self) -> SliderValue {
		self.model.value()
	}

	/// A copy of the pure model state.
	pub fn model(&self) -> SliderModel {
		self.model
	}

	/// Whether direct entry is currently active.
	pub fn is_editing(&self) -> bool {
		self.editing.is_some()
	}

	/// The active editor entity, if direct entry is open.
	///
	/// Hosts and tests can use this to seed or inspect the text being
	/// edited (e.g. `editor.update(cx, |editor, cx| editor.emplace("0.7", cx))`).
	pub fn editor(&self) -> Option<Entity<InputState>> {
		self.editing.clone()
	}

	/// Set the value directly (clamped and snapped by the model).
	///
	/// Delegates to `SliderModel::set_value`: clamps to `[min, max]`, snaps to the
	/// coarse step and converts between `SliderValue` kinds where possible;
	/// non-finite input is ignored. This is the programmatic path for hosts: it
	/// emits no `ValueChanged` and does not repaint.
	pub fn set_value(&mut self, value: SliderValue) {
		self.model.set_value(value);
	}

	/// Emit `ValueChanged` if `changed` is true, then request a repaint.
	///
	/// The shared tail of every value gesture: `changed` is the model's answer
	/// (`set_value`/`apply_step`/`reset`/`drag_delta`), so a gesture that leaves
	/// the value where it was emits nothing. The repaint happens only when an
	/// event was actually emitted.
	///
	/// # Arguments
	///
	/// * `changed` - whether the model position actually moved.
	/// * `cx` - emit/notify target; the event is filled in with `control` and the
	///   current value.
	fn apply_and_notify(&mut self, changed: bool, cx: &mut Context<Self>) {
		if changed {
			cx.emit(SliderEvent::ValueChanged {
				control: self.control,
				value: self.model.value(),
			});
			cx.notify();
		}
	}

	/// Begin double-click direct entry.
	///
	/// Formats the current value through the injected formatter, creates an
	/// `InputState` seeded with that text, focuses it and subscribes to its Enter
	/// key and to focus loss. Clicking away commits what was typed, unless escape
	/// already cancelled the session (`edit_cancelled`). An already open editor
	/// makes this a no-op.
	///
	/// # Arguments
	///
	/// * `window` - used to create the editor input and to take focus.
	/// * `cx` - used to create the editor entity and to subscribe; the view is
	///   repainted at the end.
	fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		if self.editing.is_some() {
			return;
		}
		let text = self.formatter.format(self.model.value());
		let editor = cx.new(|cx| InputState::new(window, cx));
		editor.update(cx, |editor, cx| {
			editor.set_value(text, window, cx);
		});
		// Enter commits (the input consumes the key and reports it as an
		// event; the wrapper's key listener below keeps Escape).
		let enter = cx.subscribe(&editor, |this, _editor, event: &InputEvent, cx| {
			if matches!(event, InputEvent::PressEnter { .. }) {
				this.commit_edit(cx);
			}
		});
		let focus_handle = editor.read(cx).focus_handle(cx);
		let subscription = cx.on_focus_out(&focus_handle, window, |this, _event, _window, cx| {
			// Clicking away commits what was typed, unless escape already
			// cancelled the session.
			if this.editing.is_some() {
				if this.edit_cancelled {
					this.cancel_edit(cx);
				} else {
					this.commit_edit(cx);
				}
			}
		});
		self.edit_cancelled = false;
		self.edit_subscription = Some(subscription);
		self.edit_enter_subscription = Some(enter);
		self.editing = Some(editor.clone());
		window.focus(&focus_handle, cx);
		cx.notify();
	}

	/// Commit the current editor text. Invalid text is rejected (the
	/// previous value is kept) and the editor closes either way.
	fn commit_edit(&mut self, cx: &mut Context<Self>) {
		if let Some(editor) = self.editing.take() {
			self.edit_subscription = None;
			self.edit_enter_subscription = None;
			let text = editor.read(cx).value().to_string();
			match self.formatter.parse(&text) {
				Ok(value) => {
					let changed = self.model.set_value(value);
					self.apply_and_notify(changed, cx);
					cx.emit(SliderEvent::EditCommitted {
						control: self.control,
						value: self.model.value(),
					});
				}
				Err(_) => {
					cx.emit(SliderEvent::EditCancelled {
						control: self.control,
					});
				}
			}
			cx.notify();
		}
	}

	/// Cancel direct entry, keeping the previous value.
	///
	/// Drops the editor and both subscriptions and emits
	/// `SliderEvent::EditCancelled`; used by the escape path and by a focus-out
	/// that follows a cancelled session. A no-op when no editor is open, so
	/// calling it twice is safe.
	///
	/// # Arguments
	///
	/// * `cx` - emit/notify target.
	fn cancel_edit(&mut self, cx: &mut Context<Self>) {
		if self.editing.take().is_some() {
			self.edit_subscription = None;
			self.edit_enter_subscription = None;
			cx.emit(SliderEvent::EditCancelled {
				control: self.control,
			});
			cx.notify();
		}
	}

	/// Apply a wheel step.
	///
	/// Converts the wheel delta to whole notches (16 px per notch, rounded) and
	/// steps the model by that many coarse or fine increments
	/// (`SliderModel::apply_step`, shift = fine); deltas below half a notch are
	/// ignored. Emits `ValueChanged` only when the model actually moved.
	///
	/// # Arguments
	///
	/// * `event` - the scroll event; only `delta.y` and the shift modifier are
	///   read.
	/// * `cx` - emit/notify target.
	fn apply_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
		let delta = event.delta.pixel_delta(px(16.0));
		let steps = (f32::from(delta.y) / 16.0).round() as i32;
		if steps == 0 {
			return;
		}
		let fine = event.modifiers.shift;
		let changed = self.model.apply_step(steps, fine);
		self.apply_and_notify(changed, cx);
	}
}

impl EventEmitter<SliderEvent> for Slider {}

impl Focusable for Slider {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl Render for Slider {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let fraction = self.model.fraction();
		let drag_payload = Arc::new(RwLock::new(SliderDrag { start_y: 0.0 }));
		let control = self.control;

		let mut track = div()
			.id(ElementId::named_usize("gpui-widgets-slider-track", control))
			.flex_1()
			.h(px(18.0))
			.relative()
			.rounded_md()
			.bg(colors.container)
			.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
			.track_focus(&self.focus_handle)
			.cursor_pointer()
			.debug_selector(|| "slider-track".into())
			.on_drag(drag_payload.clone(), slider_drag_ghost)
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<SliderDrag>>>, _window, cx| {
					let drag = event.drag(cx).clone();
					let drag = drag.read().unwrap();
					let dy = f32::from(event.event.position.y) - drag.start_y;
					let fine = event.event.modifiers.shift;
					let changed = this.model.drag_delta(dy, VERTICAL_DRAG_RANGE_PX, fine);
					this.apply_and_notify(changed, cx);
				},
			))
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<SliderDrag>>>, _window, cx| {
					if !this.dragging {
						this.dragging = true;
						cx.emit(SliderEvent::DragStarted {
							control: this.control,
						});
					}
					let _ = event;
					cx.notify();
				},
			))
			.on_drop(
				cx.listener(|this, _drag: &Arc<RwLock<SliderDrag>>, _window, cx| {
					if this.dragging {
						this.dragging = false;
						cx.emit(SliderEvent::DragFinished {
							control: this.control,
						});
					}
				}),
			)
			.on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
				if event.click_count() >= 2 {
					this.begin_edit(window, cx);
				}
			}))
			.on_mouse_down(
				MouseButton::Middle,
				cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
					let changed = this.model.reset();
					this.apply_and_notify(changed, cx);
				}),
			)
			.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
				this.apply_scroll(event, cx);
			}))
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
				match event.keystroke.key.as_str() {
					"left" | "down" => {
						let changed = this.model.apply_step(-1, event.keystroke.modifiers.shift);
						this.apply_and_notify(changed, cx);
					}
					"right" | "up" => {
						let changed = this.model.apply_step(1, event.keystroke.modifiers.shift);
						this.apply_and_notify(changed, cx);
					}
					_ => {}
				}
			}))
			.child(canvas(
				move |bounds, _window, _cx| bounds,
				move |bounds, content, window, cx| {
					paint_slider(bounds, content, fraction, window, cx);
				},
			));

		// Direct-entry editor overlay while editing.
		if let Some(editor) = self.editing.clone() {
			track = track.child(
				div()
					.absolute()
					.left_0()
					.right_0()
					.top_0()
					.bottom_0()
					.flex()
					.items_center()
					.px_1()
					.bg(colors.background)
					.on_mouse_down_out(cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
						// Clicking outside the editor commits.
						this.commit_edit(cx);
					}))
					.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
						match event.keystroke.key.as_str() {
							"enter" => this.commit_edit(cx),
							"escape" => {
								this.edit_cancelled = true;
								this.cancel_edit(cx);
							}
							_ => {}
						}
					}))
					.child(
						Input::new(&editor)
							.id(ElementId::named_usize(
								"gpui-widgets-slider-edit-input",
								control,
							))
							.appearance(false)
							.small(),
					),
			);
		}

		div()
			.w_full()
			.flex()
			.items_center()
			.gap(px(6.0))
			.child(track)
			.child(
				keying_diamond(self.control, self.keying).on_click(cx.listener(
					|this, _event: &ClickEvent, _window, cx| {
						cx.emit(SliderEvent::Keying(keying_request(
							this.control,
							this.keying,
						)));
						cx.stop_propagation();
					},
				)),
			)
	}
}

/// Build the (invisible) ghost view that accompanies a slider drag.
fn slider_drag_ghost(
	drag: &Arc<RwLock<SliderDrag>>,
	_offset: Point<Pixels>,
	window: &mut Window,
	cx: &mut App,
) -> Entity<SliderDragGhost> {
	if let Ok(mut drag) = drag.write() {
		drag.start_y = f32::from(window.mouse_position().y);
	}
	cx.new(|_| SliderDragGhost)
}

/// Paint a slider track, its active fill and the handle into `bounds`.
///
/// The fill runs from the left edge to `fraction` of the width; the handle is
/// a circle centered on that end point, so it overhangs the track by its
/// radius at either extreme. Called from the view's canvas paint callback with
/// `SliderModel::fraction()`.
///
/// # Arguments
///
/// * `bounds` - element-local rectangle the track spans; the height is only
///   used to center the 4 px track vertically.
/// * `_content` - unused canvas content bounds.
/// * `fraction` - normalized position in `0..=1` (clamped before painting).
/// * `window` - paint target for the quads.
/// * `cx` - theme colors: `border` for the track, `selected` for the fill,
///   `text` for the handle.
fn paint_slider(
	bounds: Bounds<Pixels>,
	_content: Bounds<Pixels>,
	fraction: f64,
	window: &mut Window,
	cx: &mut App,
) {
	use gpui_kit::Hsla;
	let colors = cx.default_colors().clone();
	let center_y = px(f32::from(bounds.top()) + f32::from(bounds.size.height) / 2.0);

	// Track.
	let track_bounds = Bounds::new(
		point(bounds.left(), center_y - px(2.0)),
		size(bounds.size.width, px(4.0)),
	);
	window.paint_quad(fill(track_bounds, Hsla::from(colors.border)));

	// Active fill up to the fraction.
	let width = f32::from(bounds.size.width);
	let fill_width = (width * fraction as f32).clamp(0.0, width);
	if fill_width > 0.0 {
		let fill_bounds = Bounds::new(
			point(bounds.left(), track_bounds.top()),
			size(px(fill_width), track_bounds.size.height),
		);
		window.paint_quad(fill(fill_bounds, Hsla::from(colors.selected)));
	}

	// Handle: a small circle at the fraction position.
	let handle_radius = 5.0_f32;
	let handle_bounds = Bounds::new(
		point(
			bounds.left() + px(fill_width - handle_radius),
			center_y - px(handle_radius),
		),
		size(px(handle_radius * 2.0), px(handle_radius * 2.0)),
	);
	window.paint_quad(quad(
		handle_bounds,
		Corners::all(px(handle_radius)),
		Hsla::from(colors.text),
		Edges::all(px(1.0)),
		Hsla::from(colors.border),
		BorderStyle::Solid,
	));
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::widgets::value::ValueKind;
	use gpui_kit::{Modifiers, MouseUpEvent, ScrollDelta, TestAppContext, VisualTestContext};

	#[derive(Clone)]
	struct Host {
		slider: Entity<Slider>,
		events: Vec<SliderEvent>,
	}

	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div()
				.size_full()
				.flex()
				.flex_col()
				.justify_center()
				.child(self.slider.clone())
		}
	}

	fn make_host(cx: &mut TestAppContext) -> (&'static mut VisualTestContext, Entity<Host>) {
		// The direct-entry editor is a gpui-kit `Input`, which renders from
		// the component theme.
		cx.update(gpui_kit::init);
		// `default_colors()` (used by the widgets) requires the global to be
		// initialized; the test platform does not do it automatically.
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(400.0), px(100.0)), |window, cx| {
			let slider = cx.new(|cx| {
				Slider::new(
					1,
					SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.1, 0.5),
					window,
					cx,
				)
			});
			let host = Host {
				slider,
				events: Vec::new(),
			};
			cx.subscribe(
				&host.slider,
				|host: &mut Host,
				 _slider: Entity<Slider>,
				 event: &SliderEvent,
				 _cx: &mut Context<Host>| {
					host.events.push(event.clone());
				},
			)
			.detach();
			host
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, host)
	}

	fn value_events(events: &[SliderEvent]) -> Vec<f64> {
		events
			.iter()
			.filter_map(|event| match event {
				SliderEvent::ValueChanged { value, .. } => Some(value.to_f64()),
				_ => None,
			})
			.collect()
	}

	fn double_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
		let modifiers = Modifiers::none();
		cx.simulate_event(MouseDownEvent {
			position,
			modifiers,
			button: MouseButton::Left,
			click_count: 2,
			first_mouse: false,
		});
		cx.simulate_event(MouseUpEvent {
			position,
			modifiers,
			button: MouseButton::Left,
			click_count: 2,
		});
		cx.run_until_parked();
		// Render the editor overlay so its input handler is registered.
		redraw(cx);
	}

	/// A point comfortably inside the track (row is vertically centered).
	fn track_point() -> Point<Pixels> {
		point(px(200.0), px(50.0))
	}

	/// The test platform does not redraw dirty windows on its own, so state
	/// changes (which only call `notify`) do not repaint until we force a
	/// draw. This is required for freshly-created elements (e.g. the editor
	/// overlay) to register their listeners and input handlers.
	fn redraw(cx: &mut VisualTestContext) {
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
	}

	/// Compare a slider value to an expected float with tolerance (stepping
	/// through non-binary steps like 0.1 accumulates representation error).
	fn assert_value_close(actual: SliderValue, expected: f64) {
		assert!(
			(actual.to_f64() - expected).abs() < 1e-6,
			"expected {expected}, got {actual:?}"
		);
	}

	#[gpui_kit::test]
	async fn track_hitbox_is_rendered(cx: &mut TestAppContext) {
		let (cx, _host) = make_host(cx);
		let bounds = cx.debug_bounds("slider-track");
		assert!(
			bounds.is_some(),
			"slider track was not painted into the rendered frame"
		);
		if let Some(bounds) = bounds {
			eprintln!("track bounds: {bounds:?}, track point: {:?}", track_point());
			assert!(bounds.contains(&track_point()));
		}
	}

	/// Seed the active editor with `text` (the test platform cannot deliver
	/// IME characters, so tests set the text directly and exercise the
	/// commit/cancel paths).
	fn seed_editor_text(cx: &mut VisualTestContext, host: &Entity<Host>, text: &str) {
		let editor = cx
			.read(|app| host.read(app).slider.read(app).editor())
			.expect("editor should be open");
		// `set_value` lays out against a window, so seed through the
		// windowed update.
		cx.update(|window, cx| {
			editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
		});
	}

	#[gpui_kit::test]
	async fn double_click_opens_editor_and_enter_commits(cx: &mut TestAppContext) {
		let (mut cx, host) = make_host(cx);
		double_click(&mut cx, track_point());
		assert!(cx.read(|app| host.read(app).slider.read(app).is_editing()));

		seed_editor_text(&mut cx, &host, "0.7");
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();

		let (value, is_editing, committed) = cx.read(|app| {
            let host = host.read(app);
            (
                host.slider.read(app).value(),
                host.slider.read(app).is_editing(),
                host.events.iter().any(|e| {
                    matches!(e, SliderEvent::EditCommitted { value, .. } if (value.to_f64() - 0.7).abs() < 1e-6)
                }),
            )
        });
		assert!(!is_editing);
		assert_value_close(value, 0.7);
		assert!(committed);
	}

	#[gpui_kit::test]
	async fn double_click_rejects_invalid_text(cx: &mut TestAppContext) {
		let (mut cx, host) = make_host(cx);
		double_click(&mut cx, track_point());
		seed_editor_text(&mut cx, &host, "abc");
		cx.simulate_keystrokes("enter");
		cx.run_until_parked();

		let (value, is_editing, cancelled) = cx.read(|app| {
			let host = host.read(app);
			(
				host.slider.read(app).value(),
				host.slider.read(app).is_editing(),
				host.events
					.iter()
					.any(|e| matches!(e, SliderEvent::EditCancelled { .. })),
			)
		});
		assert!(!is_editing);
		// The previous value is kept.
		assert_eq!(value, SliderValue::Float(0.5));
		assert!(cancelled);
	}

	#[gpui_kit::test]
	async fn escape_cancels_editing(cx: &mut TestAppContext) {
		let (mut cx, host) = make_host(cx);
		double_click(&mut cx, track_point());
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();

		let (value, is_editing, cancelled) = cx.read(|app| {
			let host = host.read(app);
			(
				host.slider.read(app).value(),
				host.slider.read(app).is_editing(),
				host.events
					.iter()
					.any(|e| matches!(e, SliderEvent::EditCancelled { .. })),
			)
		});
		assert!(!is_editing);
		assert_eq!(value, SliderValue::Float(0.5));
		assert!(cancelled);
	}

	#[gpui_kit::test]
	async fn middle_click_resets_to_default(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// Move the value first.
		cx.simulate_event(ScrollWheelEvent {
			position: track_point(),
			delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
			..Default::default()
		});
		cx.run_until_parked();
		{
			let moved = cx.read(|app| host.read(app).slider.read(app).value().to_f64());
			assert!(
				(moved - 0.5).abs() > 0.01,
				"value should have moved, got {moved}"
			);
		}

		// Middle-click resets.
		cx.simulate_mouse_down(track_point(), MouseButton::Middle, Modifiers::none());
		cx.simulate_mouse_up(track_point(), MouseButton::Middle, Modifiers::none());
		cx.run_until_parked();

		let (value, events) = cx.read(|app| {
			let host = host.read(app);
			(host.slider.read(app).value(), host.events.clone())
		});
		assert_eq!(value, SliderValue::Float(0.5));
		assert!(value_events(&events).contains(&0.5));
	}

	#[gpui_kit::test]
	async fn wheel_steps_value(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// Scrolling up (positive y) increases the value.
		cx.simulate_event(ScrollWheelEvent {
			position: track_point(),
			delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
			..Default::default()
		});
		cx.run_until_parked();

		let (value, events) = cx.read(|app| {
			let host = host.read(app);
			(host.slider.read(app).value(), host.events.clone())
		});
		// 80px at 16px per notch = 5 coarse steps of 0.1, clamped to max.
		assert_value_close(value, 1.0);
		assert!(!value_events(&events).is_empty());
	}

	#[gpui_kit::test]
	async fn vertical_drag_changes_value(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// Press on the track, then drag up. The first move past the drag
		// threshold anchors the gesture; the second move applies the delta.
		let modifiers = Modifiers::none();
		cx.simulate_mouse_down(track_point(), MouseButton::Left, modifiers);
		cx.simulate_mouse_move(point(px(200.0), px(45.0)), MouseButton::Left, modifiers);
		cx.simulate_mouse_move(point(px(200.0), px(25.0)), MouseButton::Left, modifiers);
		// Release inside the track so the drop listener on the slider fires.
		cx.simulate_mouse_up(point(px(200.0), px(45.0)), MouseButton::Left, modifiers);
		cx.run_until_parked();

		let (value, started, finished) = cx.read(|app| {
			let host = host.read(app);
			(
				host.slider.read(app).value().to_f64(),
				host.events
					.iter()
					.any(|e| matches!(e, SliderEvent::DragStarted { .. })),
				host.events
					.iter()
					.any(|e| matches!(e, SliderEvent::DragFinished { .. })),
			)
		});
		// 20px / 100px of range = +0.2 over the default 0.5.
		assert!((value - 0.7).abs() < 0.001, "expected ~0.7, got {value}");
		assert!(started);
		assert!(finished);
	}

	#[gpui_kit::test]
	async fn focused_slider_steps_with_arrow_keys(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// Click once to focus the slider (click_count 1 is a no-op for value).
		cx.simulate_click(track_point(), Modifiers::none());
		cx.simulate_keystrokes("right");
		cx.run_until_parked();

		let (value, events) = cx.read(|app| {
			let host = host.read(app);
			(host.slider.read(app).value(), host.events.clone())
		});
		assert_value_close(value, 0.6);
		assert!(value_events(&events).iter().any(|v| (v - 0.6).abs() < 1e-6));
	}

	#[gpui_kit::test]
	async fn keying_diamond_emits_request(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// The diamond sits at the right edge of the row.
		cx.simulate_click(point(px(390.0), px(50.0)), Modifiers::none());
		cx.run_until_parked();

		let requested = cx.read(|app| {
			host.read(app)
				.events
				.iter()
				.any(|e| matches!(e, SliderEvent::Keying(request) if request.control == 1))
		});
		assert!(requested);
	}

	#[gpui_kit::test]
	async fn clicking_away_commits_editor(cx: &mut TestAppContext) {
		let (mut cx, host) = make_host(cx);
		double_click(&mut cx, track_point());
		// 0.3 is exactly representable at the slider's 0.1 step.
		seed_editor_text(&mut cx, &host, "0.3");
		// Clicking outside the editor overlay commits the typed value.
		cx.simulate_click(point(px(200.0), px(90.0)), Modifiers::none());
		cx.run_until_parked();

		let (value, is_editing, committed) = cx.read(|app| {
            let host = host.read(app);
            (
                host.slider.read(app).value(),
                host.slider.read(app).is_editing(),
                host.events.iter().any(|e| {
                    matches!(e, SliderEvent::EditCommitted { value, .. } if (value.to_f64() - 0.3).abs() < 1e-6)
                }),
            )
        });
		assert!(!is_editing);
		assert_value_close(value, 0.3);
		assert!(committed);
	}
}

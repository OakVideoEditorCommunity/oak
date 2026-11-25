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

//! The app's editor controls: slider, spin box, check box, combo box and
//! the colour picker, themed like the rest of the UI.
//!
//! # Interaction contract
//!
//! Every control answers to the mouse **and** the keyboard once focused:
//!
//! - [`Slider`] — horizontal drag (the gpui_widgets slider only reacted
//!   to *vertical* cursor movement, so a normal horizontal drag looked
//!   dead), click-to-jump, wheel, arrow keys (Shift = fine steps, Home/End
//!   = range ends), and a persistent value box (Enter commits).
//! - [`SpinBox`] / [`ComboBox`] / [`CheckBox`] — the gpui_widgets
//!   implementations, re-exported here so the app never reaches into
//!   gpui_widgets directly.
//! - [`OfxColorPicker`] — the colour swatch + deferred popup picker:
//!   channel sliders, hex field, Cancel/OK; only OK commits (a drag
//!   session is a single undo row).
//!
//! Colours come from `App::default_colors` (the app theme), matching the
//! text inputs and menus.

use crate::oakui;
use gpui_kit::Modifiers;
use gpui_kit::Point;
use gpui_kit::ScrollDelta;
use gpui_kit::base::input::InputState;
use gpui_kit::component::input::InputEvent;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
	AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
	InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent,
	ParentElement, Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
	Window, canvas, colors::DefaultColors, div, px,
};
use std::sync;

pub use crate::widgets::value::{SliderValue, ValueKind};

/// The colour swatch + deferred popup picker (the params panel's
/// implementation, surfaced here so effect controls live under
/// `oakui::component`).
pub use crate::panels::ofx_params::OfxColorPicker;

/// The value model of a [`Slider`] (range, step, current value).
pub use crate::widgets::slider::SliderModel;

/// Events emitted by a [`Slider`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliderEvent {
	/// The value changed (drag, wheel, click, arrow key).
	ValueChanged {
		/// The slider's stable control id.
		control: usize,
		/// The new value.
		value: SliderValue,
	},
	/// A drag gesture started.
	DragStarted { control: usize },
	/// A drag gesture ended.
	DragFinished { control: usize },
}

/// The in-flight drag gesture payload shared with the drag events.
struct SliderDrag {
	/// The control that STARTED the drag. gpui delivers drag-move events
	/// to every drop target with a matching payload type under the
	/// cursor — without this gate, dragging one slider also moved every
	/// other slider the cursor passed over (the OFX-panel drag that
	/// moved the timeline zoom/track-height sliders).
	control: usize,
	/// The cursor x of the first drag-move (the drag origin).
	first_x: f32,
	/// The model fraction when the drag started.
	origin_fraction: f64,
	/// Whether this is the first move (the origin is set then).
	first: bool,
}

/// The invisible ghost that accompanies a slider drag (gpui requires one
/// for `on_drag`).
struct SliderDragGhost;

impl Render for SliderDragGhost {
	fn render(
		&mut self,
		_window: &mut Window,
		_cx: &mut gpui_kit::Context<Self>,
	) -> impl IntoElement {
		div()
	}
}

/// A horizontal slider: track + filled portion + handle.
///
/// Mouse: a single click on the track jumps the value to the click
/// position (a double click opens the editor instead), a drag adjusts the
/// value with 1:1 cursor tracking (the handle follows the cursor; Shift =
/// 1/10 sensitivity), the wheel steps, middle click resets.
/// Keyboard (once focused): Left/Right step by one step, Shift-Left/Right
/// by 1/10 step, Home/End jump to the range ends.
/// A persistent value box shows the current value and accepts typed input
/// (Enter commits, clamped to the range); double-clicking the track opens
/// the same editor full-width over the track.
///
/// A gesture (drag, wheel tick, arrow key, click or typed commit) emits
/// [`SliderEvent::ValueChanged`] once per gesture — on drop for drags — so
/// the host applies one undoable edit per gesture instead of one per mouse
/// move.
pub struct Slider {
	control: usize,
	model: SliderModel,
	focus: FocusHandle,
	/// The numeric editor, while a double-click edit is open.
	edit: Option<Entity<InputState>>,
	/// True while a drag is in flight (to report start/finish).
	dragging: bool,
	/// Whether the open editor should cancel (Escape) instead of commit
	/// on blur.
	edit_cancel: bool,
	/// The persistent value box: shows the current value and accepts typed
	/// input (Enter commits, exactly like the double-click editor).
	value_editor: Entity<InputState>,
}

impl Slider {
	/// Creates a slider control over `model`.
	///
	/// Seeds the inline value box with the model's formatted value and
	/// subscribes to its Enter key so typed input commits through the
	/// control's apply path. The control never validates the model itself: it
	/// reads/clamps through [`SliderModel`] and emits [`SliderEvent`]s for the
	/// host to apply.
	///
	/// # Arguments
	///
	/// * `control` - Stable control id reported in events and used in element ids.
	/// * `model` - Range/value model; owned by the control.
	/// * `window` - Window used to build the attached text input.
	/// * `cx` - Context used to create the focus handle and subscription.
	pub fn new(
		control: usize,
		model: SliderModel,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let display = Self::display_value(&model);
		let value_editor = cx.new(|cx| InputState::new(window, cx));
		value_editor.update(cx, |editor, cx| {
			editor.set_value(display.to_string(), window, cx);
		});
		// The input consumes Enter; commit on its PressEnter event.
		cx.subscribe_in(
			&value_editor,
			window,
			|this, _editor, event: &InputEvent, window, cx| {
				if matches!(event, InputEvent::PressEnter { .. }) {
					let text = this.value_editor.read(cx).value().to_string();
					this.apply_text(&text, cx);
					// Move focus to the slider so the box re-syncs to the
					// normalized display text on the next frame.
					let focus = this.focus.clone();
					window.focus(&focus, cx);
				}
			},
		)
		.detach();
		Self {
			control,
			model,
			focus: cx.focus_handle(),
			edit: None,
			dragging: false,
			edit_cancel: false,
			value_editor,
		}
	}

	/// The current value (the params panel re-syncs external edits).
	pub fn set_value(&mut self, value: SliderValue) {
		self.model.set_value(value);
	}

	/// Replace the range model without rebuilding the entity — the colour
	/// picker re-ranges its primary sliders when switching RGB ↔ HSV.
	pub fn set_model(&mut self, model: SliderModel) {
		self.model = model;
	}

	/// The current value.
	pub fn value(&self) -> SliderValue {
		self.model.value()
	}

	/// Whether a drag is in flight (the params panel skips its
	/// engine-snapshot re-sync while it is — otherwise the read-back value
	/// fights the pointer and the thumb stops following).
	pub fn is_dragging(&self) -> bool {
		self.dragging
	}

	fn emit_changed(&mut self, cx: &mut Context<Self>) {
		cx.emit(SliderEvent::ValueChanged {
			control: self.control,
			value: self.model.value(),
		});
		cx.notify();
	}

	/// Complete an in-flight drag (one undoable edit per gesture): emits
	/// the final value and the DragFinished notice. No-op without a drag.
	fn finish_drag(&mut self, cx: &mut Context<Self>) {
		if self.dragging {
			self.dragging = false;
			self.emit_changed(cx);
			cx.emit(SliderEvent::DragFinished {
				control: self.control,
			});
		}
	}

	/// Opens the numeric editor seeded with the current value.
	///
	/// The editor commits on focus loss unless Escape set `edit_cancel`; the
	/// slider then re-syncs its display text. The editor entity is stored so
	/// the render pass can show it, and a repaint is requested.
	///
	/// # Arguments
	///
	/// * `window` - Window used to create and focus the editor.
	/// * `cx` - Context used for the focus-out subscription and repaint.
	fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let text = self.display();
		let editor = cx.new(|cx| InputState::new(window, cx));
		editor.update(cx, |editor, cx| {
			editor.set_value(text.to_string(), window, cx);
		});
		// Commit (or cancel) when the editor loses focus.
		let this_control = self.control;
		let weak = editor.downgrade();
		let focus = editor.read(cx).focus_handle(cx);
		cx.on_focus_out(
			&focus,
			window,
			move |this: &mut Self, _event, _window, cx| {
				if let Some(editor) = weak.upgrade() {
					let text = editor.read(cx).value().to_string();
					if !this.edit_cancel {
						this.apply_text(&text, cx);
					}
				}
				this.edit = None;
				this.edit_cancel = false;
				cx.notify();
			},
		)
		.detach();
		window.focus(&editor.read(cx).focus_handle(cx), cx);
		let _ = this_control;
		self.edit = Some(editor);
		cx.notify();
	}

	/// Parses typed text, clamps it through the model and reports a change.
	///
	/// Integer-valued models parse as `i64`; all other kinds parse as `f64`.
	/// Unparsable text is ignored silently (no event, no repaint). When the
	/// clamped value differs from the model's current value, emits
	/// [`SliderEvent::ValueChanged`] and notifies.
	///
	/// # Arguments
	///
	/// * `text` - Raw editor text; surrounding whitespace is ignored.
	/// * `cx` - Context used to emit the change and repaint.
	fn apply_text(&mut self, text: &str, cx: &mut Context<Self>) {
		let parsed: Option<f64> = match self.model.value() {
			SliderValue::Integer(_) => text.trim().parse::<i64>().ok().map(|v| v as f64),
			_ => text.trim().parse::<f64>().ok(),
		};
		let Some(raw) = parsed else {
			return;
		};
		let changed = self.model.apply_raw(raw);
		if changed {
			self.emit_changed(cx);
		}
	}

	/// The current value formatted for display.
	fn display(&self) -> SharedString {
		Self::display_value(&self.model)
	}

	/// Formats a model's value for display in a control.
	///
	/// Integer -> plain decimal, Float -> [`format_value`], Angle -> one decimal
	/// plus a degree sign, Rational -> `num/den`. This is the text seeded into
	/// the inline value box and the numeric editor.
	///
	/// # Arguments
	///
	/// * `model` - Model whose current value is formatted.
	fn display_value(model: &SliderModel) -> SharedString {
		match model.value() {
			SliderValue::Integer(v) => v.to_string().into(),
			SliderValue::Float(v) => format_value(v).into(),
			SliderValue::Angle(v) => format!("{v:.1}°").into(),
			SliderValue::Rational(r) => format!("{}/{}", r.num(), r.den()).into(),
		}
	}
}

impl EventEmitter<SliderEvent> for Slider {}

impl Render for Slider {
	fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let fraction = self.model.fraction();
		let control = self.control;
		let handle_x = (fraction * 100.0).clamp(0.0, 100.0) as f32;
		// The track's layout bounds, recorded by an invisible canvas every
		// frame so the click-to-jump handler can map a click x to a value
		// (mouse events carry no bounds of their own).
		let track_bounds = sync::Arc::new(sync::RwLock::new(
			None::<gpui_kit::Bounds<gpui_kit::Pixels>>,
		));
		let drag_payload = sync::Arc::new(sync::RwLock::new(SliderDrag {
			control,
			first_x: 0.0,
			origin_fraction: 0.0,
			first: true,
		}));

		let mut track = div()
			.id(gpui_kit::ElementId::named_usize("oak-slider", control))
			.debug_selector(move || format!("oak-slider-{control}"))
			.flex_1()
			.h(px(18.0))
			.relative()
			.rounded_md()
			.bg(colors.container)
			.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
			.track_focus(&self.focus)
			.cursor_pointer()
			.on_drag(drag_payload.clone(), |_payload, _offset, _window, cx| {
				cx.new(|_| SliderDragGhost)
			})
			.on_drag_move(cx.listener(
				|this,
				 event: &gpui_kit::DragMoveEvent<sync::Arc<sync::RwLock<SliderDrag>>>,
				 _window,
				 cx| {
					{
						let mut drag = event.drag(cx).write().unwrap();
						if drag.control != this.control {
							// Another slider's drag passing over us: not ours.
							return;
						}
						if drag.first {
							drag.first = false;
							drag.first_x = f32::from(event.event.position.x);
							drag.origin_fraction = this.model.fraction();
						}
					}
					if !this.dragging {
						this.dragging = true;
						cx.emit(SliderEvent::DragStarted {
							control: this.control,
						});
					}
					let drag = event.drag(cx).read().unwrap();
					let fine = event.event.modifiers.shift;
					let scale = if fine { 0.1 } else { 1.0 };
					let width = event.bounds.size.width;
					let t = if f32::from(width) > 0.0 {
						(drag.origin_fraction
							+ (f32::from(event.event.position.x) - drag.first_x) as f64
								/ f32::from(width) as f64
								* scale)
							.clamp(0.0, 1.0)
					} else {
						drag.origin_fraction
					};
					drop(drag);
					this.model.set_fraction(t);
					cx.notify();
				},
			))
			// Finish the gesture on mouse up — wherever it lands (gpui only
			// delivers drop events to the hovered target, so finishing via
			// `on_drop` lost the gesture when the cursor ended outside the
			// track). Both the inside and the outside variants gate on
			// `dragging`, so only a gesture this slider started completes.
			.on_mouse_up(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
					this.finish_drag(cx);
				}),
			)
			.on_mouse_up_out(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
					this.finish_drag(cx);
				}),
			)
			.on_click(
				cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
					if event.click_count() >= 2 {
						this.begin_edit(window, cx);
					}
				}),
			)
			.on_mouse_down(MouseButton::Left, {
				let click_bounds = track_bounds.clone();
				cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
					// A single click jumps to the click position. Double
					// clicks skip the jump so the editor opens at the value
					// that was already there.
					if event.click_count >= 2 || this.edit.is_some() {
						return;
					}
					let Some(bounds) = click_bounds.read().unwrap().as_ref().copied() else {
						return;
					};
					let width = f32::from(bounds.size.width);
					if width <= 0.0 {
						return;
					}
					let t =
						((f32::from(event.position.x) - f32::from(bounds.left())) / width) as f64;
					let changed = this.model.set_fraction(t);
					if changed {
						this.emit_changed(cx);
					}
				})
			})
			.on_mouse_down(
				MouseButton::Middle,
				cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
					let changed = this.model.reset();
					if changed {
						this.emit_changed(cx);
					}
				}),
			)
			.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
				let dir = match event.delta {
					ScrollDelta::Pixels(p) => {
						if f32::from(p.y) > 0.0 {
							1
						} else {
							-1
						}
					}
					ScrollDelta::Lines(l) => {
						if l.y > 0.0 {
							1
						} else {
							-1
						}
					}
				};
				let fine = event.modifiers.shift;
				let changed = this.model.apply_step(dir, fine);
				if changed {
					this.emit_changed(cx);
				}
			}))
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
				// While the numeric editor is open, Escape cancels the edit
				// (blur still fires; edit_cancel makes it restore).
				if this.edit.is_some() {
					if event.keystroke.key.as_str() == "escape" {
						this.edit_cancel = true;
					}
					return;
				}
				let fine = event.keystroke.modifiers.shift;
				let changed = match event.keystroke.key.as_str() {
					"left" => this.model.apply_step(-1, fine),
					"right" => this.model.apply_step(1, fine),
					"home" => this.model.set_fraction(0.0),
					"end" => this.model.set_fraction(1.0),
					_ => return,
				};
				if changed {
					this.emit_changed(cx);
				}
				cx.stop_propagation();
			}));

		// Invisible overlay recording the track's bounds each frame so the
		// click-to-jump handler above can map a click x to a value.
		let record_bounds = track_bounds.clone();
		track = track.child(
			canvas(
				move |bounds, _window, _cx| {
					*record_bounds.write().unwrap() = Some(bounds);
					bounds
				},
				|_bounds, _content, _window, _cx| {},
			)
			.absolute()
			.left(px(0.0))
			.right(px(0.0))
			.top(px(0.0))
			.bottom(px(0.0)),
		);

		if let Some(editor) = self.edit.clone() {
			// Numeric editor: an app text input bound to the editor state.
			track = track.child(
				oakui::component::text_input(
					gpui_kit::ElementId::named_usize("oak-slider-edit", control),
					cx,
				)
				.state(editor.downgrade())
				.accepts_input(true),
			);
		} else {
			track = track
				.child(
					div()
						.absolute()
						.left(px(0.0))
						.top(px(0.0))
						.bottom(px(0.0))
						.w(px(handle_x))
						.rounded_md()
						.bg(colors.selected),
				)
				.child(
					div()
						.absolute()
						.left(px(handle_x))
						.top(px(3.0))
						.size(px(12.0))
						.rounded_full()
						.bg(colors.text),
				);
		}

		// Persistent value box: shows the current value and accepts typed
		// input (Enter commits). The text is re-synced to the model's
		// display format whenever the box isn't focused.
		let display_text = self.display();
		let value_editor = self.value_editor.clone();
		if !value_editor.read(cx).focus_handle(cx).is_focused(window)
			&& value_editor.read(cx).value() != display_text.as_ref()
		{
			value_editor.update(cx, |editor, cx| {
				editor.set_value(display_text.as_ref(), window, cx);
			});
		}
		let value_box = div()
			.id(ElementId::named_usize("oak-slider-value", control))
			.w(px(56.0))
			.flex_shrink_0()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.px_1()
			.flex()
			.items_center()
			.child(
				oakui::component::text_input(
					ElementId::named_usize("oak-slider-value-input", control),
					cx,
				)
				.state(value_editor.downgrade())
				.accepts_input(true),
			)
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
				if event.keystroke.key.as_str() != "enter" {
					return;
				}
				let text = this.value_editor.read(cx).value().to_string();
				this.apply_text(&text, cx);
				// Move focus to the slider so the box re-syncs to the
				// normalized display text on the next frame.
				window.focus(&this.focus, cx);
				cx.stop_propagation();
			}));

		div()
			.flex()
			.items_center()
			.gap_1()
			.child(track)
			.child(value_box)
	}
}

/// Formats a float without trailing zeros.
///
/// Whole numbers print with no decimal point (`1`), other values are
/// rounded to three decimals and stripped of trailing zeros (`0.5`,
/// `0.25`). Negative zero follows the whole-number branch and prints `-0`.
///
/// # Arguments
///
/// * `v` - Value to format.
fn format_value(v: f64) -> String {
	if v.fract() == 0.0 {
		format!("{v:.0}")
	} else {
		format!("{v:.3}")
			.trim_end_matches('0')
			.trim_end_matches('.')
			.to_string()
	}
}

// ---------------------------------------------------------------------------
// Check box
// ---------------------------------------------------------------------------

/// A checkbox state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
	/// The box is empty.
	Unchecked,
	/// The box is filled with a check mark.
	Checked,
	/// The box shows a horizontal bar (partially checked).
	Indeterminate,
}

/// Events emitted by a [`CheckBox`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckBoxEvent {
	/// The user clicked (or pressed space/enter on) the box — the state
	/// the control *would* move to. The host applies it through its model
	/// and calls [`CheckBox::set_state`] when it accepts.
	Toggled {
		/// The control's stable id.
		control: usize,
		/// The state the control should move to.
		state: CheckState,
	},
}

/// A checkbox row: box + check mark.
///
/// Mouse: left click toggles. Keyboard (once focused): Space / Enter
/// toggle. Request-only: the widget never changes its own state on
/// interaction — it emits [`CheckBoxEvent::Toggled`] and the host calls
/// [`CheckBox::set_state`] to accept.
pub struct CheckBox {
	control: usize,
	state: CheckState,
	label: Option<SharedString>,
	focus: FocusHandle,
}

impl CheckBox {
	/// Creates a checkbox row.
	///
	/// Request-only: the box never changes its own state; it emits
	/// [`CheckBoxEvent::Toggled`] and waits for [`CheckBox::set_state`].
	///
	/// # Arguments
	///
	/// * `control` - Stable control id reported in events and used in element ids.
	/// * `state` - Initial visual state.
	/// * `_window` - Unused; kept for constructor symmetry with the other controls.
	/// * `cx` - Context used to create the focus handle.
	pub fn new(
		control: usize,
		state: CheckState,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			state,
			label: None,
			focus: cx.focus_handle(),
		}
	}

	/// Sets the label shown next to the box.
	///
	/// Builder-style: consumes and returns `self`. Without it the row shows only
	/// the box.
	///
	/// # Arguments
	///
	/// * `label` - Text displayed to the right of the box.
	pub fn with_label(mut self, label: impl Into<SharedString>) -> Self {
		self.label = Some(label.into());
		self
	}

	/// Returns the current visual state.
	///
	/// This is the state last accepted through [`CheckBox::set_state`], not the
	/// state the next click would request.
	pub fn state(&self) -> CheckState {
		self.state
	}

	/// Applies an accepted state to the checkbox (the host's acceptance path).
	///
	/// The control emits nothing; callers that need a repaint must notify the
	/// context themselves (`_cx` is unused here).
	///
	/// # Arguments
	///
	/// * `state` - State to display.
	/// * `_cx` - Unused; kept for symmetry with the other controls' setters.
	pub fn set_state(&mut self, state: CheckState, _cx: &mut Context<Self>) {
		self.state = state;
	}

	fn request_toggle(&mut self, cx: &mut Context<Self>) {
		let next = match self.state {
			CheckState::Checked => CheckState::Unchecked,
			_ => CheckState::Checked,
		};
		cx.emit(CheckBoxEvent::Toggled {
			control: self.control,
			state: next,
		});
	}
}

/// The checkbox's fill: the theme selection colour when checked, opaque
/// white when not (the design's unchecked box is a white well with a
/// black border — theme-independent, so it reads the same on the dark
/// panel where the theme border is a light blue).
/// Kept as a pure function so the unit test below pins the colours without
/// needing pixel reads.
fn checkbox_fill(colors: &gpui_kit::colors::Colors, checked: bool) -> gpui_kit::Rgba {
	if checked {
		colors.selected
	} else {
		gpui_kit::rgba(0xFFFFFF)
	}
}

/// The checkbox's border: black around the white unchecked box (the theme
/// border reads as light blue on the dark panel), the theme border when
/// checked.
fn checkbox_border(colors: &gpui_kit::colors::Colors, checked: bool) -> gpui_kit::Rgba {
	if checked {
		colors.border
	} else {
		gpui_kit::rgba(0x000000)
	}
}

impl EventEmitter<CheckBoxEvent> for CheckBox {}

impl Render for CheckBox {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let checked = self.state == CheckState::Checked;
		let control = self.control;

		let label = self.label.clone();
		div()
			.id(ElementId::named_usize("oak-checkbox", control))
			.flex()
			.items_center()
			.gap_1()
			.cursor_pointer()
			.group(format!("oak-checkbox-{control}"))
			.child(
				div()
					.size(px(16.0))
					.rounded_md()
					.border_1()
					.border_color(checkbox_border(&colors, checked))
					.group_hover(format!("oak-checkbox-{control}"), |style| {
						style.border_color(colors.selected)
					})
					.bg(checkbox_fill(&colors, checked))
					.shadow(if checked {
						vec![]
					} else {
						// Inset shadow makes the unchecked box read as an
						// empty well on the dark panel.
						vec![gpui_kit::BoxShadow {
							color: gpui_kit::Hsla {
								h: 0.0,
								s: 0.0,
								l: 0.0,
								a: 0.25,
							},
							offset: Point::new(px(0.0), px(1.0)),
							blur_radius: px(2.0),
							spread_radius: px(0.0),
							inset: true,
						}]
					})
					.track_focus(&self.focus)
					.cursor_pointer()
					.on_mouse_down(
						MouseButton::Left,
						cx.listener(|this, _event: &MouseDownEvent, window, cx| {
							window.focus(&this.focus, cx);
							this.request_toggle(cx);
						}),
					)
					.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
						if !matches!(event.keystroke.key.as_str(), "space" | "enter") {
							return;
						}
						this.request_toggle(cx);
						cx.stop_propagation();
					}))
					.child(if checked {
						div()
							.w_full()
							.h_full()
							.flex()
							.items_center()
							.justify_center()
							.child(crate::widgets::glyphs::icon(
								crate::widgets::glyphs::Glyph::Check,
								colors.selected_text,
							))
							.into_any_element()
					} else {
						div().into_any_element()
					}),
			)
			.child(
				label
					.map(|l| div().text_color(colors.text).child(l).into_any_element())
					.unwrap_or_else(|| div().into_any_element()),
			)
	}
}

// ---------------------------------------------------------------------------
// Combo box
// ---------------------------------------------------------------------------

/// One combo-box option: its index and display label.
#[derive(Debug, Clone)]
pub struct ComboBoxOption {
	/// The option's value (what [`ComboBoxEvent::Selected`] reports).
	pub index: usize,
	/// The display label.
	pub label: SharedString,
}

impl ComboBoxOption {
	/// Creates a combo-box option.
	///
	/// `index` is the value reported by [`ComboBoxEvent::Selected`] (not the
	/// option's position); `label` is the text shown in the button and popup.
	///
	/// # Arguments
	///
	/// * `index` - Caller-defined value for this option.
	/// * `label` - Display label; converted into a `SharedString`.
	pub fn new(index: usize, label: impl Into<SharedString>) -> Self {
		Self {
			index,
			label: label.into(),
		}
	}
}

/// Events emitted by a [`ComboBox`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboBoxEvent {
	/// An option was selected (popup click or arrow-key commit).
	Selected { value: usize },
}

/// A combo box: a button showing the selected label that opens a popup
/// list.
///
/// Mouse: click the button to open/close; click an option to select it.
/// Keyboard (once focused): Up/Down move the selection (when the popup
/// is open they move the highlight, otherwise they change the selection
/// directly), Enter commits the highlighted option (or opens the popup
/// when closed), Escape closes the popup.
pub struct ComboBox {
	control: usize,
	options: Vec<ComboBoxOption>,
	selected: Option<usize>,
	placeholder: SharedString,
	open: bool,
	highlight: usize,
	focus: FocusHandle,
}

impl ComboBox {
	/// Creates a combo box over a fixed option list.
	///
	/// Initial state: nothing selected (the placeholder shows) and the popup
	/// closed. The host must call [`ComboBox::set_selected`] to reflect its own
	/// model.
	///
	/// # Arguments
	///
	/// * `control` - Stable control id used in element ids.
	/// * `options` - Options in display order; their `index` values are what
	///   selection events report.
	/// * `_window` - Unused; kept for constructor symmetry.
	/// * `cx` - Context used to create the focus handle.
	pub fn new(
		control: usize,
		options: Vec<ComboBoxOption>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			options,
			selected: None,
			placeholder: SharedString::default(),
			open: false,
			highlight: 0,
			focus: cx.focus_handle(),
		}
	}

	/// Sets the text shown while nothing is selected.
	///
	/// Builder-style: consumes and returns `self`.
	///
	/// # Arguments
	///
	/// * `placeholder` - Text displayed when `selected` is `None`.
	pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
		self.placeholder = placeholder.into();
		self
	}

	/// The selected option index.
	pub fn selected(&self) -> Option<usize> {
		self.selected
	}

	/// Replaces the option list and clears the selection.
	///
	/// Used when a container/format switch rebuilds the compatible codec list.
	/// After the replacement the selection is `None` (the placeholder shows),
	/// the highlight resets to the first row, any open popup closes, and the
	/// control repaints.
	///
	/// # Arguments
	///
	/// * `options` - New options in display order.
	/// * `cx` - Context notified for the repaint.
	pub fn set_options(&mut self, options: Vec<ComboBoxOption>, cx: &mut Context<Self>) {
		self.options = options;
		self.selected = None;
		self.highlight = 0;
		self.open = false;
		cx.notify();
	}

	/// Sets the selected option index from an external sync.
	///
	/// The host calls this to reflect its model; nothing is emitted. The value
	/// is stored as-is (it indexes `options` when rendering, so an out-of-range
	/// index renders as no selection) and no repaint is requested — the caller
	/// notifies.
	///
	/// # Arguments
	///
	/// * `selected` - New selected option index, or `None` for no selection.
	/// * `_cx` - Unused; kept for symmetry with the other controls' setters.
	pub fn set_selected(&mut self, selected: Option<usize>, _cx: &mut Context<Self>) {
		self.selected = selected;
	}

	/// Selects an option, closes the popup and broadcasts the choice.
	///
	/// Emits [`ComboBoxEvent::Selected`] with the option's value and repaints.
	///
	/// # Arguments
	///
	/// * `index` - Value to select; callers pass an index from `options` (no
	///   bounds check is performed here).
	/// * `cx` - Context used to emit the event and repaint.
	fn select(&mut self, index: usize, cx: &mut Context<Self>) {
		self.selected = Some(index);
		self.open = false;
		cx.emit(ComboBoxEvent::Selected { value: index });
		cx.notify();
	}

	/// Moves the selection by `dir` steps, wrapping around the option list.
	///
	/// Starts from the current selection (or the first option when nothing is
	/// selected) and wraps with `rem_euclid`, then commits through
	/// [`ComboBox::select`] (which emits the event and repaints). An empty
	/// option list is a no-op.
	///
	/// # Arguments
	///
	/// * `dir` - Signed step: positive moves forward, negative backward.
	/// * `cx` - Context used to commit the selection.
	fn move_selection(&mut self, dir: i32, cx: &mut Context<Self>) {
		if self.options.is_empty() {
			return;
		}
		let current = self.selected.unwrap_or(0);
		let next = ((current as i32 + dir).rem_euclid(self.options.len() as i32)) as usize;
		self.select(next, cx);
	}
}

impl EventEmitter<ComboBoxEvent> for ComboBox {}

impl Render for ComboBox {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let control = self.control;
		let label = self
			.selected
			.and_then(|i| self.options.get(i))
			.map(|o| o.label.clone())
			.unwrap_or_else(|| self.placeholder.clone());
		let open = self.open;
		let options = self.options.clone();
		let highlight = self.highlight;
		let selected = self.selected;

		let mut root = div()
			.id(ElementId::named_usize("oak-combo", control))
			.relative()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.hover(|style| style.border_color(colors.selected))
			.px_2()
			.py_1()
			.track_focus(&self.focus)
			.cursor_pointer()
			.text_color(colors.text)
			.child(label.clone())
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseDownEvent, window, cx| {
					window.focus(&this.focus, cx);
					this.open = !this.open;
					this.highlight = this.selected.unwrap_or(0);
					cx.notify();
				}),
			)
			// A click anywhere outside the combo closes the open popup (the
			// dropdown-dismiss users expect; the toggle click above is inside,
			// so it never triggers this).
			.on_mouse_down_out(cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
				if this.open {
					this.open = false;
					cx.notify();
				}
			}))
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
				match event.keystroke.key.as_str() {
					"down" | "up" => {
						let dir = if event.keystroke.key.as_str() == "down" {
							1
						} else {
							-1
						};
						if this.open {
							if !this.options.is_empty() {
								this.highlight = ((this.highlight as i32 + dir)
									.rem_euclid(this.options.len() as i32))
									as usize;
								cx.notify();
							}
						} else {
							this.move_selection(dir, cx);
						}
					}
					"enter" | "space" => {
						if this.open {
							if let Some(opt) = this.options.get(this.highlight) {
								this.select(opt.index, cx);
							}
						} else {
							this.open = true;
							this.highlight = this.selected.unwrap_or(0);
							cx.notify();
						}
					}
					"escape" => {
						if this.open {
							this.open = false;
							cx.notify();
						}
					}
					_ => return,
				}
				cx.stop_propagation();
			}));

		if open {
			// deferred(): the popup must paint AFTER the rows that follow the
			// combo in the dialog, otherwise they draw over it and the list
			// looks transparent (the preferences render-backend dropdown
			// regression).
			root = root.child(gpui_kit::deferred(
				div()
					.absolute()
					.left(px(0.0))
					.right(px(0.0))
					.top(px(22.0))
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.bg(colors.background)
					.shadow_md()
					// Block clicks on the dropdown's padding/empty areas
					// from falling through to the controls below (this is
					// a plain deferred div, not an anchored element, so it
					// gets no automatic BlockMouse hitbox).
					.occlude()
					.py_1()
					.children(options.iter().enumerate().map(|(i, opt)| {
						let highlighted = i == highlight;
						div()
							.px_2()
							.py_1()
							.bg(if highlighted {
								colors.selected
							} else {
								colors.background
							})
							.text_color(if highlighted {
								colors.selected_text
							} else {
								colors.text
							})
							.cursor_pointer()
							// The highlight follows the mouse (standard combo
							// behaviour; Enter then commits the hovered row).
							.on_mouse_move(cx.listener(
								move |this, _event: &gpui_kit::MouseMoveEvent, _window, cx| {
									if this.highlight != i {
										this.highlight = i;
										cx.notify();
									}
								},
							))
							.on_mouse_down(MouseButton::Left, {
								let opt = opt.clone();
								cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
									window.focus(&this.focus, cx);
									this.select(opt.index, cx);
								})
							})
							.child(opt.label.clone())
					})),
			));
		}
		let _ = selected;
		root
	}
}

// ---------------------------------------------------------------------------
// Spin box
// ---------------------------------------------------------------------------

/// Events emitted by a [`SpinBox`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpinBoxEvent {
	/// The value changed (wheel or arrow key).
	ValueChanged {
		/// The control's stable id.
		control: usize,
		/// The new value.
		value: SliderValue,
	},
	/// Direct text entry was committed (the field is wheel/arrow driven
	/// for now, so this is only emitted by hosts that add a text editor).
	EditCommitted {
		/// The control's stable id.
		control: usize,
		/// The committed value.
		value: SliderValue,
	},
}

/// A numeric field over a [`SliderModel`].
///
/// Mouse: click focuses, double click opens a numeric editor (the app's
/// text input; commit on blur/Enter, Escape cancels), the wheel steps ONLY
/// while the field is focused (Shift = fine) — hover-wheel is deliberately
/// inert so two-finger trackpad scrolling across a dialog never drifts the
/// value. Keyboard (once focused): Up/Down step, Shift-Up/Down fine-step,
/// Home/End jump to the range ends, typing a digit starts editing.
pub struct SpinBox {
	control: usize,
	model: SliderModel,
	focus: FocusHandle,
	/// The numeric editor, while a direct-entry edit is open.
	edit: Option<Entity<InputState>>,
	/// Whether the open editor should cancel (Escape) instead of commit
	/// on blur.
	edit_cancel: bool,
}

impl SpinBox {
	/// Creates a numeric spin box over `model`.
	///
	/// The widget has no inline text box (unlike [`Slider`]); typed values
	/// arrive through the double-click editor. Initial state: no editor open.
	///
	/// # Arguments
	///
	/// * `control` - Stable control id reported in events and used in element ids.
	/// * `model` - Range/value model; owned by the control.
	/// * `_window` - Unused; kept for constructor symmetry with the other controls.
	/// * `cx` - Context used to create the focus handle.
	pub fn new(
		control: usize,
		model: SliderModel,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			model,
			focus: cx.focus_handle(),
			edit: None,
			edit_cancel: false,
		}
	}

	/// Sets the field's value from an external sync.
	///
	/// This is the host's write path (engine read-back / panel re-sync): the
	/// value is stored in the model silently. It does not emit
	/// [`SpinBoxEvent`] or repaint; the caller notifies if a repaint is needed.
	///
	/// # Arguments
	///
	/// * `value` - New model value, in the model's own kind/range.
	/// * `_cx` - Unused; kept for symmetry with the other controls' setters.
	pub fn set_value(&mut self, value: SliderValue, _cx: &mut Context<Self>) {
		self.model.set_value(value);
	}

	/// Replaces the spin box's range/step/value model and repaints.
	///
	/// Closes any active direct-entry editor so stale text cannot commit
	/// against the new bounds.
	pub fn set_model(&mut self, model: SliderModel, cx: &mut Context<Self>) {
		self.model = model;
		self.edit = None;
		self.edit_cancel = false;
		cx.notify();
	}

	/// The current value.
	pub fn value(&self) -> SliderValue {
		self.model.value()
	}

	/// Emits a value-changed event when the model actually moved.
	///
	/// A `false` `changed` is a no-op (no event, no repaint), so gestures that
	/// clamp back to the same value stay silent.
	///
	/// # Arguments
	///
	/// * `changed` - Whether the model's value changed in this gesture.
	/// * `cx` - Context used to emit [`SpinBoxEvent::ValueChanged`] and repaint.
	fn emit_changed(&mut self, changed: bool, cx: &mut Context<Self>) {
		if changed {
			cx.emit(SpinBoxEvent::ValueChanged {
				control: self.control,
				value: self.model.value(),
			});
			cx.notify();
		}
	}

	/// Open the numeric editor; `seed` replaces the text (a typed digit
	/// starts a fresh entry, double-click edits the current value).
	fn begin_edit(&mut self, seed: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
		let text = seed.unwrap_or_else(|| self.display().to_string());
		let editor = cx.new(|cx| InputState::new(window, cx));
		editor.update(cx, |editor, cx| {
			editor.set_value(text, window, cx);
		});
		let weak = editor.downgrade();
		let focus = editor.read(cx).focus_handle(cx);
		cx.on_focus_out(
			&focus,
			window,
			move |this: &mut Self, _event, _window, cx| {
				if let Some(editor) = weak.upgrade() {
					let text = editor.read(cx).value().to_string();
					if !this.edit_cancel {
						this.apply_text(&text, cx);
					}
				}
				this.edit = None;
				this.edit_cancel = false;
				cx.notify();
			},
		)
		.detach();
		window.focus(&editor.read(cx).focus_handle(cx), cx);
		self.edit = Some(editor);
		cx.notify();
	}

	/// Parses typed text, clamps it through the model and reports a change.
	///
	/// Integer-valued models parse as `i64`; all other kinds parse as `f64`.
	/// Unparsable text is ignored silently. A real change is reported as
	/// [`SpinBoxEvent::ValueChanged`] (the same path as wheel/arrow gestures)
	/// when the model actually moved.
	///
	/// # Arguments
	///
	/// * `text` - Raw editor text; surrounding whitespace is ignored.
	/// * `cx` - Context used to emit the change and repaint.
	fn apply_text(&mut self, text: &str, cx: &mut Context<Self>) {
		let parsed: Option<f64> = match self.model.value() {
			SliderValue::Integer(_) => text.trim().parse::<i64>().ok().map(|v| v as f64),
			_ => text.trim().parse::<f64>().ok(),
		};
		let Some(raw) = parsed else {
			return;
		};
		// The commit reports through ValueChanged like every other gesture
		// (the hosts subscribe to it; EditCommitted stays for API parity
		// with the slider-less hosts).
		let changed = self.model.apply_raw(raw);
		self.emit_changed(changed, cx);
	}

	/// Format the current value for display.
	fn display(&self) -> SharedString {
		match self.model.value() {
			SliderValue::Integer(v) => v.to_string().into(),
			SliderValue::Float(v) => format_value(v).into(),
			SliderValue::Angle(v) => format!("{v:.1}°").into(),
			SliderValue::Rational(r) => format!("{}/{}", r.num(), r.den()).into(),
		}
	}
}

impl EventEmitter<SpinBoxEvent> for SpinBox {}

impl Render for SpinBox {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let control = self.control;
		let text = self.display();

		let mut root = div()
			.id(ElementId::named_usize("oak-spinbox", control))
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.px_2()
			.py_1()
			.track_focus(&self.focus)
			.cursor_text()
			.text_color(colors.text)
			// While editing, the text input replaces the readout (not stacks
			// under it).
			.when(self.edit.is_none(), |root| root.child(text))
			.on_click(
				cx.listener(|this, event: &gpui_kit::ClickEvent, window, cx| {
					if event.click_count() >= 2 {
						this.begin_edit(None, window, cx);
					}
				}),
			)
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseDownEvent, window, cx| {
					window.focus(&this.focus, cx);
				}),
			)
			.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
				// Touchpad-safe: the wheel only steps a FOCUSED field, so
				// two-finger scrolling over a dialog never drifts values.
				if this.edit.is_some() || !this.focus.is_focused(window) {
					return;
				}
				let dir = match event.delta {
					ScrollDelta::Pixels(p) => {
						if f32::from(p.y) > 0.0 {
							1
						} else {
							-1
						}
					}
					ScrollDelta::Lines(l) => {
						if l.y > 0.0 {
							1
						} else {
							-1
						}
					}
				};
				let fine = event.modifiers.shift;
				let changed = this.model.apply_step(dir, fine);
				this.emit_changed(changed, cx);
			}))
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
				// While the numeric editor is open, Escape cancels the edit
				// (blur still fires; edit_cancel makes it restore).
				if this.edit.is_some() {
					if event.keystroke.key.as_str() == "escape" {
						this.edit_cancel = true;
					}
					return;
				}
				let fine = event.keystroke.modifiers.shift;
				let changed = match event.keystroke.key.as_str() {
					"up" => this.model.apply_step(1, fine),
					"down" => this.model.apply_step(-1, fine),
					"home" => this.model.set_fraction(0.0),
					"end" => this.model.set_fraction(1.0),
					_ => {
						// Typing a number key (or sign/point) starts editing
						// with that character — click, then just type.
						let key = &event.keystroke.key;
						let starts_number = key.len() == 1
							&& key
								.chars()
								.next()
								.is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '.')
							&& event.keystroke.modifiers == Modifiers::none();
						if starts_number {
							this.begin_edit(Some(key.clone()), window, cx);
						}
						return;
					}
				};
				this.emit_changed(changed, cx);
				cx.stop_propagation();
			}));

		if let Some(editor) = self.edit.clone() {
			// Numeric editor: an app text input bound to the editor state.
			root = root.child(
				oakui::component::text_input(
					ElementId::named_usize("oak-spinbox-edit", control),
					cx,
				)
				.state(editor.downgrade())
				.accepts_input(true),
			);
		}
		root
	}
}

#[cfg(test)]
mod tests;

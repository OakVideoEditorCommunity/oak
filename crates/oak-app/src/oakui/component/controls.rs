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

use gpui::{
	colors::DefaultColors,
	canvas, div, px, Context, ElementId, EventEmitter, FocusHandle, IntoElement, InteractiveElement, ParentElement,
	Entity, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent,
	Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Window, AppContext, Focusable,
};
use gpui::prelude::FluentBuilder;

pub use gpui_widgets::value::{SliderValue, ValueKind};

/// The colour swatch + deferred popup picker (the params panel's
/// implementation, surfaced here so effect controls live under
/// `oakui::component`).
pub use crate::panels::ofx_params::OfxColorPicker;

/// The value model of a [`Slider`] (range, step, current value).
pub use gpui_widgets::slider::SliderModel;

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
	fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
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
	edit: Option<Entity<gpui_elements::editable_text::EditableTextState>>,
	/// True while a drag is in flight (to report start/finish).
	dragging: bool,
	/// Whether the open editor should cancel (Escape) instead of commit
	/// on blur.
	edit_cancel: bool,
	/// The persistent value box: shows the current value and accepts typed
	/// input (Enter commits, exactly like the double-click editor).
	value_editor: Entity<gpui_elements::editable_text::EditableTextState>,
}

impl Slider {
	/// Create a slider for `control` over `model`.
	pub fn new(
		control: usize,
		model: SliderModel,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let _ = window;
		let display = Self::display_value(&model);
		let value_editor = cx.new(|cx| {
			gpui_elements::editable_text::EditableTextState::new(
				gpui_elements::editable_text::StringStorage::from(display.to_string()),
				cx,
			)
		});
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
			cx.emit(SliderEvent::DragFinished { control: self.control });
		}
	}

	/// Open the numeric editor with the current value as its text.
	fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let text = self.display();
		let editor = cx.new(|cx| {
			gpui_elements::editable_text::EditableTextState::new(
				gpui_elements::editable_text::StringStorage::from(text.to_string()),
				cx,
			)
		});
		// Commit (or cancel) when the editor loses focus.
		let this_control = self.control;
		let weak = editor.downgrade();
		let focus = editor.read(cx).focus_handle(cx);
		cx.on_focus_out(&focus, window, move |this: &mut Self, _event, _window, cx| {
			if let Some(editor) = weak.upgrade() {
				let text = editor.read(cx).as_str().to_string();
				if !this.edit_cancel {
					this.apply_text(&text, cx);
				}
			}
			this.edit = None;
			this.edit_cancel = false;
			cx.notify();
		})
		.detach();
		window.focus(&editor.read(cx).focus_handle(cx), cx);
		let _ = this_control;
		self.edit = Some(editor);
		cx.notify();
	}

	/// Parse `text` as the model's value kind, clamp it and apply + emit.
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

	/// Format a model's value for display.
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
		let track_bounds =
			std::sync::Arc::new(std::sync::RwLock::new(None::<gpui::Bounds<gpui::Pixels>>));
		let drag_payload = std::sync::Arc::new(std::sync::RwLock::new(SliderDrag {
			control,
			first_x: 0.0,
			origin_fraction: 0.0,
			first: true,
		}));

		let mut track = div()
			.id(gpui::ElementId::named_usize("oak-slider", control))
			.flex_1()
			.h(px(18.0))
			.relative()
			.rounded_md()
			.bg(colors.container)
			.track_focus(&self.focus)
			.cursor_pointer()
			.on_drag(drag_payload.clone(), |_payload, _offset, _window, cx| {
				cx.new(|_| SliderDragGhost)
			})
			.on_drag_move(
				cx.listener(|this, event: &gpui::DragMoveEvent<std::sync::Arc<std::sync::RwLock<SliderDrag>>>, _window, cx| {
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
						cx.emit(SliderEvent::DragStarted { control: this.control });
					}
					let drag = event.drag(cx).read().unwrap();
					let fine = event.event.modifiers.shift;
					let scale = if fine { 0.1 } else { 1.0 };
					let width = event.bounds.size.width;
					let t = if f32::from(width) > 0.0 {
						(drag.origin_fraction
							+ (f32::from(event.event.position.x) - drag.first_x) as f64
								/ f32::from(width) as f64 * scale)
							.clamp(0.0, 1.0)
					} else {
						drag.origin_fraction
					};
					drop(drag);
					this.model.set_fraction(t);
					cx.notify();
				}),
			)
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
			.on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
				if event.click_count() >= 2 {
					this.begin_edit(window, cx);
				}
			}))
			.on_mouse_down(
				MouseButton::Left,
				{
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
					let t = ((f32::from(event.position.x) - f32::from(bounds.left())) / width)
						as f64;
					let changed = this.model.set_fraction(t);
					if changed {
						this.emit_changed(cx);
					}
					})
				}
			)
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
					gpui::ScrollDelta::Pixels(p) => {
						if f32::from(p.y) > 0.0 { 1 } else { -1 }
					}
					gpui::ScrollDelta::Lines(l) => {
						if l.y > 0.0 { 1 } else { -1 }
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
				crate::oakui::component::text_input(
					gpui::ElementId::named_usize("oak-slider-edit", control),
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
			&& value_editor.read(cx).as_str() != display_text.as_ref()
		{
			value_editor.update(cx, |editor, cx| {
				editor.emplace(display_text.as_ref(), cx);
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
				crate::oakui::component::text_input(
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
				let text = this.value_editor.read(cx).as_str().to_string();
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

/// Format a float without trailing zeros ("0.5", "1", "0.25").
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
	/// Create a checkbox for `control` in `state`.
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

	/// A label shown next to the box.
	pub fn with_label(mut self, label: impl Into<SharedString>) -> Self {
		self.label = Some(label.into());
		self
	}

	/// The current state.
	pub fn state(&self) -> CheckState {
		self.state
	}

	/// The host's acceptance path: apply `state` (also repaints).
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
fn checkbox_fill(colors: &gpui::colors::Colors, checked: bool) -> gpui::Rgba {
	if checked {
		colors.selected
	} else {
		gpui::rgba(0xFFFFFF)
	}
}

/// The checkbox's border: black around the white unchecked box (the theme
/// border reads as light blue on the dark panel), the theme border when
/// checked.
fn checkbox_border(colors: &gpui::colors::Colors, checked: bool) -> gpui::Rgba {
	if checked {
		colors.border
	} else {
		gpui::rgba(0x000000)
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
			.child(
				div()
					.size(px(16.0))
					.rounded_md()
					.border_1()
					.border_color(checkbox_border(&colors, checked))
					.bg(checkbox_fill(&colors, checked))
					.shadow(if checked {
						vec![]
					} else {
						// Inset shadow makes the unchecked box read as an
						// empty well on the dark panel.
						vec![gpui::BoxShadow {
							color: gpui::Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.25 },
							offset: gpui::Point::new(px(0.0), px(1.0)),
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
							.child(div().text_color(colors.selected_text).child("✓"))
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
	/// Create an option with `index` and `label`.
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
	/// Create a combo box for `control` with `options`.
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

	/// The placeholder shown while nothing is selected.
	pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
		self.placeholder = placeholder.into();
		self
	}

	/// The selected option index.
	pub fn selected(&self) -> Option<usize> {
		self.selected
	}

	/// Replaces the option list (a container/format switch rebuilds the
	/// compatible codec lists); selection resets to the first entry.
	pub fn set_options(&mut self, options: Vec<ComboBoxOption>, cx: &mut Context<Self>) {
		self.options = options;
		self.selected = None;
		self.highlight = 0;
		self.open = false;
		cx.notify();
	}

	/// The selected option index (external sync; also repaints).
	pub fn set_selected(&mut self, selected: Option<usize>, _cx: &mut Context<Self>) {
		self.selected = selected;
	}

	fn select(&mut self, index: usize, cx: &mut Context<Self>) {
		self.selected = Some(index);
		self.open = false;
		cx.emit(ComboBoxEvent::Selected { value: index });
		cx.notify();
	}

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
						let dir = if event.keystroke.key.as_str() == "down" { 1 } else { -1 };
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
			root = root.child(
				gpui::deferred(
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
							.bg(if highlighted { colors.selected } else { colors.background })
							.text_color(if highlighted {
								colors.selected_text
							} else {
								colors.text
							})
							.cursor_pointer()
							.on_mouse_down(
								MouseButton::Left,
								{
									let opt = opt.clone();
									cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
										window.focus(&this.focus, cx);
										this.select(opt.index, cx);
									})
								},
							)
							.child(opt.label.clone())
					})),
				),
			);
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
	edit: Option<Entity<gpui_elements::editable_text::EditableTextState>>,
	/// Whether the open editor should cancel (Escape) instead of commit
	/// on blur.
	edit_cancel: bool,
}

impl SpinBox {
	/// Create a spin box for `control` over `model`.
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

	/// The current value (external sync).
	pub fn set_value(&mut self, value: SliderValue, _cx: &mut Context<Self>) {
		self.model.set_value(value);
	}

	/// The current value.
	pub fn value(&self) -> SliderValue {
		self.model.value()
	}

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
		let editor = cx.new(|cx| {
			gpui_elements::editable_text::EditableTextState::new(
				gpui_elements::editable_text::StringStorage::from(text),
				cx,
			)
		});
		let weak = editor.downgrade();
		let focus = editor.read(cx).focus_handle(cx);
		cx.on_focus_out(&focus, window, move |this: &mut Self, _event, _window, cx| {
			if let Some(editor) = weak.upgrade() {
				let text = editor.read(cx).as_str().to_string();
				if !this.edit_cancel {
					this.apply_text(&text, cx);
				}
			}
			this.edit = None;
			this.edit_cancel = false;
			cx.notify();
		})
		.detach();
		window.focus(&editor.read(cx).focus_handle(cx), cx);
		self.edit = Some(editor);
		cx.notify();
	}

	/// Parse `text` as the model's value kind, clamp it and apply + emit.
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
			.on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
				if event.click_count() >= 2 {
					this.begin_edit(None, window, cx);
				}
			}))
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
					gpui::ScrollDelta::Pixels(p) => {
						if f32::from(p.y) > 0.0 { 1 } else { -1 }
					}
					gpui::ScrollDelta::Lines(l) => {
						if l.y > 0.0 { 1 } else { -1 }
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
							&& key.chars().next().is_some_and(|c| {
								c.is_ascii_digit() || c == '-' || c == '.'
							}) && event.keystroke.modifiers == gpui::Modifiers::none();
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
				crate::oakui::component::text_input(
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
mod tests {
	use super::*;

	/// The unchecked box is a white well with a black border (the theme
	/// border reads as light blue on the dark panel); the checked box uses
	/// the theme selection colours. The expressions are pure functions, so
	/// the colours are pinned without pixel reads, which gpui's test
	/// renderer does not expose.
	#[test]
	fn checkbox_fill_uses_theme_colors() {
		let dark = gpui::colors::Colors::dark();
		assert_eq!(
			checkbox_fill(&dark, false),
			gpui::rgba(0xFFFFFF),
			"an unchecked box is white"
		);
		assert_eq!(
			checkbox_border(&dark, false),
			gpui::rgba(0x000000),
			"an unchecked box has a black border"
		);
		assert_eq!(
			checkbox_fill(&dark, true),
			dark.selected,
			"a checked box uses the theme selection colour"
		);
		assert_eq!(checkbox_border(&dark, true), dark.border);

		let light = gpui::colors::Colors::light();
		assert_eq!(checkbox_fill(&light, false), gpui::rgba(0xFFFFFF));
		assert_eq!(checkbox_border(&light, false), gpui::rgba(0x000000));
		assert_eq!(checkbox_fill(&light, true), light.selected);
		assert_eq!(checkbox_border(&light, true), light.border);
	}

	// ---- widget interaction boundaries -----------------------------------

	use gpui::{size, Modifiers, Subscription, TestAppContext, VisualTestContext};
	use std::sync::{Arc, Mutex};

	/// A view that renders one instance of every control, so the widget
	/// `Render` bodies execute during paint. It also subscribes to the
	/// checkbox's [`CheckBoxEvent::Toggled`]s so the interaction tests can
	/// assert the request actually fired (and with which next state), not
	/// only that the control's own state stayed put.
	struct ControlsProbe {
		slider: Entity<Slider>,
		check: Entity<CheckBox>,
		combo: Entity<ComboBox>,
		spin: Entity<SpinBox>,
		check_events: Arc<Mutex<Vec<CheckBoxEvent>>>,
		_check_subscription: Subscription,
	}

	impl Render for ControlsProbe {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div()
				.size_full()
				.flex()
				.flex_col()
				.child(self.slider.clone())
				.child(self.check.clone())
				.child(self.combo.clone())
				.child(self.spin.clone())
		}
	}

	fn probe_window(
		cx: &mut TestAppContext,
	) -> (&'static mut VisualTestContext, Entity<ControlsProbe>) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(240.0), px(180.0)), |window, cx| {
			let slider = cx.new(|cx| {
				Slider::new(
					1,
					SliderModel::new(ValueKind::Integer, 0.0, 100.0, 5.0, 10.0),
					window,
					cx,
				)
			});
			let check = cx.new(|cx| {
				CheckBox::new(2, CheckState::Unchecked, window, cx).with_label("Enable")
			});
			let combo = cx.new(|cx| {
				ComboBox::new(
					3,
					vec![
						ComboBoxOption::new(0, "One"),
						ComboBoxOption::new(1, "Two"),
					],
					window,
					cx,
				)
				.with_placeholder("Pick")
			});
			let spin = cx.new(|cx| {
				SpinBox::new(
					4,
					SliderModel::new(ValueKind::Integer, 120.0, 2160.0, 8.0, 180.0),
					window,
					cx,
				)
			});
			let check_events = Arc::new(Mutex::new(Vec::new()));
			let events = check_events.clone();
			let _check_subscription = cx.subscribe(
				&check,
				move |_probe, _check, event: &CheckBoxEvent, _cx| {
					events.lock().unwrap().push(*event);
				},
			);
			ControlsProbe {
				slider,
				check,
				combo,
				spin,
				check_events,
				_check_subscription,
			}
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("controls probe root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, probe)
	}

	/// Slider value clamping/snapping, text-edit parsing, drag reporting and
	/// the display formatting of every value kind.
	#[gpui::test]
	async fn slider_edit_drag_and_display_paths(cx: &mut TestAppContext) {
		let (visual, probe) = probe_window(cx);
		let slider = visual.read(|app| probe.read(app).slider.clone());
		visual.update(|_window, app| {
			slider.update(app, |s, cx| {
				s.set_value(SliderValue::Integer(1000));
				assert_eq!(s.value(), SliderValue::Integer(100));
				s.set_value(SliderValue::Integer(-3));
				assert_eq!(s.value(), SliderValue::Integer(0));

				s.set_model(SliderModel::new(ValueKind::Float, -1.0, 1.0, 0.25, 0.0));
				s.set_value(SliderValue::Float(0.5));
				assert_eq!(s.value(), SliderValue::Float(0.5));
				s.apply_text(" 0.75 ", cx);
				assert_eq!(s.value(), SliderValue::Float(0.75));
				s.apply_text("not-a-number", cx);
				assert_eq!(s.value(), SliderValue::Float(0.75));
				s.apply_text("9", cx);
				assert_eq!(s.value(), SliderValue::Float(1.0));
				assert_eq!(s.display().as_ref(), "1");

				// finish_drag: no-op without a gesture, then one report.
				s.finish_drag(cx);
				s.dragging = true;
				s.finish_drag(cx);
				assert!(!s.dragging);

				assert_eq!(
					Slider::display_value(&SliderModel::new(ValueKind::Integer, 0.0, 9.0, 1.0, 3.0))
						.as_ref(),
					"3"
				);
				assert_eq!(
					Slider::display_value(&SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.05, 0.5))
						.as_ref(),
					"0.5"
				);
				assert_eq!(
					Slider::display_value(&SliderModel::new(ValueKind::Float, 0.0, 2.0, 0.5, 1.0))
						.as_ref(),
					"1"
				);
				let angle =
					Slider::display_value(&SliderModel::new(ValueKind::Angle, 0.0, 360.0, 1.0, 45.0));
				assert!(angle.contains("45"), "{angle}");
				let rational =
					SliderModel::new(ValueKind::Rational, 0.0, 100.0, 1.0, 3.0).with_rational_den(25);
				assert_eq!(Slider::display_value(&rational).as_ref(), "3/25");
			});
		});
	}

	/// Spin box clamping, text-edit parsing, the no-change emit path and
	/// opening the inline editor.
	#[gpui::test]
	async fn spinbox_edit_and_emit_paths(cx: &mut TestAppContext) {
		let (visual, probe) = probe_window(cx);
		let spin = visual.read(|app| probe.read(app).spin.clone());
		visual.update(|window, app| {
			spin.update(app, |s, cx| {
				assert_eq!(s.value(), SliderValue::Integer(180));
				s.set_value(SliderValue::Integer(1000), cx);
				assert_eq!(s.value(), SliderValue::Integer(1000));
				s.set_value(SliderValue::Integer(100), cx);
				assert_eq!(s.value(), SliderValue::Integer(120), "clamps to the minimum");
				s.apply_text(" 320 ", cx);
				assert_eq!(s.value(), SliderValue::Integer(320));
				s.apply_text("garbage", cx);
				assert_eq!(s.value(), SliderValue::Integer(320), "unparseable text is ignored");
				s.apply_text("5000", cx);
				assert_eq!(s.value(), SliderValue::Integer(2160), "clamps to the maximum");
				s.emit_changed(false, cx);
				assert_eq!(s.display().as_ref(), "2160");

				s.begin_edit(Some("42".into()), window, cx);
				assert!(s.edit.is_some(), "the editor is open");
				s.edit = None;
				s.edit_cancel = false;
			});
		});
	}

	/// The checkbox's recorded `Toggled` events, in emission order.
	fn check_events(probe: &Entity<ControlsProbe>, app: &gpui::App) -> Vec<CheckBoxEvent> {
		probe.read(app).check_events.lock().unwrap().clone()
	}

	/// Combo-box wrap-around selection, option replacement and the empty
	/// list guard; checkbox request-only toggling.
	#[gpui::test]
	async fn combobox_and_checkbox_state_paths(cx: &mut TestAppContext) {
		let (visual, probe) = probe_window(cx);
		let combo = visual.read(|app| probe.read(app).combo.clone());
		let check = visual.read(|app| probe.read(app).check.clone());
		visual.update(|_window, app| {
			combo.update(app, |c, cx| {
				assert_eq!(c.selected(), None);
				c.set_selected(Some(1), cx);
				assert_eq!(c.selected(), Some(1));
				c.move_selection(1, cx);
				assert_eq!(c.selected(), Some(0), "wraps forward");
				c.move_selection(-1, cx);
				assert_eq!(c.selected(), Some(1), "wraps backward");
				c.select(0, cx);
				assert_eq!(c.selected(), Some(0));
				assert!(!c.open);

				c.set_options(vec![ComboBoxOption::new(0, "Only")], cx);
				assert_eq!(c.selected(), None, "a new option list resets selection");
				assert!(!c.open);
				c.move_selection(1, cx);
				assert_eq!(c.selected(), Some(0));

				c.set_options(Vec::new(), cx);
				c.move_selection(1, cx);
				assert_eq!(c.selected(), None, "an empty list ignores moves");
			});
		});

		// Unchecked -> the request asks the host for Checked and leaves the
		// widget's own state alone.
		visual.update(|_window, app| {
			check.update(app, |c, cx| {
				assert_eq!(c.state(), CheckState::Unchecked);
				c.request_toggle(cx);
				assert_eq!(c.state(), CheckState::Unchecked, "request-only");
			});
		});
		assert_eq!(
			visual.read(|app| check_events(&probe, app)),
			vec![CheckBoxEvent::Toggled {
				control: 2,
				state: CheckState::Checked,
			}],
			"an unchecked request emits the Checked toggle"
		);

		// Indeterminate -> also Checked.
		visual.update(|_window, app| {
			check.update(app, |c, cx| {
				c.set_state(CheckState::Indeterminate, cx);
				c.request_toggle(cx);
				assert_eq!(c.state(), CheckState::Indeterminate);
			});
		});
		assert_eq!(
			visual.read(|app| check_events(&probe, app)),
			vec![
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
			],
			"an indeterminate request also asks for Checked"
		);

		// Checked -> Unchecked.
		visual.update(|_window, app| {
			check.update(app, |c, cx| {
				c.set_state(CheckState::Checked, cx);
				c.request_toggle(cx);
				assert_eq!(c.state(), CheckState::Checked);
			});
		});
		assert_eq!(
			visual.read(|app| check_events(&probe, app)),
			vec![
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Unchecked,
				},
			],
			"a checked request asks for Unchecked"
		);
	}

	/// Every control paints in both state branches (unchecked/checked,
	/// closed/open, idle/dragging).
	#[gpui::test]
	async fn controls_render_state_branches(cx: &mut TestAppContext) {
		let (visual, probe) = probe_window(cx);
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		let (check, combo, slider) = visual.read(|app| {
			let probe = probe.read(app);
			(probe.check.clone(), probe.combo.clone(), probe.slider.clone())
		});
		visual.update(|_window, app| {
			check.update(app, |c, cx| c.set_state(CheckState::Checked, cx));
			combo.update(app, |c, cx| {
				c.set_selected(Some(0), cx);
				c.open = true;
				cx.notify();
			});
			slider.update(app, |s, cx| {
				s.dragging = true;
				s.set_value(SliderValue::Integer(50));
				cx.notify();
			});
		});
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
	}

	/// A probe that gives the slider the whole window, so pointer events
	/// land on its track.
	struct SliderProbe {
		slider: Entity<Slider>,
	}

	impl Render for SliderProbe {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.slider.clone())
		}
	}

	/// Slider pointer paths (click-to-jump, middle-click reset, wheel) and
	/// keyboard paths (steps, Home/End, unknown keys), plus the editor's
	/// open/Escape branches.
	#[gpui::test]
	async fn slider_pointer_and_keyboard_paths(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(220.0), px(48.0)), |window, cx| {
			let slider = cx.new(|cx| {
				Slider::new(
					9,
					SliderModel::new(ValueKind::Integer, 0.0, 100.0, 10.0, 50.0),
					window,
					cx,
				)
			});
			SliderProbe { slider }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("slider probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		let slider = visual.read(|app| probe.read(app).slider.clone());
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// A single left click jumps to the clicked fraction.
		visual.simulate_mouse_down(
			gpui::point(px(140.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui::point(px(140.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		assert!(visual.read(|app| slider.read(app).value().to_f64()) > 50.0);

		// Middle click resets to the model's default.
		visual.simulate_mouse_down(
			gpui::point(px(20.0), px(9.0)),
			MouseButton::Middle,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui::point(px(20.0), px(9.0)),
			MouseButton::Middle,
			Modifiers::none(),
		);
		assert!((visual.read(|app| slider.read(app).value().to_f64()) - 50.0).abs() < 1.0);

		// Wheel steps the value (line delta), fine with Shift isn't needed.
		visual.simulate_event(gpui::ScrollWheelEvent {
			position: gpui::point(px(90.0), px(9.0)),
			delta: gpui::ScrollDelta::Lines(gpui::Point::new(0.0, 1.0)),
			modifiers: Modifiers::none(),
			touch_phase: gpui::TouchPhase::Ended,
		});
		visual.simulate_event(gpui::ScrollWheelEvent {
			position: gpui::point(px(90.0), px(9.0)),
			delta: gpui::ScrollDelta::Pixels(gpui::Point::new(px(0.0), px(-3.0))),
			modifiers: Modifiers::none(),
			touch_phase: gpui::TouchPhase::Ended,
		});

		// Keyboard: focus and step / jump to the range ends.
		let focus = visual.read(|app| slider.read(app).focus.clone());
		visual.update(|window, app| {
			window.focus(&focus, app);
		});
		visual.simulate_keystrokes("right");
		visual.simulate_keystrokes("shift-left");
		visual.simulate_keystrokes("home");
		assert_eq!(visual.read(|app| slider.read(app).value()), SliderValue::Integer(0));
		visual.simulate_keystrokes("end");
		assert_eq!(visual.read(|app| slider.read(app).value()), SliderValue::Integer(100));
		// An unbound key returns without changing anything.
		visual.simulate_keystrokes("a");

		// Double-click opens the editor; Escape marks the pending edit as
		// cancelled (blur then restores instead of committing).
		visual.simulate_click(gpui::point(px(90.0), px(9.0)), Modifiers::none());
		visual.simulate_click(gpui::point(px(90.0), px(9.0)), Modifiers::none());
		visual.simulate_keystrokes("escape");
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
	}

	// ---- deeper interaction paths ----------------------------------------

	/// Simulate the second half of a double click: a mouse-up carrying
	/// `click_count: 2` (the widget test helpers always send 1).
	fn double_click_at(visual: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
		visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
		visual.simulate_event(gpui::MouseUpEvent {
			position: at,
			modifiers: Modifiers::none(),
			button: MouseButton::Left,
			click_count: 2,
		});
	}

	/// The slider's numeric editor: double click opens it (the render's
	/// editor branch paints), blur commits the typed text, and the Escape +
	/// blur pair restores the old value.
	#[gpui::test]
	async fn slider_editor_commit_and_cancel_paths(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(220.0), px(48.0)), |window, cx| {
			let slider = cx.new(|cx| {
				Slider::new(
					9,
					SliderModel::new(ValueKind::Integer, 0.0, 100.0, 10.0, 50.0),
					window,
					cx,
				)
			});
			SliderProbe { slider }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("slider probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		// Activate the test window: focus-out listeners only fire on an
		// active window (the test platform starts inactive).
		visual.update(|window, _app| window.activate_window());
		cx.run_until_parked();
		let slider = visual.read(|app| probe.read(app).slider.clone());
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Double click opens the editor.
		double_click_at(visual, gpui::point(px(90.0), px(9.0)));
		cx.run_until_parked();
		assert!(visual.read(|app| slider.read(app).edit.is_some()), "editor open");

		// A single click while the editor is open is ignored (the guard
		// keeps double clicks from jumping the value under the editor).
		visual.simulate_mouse_down(
			gpui::point(px(20.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui::point(px(20.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert!(visual.read(|app| slider.read(app).edit.is_some()));

		// The open editor's render branch paints over the track.
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Type a value, then blur: the focus-out listener commits it.
		let editor = visual
			.read(|app| slider.read(app).edit.clone())
			.expect("open editor");
		visual.update(|_window, app| {
			editor.update(app, |editor, cx| editor.emplace("75", cx));
		});
		let focus = visual.read(|app| slider.read(app).focus.clone());
		visual.update(|window, app| window.focus(&focus, app));
		cx.run_until_parked();
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert_eq!(
			visual.read(|app| slider.read(app).value()),
			SliderValue::Integer(80),
			"blur commits the typed value (snapped to the model's step)"
		);
		assert!(
			visual.read(|app| slider.read(app).edit.is_none()),
			"the editor closes on blur"
		);

		// Reopen, cancel with Escape, then blur: the old value survives.
		double_click_at(visual, gpui::point(px(90.0), px(9.0)));
		cx.run_until_parked();
		let editor = visual
			.read(|app| slider.read(app).edit.clone())
			.expect("editor reopened");
		visual.update(|_window, app| {
			editor.update(app, |editor, cx| editor.emplace("3", cx));
		});
		visual.simulate_keystrokes("escape");
		cx.run_until_parked();
		assert!(visual.read(|app| slider.read(app).edit_cancel));
		visual.update(|window, app| window.focus(&focus, app));
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert_eq!(
			visual.read(|app| slider.read(app).value()),
			SliderValue::Integer(60),
			"an Escape-cancelled edit restores the value the click jumped to"
		);
		assert!(visual.read(|app| slider.read(app).edit.is_none()));
	}

	/// The persistent value box: focusing the embedded text input and
	/// pressing Enter parses, clamps and commits the typed text.
	#[gpui::test]
	async fn slider_value_box_enter_commits(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(220.0), px(48.0)), |window, cx| {
			let slider = cx.new(|cx| {
				Slider::new(
					9,
					SliderModel::new(ValueKind::Integer, 0.0, 100.0, 10.0, 50.0),
					window,
					cx,
				)
			});
			SliderProbe { slider }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("slider probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		let slider = visual.read(|app| probe.read(app).slider.clone());
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		let value_input = visual.read(|app| slider.read(app).value_editor.clone());
		let input_focus = visual.read(|app| value_input.read(app).focus_handle(app));
		visual.update(|window, app| window.focus(&input_focus, app));
		visual.update(|_window, app| {
			value_input.update(app, |editor, cx| editor.emplace("42", cx));
		});
		visual.simulate_keystrokes("enter");
		cx.run_until_parked();
		assert_eq!(
			visual.read(|app| slider.read(app).value()),
			SliderValue::Integer(40),
			"Enter in the value box commits (snapped to the model's step)"
		);
		// The focus moved back to the slider so the box re-syncs.
		assert!(visual.update(|window, app| slider
			.read(app)
			.focus
			.is_focused(window)));
	}

	/// Two stacked sliders exercise the drag gesture: the moved slider
	/// tracks the cursor, a Shift drag is fine-grained, the other slider
	/// ignores the passing payload, and mouse-up-anywhere finishes.
	struct TwoSliderProbe {
		top: Entity<Slider>,
		bottom: Entity<Slider>,
	}

	impl Render for TwoSliderProbe {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div()
				.size_full()
				.flex()
				.flex_col()
				.child(div().flex_1().min_h_0().child(self.top.clone()))
				.child(div().flex_1().min_h_0().child(self.bottom.clone()))
		}
	}

	#[gpui::test]
	async fn slider_drag_tracks_the_cursor_and_gates_other_sliders(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(480.0), px(120.0)), |window, cx| {
			let model = || SliderModel::new(ValueKind::Integer, 0.0, 100.0, 10.0, 50.0);
			let top = cx.new(|cx| Slider::new(1, model(), window, cx));
			let bottom = cx.new(|cx| Slider::new(2, model(), window, cx));
			TwoSliderProbe { top, bottom }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("two-slider probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		let (top, bottom) = visual.read(|app| {
			let probe = probe.read(app);
			(probe.top.clone(), probe.bottom.clone())
		});
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		let move_to = |visual: &mut VisualTestContext, x: f32, y: f32, shift: bool| {
			visual.simulate_mouse_move(
				gpui::point(px(x), px(y)),
				MouseButton::Left,
				if shift { Modifiers::shift() } else { Modifiers::none() },
			);
		};

		// Drag the top slider right: gpui starts the gesture after the
		// threshold move, the following moves track the cursor 1:1.
		visual.simulate_mouse_down(
			gpui::point(px(60.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		move_to(visual, 120.0, 9.0, false);
		move_to(visual, 240.0, 9.0, false);
		move_to(visual, 360.0, 9.0, false);
		assert!(visual.read(|app| top.read(app).dragging), "drag started");
		// The drag ghost renders while the gesture is in flight.
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
		// Move over the second slider: its handler must ignore the pass.
		move_to(visual, 360.0, 69.0, false);
		cx.run_until_parked();
		assert_eq!(
			visual.read(|app| bottom.read(app).value()),
			SliderValue::Integer(50),
			"the other slider ignores the passing drag payload"
		);
		assert!(!visual.read(|app| bottom.read(app).dragging));
		visual.simulate_mouse_up(
			gpui::point(px(360.0), px(69.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert!(!visual.read(|app| top.read(app).dragging), "mouse up finished");
		let dragged = visual.read(|app| top.read(app).value().to_f64());
		assert!(dragged >= 30.0, "the drag followed the cursor: {dragged}");

		// Middle click resets; a Shift drag moves 1/10 as far.
		visual.simulate_mouse_down(
			gpui::point(px(30.0), px(9.0)),
			MouseButton::Middle,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert_eq!(visual.read(|app| top.read(app).value()), SliderValue::Integer(50));
		visual.simulate_mouse_down(
			gpui::point(px(60.0), px(9.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		move_to(visual, 120.0, 9.0, true);
		move_to(visual, 240.0, 9.0, true);
		move_to(visual, 360.0, 9.0, true);
		visual.simulate_mouse_up(
			gpui::point(px(360.0), px(9.0)),
			MouseButton::Left,
			Modifiers::shift(),
		);
		cx.run_until_parked();
		let fine = visual.read(|app| top.read(app).value().to_f64());
		assert!(
			fine <= 30.0,
			"a Shift drag is fine-grained: {fine}"
		);
	}

	/// The checkbox keyboard path: Space / Enter request the toggle (the
	/// state stays put — the host applies it), other keys are ignored.
	#[gpui::test]
	async fn checkbox_keyboard_requests_the_toggle(cx: &mut TestAppContext) {
		let (visual, probe) = probe_window(cx);
		let check = visual.read(|app| probe.read(app).check.clone());
		let focus = visual.read(|app| check.read(app).focus.clone());
		visual.update(|window, app| window.focus(&focus, app));

		visual.simulate_keystrokes("space");
		assert_eq!(
			visual.read(|app| check_events(&probe, app)),
			vec![CheckBoxEvent::Toggled {
				control: 2,
				state: CheckState::Checked,
			}],
			"Space requests the toggle"
		);
		assert_eq!(
			visual.read(|app| check.read(app).state()),
			CheckState::Unchecked,
			"request-only: the box never toggles itself"
		);

		visual.simulate_keystrokes("enter");
		assert_eq!(
			visual.read(|app| check_events(&probe, app)),
			vec![
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
				CheckBoxEvent::Toggled {
					control: 2,
					state: CheckState::Checked,
				},
			],
			"Enter requests the toggle too (still Unchecked, so still Checked)"
		);

		// An unbound key must not emit a toggle.
		visual.simulate_keystrokes("a");
		assert_eq!(
			visual.read(|app| check_events(&probe, app)).len(),
			2,
			"an unbound key does not request a toggle"
		);
		assert_eq!(
			visual.read(|app| check.read(app).state()),
			CheckState::Unchecked,
			"the state never moved"
		);
	}

	/// A probe that gives the combo box the top-left corner, so the popup
	/// coordinates are predictable.
	struct ComboProbe {
		combo: Entity<ComboBox>,
	}

	impl Render for ComboProbe {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.combo.clone())
		}
	}

	#[gpui::test]
	async fn combobox_pointer_and_keyboard_selection_paths(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(240.0), px(200.0)), |window, cx| {
			let combo = cx.new(|cx| {
				ComboBox::new(
					3,
					vec![
						ComboBoxOption::new(0, "One"),
						ComboBoxOption::new(1, "Two"),
						ComboBoxOption::new(2, "Three"),
					],
					window,
					cx,
				)
				.with_placeholder("Pick")
			});
			ComboProbe { combo }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("combo probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		let combo = visual.read(|app| probe.read(app).combo.clone());
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Clicking the button opens the popup; the highlight follows the
		// current selection.
		visual.simulate_mouse_down(
			gpui::point(px(20.0), px(12.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui::point(px(20.0), px(12.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert!(visual.read(|app| combo.read(app).open), "click opened the popup");
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Keyboard while open: Down/Up move the highlight (which wraps),
		// Enter commits the highlighted option and closes.
		let focus = visual.read(|app| combo.read(app).focus.clone());
		visual.update(|window, app| window.focus(&focus, app));
		visual.simulate_keystrokes("down");
		visual.simulate_keystrokes("up");
		visual.simulate_keystrokes("up");
		assert_eq!(
			visual.read(|app| combo.read(app).highlight),
			2,
			"Up wrapped the highlight to the last option"
		);
		visual.simulate_keystrokes("enter");
		assert_eq!(visual.read(|app| combo.read(app).selected()), Some(2));
		assert!(!visual.read(|app| combo.read(app).open));

		// Keyboard while closed: Down moves the selection directly; an
		// unbound key is ignored.
		visual.simulate_keystrokes("down");
		assert_eq!(visual.read(|app| combo.read(app).selected()), Some(0), "wrap");
		visual.simulate_keystrokes("a");
		assert_eq!(visual.read(|app| combo.read(app).selected()), Some(0));

		// Space opens the popup when closed; Escape closes it again.
		visual.simulate_keystrokes("space");
		assert!(visual.read(|app| combo.read(app).open));
		visual.simulate_keystrokes("escape");
		assert!(!visual.read(|app| combo.read(app).open));

		// A click on a dropdown option selects it (the popup sits 22px
		// below the button with 4px of padding and ~22.5px rows).
		visual.simulate_keystrokes("space");
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
		let mut clicked = None;
		for offset in 30..60 {
			visual.simulate_mouse_down(
				gpui::point(px(20.0), px(offset as f32)),
				MouseButton::Left,
				Modifiers::none(),
			);
			visual.simulate_mouse_up(
				gpui::point(px(20.0), px(offset as f32)),
				MouseButton::Left,
				Modifiers::none(),
			);
			if visual.read(|app| combo.read(app).selected()) == Some(0) {
				clicked = Some(offset);
				break;
			}
		}
		assert!(clicked.is_some(), "an option row was clicked");
		assert!(!visual.read(|app| combo.read(app).open), "the option click closes");

		// A click outside the open popup dismisses it without selecting.
		let before = visual.read(|app| combo.read(app).selected());
		visual.simulate_mouse_down(
			gpui::point(px(20.0), px(12.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert!(visual.read(|app| combo.read(app).open));
		visual.simulate_mouse_down(
			gpui::point(px(200.0), px(180.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui::point(px(200.0), px(180.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.run_until_parked();
		assert!(!visual.read(|app| combo.read(app).open), "outside click dismisses");
		assert_eq!(visual.read(|app| combo.read(app).selected()), before);

		// The empty-list guards: opening and pressing Down / Enter keeps
		// the popup highlight stable and selects nothing.
		visual.update(|_window, app| {
			combo.update(app, |combo, cx| combo.set_options(Vec::new(), cx));
		});
		visual.simulate_keystrokes("space");
		visual.simulate_keystrokes("down");
		visual.simulate_keystrokes("enter");
		assert_eq!(visual.read(|app| combo.read(app).selected()), None);
	}

	/// A probe that gives the spin box the top-left corner.
	struct SpinProbe {
		spin: Entity<SpinBox>,
	}

	impl Render for SpinProbe {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.spin.clone())
		}
	}

	#[gpui::test]
	async fn spinbox_scroll_keyboard_and_editor_paths(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(240.0), px(120.0)), |window, cx| {
			let spin = cx.new(|cx| {
				SpinBox::new(
					4,
					SliderModel::new(ValueKind::Float, 0.0, 2160.0, 8.0, 184.0),
					window,
					cx,
				)
			});
			SpinProbe { spin }
		});
		cx.run_until_parked();
		let probe = window.root(cx).expect("spin probe");
		let visual = VisualTestContext::from_window(window.into(), cx).into_mut();
		// Focus-out listeners only fire on an active window.
		visual.update(|window, _app| window.activate_window());
		cx.run_until_parked();
		let spin = visual.read(|app| probe.read(app).spin.clone());
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		let wheel = |visual: &mut VisualTestContext, delta: gpui::ScrollDelta| {
			visual.simulate_event(gpui::ScrollWheelEvent {
				position: gpui::point(px(20.0), px(12.0)),
				delta,
				modifiers: Modifiers::none(),
				touch_phase: gpui::TouchPhase::Ended,
			});
		};

		// Hover-wheel is inert: the field must be focused first.
		wheel(
			visual,
			gpui::ScrollDelta::Lines(gpui::Point::new(0.0, 1.0)),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(184.0));

		let focus = visual.read(|app| spin.read(app).focus.clone());
		visual.update(|window, app| window.focus(&focus, app));
		wheel(
			visual,
			gpui::ScrollDelta::Lines(gpui::Point::new(0.0, 1.0)),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(192.0));
		wheel(
			visual,
			gpui::ScrollDelta::Lines(gpui::Point::new(0.0, -1.0)),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(184.0));
		wheel(
			visual,
			gpui::ScrollDelta::Pixels(gpui::Point::new(px(0.0), px(-4.0))),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(176.0));
		wheel(
			visual,
			gpui::ScrollDelta::Pixels(gpui::Point::new(px(0.0), px(4.0))),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(184.0));

		// Keyboard: steps, fine steps, range ends, typing starts an edit.
		visual.simulate_keystrokes("up");
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(192.0));
		visual.simulate_keystrokes("shift-up");
		let fine = visual.read(|app| spin.read(app).value().to_f64());
		assert!(fine > 192.0 && fine < 194.0, "Shift = fine step: {fine}");
		visual.simulate_keystrokes("home");
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(0.0));
		visual.simulate_keystrokes("end");
		assert_eq!(visual.read(|app| spin.read(app).value()), SliderValue::Float(2160.0));
		visual.simulate_keystrokes("a");
		assert!(visual.read(|app| spin.read(app).edit.is_none()), "a letter is inert");
		visual.simulate_keystrokes("7");
		cx.run_until_parked();
		assert!(visual.read(|app| spin.read(app).edit.is_some()), "a digit starts an edit");
		// The wheel is inert while the editor is open.
		let before_wheel = visual.read(|app| spin.read(app).value());
		wheel(
			visual,
			gpui::ScrollDelta::Lines(gpui::Point::new(0.0, 1.0)),
		);
		assert_eq!(visual.read(|app| spin.read(app).value()), before_wheel);
		// The open editor renders under the readout; its text is the seed.
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});
		let editor = visual
			.read(|app| spin.read(app).edit.clone())
			.expect("open editor");
		visual.update(|_window, app| {
			editor.update(app, |editor, cx| editor.emplace("500", cx));
		});
		visual.simulate_keystrokes("escape");
		cx.run_until_parked();
		assert!(visual.read(|app| spin.read(app).edit_cancel));
		visual.update(|window, app| window.focus(&focus, app));
		cx.run_until_parked();
		assert_eq!(
			visual.read(|app| spin.read(app).value()),
			SliderValue::Float(2160.0),
			"Escape + blur does not commit"
		);
		assert!(visual.read(|app| spin.read(app).edit.is_none()));

		// Refresh the frame so the closed editor's input no longer captures
		// the next click.
		visual.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Double click opens the editor seeded with the current value; a
		// blur commits the typed replacement (clamped).
		double_click_at(visual, gpui::point(px(20.0), px(12.0)));
		cx.run_until_parked();
		let editor = visual
			.read(|app| spin.read(app).edit.clone())
			.expect("double click opened the editor");
		visual.update(|_window, app| {
			editor.update(app, |editor, cx| editor.emplace("9000", cx));
		});
		visual.update(|window, app| window.focus(&focus, app));
		cx.run_until_parked();
		assert_eq!(
			visual.read(|app| spin.read(app).value()),
			SliderValue::Float(2160.0),
			"the committed text clamps to the model's range"
		);

		// The display formats every value kind (Angle / Rational branches).
		visual.update(|_window, app| {
			spin.update(app, |spin, _cx| {
				spin.model = SliderModel::new(ValueKind::Angle, 0.0, 360.0, 1.0, 45.0);
				assert!(spin.display().contains("45"));
				spin.model = SliderModel::new(ValueKind::Rational, 0.0, 100.0, 1.0, 3.0)
					.with_rational_den(25);
				assert_eq!(spin.display().as_ref(), "3/25");
				spin.model = SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.25, 0.0);
				assert_eq!(spin.display().as_ref(), "0");
			});
		});
	}
}

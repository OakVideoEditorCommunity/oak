use super::*;

/// The unchecked box is a white well with a black border (the theme
/// border reads as light blue on the dark panel); the checked box uses
/// the theme selection colours. The expressions are pure functions, so
/// the colours are pinned without pixel reads, which gpui's test
/// renderer does not expose.
#[test]
fn checkbox_fill_uses_theme_colors() {
	let dark = gpui_kit::colors::Colors::dark();
	assert_eq!(
		checkbox_fill(&dark, false),
		gpui_kit::rgba(0xFFFFFF),
		"an unchecked box is white"
	);
	assert_eq!(
		checkbox_border(&dark, false),
		gpui_kit::rgba(0x000000),
		"an unchecked box has a black border"
	);
	assert_eq!(
		checkbox_fill(&dark, true),
		dark.selected,
		"a checked box uses the theme selection colour"
	);
	assert_eq!(checkbox_border(&dark, true), dark.border);

	let light = gpui_kit::colors::Colors::light();
	assert_eq!(checkbox_fill(&light, false), gpui_kit::rgba(0xFFFFFF));
	assert_eq!(checkbox_border(&light, false), gpui_kit::rgba(0x000000));
	assert_eq!(checkbox_fill(&light, true), light.selected);
	assert_eq!(checkbox_border(&light, true), light.border);
}

// ---- widget interaction boundaries -----------------------------------

use gpui_kit::{Modifiers, Subscription, TestAppContext, VisualTestContext, size};
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
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(240.0), px(180.0)), |window, cx| {
		let slider = cx.new(|cx| {
			Slider::new(
				1,
				SliderModel::new(ValueKind::Integer, 0.0, 100.0, 5.0, 10.0),
				window,
				cx,
			)
		});
		let check =
			cx.new(|cx| CheckBox::new(2, CheckState::Unchecked, window, cx).with_label("Enable"));
		let combo = cx.new(|cx| {
			ComboBox::new(
				3,
				vec![ComboBoxOption::new(0, "One"), ComboBoxOption::new(1, "Two")],
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
		let _check_subscription =
			cx.subscribe(&check, move |_probe, _check, event: &CheckBoxEvent, _cx| {
				events.lock().unwrap().push(*event);
			});
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
#[gpui_kit::test]
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
#[gpui_kit::test]
async fn spinbox_edit_and_emit_paths(cx: &mut TestAppContext) {
	let (visual, probe) = probe_window(cx);
	let spin = visual.read(|app| probe.read(app).spin.clone());
	visual.update(|window, app| {
		spin.update(app, |s, cx| {
			assert_eq!(s.value(), SliderValue::Integer(180));
			s.set_value(SliderValue::Integer(1000), cx);
			assert_eq!(s.value(), SliderValue::Integer(1000));
			s.set_value(SliderValue::Integer(100), cx);
			assert_eq!(
				s.value(),
				SliderValue::Integer(120),
				"clamps to the minimum"
			);
			s.apply_text(" 320 ", cx);
			assert_eq!(s.value(), SliderValue::Integer(320));
			s.apply_text("garbage", cx);
			assert_eq!(
				s.value(),
				SliderValue::Integer(320),
				"unparseable text is ignored"
			);
			s.apply_text("5000", cx);
			assert_eq!(
				s.value(),
				SliderValue::Integer(2160),
				"clamps to the maximum"
			);
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
fn check_events(probe: &Entity<ControlsProbe>, app: &gpui_kit::App) -> Vec<CheckBoxEvent> {
	probe.read(app).check_events.lock().unwrap().clone()
}

/// Combo-box wrap-around selection, option replacement and the empty
/// list guard; checkbox request-only toggling.
#[gpui_kit::test]
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
#[gpui_kit::test]
async fn controls_render_state_branches(cx: &mut TestAppContext) {
	let (visual, probe) = probe_window(cx);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let (check, combo, slider) = visual.read(|app| {
		let probe = probe.read(app);
		(
			probe.check.clone(),
			probe.combo.clone(),
			probe.slider.clone(),
		)
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
		window.draw(cx).clear(cx);
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
#[gpui_kit::test]
async fn slider_pointer_and_keyboard_paths(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	// A single left click jumps to the clicked fraction.
	visual.simulate_mouse_down(
		gpui_kit::point(px(140.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	visual.simulate_mouse_up(
		gpui_kit::point(px(140.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	assert!(visual.read(|app| slider.read(app).value().to_f64()) > 50.0);

	// Middle click resets to the model's default.
	visual.simulate_mouse_down(
		gpui_kit::point(px(20.0), px(9.0)),
		MouseButton::Middle,
		Modifiers::none(),
	);
	visual.simulate_mouse_up(
		gpui_kit::point(px(20.0), px(9.0)),
		MouseButton::Middle,
		Modifiers::none(),
	);
	assert!((visual.read(|app| slider.read(app).value().to_f64()) - 50.0).abs() < 1.0);

	// Wheel steps the value (line delta), fine with Shift isn't needed.
	visual.simulate_event(gpui_kit::ScrollWheelEvent {
		position: gpui_kit::point(px(90.0), px(9.0)),
		delta: gpui_kit::ScrollDelta::Lines(gpui_kit::Point::new(0.0, 1.0)),
		modifiers: Modifiers::none(),
		touch_phase: gpui_kit::TouchPhase::Ended,
	});
	visual.simulate_event(gpui_kit::ScrollWheelEvent {
		position: gpui_kit::point(px(90.0), px(9.0)),
		delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::Point::new(px(0.0), px(-3.0))),
		modifiers: Modifiers::none(),
		touch_phase: gpui_kit::TouchPhase::Ended,
	});

	// Keyboard: focus and step / jump to the range ends.
	let focus = visual.read(|app| slider.read(app).focus.clone());
	visual.update(|window, app| {
		window.focus(&focus, app);
	});
	visual.simulate_keystrokes("right");
	visual.simulate_keystrokes("shift-left");
	visual.simulate_keystrokes("home");
	assert_eq!(
		visual.read(|app| slider.read(app).value()),
		SliderValue::Integer(0)
	);
	visual.simulate_keystrokes("end");
	assert_eq!(
		visual.read(|app| slider.read(app).value()),
		SliderValue::Integer(100)
	);
	// An unbound key returns without changing anything.
	visual.simulate_keystrokes("a");

	// Double-click opens the editor; Escape marks the pending edit as
	// cancelled (blur then restores instead of committing).
	visual.simulate_click(gpui_kit::point(px(90.0), px(9.0)), Modifiers::none());
	visual.simulate_click(gpui_kit::point(px(90.0), px(9.0)), Modifiers::none());
	visual.simulate_keystrokes("escape");
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
}

// ---- deeper interaction paths ----------------------------------------

/// Simulate the second half of a double click: a mouse-up carrying
/// `click_count: 2` (the widget test helpers always send 1).
fn double_click_at(visual: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
	visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
	visual.simulate_event(gpui_kit::MouseUpEvent {
		position: at,
		modifiers: Modifiers::none(),
		button: MouseButton::Left,
		click_count: 2,
	});
}

/// The slider's numeric editor: double click opens it (the render's
/// editor branch paints), blur commits the typed text, and the Escape +
/// blur pair restores the old value.
#[gpui_kit::test]
async fn slider_editor_commit_and_cancel_paths(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	// Double click opens the editor.
	double_click_at(visual, gpui_kit::point(px(90.0), px(9.0)));
	cx.run_until_parked();
	assert!(
		visual.read(|app| slider.read(app).edit.is_some()),
		"editor open"
	);

	// A single click while the editor is open is ignored (the guard
	// keeps double clicks from jumping the value under the editor).
	visual.simulate_mouse_down(
		gpui_kit::point(px(20.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	visual.simulate_mouse_up(
		gpui_kit::point(px(20.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert!(visual.read(|app| slider.read(app).edit.is_some()));

	// The open editor's render branch paints over the track.
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Type a value, then blur: the focus-out listener commits it.
	let editor = visual
		.read(|app| slider.read(app).edit.clone())
		.expect("open editor");
	visual.update(|window, app| {
		editor.update(app, |editor, cx| editor.set_value("75", window, cx));
	});
	let focus = visual.read(|app| slider.read(app).focus.clone());
	visual.update(|window, app| window.focus(&focus, app));
	cx.run_until_parked();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
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
	double_click_at(visual, gpui_kit::point(px(90.0), px(9.0)));
	cx.run_until_parked();
	let editor = visual
		.read(|app| slider.read(app).edit.clone())
		.expect("editor reopened");
	visual.update(|window, app| {
		editor.update(app, |editor, cx| editor.set_value("3", window, cx));
	});
	visual.simulate_keystrokes("escape");
	cx.run_until_parked();
	assert!(visual.read(|app| slider.read(app).edit_cancel));
	visual.update(|window, app| window.focus(&focus, app));
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
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
#[gpui_kit::test]
async fn slider_value_box_enter_commits(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	let value_input = visual.read(|app| slider.read(app).value_editor.clone());
	let input_focus = visual.read(|app| value_input.read(app).focus_handle(app));
	visual.update(|window, app| window.focus(&input_focus, app));
	visual.update(|window, app| {
		value_input.update(app, |editor, cx| editor.set_value("42", window, cx));
	});
	visual.simulate_keystrokes("enter");
	cx.run_until_parked();
	assert_eq!(
		visual.read(|app| slider.read(app).value()),
		SliderValue::Integer(40),
		"Enter in the value box commits (snapped to the model's step)"
	);
	// The focus moved back to the slider so the box re-syncs.
	assert!(visual.update(|window, app| slider.read(app).focus.is_focused(window)));
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

#[gpui_kit::test]
async fn slider_drag_tracks_the_cursor_and_gates_other_sliders(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	let move_to = |visual: &mut VisualTestContext, x: f32, y: f32, shift: bool| {
		visual.simulate_mouse_move(
			gpui_kit::point(px(x), px(y)),
			MouseButton::Left,
			if shift {
				Modifiers::shift()
			} else {
				Modifiers::none()
			},
		);
	};

	// Drag the top slider right: gpui starts the gesture after the
	// threshold move, the following moves track the cursor 1:1.
	visual.simulate_mouse_down(
		gpui_kit::point(px(60.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	move_to(visual, 120.0, 9.0, false);
	move_to(visual, 240.0, 9.0, false);
	move_to(visual, 360.0, 9.0, false);
	assert!(visual.read(|app| top.read(app).dragging), "drag started");
	// The drag ghost renders while the gesture is in flight.
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
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
		gpui_kit::point(px(360.0), px(69.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert!(
		!visual.read(|app| top.read(app).dragging),
		"mouse up finished"
	);
	let dragged = visual.read(|app| top.read(app).value().to_f64());
	assert!(dragged >= 30.0, "the drag followed the cursor: {dragged}");

	// Middle click resets; a Shift drag moves 1/10 as far.
	visual.simulate_mouse_down(
		gpui_kit::point(px(30.0), px(9.0)),
		MouseButton::Middle,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert_eq!(
		visual.read(|app| top.read(app).value()),
		SliderValue::Integer(50)
	);
	visual.simulate_mouse_down(
		gpui_kit::point(px(60.0), px(9.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	move_to(visual, 120.0, 9.0, true);
	move_to(visual, 240.0, 9.0, true);
	move_to(visual, 360.0, 9.0, true);
	visual.simulate_mouse_up(
		gpui_kit::point(px(360.0), px(9.0)),
		MouseButton::Left,
		Modifiers::shift(),
	);
	cx.run_until_parked();
	let fine = visual.read(|app| top.read(app).value().to_f64());
	assert!(fine <= 30.0, "a Shift drag is fine-grained: {fine}");
}

/// The checkbox keyboard path: Space / Enter request the toggle (the
/// state stays put — the host applies it), other keys are ignored.
#[gpui_kit::test]
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

#[gpui_kit::test]
async fn combobox_pointer_and_keyboard_selection_paths(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	// Clicking the button opens the popup; the highlight follows the
	// current selection.
	visual.simulate_mouse_down(
		gpui_kit::point(px(20.0), px(12.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	visual.simulate_mouse_up(
		gpui_kit::point(px(20.0), px(12.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert!(
		visual.read(|app| combo.read(app).open),
		"click opened the popup"
	);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
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
	assert_eq!(
		visual.read(|app| combo.read(app).selected()),
		Some(0),
		"wrap"
	);
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
		window.draw(cx).clear(cx);
	});
	let mut clicked = None;
	for offset in 30..60 {
		visual.simulate_mouse_down(
			gpui_kit::point(px(20.0), px(offset as f32)),
			MouseButton::Left,
			Modifiers::none(),
		);
		visual.simulate_mouse_up(
			gpui_kit::point(px(20.0), px(offset as f32)),
			MouseButton::Left,
			Modifiers::none(),
		);
		if visual.read(|app| combo.read(app).selected()) == Some(0) {
			clicked = Some(offset);
			break;
		}
	}
	assert!(clicked.is_some(), "an option row was clicked");
	assert!(
		!visual.read(|app| combo.read(app).open),
		"the option click closes"
	);

	// A click outside the open popup dismisses it without selecting.
	let before = visual.read(|app| combo.read(app).selected());
	visual.simulate_mouse_down(
		gpui_kit::point(px(20.0), px(12.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert!(visual.read(|app| combo.read(app).open));
	visual.simulate_mouse_down(
		gpui_kit::point(px(200.0), px(180.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	visual.simulate_mouse_up(
		gpui_kit::point(px(200.0), px(180.0)),
		MouseButton::Left,
		Modifiers::none(),
	);
	cx.run_until_parked();
	assert!(
		!visual.read(|app| combo.read(app).open),
		"outside click dismisses"
	);
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

#[gpui_kit::test]
async fn spinbox_scroll_keyboard_and_editor_paths(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
	});

	let wheel = |visual: &mut VisualTestContext, delta: gpui_kit::ScrollDelta| {
		visual.simulate_event(gpui_kit::ScrollWheelEvent {
			position: gpui_kit::point(px(20.0), px(12.0)),
			delta,
			modifiers: Modifiers::none(),
			touch_phase: gpui_kit::TouchPhase::Ended,
		});
	};

	// Hover-wheel is inert: the field must be focused first.
	wheel(
		visual,
		gpui_kit::ScrollDelta::Lines(gpui_kit::Point::new(0.0, 1.0)),
	);
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(184.0)
	);

	let focus = visual.read(|app| spin.read(app).focus.clone());
	visual.update(|window, app| window.focus(&focus, app));
	wheel(
		visual,
		gpui_kit::ScrollDelta::Lines(gpui_kit::Point::new(0.0, 1.0)),
	);
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(192.0)
	);
	wheel(
		visual,
		gpui_kit::ScrollDelta::Lines(gpui_kit::Point::new(0.0, -1.0)),
	);
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(184.0)
	);
	wheel(
		visual,
		gpui_kit::ScrollDelta::Pixels(gpui_kit::Point::new(px(0.0), px(-4.0))),
	);
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(176.0)
	);
	wheel(
		visual,
		gpui_kit::ScrollDelta::Pixels(gpui_kit::Point::new(px(0.0), px(4.0))),
	);
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(184.0)
	);

	// Keyboard: steps, fine steps, range ends, typing starts an edit.
	visual.simulate_keystrokes("up");
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(192.0)
	);
	visual.simulate_keystrokes("shift-up");
	let fine = visual.read(|app| spin.read(app).value().to_f64());
	assert!(fine > 192.0 && fine < 194.0, "Shift = fine step: {fine}");
	visual.simulate_keystrokes("home");
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(0.0)
	);
	visual.simulate_keystrokes("end");
	assert_eq!(
		visual.read(|app| spin.read(app).value()),
		SliderValue::Float(2160.0)
	);
	visual.simulate_keystrokes("a");
	assert!(
		visual.read(|app| spin.read(app).edit.is_none()),
		"a letter is inert"
	);
	visual.simulate_keystrokes("7");
	cx.run_until_parked();
	assert!(
		visual.read(|app| spin.read(app).edit.is_some()),
		"a digit starts an edit"
	);
	// The wheel is inert while the editor is open.
	let before_wheel = visual.read(|app| spin.read(app).value());
	wheel(
		visual,
		gpui_kit::ScrollDelta::Lines(gpui_kit::Point::new(0.0, 1.0)),
	);
	assert_eq!(visual.read(|app| spin.read(app).value()), before_wheel);
	// The open editor renders under the readout; its text is the seed.
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let editor = visual
		.read(|app| spin.read(app).edit.clone())
		.expect("open editor");
	visual.update(|window, app| {
		editor.update(app, |editor, cx| editor.set_value("500", window, cx));
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
		window.draw(cx).clear(cx);
	});

	// Double click opens the editor seeded with the current value; a
	// blur commits the typed replacement (clamped).
	double_click_at(visual, gpui_kit::point(px(20.0), px(12.0)));
	cx.run_until_parked();
	let editor = visual
		.read(|app| spin.read(app).edit.clone())
		.expect("double click opened the editor");
	visual.update(|window, app| {
		editor.update(app, |editor, cx| editor.set_value("9000", window, cx));
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
			spin.model =
				SliderModel::new(ValueKind::Rational, 0.0, 100.0, 1.0, 3.0).with_rational_den(25);
			assert_eq!(spin.display().as_ref(), "3/25");
			spin.model = SliderModel::new(ValueKind::Float, 0.0, 1.0, 0.25, 0.0);
			assert_eq!(spin.display().as_ref(), "0");
		});
	});
}

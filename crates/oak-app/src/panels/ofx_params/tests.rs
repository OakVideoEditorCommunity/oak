use super::*;
use gpui_kit::{Modifiers, Subscription, TestAppContext};
use std::sync::Mutex;

#[test]
fn hex_parsing_and_formatting() {
	// #RRGGBB parses with opaque alpha; #RRGGBBAA keeps the alpha.
	let c = parse_hex("#1A80E6").expect("6-digit hex parses");
	assert!((c.r - 0x1A as f32 / 255.0).abs() < 1e-6);
	assert!((c.g - 0x80 as f32 / 255.0).abs() < 1e-6);
	assert!((c.b - 0xE6 as f32 / 255.0).abs() < 1e-6);
	assert!((c.a - 1.0).abs() < 1e-6);
	let c = parse_hex("#1A80E6FF").expect("8-digit opaque hex parses");
	assert!((c.a - 1.0).abs() < 1e-6);
	let c = parse_hex("#1A80E67F").expect("8-digit alpha hex parses");
	assert!((c.a - 0x7F as f32 / 255.0).abs() < 1e-6);

	// format -> parse round-trips exactly (both sides are 8-bit).
	for hex in ["#102030", "#0A0B0C", "#11223344", "#FF000080"] {
		let parsed = parse_hex(hex).expect("round-trip source parses");
		assert_eq!(format_hex(parsed), hex.to_ascii_uppercase());
	}

	// Opaque colours format to 6 digits, translucent to 8.
	assert_eq!(
		format_hex(Rgba {
			r: 0.0,
			g: 0.0,
			b: 0.0,
			a: 1.0,
		}),
		"#000000"
	);
	assert_eq!(
		format_hex(Rgba {
			r: 1.0,
			g: 1.0,
			b: 1.0,
			a: 0.5,
		}),
		"#FFFFFF80"
	);
}

#[test]
fn hex_parse_rejects_malformed() {
	for bad in [
		"",           // empty
		"102030",     // missing '#'
		"#12345",     // too short
		"#1234567",   // 7 digits
		"#GGHHII",    // non-hex digits
		"#123456789", // too long
		"# 123456",   // whitespace inside
	] {
		assert!(parse_hex(bad).is_none(), "expected {bad:?} to be rejected");
	}
}

/// The colour picker (and its four sliders + hex editor) constructs
/// without panicking and paints a swatch.
/// Regression: a palette click inside window event dispatch must land
/// on commit. The draft updates immediately; the hex field refresh is
/// deferred past the dispatch (the window is borrowed), and OK must
/// commit the draft — not resurrect the stale hex text.
#[gpui_kit::test]
async fn color_picker_palette_click_commits(cx: &mut TestAppContext) {
	let (window, picker, events) = open_picker(
		cx,
		Rgba {
			r: 0.4,
			g: 0.2,
			b: 0.8,
			a: 0.5,
		},
	);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let swatch = visual.debug_bounds("ofx-color-swatch").expect("swatch");
	visual.simulate_click(swatch.center(), Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let palette = visual
		.debug_bounds("ofx-color-sv-palette")
		.expect("palette");
	visual.simulate_click(palette.center(), Modifiers::default());
	cx.run_until_parked();

	let draft = visual.read(|cx| picker.read(cx).draft);
	assert_ne!(draft.r, 0.4, "the palette click moves the draft");
	// The deferred refresh updates the hex field to match the draft.
	let hex_text = visual.read(|cx| picker.read(cx).hex.read(cx).value().to_string());
	assert_eq!(
		hex_text,
		format_hex(draft),
		"the hex field follows the draft"
	);

	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let ok = visual.debug_bounds("ofx-color-ok-1").expect("ok button");
	visual.simulate_click(ok.center(), Modifiers::default());
	cx.run_until_parked();
	let committed = visual.read(|cx| picker.read(cx).committed);
	assert_eq!(committed, draft, "OK commits the palette-picked draft");
	assert!(
		picker_events(&events)
			.iter()
			.any(|e| matches!(e, OfxColorEvent::Committed(c) if *c == draft))
	);
}

#[gpui_kit::test]
async fn color_picker_constructs_without_panicking(cx: &mut TestAppContext) {
	struct Host {
		picker: Entity<OfxColorPicker>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.picker.clone())
		}
	}

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(200.0)), |window, cx| {
		let picker = cx.new(|cx| {
			OfxColorPicker::new(
				1,
				Rgba {
					r: 0.4,
					g: 0.2,
					b: 0.8,
					a: 0.5,
				},
				window,
				cx,
			)
		});
		Host { picker }
	});
	cx.run_until_parked();

	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		visual.debug_bounds("ofx-color-swatch").is_some(),
		"the colour swatch should be painted"
	);
}

/// The palette (SV square) mapping works from inside the popup: the
/// canvas records its layout bounds each frame and the click maps them
/// to (s, v). Regression test for the bare canvases — without
/// `.size_full()` the canvas leaf collapses to zero height in the
/// block layout, the recorded bounds are 0 tall, and a click at the
/// palette centre maps to v ≈ 0 (black) instead of v ≈ 0.5: the popup
/// opens but shows/behaves as an empty box (the reported "色板没显示
/// 出来").
#[gpui_kit::test]
async fn palette_click_maps_center_to_mid_saturation_value(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(560.0)), |window, cx| {
		OfxColorPicker::new(
			1,
			Rgba {
				r: 0.4,
				g: 0.2,
				b: 0.8,
				a: 0.5,
			},
			window,
			cx,
		)
	});
	cx.run_until_parked();

	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Open the popup by clicking the swatch, then re-draw so the popup's
	// deferred layer is laid out (and the palette canvas records its
	// bounds for the click mapping).
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	let swatch_center = Point::new(
		swatch.origin.x + swatch.size.width * 0.5,
		swatch.origin.y + swatch.size.height * 0.5,
	);
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Click the centre of the SV palette.
	let sv = visual
		.debug_bounds("ofx-color-sv-palette")
		.expect("sv palette bounds");
	assert!(
		f32::from(sv.size.height) >= 170.0,
		"sv palette height collapsed to {}",
		f32::from(sv.size.height)
	);
	let sv_center = Point::new(
		sv.origin.x + sv.size.width * 0.5,
		sv.origin.y + sv.size.height * 0.5,
	);
	visual.simulate_click(sv_center, Modifiers::default());
	cx.run_until_parked();

	// The centre of the palette maps to ~(0.5, 0.5); the hue bar still
	// shows the initial colour's hue is kept (reading the draft's s/v).
	let picker = window.root(cx).expect("picker root");
	let draft = cx.read(|cx| picker.read(cx).draft);
	let (_, s, v) = rgb_to_hsv(draft.r, draft.g, draft.b);
	assert!(
		(s - 0.5).abs() < 0.05,
		"centre click maps to mid saturation (got {s})"
	);
	assert!(
		(v - 0.5).abs() < 0.05,
		"centre click maps to mid value, not the collapsed-canvas 0 (got {v})"
	);
}

/// The "pick from viewer" button toggles the eyedropper and emits
/// `PickViewerToggle` events that the params view routes to the engine.
#[gpui_kit::test]
async fn pick_viewer_button_toggles_armed(cx: &mut TestAppContext) {
	struct Host {
		picker: Entity<OfxColorPicker>,
		events: Arc<Mutex<Vec<OfxColorEvent>>>,
		_subscription: Subscription,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.picker.clone())
		}
	}

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(560.0)), |window, cx| {
		let picker = cx.new(|cx| {
			OfxColorPicker::new(
				1,
				Rgba {
					r: 0.4,
					g: 0.2,
					b: 0.8,
					a: 0.5,
				},
				window,
				cx,
			)
		});
		let events = Arc::new(Mutex::new(Vec::new()));
		let events_sub = events.clone();
		let _subscription = cx.subscribe(
			&picker,
			move |_this, _emitter, event: &OfxColorEvent, _cx| {
				events_sub.lock().unwrap().push(*event);
			},
		);
		Host {
			picker,
			events,
			_subscription,
		}
	});
	cx.run_until_parked();

	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Open the popup by clicking the swatch, then re-draw so the popup's
	// deferred layer is laid out.
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	let swatch_center = Point::new(
		swatch.origin.x + swatch.size.width * 0.5,
		swatch.origin.y + swatch.size.height * 0.5,
	);
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Clicking "pick from viewer" arms the eyedropper...
	let button = visual
		.debug_bounds("ofx-color-pick-viewer-1")
		.expect("pick button painted");
	let button_center = Point::new(
		button.origin.x + button.size.width * 0.5,
		button.origin.y + button.size.height * 0.5,
	);
	visual.simulate_click(button_center, Modifiers::default());
	cx.run_until_parked();
	let host = window.root(cx).expect("host root");
	let events = cx.read(|cx| host.read(cx).events.lock().unwrap().clone());
	assert!(
		matches!(
			events.last(),
			Some(OfxColorEvent::PickViewerToggle { armed: true })
		),
		"clicking pick should arm the eyedropper, got {events:?}"
	);

	// ...and a second click disarms it again.
	visual.simulate_click(button_center, Modifiers::default());
	cx.run_until_parked();
	let events = cx.read(|cx| host.read(cx).events.lock().unwrap().clone());
	assert!(
		matches!(
			events.last(),
			Some(OfxColorEvent::PickViewerToggle { armed: false })
		),
		"second click should disarm the eyedropper, got {events:?}"
	);
}

/// While the viewer eyedropper is armed, a click outside the popup is a
/// *pick* on the program viewer, not a dismissal: the popup must survive
/// it (no `Cancelled` emitted, the arm state kept), or the sampled colour
/// would be dropped by `apply_viewer_pick`'s `picking` guard. Once the
/// eyedropper is disarmed, an outside click dismisses as usual.
#[gpui_kit::test]
async fn outside_click_while_picking_keeps_popup_open(cx: &mut TestAppContext) {
	struct Host {
		picker: Entity<OfxColorPicker>,
		events: Arc<Mutex<Vec<OfxColorEvent>>>,
		_subscription: Subscription,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.picker.clone())
		}
	}

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(560.0)), |window, cx| {
		let picker = cx.new(|cx| {
			OfxColorPicker::new(
				1,
				Rgba {
					r: 0.4,
					g: 0.2,
					b: 0.8,
					a: 0.5,
				},
				window,
				cx,
			)
		});
		let events = Arc::new(Mutex::new(Vec::new()));
		let events_sub = events.clone();
		let _subscription = cx.subscribe(
			&picker,
			move |_this, _emitter, event: &OfxColorEvent, _cx| {
				events_sub.lock().unwrap().push(*event);
			},
		);
		Host {
			picker,
			events,
			_subscription,
		}
	});
	cx.run_until_parked();

	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Open the popup by clicking the swatch, then re-draw so the popup's
	// deferred layer is laid out.
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	let swatch_center = Point::new(
		swatch.origin.x + swatch.size.width * 0.5,
		swatch.origin.y + swatch.size.height * 0.5,
	);
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Arm the eyedropper.
	let button = visual
		.debug_bounds("ofx-color-pick-viewer-1")
		.expect("pick button painted");
	let button_center = Point::new(
		button.origin.x + button.size.width * 0.5,
		button.origin.y + button.size.height * 0.5,
	);
	visual.simulate_click(button_center, Modifiers::default());
	cx.run_until_parked();

	// A click outside the popup (clear of its right/bottom edges, inside
	// the window) while armed: the popup survives, the arm is kept, and no
	// dismissal is emitted.
	let popup = visual
		.debug_bounds("ofx-color-popup")
		.expect("popup painted");
	let outside = Point::new(
		px((f32::from(popup.origin.x) + f32::from(popup.size.width) + 10.0).min(318.0)),
		px((f32::from(popup.origin.y) + f32::from(popup.size.height) + 10.0).min(399.0)),
	);
	visual.simulate_click(outside, Modifiers::default());
	cx.run_until_parked();
	let host = window.root(cx).expect("host root");
	let (open, picking, cancelled) = cx.read(|cx| {
		let host = host.read(cx);
		(
			host.picker.read(cx).open,
			host.picker.read(cx).picking,
			host.events
				.lock()
				.unwrap()
				.iter()
				.any(|e| matches!(e, OfxColorEvent::Cancelled)),
		)
	});
	assert!(
		open,
		"the popup must survive an outside click while picking"
	);
	assert!(picking, "the eyedropper stays armed through the pick");
	assert!(!cancelled, "no dismissal may be emitted while picking");

	// Once disarmed (the pick landed / the button toggled again), an
	// outside click dismisses the popup as usual.
	visual.simulate_click(button_center, Modifiers::default());
	cx.run_until_parked();
	visual.simulate_click(outside, Modifiers::default());
	cx.run_until_parked();
	let (open, cancelled) = cx.read(|cx| {
		let host = host.read(cx);
		(
			host.picker.read(cx).open,
			host.events
				.lock()
				.unwrap()
				.iter()
				.any(|e| matches!(e, OfxColorEvent::Cancelled)),
		)
	});
	assert!(!open, "outside click dismisses the popup once disarmed");
	assert!(cancelled, "the dismissal emits Cancelled");
}

/// Applying a viewer pick lands the sampled colour in the draft and
/// disarms the eyedropper.
#[gpui_kit::test]
async fn apply_viewer_pick_updates_draft(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(200.0)), |window, cx| {
		OfxColorPicker::new(
			1,
			Rgba {
				r: 0.0,
				g: 0.0,
				b: 0.0,
				a: 1.0,
			},
			window,
			cx,
		)
	});
	cx.run_until_parked();

	let picker = window.root(cx).expect("picker root");
	let red = Rgba {
		r: 1.0,
		g: 0.0,
		b: 0.0,
		a: 1.0,
	};
	cx.update(|cx| {
		picker.update(cx, |picker, cx| {
			picker.toggle_viewer_pick(cx);
			picker.apply_viewer_pick(red, cx);
		});
	});

	let (draft, picking) = cx.read(|cx| {
		let picker = picker.read(cx);
		(picker.draft, picker.picking)
	});
	assert_eq!(draft, red, "viewer pick should land in the draft");
	assert!(!picking, "a viewer pick disarms the eyedropper");
}

/// The pick's own click must not close the popup: the sampled colour
/// lands (disarming the eyedropper) between the button going down on
/// the viewer and coming up, so the trailing mouse-up arrives with
/// `picking` already false — without the one-shot swallow it reads as
/// an outside click, closes the popup and discards the picked colour
/// (the user can never press OK).
#[gpui_kit::test]
async fn pick_click_mouse_up_does_not_dismiss_popup(cx: &mut TestAppContext) {
	struct Host {
		picker: Entity<OfxColorPicker>,
		events: Arc<Mutex<Vec<OfxColorEvent>>>,
		_subscription: Subscription,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.picker.clone())
		}
	}

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(560.0)), |window, cx| {
		let picker = cx.new(|cx| {
			OfxColorPicker::new(
				1,
				Rgba {
					r: 0.4,
					g: 0.2,
					b: 0.8,
					a: 0.5,
				},
				window,
				cx,
			)
		});
		let events = Arc::new(Mutex::new(Vec::new()));
		let events_sub = events.clone();
		let _subscription = cx.subscribe(
			&picker,
			move |_this, _emitter, event: &OfxColorEvent, _cx| {
				events_sub.lock().unwrap().push(*event);
			},
		);
		Host {
			picker,
			events,
			_subscription,
		}
	});
	cx.run_until_parked();

	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Open the popup, re-draw for the deferred layer, arm the eyedropper.
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	visual.simulate_click(
		Point::new(
			swatch.origin.x + swatch.size.width * 0.5,
			swatch.origin.y + swatch.size.height * 0.5,
		),
		Modifiers::default(),
	);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let button = visual
		.debug_bounds("ofx-color-pick-viewer-1")
		.expect("pick button painted");
	visual.simulate_click(
		Point::new(
			button.origin.x + button.size.width * 0.5,
			button.origin.y + button.size.height * 0.5,
		),
		Modifiers::default(),
	);
	cx.run_until_parked();

	// The pick lands mid-click (the sampled colour arrives while the
	// button is still down on the viewer), then the button comes up
	// outside the popup.
	let red = Rgba {
		r: 1.0,
		g: 0.0,
		b: 0.0,
		a: 1.0,
	};
	let host = window.root(cx).expect("host root");
	visual.update(|_window, cx| {
		host.update(cx, |host, cx| {
			host.picker
				.update(cx, |picker, cx| picker.apply_viewer_pick(red, cx));
		});
	});
	let popup = visual
		.debug_bounds("ofx-color-popup")
		.expect("popup painted");
	let outside = Point::new(
		px((f32::from(popup.origin.x) + f32::from(popup.size.width) + 10.0).min(318.0)),
		px((f32::from(popup.origin.y) + f32::from(popup.size.height) + 10.0).min(399.0)),
	);
	visual.simulate_click(outside, Modifiers::default());
	cx.run_until_parked();

	let (open, draft, cancelled) = cx.read(|cx| {
		let host = host.read(cx);
		(
			host.picker.read(cx).open,
			host.picker.read(cx).draft,
			host.events
				.lock()
				.unwrap()
				.iter()
				.any(|e| matches!(e, OfxColorEvent::Cancelled)),
		)
	});
	assert!(open, "the pick's own mouse-up must not dismiss the popup");
	assert_eq!(draft, red, "the picked colour survives in the draft");
	assert!(!cancelled, "no dismissal may be emitted for the pick click");

	// A later genuine outside click still dismisses as usual.
	visual.simulate_click(outside, Modifiers::default());
	cx.run_until_parked();
	let (open, cancelled) = cx.read(|cx| {
		let host = host.read(cx);
		(
			host.picker.read(cx).open,
			host.events
				.lock()
				.unwrap()
				.iter()
				.any(|e| matches!(e, OfxColorEvent::Cancelled)),
		)
	});
	assert!(!open, "a later outside click dismisses the popup");
	assert!(cancelled, "the later dismissal emits Cancelled");
}

/// Known-value checks + round-trips for the RGB↔HSV conversion.
#[test]
fn hsv_rgb_known_values() {
	let hsv = |c: Rgba| rgb_to_hsv(c.r, c.g, c.b);
	let close = |a: f32, b: f32| (a - b).abs() < 1e-4;

	// The six canonical corners of the RGB cube.
	assert_eq!(
		hsv(Rgba {
			r: 1.0,
			g: 0.0,
			b: 0.0,
			a: 1.0
		}),
		(0.0, 1.0, 1.0)
	);
	assert_eq!(
		hsv(Rgba {
			r: 0.0,
			g: 1.0,
			b: 0.0,
			a: 1.0
		}),
		(120.0, 1.0, 1.0)
	);
	assert_eq!(
		hsv(Rgba {
			r: 0.0,
			g: 0.0,
			b: 1.0,
			a: 1.0
		}),
		(240.0, 1.0, 1.0)
	);
	assert_eq!(
		hsv(Rgba {
			r: 1.0,
			g: 1.0,
			b: 0.0,
			a: 1.0
		}),
		(60.0, 1.0, 1.0)
	);
	assert_eq!(
		hsv(Rgba {
			r: 1.0,
			g: 1.0,
			b: 1.0,
			a: 1.0
		}),
		(0.0, 0.0, 1.0)
	);
	assert_eq!(
		hsv(Rgba {
			r: 0.0,
			g: 0.0,
			b: 0.0,
			a: 1.0
		}),
		(0.0, 0.0, 0.0)
	);

	// The same corners back to RGB.
	let rgb = |h: f32, s: f32, v: f32| hsv_to_rgb(h, s, v);
	assert_eq!(
		rgb(0.0, 1.0, 1.0),
		Rgba {
			r: 1.0,
			g: 0.0,
			b: 0.0,
			a: 1.0
		}
	);
	assert_eq!(
		rgb(120.0, 1.0, 1.0),
		Rgba {
			r: 0.0,
			g: 1.0,
			b: 0.0,
			a: 1.0
		}
	);
	assert_eq!(
		rgb(240.0, 1.0, 1.0),
		Rgba {
			r: 0.0,
			g: 0.0,
			b: 1.0,
			a: 1.0
		}
	);

	// Arbitrary RGB round-trips.
	for (r, g, b) in [(0.5, 0.25, 0.75), (0.1, 0.9, 0.2), (0.33, 0.67, 0.44)] {
		let (h, s, v) = rgb_to_hsv(r, g, b);
		let back = hsv_to_rgb(h, s, v);
		assert!(
			close(back.r, r) && close(back.g, g) && close(back.b, b),
			"RGB round-trip failed for ({r}, {g}, {b}) -> {back:?}"
		);
	}

	// Arbitrary HSV round-trips (the hue may land on the other side of
	// 0° / 360°, hence the looser tolerance).
	for (h, s, v) in [(30.0, 0.5, 0.5), (200.0, 0.3, 0.8), (345.0, 1.0, 0.2)] {
		let rgb = hsv_to_rgb(h, s, v);
		let (h2, s2, v2) = rgb_to_hsv(rgb.r, rgb.g, rgb.b);
		assert!(
			(h2 - h).abs() < 1e-3 && (s2 - s).abs() < 1e-4 && (v2 - v).abs() < 1e-4,
			"HSV round-trip failed for ({h}, {s}, {v}) -> ({h2}, {s2}, {v2})"
		);
	}
}

/// The palette ↔ cursor mapping: corners, centre and out-of-bounds
/// clamping.
#[test]
fn sv_point_mapping() {
	let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(50.0)));
	let at = |x: f32, y: f32| point(px(x), px(y));

	assert_eq!(sv_from_point(bounds, at(10.0, 20.0)), (0.0, 1.0));
	assert_eq!(sv_from_point(bounds, at(110.0, 70.0)), (1.0, 0.0));
	assert_eq!(sv_from_point(bounds, at(60.0, 45.0)), (0.5, 0.5));
	// Out of bounds clamps to the edges.
	assert_eq!(sv_from_point(bounds, at(0.0, 0.0)), (0.0, 1.0));
	assert_eq!(sv_from_point(bounds, at(500.0, 500.0)), (1.0, 0.0));

	assert_eq!(point_from_sv(bounds, 0.0, 1.0), at(10.0, 20.0));
	assert_eq!(point_from_sv(bounds, 1.0, 0.0), at(110.0, 70.0));
	assert_eq!(point_from_sv(bounds, 0.5, 0.5), at(60.0, 45.0));
	assert_eq!(point_from_sv(bounds, -1.0, 2.0), at(10.0, 20.0));
}
/// A text field the user is editing is never re-synced mid-edit: the
/// params view re-renders on every engine change, and reapplying the
/// engine snapshot would wipe the in-progress text (the "cannot type
/// into the text field" report). On blur the field re-syncs to the
/// engine value.
#[gpui_kit::test]
async fn text_field_keeps_in_progress_edits_while_focused(cx: &mut TestAppContext) {
	use crate::oakui::mock::MockEngine;
	struct Host {
		view: Entity<OfxParamsView<MockEngine>>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.view.clone())
		}
	}
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(400.0), px(200.0)), |window, cx| {
		let engine = cx.new(MockEngine::create);
		let view = cx.new(|cx| OfxParamsView::<MockEngine>::new(EffectId(900), engine, window, cx));
		Host { view }
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("host root");
	let view = cx.read(|cx| host.read(cx).view.clone());
	// Two inputs are text fields: the single-line `args_in` and the
	// multi-line `plain_text_in`. This test types into the latter.
	let editor = cx.read(|cx| {
		view.read(cx)
			.controls
			.iter()
			.find(|c| c.input_id == "plain_text_in")
			.map(|c| match &c.kind {
				ControlKind::Text { editor, .. } => editor.clone(),
				_ => panic!("plain_text_in should build a text control"),
			})
			.expect("the plain_text_in control")
	});
	assert_eq!(cx.read(|cx| editor.value(cx)), "文本\nsecond line");

	// Focus the field and type: the in-progress text must survive the
	// engine-notify re-sync (the tick-rate repaint).
	window
		.update(cx, |_root, window, cx| {
			let handle = editor.focus_handle(cx);
			window.focus(&handle, cx);
		})
		.unwrap();
	cx.update_window(window.clone().into(), |_, window, cx| {
		editor.set_text("user typed", window, cx);
	})
	.expect("window open");
	cx.update(|cx| {
		view.update(cx, |view, cx| {
			let engine = view.engine.clone();
			engine.update(cx, |_engine, cx| cx.notify());
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| editor.value(cx)),
		"user typed",
		"the in-progress text survives the re-sync while focused"
	);

	// On blur the field re-syncs to the engine value.
	window
		.update(cx, |_root, window, cx| {
			window.blur(cx);
		})
		.unwrap();
	cx.update(|cx| {
		view.update(cx, |view, cx| {
			let engine = view.engine.clone();
			engine.update(cx, |_engine, cx| cx.notify());
		});
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| editor.value(cx)),
		"文本\nsecond line",
		"the field re-syncs to the engine value on blur"
	);
}

/// The v3 text node's parameter set renders as one structured control per
/// visible input — a multi-line text field, a font-family combo, colour
/// pickers, sliders, checkboxes, vec2 spinboxes — and the three hidden
/// inputs (the legacy text, the vertical align, the use-args toggle)
/// render nothing at all.
#[gpui_kit::test]
async fn text3_params_render_as_structured_controls(cx: &mut TestAppContext) {
	use crate::oakui::mock::MockEngine;
	struct Host {
		view: Entity<OfxParamsView<MockEngine>>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.view.clone())
		}
	}
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(400.0), px(600.0)), |window, cx| {
		let engine = cx.new(MockEngine::create);
		let view = cx.new(|cx| OfxParamsView::<MockEngine>::new(EffectId(900), engine, window, cx));
		Host { view }
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("host root");
	let view = cx.read(|cx| host.read(cx).view.clone());

	// The mock hands the hidden inputs over (a plugin engine's snapshot
	// can carry them too); the view must drop them before any control is
	// built.
	let raw_ids: Vec<String> = cx.read(|cx| {
		view.read(cx)
			.engine
			.read(cx)
			.effect_params(EffectId(900))
			.expect("the mock carries the text3 parameter set")
			.into_iter()
			.map(|p| p.input_id)
			.collect()
	});
	for hidden in ["text_in", "valign_in", "use_args_in"] {
		assert!(
			raw_ids.iter().any(|id| id == hidden),
			"the mock snapshot carries {hidden} ({raw_ids:?})"
		);
	}

	// Draw once: the render pass is where `sync_values` reapplies the
	// engine snapshot to the widgets (the sliders snap to their grid).
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	/// One plain-data snapshot of the control set (the kinds carry
	/// entities, so they cannot be compared directly).
	struct Snapshot {
		kinds: Vec<(String, &'static str)>,
		sliders: Vec<(String, f64)>,
		texts: Vec<(String, String, bool)>,
		combos: Vec<(String, Option<usize>)>,
		spins: Vec<(String, Vec<f64>)>,
		colors: Vec<String>,
	}
	let snap = visual.read(|cx| {
		let view = view.read(cx);
		let mut snap = Snapshot {
			kinds: Vec::new(),
			sliders: Vec::new(),
			texts: Vec::new(),
			combos: Vec::new(),
			spins: Vec::new(),
			colors: Vec::new(),
		};
		for control in &view.controls {
			let kind = match &control.kind {
				ControlKind::Slider(slider) => {
					snap.sliders
						.push((control.input_id.clone(), slider.read(cx).value().to_f64()));
					"slider"
				}
				ControlKind::CheckBox(_) => "checkbox",
				ControlKind::Combo(combo) => {
					snap.combos
						.push((control.input_id.clone(), combo.read(cx).selected()));
					"combo"
				}
				ControlKind::Spin(spins) => {
					snap.spins.push((
						control.input_id.clone(),
						spins
							.iter()
							.map(|(spin, _)| spin.read(cx).value().to_f64())
							.collect(),
					));
					"spin"
				}
				ControlKind::Color(_) => {
					snap.colors.push(control.input_id.clone());
					"color"
				}
				ControlKind::Text { editor, multiline } => {
					snap.texts
						.push((control.input_id.clone(), editor.value(cx), *multiline));
					if *multiline { "text-area" } else { "text" }
				}
				ControlKind::WheelGroup(_) => "wheel-group",
				ControlKind::Curve(_) => "curve",
				ControlKind::PushButton => "button",
				ControlKind::ReadOnly(_) => "readonly",
			};
			snap.kinds.push((control.input_id.clone(), kind));
		}
		snap
	});

	let kinds: Vec<(&str, &str)> = snap
		.kinds
		.iter()
		.map(|(id, kind)| (id.as_str(), *kind))
		.collect();
	assert_eq!(
		kinds,
		vec![
			("pos_in", "spin"),
			("size_in", "spin"),
			("plain_text_in", "text-area"),
			("font_family_in", "combo"),
			("font_size_in", "slider"),
			("outline_enabled_in", "checkbox"),
			("outline_color_in", "color"),
			("outline_width_in", "slider"),
			("glow_enabled_in", "checkbox"),
			("glow_color_in", "color"),
			("glow_radius_in", "slider"),
			("args_in", "text"),
		],
		"one control per visible text3 input, hidden inputs and secret textures skipped"
	);

	// Each numeric widget shows the engine value (not a grid neighbour).
	let slider_values: Vec<(&str, f64)> = snap
		.sliders
		.iter()
		.map(|(id, value)| (id.as_str(), *value))
		.collect();
	for (id, want) in [
		("font_size_in", 72.0),
		("outline_width_in", 2.0),
		("glow_radius_in", 8.0),
	] {
		let got = slider_values
			.iter()
			.find(|(input_id, _)| *input_id == id)
			.unwrap_or_else(|| panic!("{id} should be a slider: {slider_values:?}"));
		assert!(
			(got.1 - want).abs() <= 1e-9 * want.abs().max(1.0),
			"{id} shows {} but the engine holds {want}",
			got.1
		);
	}

	assert_eq!(
		snap.texts
			.iter()
			.map(|(id, text, multiline)| (id.as_str(), text.as_str(), *multiline))
			.collect::<Vec<_>>(),
		vec![
			("plain_text_in", "文本\nsecond line", true),
			("args_in", "", false),
		],
		"the editable text is multi-line and holds the node's text verbatim"
	);
	assert_eq!(
		snap.combos,
		vec![("font_family_in".to_string(), Some(0))],
		"the font family is a (string) combo"
	);
	assert_eq!(
		snap.colors,
		vec!["outline_color_in".to_string(), "glow_color_in".to_string()],
		"both colours get a picker"
	);
	assert_eq!(
		snap.spins,
		vec![
			("pos_in".to_string(), vec![0.0, 0.0]),
			("size_in".to_string(), vec![400.0, 300.0]),
		],
		"the vec2 params get one spinbox per component"
	);
}

/// Every numeric parameter seeds its slider on the step grid, inside the
/// range, and a re-sync does not move it: `font_size_in` (72 with a floor
/// of 1) used to show 50.995, because the range came from the type
/// default and the value snapped onto a coarse grid nowhere near it.
#[test]
fn numeric_params_stay_on_their_slider_grid() {
	fn param(
		value_type: ValueType,
		value: NodeValue,
		properties: Vec<(&str, NodeValue)>,
	) -> EffectParam {
		EffectParam {
			input_id: "test_in".to_string(),
			display_name: "Test".to_string(),
			value_type,
			value,
			flags: 0,
			properties: properties
				.into_iter()
				.map(|(k, v)| (k.to_string(), v))
				.collect(),
		}
	}
	fn close(got: f64, want: f64) -> bool {
		(got - want).abs() <= 1e-9 * want.abs().max(1.0)
	}

	struct Case {
		label: &'static str,
		param: EffectParam,
		want_value: f64,
		want_range: (f64, f64),
	}
	let cases = vec![
		Case {
			label: "font size 72 over a floor of 1",
			param: param(
				ValueType::Float,
				NodeValue::Float(72.0),
				vec![("min", NodeValue::Float(1.0))],
			),
			want_value: 72.0,
			want_range: (1.0, 201.0),
		},
		Case {
			label: "outline width 2 over a floor of 0",
			param: param(
				ValueType::Float,
				NodeValue::Float(2.0),
				vec![("min", NodeValue::Float(0.0))],
			),
			want_value: 2.0,
			want_range: (0.0, 20.0),
		},
		Case {
			label: "glow radius 8 over a floor of 0",
			param: param(
				ValueType::Float,
				NodeValue::Float(8.0),
				vec![("min", NodeValue::Float(0.0))],
			),
			want_value: 8.0,
			want_range: (0.0, 100.0),
		},
		Case {
			label: "a value sitting on its floor",
			param: param(
				ValueType::Float,
				NodeValue::Float(1.0),
				vec![("min", NodeValue::Float(1.0))],
			),
			want_value: 1.0,
			want_range: (1.0, 201.0),
		},
		Case {
			label: "a bounded 0..1 float",
			param: param(
				ValueType::Float,
				NodeValue::Float(0.5),
				vec![
					("min", NodeValue::Float(0.0)),
					("max", NodeValue::Float(1.0)),
				],
			),
			want_value: 0.5,
			want_range: (0.0, 1.0),
		},
		Case {
			label: "an unbounded float (the wide default range)",
			param: param(ValueType::Float, NodeValue::Float(0.7234), Vec::new()),
			want_value: 0.7234,
			want_range: (-10000.0, 10000.0),
		},
		Case {
			label: "an int over 0..10",
			param: param(
				ValueType::Int,
				NodeValue::Int(5),
				vec![("min", NodeValue::Int(0)), ("max", NodeValue::Int(10))],
			),
			want_value: 5.0,
			want_range: (0.0, 10.0),
		},
	];

	for case in cases {
		let label = case.label;
		let (min, max, step) = slider_range_and_step(&case.param);
		assert!(
			min.is_finite() && max.is_finite() && step.is_finite() && step > 0.0,
			"{label}: bad slider geometry ({min}, {max}) step {step}"
		);
		assert_eq!((min, max), case.want_range, "{label}: the slider range");
		let kind = if case.param.value_type == ValueType::Int {
			ValueKind::Integer
		} else {
			ValueKind::Float
		};
		let engine_value = case.param.value.to_double();
		let mut model = SliderModel::new(kind, min, max, step, engine_value.clamp(min, max));
		assert!(
			close(model.raw, case.want_value),
			"{label}: seeded at {} instead of {}",
			model.raw,
			case.want_value
		);
		// `sync_values` re-applies the engine value on every render: the
		// snapshot must land back on the same position.
		model.set_value(SliderValue::Float(engine_value));
		assert!(
			close(model.raw, case.want_value),
			"{label}: a re-sync moved the value to {} (want {})",
			model.raw,
			case.want_value
		);
		assert!(
			min <= model.raw && model.raw <= max,
			"{label}: {} is outside {min}..{max}",
			model.raw
		);
	}
}

// -----------------------------------------------------------------------
// Parameter helpers (pure)
// -----------------------------------------------------------------------

fn approx(a: f64, b: f64) -> bool {
	(a - b).abs() < 1e-9
}

fn mk_param(
	input_id: &str,
	value_type: ValueType,
	value: NodeValue,
	properties: Vec<(&str, NodeValue)>,
) -> EffectParam {
	EffectParam {
		input_id: input_id.to_string(),
		display_name: input_id.to_string(),
		value_type,
		value,
		flags: 0,
		properties: properties
			.into_iter()
			.map(|(k, v)| (k.to_string(), v))
			.collect(),
	}
}

/// `curve_domain`: the parametric key range comes from the
/// `parametric_range` property (repaired when degenerate), and the
/// value domain is the unit range for unit data, else the data extent
/// with a 10% pad (a unit span for flat data).
#[test]
fn curve_domain_defaults_to_unit_and_pads_non_unit_data() {
	use quercus_engine_rs::ofx::{ControlPoint, Curve};
	let cp = |key: f64, value: f64| ControlPoint {
		key,
		value,
		slope: 0.0,
	};
	let curve = |points: Vec<ControlPoint>| Curve { points };
	let text = || NodeValue::Text(String::new());

	// No curve and no range property: the unit domain.
	let p = mk_param("curves_in", ValueType::Parametric, text(), Vec::new());
	assert_eq!(curve_domain(&p, &[]), (0.0, 1.0, 0.0, 1.0));

	// A unit-range curve keeps the unit value domain.
	let c = curve(vec![cp(0.0, 0.0), cp(0.5, 1.0)]);
	assert_eq!(
		curve_domain(&p, std::slice::from_ref(&c)),
		(0.0, 1.0, 0.0, 1.0)
	);

	// The parametric_range property overrides the key domain.
	let p = mk_param(
		"curves_in",
		ValueType::Parametric,
		text(),
		vec![("parametric_range", NodeValue::Vec2([2.0, 8.0]))],
	);
	assert_eq!(
		curve_domain(&p, std::slice::from_ref(&c)),
		(2.0, 8.0, 0.0, 1.0)
	);

	// Degenerate and inverted key ranges are widened to a unit span.
	let p = mk_param(
		"curves_in",
		ValueType::Parametric,
		text(),
		vec![("parametric_range", NodeValue::Vec2([5.0, 5.0]))],
	);
	assert_eq!(curve_domain(&p, &[]), (5.0, 6.0, 0.0, 1.0));
	let p = mk_param(
		"curves_in",
		ValueType::Parametric,
		text(),
		vec![("parametric_range", NodeValue::Vec2([8.0, 2.0]))],
	);
	assert_eq!(curve_domain(&p, &[]), (8.0, 9.0, 0.0, 1.0));

	// Out-of-unit values: the data extent plus a 10% pad.
	let p = mk_param("curves_in", ValueType::Parametric, text(), Vec::new());
	let c = curve(vec![cp(0.0, 10.0), cp(1.0, 20.0)]);
	assert_eq!(
		curve_domain(&p, std::slice::from_ref(&c)),
		(0.0, 1.0, 9.0, 21.0)
	);

	// Flat non-unit data gets a unit span before the pad.
	let c = curve(vec![cp(0.0, 10.0), cp(1.0, 10.0)]);
	let (_, _, vmin, vmax) = curve_domain(&p, std::slice::from_ref(&c));
	assert!(
		approx(vmin, 9.9) && approx(vmax, 11.1),
		"flat curve should pad to 9.9..11.1, got {vmin}..{vmax}"
	);
}

/// `curve_point_to_editor`: real → normalized coordinates, Hermite
/// slopes become bezier handles, and a key not in the list falls back
/// to the first control point for the handle lookup.
#[test]
fn curve_point_to_editor_normalizes_keys_values_and_slopes() {
	use quercus_engine_rs::ofx::ControlPoint;
	let real = |key: f64, value: f64, slope: f64| ControlPoint { key, value, slope };
	let points = vec![
		real(0.0, 0.0, 0.0),
		real(0.5, 0.25, 0.5),
		real(1.0, 1.0, 1.0),
	];
	let unit = (0.0, 1.0, 0.0, 1.0);

	// First point: only the out handle (the next point exists).
	let e = curve_point_to_editor(&points[0], &points, unit);
	assert_eq!((e.x, e.y), (0.0, 0.0));
	assert!(e.handle_in.is_none());
	let out = e.handle_out.expect("first point out handle");
	assert!(approx(out.x, 0.5 / 3.0) && approx(out.y, 0.0));

	// Interior point: both handles, slope scaled by sx/sy.
	let e = curve_point_to_editor(&points[1], &points, unit);
	assert_eq!((e.x, e.y), (0.5, 0.25));
	let i = e.handle_in.expect("interior in handle");
	let o = e.handle_out.expect("interior out handle");
	assert!(approx(i.x, -0.5 / 3.0) && approx(i.y, -0.5 * 0.5 / 3.0));
	assert!(approx(o.x, 0.5 / 3.0) && approx(o.y, 0.5 * 0.5 / 3.0));

	// Last point: only the in handle (the previous point exists).
	let e = curve_point_to_editor(&points[2], &points, unit);
	assert!(e.handle_out.is_none());
	let i = e.handle_in.expect("last point in handle");
	assert!(approx(i.x, -0.5 / 3.0) && approx(i.y, -0.5 * 1.0 / 3.0));

	// A key absent from the list uses index 0 for the handle lookup.
	let stray = real(0.75, 0.1, 2.0);
	let e = curve_point_to_editor(&stray, &points, unit);
	assert!(approx(e.x, 0.75) && approx(e.y, 0.1));
	assert!(e.handle_in.is_none());
	let o = e.handle_out.expect("stray out handle via index 0");
	assert!(approx(o.x, (0.5 - 0.75) / 3.0) && approx(o.y, 2.0 * (0.5 - 0.75) / 3.0));

	// Non-unit domains scale the normalized coordinates.
	let pair = [real(1.0, 2.0, 3.0), real(2.0, 4.0, 0.0)];
	let e = curve_point_to_editor(&pair[0], &pair, (0.0, 2.0, 0.0, 4.0));
	assert!(approx(e.x, 0.5) && approx(e.y, 0.5));
	let o = e.handle_out.expect("non-unit out handle");
	assert!(approx(o.x, 0.5 / 3.0) && approx(o.y, 1.5 * 0.5 / 3.0));
}

/// `curve_from_editor`: slopes come from the out handle, then the in
/// handle, then the centered/one-sided auto difference; a zero-length
/// segment falls through to the auto slope.
#[test]
fn curve_from_editor_recovers_slopes_from_handles_and_neighbours() {
	use crate::widgets::curve_editor::{CurvePoint, CurveVec2};
	let v = CurveVec2::new;
	let unit = (0.0, 1.0, 0.0, 1.0);

	// Out handle (preferred) then in handle: both give slope 0.3.
	let points = vec![
		CurvePoint {
			x: 0.0,
			y: 0.0,
			handle_in: None,
			handle_out: Some(v(1.0 / 3.0, 0.1)),
		},
		CurvePoint {
			x: 1.0,
			y: 1.0,
			handle_in: Some(v(-1.0 / 3.0, -0.1)),
			handle_out: None,
		},
	];
	let c = curve_from_editor(&points, unit);
	assert!(approx(c.points[0].slope, 0.3), "out-handle slope");
	assert!(approx(c.points[1].slope, 0.3), "in-handle slope");

	// A zero-length segment behind the out handle falls back to the
	// centred difference, which is also zero here.
	let points = vec![
		CurvePoint {
			x: 0.5,
			y: 0.0,
			handle_in: None,
			handle_out: Some(v(0.1, 0.1)),
		},
		CurvePoint {
			x: 0.5,
			y: 1.0,
			handle_in: None,
			handle_out: None,
		},
	];
	let c = curve_from_editor(&points, unit);
	assert!(approx(c.points[0].slope, 0.0));
	assert!(approx(c.points[1].slope, 0.0));

	// A zero-length segment behind the in handle: same fallback.
	let points = vec![
		CurvePoint {
			x: 0.5,
			y: 1.0,
			handle_in: None,
			handle_out: None,
		},
		CurvePoint {
			x: 0.5,
			y: 0.0,
			handle_in: Some(v(-0.1, 0.3)),
			handle_out: None,
		},
	];
	let c = curve_from_editor(&points, unit);
	assert!(approx(c.points[1].slope, 0.0));

	// No handles: centred difference for an interior point, one-sided
	// differences for the ends.
	let points = vec![
		CurvePoint::new(0.0, 0.0),
		CurvePoint::new(0.5, 1.0),
		CurvePoint::new(1.0, 2.0),
	];
	let c = curve_from_editor(&points, unit);
	assert!(approx(c.points[0].slope, 2.0), "first one-sided slope");
	assert!(approx(c.points[1].slope, 2.0), "interior centred slope");
	assert!(approx(c.points[2].slope, 2.0), "last one-sided slope");

	// A single point has no neighbours: slope zero.
	let c = curve_from_editor(&[CurvePoint::new(0.5, 0.5)], unit);
	assert!(approx(c.points[0].slope, 0.0));

	// A degenerate centred difference (both neighbours on one x): zero.
	let points = vec![
		CurvePoint::new(0.0, 0.0),
		CurvePoint::new(0.5, 1.0),
		CurvePoint::new(0.0, 2.0),
	];
	let c = curve_from_editor(&points, unit);
	assert!(approx(c.points[1].slope, 0.0));

	// Non-unit domain: the normalized slope scales back by sy/sx.
	let points = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
	let c = curve_from_editor(&points, (0.0, 2.0, 0.0, 4.0));
	assert!(approx(c.points[0].slope, 2.0));
	assert!(approx(c.points[1].slope, 2.0));

	// A point's key/value map back through the domain.
	let c = curve_from_editor(&[CurvePoint::new(0.5, 0.25)], (0.0, 2.0, 0.0, 4.0));
	assert!(approx(c.points[0].key, 1.0) && approx(c.points[0].value, 1.0));
}

/// `curve_points_close`: length, coordinates (1e-6 epsilon) and both
/// handles must match.
#[test]
fn curve_points_close_compares_coordinates_and_handles() {
	use crate::widgets::curve_editor::{CurvePoint, CurveVec2};
	let a = vec![
		CurvePoint::new(0.0, 0.0),
		CurvePoint::with_handles(0.5, 0.5, CurveVec2::new(0.1, 0.1)),
	];
	assert!(curve_points_close(&a, &a.clone()));

	// Different lengths.
	assert!(!curve_points_close(&a, &a[..1]));

	// A coordinate outside the epsilon.
	let mut b = a.clone();
	b[0].x += 1e-5;
	assert!(!curve_points_close(&a, &b));

	// Handle presence and value mismatches.
	let mut b = a.clone();
	b[1].handle_in = None;
	assert!(!curve_points_close(&a, &b));
	let mut b = a.clone();
	b[1].handle_out = Some(CurveVec2::new(0.1, 0.2));
	assert!(!curve_points_close(&a, &b));

	// Within the epsilon: close.
	let mut b = a.clone();
	b[1].handle_out = Some(CurveVec2::new(0.1, 0.1 + 1e-9));
	assert!(curve_points_close(&a, &b));
}

/// `slider_value`: ints stay integers, floats stay floats, everything
/// else converts through `to_double`.
#[test]
fn slider_value_maps_int_float_and_other_values() {
	let p = mk_param("v", ValueType::Int, NodeValue::Int(-3), Vec::new());
	assert_eq!(slider_value(&p), SliderValue::Integer(-3));
	let p = mk_param("v", ValueType::Float, NodeValue::Float(2.5), Vec::new());
	assert_eq!(slider_value(&p), SliderValue::Float(2.5));
	let p = mk_param(
		"v",
		ValueType::Boolean,
		NodeValue::Boolean(true),
		Vec::new(),
	);
	assert_eq!(slider_value(&p), SliderValue::Float(1.0));
}

/// `combo_haystack`: string combos prefer the `combo_value` list, fall
/// back to the labels, and keep an unknown current value in front.
#[test]
fn combo_haystack_prefers_values_and_keeps_unknown_selection() {
	let mut p = mk_param(
		"s",
		ValueType::StrCombo,
		NodeValue::StrCombo("b".to_string()),
		vec![
			("combo_option", NodeValue::Text("Alpha".to_string())),
			("combo_option", NodeValue::Text("Beta".to_string())),
			("combo_value", NodeValue::Text("a".to_string())),
			("combo_value", NodeValue::Text("b".to_string())),
		],
	);
	assert_eq!(combo_haystack(&p), vec!["a", "b"]);

	// A saved value that is not in the list goes in front (so the
	// combo shows what the node holds).
	p.value = NodeValue::StrCombo("saved-elsewhere".to_string());
	assert_eq!(combo_haystack(&p), vec!["saved-elsewhere", "a", "b"]);

	// An empty current value never is inserted.
	p.value = NodeValue::StrCombo(String::new());
	assert_eq!(combo_haystack(&p), vec!["a", "b"]);

	// No combo_value: the option labels are the haystack; a Text value
	// counts as the current string too.
	let p = mk_param(
		"s",
		ValueType::StrCombo,
		NodeValue::Text("Serif".to_string()),
		vec![("combo_option", NodeValue::Text("Sans".to_string()))],
	);
	assert_eq!(combo_haystack(&p), vec!["Serif", "Sans"]);

	// Integer combos only have labels.
	let p = mk_param(
		"c",
		ValueType::Combo,
		NodeValue::Combo(1),
		vec![
			("combo_option", NodeValue::Text("One".to_string())),
			("combo_option", NodeValue::Text("Two".to_string())),
		],
	);
	assert_eq!(combo_haystack(&p), vec!["One", "Two"]);
}

/// `combo_index_for`: string combos match by value (unknown → 0),
/// integer combos carry the index clamped at zero.
#[test]
fn combo_index_for_matches_strings_and_clamps_integers() {
	let options = vec![
		("combo_option", NodeValue::Text("Alpha".to_string())),
		("combo_option", NodeValue::Text("Beta".to_string())),
		("combo_value", NodeValue::Text("a".to_string())),
		("combo_value", NodeValue::Text("b".to_string())),
	];
	let mut p = mk_param(
		"s",
		ValueType::StrCombo,
		NodeValue::StrCombo("b".to_string()),
		options.clone(),
	);
	assert_eq!(combo_index_for(&p), 1);
	p.value = NodeValue::Text("b".to_string());
	assert_eq!(combo_index_for(&p), 1);
	p.value = NodeValue::StrCombo("missing".to_string());
	assert_eq!(combo_index_for(&p), 0, "an unknown string falls back to 0");
	p.value = NodeValue::Combo(0);
	assert_eq!(combo_index_for(&p), 0, "a non-string value falls back to 0");

	let p = mk_param("c", ValueType::Combo, NodeValue::Int(3), Vec::new());
	assert_eq!(combo_index_for(&p), 3);
	let p = mk_param("c", ValueType::Combo, NodeValue::Int(-5), Vec::new());
	assert_eq!(combo_index_for(&p), 0, "negative indices clamp to 0");
	let p = mk_param("c", ValueType::Combo, NodeValue::Float(2.9), Vec::new());
	assert_eq!(combo_index_for(&p), 2);
}

/// The OCIO grading primaries are vec4 inputs (x = master, y/z/w =
/// RGB): they must build a four-spin control, and a bound-less
/// component must use the node's `base` step instead of the wide
/// ±10000 default (the `pivot` 0.18 → −7399 drag regression).
#[test]
fn vec4_grading_components_build_four_spins_with_sane_ranges() {
	let mut param = mk_param(
		"contrast_in",
		ValueType::Vec4,
		NodeValue::Vec4([1.0, 1.0, 1.0, 1.0]),
		vec![
			("min", NodeValue::Vec4([0.01, 0.01, 0.01, 0.01])),
			("base", NodeValue::Float(0.01)),
		],
	);
	assert_eq!(
		component_range_and_step(&param, 0, 1.0),
		(0.01, 2.0, 0.01),
		"a per-component vec4 min bounds the master component"
	);
	param
		.properties
		.push(("max".to_string(), NodeValue::Float(4.0)));
	assert_eq!(component_range_and_step(&param, 1, 3.0), (0.01, 4.0, 0.01));

	// A bound-less vec4 (offset/exposure) anchors on the value.
	let mut free = mk_param(
		"offset_in",
		ValueType::Vec4,
		NodeValue::Vec4([0.0, 0.0, 0.0, 0.0]),
		vec![("base", NodeValue::Float(0.01))],
	);
	assert_eq!(component_range_and_step(&free, 0, 0.0), (-1.0, 1.0, 0.01));
	free.properties.clear();
	assert_eq!(
		component_range_and_step(&free, 2, 0.5),
		(0.4, 0.6, 0.001),
		"no base falls back to the fine default step"
	);
}

/// A scalar float with only a `base` step (the OCIO grading `pivot`,
/// `clampBlack`/`clampWhite`) gets a value-anchored slider range.
#[test]
fn base_only_float_sliders_anchor_on_the_value() {
	let pivot = mk_param(
		"pivot_in",
		ValueType::Float,
		NodeValue::Float(0.18),
		vec![("base", NodeValue::Float(0.01))],
	);
	let (lo, hi, step) = slider_range_and_step(&pivot);
	assert!(
		(lo + 0.82).abs() < 1e-9 && (hi - 1.18).abs() < 1e-9 && step == 0.01,
		"the pivot slider spans ±100 base steps around the value (got {lo}, {hi}, {step})"
	);

	// Without a base the wide default is kept (e.g. a node that only
	// exposes a raw numeric box with no declared domain).
	let wide = mk_param(
		"wide_in",
		ValueType::Float,
		NodeValue::Float(0.5),
		Vec::new(),
	);
	let (lo, hi, _) = slider_range_and_step(&wide);
	assert!(lo <= -1000.0 && hi >= 1000.0);
}

/// `value_components`: vectors and colours flatten, everything else is
/// empty.
#[test]
fn value_components_covers_vectors_and_colors() {
	assert_eq!(
		value_components(&NodeValue::Vec2([1.0, 2.0])),
		vec![1.0, 2.0]
	);
	assert_eq!(
		value_components(&NodeValue::Vec3([1.0, 2.0, 3.0])),
		vec![1.0, 2.0, 3.0]
	);
	assert_eq!(
		value_components(&NodeValue::Vec4([1.0, 2.0, 3.0, 4.0])),
		vec![1.0, 2.0, 3.0, 4.0]
	);
	assert_eq!(
		value_components(&NodeValue::Color([0.1, 0.2, 0.3, 0.4])),
		vec![0.1, 0.2, 0.3, 0.4]
	);
	assert!(value_components(&NodeValue::Float(1.0)).is_empty());
	assert!(value_components(&NodeValue::None).is_empty());
}

/// `default_range` / `numeric_range` / `range_property`: the default
/// ranges per type, the attached min/max, and the finite-number filter
/// that ignores NaN, infinities and non-numeric properties.
#[test]
fn numeric_range_reads_properties_and_filters_bad_bounds() {
	assert_eq!(default_range(ValueType::Int), (-100_000.0, 100_000.0));
	assert_eq!(default_range(ValueType::Float), (-10_000.0, 10_000.0));
	assert_eq!(default_range(ValueType::Vec2), (-10_000.0, 10_000.0));

	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(0.0),
		vec![("min", NodeValue::Float(1.5)), ("max", NodeValue::Int(9))],
	);
	assert_eq!(numeric_range(&p), (1.5, 9.0));
	assert_eq!(range_property(&p, "min"), Some(1.5));
	assert_eq!(range_property(&p, "max"), Some(9.0));
	assert_eq!(range_property(&p, "missing"), None);

	// NaN / infinite bounds are ignored (a broken slider range).
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(0.0),
		vec![
			("min", NodeValue::Float(f64::NAN)),
			("max", NodeValue::Float(f64::INFINITY)),
		],
	);
	assert_eq!(range_property(&p, "min"), None);
	assert_eq!(range_property(&p, "max"), None);
	assert_eq!(numeric_range(&p), (-10_000.0, 10_000.0));

	// Non-numeric bounds are ignored too.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(0.0),
		vec![("min", NodeValue::Text("x".to_string()))],
	);
	assert_eq!(range_property(&p, "min"), None);

	// The int default range is wider than the float one.
	let p = mk_param("i", ValueType::Int, NodeValue::Int(0), Vec::new());
	assert_eq!(numeric_range(&p), (-100_000.0, 100_000.0));
}

/// `slider_range_and_step`: degenerate and non-finite cases for both
/// value kinds (ordered bounds, floor/ceiling-only, bare ranges).
#[test]
fn slider_range_and_step_handles_degenerate_bounds() {
	// Int: an inverted range is ordered and the value expands the ends.
	let p = mk_param(
		"i",
		ValueType::Int,
		NodeValue::Int(5),
		vec![("min", NodeValue::Int(10)), ("max", NodeValue::Int(0))],
	);
	assert_eq!(slider_range_and_step(&p), (0.0, 10.0, 1.0));

	// Int: a degenerate range widens by 200.
	let p = mk_param(
		"i",
		ValueType::Int,
		NodeValue::Int(5),
		vec![("min", NodeValue::Int(5)), ("max", NodeValue::Int(5))],
	);
	assert_eq!(slider_range_and_step(&p), (5.0, 205.0, 1.0));

	// Int: a non-finite value does not expand the range.
	let p = mk_param(
		"i",
		ValueType::Int,
		NodeValue::Float(f64::NAN),
		vec![("min", NodeValue::Int(2)), ("max", NodeValue::Int(4))],
	);
	assert_eq!(slider_range_and_step(&p), (2.0, 4.0, 1.0));

	// Float with a floor only: a non-finite value sits on the floor,
	// its zero span means step one.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(f64::NAN),
		vec![("min", NodeValue::Float(3.0))],
	);
	assert_eq!(slider_range_and_step(&p), (3.0, 203.0, 1.0));

	// Float with a floor only and a value below it: same.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(-5.0),
		vec![("min", NodeValue::Float(3.0))],
	);
	assert_eq!(slider_range_and_step(&p), (3.0, 203.0, 1.0));

	// Float with a ceiling only: the mirror image.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(f64::NAN),
		vec![("max", NodeValue::Float(7.0))],
	);
	assert_eq!(slider_range_and_step(&p), (-193.0, 7.0, 1.0));

	// Both bounds inverted: ordered, and the grid rounds to whole steps.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(3.0),
		vec![
			("min", NodeValue::Float(5.0)),
			("max", NodeValue::Float(1.0)),
		],
	);
	let (min, max, step) = slider_range_and_step(&p);
	assert_eq!((min, max), (1.0, 5.0));
	assert!(approx(step, 0.02), "inverted range step {step}");

	// Neither bound, non-finite value: the wide default range with the
	// 200-step grid.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(f64::NAN),
		Vec::new(),
	);
	let (min, max, step) = slider_range_and_step(&p);
	assert_eq!((min, max), (-10_000.0, 10_000.0));
	assert!(approx(step, 100.0), "unbounded step {step}");

	// A value barely above the floor rounds to a single (fine) step.
	let p = mk_param(
		"f",
		ValueType::Float,
		NodeValue::Float(0.0001),
		vec![
			("min", NodeValue::Float(0.0)),
			("max", NodeValue::Float(1.0)),
		],
	);
	let (min, max, step) = slider_range_and_step(&p);
	assert_eq!((min, max), (0.0, 1.0));
	assert!(approx(step, 0.0001), "tiny span step {step}");
}

/// `nice_grid_step`: an exact 1/2/5 divisor when one exists, the
/// twenty-five-step fallback otherwise, and one for bad spans.
#[test]
fn nice_grid_step_picks_exact_dividing_steps_and_falls_back() {
	assert!(approx(nice_grid_step(100.0), 5.0));
	assert!(approx(nice_grid_step(1.0), 0.05));

	// No 1/2/5 × 10^k step divides this span into a whole number.
	let span = std::f64::consts::PI * 2.0;
	assert!(
		approx(nice_grid_step(span), span / 25.0),
		"fallback {}",
		nice_grid_step(span)
	);

	// Bad spans fall back to one.
	assert_eq!(nice_grid_step(0.0), 1.0);
	assert_eq!(nice_grid_step(-10.0), 1.0);
	assert_eq!(nice_grid_step(f64::INFINITY), 1.0);
	assert_eq!(nice_grid_step(f64::NAN), 1.0);
}

/// `is_multiline` reads only the `("multiline", true)` property.
#[test]
fn is_multiline_reads_the_boolean_property() {
	let p = mk_param(
		"t",
		ValueType::Text,
		NodeValue::Text("x".to_string()),
		vec![("multiline", NodeValue::Boolean(true))],
	);
	assert!(is_multiline(&p));
	let p = mk_param(
		"t",
		ValueType::Text,
		NodeValue::Text("x".to_string()),
		vec![("multiline", NodeValue::Boolean(false))],
	);
	assert!(!is_multiline(&p));
	let p = mk_param(
		"t",
		ValueType::Text,
		NodeValue::Text("x".to_string()),
		vec![("multiline", NodeValue::Int(1))],
	);
	assert!(!is_multiline(&p));
	let p = mk_param(
		"t",
		ValueType::Text,
		NodeValue::Text("x".to_string()),
		Vec::new(),
	);
	assert!(!is_multiline(&p));
}

/// `patch_component` replaces in-range channels of vec/colour values
/// and leaves everything else untouched.
#[test]
fn patch_component_replaces_in_range_and_ignores_other_values() {
	let patched = patch_component(&NodeValue::Vec2([0.0, 1.0]), 1, 2.0);
	assert_eq!(value_components(&patched), vec![0.0, 2.0]);
	let patched = patch_component(&NodeValue::Vec2([0.0, 1.0]), 2, 2.0);
	assert_eq!(
		value_components(&patched),
		vec![0.0, 1.0],
		"channel out of range"
	);

	let patched = patch_component(&NodeValue::Vec3([0.0, 1.0, 2.0]), 2, 9.0);
	assert_eq!(value_components(&patched), vec![0.0, 1.0, 9.0]);
	let patched = patch_component(&NodeValue::Vec3([0.0, 1.0, 2.0]), 3, 9.0);
	assert_eq!(value_components(&patched), vec![0.0, 1.0, 2.0]);

	let patched = patch_component(&NodeValue::Vec4([0.0, 1.0, 2.0, 3.0]), 3, 9.0);
	assert_eq!(value_components(&patched), vec![0.0, 1.0, 2.0, 9.0]);
	let patched = patch_component(&NodeValue::Vec4([0.0, 1.0, 2.0, 3.0]), 4, 9.0);
	assert_eq!(value_components(&patched), vec![0.0, 1.0, 2.0, 3.0]);

	let patched = patch_component(&NodeValue::Color([0.0, 0.0, 0.0, 1.0]), 0, 0.5);
	assert_eq!(value_components(&patched), vec![0.5, 0.0, 0.0, 1.0]);
	let patched = patch_component(&NodeValue::Color([0.0, 0.0, 0.0, 1.0]), 4, 0.5);
	assert_eq!(value_components(&patched), vec![0.0, 0.0, 0.0, 1.0]);

	let patched = patch_component(&NodeValue::Float(1.0), 0, 2.0);
	assert_eq!(patched.to_double(), 1.0, "non-vector values are untouched");
}

/// `hue_from_point`: top-to-bottom maps to 0..360 with clamping.
#[test]
fn hue_from_point_maps_and_clamps() {
	let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(18.0), px(180.0)));
	let at = |y: f32| point(px(15.0), px(y));
	assert_eq!(hue_from_point(bounds, at(20.0)), 0.0);
	assert_eq!(hue_from_point(bounds, at(200.0)), 360.0);
	assert_eq!(hue_from_point(bounds, at(110.0)), 180.0);
	// Out of bounds clamps.
	assert_eq!(hue_from_point(bounds, at(-100.0)), 0.0);
	assert_eq!(hue_from_point(bounds, at(900.0)), 360.0);
}

/// `hsv_to_rgb`: every hue sector, including the wraparound sector.
#[test]
fn hsv_to_rgb_covers_all_hue_sectors() {
	let rgb = |h: f32, s: f32, v: f32| hsv_to_rgb(h, s, v);
	// Sector 0, 1, 2, 3.
	assert_eq!(rgb(0.0, 1.0, 1.0).r, 1.0);
	assert_eq!(rgb(60.0, 1.0, 1.0).g, 1.0);
	assert_eq!(rgb(120.0, 1.0, 1.0).g, 1.0);
	assert_eq!(rgb(180.0, 1.0, 1.0).g, 1.0);
	assert_eq!(rgb(180.0, 1.0, 1.0).b, 1.0);
	// Sector 4 and the wraparound sector 5.
	assert_eq!(rgb(240.0, 1.0, 1.0).b, 1.0);
	let magenta = rgb(300.0, 1.0, 1.0);
	assert!(magenta.r == 1.0 && magenta.g == 0.0 && magenta.b == 1.0);
	// Negative hues wrap into the circle.
	let wrapped = rgb(-60.0, 1.0, 1.0);
	assert!(wrapped.r == 1.0 && wrapped.g == 0.0 && wrapped.b == 1.0);
}

/// `sv_from_point` guards against zero-size bounds (a collapsed
/// canvas must not divide by zero).
#[test]
fn sv_from_point_handles_zero_size_bounds() {
	let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0)));
	let (s, v) = sv_from_point(bounds, point(px(0.0), px(0.0)));
	assert!(s.is_finite() && v.is_finite());
}

/// The hex parser trims surrounding whitespace before validating.
#[test]
fn hex_parse_trims_surrounding_whitespace() {
	let c = parse_hex("  #112233  ").expect("trimmed hex parses");
	assert!((c.r - 0x11 as f32 / 255.0).abs() < 1e-6);
	assert_eq!(format_hex(c), "#112233");
}

// -----------------------------------------------------------------------
// Params view: control building, value sync and event wiring (gpui)
// -----------------------------------------------------------------------

use crate::oakui::mock::MockEngine;

struct ParamsHost {
	view: Entity<OfxParamsView<MockEngine>>,
}

impl Render for ParamsHost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div()
			.debug_selector(|| "params-host".into())
			.size_full()
			.child(self.view.clone())
	}
}

/// Opens a params view over the mock engine's text3-shaped parameter set
/// (sentinel effect 900) in a test window.
fn open_params_view(
	cx: &mut TestAppContext,
	width: f32,
	height: f32,
) -> (
	gpui_kit::WindowHandle<ParamsHost>,
	Entity<OfxParamsView<MockEngine>>,
) {
	open_params_view_for(cx, EffectId(900), width, height)
}

/// [`open_params_view`] for a specific mock effect id (901 is the
/// canned three-way corrector).
fn open_params_view_for(
	cx: &mut TestAppContext,
	effect: EffectId,
	width: f32,
	height: f32,
) -> (
	gpui_kit::WindowHandle<ParamsHost>,
	Entity<OfxParamsView<MockEngine>>,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(width), px(height)), |window, cx| {
		let engine = cx.new(MockEngine::create);
		let view = cx.new(|cx| OfxParamsView::<MockEngine>::new(effect, engine, window, cx));
		ParamsHost { view }
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("params host root");
	let view = cx.read(|cx| host.read(cx).view.clone());
	(window, view)
}

/// Builds `params` inside the view's context (the shared entity context
/// keeps child entities alive) and returns them for inspection. The
/// controls are deliberately NOT added to the view.
fn build_in_view(
	view: &Entity<OfxParamsView<MockEngine>>,
	visual: &mut gpui_kit::VisualTestContext,
	params: Vec<EffectParam>,
) -> Vec<ParamControl> {
	visual.update(|window, cx| {
		view.update(cx, |_view, cx| {
			let mut control_id = 10_000usize;
			params
				.iter()
				.map(|p| build_control(p, &mut control_id, window, cx))
				.collect()
		})
	})
}

/// Builds the extra controls AND wires them to the view (subscriptions
/// live on the view entity), moving them into `view.controls` so the
/// child entities stay alive.
fn add_and_wire_in_view(
	view: &Entity<OfxParamsView<MockEngine>>,
	visual: &mut gpui_kit::VisualTestContext,
	params: Vec<EffectParam>,
) {
	visual.update(|window, cx| {
		view.update(cx, |view, cx| {
			let mut control_id = 20_000usize;
			let controls = params
				.iter()
				.map(|p| build_control(p, &mut control_id, window, cx))
				.collect::<Vec<_>>();
			let temp = OfxParamsView {
				engine: view.engine.clone(),
				effect: view.effect,
				controls,
			};
			wire_controls(&temp, cx);
			view.controls.extend(temp.controls);
		})
	});
}

/// Returns `(kind_name, entity)`-style descriptors for the view's
/// controls, so tests can assert the control set without moving entities.
fn control_kinds(view: &Entity<OfxParamsView<MockEngine>>, cx: &App) -> Vec<(String, String)> {
	view.read(cx)
		.controls
		.iter()
		.map(|c| {
			let kind = match &c.kind {
				ControlKind::Slider(_) => "slider",
				ControlKind::CheckBox(_) => "checkbox",
				ControlKind::Combo(_) => "combo",
				ControlKind::Spin(_) => "spin",
				ControlKind::Color(_) => "color",
				ControlKind::Text { multiline, .. } => {
					if *multiline {
						"text-area"
					} else {
						"text"
					}
				}
				ControlKind::WheelGroup(_) => "wheel-group",
				ControlKind::Curve(_) => "curve",
				ControlKind::PushButton => "button",
				ControlKind::ReadOnly(_) => "readonly",
			};
			(c.input_id.clone(), kind.to_string())
		})
		.collect()
}

/// The three-way corrector's colour params render as grading wheels
/// (one per tonal band, captions beneath), the amounts stay sliders,
/// and a wheel drag writes the band's colour param to the engine.
#[gpui_kit::test]
async fn three_way_color_gets_grading_wheels(cx: &mut TestAppContext) {
	// 340px wide: the inspector's default docked width — the regression
	// window (a wider window would not have caught the clipping).
	let (window, view) = open_params_view_for(cx, EffectId(901), 340.0, 900.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// The control set: one wheel group, then the three amount sliders —
	// no generic colour pickers for the band colours.
	let kinds = cx.read(|app| control_kinds(&view, app));
	assert_eq!(
		kinds.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>(),
		vec!["wheel-group", "slider", "slider", "slider"],
		"the wheel group replaces the three pickers: {kinds:?}"
	);
	let wheel = visual.debug_bounds("color-wheel-0").expect("shadows wheel");
	let center = wheel.center();
	visual.simulate_click(
		point(center.x + px(f32::from(wheel.size.width) * 0.45), center.y),
		Modifiers::default(),
	);
	cx.run_until_parked();

	// The click drove the shadows colour red-ward (rim right = 0° hue).
	let attempts = cx.read(|app| view.read(app).engine.read(app).effect_param_attempts());
	let write = attempts
		.iter()
		.find(|(_, id, value)| id == "shadows_color_in" && value.starts_with("Color"))
		.expect("a colour write for the shadows band");
	assert!(
		write.2.contains("0.9") || write.2.contains("1.0"),
		"red-ward: {write:?}"
	);

	// Regression: the wheel row must fit the panel — a clipped wheel is
	// unreachable (the content mask swallows its clicks).
	let window_right = {
		let first = visual.debug_bounds("color-wheel-0").expect("wheel 0");
		first.origin.x
	};
	let _ = window_right;
	for wheel_id in ["color-wheel-0", "color-wheel-1", "color-wheel-2"] {
		let bounds = visual
			.debug_bounds(wheel_id)
			.unwrap_or_else(|| panic!("{wheel_id} painted"));
		let panel_right = px(340.0);
		assert!(
			bounds.origin.x + bounds.size.width <= panel_right,
			"{wheel_id} must not be clipped by the panel ({:?} exceeds {panel_right:?})",
			bounds,
		);
	}
}

/// `build_control` maps every value type to its widget (or read-only
/// line), seeding the values and deriving the wrapped labels.
#[gpui_kit::test]
async fn build_control_builds_a_widget_for_every_param_kind(cx: &mut TestAppContext) {
	let (window, view) = open_params_view(cx, 420.0, 900.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();

	let params = vec![
		mk_param(
			"int_in",
			ValueType::Int,
			NodeValue::Int(5),
			vec![("min", NodeValue::Int(0)), ("max", NodeValue::Int(10))],
		),
		mk_param(
			"float_clamped_in",
			ValueType::Float,
			NodeValue::Float(2.5),
			vec![("min", NodeValue::Int(0)), ("max", NodeValue::Int(1))],
		),
		mk_param(
			"bool_on_in",
			ValueType::Boolean,
			NodeValue::Boolean(true),
			Vec::new(),
		),
		mk_param(
			"bool_off_in",
			ValueType::Boolean,
			NodeValue::Boolean(false),
			Vec::new(),
		),
		mk_param(
			"combo_in",
			ValueType::Combo,
			NodeValue::Combo(1),
			vec![
				("combo_option", NodeValue::Text("One".into())),
				("combo_option", NodeValue::Text("Two".into())),
			],
		),
		mk_param(
			"combo_bare_in",
			ValueType::Combo,
			NodeValue::Combo(2),
			Vec::new(),
		),
		mk_param(
			"strcombo_bare_in",
			ValueType::StrCombo,
			NodeValue::StrCombo("Sans".into()),
			Vec::new(),
		),
		mk_param(
			"strcombo_odd_in",
			ValueType::StrCombo,
			NodeValue::Combo(0),
			Vec::new(),
		),
		mk_param(
			"text_multi_in",
			ValueType::Text,
			NodeValue::Text("hello".into()),
			vec![("multiline", NodeValue::Boolean(true))],
		),
		mk_param(
			"text_single_in",
			ValueType::Text,
			NodeValue::Text("hi".into()),
			Vec::new(),
		),
		mk_param(
			"vec2_in",
			ValueType::Vec2,
			NodeValue::Vec2([1.0, 2.0]),
			Vec::new(),
		),
		// A vector param without a vector value: components default to
		// zero.
		mk_param(
			"vec2_empty_in",
			ValueType::Vec2,
			NodeValue::None,
			Vec::new(),
		),
		// A colour param without a colour value: black, opaque.
		mk_param(
			"color_empty_in",
			ValueType::Color,
			NodeValue::None,
			Vec::new(),
		),
		mk_param(
			"vec3_in",
			ValueType::Vec3,
			NodeValue::Vec3([1.0, 2.0, 3.0]),
			Vec::new(),
		),
		mk_param(
			"color_in",
			ValueType::Color,
			NodeValue::Color([0.1, 0.2, 0.3, 0.4]),
			Vec::new(),
		),
		mk_param(
			"button_in",
			ValueType::PushButton,
			NodeValue::PushButton,
			Vec::new(),
		),
		mk_param(
			"curves_in",
			ValueType::Parametric,
			NodeValue::Text(quercus_engine_rs::ofx::curves_to_json(&[
				quercus_engine_rs::ofx::Curve::identity(0.0, 1.0),
			])),
			vec![
				("parametric_range", NodeValue::Vec2([0.0, 1.0])),
				("ui_group", NodeValue::Text("Group".into())),
				("ui_page", NodeValue::Text("Page".into())),
			],
		),
		mk_param(
			"curves_bad_in",
			ValueType::Parametric,
			NodeValue::Text("not json".into()),
			Vec::new(),
		),
		mk_param(
			"binary_in",
			ValueType::Binary,
			NodeValue::Binary(vec![1, 2]),
			Vec::new(),
		),
	];
	let controls = build_in_view(&view, visual, params);

	let by_id = |id: &str| {
		controls
			.iter()
			.find(|c| c.input_id == id)
			.unwrap_or_else(|| panic!("control {id}"))
	};
	visual.read(|cx| {
		match &by_id("int_in").kind {
			ControlKind::Slider(s) => {
				assert_eq!(s.read(cx).value(), SliderValue::Integer(5));
			}
			_ => panic!("int params build a slider"),
		}
		// The seeded value clamps into the range.
		match &by_id("float_clamped_in").kind {
			ControlKind::Slider(s) => {
				assert_eq!(s.read(cx).value(), SliderValue::Float(1.0));
			}
			_ => panic!("float params build a slider"),
		}
		match &by_id("bool_on_in").kind {
			ControlKind::CheckBox(c) => assert_eq!(c.read(cx).state(), CheckState::Checked),
			_ => panic!("boolean params build a checkbox"),
		}
		match &by_id("bool_off_in").kind {
			ControlKind::CheckBox(c) => assert_eq!(c.read(cx).state(), CheckState::Unchecked),
			_ => panic!("boolean params build a checkbox"),
		}
		match &by_id("combo_in").kind {
			ControlKind::Combo(c) => assert_eq!(c.read(cx).selected(), Some(1)),
			_ => panic!("combo params build a combo box"),
		}
		// No option list: the raw values become read-only text.
		match &by_id("combo_bare_in").kind {
			ControlKind::ReadOnly(t) => assert_eq!(t.as_ref(), "2"),
			_ => panic!("bare combos are read-only"),
		}
		match &by_id("strcombo_bare_in").kind {
			ControlKind::Combo(c) => assert_eq!(c.read(cx).selected(), Some(0)),
			_ => panic!("a string combo with a current value builds a combo"),
		}
		match &by_id("strcombo_odd_in").kind {
			ControlKind::ReadOnly(t) => assert_eq!(t.as_ref(), ""),
			_ => panic!("a non-string value renders empty"),
		}
		match &by_id("text_multi_in").kind {
			ControlKind::Text { editor, multiline } => {
				assert!(*multiline);
				assert_eq!(editor.value(cx), "hello");
			}
			_ => panic!("text params build an editor"),
		}
		match &by_id("text_single_in").kind {
			ControlKind::Text { multiline, .. } => assert!(!*multiline),
			_ => panic!("text params build an editor"),
		}
		match &by_id("vec2_in").kind {
			ControlKind::Spin(spins) => {
				assert_eq!(spins.len(), 2);
				assert_eq!(spins[0].0.read(cx).value(), SliderValue::Float(1.0));
				assert_eq!(spins[1].1, 1);
			}
			_ => panic!("vec2 params build spin boxes"),
		}
		match &by_id("vec2_empty_in").kind {
			ControlKind::Spin(spins) => {
				assert_eq!(spins.len(), 2);
				assert_eq!(spins[0].0.read(cx).value(), SliderValue::Float(0.0));
				assert_eq!(spins[1].0.read(cx).value(), SliderValue::Float(0.0));
			}
			_ => panic!("vec2 params build spin boxes"),
		}
		match &by_id("color_empty_in").kind {
			ControlKind::Color(p) => {
				assert_eq!(
					p.read(cx).committed,
					Rgba {
						r: 0.0,
						g: 0.0,
						b: 0.0,
						a: 1.0
					}
				);
			}
			_ => panic!("colour params build a picker"),
		}
		match &by_id("vec3_in").kind {
			ControlKind::Spin(spins) => {
				assert_eq!(spins.len(), 3);
				assert_eq!(spins[2].1, 2);
			}
			_ => panic!("vec3 params build spin boxes"),
		}
		match &by_id("color_in").kind {
			ControlKind::Color(p) => {
				let c = p.read(cx);
				assert!((c.committed.r - 0.1).abs() < 1e-6);
				assert!((c.committed.a - 0.4).abs() < 1e-6);
			}
			_ => panic!("colour params build a picker"),
		}
		assert!(matches!(by_id("button_in").kind, ControlKind::PushButton));
		match &by_id("curves_in").kind {
			ControlKind::Curve(editors) => assert_eq!(editors.len(), 1),
			_ => panic!("parametric params build curve editors"),
		}
		assert!(by_id("curves_in").curve_domain.is_some());
		match &by_id("curves_bad_in").kind {
			ControlKind::Curve(editors) => assert!(editors.is_empty()),
			_ => panic!("parametric params build curve editors"),
		}
		match &by_id("binary_in").kind {
			ControlKind::ReadOnly(t) => assert_eq!(t.as_ref(), ""),
			_ => panic!("binary params are read-only"),
		}
		// The section group/page travels on the control.
		assert_eq!(
			by_id("curves_in").section,
			Some(("Group".to_string(), "Page".to_string()))
		);
		assert_eq!(by_id("binary_in").section, None);
	});
}

/// `sync_values` reapplies the engine snapshot to every widget kind
/// (including hidden params a mock/plugin engine may hand over) and
/// drains a viewer eyedropper pick into the colour picker's draft.
#[gpui_kit::test]
async fn sync_values_applies_the_engine_snapshot_and_viewer_picks(cx: &mut TestAppContext) {
	let (window, view) = open_params_view(cx, 420.0, 900.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Extra controls whose input ids collide with engine params: a
	// checkbox on the hidden Boolean(true), a text field on a string
	// combo, a text field on a float (skipped), and push/read-only
	// no-ops. These are synced directly (never rendered, so no
	// duplicate element ids).
	let extras = vec![
		mk_param(
			"use_args_in",
			ValueType::Boolean,
			NodeValue::Boolean(false),
			Vec::new(),
		),
		mk_param(
			"font_family_in",
			ValueType::Text,
			NodeValue::Text(String::new()),
			Vec::new(),
		),
		mk_param(
			"font_size_in",
			ValueType::Text,
			NodeValue::Text(String::new()),
			Vec::new(),
		),
		mk_param(
			"outline_enabled_in",
			ValueType::PushButton,
			NodeValue::PushButton,
			Vec::new(),
		),
		mk_param(
			"glow_color_in",
			ValueType::Combo,
			NodeValue::Combo(0),
			Vec::new(),
		),
		// A curve control on a text-valued engine param: the JSON
		// mirror does not parse, so the sync reseeds with no curves.
		mk_param(
			"plain_text_in",
			ValueType::Parametric,
			NodeValue::Text(String::new()),
			Vec::new(),
		),
		// A picker on a numeric param: the colour arm's value guard is
		// false and the draft is left alone.
		mk_param(
			"font_size_in",
			ValueType::Color,
			NodeValue::Color([0.0, 0.0, 0.0, 1.0]),
			Vec::new(),
		),
		// A vec3 spin control on a vec2 param: the third channel has
		// no component to sync.
		mk_param(
			"pos_in",
			ValueType::Vec3,
			NodeValue::Vec3([1.0, 2.0, 3.0]),
			Vec::new(),
		),
		// A curve control on a numeric param: the JSON mirror is not
		// text, so the reseed is empty.
		mk_param(
			"font_size_in",
			ValueType::Parametric,
			NodeValue::Text(String::new()),
			Vec::new(),
		),
	];
	let extras = build_in_view(&view, visual, extras);
	visual.update(|_window, cx| {
		view.update(cx, |view, _cx| view.controls.extend(extras));
	});

	// Corrupt every widget, then let sync_values restore it.
	visual.update(|window, cx| {
		let window = window as &mut gpui_kit::Window;
		view.update(cx, |view, cx| {
			for control in &view.controls {
				match &control.kind {
					ControlKind::Slider(s) => {
						s.update(cx, |s, _| s.set_value(SliderValue::Float(0.0)));
					}
					ControlKind::CheckBox(c) => {
						c.update(cx, |c, cx| c.set_state(CheckState::Checked, cx));
					}
					ControlKind::Combo(c) => {
						c.update(cx, |c, cx| c.set_selected(Some(99), cx));
					}
					ControlKind::WheelGroup(_) => {}
					ControlKind::Spin(spins) => {
						for (s, _) in spins {
							s.update(cx, |s, cx| {
								s.set_value(SliderValue::Float(-42.0), cx);
							});
						}
					}
					ControlKind::Color(p) => {
						p.update(cx, |p, cx| {
							p.set_committed(
								Rgba {
									r: 1.0,
									g: 1.0,
									b: 1.0,
									a: 1.0,
								},
								cx,
							);
						});
					}
					ControlKind::Text { editor, .. } => {
						editor.set_text("corrupted", window, cx);
					}
					ControlKind::Curve(_) | ControlKind::PushButton | ControlKind::ReadOnly(_) => {}
				}
			}
		});
		// Arm the picker and feed it a sampled colour; the next sync
		// takes the pick into the draft.
		let picker = view.read(cx).controls.iter().find_map(|c| match &c.kind {
			ControlKind::Color(p) => Some(p.clone()),
			_ => None,
		});
		if let Some(picker) = picker {
			picker.update(cx, |picker, cx| picker.toggle_viewer_pick(cx));
		}
		let engine = view.read(cx).engine.clone();
		let green = Rgba {
			r: 0.0,
			g: 1.0,
			b: 0.0,
			a: 1.0,
		};
		engine.update(cx, |engine, cx| engine.eyedropper_picked(green, cx));
	});
	visual.update(|window, cx| {
		view.update(cx, |view, cx| view.sync_values(window, cx));
	});

	let (slider_value, check_state, combo_index, spins, custom_text, picker_state) =
		visual.read(|cx| {
			let view = view.read(cx);
			let find = |id: &str| {
				view.controls
					.iter()
					.find(|c| c.input_id == id)
					.unwrap_or_else(|| panic!("control {id}"))
			};
			let slider = match &find("font_size_in").kind {
				ControlKind::Slider(s) => s.read(cx).value(),
				_ => panic!("font_size_in"),
			};
			let check = match &find("outline_enabled_in").kind {
				ControlKind::CheckBox(c) => c.read(cx).state(),
				_ => panic!("outline_enabled_in"),
			};
			let combo = match &find("font_family_in").kind {
				ControlKind::Combo(c) => c.read(cx).selected(),
				_ => panic!("font_family_in"),
			};
			let spins = match &find("size_in").kind {
				ControlKind::Spin(spins) => spins
					.iter()
					.map(|(s, _)| s.read(cx).value())
					.collect::<Vec<_>>(),
				_ => panic!("size_in"),
			};
			// The custom text control sharing the string-combo id holds
			// the re-synced string.
			let custom_text = view
				.controls
				.iter()
				.filter(|c| c.input_id == "font_family_in")
				.find_map(|c| match &c.kind {
					ControlKind::Text { editor, .. } => Some(editor.value(cx)),
					_ => None,
				})
				.expect("the custom text control survives");
			let picker_state = view.controls.iter().find_map(|c| match &c.kind {
				ControlKind::Color(p) => {
					let p = p.read(cx);
					Some((p.draft, p.picking))
				}
				_ => None,
			});
			(slider, check, combo, spins, custom_text, picker_state)
		});
	let (draft, picking) = picker_state.expect("the colour picker");
	assert_eq!(slider_value, SliderValue::Float(72.0));
	assert_eq!(check_state, CheckState::Unchecked, "the engine holds false");
	assert_eq!(combo_index, Some(0));
	assert_eq!(
		spins,
		vec![SliderValue::Float(400.0), SliderValue::Float(300.0)]
	);
	assert_eq!(custom_text, "", "a StrCombo syncs into the text field");
	assert_eq!(
		draft,
		Rgba {
			r: 0.0,
			g: 1.0,
			b: 0.0,
			a: 1.0
		},
		"the viewer pick lands in the picker's draft"
	);
	assert!(!picking, "the pick disarms the eyedropper");

	// The hidden Boolean(true) param drives a checkbox to Checked.
	let hidden_check = visual.read(|cx| {
		view.read(cx)
			.controls
			.iter()
			.filter(|c| c.input_id == "use_args_in")
			.find_map(|c| match &c.kind {
				ControlKind::CheckBox(c) => Some(c.read(cx).state()),
				_ => None,
			})
	});
	assert_eq!(hidden_check, Some(CheckState::Checked));
}

/// The render pass lays out every control kind and emits the section
/// titles (group-only, combined, page-only, none); an empty control set
/// renders the placeholder line instead.
#[gpui_kit::test]
async fn params_view_renders_sections_and_extended_controls(cx: &mut TestAppContext) {
	let (window, view) = open_params_view(cx, 420.0, 900.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();

	let params = vec![
		mk_param(
			"sec_group_in",
			ValueType::Int,
			NodeValue::Int(3),
			vec![
				("min", NodeValue::Int(0)),
				("max", NodeValue::Int(10)),
				("ui_group", NodeValue::Text("Group".into())),
			],
		),
		mk_param(
			"sec_both_in",
			ValueType::Text,
			NodeValue::Text("x".into()),
			vec![
				("ui_group", NodeValue::Text("Group".into())),
				("ui_page", NodeValue::Text("Page".into())),
			],
		),
		mk_param(
			"sec_page_in",
			ValueType::Text,
			NodeValue::Text("y".into()),
			vec![("ui_page", NodeValue::Text("PageOnly".into()))],
		),
		mk_param(
			"plain_row_in",
			ValueType::Vec3,
			NodeValue::Vec3([1.0, 2.0, 3.0]),
			Vec::new(),
		),
		mk_param(
			"render_button_in",
			ValueType::PushButton,
			NodeValue::PushButton,
			Vec::new(),
		),
		mk_param(
			"render_ro_in",
			ValueType::Combo,
			NodeValue::Combo(0),
			Vec::new(),
		),
		mk_param(
			"render_curves_in",
			ValueType::Parametric,
			NodeValue::Text(quercus_engine_rs::ofx::curves_to_json(&[
				quercus_engine_rs::ofx::Curve::identity(0.0, 1.0),
				quercus_engine_rs::ofx::Curve::from_pairs(&[(0.0, 0.0), (1.0, 0.5)]),
			])),
			vec![("parametric_range", NodeValue::Vec2([0.0, 1.0]))],
		),
	];
	let extras = build_in_view(&view, visual, params);
	visual.update(|_window, cx| {
		view.update(cx, |view, _cx| view.controls = extras);
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(visual.debug_bounds("params-host").is_some());

	let kinds = visual.read(|cx| control_kinds(&view, cx));
	assert_eq!(
		kinds,
		vec![
			("sec_group_in".to_string(), "slider".to_string()),
			("sec_both_in".to_string(), "text".to_string()),
			("sec_page_in".to_string(), "text".to_string()),
			("plain_row_in".to_string(), "spin".to_string()),
			("render_button_in".to_string(), "button".to_string()),
			("render_ro_in".to_string(), "readonly".to_string()),
			("render_curves_in".to_string(), "curve".to_string()),
		]
	);
	let curve_count = visual.read(|cx| {
		view.read(cx)
			.controls
			.iter()
			.find(|c| c.input_id == "render_curves_in")
			.map(|c| match &c.kind {
				ControlKind::Curve(editors) => editors.len(),
				_ => 0,
			})
			.unwrap_or(0)
	});
	assert_eq!(curve_count, 2, "one editor per curve dimension");

	// A second draw re-runs sync_values over every custom control (their
	// ids have no engine param, so the sync skips them) and paints
	// again.
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// An empty view renders its placeholder text branch.
	visual.update(|_window, cx| {
		view.update(cx, |view, _cx| view.controls.clear());
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(visual.debug_bounds("params-host").is_some());
}

/// The push-button row and the text row's commit button are clickable:
/// their clicks route to `effect_push_button` / `set_effect_param` (the
/// mock engine rejects both, which the view swallows).
#[gpui_kit::test]
async fn push_button_and_text_commit_clicks_fire(cx: &mut TestAppContext) {
	let (window, view) = open_params_view(cx, 420.0, 240.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let params = vec![
		mk_param(
			"render_button_in",
			ValueType::PushButton,
			NodeValue::PushButton,
			Vec::new(),
		),
		mk_param(
			"render_text_in",
			ValueType::Text,
			NodeValue::Text("value".into()),
			Vec::new(),
		),
	];
	let extras = build_in_view(&view, visual, params);
	visual.update(|_window, cx| {
		view.update(cx, |view, _cx| view.controls = extras);
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let host = visual.debug_bounds("params-host").expect("host painted");
	// The push button is the first row, full-width under the body's p_2
	// padding.
	visual.simulate_click(
		Point::new(
			host.origin.x + host.size.width * 0.5,
			host.origin.y + px(16.0),
		),
		Modifiers::default(),
	);
	// The text row's commit button sits at the row's right edge; the
	// second row starts below the first (~28px row + 4px gap).
	visual.simulate_click(
		Point::new(
			host.origin.x + host.size.width - px(16.0),
			host.origin.y + px(50.0),
		),
		Modifiers::default(),
	);
	cx.run_until_parked();
	assert!(
		visual.debug_bounds("params-host").is_some(),
		"the view survives the clicks"
	);
	// The clicks must actually reach the engine: the mock records every
	// attempt even though it rejects the write (review §10.2 — the
	// previous version only asserted that the view survived).
	let (push, sets) = cx.read(|cx| {
		let engine = view.read(cx).engine.read(cx);
		(
			engine.effect_push_attempts(),
			engine.effect_param_attempts(),
		)
	});
	assert_eq!(
		push,
		vec![(EffectId(900), "render_button_in".to_string())],
		"the push button routes to effect_push_button"
	);
	assert!(
		sets.iter()
			.any(|(_, id, value)| id == "render_text_in" && value.contains("value")),
		"the text commit routes to set_effect_param: {sets:?}"
	);
}

/// Every wired control event routes through its engine callback:
/// sliders (int and float), checkboxes, combos (int and string),
/// spin boxes (the whole value is rebuilt from the current one), curve
/// editors (all three edit events rebuild and commit the JSON) and the
/// colour picker's commit / eyedropper events.
#[gpui_kit::test]
async fn control_events_route_through_the_engine_callbacks(cx: &mut TestAppContext) {
	use crate::widgets::curve_editor::{CurveEditorEvent, CurvePoint, CurveVec2, HandleSide};

	let (window, view) = open_params_view(cx, 420.0, 900.0);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// The mock parameter set has no int slider, int combo or curves:
	// build and wire custom ones.
	add_and_wire_in_view(
		&view,
		visual,
		vec![
			mk_param(
				"wire_int_in",
				ValueType::Int,
				NodeValue::Int(1),
				vec![("min", NodeValue::Int(0)), ("max", NodeValue::Int(10))],
			),
			mk_param(
				"wire_combo_in",
				ValueType::Combo,
				NodeValue::Combo(0),
				vec![
					("combo_option", NodeValue::Text("A".into())),
					("combo_option", NodeValue::Text("B".into())),
				],
			),
			mk_param(
				"wire_curves_in",
				ValueType::Parametric,
				NodeValue::Text(quercus_engine_rs::ofx::curves_to_json(&[
					quercus_engine_rs::ofx::Curve::identity(0.0, 1.0),
				])),
				vec![("parametric_range", NodeValue::Vec2([0.0, 1.0]))],
			),
		],
	);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let (slider, check, combo, spin, picker, int_slider, int_combo, curve) = visual.read(|cx| {
		let view = view.read(cx);
		let get = |id: &str| {
			view.controls
				.iter()
				.find(|c| c.input_id == id)
				.unwrap_or_else(|| panic!("control {id}"))
		};
		let slider = match &get("font_size_in").kind {
			ControlKind::Slider(s) => s.clone(),
			_ => panic!("font_size_in"),
		};
		let check = match &get("outline_enabled_in").kind {
			ControlKind::CheckBox(c) => c.clone(),
			_ => panic!("outline_enabled_in"),
		};
		let combo = match &get("font_family_in").kind {
			ControlKind::Combo(c) => c.clone(),
			_ => panic!("font_family_in"),
		};
		let spin = match &get("size_in").kind {
			ControlKind::Spin(spins) => spins[0].0.clone(),
			_ => panic!("size_in"),
		};
		let picker = match &get("outline_color_in").kind {
			ControlKind::Color(p) => p.clone(),
			_ => panic!("outline_color_in"),
		};
		let int_slider = match &get("wire_int_in").kind {
			ControlKind::Slider(s) => s.clone(),
			_ => panic!("wire_int_in"),
		};
		let int_combo = match &get("wire_combo_in").kind {
			ControlKind::Combo(c) => c.clone(),
			_ => panic!("wire_combo_in"),
		};
		let curve = match &get("wire_curves_in").kind {
			ControlKind::Curve(editors) => editors[0].clone(),
			_ => panic!("wire_curves_in"),
		};
		(
			slider, check, combo, spin, picker, int_slider, int_combo, curve,
		)
	});

	// Slider: both value families reach the engine as Int / Float.
	cx.update(|cx| {
		slider.update(cx, |_s, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control: 0,
				value: SliderValue::Float(12.0),
			});
		});
	});
	cx.update(|cx| {
		int_slider.update(cx, |_s, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control: 1,
				value: SliderValue::Integer(4),
			});
		});
	});
	cx.update(|cx| {
		int_slider.update(cx, |_s, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control: 1,
				value: SliderValue::Float(4.5),
			});
		});
	});

	// Checkbox.
	cx.update(|cx| {
		check.update(cx, |_c, cx| {
			cx.emit(CheckBoxEvent::Toggled {
				control: 2,
				state: CheckState::Checked,
			});
		});
	});

	// String combo: an in-range pick maps back to its string; an
	// out-of-range pick falls back to the empty string.
	cx.update(|cx| {
		combo.update(cx, |_c, cx| {
			cx.emit(ComboBoxEvent::Selected { value: 1 });
		});
	});
	cx.update(|cx| {
		combo.update(cx, |_c, cx| {
			cx.emit(ComboBoxEvent::Selected { value: 99 });
		});
	});
	// Integer combo.
	cx.update(|cx| {
		int_combo.update(cx, |_c, cx| {
			cx.emit(ComboBoxEvent::Selected { value: 1 });
		});
	});

	// Spin box: the value-changed path rebuilds the vector; the
	// edit-committed variant is ignored.
	cx.update(|cx| {
		spin.update(cx, |_s, cx| {
			cx.emit(SpinBoxEvent::ValueChanged {
				control: 3,
				value: SliderValue::Float(7.0),
			});
		});
	});
	cx.update(|cx| {
		spin.update(cx, |_s, cx| {
			cx.emit(SpinBoxEvent::EditCommitted {
				control: 3,
				value: SliderValue::Integer(9),
			});
		});
	});

	// Curve editor: every edit event rebuilds the whole curve set and
	// commits the JSON mirror. The widget owns the points, so mirror a
	// real gesture by updating its state before emitting (an emitted
	// event alone does not move the point) — this makes the JSON
	// assertion below value-falsifiable.
	cx.update(|cx| {
		curve.update(cx, |e, cx| {
			e.set_points(
				vec![
					CurvePoint::new(0.0, 0.0),
					CurvePoint::new(0.5, 0.25),
					CurvePoint::new(1.0, 1.0),
				],
				cx,
			)
		});
	});
	cx.update(|cx| {
		curve.update(cx, |_e, cx| {
			cx.emit(CurveEditorEvent::PointMoved {
				control: 4,
				index: 0,
				point: CurvePoint::new(0.1, 0.1),
			});
		});
	});
	cx.update(|cx| {
		curve.update(cx, |_e, cx| {
			cx.emit(CurveEditorEvent::HandleMoved {
				control: 4,
				index: 0,
				side: HandleSide::Out,
				handle: CurveVec2::new(0.01, 0.01),
			});
		});
	});
	cx.update(|cx| {
		curve.update(cx, |_e, cx| {
			cx.emit(CurveEditorEvent::PointAdded {
				control: 4,
				index: 2,
				point: CurvePoint::new(0.9, 0.9),
			});
		});
	});

	// Colour picker: commit (the mock rejects the set — the failure log
	// path), eyedropper arming routed to the engine, and the local
	// open/cancel no-ops.
	let red = Rgba {
		r: 1.0,
		g: 0.0,
		b: 0.0,
		a: 1.0,
	};
	cx.update(|cx| {
		picker.update(cx, |_p, cx| cx.emit(OfxColorEvent::Committed(red)));
	});
	cx.update(|cx| {
		picker.update(cx, |_p, cx| {
			cx.emit(OfxColorEvent::PickViewerToggle { armed: true });
		});
	});
	let armed = cx.read(|cx| view.read(cx).engine.read(cx).eyedropper_armed(cx));
	assert!(armed, "arming the picker arms the engine eyedropper");
	cx.update(|cx| {
		picker.update(cx, |_p, cx| {
			cx.emit(OfxColorEvent::PickViewerToggle { armed: false });
		});
	});
	let armed = cx.read(|cx| view.read(cx).engine.read(cx).eyedropper_armed(cx));
	assert!(!armed, "disarming the picker disarms the eyedropper");
	cx.update(|cx| {
		picker.update(cx, |_p, cx| cx.emit(OfxColorEvent::Opened));
	});
	cx.update(|cx| {
		picker.update(cx, |_p, cx| cx.emit(OfxColorEvent::Cancelled));
	});

	// Routing is falsifiable now: every event above must have reached the
	// engine (recorded even though the mock rejects the write).
	let sets = cx.read(|cx| view.read(cx).engine.read(cx).effect_param_attempts());
	let attempted = |id: &str| sets.iter().any(|(_, i, _)| i == id);
	for id in [
		"font_size_in",
		"wire_int_in",
		"outline_enabled_in",
		"font_family_in",
		"wire_combo_in",
		"size_in",
		"wire_curves_in",
		"outline_color_in",
	] {
		assert!(attempted(id), "no engine write for {id}: {sets:?}");
	}
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "font_size_in" && v.contains("Float(12.0)"))
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "outline_enabled_in" && v.contains("Boolean(true)"))
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "wire_int_in" && v.contains("Int(4)"))
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "wire_combo_in" && v.contains("Combo(1)"))
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "size_in" && v.contains("Vec2"))
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "wire_curves_in" && v.contains("0.25")),
		"curve JSON write missing: {sets:?}"
	);
	assert!(
		sets.iter()
			.any(|(_, i, v)| i == "outline_color_in" && v.contains("Color"))
	);
}

// -----------------------------------------------------------------------
// OfxColorPicker interactions
// -----------------------------------------------------------------------

struct PickerHost {
	picker: Entity<OfxColorPicker>,
	events: Arc<Mutex<Vec<OfxColorEvent>>>,
	_subscription: Subscription,
}

impl Render for PickerHost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.picker.clone())
	}
}

fn open_picker(
	cx: &mut TestAppContext,
	color: Rgba,
) -> (
	gpui_kit::WindowHandle<PickerHost>,
	Entity<OfxColorPicker>,
	Arc<Mutex<Vec<OfxColorEvent>>>,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(320.0), px(560.0)), |window, cx| {
		let picker = cx.new(|cx| OfxColorPicker::new(1, color, window, cx));
		let events = Arc::new(Mutex::new(Vec::new()));
		let events_sub = events.clone();
		let _subscription = cx.subscribe(
			&picker,
			move |_this, _emitter, event: &OfxColorEvent, _cx| {
				events_sub.lock().unwrap().push(*event);
			},
		);
		PickerHost {
			picker,
			events,
			_subscription,
		}
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("picker host root");
	let picker = cx.read(|cx| host.read(cx).picker.clone());
	let events = cx.read(|cx| host.read(cx).events.clone());
	(window, picker, events)
}

fn emit_slider(cx: &mut TestAppContext, slider: &Entity<Slider>, control: usize, value: f64) {
	let slider = slider.clone();
	cx.update(|cx| {
		slider.update(cx, |_s, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control,
				value: SliderValue::Float(value),
			});
		});
	});
}

fn picker_events(events: &Arc<Mutex<Vec<OfxColorEvent>>>) -> Vec<OfxColorEvent> {
	events.lock().unwrap().clone()
}

/// The picker's RGB channel sliders only mutate the draft (never the
/// committed colour), refresh the hex field, and ignore non-value /
/// out-of-range events.
#[gpui_kit::test]
async fn color_picker_rgb_sliders_update_the_draft(cx: &mut TestAppContext) {
	let (_window, picker, events) = open_picker(
		cx,
		Rgba {
			r: 0.2,
			g: 0.4,
			b: 0.6,
			a: 0.8,
		},
	);
	let committed = cx.read(|cx| picker.read(cx).committed);
	let (c0, c1, c2, a) = cx.read(|cx| {
		let p = picker.read(cx);
		(p.c0.clone(), p.c1.clone(), p.c2.clone(), p.a.clone())
	});

	emit_slider(cx, &c0, 100, 0.9);
	emit_slider(cx, &c1, 1100, 0.1);
	emit_slider(cx, &c2, 2100, 0.05);
	emit_slider(cx, &a, 3100, 0.25);

	let draft = cx.read(|cx| picker.read(cx).draft);
	assert!((draft.r - 0.9).abs() < 1e-6);
	assert!((draft.g - 0.1).abs() < 1e-6);
	assert!((draft.b - 0.05).abs() < 1e-6);
	assert!((draft.a - 0.25).abs() < 1e-6);
	assert_eq!(cx.read(|cx| picker.read(cx).committed), committed);
	assert_eq!(
		cx.read(|cx| picker.read(cx).hex.read(cx).value().to_string()),
		format_hex(draft),
		"the hex field follows the draft"
	);
	assert!(picker_events(&events).is_empty(), "no commit without OK");

	// An out-of-range channel index and non-value events are ignored.
	cx.update(|cx| {
		picker.update(cx, |picker, cx| {
			let value = SliderEvent::ValueChanged {
				control: 7,
				value: SliderValue::Float(0.5),
			};
			picker.on_slider(9, &value, cx);
			let drag = SliderEvent::DragFinished { control: 7 };
			picker.on_slider(0, &drag, cx);
		});
	});
	assert_eq!(cx.read(|cx| picker.read(cx).draft), draft);
}

/// HSV mode: the primary sliders edit hue/saturation/value, the palette
/// keeps the hue, the hue bar keeps saturation/value, and switching back
/// re-ranges the sliders.
#[gpui_kit::test]
async fn color_picker_hsv_mode_palette_and_hue_update_the_draft(cx: &mut TestAppContext) {
	let (_window, picker, _events) = open_picker(
		cx,
		Rgba {
			r: 1.0,
			g: 0.0,
			b: 0.0,
			a: 1.0,
		},
	);
	// Switching to HSV twice is a no-op the second time; switching back
	// preserves the draft.
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.set_mode(ColorMode::Hsv, cx));
		picker.update(cx, |p, cx| p.set_mode(ColorMode::Hsv, cx));
	});
	assert_eq!(cx.read(|cx| picker.read(cx).mode), ColorMode::Hsv);

	let (c0, c1, c2, a) = cx.read(|cx| {
		let p = picker.read(cx);
		(p.c0.clone(), p.c1.clone(), p.c2.clone(), p.a.clone())
	});
	emit_slider(cx, &c0, 0, 120.0);
	emit_slider(cx, &c1, 1000, 0.5);
	emit_slider(cx, &c2, 2000, 1.0);
	emit_slider(cx, &a, 3000, 0.5);
	let draft = cx.read(|cx| picker.read(cx).draft);
	let (h, s, v) = rgb_to_hsv(draft.r, draft.g, draft.b);
	assert!((h - 120.0).abs() < 1e-3, "hue {h}");
	assert!((s - 0.5).abs() < 1e-4, "saturation {s}");
	assert!((v - 1.0).abs() < 1e-4, "value {v}");
	assert!((draft.a - 0.5).abs() < 1e-6);

	// An out-of-range channel index in HSV mode is ignored.
	cx.update(|cx| {
		picker.update(cx, |picker, cx| {
			let value = SliderEvent::ValueChanged {
				control: 7,
				value: SliderValue::Float(0.5),
			};
			picker.on_slider(9, &value, cx);
		});
	});

	// The palette keeps the hue and takes saturation/value.
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.on_palette(0.25, 0.75, cx));
	});
	let draft = cx.read(|cx| picker.read(cx).draft);
	let (h, s, v) = rgb_to_hsv(draft.r, draft.g, draft.b);
	assert!((h - 120.0).abs() < 1e-3, "palette keeps the hue ({h})");
	assert!((s - 0.25).abs() < 1e-4, "palette takes s ({s})");
	assert!((v - 0.75).abs() < 1e-4, "palette takes v ({v})");

	// The hue bar keeps saturation/value.
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.on_hue(240.0, cx));
	});
	let draft = cx.read(|cx| picker.read(cx).draft);
	let (h, s, v) = rgb_to_hsv(draft.r, draft.g, draft.b);
	assert!((h - 240.0).abs() < 1e-3, "hue bar takes h ({h})");
	assert!((s - 0.25).abs() < 1e-4);
	assert!((v - 0.75).abs() < 1e-4);

	// Back to RGB: the draft survives and the mode switches.
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.set_mode(ColorMode::Rgb, cx));
	});
	assert_eq!(cx.read(|cx| picker.read(cx).mode), ColorMode::Rgb);
}

/// Commit validates a hand-typed hex string (an invalid one keeps the
/// popup open with the error hint and commits nothing), cancel discards
/// the draft, and both disarm a left-armed eyedropper.
#[gpui_kit::test]
async fn color_picker_commit_validates_hex_and_cancel_discards(cx: &mut TestAppContext) {
	let (_window, picker, events) = open_picker(
		cx,
		Rgba {
			r: 0.1,
			g: 0.2,
			b: 0.3,
			a: 1.0,
		},
	);
	// `set_value` does not emit `Change`, so run the live-parse handler
	// the Change subscription runs for a real keypress afterwards.
	let set_hex = |cx: &mut TestAppContext, text: &str| {
		cx.update(|cx| {
			picker.update(cx, |p, cx| {
				let hex = p.hex.clone();
				crate::oakui::component::text_input::set_input_text(&hex, text, cx);
			});
		});
		cx.update(|cx| {
			picker.update(cx, |p, cx| p.on_hex_edited(cx));
		});
	};
	let open = |cx: &mut TestAppContext| {
		cx.update(|cx| {
			picker.update(cx, |p, cx| {
				p.open_menu(Point::new(px(10.0), px(10.0)), cx);
			});
		});
	};

	// An external committed colour re-syncs the closed picker; an equal
	// colour is an early no-op.
	let blue = Rgba {
		r: 0.4,
		g: 0.5,
		b: 0.6,
		a: 1.0,
	};
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.set_committed(blue, cx));
	});
	assert_eq!(cx.read(|cx| picker.read(cx).draft), blue);
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.set_committed(blue, cx));
	});

	// Opening resets the draft to the committed colour.
	open(cx);
	assert!(cx.read(|cx| picker.read(cx).open));
	assert_eq!(cx.read(|cx| picker.read(cx).draft), blue);

	// Invalid hex: error hint, popup stays open, no commit.
	set_hex(cx, "#ZZZ");
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.commit(cx));
	});
	let (open_flag, hex_error, committed) = cx.read(|cx| {
		let p = picker.read(cx);
		(p.open, p.hex_error, p.committed)
	});
	assert!(open_flag && hex_error);
	assert_eq!(committed, blue, "an invalid hex commits nothing");
	assert!(
		!picker_events(&events)
			.iter()
			.any(|e| matches!(e, OfxColorEvent::Committed(_))),
		"no Committed event for invalid hex"
	);

	// Valid hex: the popup closes and the colour commits.
	set_hex(cx, "#00FF00");
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.commit(cx));
	});
	let (open_flag, committed) = cx.read(|cx| (picker.read(cx).open, picker.read(cx).committed));
	assert!(!open_flag);
	assert!(committed.r < 1e-6 && (committed.g - 1.0).abs() < 1e-6);
	assert!(
		picker_events(&events)
			.iter()
			.any(|e| matches!(e, OfxColorEvent::Committed(_)))
	);

	// While the popup is open, an external committed update must not
	// clobber the in-progress draft.
	open(cx);
	let draft_before = cx.read(|cx| picker.read(cx).draft);
	cx.update(|cx| {
		picker.update(cx, |p, cx| {
			p.set_committed(
				Rgba {
					r: 0.9,
					g: 0.9,
					b: 0.9,
					a: 1.0,
				},
				cx,
			);
		});
	});
	assert_eq!(cx.read(|cx| picker.read(cx).draft), draft_before);

	// Cancel with the eyedropper armed discloses both events.
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.toggle_viewer_pick(cx));
		picker.update(cx, |p, cx| p.close_menu(cx));
	});
	let (open_flag, picking) = cx.read(|cx| (picker.read(cx).open, picker.read(cx).picking));
	assert!(!open_flag && !picking);
	let events_now = picker_events(&events);
	assert!(
		events_now
			.iter()
			.any(|e| matches!(e, OfxColorEvent::Cancelled))
	);
	assert!(
		events_now
			.iter()
			.any(|e| matches!(e, OfxColorEvent::PickViewerToggle { armed: false }))
	);

	// Commit with the eyedropper armed disarms it too.
	open(cx);
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.toggle_viewer_pick(cx));
	});
	set_hex(cx, "");
	let draft_at_commit = cx.read(|cx| picker.read(cx).draft);
	cx.update(|cx| {
		picker.update(cx, |p, cx| p.commit(cx));
	});
	let (picking, committed, hex) = cx.read(|cx| {
		let p = picker.read(cx);
		(p.picking, p.committed, p.hex.read(cx).value().to_string())
	});
	assert!(!picking, "commit disarms the eyedropper");
	assert_eq!(committed, draft_at_commit, "an empty hex commits the draft");
	assert_eq!(hex, format_hex(committed));

	// `apply_viewer_pick` is a no-op while the eyedropper is disarmed.
	cx.update(|cx| {
		picker.update(cx, |p, cx| {
			p.apply_viewer_pick(
				Rgba {
					r: 1.0,
					g: 0.0,
					b: 0.0,
					a: 1.0,
				},
				cx,
			);
		});
	});
	assert_eq!(cx.read(|cx| picker.read(cx).draft), committed);
}

/// The popup's interactive chrome: the swatch toggles it, the RGB/HSV
/// tabs switch modes, the hue bar samples, Escape closes (or disarms a
/// picking eyedropper first), and the Cancel/OK buttons close it.
#[gpui_kit::test]
async fn color_picker_popup_swatch_tabs_escape_and_buttons(cx: &mut TestAppContext) {
	let (window, picker, events) = open_picker(
		cx,
		Rgba {
			r: 0.4,
			g: 0.2,
			b: 0.8,
			a: 0.5,
		},
	);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	let swatch_center = swatch.center();

	// Open the popup.
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(visual.debug_bounds("ofx-color-popup").is_some());

	// The mode tabs switch the primary sliders.
	let hsv_tab = visual
		.debug_bounds("ofx-color-mode-1-hsv")
		.expect("hsv tab painted");
	visual.simulate_click(hsv_tab.center(), Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert_eq!(visual.read(|cx| picker.read(cx).mode), ColorMode::Hsv);
	let rgb_tab = visual
		.debug_bounds("ofx-color-mode-1-rgb")
		.expect("rgb tab painted");
	visual.simulate_click(rgb_tab.center(), Modifiers::default());
	assert_eq!(visual.read(|cx| picker.read(cx).mode), ColorMode::Rgb);

	// The hue bar maps its vertical position to a hue.
	let hue_bar = visual
		.debug_bounds("ofx-color-hue-bar")
		.expect("hue bar painted");
	let hue_click = Point::new(
		hue_bar.origin.x + hue_bar.size.width * 0.5,
		hue_bar.origin.y + hue_bar.size.height * 0.25,
	);
	visual.simulate_click(hue_click, Modifiers::default());
	cx.run_until_parked();
	let draft = visual.read(|cx| picker.read(cx).draft);
	let (h, _, _) = rgb_to_hsv(draft.r, draft.g, draft.b);
	assert!((h - 90.0).abs() < 10.0, "hue bar click hue {h}");

	// A second click on the swatch closes the popup (cancel).
	visual.simulate_click(swatch_center, Modifiers::default());
	cx.run_until_parked();
	assert!(!visual.read(|cx| picker.read(cx).open));
	assert!(
		picker_events(&events)
			.iter()
			.any(|e| matches!(e, OfxColorEvent::Cancelled))
	);

	// Reopen and click Cancel.
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let cancel = visual
		.debug_bounds("ofx-color-cancel-1")
		.expect("cancel button painted");
	visual.simulate_click(cancel.center(), Modifiers::default());
	cx.run_until_parked();
	assert!(
		!visual.read(|cx| picker.read(cx).open),
		"the Cancel button closes the popup"
	);

	// Reopen, type an invalid hex, and click OK: the popup stays open
	// (the error hint branch paints) and nothing commits.
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	visual.update(|window, cx| {
		picker.update(cx, |p, cx| {
			let hex = p.hex.clone();
			hex.update(cx, |hex, cx| hex.set_value("#nope", window, cx));
			// `set_value` emits no `Change`; run the live-parse handler
			// the subscription runs for a real keypress.
			p.on_hex_edited(cx);
		});
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let ok = visual
		.debug_bounds("ofx-color-ok-1")
		.expect("ok button painted");
	visual.simulate_click(ok.center(), Modifiers::default());
	cx.run_until_parked();
	assert!(
		visual.read(|cx| picker.read(cx).open),
		"invalid hex keeps the popup open"
	);
	assert!(visual.read(|cx| picker.read(cx).hex_error));

	// A valid hex + OK commits and closes.
	visual.update(|window, cx| {
		picker.update(cx, |p, cx| {
			let hex = p.hex.clone();
			hex.update(cx, |hex, cx| hex.set_value("#123456", window, cx));
			p.on_hex_edited(cx);
		});
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let ok = visual
		.debug_bounds("ofx-color-ok-1")
		.expect("ok button painted");
	visual.simulate_click(ok.center(), Modifiers::default());
	cx.run_until_parked();
	assert!(
		!visual.read(|cx| picker.read(cx).open),
		"OK closes the popup"
	);
	let committed = visual.read(|cx| picker.read(cx).committed);
	assert!(
		(committed.r - 0x12 as f32 / 255.0).abs() < 1e-3,
		"OK commits the typed hex: {committed:?}"
	);

	// Escape closes the popup when the hex field has focus.
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let hex_handle = visual.read(|cx| picker.read(cx).hex.read(cx).focus_handle(cx));
	visual.update(|window, cx| {
		window.focus(&hex_handle, cx);
	});
	visual.simulate_keystrokes("escape");
	assert!(
		!visual.read(|cx| picker.read(cx).open),
		"Escape closes the popup"
	);

	// While the eyedropper is armed, Escape disarms it without closing;
	// a second Escape then closes.
	visual.simulate_click(swatch_center, Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let pick_button = visual
		.debug_bounds("ofx-color-pick-viewer-1")
		.expect("pick button painted");
	visual.simulate_click(pick_button.center(), Modifiers::default());
	cx.run_until_parked();
	assert!(visual.read(|cx| picker.read(cx).picking));
	let hex_handle = visual.read(|cx| picker.read(cx).hex.read(cx).focus_handle(cx));
	visual.update(|window, cx| {
		window.focus(&hex_handle, cx);
	});
	visual.simulate_keystrokes("escape");
	let (open_flag, picking) = visual.read(|cx| (picker.read(cx).open, picker.read(cx).picking));
	assert!(open_flag, "escape while picking keeps the popup open");
	assert!(!picking, "escape while picking disarms the eyedropper");
	visual.simulate_keystrokes("escape");
	assert!(
		!visual.read(|cx| picker.read(cx).open),
		"a second escape closes the popup"
	);
}

/// Dragging inside the S/V palette and the hue bar updates the draft
/// through the drag-move handlers (and renders the drag ghosts).
#[gpui_kit::test]
async fn color_picker_palette_and_hue_drags_update_the_draft(cx: &mut TestAppContext) {
	let (window, picker, _events) = open_picker(
		cx,
		Rgba {
			r: 0.4,
			g: 0.2,
			b: 0.8,
			a: 0.5,
		},
	);
	let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx).into_mut();
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let swatch = visual
		.debug_bounds("ofx-color-swatch")
		.expect("swatch painted");
	visual.simulate_click(swatch.center(), Modifiers::default());
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Press inside the palette, then move well past the drag threshold:
	// gpui starts the palette drag and routes the moves to its
	// `on_drag_move` handler.
	let sv = visual
		.debug_bounds("ofx-color-sv-palette")
		.expect("sv palette painted");
	let start = Point::new(sv.origin.x + px(15.0), sv.origin.y + px(15.0));
	visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
	cx.run_until_parked();
	let after_down = visual.read(|cx| picker.read(cx).draft);
	visual.simulate_mouse_move(
		Point::new(start.x + px(50.0), start.y + px(50.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	visual.simulate_mouse_move(
		Point::new(start.x + px(120.0), start.y + px(90.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	cx.run_until_parked();
	let after_sv_drag = visual.read(|cx| picker.read(cx).draft);
	assert_ne!(
		after_down, after_sv_drag,
		"dragging the palette follows the cursor"
	);
	visual.simulate_mouse_up(
		Point::new(start.x + px(120.0), start.y + px(90.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	cx.run_until_parked();

	// Same for the hue bar: the drag y position sets the hue.
	let hue = visual
		.debug_bounds("ofx-color-hue-bar")
		.expect("hue bar painted");
	let hue_start = Point::new(hue.origin.x + hue.size.width * 0.5, hue.origin.y + px(10.0));
	visual.simulate_mouse_down(hue_start, MouseButton::Left, Modifiers::default());
	visual.simulate_mouse_move(
		Point::new(hue_start.x, hue_start.y + px(100.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	visual.simulate_mouse_move(
		Point::new(hue_start.x, hue_start.y + px(130.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	cx.run_until_parked();
	let (h, _, _) = {
		let draft = visual.read(|cx| picker.read(cx).draft);
		rgb_to_hsv(draft.r, draft.g, draft.b)
	};
	assert!(
		(240.0..310.0).contains(&h),
		"the hue drag to y≈140 of the 180px bar should set ~280°, got {h}"
	);
	visual.simulate_mouse_up(
		Point::new(hue_start.x, hue_start.y + px(130.0)),
		MouseButton::Left,
		Modifiers::default(),
	);
	cx.run_until_parked();
}

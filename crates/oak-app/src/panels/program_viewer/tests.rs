use super::*;
use crate::oakui::MockEngine;
use crate::oakui::component::menu::ViewerMenuAction;
use crate::oakui::graphops;
use crate::widgets::timeline::Frame;
use crate::widgets::viewer::PlaybackClock;
use gpui_kit::{Keystroke, Modifiers, Rgba, TestAppContext, VisualTestContext, size};

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
		unsafe { std::env::set_var("HOME", path) };
		HomeGuard(previous)
	}
}

impl Drop for HomeGuard {
	fn drop(&mut self) {
		match self.0.take() {
			Some(value) => unsafe { std::env::set_var("HOME", value) },
			None => unsafe { std::env::remove_var("HOME") },
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
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
#[gpui_kit::test]
async fn viewer_events_route_through_the_panel(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
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
#[gpui_kit::test]
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
			entries.filter_map(|e| e.ok()).any(|e| {
				e.file_name()
					.to_string_lossy()
					.starts_with("oak-frame-program-")
			})
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
#[gpui_kit::test]
async fn mouse_downs_focus_and_open_the_context_menu(cx: &mut TestAppContext) {
	let (cx, _panel) = panel_window(cx);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let center = gpui_kit::point(px(320.0), px(180.0));
	cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
	cx.run_until_parked();
	cx.simulate_mouse_down(center, MouseButton::Right, Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		cx.debug_bounds("menu-popup").is_some(),
		"the viewer context menu renders"
	);
}

/// Every focused-panel transport hook, the tab button click and the
/// dock tab content.
#[gpui_kit::test]
async fn transport_hooks_and_dock_content(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
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
	assert_eq!(
		cx.update(|_, cx| panel.read(cx).panel_name()),
		crate::panels::ids::PROGRAM_VIEWER
	);

	// The header's tab buttons switch the body. They are the rightmost
	// row elements; sweep from the right edge to find them.
	let mut switched = false;
	for step in 0..320 {
		let x = 639.0 - f64::from(step) * 2.0;
		cx.simulate_click(gpui_kit::point(px(x as f32), px(14.0)), Modifiers::none());
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
#[gpui_kit::test]
async fn sync_frame_debug_and_interact_noops(cx: &mut TestAppContext) {
	let _lock = stack_lock();
	let (cx, panel) = panel_window(cx);

	let prev_debug = std::env::var_os("OAK_DEBUG_VIEWER");
	unsafe { std::env::set_var("OAK_DEBUG_VIEWER", "1") };
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	cx.run_until_parked();
	std::fs::remove_file("/tmp/oak_viewer_frame.ppm").ok();
	match prev_debug {
		Some(value) => unsafe { std::env::set_var("OAK_DEBUG_VIEWER", value) },
		None => unsafe { std::env::remove_var("OAK_DEBUG_VIEWER") },
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
#[gpui_kit::test]
async fn scopes_tab_renders_from_the_current_frame(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
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
		window.draw(cx).clear(cx);
		panel.update(cx, |panel, cx| {
			panel.tab = ProgramViewTab::Scopes;
			cx.notify();
		});
	});
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let (scope_width, scope_height, luma_len, chroma_len, bins, envelope) = cx.read(|app| {
		let panel = panel.read(app);
		let state = panel.scope_state.read(app);
		(
			state.width,
			state.height,
			state.luma.len(),
			state.chroma.len(),
			panel.histogram.read(app).bins(app),
			panel.waveform.read(app).envelope(app),
		)
	});
	let pixels = (crate::oakui::frames::SYNTH_FRAME_WIDTH
		* crate::oakui::frames::SYNTH_FRAME_HEIGHT) as usize;
	assert_eq!(scope_width, crate::oakui::frames::SYNTH_FRAME_WIDTH);
	assert_eq!(scope_height, crate::oakui::frames::SYNTH_FRAME_HEIGHT);
	assert_eq!(luma_len, pixels);
	assert_eq!(chroma_len, pixels);
	assert_eq!(bins.iter().sum::<u32>() as usize, pixels);
	assert_eq!(envelope.len(), 128);
}

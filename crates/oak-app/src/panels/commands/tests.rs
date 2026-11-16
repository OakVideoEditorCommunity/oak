use super::*;
use crate::actions::{ActionId, REGISTRY};
use crate::oakui::MockEngine;
use crate::widgets::timeline::{Frame, TimelineDataSource as _};
use gpui_kit::{AppContext as _, TestAppContext};

/// A panel that overrides nothing: every command falls through.
struct DefaultPanel;
impl PanelCommandHandler for DefaultPanel {}

/// A panel that overrides a couple of commands and records the
/// multi-camera routing arguments.
#[derive(Default)]
struct RecordingPanel {
	value: bool,
	multicam: Option<(i32, bool)>,
}

impl PanelCommandHandler for RecordingPanel {
	fn play_pause(&mut self, _cx: &mut Context<Self>) -> bool {
		self.value = true;
		self.value
	}
	fn multicam_switch(&mut self, source: i32, split_clip: bool, _cx: &mut Context<Self>) -> bool {
		self.multicam = Some((source, split_clip));
		true
	}
}

/// Every registry action dispatches onto the matching trait method,
/// and a panel that implements none of them declines all of them —
/// which also exercises every default body.
#[gpui_kit::test]
async fn default_panel_declines_every_registered_action(cx: &mut TestAppContext) {
	cx.update(|app| {
		let panel = app.new(|_| DefaultPanel);
		for entry in REGISTRY {
			let handled = panel.update(app, |panel, cx| dispatch_to(panel, entry.action, cx));
			assert!(
				!handled,
				"the default panel must decline {:?}",
				entry.action
			);
		}
	});
}

/// A panel that handles `PlayPause` stops the fall-through for that
/// action only; the multicam actions map their numbered ids onto
/// `(source, split_clip)` pairs.
#[gpui_kit::test]
async fn dispatch_propagates_handled_actions_and_multicam_arguments(cx: &mut TestAppContext) {
	cx.update(|app| {
		let panel = app.new(|_| RecordingPanel::default());

		let handled = panel.update(app, |panel, cx| dispatch_to(panel, ActionId::PlayPause, cx));
		assert!(handled, "the overridden command handles the action");
		assert!(panel.read(app).value);

		// Everything else still falls through to the shell.
		let handled = panel.update(app, |panel, cx| dispatch_to(panel, ActionId::Cut, cx));
		assert!(!handled);

		let handled = panel.update(app, |panel, cx| {
			dispatch_to(panel, ActionId::MulticamSwitch3, cx)
		});
		assert!(handled);
		assert_eq!(panel.read(app).multicam, Some((2, true)));

		let handled = panel.update(app, |panel, cx| {
			dispatch_to(panel, ActionId::MulticamSwitchNoSplit7, cx)
		});
		assert!(handled);
		assert_eq!(panel.read(app).multicam, Some((6, false)));

		// Actions with no panel command at all also decline.
		let handled = panel.update(app, |panel, cx| {
			dispatch_to(panel, ActionId::NewProject, cx)
		});
		assert!(!handled);
	});
}

/// The viewer transport maps actions onto the monitor's clock: play /
/// pause toggling, stepping, jumps to start/end and the shuttle keys.
#[gpui_kit::test]
async fn viewer_transport_drives_the_monitor_clock(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = app.new(MockEngine::create);
		let clock = engine.read(app).program_clock().clone();
		let monitor = Monitor::Program;

		// Commands the viewers do not implement fall through.
		assert!(!viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::Cut,
			app
		));

		// PlayPause toggles, and ShuttleStop forces the paused state.
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::PlayPause,
			app
		));
		assert!(clock.read(app).is_playing());
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::PlayPause,
			app
		));
		assert!(!clock.read(app).is_playing());
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::ShuttleRight,
			app
		));
		assert!(clock.read(app).is_playing());
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::ShuttleStop,
			app
		));
		assert!(!clock.read(app).is_playing());

		// Frame stepping clamps at the sequence bounds.
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::GoToStart,
			app
		));
		assert_eq!(engine.read(app).clock_frame(monitor, app), Frame::ZERO);
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::NextFrame,
			app
		));
		assert_eq!(engine.read(app).clock_frame(monitor, app), Frame(1));
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::PrevFrame,
			app
		));
		assert_eq!(engine.read(app).clock_frame(monitor, app), Frame(0));
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::ShuttleLeft,
			app
		));
		assert_eq!(engine.read(app).clock_frame(monitor, app), Frame(0));

		// GoToEnd requests one past the end; the seek clamps to the
		// last frame.
		let length = engine.read(app).sequence_length();
		assert!(viewer_transport(
			&engine,
			&clock,
			monitor,
			ActionId::GoToEnd,
			app
		));
		assert_eq!(
			engine.read(app).clock_frame(monitor, app),
			Frame(length.0 - 1)
		);

		// Work-area commands still claim the action even when the
		// engine has no work area to jump to.
		for action in [ActionId::GoToIn, ActionId::GoToOut, ActionId::PlayInToOut] {
			assert!(
				viewer_transport(&engine, &clock, monitor, action, app),
				"{action:?} is handled"
			);
		}
	});
}

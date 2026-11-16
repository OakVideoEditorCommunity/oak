use super::*;
use crate::widgets::timeline::{Frame, FrameRate};
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, px, size};

struct MockClock {
	frame: Frame,
	playing: bool,
}
impl PlaybackClock for MockClock {
	fn current_frame(&self) -> Frame {
		self.frame
	}
	fn is_playing(&self) -> bool {
		self.playing
	}
	fn frame_rate(&self) -> FrameRate {
		FrameRate::new(30, 1)
	}
}

struct Host {
	viewer: Entity<ViewerWidget<MockClock>>,
	events: Vec<ViewerEvent>,
}
impl Render for Host {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.viewer.clone())
	}
}

#[gpui_kit::test]
async fn play_button_emits_play_request(cx: &mut TestAppContext) {
	let (cx, host) = make_host(cx);
	// The play button sits on the left of the transport bar at the bottom.
	let play = cx
		.debug_bounds("gpui-widgets-viewer-play")
		.expect("play button rendered");
	cx.simulate_click(play.center(), Modifiers::none());
	cx.run_until_parked();

	let requested = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, ViewerEvent::PlayRequested { control: 1 }))
	});
	assert!(requested, "expected a PlayRequested event");
}

#[test]
fn timecode_formatting_reuses_timeline() {
	let frame = Frame(3000);
	let text = format_timecode(frame, FrameRate::new(30, 1), TimeDisplay::Timecode);
	assert_eq!(text, "00:01:40:00");
}

#[gpui_kit::test]
async fn cpu_frame_source_renders_without_a_platform_surface(cx: &mut TestAppContext) {
	// The CPU-frame path (for non-macOS platforms without CVPixelBuffer)
	// accepts raw BGRA8 bytes in a RenderImage and renders through the
	// sprite atlas — no SurfaceSource involved.
	use gpui_kit::RenderImage;
	use image::{Frame, RgbaImage};

	// A 2x2 opaque red frame, converted RGBA -> BGRA as gpui expects.
	let mut rgba = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
	for pixel in rgba.chunks_exact_mut(4) {
		pixel.swap(0, 2);
	}
	let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
		Frame::new(rgba),
		1,
	)));

	let (cx, host) = make_host(cx);
	cx.update(|window, app| {
		host.read(app)
			.viewer
			.clone()
			.update(app, |viewer, cx| viewer.set_cpu_frame(Some(frame), cx));
		let _ = window.draw(app);
	});
	cx.run_until_parked();

	let is_cpu = cx.read(|app| {
		matches!(
			host.read(app).viewer.read(app).frame_source,
			Some(ViewerFrameSource::CpuFrame(_))
		)
	});
	assert!(is_cpu, "the frame source should be the CPU-frame variant");

	// Clearing the CPU frame falls back to the placeholder.
	cx.update(|window, app| {
		host.read(app)
			.viewer
			.clone()
			.update(app, |viewer, cx| viewer.set_cpu_frame(None, cx));
		let _ = window.draw(app);
	});
	cx.run_until_parked();
	let is_none = cx.read(|app| host.read(app).viewer.read(app).frame_source.is_none());
	assert!(is_none);
}

// The surface path exists only where `SurfaceSource::Texture` does
// (Linux/FreeBSD); on macOS/Windows a registered GPU frame stays on
// the CPU-frame path by design.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[gpui_kit::test]
async fn registered_gpu_frame_switches_to_surface(cx: &mut TestAppContext) {
	// The 10-bit display path: the engine registers an RGBA16F texture
	// under the RenderImage's id, so handing that image to the viewer
	// takes the surface path (sampled straight into the swapchain) —
	// the BGRA8 bytes are only the CPU fallback. Any Send+Sync value
	// stands in for the wgpu texture here; the registry never touches
	// its contents (headless tests have no GPU).
	use gpui_kit::{DevicePixels, RenderImage, Size};
	use image::{Frame, RgbaImage};

	let rgba = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
	let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
		Frame::new(rgba),
		1,
	)));
	register_gpu_frame(
		frame.id.0,
		Arc::new(0u32),
		Size {
			width: DevicePixels::from(2),
			height: DevicePixels::from(2),
		},
	);

	let (cx, host) = make_host(cx);
	cx.update(|window, app| {
		host.read(app)
			.viewer
			.clone()
			.update(app, |viewer, cx| viewer.set_cpu_frame(Some(frame), cx));
		let _ = window.draw(app);
	});
	cx.run_until_parked();

	let is_surface = cx.read(|app| {
		matches!(
			host.read(app).viewer.read(app).frame_source,
			Some(ViewerFrameSource::Surface(_))
		)
	});
	assert!(
		is_surface,
		"a registered GPU frame must take the surface path"
	);

	// Clearing the CPU frame drops the surface source too.
	cx.update(|window, app| {
		host.read(app)
			.viewer
			.clone()
			.update(app, |viewer, cx| viewer.set_cpu_frame(None, cx));
		let _ = window.draw(app);
	});
	cx.run_until_parked();
	let is_none = cx.read(|app| host.read(app).viewer.read(app).frame_source.is_none());
	assert!(is_none);
}

#[test]
fn contain_uv_maps_letterbox() {
	// A 4x3 image inside a 72x48 box: scale = min(18, 16) = 16, so the
	// content is 64x48, letterboxed by 4px on each side.
	assert_eq!(contain_uv(4, 3, 72.0, 48.0, 0.5, 0.5), Some((0.5, 0.5)));
	assert_eq!(
		contain_uv(4, 3, 72.0, 48.0, 0.0, 0.5),
		None,
		"left letterbox"
	);
	assert_eq!(
		contain_uv(4, 3, 72.0, 48.0, 1.0, 0.5),
		None,
		"right letterbox"
	);
	// The content's right edge sits at u = 4/72 + 64/72.
	let right = contain_uv(4, 3, 72.0, 48.0, 68.0 / 72.0, 0.5).unwrap();
	assert!((right.0 - 1.0).abs() < 1e-6 && (right.1 - 0.5).abs() < 1e-6);
	// Degenerate boxes / images sample nothing.
	assert_eq!(contain_uv(0, 3, 72.0, 48.0, 0.5, 0.5), None);
	assert_eq!(contain_uv(4, 3, 0.0, 48.0, 0.5, 0.5), None);
}

#[test]
fn sample_bgra8_reads_corners() {
	// A 4x2 BGRA frame: red top-left, green top-right, blue bottom-left,
	// white/128 bottom-right.
	let mut bytes = vec![0u8; 4 * 2 * 4];
	let put = |bytes: &mut [u8], x: u32, y: u32, b: u8, g: u8, r: u8, a: u8| {
		let i = ((y * 4 + x) * 4) as usize;
		bytes[i..i + 4].copy_from_slice(&[b, g, r, a]);
	};
	put(&mut bytes, 0, 0, 0, 0, 255, 255); // red
	put(&mut bytes, 3, 0, 0, 255, 0, 255); // green
	put(&mut bytes, 0, 1, 255, 0, 0, 255); // blue
	put(&mut bytes, 3, 1, 255, 255, 255, 128); // white half-alpha

	let close = |a: Rgba, b: Rgba| {
		(a.r - b.r).abs() < 1e-6
			&& (a.g - b.g).abs() < 1e-6
			&& (a.b - b.b).abs() < 1e-6
			&& (a.a - b.a).abs() < 1e-6
	};

	let red = Rgba {
		r: 1.0,
		g: 0.0,
		b: 0.0,
		a: 1.0,
	};
	let green = Rgba {
		r: 0.0,
		g: 1.0,
		b: 0.0,
		a: 1.0,
	};
	let blue = Rgba {
		r: 0.0,
		g: 0.0,
		b: 1.0,
		a: 1.0,
	};
	let white_half = Rgba {
		r: 1.0,
		g: 1.0,
		b: 1.0,
		a: 128.0 / 255.0,
	};
	assert!(
		close(sample_bgra8(&bytes, 4, 2, 0.0, 0.0).unwrap(), red),
		"top-left"
	);
	assert!(
		close(sample_bgra8(&bytes, 4, 2, 1.0, 0.0).unwrap(), green),
		"top-right"
	);
	assert!(
		close(sample_bgra8(&bytes, 4, 2, 0.0, 1.0).unwrap(), blue),
		"bottom-left"
	);
	assert!(
		close(sample_bgra8(&bytes, 4, 2, 1.0, 1.0).unwrap(), white_half),
		"bottom-right translucent"
	);

	// Out-of-range coordinates clamp to the nearest edge pixel.
	assert!(
		close(sample_bgra8(&bytes, 4, 2, 2.0, -0.5).unwrap(), green),
		"clamps u/v"
	);
	assert!(
		close(sample_bgra8(&bytes, 4, 2, -1.0, 2.0).unwrap(), blue),
		"clamps negative"
	);
	// (0.5, 0.5) rounds to pixel (2, 1), which was never set (transparent
	// black) — exercises the rounding path.
	let mid = sample_bgra8(&bytes, 4, 2, 0.5, 0.5).unwrap();
	assert!(
		mid.r.abs() < 1e-6 && mid.a.abs() < 1e-6,
		"middle pixel unset"
	);
	// Truncated bytes sample nothing: pixel (2, 1) needs bytes 24..28, so
	// a 27-byte buffer leaves the range short.
	assert!(sample_bgra8(&bytes[..27], 4, 2, 0.5, 0.5).is_none());
}

/// While armed, clicking the picture samples the pixel under the cursor
/// into an [`ViewerEvent::EyedropperPick`] and swallows the interact
/// pointer events.
#[gpui_kit::test]
async fn armed_eyedropper_click_samples_the_picture(cx: &mut TestAppContext) {
	use gpui_kit::RenderImage;
	use image::{Frame, RgbaImage};

	// A 2x2 frame: red / green / blue / white corners, RGBA -> BGRA as
	// gpui expects.
	let mut rgba = RgbaImage::new(2, 2);
	rgba.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
	rgba.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
	rgba.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
	rgba.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
	for pixel in rgba.chunks_exact_mut(4) {
		pixel.swap(0, 2);
	}
	let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
		Frame::new(rgba),
		1,
	)));

	let (cx, host) = make_host(cx);
	cx.update(|window, app| {
		host.read(app).viewer.clone().update(app, |viewer, cx| {
			viewer.set_cpu_frame(Some(frame), cx);
			viewer.set_eyedropper_armed(true, cx);
		});
		let _ = window.draw(app);
	});
	cx.run_until_parked();

	// The 2x2 square is contained in the (wide) picture area, so the
	// picture centre maps to image pixel (1, 1) = white.
	let picture = cx
		.debug_bounds("gpui-widgets-viewer-picture")
		.expect("picture painted");
	cx.simulate_click(picture.center(), Modifiers::none());
	cx.run_until_parked();

	let (pick, no_interact) = cx.read(|app| {
		let events = &host.read(app).events;
		(
			events.iter().find_map(|e| match e {
				ViewerEvent::EyedropperPick { color } => Some(*color),
				_ => None,
			}),
			!events
				.iter()
				.any(|e| matches!(e, ViewerEvent::InteractPointer { .. })),
		)
	});
	let pick = pick.expect("an EyedropperPick should be emitted");
	assert!(
		(pick.r - 1.0).abs() < 1e-6 && (pick.g - 1.0).abs() < 1e-6 && (pick.b - 1.0).abs() < 1e-6,
		"the picture centre should sample white, got {pick:?}"
	);
	assert!(
		no_interact,
		"an armed click must not forward interact pointers"
	);
}

fn make_host(cx: &mut TestAppContext) -> (&'static mut VisualTestContext, Entity<Host>) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(640.0), px(420.0)), |window, cx| {
		let clock = cx.new(|_| MockClock {
			frame: Frame(0),
			playing: false,
		});
		let viewer = cx.new(|cx| ViewerWidget::new(1, clock, window, cx));
		let host = Host {
			viewer,
			events: Vec::new(),
		};
		cx.subscribe(
			&host.viewer,
			|host: &mut Host,
			 _v: Entity<ViewerWidget<MockClock>>,
			 event: &ViewerEvent,
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

#[gpui_kit::test]
async fn step_button_emits_step_request(cx: &mut TestAppContext) {
	let (cx, host) = make_host(cx);
	let step = cx
		.debug_bounds("gpui-widgets-viewer-step-forward")
		.expect("step button rendered");
	cx.simulate_click(step.center(), Modifiers::none());
	cx.run_until_parked();

	let requested = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, ViewerEvent::StepRequested { delta: 1, .. }))
	});
	assert!(requested, "expected a StepRequested(+1) event");
}

#[gpui_kit::test]
async fn safe_frame_toggle_emits_and_switches(cx: &mut TestAppContext) {
	let (cx, host) = make_host(cx);
	let toggle = cx
		.debug_bounds("gpui-widgets-viewer-safe")
		.expect("safe-frame button rendered");
	cx.simulate_click(toggle.center(), Modifiers::none());
	cx.run_until_parked();

	let (requested, shown) = cx.read(|app| {
		let host = host.read(app);
		(
			host.events
				.iter()
				.any(|e| matches!(e, ViewerEvent::ToggleSafeFramesRequested { .. })),
			host.viewer.read(app).safe_margins,
		)
	});
	assert!(requested, "expected a ToggleSafeFramesRequested event");
	assert_eq!(shown, SafeMargins::On, "safe margins should now be shown");
}

#[test]
fn zoom_cycles_fit_through_levels() {
	// The toolbar zoom button cycles Fit → 100% → 200% → Fit.
	assert_eq!(ViewerZoom::Fit.next(), ViewerZoom::Level(4));
	assert_eq!(ViewerZoom::Level(4).next(), ViewerZoom::Level(5));
	assert_eq!(ViewerZoom::Level(3).next(), ViewerZoom::Level(7));
	assert_eq!(ViewerZoom::Level(7).next(), ViewerZoom::Level(8));
	assert_eq!(
		ViewerZoom::Level(VIEWER_ZOOM_LEVELS.len() - 1).next(),
		ViewerZoom::Fit
	);
}

#[test]
fn safe_margins_cycle_and_ratio() {
	assert_eq!(SafeMargins::Off.next(), SafeMargins::On);
	assert_eq!(SafeMargins::On.next(), SafeMargins::Off);
	assert_eq!(SafeMargins::Custom(0.5, 0.5).next(), SafeMargins::Off);
	assert_eq!(SafeMargins::Off.ratio(), None);
	assert_eq!(SafeMargins::On.ratio(), Some((0.9, 0.8)));
	assert_eq!(SafeMargins::Custom(0.7, 0.6).ratio(), Some((0.7, 0.6)));
}

#[test]
fn waveform_mode_config_round_trips() {
	assert_eq!(WaveformMode::Automatic.config_value(), 0);
	assert_eq!(WaveformMode::Only.config_value(), 1);
	assert_eq!(WaveformMode::Both.config_value(), 2);
	assert_eq!(WaveformMode::from_config_value(0), WaveformMode::Automatic);
	assert_eq!(WaveformMode::from_config_value(1), WaveformMode::Only);
	assert_eq!(WaveformMode::from_config_value(2), WaveformMode::Both);
	// Out-of-range values clamp to the nearest valid mode.
	assert_eq!(WaveformMode::from_config_value(-5), WaveformMode::Automatic);
	assert_eq!(WaveformMode::from_config_value(9), WaveformMode::Both);
}

#[test]
fn fps_counter_smooths_measured_rate() {
	let mut counter = FpsCounter::new();
	let t0 = std::time::Instant::now();
	counter.record(t0);
	assert_eq!(counter.value(), 0.0, "no rate before a second sample");
	// 100ms apart => 10 fps instant.
	counter.record(t0 + std::time::Duration::from_millis(100));
	assert!((counter.value() - 10.0).abs() < 1.0);
	counter.record(t0 + std::time::Duration::from_millis(200));
	assert!((counter.value() - 10.0).abs() < 1.5);
}

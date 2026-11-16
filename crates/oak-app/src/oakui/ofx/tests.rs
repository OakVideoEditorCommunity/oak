use super::*;

mod ofx_key {
	pub use quercus_engine_rs::ofx::key::{
		A as KEY_A, BACKSPACE as KEY_BACKSPACE, DELETE as KEY_DELETE, DOWN as KEY_DOWN,
		END as KEY_END, ESCAPE as KEY_ESCAPE, F1 as KEY_F1, HOME as KEY_HOME, LEFT as KEY_LEFT,
		PAGE_DOWN as KEY_PAGE_DOWN, PAGE_UP as KEY_PAGE_UP, RETURN as KEY_RETURN,
		RIGHT as KEY_RIGHT, SPACE as KEY_SPACE, TAB as KEY_TAB, UNKNOWN as KEY_UNKNOWN,
		UP as KEY_UP, Z as KEY_Z,
	};
}

#[test]
fn viewer_time_and_extent_are_safe_to_update() {
	update_viewer_time(42.5, 10.0, 200.0);
	update_project_extent(1280.0, 720.0);
}

#[test]
fn cancel_is_safe_and_idempotent() {
	cancel_plugin_render();
	cancel_plugin_render();
}

// ---------------------------------------------------------------------------
// Interact viewport mapping / key symbols / compositing (pure)
// ---------------------------------------------------------------------------

use gpui_kit::{point, size};

/// 视口像素 → pen 坐标：the picture shows the frame with a contain fit,
/// so a 2:1 area/frame scale maps local pixels 1:2 to frame pixels, and
/// the letterbox returns None.
#[test]
fn viewport_pixel_to_pen_maps_contain_fit() {
	let area = size(640.0, 360.0);
	let frame = size(320.0, 180.0);
	// Frame fills the area at scale 2 (no letterbox): local 40,40 → 20,20.
	let p = viewport_pixel_to_pen(point(40.0, 40.0), area, frame);
	assert_eq!(p, Some((20.0, 20.0)));
	// Corner maps to the frame corner.
	let p = viewport_pixel_to_pen(point(0.0, 0.0), area, frame);
	assert_eq!(p, Some((0.0, 0.0)));
	let p = viewport_pixel_to_pen(point(640.0, 360.0), area, frame);
	// Exclusive upper bound: exactly the far edge is outside the frame.
	assert_eq!(p, None);
}

/// Letterboxing: a 4:3 area showing a 16:9 frame leaves vertical bars;
/// pointers in the bars map to None, the scaled rect maps 1:2.
#[test]
fn viewport_pixel_to_pen_respects_letterbox() {
	let area = size(640.0, 480.0);
	let frame = size(320.0, 180.0);
	// Scale = min(640/320, 480/180) = 2; the frame occupies 640×360,
	// leaving 60px top/bottom bars.
	let p = viewport_pixel_to_pen(point(60.0, 240.0), area, frame);
	assert_eq!(p, Some((30.0, 90.0)));
	// Inside the top letterbox bar.
	let p = viewport_pixel_to_pen(point(320.0, 10.0), area, frame);
	assert_eq!(p, None);
	// Zero-size inputs.
	assert_eq!(
		viewport_pixel_to_pen(point(0.0, 0.0), size(0.0, 0.0), frame),
		None
	);
	assert_eq!(
		viewport_pixel_to_pen(point(0.0, 0.0), area, size(0.0, 0.0)),
		None
	);
}

/// Key mapping: the common keys land on the ofxKeySyms values; printable
/// single characters carry their ASCII symbol + the character string.
#[test]
fn key_symbol_maps_common_keys() {
	let ks = |key: &str| gpui_kit::Keystroke::parse(key).unwrap();
	// Alphanumerics: symbol = ASCII code, string = the char.
	assert_eq!(key_symbol(&ks("a")), (ofx_key::KEY_A, "a".to_string()));
	assert_eq!(key_symbol(&ks("z")), (ofx_key::KEY_Z, "z".to_string()));
	assert_eq!(key_symbol(&ks("1")), (b'1' as i32, "1".to_string()));
	// Named keys: symbol only, empty string.
	assert_eq!(
		key_symbol(&ks("space")),
		(ofx_key::KEY_SPACE, String::new())
	);
	assert_eq!(
		key_symbol(&ks("enter")),
		(ofx_key::KEY_RETURN, String::new())
	);
	assert_eq!(
		key_symbol(&ks("escape")),
		(ofx_key::KEY_ESCAPE, String::new())
	);
	assert_eq!(
		key_symbol(&ks("backspace")),
		(ofx_key::KEY_BACKSPACE, String::new())
	);
	assert_eq!(
		key_symbol(&ks("delete")),
		(ofx_key::KEY_DELETE, String::new())
	);
	assert_eq!(key_symbol(&ks("left")), (ofx_key::KEY_LEFT, String::new()));
	assert_eq!(
		key_symbol(&ks("right")),
		(ofx_key::KEY_RIGHT, String::new())
	);
	assert_eq!(key_symbol(&ks("up")), (ofx_key::KEY_UP, String::new()));
	assert_eq!(key_symbol(&ks("down")), (ofx_key::KEY_DOWN, String::new()));
	assert_eq!(key_symbol(&ks("home")), (ofx_key::KEY_HOME, String::new()));
	assert_eq!(key_symbol(&ks("end")), (ofx_key::KEY_END, String::new()));
	assert_eq!(
		key_symbol(&ks("pageup")),
		(ofx_key::KEY_PAGE_UP, String::new())
	);
	assert_eq!(
		key_symbol(&ks("pagedown")),
		(ofx_key::KEY_PAGE_DOWN, String::new())
	);
	assert_eq!(key_symbol(&ks("tab")), (ofx_key::KEY_TAB, String::new()));
	assert_eq!(key_symbol(&ks("f1")), (ofx_key::KEY_F1, String::new()));
	assert_eq!(
		key_symbol(&ks("f12")),
		(ofx_key::KEY_F1 + 11, String::new())
	);
	// Unknown multi-character keys.
	let (sym, s) = key_symbol(&ks("insert"));
	assert_eq!(sym, ofx_key::KEY_UNKNOWN);
	assert!(s.is_empty());
}

/// Straight-alpha "over" compositing: the overlay replaces the base where
/// it is opaque, blends where it is translucent, and stays 1.0 alpha.
#[test]
fn composite_overlay_blends_alpha() {
	// Opaque red over blue → red.
	let overlay = [1.0, 0.0, 0.0, 1.0];
	let base = [0.0, 0.0, 1.0, 1.0];
	let out = composite_overlay(&overlay, &base).unwrap();
	assert_eq!(out, [1.0, 0.0, 0.0, 1.0]);
	// Half-alpha red over blue → 0.5 red + 0.5 blue.
	let overlay = [1.0, 0.0, 0.0, 0.5];
	let out = composite_overlay(&overlay, &base).unwrap();
	for (i, expected) in [(0, 0.5), (1, 0.0), (2, 0.5), (3, 1.0)] {
		assert!(
			(out[i] - expected).abs() < 1e-6,
			"channel {i}: {} != {expected}",
			out[i]
		);
	}
	// Transparent overlay → base unchanged.
	let overlay = [0.0, 0.0, 0.0, 0.0];
	assert_eq!(composite_overlay(&overlay, &base).unwrap(), base);
	// Length mismatch → None.
	assert!(composite_overlay(&[0.0; 4], &[0.0; 8]).is_none());
}

/// The engine frame's BGRA8 → F32 RGBA conversion round-trips through
/// the viewer format (the compositing path's input).
#[test]
fn bgra_frame_roundtrips_to_f32_rgba() {
	let (w, h) = (2u32, 1u32);
	let samples = [1.0, 0.0, 0.5, 1.0, 0.25, 0.5, 0.75, 1.0];
	let img = super::super::frames::f32_rgba_to_bgra_image(w, h, &samples);
	let (got_w, got_h, rgba) = bgra_image_to_f32_rgba(&img).unwrap();
	assert_eq!((got_w, got_h), (w, h));
	for (i, expected) in [
		(0, 1.0),
		(1, 0.0),
		(2, 0.5),
		(3, 1.0),
		(4, 0.25),
		(5, 0.5),
		(6, 0.75),
		(7, 1.0),
	] {
		assert!(
			(rgba[i] - expected).abs() < 0.01,
			"channel {i}: {} != {expected}",
			rgba[i]
		);
	}
}

// ---------------------------------------------------------------------------
// End-to-end with the minimal test plugin (cbits/oak_test_plugin.c)
// ---------------------------------------------------------------------------

const INTERACT_PLUGIN_ID: &str = "org.oak.test-plugin.interact";
/// The plugin records every interact action into the file this env var
/// names.
const MARKER_ENV: &str = "OAK_TEST_PLUGIN_INTERACT_MARKER";
/// Serializes the process-global Host singleton across host-touching
/// tests (the plugin cache is a process singleton with no lock).
static HOST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Saves the marker env var and restores it (including the was-unset
/// case) on drop, so a panicking assertion cannot leak the redirect into
/// the next serialized host test.
struct MarkerEnvGuard(Option<std::ffi::OsString>);

impl MarkerEnvGuard {
	fn set(path: &std::path::Path) -> MarkerEnvGuard {
		let previous = std::env::var_os(MARKER_ENV);
		unsafe { std::env::set_var(MARKER_ENV, path) };
		MarkerEnvGuard(previous)
	}
}

impl Drop for MarkerEnvGuard {
	fn drop(&mut self) {
		match self.0.take() {
			Some(value) => unsafe { std::env::set_var(MARKER_ENV, value) },
			None => unsafe { std::env::remove_var(MARKER_ENV) },
		}
	}
}

/// The minimal test plugin's scan directory (the bundle is assembled
/// from the dylib the app build script compiles into OUT_DIR) }, or `None`
/// when the plugin is unavailable (the test skips).
fn test_plugin_scan_dir() -> Option<std::path::PathBuf> {
	let out = std::path::PathBuf::from(env!("OUT_DIR"));
	let lib = if cfg!(target_os = "macos") {
		out.join("oak_test_plugin.dylib")
	} else {
		out.join("oak_test_plugin.so")
	};
	if !lib.is_file() {
		return None;
	}
	let bundle = std::env::temp_dir()
		.join(format!("oak-app-test-plugin-{}", std::process::id()))
		.join("oak-test-plugin.ofx.bundle");
	let platform = if cfg!(target_os = "macos") {
		"MacOS"
	} else if cfg!(target_os = "windows") {
		"Win64"
	} else {
		"Linux-x86-64"
	};
	let bin_dir = bundle.join("Contents").join(platform);
	std::fs::create_dir_all(&bin_dir).ok()?;
	// Windows needs the .dll extension: LoadLibrary appends ".dll" to
	// extension-less module names, so a bare "plugin" file never loads.
	let target = bin_dir.join(if cfg!(target_os = "windows") {
		"plugin.dll"
	} else {
		"plugin"
	});
	if !target.exists() {
		std::fs::copy(&lib, &target).ok()?;
	}
	Some(bundle.parent().unwrap().to_path_buf())
}

fn scan_interact_plugin() -> bool {
	let Some(dir) = test_plugin_scan_dir() else {
		println!("SKIP: minimal test plugin not built");
		return false;
	};
	if !quercus_engine_rs::ofx::scan_path(&dir.to_string_lossy()) {
		println!("SKIP: test plugin scan failed");
		return false;
	}
	quercus_engine_rs::ofx::register_plugin_nodes();
	true
}

fn marker_path(tag: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!(
		"oak-app-interact-{}-{}.log",
		std::process::id(),
		tag
	))
}

fn read_marker(path: &std::path::Path) -> Vec<String> {
	std::fs::read_to_string(path)
		.unwrap_or_default()
		.lines()
		.map(|l| l.to_string())
		.collect()
}

/// App-layer interact creation + synthetic event forwarding, asserting
/// the plugin really received each action and its arguments (the marker
/// file the plugin appends to).
#[test]
fn interact_e2e_lifecycle_and_event_forwarding() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	if !scan_interact_plugin() {
		return;
	}
	let marker = marker_path("e2e");
	let _ = std::fs::remove_file(&marker);
	let _marker_env = MarkerEnvGuard::set(&marker);

	// (1) app-layer creation: `sync_active_interact` on the plugin
	// instance handle creates the interact (new_interact → describe →
	// create_instance).
	let inst = quercus_engine_rs::ofx::Instance::create(INTERACT_PLUGIN_ID, None)
		.expect("interact variant instance");
	let handle = inst.id();
	sync_active_interact(Some(handle));

	let (active_handle, interact, _viewport) = active_interact().expect("active interact created");
	assert_eq!(active_handle, handle);

	// (2) synthetic mouse forwarding: the picture-local → pen mapping
	// (the panel's forward path) then the pen actions, with the pen-down
	// state carried by the move's button state.
	let area = size(640.0, 360.0);
	let frame = size(320.0, 180.0);
	let (px, py) =
		viewport_pixel_to_pen(point(40.0, 40.0), area, frame).expect("contain fit maps 2:1");
	assert_eq!((px, py), (20.0, 20.0));
	assert_eq!(interact.pen_motion((px, py), true, 5.0), status::OK);
	assert_eq!(interact.pen_down((px, py), 5.0), status::OK);
	assert_eq!(interact.pen_up((px, py), 5.0), status::OK);

	// (3) keys and idle.
	assert_eq!(
		interact.key_down(quercus_engine_rs::ofx::key::A, "a", 5.0),
		status::OK
	);
	assert_eq!(
		interact.key_up(quercus_engine_rs::ofx::key::A, "a", 5.0),
		status::OK
	);
	assert_eq!(interact.idle(), status::OK);

	// (4) clearing the selection destroys the interact.
	sync_active_interact(None);
	assert!(
		active_interact().is_none(),
		"clearing the selection should destroy the active interact"
	);

	// Wait for the destroy record: the active-interact slot is
	// process-global, and a concurrent viewer frame sync (another
	// test's real-engine viewer ticking in this binary) can take the
	// interact out of the slot and destroy it a few milliseconds
	// after our own clearing. Keep the marker env var set and poll —
	// the contract (destroy reaches the plugin, in order) is still
	// asserted in full below.
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
	let lines = loop {
		let lines = read_marker(&marker);
		if lines.iter().any(|l| l == "destroy") || std::time::Instant::now() >= deadline {
			break lines;
		}
		std::thread::sleep(std::time::Duration::from_millis(10));
	};

	let _ = std::fs::remove_file(&marker);

	// Lifecycle reached the plugin, in order.
	let seq = ["new_interact", "describe", "create", "destroy"];
	let pos: Vec<usize> = seq
		.iter()
		.map(|s| lines.iter().position(|l| l == s))
		.collect::<Option<Vec<_>>>()
		.unwrap_or_else(|| panic!("lifecycle actions missing: {lines:?}"));
	assert!(
		pos.windows(2).all(|w| w[0] < w[1]),
		"lifecycle order should be new_interact→describe→create→destroy"
	);

	// The forwarded events with real arguments (C %g drops trailing
	// zeros).
	assert!(
		lines
			.iter()
			.any(|l| l == "pen_motion vp=20,20 canon=20,20 pressure=1"),
		"pen_motion not recorded with pen-down state: {lines:?}"
	);
	assert!(
		lines
			.iter()
			.any(|l| l == "pen_down vp=20,20 canon=20,20 pressure=1"),
		"pen_down not recorded: {lines:?}"
	);
	assert!(
		lines
			.iter()
			.any(|l| l == "pen_up vp=20,20 canon=20,20 pressure=0"),
		"pen_up not recorded: {lines:?}"
	);
	assert!(
		lines.iter().any(|l| l == "key_down sym=97 str=a"),
		"key_down not recorded: {lines:?}"
	);
	assert!(
		lines.iter().any(|l| l == "idle"),
		"idle not recorded: {lines:?}"
	);

	quercus_engine_rs::ofx::shutdown();
}

/// End-to-end overlay: the plugin's draw action really renders into the
/// offscreen FBO, and the composite over a synthetic base frame carries
/// the plugin-drawn colours (macOS real GL; gated by `OAK_GPU_TESTS`).
#[test]
fn interact_e2e_draw_overlay_composites_plugin_colours() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	if !scan_interact_plugin() {
		return;
	}
	if !cfg!(target_os = "macos") || std::env::var_os("OAK_GPU_TESTS").is_none() {
		println!("SKIP: draw overlay needs macOS GL (set OAK_GPU_TESTS)");
		return;
	}
	let marker = marker_path("draw");
	let _ = std::fs::remove_file(&marker);
	let _marker_env = MarkerEnvGuard::set(&marker);

	let inst = quercus_engine_rs::ofx::Instance::create(INTERACT_PLUGIN_ID, None)
		.expect("interact variant instance");
	let handle = inst.id();
	sync_active_interact(Some(handle));
	let (_, interact, _) = active_interact().expect("active interact created");

	// A 64×64 opaque blue base frame.
	let (w, h) = (64u32, 64u32);
	let mut base_samples = vec![0.0f32; (w * h * 4) as usize];
	for px in base_samples.chunks_mut(4) {
		px.copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
	}
	let base = Arc::new(super::super::frames::f32_rgba_to_bgra_image(
		w,
		h,
		&base_samples,
	));
	let viewport = InteractViewport::at_frame_size(w, h);
	let composite = draw_interact_composite(handle, &interact, &viewport, 0.0, &base)
		.expect("GL overlay composite");

	// The test plugin clears to opaque dark grey (0.05) and draws a
	// solid rectangle (0.9,0.1,0.2) at canonical 10..30 — pixel scale 1
	// maps it to pixels 10..30. The opaque overlay fully replaces the
	// base frame.
	let bytes = composite.as_bytes(0).expect("composite bytes");
	let px_at = |x: u32, y: u32| {
		let i = ((y * w + x) * 4) as usize;
		(
			bytes[i + 2] as f32 / 255.0,
			bytes[i + 1] as f32 / 255.0,
			bytes[i] as f32 / 255.0,
		)
	};
	let inside = px_at(20, 20);
	assert!(
		(inside.0 - 0.9).abs() < 0.03
			&& (inside.1 - 0.1).abs() < 0.03
			&& (inside.2 - 0.2).abs() < 0.03,
		"plugin-drawn rectangle should composite at (20,20): {inside:?}"
	);
	let outside = px_at(5, 5);
	assert!(
		(outside.0 - 0.05).abs() < 0.03
			&& (outside.1 - 0.05).abs() < 0.03
			&& (outside.2 - 0.05).abs() < 0.03,
		"plugin clear colour should fill the rest: {outside:?}"
	);

	// The draw action reached the plugin with the viewport args.
	let lines = read_marker(&marker);
	let _ = std::fs::remove_file(&marker);
	assert!(
		lines.iter().any(|l| l.starts_with("draw vp=64x64")),
		"draw not recorded with viewport: {lines:?}"
	);

	sync_active_interact(None);
	quercus_engine_rs::ofx::shutdown();
}

// ---------------------------------------------------------------------------
// Pure helpers / idle pump
// ---------------------------------------------------------------------------

/// `InteractViewport::at_frame_size` is the 1:1 viewport of a frame.
#[test]
fn interact_viewport_at_frame_size_is_unit_scale() {
	let vp = InteractViewport::at_frame_size(1920, 1080);
	assert_eq!(vp.width, 1920.0);
	assert_eq!(vp.height, 1080.0);
	assert_eq!(vp.pixel_scale, (1.0, 1.0));
}

/// A `RenderImage` without CPU bytes converts to `None` (the viewer
/// shows the base frame unchanged).
#[test]
fn bgra_image_to_f32_rgba_returns_none_without_bytes() {
	let empty = RenderImage::new(smallvec::SmallVec::<[image::Frame; 1]>::new());
	assert!(bgra_image_to_f32_rgba(&empty).is_none());
}

/// `composite_overlay` rejects buffers that are not whole RGBA pixels
/// and clamps out-of-range alpha.
#[test]
fn composite_overlay_rejects_unaligned_and_clamps_alpha() {
	assert!(
		composite_overlay(&[0.0, 0.0], &[0.0, 0.0]).is_none(),
		"a two-sample buffer is not an RGBA pixel"
	);
	// Alpha above one clamps to fully opaque (the overlay wins).
	let out = composite_overlay(&[1.0, 0.0, 0.0, 2.0], &[0.0, 0.0, 1.0, 1.0]).unwrap();
	assert_eq!(out, [1.0, 0.0, 0.0, 1.0]);
	// Alpha below zero clamps to fully transparent (the base stays).
	let out = composite_overlay(&[1.0, 0.0, 0.0, -1.0], &[0.0, 0.0, 1.0, 1.0]).unwrap();
	assert_eq!(out, [0.0, 0.0, 1.0, 1.0]);
}

/// The f-key mapping parses f1..f35; out-of-range or unparseable
/// suffixes fall through to the unknown symbol.
#[test]
fn key_symbol_function_keys_and_unknown_multi_char_keys() {
	let ks = |key: &str| gpui_kit::Keystroke::parse(key).unwrap();
	assert_eq!(
		key_symbol(&ks("f35")),
		(ofx_key::KEY_F1 + 34, String::new())
	);
	assert_eq!(
		key_symbol(&ks("f36")),
		(ofx_key::KEY_UNKNOWN, String::new())
	);
	assert_eq!(key_symbol(&ks("f0")), (ofx_key::KEY_UNKNOWN, String::new()));
	// A bare "f" is the printable character, not a function key.
	assert_eq!(key_symbol(&ks("f")), (b'f' as i32, "f".to_string()));
	assert_eq!(
		key_symbol(&ks("f99999999999999999999")),
		(ofx_key::KEY_UNKNOWN, String::new())
	);
}

/// The idle pump is a no-op without an active interact and throttles
/// repeat calls: two back-to-back calls must not run the idle action
/// twice.
#[test]
fn pump_interact_idle_is_inert_without_an_interact() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	// Neither call may panic; with no interact installed they only
	// exercise the throttle guard.
	pump_interact_idle();
	pump_interact_idle();
}

/// `sync_active_interact`: an unknown instance handle and a cleared
/// target are no-ops (no interact is installed).
#[test]
fn sync_active_interact_missing_instance_is_a_noop() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	assert!(
		quercus_engine_rs::ofx::create_interact(u64::MAX - 7).is_none(),
		"the probe handle is not registered"
	);
	// The node/instance is gone: nothing to attach.
	sync_active_interact(Some(u64::MAX - 7));
	// A cleared target with a (possibly) empty slot: nothing to destroy.
	sync_active_interact(None);
}

/// `init` is idempotent: a second scan/registration must not duplicate
/// plugin node types, and it installs the app-side bridges.
#[test]
fn init_wiring_is_idempotent() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let first = init();
	let second = init();
	assert_eq!(
		second, 0,
		"a second init must not register duplicates (the first registered {first} plugin type(s))"
	);
	assert!(
		quercus_engine_rs::ofx::has_progress_handler(),
		"init installs the progress reporter factory"
	);
}

// ---------------------------------------------------------------------------
// Interact lifecycle / composite early returns (needs the test plugin)
// ---------------------------------------------------------------------------

/// The app-layer interact management with a real plugin instance:
/// re-targeting the same instance is a no-op, `note_interact_viewport`
/// updates only the matching instance, and `draw_interact_composite`
/// reports `None` for an inert viewport/frame (zero size, mismatch, no
/// CPU bytes) without touching GL.
#[test]
fn interact_viewport_tracking_and_composite_early_returns() {
	let _lock = HOST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	if !scan_interact_plugin() {
		return;
	}
	let inst = quercus_engine_rs::ofx::Instance::create(INTERACT_PLUGIN_ID, None)
		.expect("interact variant instance");
	let handle = inst.id();
	sync_active_interact(Some(handle));

	// Re-targeting the same instance keeps the live interact (the
	// unchanged-target early return).
	sync_active_interact(Some(handle));
	let (active_handle, interact, _) = active_interact().expect("active interact");
	assert_eq!(active_handle, handle);

	// The viewport tracker updates the matching instance...
	let vp = InteractViewport::at_frame_size(64, 36);
	note_interact_viewport(handle, vp);
	assert_eq!(active_interact().expect("active interact").2, vp);
	// ... and ignores other instances.
	note_interact_viewport(
		handle.wrapping_add(1),
		InteractViewport::at_frame_size(1, 1),
	);
	assert_eq!(active_interact().expect("active interact").2, vp);

	// A 2×2 base frame.
	let samples = vec![0.25f32; 2 * 2 * 4];
	let base = super::super::frames::f32_rgba_to_bgra_image(2, 2, &samples);

	// A zero-size viewport is inert before any GL work.
	let zero = InteractViewport {
		width: 0.0,
		height: 0.0,
		pixel_scale: (1.0, 1.0),
	};
	assert!(draw_interact_composite(handle, &interact, &zero, 0.0, &base).is_none());

	// Viewport/frame size mismatch.
	let mismatch = InteractViewport::at_frame_size(4, 4);
	assert!(draw_interact_composite(handle, &interact, &mismatch, 0.0, &base).is_none());

	// A base image without CPU bytes.
	let empty = RenderImage::new(smallvec::SmallVec::<[image::Frame; 1]>::new());
	let matching = InteractViewport::at_frame_size(2, 2);
	assert!(draw_interact_composite(handle, &interact, &matching, 0.0, &empty).is_none());

	// The in-process GL path: headless Linux has no GL implementation,
	// so this reports None; a GL-capable host may composite. Either way
	// it must not panic.
	let _ = draw_interact_composite(handle, &interact, &matching, 0.0, &base);

	sync_active_interact(None);

	// A plugin that does not declare an overlay interact reports
	// `new_interact() == None` and stays inert.
	let plain = quercus_engine_rs::ofx::Instance::create("org.oak.test-plugin", None)
		.expect("plain effect variant instance");
	let plain_handle = plain.id();
	sync_active_interact(Some(plain_handle));
	assert!(
		active_interact().is_none(),
		"a plugin without an interact installs nothing"
	);

	quercus_engine_rs::ofx::shutdown();
}

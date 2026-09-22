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

//! OFX plugin startup wiring (stage 6b).
//!
//! The app is the only place that holds both the oakplugin host and the UI
//! services the OFX suites consult at runtime, so the wiring lives here:
//!
//! - [`init`] scans the standard plugin paths ([`oak_plugin::host::Host`]
//!   default path set, `host.rs:440-449`), registers every discovered
//!   plugin into the node factory (the effect library and the add-effect
//!   menu consume those entries), installs the render executor and the
//!   plugin-node duplicator (both idempotent), and registers the
//!   progress-reporter factory plus the active-viewer provider.
//! - [`update_project_extent`] / [`update_viewer_time`] keep the
//!   oakplugin side's fallback project size and timeline time in sync with
//!   the current sequence (the engine calls them on open / seek / tick).
//! - [`set_progress_tx`] wires a progress-event channel the app drains in
//!   its tick loop to drive the progress dialog.
//!
//! Every failure degrades to a log: plugin support is an optional
//! capability, never a startup dependency.
//!
//! ## Rendering topology and progress
//!
//! Preview/export rendering runs through the process-isolated oak-worker
//! pool (M15 S2), so plugin rendering happens in the worker process where
//! this main-process reporter factory is not in effect. The wiring still
//! serves the in-process render paths (e.g. the test-only inline backend).
//! Worker-side progress is forwarded over the control plane: the worker
//! installs its own reporter factory whose reporters stream
//! `plugin_progress` NDJSON events to the dispatcher ([`init`] registers
//! the dispatcher callback, which feeds the same channel as the inline
//! reporter); the dialog's Cancel button broadcasts `plugin_cancel` to the
//! workers ([`cancel_plugin_render`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gpui::{Keystroke, Point, RenderImage, Size};
use oak_plugin::progress::{ReporterFactory, UiProgressReporter};
use oak_plugin::suites::interact::Interact;
use oak_plugin::suites::status;
use oak_plugin::suites::timeline::{ActiveViewerProvider, ViewerTimeInfo};

/// One progress event a plugin reporter pushed to the app channel (drained
/// by the app tick, which drives a progress dialog).
#[derive(Debug, Clone, Default)]
pub struct PluginProgressEvent {
	/// The label the plugin passed to progressStart.
	pub label: String,
	/// The message the plugin passed to progressStart.
	pub message: String,
	/// Progress fraction in 0.0..=1.0.
	pub fraction: f64,
}

/// The app's progress-event channel (registered by [`set_progress_tx`]).
static PROGRESS_TX: OnceLock<Mutex<Option<std::sync::mpsc::Sender<PluginProgressEvent>>>> =
	OnceLock::new();

/// The sticky cancel flag read by every live reporter (`update` returns
/// false once set; the progress dialog's cancel button sets it).
static CANCEL: AtomicBool = AtomicBool::new(false);

/// The last active-viewer time snapshot (the timeline-suite provider reads
/// it; the engine refreshes it on seek / tick).
static VIEWER_TIME: OnceLock<Mutex<ViewerTimeInfo>> = OnceLock::new();

/// The last known project extent (normalised-coordinate default conversion;
/// the engine refreshes it whenever the sequence changes).
static PROJECT_EXTENT: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

fn viewer_slot() -> &'static Mutex<ViewerTimeInfo> {
	VIEWER_TIME.get_or_init(|| {
		Mutex::new(ViewerTimeInfo {
			time: 0.0,
			range_min: 0.0,
			range_max: 0.0,
		})
	})
}

fn extent_slot() -> &'static Mutex<(f64, f64)> {
	PROJECT_EXTENT.get_or_init(|| Mutex::new((1920.0, 1080.0)))
}

// ---------------------------------------------------------------------------
// App-driven state sync
// ---------------------------------------------------------------------------

/// Wires the app's progress-event channel into the OFX progress suite. The
/// app keeps the receiving half and drains it in its tick loop.
pub fn set_progress_tx(tx: std::sync::mpsc::Sender<PluginProgressEvent>) {
	*PROGRESS_TX
		.get_or_init(|| Mutex::new(None))
		.lock()
		.unwrap_or_else(|e| e.into_inner()) = Some(tx);
}

/// Clone of the registered sender, or `None` before
/// [`set_progress_tx`] (a reporter then silently continues).
fn progress_tx() -> Option<std::sync::mpsc::Sender<PluginProgressEvent>> {
	PROGRESS_TX
		.get_or_init(|| Mutex::new(None))
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.clone()
}

/// Updates the active-viewer time snapshot the timeline suite falls back
/// to when no render context is live (engine seek / tick path).
pub fn update_viewer_time(time: f64, range_min: f64, range_max: f64) {
	*viewer_slot().lock().unwrap_or_else(|e| e.into_inner()) = ViewerTimeInfo {
		time,
		range_min,
		range_max,
	};
}

/// Updates the project extent (width/height) the OFX normalised-coordinate
/// default conversion uses, and pushes it into oakplugin.
pub fn update_project_extent(width: f64, height: f64) {
	let (w, h) = (width.max(1.0), height.max(1.0));
	*extent_slot().lock().unwrap_or_else(|e| e.into_inner()) = (w, h);
	oak_plugin::node_factory::set_project_extent(w, h);
}

/// Requests cancellation of the running plugin render (the progress
/// dialog's Cancel button). The next progressStart resets the flag. Also
/// forwards the cancel to the render workers (their progress reporters
/// answer false from the next batch boundary on).
pub fn cancel_plugin_render() {
	CANCEL.store(true, Ordering::Relaxed);
	// The worker-process plugin renders are cancelled through the control
	// plane (`plugin_cancel`); the in-flight frame completes (batch
	// granularity — the worker processes messages between batches).
	oak_render::procpool::request_plugin_cancel_all();
}

// ---------------------------------------------------------------------------
// Reporters / providers
// ---------------------------------------------------------------------------

/// A reporter that forwards (label, message, fraction) to the app channel
/// and honours the global cancel flag.
struct ChannelProgressReporter {
	tx: Option<std::sync::mpsc::Sender<PluginProgressEvent>>,
	label: String,
	message: String,
}

impl UiProgressReporter for ChannelProgressReporter {
	fn update(&mut self, progress: f64) -> bool {
		if let Some(tx) = &self.tx {
			let _ = tx.send(PluginProgressEvent {
				label: self.label.clone(),
				message: self.message.clone(),
				fraction: progress,
			});
		}
		!CANCEL.load(Ordering::Relaxed)
	}
}

fn reporter_factory() -> ReporterFactory {
	Arc::new(|label, message| {
		// A fresh render begins: reset the sticky cancel flag.
		CANCEL.store(false, Ordering::Relaxed);
		Box::new(ChannelProgressReporter {
			tx: progress_tx(),
			label: label.to_string(),
			message: message.to_string(),
		})
	})
}

fn viewer_provider() -> ActiveViewerProvider {
	Arc::new(|| {
		let info = *viewer_slot().lock().unwrap_or_else(|e| e.into_inner());
		Some(info)
	})
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Idempotent OFX startup wiring. Scans the standard plugin directories,
/// registers every discovered plugin into the node factory, installs the
/// render executor / duplicator, and registers the progress factory and
/// the active-viewer provider. Returns the number of plugin node types
/// registered (0 when no plugins were discovered or the scan failed).
pub fn init() -> usize {
	// 1. Scan the standard OFX plugin directories (host.rs:440-449 default
	//    path set: ~/.OFX/Plugins, ~/.local/share/OFX/Plugins, ...
	//    plus the OLIVE_OFX_PLUGIN_PATH / OLIVE_PLUGIN_PATH /
	//    OFX_PLUGIN_PATH environment variables). A scan failure only
	//    logs — plugins are optional.
	if let Err(e) = oak_plugin::host::Host::global().cache.scan() {
		eprintln!("[ofx] plugin scan failed: {e}");
	}
	// 2. Register discovered plugins into the node factory (idempotent;
	//    also installs the render executor and the plugin-node duplicator).
	let registered = oak_plugin::node_factory::register_plugin_nodes();
	// 3. Progress reporter factory -> the app progress channel.
	oak_plugin::progress::set_reporter_factory(Some(reporter_factory()));
	// 4. Active-viewer time provider (timeline suite fallback).
	oak_plugin::suites::timeline::set_active_viewer_provider(Some(viewer_provider()));
	// 5. Worker-forwarded plugin progress (the render workers run plugin
	//    renders in their own process and stream `plugin_progress` NDJSON
	//    events over the control plane; the dispatcher hands them to this
	//    callback, which feeds the same channel the inline reporter uses).
	oak_render::procpool::set_plugin_progress_cb(Some(Arc::new(|label, message, fraction| {
		if let Some(tx) = progress_tx() {
			let _ = tx.send(PluginProgressEvent {
				label,
				message,
				fraction,
			});
		}
	})));
	// 6. Project extent (the engine refreshes it whenever the sequence
	//    changes; keep the oakplugin side in sync with the default).
	let (w, h) = *extent_slot().lock().unwrap_or_else(|e| e.into_inner());
	oak_plugin::node_factory::set_project_extent(w, h);
	registered.len()
}

// ---------------------------------------------------------------------------
// Main-process Interact instance management (the program viewer's overlay +
// event target)
// ---------------------------------------------------------------------------

/// The viewport parameters of an interact: the plugin's draw/pen coordinate
/// space. `width`/`height` are the viewport size in pixels (the displayed
/// frame's pixel grid), `pixel_scale` is the canonical→pixel ratio (1.0 at
/// 1:1, the offscreen buffer's scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractViewport {
	/// Viewport width in pixels.
	pub width: f64,
	/// Viewport height in pixels.
	pub height: f64,
	/// Canonical→pixel scale (`(1.0, 1.0)` at 1:1).
	pub pixel_scale: (f64, f64),
}

impl InteractViewport {
	/// The 1:1 viewport matching a frame of `width`×`height` pixels.
	pub fn at_frame_size(width: u32, height: u32) -> Self {
		Self {
			width: width as f64,
			height: height as f64,
			pixel_scale: (1.0, 1.0),
		}
	}
}

/// The app-side holder of the selected effect's live interact.
///
/// The interact is created on the *main process* `oak_plugin::Instance` of
/// the selected plugin effect node (the same registry the inspector's
/// push-button path uses), distinct from the worker-process render
/// instances — OFX allows a plugin to have several instances, and the
/// interact is a UI-event host, not a renderer.
struct ActiveInteract {
	/// The oakplugin instance registry key (the plugin node's
	/// `plugin_instance_handle`).
	instance: u64,
	/// The live interact (created via `Instance::new_interact`).
	interact: Arc<Interact>,
	/// The viewport of the last overlay draw (updated on every successful
	/// composite; the source of [`active_interact`]'s viewport).
	viewport: InteractViewport,
}

/// The single active interact: at most one plugin's custom UI is on the
/// program viewer at a time (the inspector's current selection target).
static ACTIVE_INTERACT: OnceLock<Mutex<Option<ActiveInteract>>> = OnceLock::new();

fn active_interact_slot() -> &'static Mutex<Option<ActiveInteract>> {
	ACTIVE_INTERACT.get_or_init(|| Mutex::new(None))
}

/// Recomputes the active interact for the current selection target. The
/// program viewer calls this from its frame sync with the selected OFX
/// effect's plugin instance handle (or `None` when the selection has no
/// plugin candidate — no selected clip, the chain has no OFX plugin card,
/// or the project closed).
///
/// Creates the interact on the target instance (`new_interact` →
/// describe → create_instance; a plugin without an interact yields `None`
/// and stays inert — a normal no-op), and destroys the previous one when
/// the target changed. No-op while the target is unchanged.
pub fn sync_active_interact(instance: Option<u64>) {
	// Unchanged target: nothing to do.
	{
		let mut slot = active_interact_slot()
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		if slot
			.as_ref()
			.is_some_and(|active| Some(active.instance) == instance)
		{
			return;
		}
		// Target changed or cleared: destroy the old interact first (the
		// kOfxActionDestroyInstanceInteract notification; idempotent). The
		// app lock is released before the plugin call — a plugin callback
		// must never re-enter this slot.
		if let Some(active) = slot.take() {
			drop(slot);
			active.interact.destroy();
		}
	}
	let Some(id) = instance else {
		return;
	};
	let Some(inst) = oak_plugin::node_factory::instance_from_id(id) else {
		// The node/instance is gone (effect deleted): nothing to attach to.
		return;
	};
	let Some(interact) = inst.value.new_interact() else {
		// The plugin has no interact: normal no-op.
		return;
	};
	// Complete the instance sequence (ofxInteract.h: NewInteract →
	// Describe → CreateInstance before any draw/pen/key action).
	interact.describe();
	interact.create_instance();
	let mut slot = active_interact_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	*slot = Some(ActiveInteract {
		instance: id,
		interact,
		viewport: InteractViewport {
			width: 0.0,
			height: 0.0,
			pixel_scale: (1.0, 1.0),
		},
	});
}

/// The active interact: `(instance, interact, viewport)`, or `None` when
/// no interact is live (no selection target, or the plugin has no
/// interact). The viewport reports the last drawn size and updates as the
/// viewer's frame size changes.
pub fn active_interact() -> Option<(u64, Arc<Interact>, InteractViewport)> {
	let slot = active_interact_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	slot.as_ref()
		.map(|a| (a.instance, a.interact.clone(), a.viewport))
}

/// Records the viewport the interact was last drawn at (keeps
/// [`active_interact`]'s viewport current).
fn note_interact_viewport(instance: u64, viewport: InteractViewport) {
	let mut slot = active_interact_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	if let Some(active) = slot.as_mut() {
		if active.instance == instance {
			active.viewport = viewport;
		}
	}
}

// ---------------------------------------------------------------------------
// Overlay rendering + compositing
// ---------------------------------------------------------------------------

/// Converts the viewer's BGRA8 [`RenderImage`] (the engine's display
/// format) into tightly packed F32 RGBA `(width, height, samples)` for the
/// compositing path. Returns `None` when the image has no CPU bytes.
fn bgra_image_to_f32_rgba(img: &RenderImage) -> Option<(u32, u32, Vec<f32>)> {
	let bytes = img.as_bytes(0)?;
	let size = img.size(0);
	let w = size.width.0 as usize;
	let h = size.height.0 as usize;
	let expected = w * h * 4;
	if bytes.len() < expected {
		return None;
	}
	let mut out = Vec::with_capacity(expected);
	for px in bytes[..expected].as_chunks::<4>().0 {
		out.push(px[2] as f32 / 255.0); // R
		out.push(px[1] as f32 / 255.0); // G
		out.push(px[0] as f32 / 255.0); // B
		out.push(px[3] as f32 / 255.0); // A
	}
	Some((w as u32, h as u32, out))
}

/// Reads an `oakplugin` F32 RGBA image into a tightly packed `Vec<f32>`.
fn read_image_f32(img: &oak_plugin::image::Image) -> Vec<f32> {
	img.pixels()
		.as_chunks::<4>()
		.0
		.iter()
		.map(|c| f32::from_ne_bytes(c[0..4].try_into().unwrap()))
		.collect()
}

/// Composites the straight-alpha overlay over `base` (both tightly packed
/// F32 RGBA, same length) with non-premultiplied "source-over":
///
/// ```text
/// out.rgb = src.rgb * src.a + dst.rgb * (1 - src.a)
/// out.a   = src.a + dst.a * (1 - src.a)
/// ```
///
/// # Why straight alpha
///
/// The plugin draws into a fresh RGBA32F FBO with GL blend factors
/// `SRC_ALPHA` / `ONE_MINUS_SRC_ALPHA` (ofxDrawSuite.h's "over"
/// compositing) after a transparent-black clear, so the readback holds
/// *straight* (non-premultiplied) alpha. The formula above is the matching
/// straight-alpha "over". `base` is the displayed frame (alpha 1.0), so
/// the result alpha stays 1.0. Returns `None` on a length mismatch.
pub fn composite_overlay(overlay: &[f32], base: &[f32]) -> Option<Vec<f32>> {
	if overlay.len() != base.len() || !overlay.len().is_multiple_of(4) {
		return None;
	}
	let mut out = vec![0.0f32; overlay.len()];
	for i in (0..overlay.len()).step_by(4) {
		let a = overlay[i + 3].clamp(0.0, 1.0);
		let dst_a = base[i + 3].clamp(0.0, 1.0);
		out[i] = overlay[i] * a + base[i] * (1.0 - a);
		out[i + 1] = overlay[i + 1] * a + base[i + 1] * (1.0 - a);
		out[i + 2] = overlay[i + 2] * a + base[i + 2] * (1.0 - a);
		out[i + 3] = a + dst_a * (1.0 - a);
	}
	Some(out)
}

/// Renders the interact's overlay into a fresh offscreen FBO (RGBA32F) and
/// returns the frame composited over `base` as a BGRA8 [`RenderImage`] for
/// the viewer.
///
/// # The GL sequence
///
/// ```text
/// app acquire            → CGL current (whole sequence)
/// create_output_texture  → RGBA32F output texture (real GL name)
/// create_fbo / bind      → the plugin draws into this FBO
/// clear to transparent 0 → straight-alpha "over" of the strokes
/// Interact::draw         → the plugin's draw action (re-acquires GL
///                          re-entrantly on the same thread, draws via
///                          native GL + the Draw suite)
/// read_pixels_to_image   → straight-alpha F32 RGBA overlay
/// composite_overlay      → alpha "over" the current frame
/// ```
///
/// The app holds one guard across the whole sequence so the readback stays
/// on the plugin's context; `Interact::draw` re-enters the guard per its
/// own contract (gl_bridge nesting avoids the deadlock). Returns `None`
/// when the interact is inert — no GL, a viewport/frame mismatch, the
/// plugin failed, or draw returned a non-OK status — in which case the
/// caller shows the base frame unchanged.
pub fn draw_interact_composite(
	instance: u64,
	interact: &Interact,
	viewport: &InteractViewport,
	time: f64,
	base: &RenderImage,
) -> Option<Arc<RenderImage>> {
	let (w, h) = (viewport.width as i32, viewport.height as i32);
	if w <= 0 || h <= 0 {
		return None;
	}
	let (bw, bh, base_f32) = bgra_image_to_f32_rgba(base)?;
	if bw != w as u32 || bh != h as u32 {
		return None;
	}
	let mut params = oak_plugin::render::VideoParams::default();
	params.width = w;
	params.height = h;
	params.format = oak_plugin::render::PIXEL_FORMAT_F32;

	let _guard = oak_plugin::gl_bridge::acquire().ok()?;
	let tex = oak_plugin::gl_bridge::create_output_texture(w, h, &params).ok()?;
	let fbo = match oak_plugin::gl_bridge::create_fbo(tex, w, h) {
		Ok(fbo) => fbo,
		Err(_) => {
			oak_plugin::gl_bridge::delete_gl_texture(tex);
			return None;
		}
	};
	oak_plugin::gl_bridge::bind_fbo(fbo);
	oak_plugin::gl_bridge::set_viewport(w, h);
	// Clear to transparent black: the plugin's strokes composite "over"
	// nothing, so the readback holds straight alpha (see
	// [`composite_overlay`]).
	oak_plugin::gl_bridge::gl_clear_color(0.0, 0.0, 0.0, 0.0);
	oak_plugin::gl_bridge::gl_clear();

	let st = interact.draw(
		(viewport.width, viewport.height),
		viewport.pixel_scale,
		time,
		// Background image: Phase 1 passes no composited background (the
		// interact draws over a flat transparent clear; the frame is
		// composited on the app side afterwards).
		None,
	);
	let overlay = oak_plugin::gl_bridge::read_pixels_to_image(w, h, &params);
	oak_plugin::gl_bridge::delete_fbo(fbo);
	oak_plugin::gl_bridge::delete_gl_texture(tex);
	let (Ok(overlay), st) = (overlay, st) else {
		return None;
	};
	if st != status::OK {
		return None;
	}
	let merged = composite_overlay(&read_image_f32(&overlay), &base_f32)?;
	note_interact_viewport(instance, *viewport);
	Some(Arc::new(super::frames::f32_rgba_to_bgra_image(
		bw, bh, &merged,
	)))
}

// ---------------------------------------------------------------------------
// Event forwarding (pen + key) and the idle pump
// ---------------------------------------------------------------------------

/// Maps a pointer position in the picture area (local pixels, top-left
/// origin) to the OFX pen coordinates — the plugin's viewport pixels (the
/// displayed frame's pixel grid). The picture shows the frame with a
/// "contain" fit (letterboxed), so the mapping is the inverse of the
/// object-fit scale plus centering. Returns `None` when the pointer is in
/// the letterbox (outside the frame's pixel rect).
pub fn viewport_pixel_to_pen(
	local: Point<f32>,
	area: Size<f32>,
	frame: Size<f32>,
) -> Option<(f64, f64)> {
	let (aw, ah) = (area.width as f64, area.height as f64);
	let (fw, fh) = (frame.width as f64, frame.height as f64);
	if aw <= 0.0 || ah <= 0.0 || fw <= 0.0 || fh <= 0.0 {
		return None;
	}
	let scale = (aw / fw).min(ah / fh);
	let offset_x = (aw - fw * scale) / 2.0;
	let offset_y = (ah - fh * scale) / 2.0;
	let fx = (local.x as f64 - offset_x) / scale;
	let fy = (local.y as f64 - offset_y) / scale;
	if fx < 0.0 || fy < 0.0 || fx >= fw || fy >= fh {
		return None;
	}
	Some((fx, fy))
}

/// Maps a gpui keystroke to the OFX key symbol (`ofxKeySyms.h` values from
/// `oak_plugin::host::KEY_*`) and the key-string character
/// (`kOfxPropKeyString`: the UTF-8 character, empty for keys without one).
///
/// Covers the common keys — alphanumerics, the arrows, return, escape,
/// backspace/delete, tab, home/end, page up/down and the function keys;
/// anything else maps to [`oak_plugin::host::KEY_UNKNOWN`] with an empty
/// string.
pub fn key_symbol(keystroke: &Keystroke) -> (i32, String) {
	use oak_plugin::host as ofx_key;
	let key = keystroke.key.as_str();
	let named = match key {
		"space" => Some(ofx_key::KEY_SPACE),
		"enter" | "return" => Some(ofx_key::KEY_RETURN),
		"escape" => Some(ofx_key::KEY_ESCAPE),
		"backspace" => Some(ofx_key::KEY_BACKSPACE),
		"delete" => Some(ofx_key::KEY_DELETE),
		"tab" => Some(ofx_key::KEY_TAB),
		"home" => Some(ofx_key::KEY_HOME),
		"end" => Some(ofx_key::KEY_END),
		"left" => Some(ofx_key::KEY_LEFT),
		"right" => Some(ofx_key::KEY_RIGHT),
		"up" => Some(ofx_key::KEY_UP),
		"down" => Some(ofx_key::KEY_DOWN),
		"pageup" => Some(ofx_key::KEY_PAGE_UP),
		"pagedown" => Some(ofx_key::KEY_PAGE_DOWN),
		_ => None,
	};
	if let Some(sym) = named {
		return (sym, String::new());
	}
	// Function keys: f1..f35 → KEY_F1 + (n-1).
	if let Some(rest) = key.strip_prefix('f') {
		if let Ok(n) = rest.parse::<u32>() {
			if (1..=35).contains(&n) {
				return (ofx_key::KEY_F1 + (n as i32 - 1), String::new());
			}
		}
	}
	// Printable single-character keys (letters/digits/punctuation): the
	// symbol is the ASCII code (ofxKeySyms maps printable ASCII 1:1;
	// KEY_A=0x61 … KEY_Z=0x7a, KEY_SPACE=0x20), the string is the char.
	if key.len() == 1 && key.is_ascii() {
		return (key.as_bytes()[0] as i32, key.to_string());
	}
	(ofx_key::KEY_UNKNOWN, String::new())
}

/// The idle pump for the active interact. The program viewer calls this
/// from its throttled timer (~10 Hz, not every render): the app forwards
/// `kOfxInteractActionIdle` so the plugin can run lightweight UI work
/// (marquee feedback, cursor animation). No-op without an active interact.
pub fn pump_interact_idle() {
	use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
	// Throttle to ~10 Hz: the panel timer may fire faster than the OFX
	// idle cadence, and idle is meant to run when the app is otherwise
	// quiet, not every animation frame.
	static LAST_IDLE_MS: AtomicU64 = AtomicU64::new(0);
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_millis() as u64)
		.unwrap_or(0);
	if now.saturating_sub(LAST_IDLE_MS.load(AtomicOrdering::Relaxed)) < 100 {
		return;
	}
	LAST_IDLE_MS.store(now, AtomicOrdering::Relaxed);
	if let Some((_, interact, _)) = active_interact() {
		let _ = interact.idle();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn viewer_time_roundtrip() {
		update_viewer_time(42.5, 10.0, 200.0);
		let provider = viewer_provider();
		let info = provider().expect("provider always reports a snapshot");
		assert_eq!(info.time, 42.5);
		assert_eq!((info.range_min, info.range_max), (10.0, 200.0));
	}

	#[test]
	fn cancel_is_sticky_until_a_new_reporter() {
		CANCEL.store(false, Ordering::Relaxed);
		let factory = reporter_factory();
		let mut a = factory("a", "b");
		assert!(a.update(0.1));
		cancel_plugin_render();
		assert!(!a.update(0.5), "a cancelled render reports no");
		// A fresh progressStart resets the flag.
		let mut b = factory("a", "b");
		assert!(b.update(0.1));
	}

	#[test]
	fn project_extent_is_forwarded() {
		update_project_extent(1280.0, 720.0);
		let slot = extent_slot();
		assert_eq!(*slot.lock().unwrap(), (1280.0, 720.0));
	}

	// ---------------------------------------------------------------------------
	// Interact viewport mapping / key symbols / compositing (pure)
	// ---------------------------------------------------------------------------

	use gpui::{point, size};

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
		use oak_plugin::host as ofx_key;
		let ks = |key: &str| gpui::Keystroke::parse(key).unwrap();
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
	/// from the dylib the app build script compiles into OUT_DIR), or `None`
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
		if oak_plugin::host::Host::global()
			.cache
			.scan_path(&dir)
			.is_err()
		{
			println!("SKIP: test plugin scan failed");
			return false;
		}
		oak_plugin::node_factory::register_plugin_nodes();
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
		let inst = oak_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact variant instance");
		let handle = oak_plugin::node_factory::register_instance(inst);
		sync_active_interact(Some(handle));

		let (active_handle, interact, _viewport) =
			active_interact().expect("active interact created");
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
			interact.key_down(oak_plugin::host::KEY_A, "a", 5.0),
			status::OK
		);
		assert_eq!(
			interact.key_up(oak_plugin::host::KEY_A, "a", 5.0),
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

		oak_plugin::host::Host::global().shutdown();
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

		let inst = oak_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact variant instance");
		let handle = oak_plugin::node_factory::register_instance(inst);
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
		oak_plugin::host::Host::global().shutdown();
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

	/// `read_image_f32` unpacks the plugin image's native-endian F32 RGBA
	/// bytes.
	#[test]
	fn read_image_f32_unpacks_native_float_samples() {
		use oak_plugin::image::{BitDepth, Components, Image};
		use oak_plugin::instance::OfxRectD;
		let mut img = Image::allocate(
			BitDepth::Float,
			Components::Rgba,
			OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: 2.0,
				y2: 1.0,
			},
		);
		let samples: [f32; 8] = [0.25, 0.5, 0.75, 1.0, -1.0, 2.0, 0.0, 0.5];
		let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
		img.pixels_mut()[..bytes.len()].copy_from_slice(&bytes);
		assert_eq!(read_image_f32(&img), samples);
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
		use oak_plugin::host as ofx_key;
		let ks = |key: &str| gpui::Keystroke::parse(key).unwrap();
		assert_eq!(
			key_symbol(&ks("f35")),
			(ofx_key::KEY_F1 + 34, String::new())
		);
		assert_eq!(key_symbol(&ks("f36")), (ofx_key::KEY_UNKNOWN, String::new()));
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
			oak_plugin::node_factory::instance_from_id(u64::MAX - 7).is_none(),
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
			oak_plugin::progress::has_reporter_factory(),
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
		let inst = oak_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact variant instance");
		let handle = oak_plugin::node_factory::register_instance(inst);
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
		let plain = oak_plugin::host::Host::global()
			.create_instance("org.oak.test-plugin", None)
			.expect("plain effect variant instance");
		let plain_handle = oak_plugin::node_factory::register_instance(plain);
		sync_active_interact(Some(plain_handle));
		assert!(
			active_interact().is_none(),
			"a plugin without an interact installs nothing"
		);

		oak_plugin::host::Host::global().shutdown();
	}
}

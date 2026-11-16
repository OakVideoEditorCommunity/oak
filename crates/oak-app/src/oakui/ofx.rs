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

//! SDK OFX lifecycle and interact display adapters.
//!
//! The engine owns plugin discovery, node registration, progress/cancel,
//! viewer time and project extent. The app keeps only its progress channel,
//! viewport/key mapping and GL overlay compositing. Every failure degrades to
//! the base frame or a log: plugin support is an optional capability.
//!
//! ## Rendering topology and progress
//!
//! Preview/export rendering now runs in-process (no worker process), so
//! plugin execution happens in the main process where
//! this main-process reporter factory is not in effect. The wiring still
//! serves the in-process render paths (e.g. the test-only inline backend).
//! Worker-side progress is forwarded over the control plane: the worker
//! installs its own reporter factory whose reporters stream
//! `plugin_progress` NDJSON events to the dispatcher ([`init`] registers
//! the dispatcher callback, which feeds the same channel as the inline
//! reporter); the dialog's Cancel button broadcasts `plugin_cancel` to the
//! workers ([`cancel_plugin_render`]).

use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::{Keystroke, Point, RenderImage, Size};
use quercus_engine_rs::ofx::{Interact, status};

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
	quercus_engine_rs::ofx::set_viewer_time(time, range_min, range_max);
}

/// Updates the project extent (width/height) the OFX normalised-coordinate
/// default conversion uses, and pushes it into oakplugin.
pub fn update_project_extent(width: f64, height: f64) {
	quercus_engine_rs::ofx::set_project_extent(width, height);
}

/// Requests cancellation of the running plugin render (the progress
/// dialog's Cancel button). The next progressStart resets the flag. Also
/// forwards the cancel to the render workers (their progress reporters
/// answer false from the next batch boundary on).
pub fn cancel_plugin_render() {
	quercus_engine_rs::ofx::cancel_render();
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
	if quercus_engine_rs::ofx::scan().is_none() {
		eprintln!("[ofx] plugin scan failed");
	}
	let registered = quercus_engine_rs::ofx::register_plugin_nodes();
	quercus_engine_rs::ofx::install_render_executor();
	quercus_engine_rs::ofx::set_progress_handler(
		|label, message, fraction| {
			if let Some(tx) = progress_tx() {
				let _ = tx.send(PluginProgressEvent {
					label: label.to_string(),
					message: message.to_string(),
					fraction,
				});
			}
			true
		},
		|| {},
	);
	quercus_engine_rs::ofx::set_project_extent(1920.0, 1080.0);
	registered
}

/// Unregisters progress callbacks, destroys interacts, then closes the host.
pub fn shutdown() {
	sync_active_interact(None);
	quercus_engine_rs::ofx::clear_progress_handler();
	quercus_engine_rs::ofx::shutdown();
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

/// The app-side holder of the selected effect's SDK interact, distinct from
/// worker-process render instances.
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
	let Some(interact) = quercus_engine_rs::ofx::create_interact(id) else {
		// The plugin has no interact: normal no-op.
		return;
	};
	let mut slot = active_interact_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	*slot = Some(ActiveInteract {
		instance: id,
		interact: Arc::new(interact),
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
	use quercus_engine_rs::ofx::gl;
	let _guard = gl::acquire()?;
	let tex = gl::create_output_texture(w, h)?;
	let fbo = match gl::create_fbo(tex, w, h) {
		Some(fbo) => fbo,
		None => {
			gl::delete_texture(tex);
			return None;
		}
	};
	gl::bind_fbo(fbo);
	gl::set_viewport(w, h);
	// Clear to transparent black: the plugin's strokes composite "over"
	// nothing, so the readback holds straight alpha (see
	// [`composite_overlay`]).
	gl::clear_color(0.0, 0.0, 0.0, 0.0);
	gl::clear();

	let st = interact.draw(
		(viewport.width, viewport.height),
		viewport.pixel_scale,
		time,
	);
	let mut overlay = vec![0.0f32; (w * h * 4) as usize];
	let read = gl::read_pixels_f32(w, h, &mut overlay);
	gl::delete_fbo(fbo);
	gl::delete_texture(tex);
	if st != status::OK || read.is_none() {
		return None;
	}
	let merged = composite_overlay(&overlay, &base_f32)?;
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

/// Maps a gpui keystroke to the SDK OFX key symbol and key-string character
/// (`kOfxPropKeyString`: the UTF-8 character, empty for keys without one).
///
/// Covers the common keys — alphanumerics, the arrows, return, escape,
/// backspace/delete, tab, home/end, page up/down and the function keys;
/// anything else maps to the SDK's unknown key with an empty string.
pub fn key_symbol(keystroke: &Keystroke) -> (i32, String) {
	use quercus_engine_rs::ofx::key as ofx_key;
	let key = keystroke.key.as_str();
	let named = match key {
		"space" => Some(ofx_key::SPACE),
		"enter" | "return" => Some(ofx_key::RETURN),
		"escape" => Some(ofx_key::ESCAPE),
		"backspace" => Some(ofx_key::BACKSPACE),
		"delete" => Some(ofx_key::DELETE),
		"tab" => Some(ofx_key::TAB),
		"home" => Some(ofx_key::HOME),
		"end" => Some(ofx_key::END),
		"left" => Some(ofx_key::LEFT),
		"right" => Some(ofx_key::RIGHT),
		"up" => Some(ofx_key::UP),
		"down" => Some(ofx_key::DOWN),
		"pageup" => Some(ofx_key::PAGE_UP),
		"pagedown" => Some(ofx_key::PAGE_DOWN),
		_ => None,
	};
	if let Some(sym) = named {
		return (sym, String::new());
	}
	// Function keys: f1..f35 → KEY_F1 + (n-1).
	if let Some(rest) = key.strip_prefix('f') {
		if let Ok(n) = rest.parse::<u32>() {
			if (1..=35).contains(&n) {
				return (ofx_key::F1 + (n as i32 - 1), String::new());
			}
		}
	}
	// Printable single-character keys (letters/digits/punctuation): the
	// symbol is the ASCII code (ofxKeySyms maps printable ASCII 1:1;
	// KEY_A=0x61 … KEY_Z=0x7a, KEY_SPACE=0x20), the string is the char.
	if key.len() == 1 && key.is_ascii() {
		return (key.as_bytes()[0] as i32, key.to_string());
	}
	(ofx_key::UNKNOWN, String::new())
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
mod tests;

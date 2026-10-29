//! Overlay interacts (`oak_ofx_interact_*`): the plugin's custom UI
//! on the viewer. An interact is created on a live instance
//! ([`oak_ofx_interact_create`] runs the full `new_interact` →
//! describe → createInstance sequence), receives draw/pen/key/idle
//! actions, and is destroyed explicitly (or with its instance).
//!
//! Action entry points return the OFX status code
//! (`oak_ofx_plugin::suites::status`); an unknown interact handle
//! reports `kOfxStatErrBadHandle` (9).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use oak_ofx_plugin::node_factory;
use oak_ofx_plugin::suites::interact::Interact;
use oak_ofx_plugin::suites::status;

use crate::codec::str_arg;

static INTERACTS: OnceLock<Mutex<HashMap<u64, Arc<Interact>>>> = OnceLock::new();
static NEXT_INTERACT_ID: AtomicU64 = AtomicU64::new(1);

fn interacts() -> &'static Mutex<HashMap<u64, Arc<Interact>>> {
	INTERACTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn interact(id: u64) -> Option<Arc<Interact>> {
	interacts()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.get(&id)
		.cloned()
}

/// Destroys every live interact before the plugin host shuts down.
pub(crate) fn shutdown() {
	let live: Vec<Arc<Interact>> = {
		let mut registry = interacts().lock().unwrap_or_else(|e| e.into_inner());
		registry.drain().map(|(_, interact)| interact).collect()
	};
	for interact in live {
		interact.destroy();
	}
}

/// Creates the overlay interact of the instance `instance` (registry
/// id), running describe + createInstance. Returns the interact
/// handle, or 0 when the instance is unknown or the plugin declares
/// no interact (a normal no-op, not an error).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_create(instance: u64) -> u64 {
	let Some(inst) = node_factory::instance_from_id(instance) else {
		return 0;
	};
	let Some(interact) = inst.value.new_interact() else {
		return 0;
	};
	// Complete the instance sequence (ofxInteract.h: NewInteract →
	// Describe → CreateInstance before any draw/pen/key action).
	interact.describe();
	interact.create_instance();
	let id = NEXT_INTERACT_ID.fetch_add(1, Ordering::Relaxed);
	interacts()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.insert(id, interact);
	id
}

/// Destroys the interact (the kOfxActionDestroyInstanceInteract
/// notification; idempotent within oak-ofx-plugin) and removes it
/// from the registry. Unknown handles are a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_destroy(id: u64) {
	let Some(interact) = interacts()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.remove(&id)
	else {
		return;
	};
	interact.destroy();
}

/// The draw action: the plugin paints its overlay into the currently
/// bound framebuffer of the current GL context (acquire the GL bridge
/// first — see [`crate::ofx::gl`]). `viewport` is the viewport size in
/// pixels, `pixel_scale` the canonical→pixel ratio. No background
/// image is passed (the caller composites the overlay over the frame
/// itself).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_draw(
	id: u64,
	viewport_width: f64,
	viewport_height: f64,
	pixel_scale_x: f64,
	pixel_scale_y: f64,
	time: f64,
) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.draw(
		(viewport_width, viewport_height),
		(pixel_scale_x, pixel_scale_y),
		time,
		None,
	)
}

/// The pen motion action; `pen_down` maps to pressure 1.0/0.0
/// (two-state pens, ofxInteract.h:114).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_pen_motion(
	id: u64,
	x: f64,
	y: f64,
	pen_down: bool,
	time: f64,
) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.pen_motion((x, y), pen_down, time)
}

/// The pen down action.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_pen_down(id: u64, x: f64, y: f64, time: f64) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.pen_down((x, y), time)
}

/// The pen up action.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_pen_up(id: u64, x: f64, y: f64, time: f64) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.pen_up((x, y), time)
}

fn key_action(id: u64, key_sym: i32, key: *const u8, key_len: usize, time: f64, down: bool) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	let key = unsafe { str_arg(key, key_len) }.unwrap_or("");
	if down {
		interact.key_down(key_sym, key, time)
	} else {
		interact.key_up(key_sym, key, time)
	}
}

/// The key down action. `key_sym` is an ofxKeySyms value (the SDK's
/// `ofx::key` constants); `key_string` the character for printable
/// keys (empty otherwise).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_interact_key_down(
	id: u64,
	key_sym: i32,
	key: *const u8,
	key_len: usize,
	time: f64,
) -> i32 {
	key_action(id, key_sym, key, key_len, time, true)
}

/// The key up action (see [`oak_ofx_interact_key_down`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_interact_key_up(
	id: u64,
	key_sym: i32,
	key: *const u8,
	key_len: usize,
	time: f64,
) -> i32 {
	key_action(id, key_sym, key, key_len, time, false)
}

/// The idle action (throttle to ~10 Hz; idle is for lightweight UI
/// work when the app is otherwise quiet).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_idle(id: u64) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.idle()
}

/// The gain focus action (a prerequisite of pen/key actions,
/// ofxInteract.h).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_gain_focus(id: u64, time: f64) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.gain_focus(time)
}

/// The lose focus action.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_lose_focus(id: u64, time: f64) -> i32 {
	let Some(interact) = interact(id) else {
		return status::ERR_BAD_HANDLE;
	};
	interact.lose_focus(time)
}

/// Takes the interact's redraw-requested flag (set by the plugin via
/// interactRedraw; false on unknown handles).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_take_redraw(id: u64) -> bool {
	let Some(interact) = interact(id) else {
		return false;
	};
	interact.redraw_requested.swap(false, Ordering::Relaxed)
}

/// Takes the interact's swap-buffers-requested flag (set via
/// interactSwapBuffers; false on unknown handles).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_interact_take_swap(id: u64) -> bool {
	let Some(interact) = interact(id) else {
		return false;
	};
	interact.swap_requested.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unknown_handles_fail_closed() {
		assert_eq!(oak_ofx_interact_create(u64::MAX), 0);
		assert_eq!(oak_ofx_interact_idle(u64::MAX), status::ERR_BAD_HANDLE);
		assert_eq!(
			oak_ofx_interact_pen_down(u64::MAX, 0.0, 0.0, 0.0),
			status::ERR_BAD_HANDLE
		);
		assert_eq!(
			unsafe { oak_ofx_interact_key_down(u64::MAX, 0x61, "a".as_ptr(), 1, 0.0) },
			status::ERR_BAD_HANDLE
		);
		assert!(!oak_ofx_interact_take_redraw(u64::MAX));
		assert!(!oak_ofx_interact_take_swap(u64::MAX));
		oak_ofx_interact_destroy(u64::MAX);
	}
}

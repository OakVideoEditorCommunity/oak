//! Safe wrapper over the engine's OFX plugin host C ABI
//! (`oak_ofx_*`): plugin discovery/enumeration, instances with
//! parameter introspection/editing, headless frame rendering,
//! interacts, the GL bridge, progress/viewer-time plumbing, and
//! parametric curves.
//!
//! This mirrors the wiring the app does in `oakui/ofx.rs`, plus the
//! headless surface functional plugins (batch processing,
//! AI-assisted editing driving effects) need. Instance identity is a
//! registry id; [`Instance`] releases it on drop. [`Interact`] and
//! [`CurveSet`] manage their engine-side handles on drop too.

use std::ffi::c_void;
use std::sync::Mutex;

use crate::codec::string_out;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_ofx_scan() -> i32;
	fn oak_ofx_scan_path(path: *const u8, path_len: usize) -> bool;
	fn oak_ofx_plugin_count() -> i32;
	fn oak_ofx_plugin_identifier_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_plugin_label_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_plugin_path_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_plugin_version_at(index: i32, major: *mut u32, minor: *mut u32) -> bool;
	fn oak_ofx_plugin_context_count_at(index: i32) -> i32;
	fn oak_ofx_plugin_context_at(
		index: i32,
		context_index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_ofx_shutdown();
	fn oak_ofx_alive_count() -> i32;

	fn oak_ofx_instance_create(
		plugin_id: *const u8,
		plugin_id_len: usize,
		context: *const u8,
		context_len: usize,
	) -> u64;
	fn oak_ofx_instance_release(id: u64);
	fn oak_ofx_instance_count() -> i32;
	fn oak_ofx_instance_context(id: u64, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_instance_push_button(id: u64, param: *const u8, param_len: usize) -> bool;
	fn oak_ofx_instance_set_cancelled(id: u64, cancelled: bool);
	fn oak_ofx_instance_is_cancelled(id: u64) -> bool;

	fn oak_ofx_register_plugin_nodes() -> i32;
	fn oak_ofx_install_render_executor();
	fn oak_ofx_set_project_extent(width: f64, height: f64);
	fn oak_ofx_set_viewer_time(time: f64, range_min: f64, range_max: f64);

	fn oak_ofx_set_progress_handler(
		cb: Option<ProgressFn>,
		end_cb: Option<ProgressEndFn>,
		userdata: *mut c_void,
	);
	fn oak_ofx_has_progress_handler() -> bool;

	fn oak_ofx_instance_persistent_message_count(id: u64) -> i32;
	fn oak_ofx_instance_clear_persistent_messages(id: u64);

	fn oak_ofx_instance_param_count(id: u64) -> i32;
	fn oak_ofx_param_name_at(id: u64, index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_param_type_at(id: u64, index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_param_label_at(id: u64, index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_param_get_int(
		id: u64,
		name: *const u8,
		name_len: usize,
		out: *mut i32,
		cap: usize,
	) -> i32;
	fn oak_ofx_param_set_int(
		id: u64,
		name: *const u8,
		name_len: usize,
		values: *const i32,
		dim: usize,
	) -> bool;
	fn oak_ofx_param_get_double(
		id: u64,
		name: *const u8,
		name_len: usize,
		out: *mut f64,
		cap: usize,
	) -> i32;
	fn oak_ofx_param_set_double(
		id: u64,
		name: *const u8,
		name_len: usize,
		values: *const f64,
		dim: usize,
	) -> bool;
	fn oak_ofx_param_get_bool(id: u64, name: *const u8, name_len: usize) -> i32;
	fn oak_ofx_param_set_bool(id: u64, name: *const u8, name_len: usize, value: bool) -> bool;
	fn oak_ofx_param_get_choice(id: u64, name: *const u8, name_len: usize) -> i32;
	fn oak_ofx_param_set_choice(id: u64, name: *const u8, name_len: usize, index: i32) -> bool;
	fn oak_ofx_param_get_color(
		id: u64,
		name: *const u8,
		name_len: usize,
		out: *mut f64,
		cap: usize,
	) -> i32;
	fn oak_ofx_param_set_color(
		id: u64,
		name: *const u8,
		name_len: usize,
		values: *const f64,
		dim: usize,
	) -> bool;
	fn oak_ofx_param_get_string(
		id: u64,
		name: *const u8,
		name_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_ofx_param_set_string(
		id: u64,
		name: *const u8,
		name_len: usize,
		value: *const u8,
		value_len: usize,
	) -> bool;
	fn oak_ofx_param_get_curves_json(
		id: u64,
		name: *const u8,
		name_len: usize,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_ofx_param_set_curves_json(
		id: u64,
		name: *const u8,
		name_len: usize,
		json: *const u8,
		json_len: usize,
	) -> bool;

	fn oak_ofx_instance_render_frame(
		id: u64,
		time: f64,
		src: *const f32,
		src_w: i32,
		src_h: i32,
		dst: *mut f32,
		dst_w: i32,
		dst_h: i32,
	) -> bool;
	fn oak_ofx_instance_begin_sequence(id: u64, min: f64, max: f64) -> bool;
	fn oak_ofx_instance_end_sequence(id: u64, min: f64, max: f64) -> bool;
	fn oak_ofx_instance_get_rod(id: u64, time: f64, out: *mut OakOfxRectD) -> bool;

	fn oak_ofx_interact_create(instance: u64) -> u64;
	fn oak_ofx_interact_destroy(id: u64);
	fn oak_ofx_interact_draw(
		id: u64,
		viewport_width: f64,
		viewport_height: f64,
		pixel_scale_x: f64,
		pixel_scale_y: f64,
		time: f64,
	) -> i32;
	fn oak_ofx_interact_pen_motion(id: u64, x: f64, y: f64, pen_down: bool, time: f64) -> i32;
	fn oak_ofx_interact_pen_down(id: u64, x: f64, y: f64, time: f64) -> i32;
	fn oak_ofx_interact_pen_up(id: u64, x: f64, y: f64, time: f64) -> i32;
	fn oak_ofx_interact_key_down(
		id: u64,
		key_sym: i32,
		key: *const u8,
		key_len: usize,
		time: f64,
	) -> i32;
	fn oak_ofx_interact_key_up(
		id: u64,
		key_sym: i32,
		key: *const u8,
		key_len: usize,
		time: f64,
	) -> i32;
	fn oak_ofx_interact_idle(id: u64) -> i32;
	fn oak_ofx_interact_gain_focus(id: u64, time: f64) -> i32;
	fn oak_ofx_interact_lose_focus(id: u64, time: f64) -> i32;
	fn oak_ofx_interact_take_redraw(id: u64) -> bool;
	fn oak_ofx_interact_take_swap(id: u64) -> bool;

	fn oak_ofx_curveset_new() -> *mut c_void;
	fn oak_ofx_curveset_add_ref(set: *const c_void);
	fn oak_ofx_curveset_release(set: *const c_void);
	fn oak_ofx_curveset_from_json(json: *const u8, json_len: usize) -> *mut c_void;
	fn oak_ofx_curveset_to_json(set: *const c_void, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_ofx_curveset_curve_count(set: *const c_void) -> i32;
	fn oak_ofx_curveset_add_curve(set: *const c_void) -> i32;
	fn oak_ofx_curveset_add_identity_curve(set: *const c_void, lo: f64, hi: f64) -> i32;
	fn oak_ofx_curveset_point_count(set: *const c_void, curve: i32) -> i32;
	fn oak_ofx_curveset_point_at(
		set: *const c_void,
		curve: i32,
		index: i32,
		out: *mut OakOfxCurvePoint,
	) -> bool;
	fn oak_ofx_curveset_upsert_point(set: *const c_void, curve: i32, key: f64, value: f64) -> bool;
	fn oak_ofx_curveset_delete_point(set: *const c_void, curve: i32, index: i32) -> bool;
	fn oak_ofx_curveset_clear_curve(set: *const c_void, curve: i32) -> bool;
	fn oak_ofx_curveset_evaluate(set: *const c_void, curve: i32, x: f64) -> f64;

	fn oak_ofx_gl_available() -> bool;
	fn oak_ofx_gl_acquire() -> bool;
	fn oak_ofx_gl_release();
	fn oak_ofx_gl_create_output_texture(width: i32, height: i32) -> i32;
	fn oak_ofx_gl_create_input_texture(width: i32, height: i32, pixels: *const f32) -> i32;
	fn oak_ofx_gl_delete_texture(texture: i32);
	fn oak_ofx_gl_create_fbo(texture: i32, width: i32, height: i32) -> i32;
	fn oak_ofx_gl_bind_fbo(fbo: i32);
	fn oak_ofx_gl_delete_fbo(fbo: i32);
	fn oak_ofx_gl_set_viewport(width: i32, height: i32);
	fn oak_ofx_gl_clear_color(r: f32, g: f32, b: f32, a: f32);
	fn oak_ofx_gl_clear();
	fn oak_ofx_gl_read_pixels_f32(
		width: i32,
		height: i32,
		out: *mut f32,
		out_capacity: usize,
	) -> i32;
	fn oak_ofx_gl_version(buf: *mut u8, buf_len: usize) -> i32;
}

/// OFX action status codes (`kOfxStat*`); interact action entry
/// points return these.
pub mod status {
	pub const OK: i32 = 0;
	pub const FAILED: i32 = 1;
	pub const ERR_FATAL: i32 = 2;
	pub const ERR_UNKNOWN: i32 = 3;
	pub const ERR_MISSING_HOST_FEATURE: i32 = 4;
	pub const ERR_UNSUPPORTED: i32 = 5;
	pub const ERR_EXISTS: i32 = 6;
	pub const ERR_FORMAT: i32 = 7;
	pub const ERR_MEMORY: i32 = 8;
	pub const ERR_BAD_HANDLE: i32 = 9;
	pub const ERR_BAD_INDEX: i32 = 10;
	pub const ERR_VALUE: i32 = 11;
	pub const REPLY_YES: i32 = 12;
	pub const REPLY_NO: i32 = 13;
	pub const REPLY_DEFAULT: i32 = 14;
}

/// ofxKeySyms values for [`Interact::key_down`]/[`Interact::key_up`]
/// (printable ASCII keys carry their ASCII code as the symbol).
pub mod key {
	pub const UNKNOWN: i32 = 0x0;
	pub const BACKSPACE: i32 = 0xFF08;
	pub const TAB: i32 = 0xFF09;
	pub const RETURN: i32 = 0xFF0D;
	pub const ESCAPE: i32 = 0xFF1B;
	pub const DELETE: i32 = 0xFFFF;
	pub const HOME: i32 = 0xFF50;
	pub const LEFT: i32 = 0xFF51;
	pub const UP: i32 = 0xFF52;
	pub const RIGHT: i32 = 0xFF53;
	pub const DOWN: i32 = 0xFF54;
	pub const PAGE_UP: i32 = 0xFF55;
	pub const PAGE_DOWN: i32 = 0xFF56;
	pub const END: i32 = 0xFF57;
	pub const F1: i32 = 0xFFBE;
	pub const SHIFT_L: i32 = 0xFFE1;
	pub const CONTROL_L: i32 = 0xFFE3;
	pub const ALT_L: i32 = 0xFFE9;
	pub const SPACE: i32 = 0x020;
	pub const A: i32 = 0x061;
	pub const Z: i32 = 0x07a;
}

// ---------------------------------------------------------------------------
// Discovery / enumeration
// ---------------------------------------------------------------------------

/// Scans the standard OFX plugin paths. None when the scan failed
/// (plugins are optional; a failure never aborts the caller).
pub fn scan() -> Option<usize> {
	let n = unsafe { oak_ofx_scan() };
	(n >= 0).then_some(n as usize)
}

/// Scans one extra plugin directory. False on failure.
pub fn scan_path(path: &str) -> bool {
	unsafe { oak_ofx_scan_path(path.as_ptr(), path.len()) }
}

/// A discovered plugin's metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginInfo {
	/// Unique reverse-DNS identifier.
	pub identifier: String,
	/// Display label.
	pub label: String,
	/// (major, minor).
	pub version: (u32, u32),
	/// Bundle path.
	pub path: String,
	/// Supported contexts (`OfxImageEffectContext*`).
	pub contexts: Vec<String>,
}

/// Number of discovered plugins.
pub fn plugin_count() -> usize {
	unsafe { oak_ofx_plugin_count() }.max(0) as usize
}

/// The metadata of the plugin at `index`; None when out of range.
pub fn plugin_at(index: usize) -> Option<PluginInfo> {
	if index >= plugin_count() {
		return None;
	}
	let i = index as i32;
	let identifier = string_out(|buf, len| unsafe { oak_ofx_plugin_identifier_at(i, buf, len) })?;
	let label = string_out(|buf, len| unsafe { oak_ofx_plugin_label_at(i, buf, len) })?;
	let path = string_out(|buf, len| unsafe { oak_ofx_plugin_path_at(i, buf, len) })?;
	let (mut major, mut minor) = (0u32, 0u32);
	if !unsafe { oak_ofx_plugin_version_at(i, &mut major, &mut minor) } {
		return None;
	}
	let context_count = unsafe { oak_ofx_plugin_context_count_at(i) }.max(0);
	let contexts = (0..context_count)
		.filter_map(|c| string_out(|buf, len| unsafe { oak_ofx_plugin_context_at(i, c, buf, len) }))
		.collect();
	Some(PluginInfo {
		identifier,
		label,
		version: (major, minor),
		path,
		contexts,
	})
}

/// All discovered plugins.
pub fn plugins() -> Vec<PluginInfo> {
	(0..plugin_count()).filter_map(plugin_at).collect()
}

/// Shuts the host down (destroys live instances, unloads plugins).
pub fn shutdown() {
	unsafe { oak_ofx_shutdown() }
}

/// Live instance count (leak diagnostic).
pub fn alive_count() -> usize {
	unsafe { oak_ofx_alive_count() }.max(0) as usize
}

/// Registers every discovered plugin into the node factory as a
/// dynamic node type (idempotent; also installs the render executor
/// and the plugin-node duplicator). Returns the number of newly
/// registered node types.
pub fn register_plugin_nodes() -> usize {
	unsafe { oak_ofx_register_plugin_nodes() }.max(0) as usize
}

/// Installs the render executor / plugin-node duplicator (idempotent).
pub fn install_render_executor() {
	unsafe { oak_ofx_install_render_executor() }
}

/// Pushes the project extent the OFX normalised-coordinate default
/// conversion uses.
pub fn set_project_extent(width: f64, height: f64) {
	unsafe { oak_ofx_set_project_extent(width, height) }
}

/// Updates the active-viewer time snapshot the timeline suite falls
/// back to when no render context is live.
pub fn set_viewer_time(time: f64, range_min: f64, range_max: f64) {
	unsafe { oak_ofx_set_viewer_time(time, range_min, range_max) }
}

// ---------------------------------------------------------------------------
// Progress handler
// ---------------------------------------------------------------------------

type ProgressFn = unsafe extern "C" fn(
	label: *const std::ffi::c_char,
	message: *const std::ffi::c_char,
	fraction: f64,
	userdata: *mut c_void,
) -> bool;
type ProgressEndFn = unsafe extern "C" fn(userdata: *mut c_void);

struct ProgressCallbacks {
	on_progress: Box<dyn Fn(&str, &str, f64) -> bool + Send>,
	on_end: Box<dyn Fn() + Send>,
}

// The boxed callbacks live in this slot for the whole registration;
// the engine hands the pointer back as userdata on every event.
// (Stored as usize: raw pointers aren't Send; the pointer targets a
// Send closure pair and the slot is mutex-guarded.)
static PROGRESS_CALLBACKS: Mutex<Option<usize>> = Mutex::new(None);

unsafe extern "C" fn progress_trampoline(
	label: *const std::ffi::c_char,
	message: *const std::ffi::c_char,
	fraction: f64,
	userdata: *mut c_void,
) -> bool {
	if userdata.is_null() {
		return true;
	}
	let cb = unsafe { &*userdata.cast::<ProgressCallbacks>() };
	let label = if label.is_null() {
		""
	} else {
		unsafe { std::ffi::CStr::from_ptr(label) }
			.to_str()
			.unwrap_or("")
	};
	let message = if message.is_null() {
		""
	} else {
		unsafe { std::ffi::CStr::from_ptr(message) }
			.to_str()
			.unwrap_or("")
	};
	(cb.on_progress)(label, message, fraction)
}

unsafe extern "C" fn end_trampoline(userdata: *mut c_void) {
	if userdata.is_null() {
		return;
	}
	let cb = unsafe { &*userdata.cast::<ProgressCallbacks>() };
	(cb.on_end)();
}

/// Registers the progress handler: `on_progress(label, message,
/// fraction)` fires on every plugin progressUpdate (return false to
/// request cancellation), `on_end()` on progressEnd. Re-registering
/// replaces the previous handler.
pub fn set_progress_handler(
	on_progress: impl Fn(&str, &str, f64) -> bool + Send + 'static,
	on_end: impl Fn() + Send + 'static,
) {
	let callbacks = Box::into_raw(Box::new(ProgressCallbacks {
		on_progress: Box::new(on_progress),
		on_end: Box::new(on_end),
	}));
	let mut slot = PROGRESS_CALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
	if let Some(old) = slot.take() {
		unsafe { drop(Box::from_raw(old as *mut ProgressCallbacks)) };
	}
	*slot = Some(callbacks as usize);
	unsafe {
		oak_ofx_set_progress_handler(
			Some(progress_trampoline),
			Some(end_trampoline),
			callbacks as *mut c_void,
		)
	};
}

/// Clears the progress handler.
pub fn clear_progress_handler() {
	unsafe { oak_ofx_set_progress_handler(None, None, std::ptr::null_mut()) };
	if let Some(old) = PROGRESS_CALLBACKS
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.take()
	{
		unsafe { drop(Box::from_raw(old as *mut ProgressCallbacks)) };
	}
}

/// Whether a progress handler is registered.
pub fn has_progress_handler() -> bool {
	unsafe { oak_ofx_has_progress_handler() }
}

// ---------------------------------------------------------------------------
// Instances
// ---------------------------------------------------------------------------

/// A live OFX effect instance (registry id). Drop unregisters it; the
/// instance is destroyed once no references remain.
#[derive(Debug)]
pub struct Instance {
	id: u64,
}

/// Number of registered instances (diagnostic).
pub fn instance_count() -> usize {
	unsafe { oak_ofx_instance_count() }.max(0) as usize
}

impl Instance {
	/// Creates an instance of `plugin_id`. `None` context uses the
	/// host's preferred-context fallback chain (filter → general →
	/// tracker → paint → rest). None on failure (unknown plugin, no
	/// supported context, plugin error).
	pub fn create(plugin_id: &str, context: Option<&str>) -> Option<Instance> {
		let (ctx, ctx_len) = match context {
			Some(c) => (c.as_ptr(), c.len()),
			None => (std::ptr::null(), 0),
		};
		let id =
			unsafe { oak_ofx_instance_create(plugin_id.as_ptr(), plugin_id.len(), ctx, ctx_len) };
		(id != 0).then_some(Instance { id })
	}

	/// The registry id.
	pub fn id(&self) -> u64 {
		self.id
	}

	/// The context the instance was created in.
	pub fn context(&self) -> Option<String> {
		string_out(|buf, len| unsafe { oak_ofx_instance_context(self.id, buf, len) })
	}

	/// Triggers a push-button parameter (the inspector's button path).
	pub fn push_button(&self, param: &str) -> bool {
		unsafe { oak_ofx_instance_push_button(self.id, param.as_ptr(), param.len()) }
	}

	/// Sets/clears the cancel flag: a running render aborts at the
	/// next progress batch boundary.
	pub fn set_cancelled(&self, cancelled: bool) {
		unsafe { oak_ofx_instance_set_cancelled(self.id, cancelled) }
	}

	/// Whether the cancel flag is set.
	pub fn is_cancelled(&self) -> bool {
		unsafe { oak_ofx_instance_is_cancelled(self.id) }
	}

	/// Number of parameters.
	pub fn param_count(&self) -> Option<usize> {
		let n = unsafe { oak_ofx_instance_param_count(self.id) };
		(n >= 0).then_some(n as usize)
	}

	/// The parameter name at `index` (definition order).
	pub fn param_name_at(&self, index: usize) -> Option<String> {
		string_out(|buf, len| unsafe { oak_ofx_param_name_at(self.id, index as i32, buf, len) })
	}

	/// The parameter's OFX type string (`kOfxParamType*`) at `index`.
	pub fn param_type_at(&self, index: usize) -> Option<String> {
		string_out(|buf, len| unsafe { oak_ofx_param_type_at(self.id, index as i32, buf, len) })
	}

	/// The parameter's display label at `index`.
	pub fn param_label_at(&self, index: usize) -> Option<String> {
		string_out(|buf, len| unsafe { oak_ofx_param_label_at(self.id, index as i32, buf, len) })
	}

	/// Reads an integer-family parameter (Integer/Integer2D/Integer3D).
	/// None on unknown name or a type mismatch.
	pub fn param_int(&self, name: &str) -> Option<Vec<i32>> {
		let dim = unsafe {
			oak_ofx_param_get_int(self.id, name.as_ptr(), name.len(), std::ptr::null_mut(), 0)
		};
		if dim < 0 {
			return None;
		}
		let mut out = vec![0i32; dim as usize];
		unsafe {
			oak_ofx_param_get_int(
				self.id,
				name.as_ptr(),
				name.len(),
				out.as_mut_ptr(),
				out.len(),
			)
		};
		Some(out)
	}

	/// Writes an integer-family parameter. False on unknown name or a
	/// type/arity mismatch.
	pub fn set_param_int(&self, name: &str, values: &[i32]) -> bool {
		unsafe {
			oak_ofx_param_set_int(
				self.id,
				name.as_ptr(),
				name.len(),
				values.as_ptr(),
				values.len(),
			)
		}
	}

	/// Reads a double-family parameter (Double/Double2D/Double3D).
	pub fn param_double(&self, name: &str) -> Option<Vec<f64>> {
		let dim = unsafe {
			oak_ofx_param_get_double(self.id, name.as_ptr(), name.len(), std::ptr::null_mut(), 0)
		};
		if dim < 0 {
			return None;
		}
		let mut out = vec![0.0f64; dim as usize];
		unsafe {
			oak_ofx_param_get_double(
				self.id,
				name.as_ptr(),
				name.len(),
				out.as_mut_ptr(),
				out.len(),
			)
		};
		Some(out)
	}

	/// Writes a double-family parameter.
	pub fn set_param_double(&self, name: &str, values: &[f64]) -> bool {
		unsafe {
			oak_ofx_param_set_double(
				self.id,
				name.as_ptr(),
				name.len(),
				values.as_ptr(),
				values.len(),
			)
		}
	}

	/// Reads a boolean parameter.
	pub fn param_bool(&self, name: &str) -> Option<bool> {
		match unsafe { oak_ofx_param_get_bool(self.id, name.as_ptr(), name.len()) } {
			0 => Some(false),
			1 => Some(true),
			_ => None,
		}
	}

	/// Writes a boolean parameter.
	pub fn set_param_bool(&self, name: &str, value: bool) -> bool {
		unsafe { oak_ofx_param_set_bool(self.id, name.as_ptr(), name.len(), value) }
	}

	/// Reads a choice parameter (the option index).
	pub fn param_choice(&self, name: &str) -> Option<i32> {
		let v = unsafe { oak_ofx_param_get_choice(self.id, name.as_ptr(), name.len()) };
		(v >= 0).then_some(v)
	}

	/// Writes a choice parameter (the option index).
	pub fn set_param_choice(&self, name: &str, index: i32) -> bool {
		unsafe { oak_ofx_param_set_choice(self.id, name.as_ptr(), name.len(), index) }
	}

	/// Reads a color parameter (RGB/RGBA → 3/4 components).
	pub fn param_color(&self, name: &str) -> Option<Vec<f64>> {
		let dim = unsafe {
			oak_ofx_param_get_color(self.id, name.as_ptr(), name.len(), std::ptr::null_mut(), 0)
		};
		if dim < 0 {
			return None;
		}
		let mut out = vec![0.0f64; dim as usize];
		unsafe {
			oak_ofx_param_get_color(
				self.id,
				name.as_ptr(),
				name.len(),
				out.as_mut_ptr(),
				out.len(),
			)
		};
		Some(out)
	}

	/// Writes a color parameter (3 or 4 components, matching the
	/// parameter's arity).
	pub fn set_param_color(&self, name: &str, values: &[f64]) -> bool {
		unsafe {
			oak_ofx_param_set_color(
				self.id,
				name.as_ptr(),
				name.len(),
				values.as_ptr(),
				values.len(),
			)
		}
	}

	/// Reads a string-family parameter (String/StrChoice).
	pub fn param_string(&self, name: &str) -> Option<String> {
		string_out(|buf, len| unsafe {
			oak_ofx_param_get_string(self.id, name.as_ptr(), name.len(), buf, len)
		})
	}

	/// Writes a string-family parameter.
	pub fn set_param_string(&self, name: &str, value: &str) -> bool {
		unsafe {
			oak_ofx_param_set_string(
				self.id,
				name.as_ptr(),
				name.len(),
				value.as_ptr(),
				value.len(),
			)
		}
	}

	/// Reads a parametric parameter as a curve set.
	pub fn param_curves(&self, name: &str) -> Option<CurveSet> {
		let json = string_out(|buf, len| unsafe {
			oak_ofx_param_get_curves_json(self.id, name.as_ptr(), name.len(), buf, len)
		})?;
		CurveSet::from_json(&json)
	}

	/// Writes a parametric parameter from a curve set.
	pub fn set_param_curves(&self, name: &str, curves: &CurveSet) -> bool {
		let json = curves.to_json();
		unsafe {
			oak_ofx_param_set_curves_json(
				self.id,
				name.as_ptr(),
				name.len(),
				json.as_ptr(),
				json.len(),
			)
		}
	}

	/// The persistent message count (the inspector badge data).
	pub fn persistent_message_count(&self) -> Option<usize> {
		let n = unsafe { oak_ofx_instance_persistent_message_count(self.id) };
		(n >= 0).then_some(n as usize)
	}

	/// Clears the persistent messages.
	pub fn clear_persistent_messages(&self) {
		unsafe { oak_ofx_instance_clear_persistent_messages(self.id) }
	}

	/// Renders one frame through the effect (CPU path): `src` is the
	/// input frame (tight RGBA F32; `None` for sources/generators),
	/// the output is written to `dst` (tight RGBA F32, must hold
	/// `dst_w*dst_h*4` floats). False on failure (invalid buffers,
	/// cancelled, plugin error). Set parameters first; long renders
	/// report progress through the registered handler.
	pub fn render_frame(
		&self,
		time: f64,
		src: Option<(&[f32], u32, u32)>,
		dst: &mut [f32],
		dst_size: (u32, u32),
	) -> bool {
		let (dst_w, dst_h) = dst_size;
		if dst_w == 0 || dst_h == 0 || dst.len() < (dst_w * dst_h * 4) as usize {
			return false;
		}
		let (src_ptr, src_w, src_h) = match src {
			Some((pixels, w, h)) => {
				if w == 0 || h == 0 || pixels.len() < (w * h * 4) as usize {
					return false;
				}
				(pixels.as_ptr(), w as i32, h as i32)
			}
			None => (std::ptr::null(), 0, 0),
		};
		unsafe {
			oak_ofx_instance_render_frame(
				self.id,
				time,
				src_ptr,
				src_w,
				src_h,
				dst.as_mut_ptr(),
				dst_w as i32,
				dst_h as i32,
			)
		}
	}

	/// The beginSequenceRender bracket (CPU mode): opens a sequence
	/// render over `[min, max]` seconds. Pair with
	/// [`Instance::end_sequence`].
	pub fn begin_sequence(&self, min: f64, max: f64) -> bool {
		unsafe { oak_ofx_instance_begin_sequence(self.id, min, max) }
	}

	/// The endSequenceRender bracket, pairing
	/// [`Instance::begin_sequence`].
	pub fn end_sequence(&self, min: f64, max: f64) -> bool {
		unsafe { oak_ofx_instance_end_sequence(self.id, min, max) }
	}

	/// The region of definition at `time` (canonical coordinates).
	pub fn region_of_definition(&self, time: f64) -> Option<RectD> {
		let mut out = OakOfxRectD::default();
		unsafe { oak_ofx_instance_get_rod(self.id, time, &mut out) }.then(|| out.into())
	}

	/// Creates the overlay interact (None when the plugin declares
	/// none). The interact is destroyed on drop.
	pub fn create_interact(&self) -> Option<Interact> {
		let id = unsafe { oak_ofx_interact_create(self.id) };
		(id != 0).then_some(Interact { id })
	}
}

impl Drop for Instance {
	fn drop(&mut self) {
		unsafe { oak_ofx_instance_release(self.id) };
	}
}

/// A rectangle in canonical coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectD {
	pub x1: f64,
	pub y1: f64,
	pub x2: f64,
	pub y2: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct OakOfxRectD {
	x1: f64,
	y1: f64,
	x2: f64,
	y2: f64,
}

impl From<OakOfxRectD> for RectD {
	fn from(r: OakOfxRectD) -> Self {
		Self {
			x1: r.x1,
			y1: r.y1,
			x2: r.x2,
			y2: r.y2,
		}
	}
}

// ---------------------------------------------------------------------------
// Interacts
// ---------------------------------------------------------------------------

/// A plugin's overlay interact (custom UI on the viewer). Drop
/// destroys it. Action methods return OFX status codes ([`status`]).
#[derive(Debug)]
pub struct Interact {
	id: u64,
}

impl Interact {
	/// The draw action: the plugin paints its overlay into the
	/// currently bound framebuffer (acquire the GL bridge first — see
	/// [`gl`]). `viewport` is the viewport size in pixels,
	/// `pixel_scale` the canonical→pixel ratio. No background image is
	/// passed; composite the overlay over the frame yourself.
	pub fn draw(&self, viewport: (f64, f64), pixel_scale: (f64, f64), time: f64) -> i32 {
		unsafe {
			oak_ofx_interact_draw(
				self.id,
				viewport.0,
				viewport.1,
				pixel_scale.0,
				pixel_scale.1,
				time,
			)
		}
	}

	/// The pen motion action (`pen_down` maps to pressure 1.0/0.0).
	pub fn pen_motion(&self, position: (f64, f64), pen_down: bool, time: f64) -> i32 {
		unsafe { oak_ofx_interact_pen_motion(self.id, position.0, position.1, pen_down, time) }
	}

	/// The pen down action.
	pub fn pen_down(&self, position: (f64, f64), time: f64) -> i32 {
		unsafe { oak_ofx_interact_pen_down(self.id, position.0, position.1, time) }
	}

	/// The pen up action.
	pub fn pen_up(&self, position: (f64, f64), time: f64) -> i32 {
		unsafe { oak_ofx_interact_pen_up(self.id, position.0, position.1, time) }
	}

	/// The key down action (`key_sym` is an ofxKeySyms value — see
	/// [`key`]; `key_string` the character for printable keys).
	pub fn key_down(&self, key_sym: i32, key_string: &str, time: f64) -> i32 {
		unsafe {
			oak_ofx_interact_key_down(
				self.id,
				key_sym,
				key_string.as_ptr(),
				key_string.len(),
				time,
			)
		}
	}

	/// The key up action.
	pub fn key_up(&self, key_sym: i32, key_string: &str, time: f64) -> i32 {
		unsafe {
			oak_ofx_interact_key_up(
				self.id,
				key_sym,
				key_string.as_ptr(),
				key_string.len(),
				time,
			)
		}
	}

	/// The idle action (throttle to ~10 Hz).
	pub fn idle(&self) -> i32 {
		unsafe { oak_ofx_interact_idle(self.id) }
	}

	/// The gain focus action (a prerequisite of pen/key actions).
	pub fn gain_focus(&self, time: f64) -> i32 {
		unsafe { oak_ofx_interact_gain_focus(self.id, time) }
	}

	/// The lose focus action.
	pub fn lose_focus(&self, time: f64) -> i32 {
		unsafe { oak_ofx_interact_lose_focus(self.id, time) }
	}

	/// Takes the redraw-requested flag (set by the plugin via
	/// interactRedraw).
	pub fn take_redraw(&self) -> bool {
		unsafe { oak_ofx_interact_take_redraw(self.id) }
	}

	/// Takes the swap-buffers-requested flag.
	pub fn take_swap(&self) -> bool {
		unsafe { oak_ofx_interact_take_swap(self.id) }
	}
}

impl Drop for Interact {
	fn drop(&mut self) {
		unsafe { oak_ofx_interact_destroy(self.id) };
	}
}

// ---------------------------------------------------------------------------
// Parametric curves
// ---------------------------------------------------------------------------

/// A curve control point: `key` is the parametric position, `value`
/// the result, `slope` the first derivative (Hermite tangent;
/// recomputed automatically by the mutating calls).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlPoint {
	pub key: f64,
	pub value: f64,
	pub slope: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct OakOfxCurvePoint {
	key: f64,
	value: f64,
	slope: f64,
}

/// A set of parametric curves (one per dimension of a parametric
/// parameter) — a refcounted engine handle; cloning shares the set.
/// JSON is the interchange format with node values and project files.
pub struct CurveSet {
	ptr: *mut c_void,
}

// The engine side is mutex-guarded; the refcount is atomic.
unsafe impl Send for CurveSet {}
unsafe impl Sync for CurveSet {}

impl CurveSet {
	/// A new, empty curve set.
	pub fn new() -> Self {
		Self {
			ptr: unsafe { oak_ofx_curveset_new() },
		}
	}

	/// Parses curve-set JSON. None on malformed JSON.
	pub fn from_json(json: &str) -> Option<Self> {
		let ptr = unsafe { oak_ofx_curveset_from_json(json.as_ptr(), json.len()) };
		(!ptr.is_null()).then_some(Self { ptr })
	}

	/// Serializes to JSON.
	pub fn to_json(&self) -> String {
		string_out(|buf, len| unsafe { oak_ofx_curveset_to_json(self.ptr, buf, len) })
			.unwrap_or_default()
	}

	/// Number of curves.
	pub fn curve_count(&self) -> usize {
		unsafe { oak_ofx_curveset_curve_count(self.ptr) }.max(0) as usize
	}

	/// Appends an empty curve, returning its index.
	pub fn add_curve(&self) -> Option<usize> {
		let i = unsafe { oak_ofx_curveset_add_curve(self.ptr) };
		(i >= 0).then_some(i as usize)
	}

	/// Appends an identity curve over `[lo, hi]` (the parametric
	/// default), returning its index.
	pub fn add_identity_curve(&self, lo: f64, hi: f64) -> Option<usize> {
		let i = unsafe { oak_ofx_curveset_add_identity_curve(self.ptr, lo, hi) };
		(i >= 0).then_some(i as usize)
	}

	/// Number of control points of a curve.
	pub fn point_count(&self, curve: usize) -> Option<usize> {
		let n = unsafe { oak_ofx_curveset_point_count(self.ptr, curve as i32) };
		(n >= 0).then_some(n as usize)
	}

	/// The control point at `index` of a curve.
	pub fn point_at(&self, curve: usize, index: usize) -> Option<ControlPoint> {
		let mut out = OakOfxCurvePoint::default();
		unsafe { oak_ofx_curveset_point_at(self.ptr, curve as i32, index as i32, &mut out) }.then(
			|| ControlPoint {
				key: out.key,
				value: out.value,
				slope: out.slope,
			},
		)
	}

	/// Inserts or overwrites the control point at `key` (key order is
	/// maintained; slopes are recomputed).
	pub fn upsert_point(&self, curve: usize, key: f64, value: f64) -> bool {
		unsafe { oak_ofx_curveset_upsert_point(self.ptr, curve as i32, key, value) }
	}

	/// Deletes the control point at `index`.
	pub fn delete_point(&self, curve: usize, index: usize) -> bool {
		unsafe { oak_ofx_curveset_delete_point(self.ptr, curve as i32, index as i32) }
	}

	/// Removes all control points of a curve (it then evaluates as
	/// identity).
	pub fn clear_curve(&self, curve: usize) -> bool {
		unsafe { oak_ofx_curveset_clear_curve(self.ptr, curve as i32) }
	}

	/// Evaluates a curve at `x` (cubic Hermite; out-of-range keys
	/// clamp to the endpoint values, an empty curve is identity).
	/// None on an out-of-range curve.
	pub fn evaluate(&self, curve: usize, x: f64) -> Option<f64> {
		let v = unsafe { oak_ofx_curveset_evaluate(self.ptr, curve as i32, x) };
		(!v.is_nan()).then_some(v)
	}
}

impl Clone for CurveSet {
	fn clone(&self) -> Self {
		unsafe { oak_ofx_curveset_add_ref(self.ptr) };
		Self { ptr: self.ptr }
	}
}

impl Drop for CurveSet {
	fn drop(&mut self) {
		unsafe { oak_ofx_curveset_release(self.ptr) };
	}
}

// ---------------------------------------------------------------------------
// GL bridge
// ---------------------------------------------------------------------------

/// The offscreen GL bridge behind interact drawing and GL-mode
/// renders. Texture/FBO creation and deletion require an acquired
/// context ([`acquire`]); everything fails closed when GL is
/// unavailable.
pub mod gl {
	use super::*;

	/// Whether the offscreen GL context can be created (false → fall
	/// back to CPU rendering).
	pub fn available() -> bool {
		unsafe { oak_ofx_gl_available() }
	}

	/// An acquired GL context (current on the acquiring thread).
	/// Releases on drop. Acquire is re-entrant per thread; guards must
	/// drop in LIFO order on the same thread.
	pub struct GlGuard {
		_private: (),
	}

	impl Drop for GlGuard {
		fn drop(&mut self) {
			unsafe { oak_ofx_gl_release() };
		}
	}

	/// Makes the GL context current on the calling thread (globally
	/// serialized). None when GL is unavailable.
	pub fn acquire() -> Option<GlGuard> {
		unsafe { oak_ofx_gl_acquire() }.then_some(GlGuard { _private: () })
	}

	/// Creates an RGBA32F output texture, returning its GL name.
	pub fn create_output_texture(width: i32, height: i32) -> Option<i32> {
		let t = unsafe { oak_ofx_gl_create_output_texture(width, height) };
		(t >= 0).then_some(t)
	}

	/// Creates an RGBA32F texture and uploads `pixels` (tight RGBA
	/// F32, `width*height*4` floats), returning its GL name.
	pub fn create_input_texture(width: i32, height: i32, pixels: &[f32]) -> Option<i32> {
		if width <= 0 || height <= 0 || pixels.len() < (width * height * 4) as usize {
			return None;
		}
		let t = unsafe { oak_ofx_gl_create_input_texture(width, height, pixels.as_ptr()) };
		(t >= 0).then_some(t)
	}

	/// Deletes a GL texture.
	pub fn delete_texture(texture: i32) {
		unsafe { oak_ofx_gl_delete_texture(texture) }
	}

	/// Creates an FBO with `texture` attached, returning its GL name.
	pub fn create_fbo(texture: i32, width: i32, height: i32) -> Option<i32> {
		let f = unsafe { oak_ofx_gl_create_fbo(texture, width, height) };
		(f >= 0).then_some(f)
	}

	/// Binds an FBO (0 = the default framebuffer).
	pub fn bind_fbo(fbo: i32) {
		unsafe { oak_ofx_gl_bind_fbo(fbo) }
	}

	/// Deletes an FBO.
	pub fn delete_fbo(fbo: i32) {
		unsafe { oak_ofx_gl_delete_fbo(fbo) }
	}

	/// Sets the GL viewport.
	pub fn set_viewport(width: i32, height: i32) {
		unsafe { oak_ofx_gl_set_viewport(width, height) }
	}

	/// Sets the clear color.
	pub fn clear_color(r: f32, g: f32, b: f32, a: f32) {
		unsafe { oak_ofx_gl_clear_color(r, g, b, a) }
	}

	/// Clears the color buffer.
	pub fn clear() {
		unsafe { oak_ofx_gl_clear() }
	}

	/// Reads the current framebuffer back as tight RGBA F32 into
	/// `out` (`width*height*4` floats). None on failure.
	pub fn read_pixels_f32(width: i32, height: i32, out: &mut [f32]) -> Option<usize> {
		let needed =
			unsafe { oak_ofx_gl_read_pixels_f32(width, height, out.as_mut_ptr(), out.len()) };
		if needed < 0 || out.len() < needed as usize {
			return None;
		}
		Some(needed as usize)
	}

	/// The GL version string.
	pub fn version() -> Option<String> {
		string_out(|buf, len| unsafe { oak_ofx_gl_version(buf, len) })
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The bundled minimal test plugin (`org.oak.test-plugin` & the
	/// interact variant), located under the workspace target dir and
	/// wrapped into the OFX bundle layout for scanning.
	fn test_plugin_scan_dir() -> Option<std::path::PathBuf> {
		crate::test_support::ofx_plugin_scan_dir()
	}

	// The host is process-global; serialize the host-touching tests.
	static HOST_SERIAL: Mutex<()> = Mutex::new(());

	#[test]
	fn curveset_build_edit_json_roundtrip() {
		let set = CurveSet::new();
		assert_eq!(set.curve_count(), 0);
		assert_eq!(set.add_identity_curve(0.0, 1.0), Some(0));
		assert_eq!(set.point_count(0), Some(2));
		assert_eq!(set.evaluate(0, 0.5), Some(0.5));

		assert!(set.upsert_point(0, 0.5, 0.25));
		assert_eq!(set.point_count(0), Some(3));
		let p = set.point_at(0, 1).expect("midpoint");
		assert_eq!((p.key, p.value), (0.5, 0.25));

		let json = set.to_json();
		let copy = CurveSet::from_json(&json).expect("parse");
		assert_eq!(copy.curve_count(), 1);
		assert_eq!(copy.point_count(0), Some(3));
		assert_eq!(copy.evaluate(0, 0.5), set.evaluate(0, 0.5));

		assert!(copy.delete_point(0, 1));
		assert!(copy.clear_curve(0));
		assert_eq!(copy.point_count(0), Some(0));
		assert_eq!(
			copy.evaluate(0, 0.7),
			Some(0.7),
			"an empty curve is identity"
		);
		assert_eq!(copy.evaluate(9, 0.0), None);

		assert!(CurveSet::from_json("{").is_none());
	}

	#[test]
	fn host_scan_enumerate_instance_params_render() {
		let _guard = HOST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		let Some(dir) = test_plugin_scan_dir() else {
			println!("SKIP: the bundled test plugin is not built");
			return;
		};
		assert!(scan_path(&dir.to_string_lossy()));

		let info = plugins()
			.into_iter()
			.find(|p| p.identifier == "org.oak.test-plugin")
			.expect("the test plugin is discovered");
		assert_eq!(info.label, "Oak Test Plugin");
		assert!(
			info.contexts
				.iter()
				.any(|c| c == "OfxImageEffectContextFilter")
		);

		let inst = Instance::create("org.oak.test-plugin", None).expect("instance");
		assert_eq!(
			inst.context().as_deref(),
			Some("OfxImageEffectContextFilter")
		);

		// Parameter introspection.
		let names: Vec<String> = (0..inst.param_count().unwrap())
			.map(|i| inst.param_name_at(i).unwrap())
			.collect();
		for expected in ["gain", "mode", "debug", "label", "button"] {
			assert!(
				names.iter().any(|n| n == expected),
				"missing param {expected}"
			);
		}
		assert_eq!(inst.param_type_at(0).as_deref(), Some("OfxParamTypeDouble"));
		assert_eq!(inst.param_label_at(0).as_deref(), Some("Gain"));

		// Typed get/set roundtrips.
		assert!(inst.set_param_double("gain", &[1.5]));
		assert_eq!(inst.param_double("gain"), Some(vec![1.5]));
		assert!(
			!inst.set_param_double("gain", &[1.0, 2.0]),
			"arity mismatch rejected"
		);
		assert!(inst.set_param_choice("mode", 1));
		assert_eq!(inst.param_choice("mode"), Some(1));
		assert!(inst.set_param_string("label", "hello"));
		assert_eq!(inst.param_string("label").as_deref(), Some("hello"));
		assert_eq!(inst.param_int("gain"), None, "type mismatch rejected");
		assert!(!inst.set_param_bool("gain", true), "type mismatch rejected");
		assert!(inst.push_button("button"));

		// Sequence bracket + RoD.
		assert!(inst.begin_sequence(0.0, 1.0));
		assert!(inst.end_sequence(0.0, 1.0));
		assert!(inst.region_of_definition(0.0).is_some());

		// Headless render: the test plugin fills the output with
		// (0.5, 0.5, 0.5, 1.0) and reports progress.
		let events = std::sync::Arc::new(Mutex::new(Vec::<(String, f64)>::new()));
		let ended = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
		{
			let events = events.clone();
			let ended = ended.clone();
			set_progress_handler(
				move |label, _message, fraction| {
					events.lock().unwrap().push((label.to_string(), fraction));
					true
				},
				move || {
					ended.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
				},
			);
		}
		let mut dst = vec![0.0f32; 4 * 2 * 4];
		assert!(inst.render_frame(0.0, None, &mut dst, (4, 2)));
		assert!(dst.chunks_exact(4).all(|px| px == [0.5, 0.5, 0.5, 1.0]));
		let seen = events.lock().unwrap();
		assert!(seen.iter().any(|(label, f)| label == "render" && *f == 0.5));
		assert!(ended.load(std::sync::atomic::Ordering::SeqCst) > 0);
		clear_progress_handler();
		assert!(!has_progress_handler());
	}

	#[test]
	fn interact_lifecycle() {
		let _guard = HOST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
		let Some(dir) = test_plugin_scan_dir() else {
			println!("SKIP: the bundled test plugin is not built");
			return;
		};
		assert!(scan_path(&dir.to_string_lossy()));
		let inst =
			Instance::create("org.oak.test-plugin.interact", None).expect("interact instance");
		let Some(interact) = inst.create_interact() else {
			panic!("the interact variant declares an overlay interact");
		};
		assert_eq!(interact.idle(), status::OK);
		// The test plugin leaves the focus actions unhandled
		// (ReplyDefault = "not consumed; the host may continue").
		assert_eq!(interact.gain_focus(0.0), status::REPLY_DEFAULT);
		assert_eq!(interact.pen_down((4.0, 4.0), 0.0), status::OK);
		assert_eq!(interact.pen_motion((5.0, 5.0), true, 0.1), status::OK);
		assert_eq!(interact.pen_up((5.0, 5.0), 0.2), status::OK);
		assert_eq!(interact.key_down(key::A, "a", 0.0), status::OK);
		assert_eq!(interact.key_up(key::A, "a", 0.1), status::OK);
		assert_eq!(interact.lose_focus(0.0), status::REPLY_DEFAULT);
		let _ = interact.take_redraw();
		let _ = interact.take_swap();
		// Drop destroys the interact.

		// A plugin without an interact yields None.
		let plain = Instance::create("org.oak.test-plugin", None).expect("plain instance");
		assert!(plain.create_interact().is_none());
	}
}

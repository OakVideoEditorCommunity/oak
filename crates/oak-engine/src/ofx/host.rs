//! Host lifecycle, plugin discovery/enumeration, the instance
//! registry, and the app-service plumbing (progress handler, viewer
//! time, project extent) — the `oakui/ofx.rs` wiring exposed over
//! the C ABI.

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use oak_ofx_plugin::host::Host;
use oak_ofx_plugin::node_factory;
use oak_ofx_plugin::progress::UiProgressReporter;
use oak_ofx_plugin::suites::timeline::{self, ViewerTimeInfo};

use crate::codec::{copy_str_out, str_arg};

/// `kOfxPropLabel` (the label property on plugin descriptors and
/// parameter definitions; `oak_ofx_plugin::host::PROP_LABEL` is
/// crate-private).
const PROP_LABEL: &str = "OfxPropLabel";

// ---------------------------------------------------------------------------
// Discovery / enumeration
// ---------------------------------------------------------------------------

/// Scans the standard OFX plugin paths. Returns the number of
/// discovered plugins, -1 when the scan failed. Plugin support is an
/// optional capability: a failure never aborts the caller.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_scan() -> i32 {
	let host = Host::global();
	if host.cache.scan().is_err() {
		return -1;
	}
	host.cache.count() as i32
}

/// Scans one extra plugin directory. False on invalid input or scan
/// failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_scan_path(path: *const u8, path_len: usize) -> bool {
	let Some(path) = (unsafe { str_arg(path, path_len) }) else {
		return false;
	};
	Host::global()
		.cache
		.scan_path(std::path::Path::new(path))
		.is_ok()
}

/// Number of discovered plugins.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_plugin_count() -> i32 {
	Host::global().cache.count() as i32
}

/// Copies a plugin string field, snprintf-style; -1 when the index is
/// out of range.
fn plugin_string_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
	f: impl Fn(&oak_ofx_plugin::host::Plugin) -> String,
) -> i32 {
	let host = Host::global();
	if index < 0 {
		return -1;
	}
	let Some(plugin) = host.cache.at(index as usize) else {
		return -1;
	};
	copy_str_out(&f(&plugin), buf, buf_len)
}

/// The plugin identifier at `index`, snprintf-style; -1 when out of
/// range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_plugin_identifier_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	plugin_string_at(index, buf, buf_len, |p| p.identifier.clone())
}

/// The plugin display label at `index` (`kOfxPropLabel` from the
/// descriptor), snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_plugin_label_at(index: i32, buf: *mut u8, buf_len: usize) -> i32 {
	plugin_string_at(index, buf, buf_len, |p| {
		match p.descriptor.props.get(PROP_LABEL, 0) {
			Some(oak_ofx_plugin::property::Value::String(s)) => s.to_string_lossy().into_owned(),
			_ => String::new(),
		}
	})
}

/// The plugin bundle path at `index`, snprintf-style; -1 when out of
/// range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_plugin_path_at(index: i32, buf: *mut u8, buf_len: usize) -> i32 {
	plugin_string_at(index, buf, buf_len, |p| {
		p.bundle_path.to_string_lossy().into_owned()
	})
}

/// The plugin version at `index`. False when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_plugin_version_at(
	index: i32,
	major: *mut u32,
	minor: *mut u32,
) -> bool {
	let host = Host::global();
	if index < 0 {
		return false;
	}
	let Some(plugin) = host.cache.at(index as usize) else {
		return false;
	};
	if !major.is_null() {
		unsafe { *major = plugin.version.0 };
	}
	if !minor.is_null() {
		unsafe { *minor = plugin.version.1 };
	}
	true
}

/// Number of contexts (`OfxImageEffectContext*`) the plugin at
/// `index` supports; -1 when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_plugin_context_count_at(index: i32) -> i32 {
	let host = Host::global();
	if index < 0 {
		return -1;
	}
	host.cache
		.at(index as usize)
		.map(|p| p.contexts.len() as i32)
		.unwrap_or(-1)
}

/// The context string at `context_index` of the plugin at `index`,
/// snprintf-style; -1 when either index is out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_plugin_context_at(
	index: i32,
	context_index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let host = Host::global();
	if index < 0 || context_index < 0 {
		return -1;
	}
	let Some(plugin) = host.cache.at(index as usize) else {
		return -1;
	};
	let Some(context) = plugin.contexts.get(context_index as usize) else {
		return -1;
	};
	copy_str_out(context, buf, buf_len)
}

/// Shuts the host down: notifies destroyInstance on every live
/// instance, then unloads the plugins. Registered instance ids stay
/// allocated but inert (their render entry short-circuits).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_shutdown() {
	crate::ofx::interact::shutdown();
	unsafe { oak_ofx_set_progress_handler(None, None, std::ptr::null_mut()) };
	Host::global().shutdown();
}

/// Live instance count (leak diagnostic).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_alive_count() -> i32 {
	Host::global().alive_count() as i32
}

// ---------------------------------------------------------------------------
// Instances (node_factory registry ids; 0 = invalid)
// ---------------------------------------------------------------------------

/// Creates an instance of `plugin_id` and registers it, returning the
/// registry id (0 on failure: unknown plugin, no supported context, or
/// the plugin's describe/createInstance failed). A null/empty
/// `context` uses the host's preferred-context fallback chain
/// (filter → general → tracker → paint → rest).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_instance_create(
	plugin_id: *const u8,
	plugin_id_len: usize,
	context: *const u8,
	context_len: usize,
) -> u64 {
	let Some(id) = (unsafe { str_arg(plugin_id, plugin_id_len) }) else {
		return 0;
	};
	let host = Host::global();
	let inst = match unsafe { str_arg(context, context_len) } {
		Some(ctx) if !ctx.is_empty() => host.create_instance(id, Some(ctx)),
		_ => host.create_instance_preferred(id).map(|(_, inst)| inst),
	};
	match inst {
		Ok(inst) => node_factory::register_instance(inst),
		Err(_) => 0,
	}
}

/// Removes the instance from the registry; it is destroyed once no
/// references remain. Unknown ids are a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_release(id: u64) {
	node_factory::unregister_instance(id);
}

/// Number of registered instances.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_count() -> i32 {
	node_factory::registered_instance_count() as i32
}

/// The context an instance was created in, snprintf-style; -1 when
/// the id is unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_instance_context(id: u64, buf: *mut u8, buf_len: usize) -> i32 {
	let Some(inst) = node_factory::instance_from_id(id) else {
		return -1;
	};
	copy_str_out(&inst.value.context, buf, buf_len)
}

/// Triggers a push-button parameter (the inspector's button path).
/// False on invalid input, unknown instance, or no such push-button
/// parameter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_instance_push_button(
	id: u64,
	param: *const u8,
	param_len: usize,
) -> bool {
	let Some(param) = (unsafe { str_arg(param, param_len) }) else {
		return false;
	};
	node_factory::push_button_clicked(id, param)
}

/// Sets/clears the instance's cancel flag: a running render aborts at
/// the next progress batch boundary.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_set_cancelled(id: u64, cancelled: bool) {
	let Some(inst) = node_factory::instance_from_id(id) else {
		return;
	};
	inst.value
		.cancel
		.store(cancelled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the instance's cancel flag is set (false on unknown id).
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_is_cancelled(id: u64) -> bool {
	let Some(inst) = node_factory::instance_from_id(id) else {
		return false;
	};
	inst.value.cancel.load(std::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Node-factory registration
// ---------------------------------------------------------------------------

/// Registers every discovered plugin into the node factory as a
/// dynamic node type (idempotent; also installs the render executor
/// and the plugin-node duplicator). Returns the number of newly
/// registered node types.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_register_plugin_nodes() -> i32 {
	node_factory::register_plugin_nodes().len() as i32
}

/// Installs the render executor / plugin-node duplicator into
/// oak-render / oak-node (idempotent). Needed by consumers that render
/// plugin nodes through the graph without registering node types.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_install_render_executor() {
	node_factory::install_render_executor();
}

/// Pushes the project extent (width/height) the OFX
/// normalised-coordinate default conversion uses.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_set_project_extent(width: f64, height: f64) {
	node_factory::set_project_extent(width.max(1.0), height.max(1.0));
}

// ---------------------------------------------------------------------------
// Viewer time (timeline suite fallback provider)
// ---------------------------------------------------------------------------

static VIEWER_TIME: OnceLock<Mutex<ViewerTimeInfo>> = OnceLock::new();
static VIEWER_PROVIDER: OnceLock<()> = OnceLock::new();

fn viewer_slot() -> &'static Mutex<ViewerTimeInfo> {
	VIEWER_TIME.get_or_init(|| {
		Mutex::new(ViewerTimeInfo {
			time: 0.0,
			range_min: 0.0,
			range_max: 0.0,
		})
	})
}

/// Updates the active-viewer time snapshot the timeline suite falls
/// back to when no render context is live. Installs the provider on
/// first call.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_set_viewer_time(time: f64, range_min: f64, range_max: f64) {
	VIEWER_PROVIDER.get_or_init(|| {
		timeline::set_active_viewer_provider(Some(Arc::new(|| {
			Some(*viewer_slot().lock().unwrap_or_else(|e| e.into_inner()))
		})));
	});
	*viewer_slot().lock().unwrap_or_else(|e| e.into_inner()) = ViewerTimeInfo {
		time,
		range_min,
		range_max,
	};
}

// ---------------------------------------------------------------------------
// Progress handler (reporter factory bridge)
// ---------------------------------------------------------------------------

/// Progress callback: `label`/`message` come from the plugin's
/// progressStart, `fraction` in 0.0..=1.0 from progressUpdate. Return
/// false to request cancellation (the render aborts with a failure
/// status). The string pointers are valid for the call duration only.
pub type OakOfxProgressFn = unsafe extern "C" fn(
	label: *const c_char,
	message: *const c_char,
	fraction: f64,
	userdata: *mut c_void,
) -> bool;

/// progressEnd notification (the progress UI can close).
pub type OakOfxProgressEndFn = unsafe extern "C" fn(userdata: *mut c_void);

struct CProgressReporter {
	cb: OakOfxProgressFn,
	end_cb: Option<OakOfxProgressEndFn>,
	userdata: *mut c_void,
	label: CString,
	message: CString,
}

// The userdata pointer is opaque consumer state; the callback contract
// keeps it valid while any reporter can fire.
unsafe impl Send for CProgressReporter {}

impl UiProgressReporter for CProgressReporter {
	fn update(&mut self, progress: f64) -> bool {
		let accepted = unsafe {
			(self.cb)(
				self.label.as_ptr(),
				self.message.as_ptr(),
				progress,
				self.userdata,
			)
		};
		accepted && !RENDER_CANCEL.load(Ordering::Relaxed)
	}
	fn end(&mut self) {
		if let Some(end) = self.end_cb {
			unsafe { end(self.userdata) };
		}
	}
}

/// Inert fallback: the handler was cleared between registration and
/// the plugin's progressStart.
struct SilentReporter;

impl UiProgressReporter for SilentReporter {
	fn update(&mut self, _progress: f64) -> bool {
		true
	}
}

static PROGRESS_HANDLER: Mutex<(Option<OakOfxProgressFn>, Option<OakOfxProgressEndFn>, usize)> =
	Mutex::new((None, None, 0));
static RENDER_CANCEL: AtomicBool = AtomicBool::new(false);

fn install_worker_progress_handler() {
	oak_render::procpool::set_plugin_progress_cb(Some(Arc::new(|label, message, fraction| {
		let (cb, _, userdata) = *PROGRESS_HANDLER.lock().unwrap_or_else(|e| e.into_inner());
		let Some(cb) = cb else {
			return;
		};
		let label = CString::new(label).unwrap_or_default();
		let message = CString::new(message).unwrap_or_default();
		unsafe {
			cb(
				label.as_ptr(),
				message.as_ptr(),
				fraction,
				userdata as *mut c_void,
			);
		}
	})));
}

/// Registers the progress handler (or with a null `cb` clears it).
/// The handler bridges into the OFX progress suite's reporter
/// factory: every plugin progressStart builds a reporter that calls
/// `cb`, and progressEnd calls `end_cb`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_ofx_set_progress_handler(
	cb: Option<OakOfxProgressFn>,
	end_cb: Option<OakOfxProgressEndFn>,
	userdata: *mut c_void,
) {
	let mut slot = PROGRESS_HANDLER.lock().unwrap_or_else(|e| e.into_inner());
	match cb {
		None => {
			*slot = (None, None, 0);
			drop(slot);
			oak_render::procpool::set_plugin_progress_cb(None);
			oak_ofx_plugin::progress::set_reporter_factory(None);
		}
		Some(_) => {
			*slot = (cb, end_cb, userdata as usize);
			drop(slot);
			install_worker_progress_handler();
			oak_ofx_plugin::progress::set_reporter_factory(Some(Arc::new(|label, message| {
				RENDER_CANCEL.store(false, Ordering::Relaxed);
				let (cb, end_cb, userdata) =
					*PROGRESS_HANDLER.lock().unwrap_or_else(|e| e.into_inner());
				let Some(cb) = cb else {
					return Box::new(SilentReporter) as Box<dyn UiProgressReporter>;
				};
				Box::new(CProgressReporter {
					cb,
					end_cb,
					userdata: userdata as *mut c_void,
					label: CString::new(label).unwrap_or_default(),
					message: CString::new(message).unwrap_or_default(),
				}) as Box<dyn UiProgressReporter>
			})));
		}
	}
}

/// Cancels inline and process-worker plugin renders at their next progress
/// boundary. A fresh progressStart clears the inline cancellation flag.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_cancel_render() {
	RENDER_CANCEL.store(true, Ordering::Relaxed);
	oak_render::procpool::request_plugin_cancel_all();
}

/// Whether a progress handler is registered.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_has_progress_handler() -> bool {
	oak_ofx_plugin::progress::has_reporter_factory()
}

// ---------------------------------------------------------------------------
// Persistent messages (the inspector's plugin badge)
// ---------------------------------------------------------------------------

/// The instance's persistent message count (the message suite keys
/// them by the instance handle — the props address); -1 when the id
/// is unknown.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_persistent_message_count(id: u64) -> i32 {
	let Some(inst) = node_factory::instance_from_id(id) else {
		return -1;
	};
	let handle = &inst.value.props as *const _ as usize;
	oak_ofx_plugin::suites::message::persistent_message_count(handle) as i32
}

/// Clears the instance's persistent messages. Unknown ids are a
/// no-op.
#[unsafe(no_mangle)]
pub extern "C" fn oak_ofx_instance_clear_persistent_messages(id: u64) {
	let Some(inst) = node_factory::instance_from_id(id) else {
		return;
	};
	let handle = &inst.value.props as *const _ as usize;
	oak_ofx_plugin::suites::message::clear_persistent_messages(handle);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn plugin_enumeration_rejects_bad_index() {
		let mut buf = [0u8; 64];
		assert_eq!(
			unsafe { oak_ofx_plugin_identifier_at(-1, buf.as_mut_ptr(), 64) },
			-1
		);
		assert!(!unsafe {
			oak_ofx_plugin_version_at(-1, std::ptr::null_mut(), std::ptr::null_mut())
		});
		assert_eq!(oak_ofx_plugin_context_count_at(-1), -1);
	}

	#[test]
	fn instance_registry_null_safety() {
		// Unknown ids: getters fail, mutators are no-ops (no panic).
		assert_eq!(
			unsafe { oak_ofx_instance_context(0, std::ptr::null_mut(), 0) },
			-1
		);
		oak_ofx_instance_release(u64::MAX);
		assert!(!unsafe { oak_ofx_instance_push_button(u64::MAX, "x".as_ptr(), 1) });
		oak_ofx_instance_set_cancelled(u64::MAX, true);
		assert!(!oak_ofx_instance_is_cancelled(u64::MAX));
		assert_eq!(oak_ofx_instance_persistent_message_count(u64::MAX), -1);
		oak_ofx_instance_clear_persistent_messages(u64::MAX);
	}

	#[test]
	fn progress_handler_registration_roundtrip() {
		assert!(!oak_ofx_has_progress_handler());
		unsafe extern "C" fn cb(
			_label: *const c_char,
			_message: *const c_char,
			_fraction: f64,
			_userdata: *mut c_void,
		) -> bool {
			true
		}
		unsafe {
			oak_ofx_set_progress_handler(Some(cb), None, std::ptr::null_mut());
		}
		assert!(oak_ofx_has_progress_handler());
		unsafe {
			oak_ofx_set_progress_handler(None, None, std::ptr::null_mut());
		}
		assert!(!oak_ofx_has_progress_handler());
	}

	#[test]
	fn viewer_time_installs_provider() {
		oak_ofx_set_viewer_time(3.5, 1.0, 99.0);
		assert!(
			VIEWER_PROVIDER.get().is_some(),
			"set_viewer_time installs the provider"
		);
		let info = *viewer_slot().lock().unwrap_or_else(|e| e.into_inner());
		assert_eq!(info.time, 3.5);
		assert_eq!((info.range_min, info.range_max), (1.0, 99.0));
	}
}

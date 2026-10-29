//! Safe wrapper over the engine's extension-plugin protocol
//! (`oak_plugin_*`): registration, command contribution, enumeration,
//! and dispatch.
//!
//! A plugin registers a descriptor plus one command callback; the host
//! enumerates plugins and their commands to build menus; invoking a
//! command calls back into the owning plugin.

use std::ffi::c_void;

use thiserror::Error;

use crate::codec::string_out;

/// `repr(C)` mirror of the engine's `OakPluginDescriptor`.
#[repr(C)]
struct OakPluginDescriptor {
	id: [u8; 128],
	name: [u8; 256],
	version: [u8; 64],
	author: [u8; 256],
	description: [u8; 1024],
}

type CommandFn = unsafe extern "C" fn(command_id: *const std::ffi::c_char, userdata: *mut c_void);

#[cfg_attr(windows, link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim"))]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_plugin_register(
		descriptor: *const OakPluginDescriptor,
		on_command: Option<CommandFn>,
		userdata: *mut c_void,
	) -> i32;
	fn oak_plugin_unregister(index: i32) -> bool;
	fn oak_plugin_count() -> i32;
	fn oak_plugin_descriptor(index: i32, out: *mut OakPluginDescriptor) -> bool;
	fn oak_plugin_add_command(
		index: i32,
		command_id: *const u8,
		command_id_len: usize,
		title: *const u8,
		title_len: usize,
		menu_path: *const u8,
		menu_path_len: usize,
	) -> bool;
	fn oak_plugin_command_count(index: i32) -> i32;
	fn oak_plugin_command_id_at(index: i32, command_index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_plugin_command_title_at(
		index: i32,
		command_index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_plugin_command_menu_path_at(
		index: i32,
		command_index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;
	fn oak_plugin_invoke_command(command_id: *const u8, command_id_len: usize) -> bool;
}

/// Plugin registration failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PluginError {
	/// The id is empty or already registered.
	#[error("the plugin id is empty or already registered")]
	DuplicateId,
}

/// A plugin's static metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginDescriptor {
	/// Unique reverse-DNS id (e.g. "com.example.myplugin").
	pub id: String,
	/// Display name.
	pub name: String,
	/// Version string.
	pub version: String,
	/// Author.
	pub author: String,
	/// Short description.
	pub description: String,
}

fn fill_field(field: &mut [u8], value: &str) {
	let n = value.len().min(field.len() - 1);
	field[..n].copy_from_slice(&value.as_bytes()[..n]);
}

impl PluginDescriptor {
	fn to_c(&self) -> OakPluginDescriptor {
		let mut d = OakPluginDescriptor {
			id: [0; 128],
			name: [0; 256],
			version: [0; 64],
			author: [0; 256],
			description: [0; 1024],
		};
		fill_field(&mut d.id, &self.id);
		fill_field(&mut d.name, &self.name);
		fill_field(&mut d.version, &self.version);
		fill_field(&mut d.author, &self.author);
		fill_field(&mut d.description, &self.description);
		d
	}

	fn from_c(d: &OakPluginDescriptor) -> Self {
		let read = |f: &[u8]| {
			let end = f.iter().position(|&b| b == 0).unwrap_or(f.len());
			String::from_utf8_lossy(&f[..end]).into_owned()
		};
		Self {
			id: read(&d.id),
			name: read(&d.name),
			version: read(&d.version),
			author: read(&d.author),
			description: read(&d.description),
		}
	}
}

/// One contributed command (a menu item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCommand {
	/// The `id` field.
	pub id: String,
	/// The `title` field.
	pub title: String,
	/// The `menu_path` field.
	pub menu_path: String,
}

/// A registered plugin. `Drop` unregisters it (its callback stops being
/// reachable).
type BoxedCallback = Box<dyn Fn(&str) + Send>;

/// The `Plugin` type.
pub struct Plugin {
	index: i32,
	// Thin pointer to a boxed callback (double-boxed so the pointer is
	// thin); the engine hands it back as userdata on every command.
	// Reclaimed on Drop, after unregistration.
	callback: *mut BoxedCallback,
}

// The pointer targets a Send closure; registry access is serialized by
// the engine's mutex and the callback only runs while registered.
unsafe impl Send for Plugin {}

impl Plugin {
	/// Registers a plugin; the callback receives command ids as the host
	/// invokes them. `None` when the id is empty or already registered.
	pub fn register(
		descriptor: PluginDescriptor,
		on_command: impl Fn(&str) + Send + 'static,
	) -> Option<Plugin> {
		unsafe extern "C" fn trampoline(command_id: *const std::ffi::c_char, userdata: *mut c_void) {
			if command_id.is_null() || userdata.is_null() {
				return;
			}
			let f = unsafe { &**(userdata as *const BoxedCallback) };
			let id = unsafe { std::ffi::CStr::from_ptr(command_id) }
				.to_string_lossy();
			f(&id);
		}
		// Double-box so the userdata pointer is thin (a Box<dyn Fn> is a
		// fat pointer and cannot cross as *mut c_void directly).
		let callback = Box::into_raw(Box::new(Box::new(on_command) as BoxedCallback));
		let c_descriptor = descriptor.to_c();
		let index = unsafe {
			oak_plugin_register(&c_descriptor, Some(trampoline), callback as *mut c_void)
		};
		if index < 0 {
			unsafe { drop(Box::from_raw(callback)) };
			return None;
		}
		Some(Plugin { index, callback })
	}

	/// Contributes a command (menu item). `menu_path` uses `/`
	/// separators (e.g. "Tools/Export"). False when the id is already
	/// contributed.
	pub fn add_command(&self, id: &str, title: &str, menu_path: &str) -> bool {
		unsafe {
			oak_plugin_add_command(
				self.index,
				id.as_ptr(),
				id.len(),
				title.as_ptr(),
				title.len(),
				menu_path.as_ptr(),
				menu_path.len(),
			)
		}
	}

	/// This plugin's descriptor.
	pub fn descriptor(&self) -> PluginDescriptor {
		let mut d = OakPluginDescriptor {
			id: [0; 128],
			name: [0; 256],
			version: [0; 64],
			author: [0; 256],
			description: [0; 1024],
		};
		unsafe { oak_plugin_descriptor(self.index, &mut d) };
		PluginDescriptor::from_c(&d)
	}

	/// This plugin's contributed commands.
	pub fn commands(&self) -> Vec<PluginCommand> {
		let count = unsafe { oak_plugin_command_count(self.index) };
		(0..count)
			.map(|i| PluginCommand {
				id: string_out(|buf, len| unsafe {
					oak_plugin_command_id_at(self.index, i, buf, len)
				})
				.unwrap_or_default(),
				title: string_out(|buf, len| unsafe {
					oak_plugin_command_title_at(self.index, i, buf, len)
				})
				.unwrap_or_default(),
				menu_path: string_out(|buf, len| unsafe {
					oak_plugin_command_menu_path_at(self.index, i, buf, len)
				})
				.unwrap_or_default(),
			})
			.collect()
	}
}

impl Drop for Plugin {
	fn drop(&mut self) {
		// Unregister first: the callback is unreachable afterwards.
		unsafe { oak_plugin_unregister(self.index) };
		unsafe { drop(Box::from_raw(self.callback)) };
	}
}

/// Number of registered plugins.
pub fn plugin_count() -> i32 {
	unsafe { oak_plugin_count() }
}

/// Invokes a command by id (dispatches to the contributing plugin).
/// False when no plugin contributes it.
pub fn invoke_command(command_id: &str) -> bool {
	unsafe { oak_plugin_invoke_command(command_id.as_ptr(), command_id.len()) }
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;
	use std::sync::atomic::{AtomicUsize, Ordering};

	// The registry is process-global; serialize the plugin tests.
	static SERIAL: Mutex<()> = Mutex::new(());

	#[test]
	fn register_contribute_invoke_drop() {
		let _guard = SERIAL.lock().unwrap();
		static CALLS: AtomicUsize = AtomicUsize::new(0);
		static LAST: Mutex<String> = Mutex::new(String::new());

		let base = plugin_count();
		let plugin = Plugin::register(
			PluginDescriptor {
				id: "com.oak.sdk-test".to_string(),
				name: "SDK Test".to_string(),
				version: "1.0.0".to_string(),
				..Default::default()
			},
			move |id| {
				CALLS.fetch_add(1, Ordering::SeqCst);
				*LAST.lock().unwrap() = id.to_string();
			},
		)
		.expect("register");
		assert_eq!(plugin_count(), base + 1);

		assert_eq!(plugin.descriptor().name, "SDK Test");

		assert!(plugin.add_command("export_markers", "Export Markers", "Tools"));
		assert!(!plugin.add_command("export_markers", "Dup", "Tools"), "duplicate command id");
		assert_eq!(plugin.commands().len(), 1);
		assert_eq!(plugin.commands()[0].menu_path, "Tools");

		let before = CALLS.load(Ordering::SeqCst);
		assert!(invoke_command("export_markers"));
		assert_eq!(CALLS.load(Ordering::SeqCst), before + 1);
		assert_eq!(LAST.lock().unwrap().as_str(), "export_markers");

		assert!(!invoke_command("no-such-command"));

		drop(plugin);
		assert_eq!(plugin_count(), base);
		assert!(!invoke_command("export_markers"), "unregistered plugins don't fire");
	}

	#[test]
	fn duplicate_ids_rejected() {
		let _guard = SERIAL.lock().unwrap();
		let plugin = Plugin::register(
			PluginDescriptor {
				id: "com.oak.sdk-dup".to_string(),
				..Default::default()
			},
			|_| {},
		)
		.expect("register");
		assert!(Plugin::register(
			PluginDescriptor {
				id: "com.oak.sdk-dup".to_string(),
				..Default::default()
			},
			|_| {},
		)
		.is_none());
		drop(plugin);
	}
}

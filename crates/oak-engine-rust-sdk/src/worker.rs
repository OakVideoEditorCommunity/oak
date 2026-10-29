//! Render-worker and OpenFX-host process entry points.
//!
//! The worker protocol (NDJSON control messages plus the shared-memory
//! frame-slot transport) is engine-internal. The `oak-worker` binary is a
//! thin launcher: it links this SDK and calls [`main`], so the protocol,
//! graph loading and render loop live in `oak-engine` just like every other
//! domain capability.
//!
//! A host application normally does not call this module directly; it spawns
//! the installed `oak-worker` binary. It is public so alternative launchers
//! and tests can start the same runtime.

use std::ffi::c_void;

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_worker_run(backend: *const u8, backend_len: usize) -> i32;
	fn oak_worker_ofx_host(args: *const u8, args_len: usize) -> i32;
}

/// The worker control-protocol version announced in the handshake.
pub const PROTOCOL_VERSION: i32 = 1;

/// Parses `--backend <name>` from `args` (the last flag wins, names are
/// lowercased, default `"opengl"`). `args` may or may not include `argv[0]`.
pub fn parse_backend(args: &[String]) -> String {
	let mut backend = "opengl".to_string();
	let mut index = 0usize;
	while index < args.len() {
		if args[index] == "--backend" && index + 1 < args.len() {
			backend = args[index + 1].to_ascii_lowercase();
			index += 2;
		} else {
			index += 1;
		}
	}
	backend
}

/// Runs the ticket worker loop with the given backend (`opengl`, `vulkan`,
/// `cpu`, `none`, ...). Returns the process exit code.
pub fn run(backend: &str) -> i32 {
	unsafe { oak_worker_run(backend.as_ptr(), backend.len()) }
}

/// Runs the single OpenFX host loop. `args` is the full argv (including the
/// program name); the `--ofx-crash-*` flags are only used by crash tests.
pub fn ofx_host_main(args: &[String]) -> i32 {
	let text = args.join("\n");
	unsafe { oak_worker_ofx_host(text.as_ptr(), text.len()) }
}

/// The `oak-worker` process entry point: dispatches `--ofx-host`, otherwise
/// parses `--backend` and runs the ticket worker. Returns the exit code.
pub fn main() -> i32 {
	let args: Vec<String> = std::env::args().collect();
	if args.iter().any(|arg| arg == "--ofx-host") {
		return ofx_host_main(&args);
	}
	run(&parse_backend(&args))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn protocol_version_is_one() {
		assert_eq!(PROTOCOL_VERSION, 1);
	}

	#[test]
	fn backend_parsing_matches_the_launcher() {
		assert_eq!(parse_backend(&["oak-worker".to_string()]), "opengl");
		assert_eq!(
			parse_backend(&["oak-worker".into(), "--backend".into(), "Vulkan".into()]),
			"vulkan"
		);
		assert_eq!(
			parse_backend(&["oak-worker".into(), "--backend".into(), "none".into()]),
			"none"
		);
		assert_eq!(
			parse_backend(&[
				"oak-worker".into(),
				"--backend".into(),
				"vulkan".into(),
				"--backend".into(),
				"opengl".into(),
			]),
			"opengl"
		);
		assert_eq!(
			parse_backend(&["oak-worker".into(), "--backend".into()]),
			"opengl"
		);
	}
}

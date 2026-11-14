//! `opp-smoke` — no-GUI acceptance driver for the OPP/1 host core (T7.1).
//!
//! usage: `opp-smoke <plugin-dir|plugin.toml> [--duration-ms N]`
//!
//! Prints every state transition, then a summary line. Exit codes:
//! `0` reached Ready and ended without failure · `2` ended `Failed` ·
//! `3` never reached Ready within the duration · `64` usage ·
//! `65` manifest error · `66` start error.

use oak_extensions::lifecycle::{LifecycleConfig, PluginHost, PluginState};
use oak_extensions::manifest::Manifest;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();
	if args.is_empty() {
		eprintln!("usage: opp-smoke <plugin-dir|plugin.toml> [--duration-ms N]");
		std::process::exit(64);
	}
	let target = PathBuf::from(&args[0]);
	let mut duration_ms: u64 = 30_000;
	if let Some(i) = args.iter().position(|a| a == "--duration-ms") {
		duration_ms = match args.get(i + 1).and_then(|v| v.parse::<u64>().ok()) {
			Some(v) => v,
			None => {
				eprintln!("--duration-ms needs a number");
				std::process::exit(64);
			}
		};
	}

	let (manifest_path, plugin_dir) = if target.is_dir() {
		(target.join("plugin.toml"), target.clone())
	} else {
		let dir = target
			.parent()
			.map(|p| p.to_path_buf())
			.unwrap_or_else(|| PathBuf::from("."));
		(target.clone(), dir)
	};

	let manifest = match Manifest::load(&manifest_path) {
		Ok(m) => m,
		Err(e) => {
			eprintln!("[smoke] manifest error: {e}");
			std::process::exit(65);
		}
	};
	let host = match PluginHost::start(manifest, plugin_dir, LifecycleConfig::default()) {
		Ok(h) => h,
		Err(e) => {
			eprintln!("[smoke] start error: {e}");
			std::process::exit(66);
		}
	};

	let states = host.subscribe_states();
	println!("[smoke] initial state: {:?}", host.state());
	let deadline = Instant::now() + Duration::from_millis(duration_ms);
	let mut reached_ready = matches!(host.state(), PluginState::Ready);
	let mut timed_out = false;
	loop {
		match states.recv_timeout(Duration::from_millis(100)) {
			Ok(s) => {
				println!("[smoke] state: {s:?}");
				if s == PluginState::Ready {
					reached_ready = true;
				}
				if s.is_terminal() {
					break;
				}
			}
			Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
			Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
		}
		if Instant::now() >= deadline && !timed_out {
			timed_out = true;
			println!("[smoke] duration reached — shutting the plugin down");
			host.shutdown("smoke_duration");
		}
	}

	println!("[smoke] final state: {:?}", host.state());
	println!(
		"[smoke] spawns={} heartbeats_ok={} failure={:?} kill={:?} exit={:?}",
		host.spawn_count(),
		host.heartbeat_ok(),
		host.failure(),
		host.kill_method(),
		host.exit_summary()
	);
	let stderr = host.stderr();
	if !stderr.is_empty() {
		println!("[smoke] stderr (last {} line(s)):", stderr.len().min(5));
		for l in stderr.iter().rev().take(5).rev() {
			println!("\t{l}");
		}
	}
	if matches!(host.state(), PluginState::Failed) {
		std::process::exit(2);
	}
	if timed_out && !reached_ready {
		std::process::exit(3);
	}
	std::process::exit(0);
}

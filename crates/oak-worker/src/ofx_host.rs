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

//! `oak-worker --ofx-host`: the single OpenFX host process (M3, design
//! §3.2).
//!
//! The host loads every OFX plugin once (the worker binary's normal plugin
//! runtime) and then serves `ofx_job` messages from the main process over
//! NDJSON, with frames moving through the input/output
//! [`FrameSlotPool`]s announced by the handshake. Each job is resolved to
//! a host-local instance by the plugin **identifier** (the cross-process
//! stable key) and rendered through the same in-process executor the
//! workers used to install ([`oak_plugin::node_factory::install_render_executor`]).
//!
//! Progress is flushed to stdout immediately (the main process's reader
//! forwards it to the plugin-progress dialog), and `plugin_cancel` sets
//! the same sticky flag protocol the worker used: the next `progressStart`
//! resets it and every `progressUpdate` after a cancel answers false, so
//! the plugin aborts at its next progress call.
//!
//! The host is deliberately single-threaded and synchronous, matching the
//! worker model; crash isolation comes from the parent respawning it.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

use oak_core::frame::VideoParamsPod;
use oak_core::texture::{Frame, Texture};
use oak_core::PixelFormat;
use oak_render::eval::{self, JobSpec, PluginJobRequest};
use oak_render::ipc::{
	error_message, write_message, FrameSlotPool, HandshakeMsg, OfxJobMsg, OfxResultMsg,
	PluginProgressMsg, SharedMemoryRegion, ShmMode, TYPE_HANDSHAKE, TYPE_OFX_JOB,
	TYPE_PLUGIN_CANCEL, TYPE_SHUTDOWN,
};

/// Sticky plugin cancel (protocol parity with `worker.rs`): set by
/// `plugin_cancel`, reset by the next `progressStart`.
static OFX_CANCEL: AtomicBool = AtomicBool::new(false);

/// stdout is shared by progress events (emitted from inside the plugin
/// render) and control responses.
static OUT_LOCK: Mutex<()> = Mutex::new(());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
	m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Write one NDJSON line to stdout, flushed (progress must stream while
/// the job renders). No-op under unit tests (they exercise the reporter
/// return values, not the pipe).
fn emit(value: &serde_json::Value) {
	if cfg!(test) {
		return;
	}
	let _guard = lock(&OUT_LOCK);
	let stdout = std::io::stdout();
	let mut out = stdout.lock();
	let _ = write_message(&mut out, value);
	let _ = out.flush();
}

/// The host's progress reporter: emits `plugin_progress` immediately and
/// reports cancellation to the plugin.
struct HostProgressReporter {
	label: String,
	message: String,
}

impl oak_plugin::progress::UiProgressReporter for HostProgressReporter {
	fn update(&mut self, progress: f64) -> bool {
		emit(
			&PluginProgressMsg {
				label: self.label.clone(),
				message: self.message.clone(),
				fraction: progress.clamp(0.0, 1.0),
			}
			.to_json(),
		);
		!OFX_CANCEL.load(Ordering::Relaxed)
	}

	fn end(&mut self) {
		emit(
			&PluginProgressMsg {
				label: self.label.clone(),
				message: self.message.clone(),
				fraction: 1.0,
			}
			.to_json(),
		);
	}
}

/// Build one progress reporter for `progressStart`: clears the sticky
/// cancel (the protocol's "a fresh render starts uncancelled") and emits
/// the fraction-0 start event. The reporter factory installed with the
/// progress suite calls this; tests call it directly.
fn host_progress_reporter(label: &str, message: &str) -> Box<dyn oak_plugin::progress::UiProgressReporter> {
	OFX_CANCEL.store(false, Ordering::Relaxed);
	emit(
		&PluginProgressMsg {
			label: label.to_string(),
			message: message.to_string(),
			fraction: 0.0,
		}
		.to_json(),
	);
	Box::new(HostProgressReporter {
		label: label.to_string(),
		message: message.to_string(),
	})
}

/// Install the reporter factory (`progressStart` → [`host_progress_reporter`]).
fn install_progress_factory() {
	oak_plugin::progress::set_reporter_factory(Some(Arc::new(host_progress_reporter)));
}

/// Read stdin on its own thread. `plugin_cancel` is handled inline (sets
/// the sticky flag) so a cancel is observed while a plugin render is in
/// flight; every other line goes to the main loop through the channel.
/// Dropping the sender on EOF closes the channel and ends the loop.
fn spawn_stdin_reader() -> mpsc::Receiver<String> {
	let (tx, rx) = mpsc::channel();
	std::thread::Builder::new()
		.name("oak-ofx-host-stdin".into())
		.spawn(move || {
			let stdin = std::io::stdin();
			let mut reader = BufReader::new(stdin.lock());
			let mut line = String::new();
			loop {
				line.clear();
				match reader.read_line(&mut line) {
					Ok(0) | Err(_) => break,
					Ok(_) => {}
				}
				if line.trim().is_empty() {
					continue;
				}
				if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
					if value.get("type").and_then(|t| t.as_str()) == Some(TYPE_PLUGIN_CANCEL) {
						OFX_CANCEL.store(true, Ordering::Relaxed);
						continue;
					}
				}
				if tx.send(line.clone()).is_err() {
					break;
				}
			}
		})
		.expect("spawn OFX host stdin reader");
	rx
}

/// The attached pools. The `SharedMemoryRegion`s must outlive the pools
/// (the pool views point into the mappings).
struct HostPools {
	_input_region: SharedMemoryRegion,
	input: FrameSlotPool,
	_output_region: SharedMemoryRegion,
	output: FrameSlotPool,
}

/// Attach the pools announced by a `handshake` (the worker's
/// `handle_handshake`, minus the render session).
fn attach_pools(msg: &serde_json::Value) -> Result<HostPools, String> {
	let hs: HandshakeMsg =
		serde_json::from_value(msg.clone()).map_err(|e| format!("invalid handshake: {e}"))?;
	if hs.shm_key.is_empty() || hs.output_slots <= 0 || hs.slot_data_bytes <= 0 {
		return Err("handshake missing output shared-memory geometry".to_string());
	}
	let output_bytes =
		FrameSlotPool::bytes_needed(hs.output_slots as u32, hs.slot_data_bytes as usize);
	let mut output_region = SharedMemoryRegion::new();
	if !output_region.open(&hs.shm_key, output_bytes, ShmMode::Attach) {
		return Err(format!(
			"failed to attach output shared memory: {}",
			output_region.error()
		));
	}
	// SAFETY: the mapping is live and sized for the pool.
	let output = unsafe { FrameSlotPool::attach(output_region.data()) };
	if !output.is_valid() {
		return Err("output shared memory does not contain a frame slot pool".to_string());
	}

	if hs.input_shm_key.is_empty() || hs.input_slots <= 0 || hs.input_slot_data_bytes <= 0 {
		return Err("handshake missing input shared-memory geometry".to_string());
	}
	let input_bytes =
		FrameSlotPool::bytes_needed(hs.input_slots as u32, hs.input_slot_data_bytes as usize);
	let mut input_region = SharedMemoryRegion::new();
	if !input_region.open(&hs.input_shm_key, input_bytes, ShmMode::Attach) {
		return Err(format!(
			"failed to attach input shared memory: {}",
			input_region.error()
		));
	}
	// SAFETY: the mapping is live and sized for the pool.
	let input = unsafe { FrameSlotPool::attach(input_region.data()) };
	if !input.is_valid() {
		return Err("input shared memory does not contain a frame slot pool".to_string());
	}

	Ok(HostPools {
		_input_region: input_region,
		input,
		_output_region: output_region,
		output,
	})
}

/// Consume one input frame (the producer is the main process).
fn read_input_frame(pool: &FrameSlotPool, slot: u32) -> Result<Frame, String> {
	let mut consumed = 0u32;
	// SAFETY: consumer side of the SPSC protocol, single-threaded host.
	if !unsafe { pool.consume(&mut consumed) } {
		return Err("input slot missing".to_string());
	}
	if consumed != slot {
		unsafe {
			pool.release(consumed);
		}
		return Err(format!(
			"input slot mismatch: expected {slot}, got {consumed}"
		));
	}
	// SAFETY: `slot` was just consumed; the meta POD is initialized by the
	// producer before publish.
	let meta = unsafe { &*pool.meta_const(slot) };
	if meta.width <= 0 || meta.height <= 0 || meta.data_size < 0 {
		unsafe {
			pool.release(slot);
		}
		return Err("input frame has invalid metadata".to_string());
	}
	let len = (meta.data_size as usize).min(pool.slot_data_bytes());
	// SAFETY: `len` is within the slot block.
	let data = unsafe { std::slice::from_raw_parts(pool.slot_data_const(slot), len).to_vec() };
	unsafe {
		pool.release(slot);
	}
	let mut frame = Frame::new();
	let mut pod = VideoParamsPod::default();
	pod.width = meta.width;
	pod.height = meta.height;
	pod.format = PixelFormat::F32 as i32;
	frame.set_video_params(pod);
	frame.data = data;
	Ok(frame)
}

/// Publish one output frame (the consumer is the main process).
fn write_output_frame(pool: &FrameSlotPool, frame: &Frame) -> Result<i32, String> {
	let mut slot = 0u32;
	// SAFETY: producer side of the SPSC protocol, single-threaded host.
	if !unsafe { pool.acquire(&mut slot) } {
		return Err("output pool is full".to_string());
	}
	let bytes = frame.data.len();
	if bytes > pool.slot_data_bytes() {
		unsafe {
			pool.release(slot);
		}
		return Err(format!(
			"output frame is {bytes} bytes, larger than the output slot ({})",
			pool.slot_data_bytes()
		));
	}
	// SAFETY: `slot` was just acquired; fill then publish.
	unsafe {
		std::ptr::copy_nonoverlapping(frame.data.as_ptr(), pool.slot_data(slot), bytes);
		let meta = &mut *pool.meta(slot);
		*meta = Default::default();
		meta.width = frame.width;
		meta.height = frame.height;
		meta.format = PixelFormat::F32 as i32;
		meta.channel_count = 4;
		meta.linesize = frame.linesize_bytes() as i32;
		meta.data_size = bytes as i32;
		meta.time_num = frame.timestamp.numerator();
		meta.time_den = frame.timestamp.denominator().max(1);
		if !pool.publish(slot) {
			pool.release(slot);
			return Err("output publish failed".to_string());
		}
	}
	Ok(slot as i32)
}

/// Render one `ofx_job` and return its `ofx_result` response.
fn handle_job(msg: serde_json::Value, pools: &HostPools) -> serde_json::Value {
	let job: OfxJobMsg = match serde_json::from_value(msg) {
		Ok(job) => job,
		Err(err) => return error_message(&format!("invalid ofx_job: {err}"), None),
	};
	let fail = |message: String| {
		OfxResultMsg {
			job: job.job,
			slot: -1,
			error: message,
		}
		.to_json()
	};

	let mut inputs = Vec::with_capacity(job.inputs.len());
	for input in &job.inputs {
		match read_input_frame(&pools.input, input.slot) {
			Ok(frame) => inputs.push((input.name.clone(), Texture::wrap_frame(frame))),
			Err(err) => return fail(err),
		}
	}
	let src = match job.src_slot {
		Some(slot) => match read_input_frame(&pools.input, slot) {
			Ok(frame) => Texture::wrap_frame(frame),
			Err(err) => return fail(err),
		},
		None => {
			// No explicit source: mirror the evaluator's fallback (the
			// declared effect input, else the first clip, else dummy).
			let named = inputs
				.iter()
				.find(|(name, _)| name == &job.effect_input_id)
				.or_else(|| inputs.first())
				.map(|(_, texture)| texture.clone());
			named.unwrap_or_else(Texture::dummy)
		}
	};

	let Some(factory) = eval::plugin_instance_factory() else {
		return fail("no plugin instance factory installed in the OFX host".to_string());
	};
	let Some(instance) = factory(&job.type_id) else {
		return fail(format!("unknown or unavailable OFX plugin: {}", job.type_id));
	};
	let Some(executor) = eval::plugin_executor() else {
		return fail("no plugin executor installed in the OFX host".to_string());
	};
	let spec = JobSpec::Plugin {
		instance,
		type_id: job.type_id.clone(),
		time: job.time,
		effect_input_id: if job.effect_input_id.is_empty() {
			None
		} else {
			Some(job.effect_input_id.clone())
		},
		inputs,
		values: job
			.values
			.iter()
			.map(|param| (param.input.clone(), param.value.to_node_value()))
			.collect(),
	};
	match executor(&PluginJobRequest { spec: &spec, src }) {
		Ok(texture) => match texture.to_frame() {
			Ok(frame) => match write_output_frame(&pools.output, &frame) {
				Ok(slot) => OfxResultMsg {
					job: job.job,
					slot,
					error: String::new(),
				}
				.to_json(),
				Err(err) => fail(err),
			},
			Err(err) => fail(format!("plugin output readback failed: {err:?}")),
		},
		Err(err) => fail(format!("plugin render failed: {err:?}")),
	}
}

/// Test-only crash hooks (deterministic crash/restart acceptance tests).
struct CrashHooks {
	/// Crash on every job.
	always: bool,
	/// Crash on the first job of each process unless the marker file
	/// exists (create it before crashing, so a respawned host renders).
	once_marker: Option<PathBuf>,
}

impl CrashHooks {
	fn from_args(args: &[String]) -> Self {
		let mut hooks = CrashHooks {
			always: false,
			once_marker: None,
		};
		let mut i = 0usize;
		while i < args.len() {
			match args[i].as_str() {
				"--ofx-crash-always" => hooks.always = true,
				"--ofx-crash-once" if i + 1 < args.len() => {
					hooks.once_marker = Some(PathBuf::from(&args[i + 1]));
					i += 1;
				}
				_ => {}
			}
			i += 1;
		}
		hooks
	}

	fn maybe_crash(&self) {
		if self.always {
			std::process::abort();
		}
		if let Some(path) = &self.once_marker {
			if !path.exists() {
				let _ = std::fs::write(path, b"ofx-host-crashed");
				std::process::abort();
			}
		}
	}
}

/// The `--ofx-host` main loop. Returns the process exit code.
pub fn ofx_host_main(args: &[String]) -> i32 {
	// The same plugin runtime the render workers install: the executor
	// (so `plugin_executor` is callable) and the identifier-keyed instance
	// factory (so jobs resolve their own instances).
	oak_plugin::node_factory::install_render_executor();
	if let Err(err) = oak_plugin::host::Host::global().cache.scan() {
		eprintln!("ofx-host: plugin scan failed: {err}");
	}
	install_progress_factory();
	let crash = CrashHooks::from_args(args);

	// stdin runs on its own thread (cancel must be observed mid-render);
	// the main loop consumes the forwarded control messages.
	let control = spawn_stdin_reader();
	let mut pools: Option<HostPools> = None;
	while let Ok(line) = control.recv() {
		let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
			emit(&error_message("invalid JSON message", None));
			continue;
		};
		match msg.get("type").and_then(|t| t.as_str()) {
			Some(TYPE_HANDSHAKE) => match attach_pools(&msg) {
				Ok(attached) => pools = Some(attached),
				Err(err) => emit(&error_message(&err, None)),
			},
			Some(TYPE_OFX_JOB) => match &pools {
				Some(pools) => {
					crash.maybe_crash();
					let response = handle_job(msg, pools);
					emit(&response);
				}
				None => emit(&error_message("ofx_job before handshake", None)),
			},
			Some(TYPE_PLUGIN_CANCEL) => OFX_CANCEL.store(true, Ordering::Relaxed),
			Some(TYPE_SHUTDOWN) => break,
			other => emit(&error_message(
				&format!(
					"unknown message type: {}",
					other.unwrap_or("<missing>")
				),
				None,
			)),
		}
	}
	0
}

#[cfg(test)]
mod tests {
	use super::*;
	use oak_core::backend::{BackendKind, GpuContextLike};
	use oak_core::error::{Error as CoreError, Result as CoreResult};
	use serde_json::{json, Value};
	use std::sync::atomic::AtomicU32;

	/// The process-global plugin/progress factories are shared with the
	/// `worker.rs` tests, so both modules serialize on this one lock.
	fn global_factory_lock() -> MutexGuard<'static, ()> {
		crate::worker::GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner())
	}

	/// A unique shm key + region holding an initialized frame slot pool.
	fn test_region(name: &str, slots: u32, slot_bytes: usize) -> (String, SharedMemoryRegion) {
		static COUNTER: AtomicU32 = AtomicU32::new(0);
		let n = COUNTER.fetch_add(1, Ordering::Relaxed);
		let key = SharedMemoryRegion::make_key(i64::from(std::process::id()), (n & 0x7FFF) as i32)
			+ &format!("-ofx-{name}");
		let bytes = FrameSlotPool::bytes_needed(slots, slot_bytes);
		let mut region = SharedMemoryRegion::new();
		assert!(
			region.open(&key, bytes, ShmMode::Create),
			"{}",
			region.error()
		);
		// SAFETY: live mapping sized by bytes_needed; the pool view is
		// discarded — peers re-attach through the region.
		let _ = unsafe { FrameSlotPool::create(region.data(), slots, slot_bytes) };
		(key, region)
	}

	/// A handshake for `attach_pools`.
	fn handshake_json(
		out_key: &str,
		out_slots: i32,
		out_bytes: i64,
		in_key: &str,
		in_slots: i32,
		in_bytes: i64,
	) -> Value {
		json!({
			"type": TYPE_HANDSHAKE,
			"shm_key": out_key,
			"output_slots": out_slots,
			"slot_data_bytes": out_bytes,
			"input_shm_key": in_key,
			"input_slots": in_slots,
			"input_slot_data_bytes": in_bytes,
		})
	}

	/// Attach a real output pool (and input pool when `in_slots > 0`),
	/// returning the pools plus the owner regions (kept alive by the caller).
	fn host_pools(
		out_slots: u32,
		out_bytes: usize,
		in_slots: u32,
		in_bytes: usize,
	) -> (HostPools, SharedMemoryRegion, SharedMemoryRegion) {
		let (out_key, out_region) = test_region("out", out_slots, out_bytes);
		let (in_key, in_region) = test_region("in", in_slots, in_bytes);
		let hs = handshake_json(
			&out_key,
			out_slots as i32,
			out_bytes as i64,
			&in_key,
			in_slots as i32,
			in_bytes as i64,
		);
		let pools = attach_pools(&hs).expect("handshake attaches the pools");
		(pools, out_region, in_region)
	}

	/// Fill and publish `slot` from the parent (producer) side. Pops past
	/// any other free slots (returning them to the ring) so callers do not
	/// need to know the free-ring order left by earlier publishes.
	unsafe fn publish_input(
		region: &SharedMemoryRegion,
		slot: u32,
		width: i32,
		height: i32,
		data_size: i32,
	) {
		// SAFETY: live region created by `test_region`.
		let pool = unsafe { FrameSlotPool::attach(region.data()) };
		let mut skipped: Vec<u32> = Vec::new();
		loop {
			let mut got = 0u32;
			assert!(unsafe { pool.acquire(&mut got) }, "free ring is seeded");
			if got == slot {
				break;
			}
			skipped.push(got);
		}
		// SAFETY: `slot` was acquired above; meta and data are live.
		unsafe {
			let meta = &mut *pool.meta(slot);
			*meta = Default::default();
			meta.width = width;
			meta.height = height;
			meta.format = PixelFormat::F32 as i32;
			meta.data_size = data_size;
			std::ptr::write_bytes(pool.slot_data(slot), 0x5A, pool.slot_data_bytes());
			for other in skipped {
				assert!(pool.release(other), "skipped slots go back");
			}
		}
		assert!(unsafe { pool.publish(slot) }, "ready ring has room");
	}

	/// A GPU context whose readback always fails, for the output-readback
	/// error path (a real GPU texture cannot be produced headless).
	struct FailingDownloadGpu;

	impl GpuContextLike for FailingDownloadGpu {
		fn kind(&self) -> BackendKind {
			BackendKind::Gl
		}
		fn destroy_texture(&self, _token: u64) {}
		fn upload(&self, _token: u64, _frame: &Frame) -> CoreResult<()> {
			Err(CoreError::Failed("fake upload".to_string()))
		}
		fn download(&self, _token: u64) -> CoreResult<Frame> {
			Err(CoreError::Failed("fake download".to_string()))
		}
		fn blit(
			&self,
			_src: u64,
			_dst: u64,
			_processor: Option<&oak_core::color::ColorProcessor>,
		) -> CoreResult<()> {
			Err(CoreError::Failed("fake blit".to_string()))
		}
	}

	#[test]
	fn cancel_flag_is_reset_by_progress_start() {
		let _guard = global_factory_lock();
		// Cancel semantics (protocol parity with the worker): a cancelled
		// reporter tells the plugin to abort at its next progressUpdate;
		// the next progressStart (the factory path below) clears the
		// sticky flag.
		OFX_CANCEL.store(true, Ordering::Relaxed);
		let mut reporter = host_progress_reporter("render", "msg");
		assert!(
			reporter.update(0.5),
			"the progressStart factory path reset the sticky cancel"
		);
		// A cancel after the start makes the next update answer false.
		OFX_CANCEL.store(true, Ordering::Relaxed);
		assert!(!reporter.update(0.6), "a cancelled reporter answers false");
		// progressEnd forwards completion (fraction 1.0); the emit is a
		// no-op under test, so this just exercises the reporter path.
		reporter.end();
		OFX_CANCEL.store(false, Ordering::Relaxed);
	}

	#[test]
	fn progress_factory_installs_a_working_reporter() {
		let _guard = global_factory_lock();
		OFX_CANCEL.store(false, Ordering::Relaxed);
		install_progress_factory();
		assert!(
			oak_plugin::progress::has_reporter_factory(),
			"the host factory must be installed for the plugin progress suite"
		);
		// Drive the installed factory through the progress suite
		// (progressStart -> host_progress_reporter, update, end).
		oak_plugin::suites::progress::set_current(Some(
			oak_plugin::progress::ProgressReporter::silent(),
		));
		let v2 = oak_plugin::suites::progress::suite_v2();
		let label = std::ffi::CString::new("render").unwrap();
		let message = std::ffi::CString::new("frame 1").unwrap();
		// SAFETY: the suite takes the null handle by contract; the strings
		// outlive the calls.
		unsafe {
			assert_eq!(
				(v2.start)(std::ptr::null_mut(), label.as_ptr(), message.as_ptr()),
				oak_plugin::suites::status::OK
			);
			assert_eq!(
				(v2.update)(std::ptr::null_mut(), 0.5),
				oak_plugin::suites::status::OK
			);
			assert_eq!(
				(v2.end)(std::ptr::null_mut()),
				oak_plugin::suites::status::OK
			);
		}
		oak_plugin::suites::progress::set_current(None);
	}

	#[test]
	fn lock_recovers_from_a_poisoned_mutex() {
		// `emit` (the other user) is a no-op under cfg(test); exercise the
		// poison-recovery helper directly so a panicking reporter can never
		// wedge the host's stdout lock.
		let _ = std::thread::spawn(|| {
			let _guard = OUT_LOCK.lock().unwrap();
			panic!("poison OUT_LOCK on purpose");
		})
		.join();
		let guard = lock(&OUT_LOCK);
		drop(guard);
	}

	#[test]
	fn crash_hooks_maybe_crash_skips_when_marker_exists() {
		let marker = std::env::temp_dir().join(format!(
			"oak-ofx-host-marker-{}.txt",
			std::process::id()
		));
		std::fs::write(&marker, b"already crashed").unwrap();
		let hooks = CrashHooks {
			always: false,
			once_marker: Some(marker.clone()),
		};
		// The marker exists: the hook must return without aborting.
		hooks.maybe_crash();
		let _ = std::fs::remove_file(&marker);

		// A dangling --ofx-crash-once without a value leaves no marker.
		let hooks = CrashHooks::from_args(&[
			"oak-worker".to_string(),
			"--ofx-host".to_string(),
			"--ofx-crash-once".to_string(),
		]);
		assert!(!hooks.always);
		assert!(hooks.once_marker.is_none());
	}

	#[test]
	fn attach_pools_rejects_bad_shapes_and_missing_geometry() {
		// Wrong field types: HandshakeMsg deserialization fails.
		let err = attach_pools(&json!({
			"type": TYPE_HANDSHAKE,
			"protocol_version": "one",
		}))
		.err().expect("wrong types are invalid");
		assert!(err.starts_with("invalid handshake: "), "{err}");

		// Empty geometry (all serde defaults).
		let err = attach_pools(&json!({ "type": TYPE_HANDSHAKE })).err().expect("empty handshake");
		assert_eq!(err, "handshake missing output shared-memory geometry");

		// Zero and negative geometry take the same branch.
		for (slots, bytes) in [(0, 16), (1, 0), (-2, 16), (1, -4)] {
			let err = attach_pools(&handshake_json("k", slots, bytes, "", 0, 0))
				.err().expect("degenerate output geometry");
			assert_eq!(err, "handshake missing output shared-memory geometry");
		}
	}

	#[test]
	fn attach_pools_attaches_real_segments_and_reports_failures() {
		// Success: both pools attach with the announced geometry.
		let (pools, _out, _in) = host_pools(2, 256, 3, 128);
		assert!(pools.output.is_valid());
		assert_eq!(pools.output.slot_count(), 2);
		assert_eq!(pools.output.slot_data_bytes(), 256);
		assert!(pools.input.is_valid());
		assert_eq!(pools.input.slot_count(), 3);
		assert_eq!(pools.input.slot_data_bytes(), 128);

		// Output segment missing.
		let missing = format!("olive-rw-{}-ofx-missing-out", std::process::id());
		let err = attach_pools(&handshake_json(&missing, 1, 16, "", 0, 0)).err().expect("no segment");
		assert!(err.starts_with("failed to attach output shared memory: "), "{err}");

		// Output segment present but not a pool (zeroed memory -> bad magic).
		let raw_key = format!("olive-rw-{}-ofx-raw-out", std::process::id());
		let bytes = FrameSlotPool::bytes_needed(1, 16);
		let mut raw = SharedMemoryRegion::new();
		assert!(raw.open(&raw_key, bytes, ShmMode::Create));
		let err = attach_pools(&handshake_json(&raw_key, 1, 16, "", 0, 0))
			.err().expect("zeroed output segment");
		assert_eq!(err, "output shared memory does not contain a frame slot pool");

		// Output is fine but the announced input geometry is incomplete.
		let (out_key, _out_region) = test_region("out-for-in", 1, 16);
		let err = attach_pools(&handshake_json(&out_key, 1, 16, "", 2, 0)).err().expect("input bytes");
		assert_eq!(err, "handshake missing input shared-memory geometry");
		let err = attach_pools(&handshake_json(&out_key, 1, 16, "", 2, 32))
			.err().expect("empty input key");
		assert_eq!(err, "handshake missing input shared-memory geometry");
		let err = attach_pools(&handshake_json(&out_key, 1, 16, "  ", 2, 32))
			.err().expect("blank input key attaches nothing");
		assert!(err.starts_with("failed to attach input shared memory: "), "{err}");

		// Input segment missing.
		let missing_in = format!("olive-rw-{}-ofx-missing-in", std::process::id());
		let err = attach_pools(&handshake_json(&out_key, 1, 16, &missing_in, 2, 32))
			.err().expect("no input segment");
		assert!(err.starts_with("failed to attach input shared memory: "), "{err}");

		// Input segment present but not a pool.
		let raw_key = format!("olive-rw-{}-ofx-raw-in", std::process::id());
		let mut raw_in = SharedMemoryRegion::new();
		assert!(raw_in.open(&raw_key, FrameSlotPool::bytes_needed(2, 32), ShmMode::Create));
		let err = attach_pools(&handshake_json(&out_key, 1, 16, &raw_key, 2, 32))
			.err().expect("zeroed input segment");
		assert_eq!(err, "input shared memory does not contain a frame slot pool");
	}

	#[test]
	fn read_input_frame_reports_missing_mismatch_and_invalid_meta() {
		let (out_key, _out_region) = test_region("read-out", 4, 64);
		let (in_key, in_region) = test_region("read-in", 2, 64);
		let hs = handshake_json(&out_key, 4, 64, &in_key, 2, 64);
		let pools = attach_pools(&hs).expect("attach");

		// Nothing published yet.
		let err = read_input_frame(&pools.input, 0).expect_err("empty ready ring");
		assert_eq!(err, "input slot missing");

		// Publish slot 1 while the job asks for slot 0: a protocol
		// violation that must release the consumed slot and fail.
		{
			// SAFETY: live region; parent (producer) side view.
			let parent = unsafe { FrameSlotPool::attach(in_region.data()) };
			let mut first = 0u32;
			let mut second = 0u32;
			assert!(unsafe { parent.acquire(&mut first) });
			assert!(unsafe { parent.acquire(&mut second) });
			assert_eq!((first, second), (0, 1));
			assert!(unsafe { parent.release(first) });
			unsafe {
				let meta = &mut *parent.meta(second);
				*meta = Default::default();
				meta.width = 4;
				meta.height = 4;
				meta.data_size = 64;
			}
			assert!(unsafe { parent.publish(second) });
		}
		let err = read_input_frame(&pools.input, 0).expect_err("slot mismatch");
		assert_eq!(err, "input slot mismatch: expected 0, got 1");

		// A published frame with degenerate metadata (zero width) is
		// rejected and released.
		unsafe { publish_input(&in_region, 0, 0, 4, 64) };
		let err = read_input_frame(&pools.input, 0).expect_err("zero width");
		assert_eq!(err, "input frame has invalid metadata");

		// A negative data size is equally invalid.
		unsafe { publish_input(&in_region, 0, 4, 4, -1) };
		let err = read_input_frame(&pools.input, 0).expect_err("negative size");
		assert_eq!(err, "input frame has invalid metadata");

		// A valid frame is copied out; an oversized `data_size` is clamped
		// to the slot capacity.
		unsafe { publish_input(&in_region, 0, 2, 2, 9999) };
		let frame = read_input_frame(&pools.input, 0).expect("valid input frame");
		assert_eq!((frame.width, frame.height), (2, 2));
		assert_eq!(frame.format, PixelFormat::F32);
		assert_eq!(frame.data.len(), 64, "clamped to the slot block");
		assert!(frame.data.iter().all(|&b| b == 0x5A));
	}

	#[test]
	fn write_output_frame_handles_full_oversize_publish_and_success() {
		let frame = cpu_frame(1, 1, 16);

		// No free slots at all: the pool view is attached directly, since
		// `attach_pools` rightly rejects a zero-slot handshake.
		let (_empty_key, empty_region) = test_region("write-empty", 0, 16);
		// SAFETY: live region created by `test_region`.
		let empty_pool = unsafe { FrameSlotPool::attach(empty_region.data()) };
		let err = write_output_frame(&empty_pool, &frame).expect_err("full pool");
		assert_eq!(err, "output pool is full");

		// `attach_pools` always requires the input geometry too, so
		// announce a (never used) one-slot input pool.
		let (pools, out_region, _in_region) = host_pools(1, 16, 1, 16);

		// A frame larger than the slot is rejected.
		let oversized = cpu_frame(2, 2, 64);
		let err = write_output_frame(&pools.output, &oversized).expect_err("oversize");
		assert!(
			err.starts_with("output frame is 64 bytes, larger than the output slot (16)"),
			"{err}"
		);

		// Success: the parent consumes the published slot and sees the meta.
		let slot = write_output_frame(&pools.output, &frame).expect("publish");
		assert_eq!(slot, 0);
		// SAFETY: live region; parent (drainer) side view.
		let parent = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut consumed = 0u32;
		assert!(unsafe { parent.consume(&mut consumed) });
		assert_eq!(consumed, 0);
		// SAFETY: `consumed` was just consumed.
		let meta = unsafe { &*parent.meta_const(consumed) };
		assert_eq!((meta.width, meta.height), (1, 1));
		assert_eq!(meta.format, PixelFormat::F32 as i32);
		assert_eq!(meta.channel_count, 4);
		assert_eq!(meta.linesize, 16);
		assert_eq!(meta.data_size, 16);
		unsafe { parent.release(consumed) };

		// Ready ring full: a duplicate publish fills the single-slot ring,
		// the slot is recycled through the free ring, and the next publish
		// fails.
		unsafe {
			let mut slot0 = 0u32;
			assert!(pools.output.acquire(&mut slot0));
			assert!(pools.output.publish(slot0));
			assert!(pools.output.release(slot0));
		}
		let err = write_output_frame(&pools.output, &frame).expect_err("ready ring full");
		assert_eq!(err, "output publish failed");
	}

	fn cpu_frame(width: i32, height: i32, bytes: usize) -> Frame {
		let pod = VideoParamsPod {
			width,
			height,
			format: PixelFormat::F32 as i32,
			..Default::default()
		};
		let mut frame = Frame::new();
		frame.set_video_params(pod);
		frame.data = vec![0x7F; bytes];
		frame
	}

	#[test]
	fn handle_job_rejects_malformed_and_unbacked_jobs() {
		let (pools, _out, _in) = host_pools(2, 64, 2, 64);

		// Wrong wire types: OfxJobMsg deserialization fails.
		let resp = handle_job(json!({ "job": "five" }), &pools);
		assert_eq!(resp["type"], crate::ipc::TYPE_ERROR);
		assert!(
			resp["message"]
				.as_str()
				.unwrap()
				.starts_with("invalid ofx_job: "),
			"{resp}"
		);

		// A declared input with nothing published fails with the job id
		// echoed and slot -1.
		let resp = handle_job(
			json!({
				"job": 9,
				"type_id": "org.oak.test",
				"inputs": [{ "name": "Source", "slot": 0 }],
			}),
			&pools,
		);
		assert_eq!(resp["type"], crate::ipc::TYPE_OFX_RESULT);
		assert_eq!(resp["job"], 9);
		assert_eq!(resp["slot"], -1);
		assert_eq!(resp["error"], "input slot missing");

		// Same for a main-source slot that was never published.
		let resp = handle_job(
			json!({ "job": 10, "type_id": "org.oak.test", "src_slot": 0 }),
			&pools,
		);
		assert_eq!(resp["job"], 10);
		assert_eq!(resp["error"], "input slot missing");
	}

	#[test]
	fn handle_job_reports_factory_and_executor_failures() {
		let _guard = global_factory_lock();
		let (pools, _out, _in) = host_pools(2, 64, 2, 64);
		let job = json!({ "job": 11, "type_id": "org.oak.test" });

		// No instance factory installed: the host cannot resolve anything.
		eval::set_plugin_instance_factory(None);
		eval::set_plugin_executor(None);
		let resp = handle_job(job.clone(), &pools);
		assert_eq!(
			resp["error"],
			"no plugin instance factory installed in the OFX host"
		);
		assert_eq!(resp["slot"], -1);

		// Factory resolves nothing: unknown plugin.
		eval::set_plugin_instance_factory(Some(Arc::new(|_type_id: &str| None)));
		let resp = handle_job(job.clone(), &pools);
		assert_eq!(
			resp["error"],
			"unknown or unavailable OFX plugin: org.oak.test"
		);

		// Instance resolved but no executor wired in.
		eval::set_plugin_instance_factory(Some(Arc::new(|_type_id: &str| Some(7))));
		eval::set_plugin_executor(None);
		let resp = handle_job(job.clone(), &pools);
		assert_eq!(
			resp["error"],
			"no plugin executor installed in the OFX host"
		);

		eval::set_plugin_instance_factory(None);
	}

	#[test]
	fn handle_job_renders_through_the_executor_and_reports_errors() {
		let _guard = global_factory_lock();
		let (pools, _out, in_region) = host_pools(2, 64, 4, 64);
		// SAFETY: live region created by host_pools.
		unsafe { publish_input(&in_region, 0, 1, 1, 16) };

		// The custom executor asserts the resolved spec and returns a
		// 1x1 frame; the host publishes it into the output pool.
		let exec: Arc<eval::PluginExecutor> = Arc::new(|req: &PluginJobRequest<'_>| {
			match req.spec {
				JobSpec::Plugin {
					instance,
					type_id,
					time,
					effect_input_id,
					inputs,
					values,
				} => {
					assert_eq!(*instance, 7);
					assert_eq!(type_id, "org.oak.test");
					assert!((*time - 0.5).abs() < 1e-9);
					assert_eq!(effect_input_id.as_deref(), Some("Source"));
					assert_eq!(inputs.len(), 1);
					assert_eq!(inputs[0].0, "Source");
					assert_eq!(values.len(), 1);
					assert_eq!(values[0].0, "brightness");
					assert_eq!(values[0].1, oak_node::value::NodeValue::Float(0.5));
				}
				other => panic!("expected a plugin spec, got {other:?}"),
			}
			let frame = eval::generate_frame(oak_core::Rational::new(0, 1), (1, 1), PixelFormat::F32)
				.expect("generate");
			Ok(Texture::wrap_frame(frame))
		});
		let factory: Arc<eval::PluginInstanceFactory> = Arc::new(|_type_id: &str| Some(7));
		eval::set_plugin_instance_factory(Some(factory));
		eval::set_plugin_executor(Some(exec));

		let resp = handle_job(
			json!({
				"job": 21,
				"type_id": "org.oak.test",
				"time": 0.5,
				"effect_input_id": "Source",
				"inputs": [{ "name": "Source", "slot": 0 }],
				"values": [{ "input": "brightness", "value": { "t": "float", "v": 0.5 } }],
			}),
			&pools,
		);
		assert_eq!(resp["type"], crate::ipc::TYPE_OFX_RESULT);
		assert_eq!(resp["job"], 21);
		assert!(resp["slot"].as_i64().unwrap() >= 0, "{resp}");
		assert_eq!(resp["error"], "");

		// Executor failure is reported as a job failure; this one arrives
		// through the explicit `src_slot` path.
		// SAFETY: live region created by host_pools.
		unsafe { publish_input(&in_region, 0, 1, 1, 16) };
		let failing: Arc<eval::PluginExecutor> =
			Arc::new(|_req: &PluginJobRequest<'_>| Err(oak_core::error::Error::Failed("boom".into())));
		eval::set_plugin_executor(Some(failing));
		let resp = handle_job(
			json!({ "job": 22, "type_id": "org.oak.test", "src_slot": 0 }),
			&pools,
		);
		assert_eq!(resp["job"], 22);
		assert!(
			resp["error"]
				.as_str()
				.unwrap()
				.starts_with("plugin render failed: "),
			"{resp}"
		);

		// A texture that cannot be read back fails before publishing.
		let gpu_returning: Arc<eval::PluginExecutor> = Arc::new(|_req: &PluginJobRequest<'_>| {
			Ok(Texture::gpu(
				Arc::new(FailingDownloadGpu),
				1,
				1,
				1,
				PixelFormat::F32,
			))
		});
		eval::set_plugin_executor(Some(gpu_returning));
		let resp = handle_job(
			json!({ "job": 23, "type_id": "org.oak.test" }),
			&pools,
		);
		assert_eq!(resp["job"], 23);
		assert!(
			resp["error"]
				.as_str()
				.unwrap()
				.starts_with("plugin output readback failed: "),
			"{resp}"
		);

		eval::set_plugin_instance_factory(None);
		eval::set_plugin_executor(None);
	}

	#[test]
	fn handle_job_falls_back_to_first_input_then_dummy_source() {
		let _guard = global_factory_lock();
		let (pools, _out, in_region) = host_pools(2, 64, 4, 64);
		// SAFETY: live region created by host_pools.
		unsafe { publish_input(&in_region, 0, 1, 1, 16) };

		// Capture the src the fallback picked. Values stay empty: the
		// resolver runs before the executor.
		let kinds: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
		let record = kinds.clone();
		let exec: Arc<eval::PluginExecutor> = Arc::new(move |req: &PluginJobRequest<'_>| {
			record
				.lock()
				.unwrap_or_else(|e| e.into_inner())
				.push(if req.src.is_dummy() { "dummy" } else { "frame" });
			let frame = eval::generate_frame(oak_core::Rational::new(0, 1), (1, 1), PixelFormat::F32)
				.expect("generate");
			Ok(Texture::wrap_frame(frame))
		});
		let factory: Arc<eval::PluginInstanceFactory> = Arc::new(|_type_id: &str| Some(1));
		eval::set_plugin_instance_factory(Some(factory));
		eval::set_plugin_executor(Some(exec));

		// The declared effect input is absent from `inputs`, so the first
		// clip is used instead.
		let resp = handle_job(
			json!({
				"job": 31,
				"type_id": "org.oak.test",
				"effect_input_id": "Source",
				"inputs": [{ "name": "Extra", "slot": 0 }],
			}),
			&pools,
		);
		assert_eq!(resp["error"], "", "{resp}");

		// No clips and no explicit src: the dummy texture is used.
		let resp = handle_job(
			json!({ "job": 32, "type_id": "org.oak.test", "effect_input_id": "Source" }),
			&pools,
		);
		assert_eq!(resp["error"], "", "{resp}");

		let seen = kinds.lock().unwrap_or_else(|e| e.into_inner()).clone();
		assert_eq!(seen, vec!["frame", "dummy"]);

		eval::set_plugin_instance_factory(None);
		eval::set_plugin_executor(None);
	}

	#[test]
	fn crash_hooks_parse_args() {
		let hooks = CrashHooks::from_args(&[
			"oak-worker".to_string(),
			"--ofx-host".to_string(),
			"--ofx-crash-once".to_string(),
			"/tmp/marker".to_string(),
		]);
		assert!(!hooks.always);
		assert_eq!(hooks.once_marker.as_deref(), Some(std::path::Path::new("/tmp/marker")));

		let hooks = CrashHooks::from_args(&["oak-worker".to_string(), "--ofx-crash-always".to_string()]);
		assert!(hooks.always);
		assert!(hooks.once_marker.is_none());

		// Unknown flags and a missing option value are ignored.
		let hooks = CrashHooks::from_args(&[
			"oak-worker".to_string(),
			"--ofx-host".to_string(),
			"--not-a-flag".to_string(),
			"--ofx-crash-once".to_string(),
		]);
		assert!(!hooks.always);
		assert!(hooks.once_marker.is_none());
	}

	// ---- M16 R2 coverage additions ----------------------------------------

	/// Every hook of the fake failing GPU context is callable and reports
	/// the failure (the executor readback path uses `download`; the other
	/// hooks document the trait contract).
	#[test]
	fn failing_download_gpu_hooks_return_errors() {
		let gpu = FailingDownloadGpu;
		assert_eq!(gpu.kind(), BackendKind::Gl);
		gpu.destroy_texture(1);
		let frame = cpu_frame(1, 1, 4);
		assert!(gpu.upload(1, &frame).is_err(), "fake upload always fails");
		assert!(gpu.download(1).is_err(), "fake download always fails");
		assert!(gpu.blit(1, 2, None).is_err(), "fake blit always fails");
	}

	/// A successful plugin render whose output pool has no free slot fails
	/// the job with "output pool is full" (the acquired-but-unpublished
	/// slot is not leaked into the ready ring).
	#[test]
	fn handle_job_reports_output_pool_full() {
		let _guard = global_factory_lock();
		let (pools, _out, _in) = host_pools(1, 16, 1, 16);
		// Drain the only free output slot through the producer side, so the
		// host's `write_output_frame` cannot acquire one.
		let mut drained = 0u32;
		// SAFETY: live attached pools; the test owns both sides and is
		// single-threaded.
		assert!(unsafe { pools.output.acquire(&mut drained) });
		assert_eq!(drained, 0);

		let exec: Arc<eval::PluginExecutor> = Arc::new(|_req: &PluginJobRequest<'_>| {
			let frame =
				eval::generate_frame(oak_core::Rational::new(0, 1), (1, 1), PixelFormat::F32)
					.expect("generate");
			Ok(Texture::wrap_frame(frame))
		});
		eval::set_plugin_instance_factory(Some(Arc::new(|_type_id: &str| Some(1))));
		eval::set_plugin_executor(Some(exec));
		let resp = handle_job(json!({ "job": 41, "type_id": "org.oak.test" }), &pools);
		eval::set_plugin_instance_factory(None);
		eval::set_plugin_executor(None);

		assert_eq!(resp["type"], crate::ipc::TYPE_OFX_RESULT);
		assert_eq!(resp["job"], 41);
		assert_eq!(resp["slot"], -1);
		assert_eq!(resp["error"], "output pool is full");
		// Nothing was published.
		assert!(!unsafe { pools.output.consume(&mut drained) });
	}
}

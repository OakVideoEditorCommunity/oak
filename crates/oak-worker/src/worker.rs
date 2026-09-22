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

//! The render worker runtime — the Rust port of
//! `engine/src/capi/worker.cpp`, owned by the oak-worker binary since
//! M14 R2 (the facade keeps its own copy for the frozen
//! `oakengine_worker_*` C ABI).
//!
//!   - **Backend selection.** [`Renderer::create`] initializes the render
//!     backend through the oakrender crate's direct Rust API
//!     ([`oak_core::backend::DisplayRenderer`]), falling back to the
//!     direct OpenGL renderer exactly like the C++ `create_renderer()`
//!     chain. The headless `"cpu"` backend (M15 S1) skips the renderer
//!     entirely — the render path is CPU evaluation + decode, driven
//!     through `render_batch`.
//!   - **The session.** [`WorkerSession`] holds the renderer, the
//!     loaded node graph, the shared-memory frame-slot pools
//!     ([`crate::ipc::FrameSlotPool`]) and the shutdown flag, and answers
//!     one NDJSON control message at a time.
//!   - **The main loop.** [`worker_main`] creates the session, loads the
//!     runtime config (including the oakplugin render executor), writes
//!     the startup handshake, and serves the stdin/stdout NDJSON loop
//!     until a `shutdown` message or EOF. The worker is single-threaded
//!     by design — there is no render thread: a `render_batch` renders
//!     its tickets synchronously on this loop thread, and parallelism
//!     comes from the main process's worker pool
//!     (`oak_render::procpool`), not from threads here (see "Where the
//!     rendering happens" in this crate's README).
//!
//! Real rendering landed in M15 S1: `load_graph` deserializes the graph
//! snapshot file (oaknode project XML, with the minimal
//! `{"project_copy":N}` payload fallback); `render_frame` and
//! `render_batch` render through [`oak_render::eval`] (generated frames,
//! footage decode, montage compositing) directly into the main-assigned
//! shm slots and publish `frame_ready` / `frame_failed` (protocol v2).

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use oak_core::backend::{BackendKind, DisplayRenderer};
use oak_core::{PixelFormat, Rational};
use oak_render::eval;
use oak_render::ticket::{AdjustmentSpan, AudioTicketParams, MontageClip, VideoTicketParams};

use crate::framecache::FrameCache;
use crate::ipc::{
	error_message, write_message, AudioTicketSpec, BatchTicketSpec, FrameSlotMeta, FrameSlotPool,
	HandshakeMsg, LoadGraphMsg, PluginProgressMsg, RenderAudioBatchMsg, RenderBatchMsg,
	RenderFrameMsg, SharedMemoryRegion, ShmMode, SLOT_FORMAT_AUDIO_F32, SLOT_FORMAT_BGRA8,
	TYPE_CANCEL, TYPE_HANDSHAKE, TYPE_LOAD_GRAPH, TYPE_PLUGIN_CANCEL, TYPE_RENDER_AUDIO_BATCH,
	TYPE_RENDER_BATCH, TYPE_RENDER_FRAME, TYPE_SHUTDOWN,
};
use crate::{log_error, PROTOCOL_VERSION};

/// Serializes unit tests that install or drive the process-global
/// plugin/progress factories (the `ofx_host` tests share this lock: the
/// worker's and the host's reporter factories are process-wide).
#[cfg(test)]
pub(crate) static GLOBAL_FACTORY_TEST_LOCK: Mutex<()> = Mutex::new(());

/// A loaded graph snapshot (M15 S1): the snapshot file path plus what it
/// deserialized into — a full oaknode project, or only the copied-project
/// identity (the minimal `{"project_copy":N}` payload the
/// [`oak_render::worker::GraphSnapshotStore`] wrote before M16 S1 shipped
/// full graph snapshots).
struct LoadedGraph {
	/// Snapshot file path (S2: graph_update diffing is path-based).
	#[allow(dead_code)]
	path: String,
	/// The deserialized project (M16 S1: node-graph tickets render the
	/// viewer's frame from here; montage/footage/generate tickets use the
	/// loaded context too).
	project: Option<Arc<Mutex<oak_node::project::Project>>>,
	/// The loaded snapshot's owning project uuid; `None` for the
	/// identity-only legacy payload. Graph-mode tickets (which carry their
	/// owning project's uuid) only render from this graph when it matches —
	/// a stale snapshot from a different project must not answer foreign
	/// viewer identities (M16 S1 cross-process snapshot race).
	project_uuid: Option<String>,
	/// Source-identity -> loaded-id map (see
	/// [`oak_node::serializer::load_with_id_map`]): tickets carry viewer
	/// identities from the *saved* project, while the loaded graph's arena
	/// slots differ (deleted slots shift every later node).
	id_map: std::collections::HashMap<u64, oak_node::id::NodeId>,
	project_copy: u64,
}

// ---------------------------------------------------------------------------
// Worker-side plugin-progress forwarding
// ---------------------------------------------------------------------------
//
// OFX plugin rendering happens in this process (crash isolation), so the
// main process's inline progress reporter is not in effect here. The
// worker installs its own progress reporter factory (see
// [`install_worker_progress_factory`]) whose reporters push `plugin_progress`
// NDJSON events into [`WORKER_PROGRESS_EVENTS`]; the main loop drains the
// buffer after each control message ([`flush_worker_progress`]). Plugin
// renders are synchronous on the loop thread, so the buffer only ever
// mutates there — a plain `Mutex` suffices.
//
// Cancel: the main process broadcasts `plugin_cancel` (the progress
// dialog's Cancel button); the worker sets [`WORKER_PLUGIN_CANCEL`] and
// every live reporter answers false (the plugin aborts at its next
// progressUpdate). Mirrors the main-process reporter factory: a fresh
// render (progressStart) resets the sticky flag. Because the worker
// processes control messages between batches, an in-flight frame
// completes before the cancel is observed (batch granularity); the main
// process stops the render loop separately (export cancel atom / preview
// window invalidation).

/// The worker's sticky plugin-cancel flag (set by the `plugin_cancel`
/// control message, read by every live progress reporter).
static WORKER_PLUGIN_CANCEL: AtomicBool = AtomicBool::new(false);

/// Buffered `plugin_progress` events awaiting the next flush.
static WORKER_PROGRESS_EVENTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Queue one `plugin_progress` NDJSON event for the main loop to flush.
fn push_worker_progress(fraction: f64, label: &str, message: &str) {
	let event = PluginProgressMsg {
		label: label.to_string(),
		message: message.to_string(),
		fraction,
	}
	.to_json();
	WORKER_PROGRESS_EVENTS
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.push(event);
}

/// Write every buffered progress event to `out` (called by the main loop
/// after each control message; `out` is the NDJSON stdout writer).
fn flush_worker_progress(out: &mut impl Write) {
	let events = std::mem::take(
		&mut *WORKER_PROGRESS_EVENTS
			.lock()
			.unwrap_or_else(|e| e.into_inner()),
	);
	for event in events {
		if write_message(out, &event).is_err() {
			break;
		}
	}
	let _ = out.flush();
}

/// The worker-side `UiProgressReporter`: forwards (label, message,
/// fraction) to the main process and honours the sticky cancel flag.
struct WorkerProgressReporter {
	label: String,
	message: String,
}

impl oak_plugin::progress::UiProgressReporter for WorkerProgressReporter {
	fn update(&mut self, progress: f64) -> bool {
		push_worker_progress(progress, &self.label, &self.message);
		!WORKER_PLUGIN_CANCEL.load(Ordering::Relaxed)
	}

	fn end(&mut self) {
		// progressEnd: forward completion (fraction 1.0) so the app closes
		// the progress dialog without waiting for a 1.0 update.
		push_worker_progress(1.0, &self.label, &self.message);
	}
}

/// Install the worker-side progress reporter factory. Called from
/// [`WorkerSession::initialize_runtime`]; mirrors the main-process factory
/// (a fresh progressStart resets the sticky cancel flag).
fn install_worker_progress_factory() {
	oak_plugin::progress::set_reporter_factory(Some(Arc::new(|label, message| {
		// A fresh render begins: reset the sticky cancel flag.
		WORKER_PLUGIN_CANCEL.store(false, Ordering::Relaxed);
		push_worker_progress(0.0, label, message);
		Box::new(WorkerProgressReporter {
			label: label.to_string(),
			message: message.to_string(),
		})
	})));
}

// ---------------------------------------------------------------------------
// Renderer (backend selection)
// ---------------------------------------------------------------------------

/// Whether `backend` requests no renderer (worker.cpp
/// `backend_requests_no_renderer()`: NULL, "" and "none").
pub fn is_no_backend(backend: &str) -> bool {
	backend.is_empty() || backend.eq_ignore_ascii_case("none")
}

/// Whether `backend` is the M15 headless CPU render mode: like "none"
/// (no GPU renderer) but the session stays fully operational — frames
/// render through the CPU evaluation path ([`oak_render::eval`]).
pub fn is_cpu_backend(backend: &str) -> bool {
	backend.eq_ignore_ascii_case("cpu")
}

/// A live, initialized oakrender display renderer (destroyed on drop).
pub struct Renderer {
	/// The oakrender crate's value-typed display renderer (single-lib
	/// unification; the CHandle-based C ABI is deleted).
	inner: DisplayRenderer,
}

impl Renderer {
	/// Create and initialize a renderer through the oakrender crate's
	/// direct Rust API, trying the named dynamic backend first and falling
	/// back to the direct OpenGL renderer — the exact fallback chain of
	/// worker.cpp `create_renderer()`.
	pub fn create(backend: &str) -> Result<Renderer, String> {
		match Self::create_dynamic(backend) {
			Ok(r) => Ok(r),
			Err(first) => {
				log_error(&format!(
					"failed to initialize dynamic {backend} backend: {first}; falling back to direct OpenGL renderer"
				));
				Self::create_opengl().map_err(|second| {
					format!("{first}; direct OpenGL fallback also failed: {second}")
				})
			}
		}
	}

	/// Try the named dynamic backend (`DisplayRenderer::new` +
	/// `init`, the single-lib equivalent of
	/// `oakrender_display_renderer_create_dynamic` + `_init`).
	fn create_dynamic(backend: &str) -> Result<Renderer, String> {
		let renderer = DisplayRenderer::new(BackendKind::from_config_string(backend));
		Self::init_inner(renderer, &format!("dynamic {backend}"))
	}

	/// Fall back to the direct OpenGL renderer.
	fn create_opengl() -> Result<Renderer, String> {
		let renderer = DisplayRenderer::new(BackendKind::Gl);
		Self::init_inner(renderer, "direct OpenGL")
	}

	/// Initialize a freshly created renderer.
	fn init_inner(mut renderer: DisplayRenderer, what: &str) -> Result<Renderer, String> {
		// NULL gl_context makes the backend use its default device/context
		// path.
		if let Err(e) = renderer.init(std::ptr::null_mut()) {
			return Err(format!("failed to initialize {what} renderer ({e})"));
		}
		Ok(Renderer { inner: renderer })
	}

	/// 1 when the renderer is OpenGL-based (the C++ worker uses the GL
	/// context to announce the negotiated GL version in the handshake).
	///
	/// Not called yet: the oakrender module exposes no GL context
	/// version, so the startup handshake omits `gl_major`/`gl_minor`.
	#[allow(dead_code)]
	pub fn is_open_gl(&self) -> bool {
		self.inner.is_open_gl()
	}
}

// ---------------------------------------------------------------------------
// WorkerSession
// ---------------------------------------------------------------------------

/// The worker-side session state machine — the Rust mirror of
/// `OakWorkerSession` in worker.cpp. Holds the renderer, the attached
/// shared-memory frame-slot pools and the shutdown flag, and answers one
/// NDJSON control message at a time.
pub struct WorkerSession {
	renderer: Option<Renderer>,
	shutdown_requested: bool,
	runtime_initialized: bool,
	output_region: Option<SharedMemoryRegion>,
	output_pool: Option<FrameSlotPool>,
	input_region: Option<SharedMemoryRegion>,
	input_pool: Option<FrameSlotPool>,
	graph: Option<LoadedGraph>,
	/// Reusable F32 staging buffer (BGRA8 slot conversion; the F32
	/// pipeline renders there before the end-of-pipe format convert).
	f32_scratch: Vec<u8>,
	/// LRU byte-budgeted memo of rendered F32 frames, keyed by the
	/// render-deterministic ticket-spec subset (M16 S2; see
	/// [`crate::framecache`]).
	frame_cache: FrameCache,
}

impl WorkerSession {
	/// Create a session for `backend`, mirroring
	/// `oakengine_worker_session_create()`: "none"/"" skips renderer
	/// creation, anything else initializes the render backend through the
	/// oakrender crate's direct Rust API (dynamic -> OpenGL fallback).
	/// The M15 `"cpu"` backend is the headless render mode: no renderer,
	/// CPU evaluation + decode via [`oak_render::eval`]. M16 S1: a failed
	/// renderer init for any other backend (e.g. "auto" on a GPU-less
	/// host) is tolerated — the session continues headless and tickets
	/// evaluate through the CPU path.
	pub fn create(backend: &str) -> Result<WorkerSession, String> {
		let renderer = match backend {
			b if is_no_backend(b) || is_cpu_backend(b) => None,
			b => match Renderer::create(b) {
				Ok(r) => Some(r),
				Err(e) => {
					log_error(&format!(
						"session: {b} renderer init failed ({e}); continuing headless (CPU eval path)"
					));
					None
				}
			},
		};
		Ok(WorkerSession {
			renderer,
			shutdown_requested: false,
			runtime_initialized: false,
			output_region: None,
			output_pool: None,
			input_region: None,
			input_pool: None,
			graph: None,
			f32_scratch: Vec::new(),
			frame_cache: FrameCache::new(),
		})
	}

	/// 1 when the session holds a successfully initialized render backend.
	pub fn has_renderer(&self) -> bool {
		self.renderer.is_some()
	}

	/// 1 once a shutdown control message has been received.
	pub fn shutdown_requested(&self) -> bool {
		self.shutdown_requested
	}

	/// Load the runtime services the session depends on — the Rust analog
	/// of the C++ `initialize_runtime()`. Of the C++ list (config, node
	/// factory, color manager, frame/disk managers, project serializer)
	/// only the color-manager default config has a Rust backing linked into
	/// the worker binary; the rest are logged and skipped. Always returns
	/// true (the C++ returns true unconditionally).
	pub fn initialize_runtime(&mut self) -> bool {
		if self.runtime_initialized {
			return true;
		}
		log_error("runtime: loading color-manager default config");
		if let Err(e) = oak_core::color::set_up_default_config() {
			log_error(&format!(
				"runtime: color-manager default config failed ({e}); continuing"
			));
		}
		// The text nodes rasterize through the textbackend hooks: the pool
		// renders text clips too, so every worker installs the same
		// cosmic-text engine as the UI process (installing it only there
		// left text invisible in previews — the hooks are per-process).
		log_error("runtime: installing text layout backends");
		oak_render::textengine::install();
		// M15 S1: the plugin execution stack lives in the worker process
		// (OFX crashes take down this process, not the editor — design
		// §3.6). oakplugin installs its render driver into the oakrender
		// executor slot.
		log_error("runtime: installing oakplugin render executor");
		oak_plugin::node_factory::install_render_executor();
		// M15 S2: graphs carrying OFX plugin nodes deserialize/evaluate in
		// the worker too, so the per-process node factory must register the
		// discovered plugins exactly like the main process. A failed scan is
		// non-fatal: the worker stays up for plugin-free graphs.
		log_error("runtime: scanning and registering OFX plugins");
		if let Err(e) = oak_plugin::host::Host::global().cache.scan() {
			log_error(&format!(
				"runtime: OFX plugin scan failed ({e}); continuing"
			));
		}
		let discovered = oak_plugin::host::Host::global().cache.count();
		let registered = oak_plugin::node_factory::register_plugin_nodes();
		log_error(&format!(
			"runtime: discovered {} OFX plugin(s), registered {} node type(s)",
			discovered,
			registered.len()
		));
		// Worker-side plugin progress forwarding (see the module docs): the
		// plugin progress suite runs in this process, so progress events
		// must cross the IPC boundary to reach the app's progress dialog.
		log_error("runtime: installing worker plugin-progress reporter factory");
		install_worker_progress_factory();
		log_error(
			"runtime: config / frame manager / disk manager / project \
			 serializer have no Rust backing in the worker binary; skipped",
		);
		self.runtime_initialized = true;
		true
	}

	/// The startup handshake the worker sends to its parent
	/// (`worker.cpp startup_handshake()`): protocol version 1 and empty
	/// shared-memory geometry — the parent creates the segments and
	/// announces their geometry in its handshake reply.
	///
	/// Deviation from the C++: `gl_major`/`gl_minor` are omitted because
	/// the oakrender module exposes no GL context version.
	pub fn startup_handshake(&self) -> Value {
		HandshakeMsg {
			protocol_version: PROTOCOL_VERSION,
			shm_key: String::new(),
			input_shm_key: String::new(),
			input_slots: 0,
			output_slots: 0,
			slot_data_bytes: 0,
			input_slot_data_bytes: 0,
		}
		.to_json()
	}

	/// Handle one complete NDJSON control line and produce the response, if
	/// any — the port of worker.cpp `handle()`. A malformed line yields an
	/// error response (the loop continues), never a failure.
	pub fn handle_line(&mut self, line: &str) -> Option<Value> {
		let msg: Value = match serde_json::from_str::<Value>(line) {
			Ok(v) if v.is_object() => v,
			_ => return Some(error_message("malformed control message", None)),
		};
		let typ = msg.get("type").and_then(Value::as_str).unwrap_or("");
		match typ {
			TYPE_HANDSHAKE => self.handle_handshake(&msg),
			TYPE_LOAD_GRAPH => self.handle_load_graph(&msg),
			TYPE_RENDER_FRAME => self.handle_render_frame(&msg),
			// cancel: the worker does synchronous single-frame work
			// (nothing in flight), so a cancel produces no response.
			TYPE_CANCEL => None,
			// plugin_cancel: the user cancelled the plugin render; sticky
			// until the next progressStart resets it (main-process
			// semantics).
			TYPE_PLUGIN_CANCEL => {
				WORKER_PLUGIN_CANCEL.store(true, Ordering::Relaxed);
				None
			}
			TYPE_SHUTDOWN => {
				self.shutdown_requested = true;
				None
			}
			other => Some(error_message(
				&format!("unknown message type: {other}"),
				None,
			)),
		}
	}

	/// `handshake`: validate and attach the shared-memory frame-slot pools
	/// — the real port of worker.cpp `attach_output_pool()`.
	fn handle_handshake(&mut self, msg: &Value) -> Option<Value> {
		let hs: HandshakeMsg = match serde_json::from_value(msg.clone()) {
			Ok(hs) => hs,
			Err(_) => return Some(error_message("invalid handshake message", None)),
		};
		if hs.protocol_version != PROTOCOL_VERSION {
			return Some(error_message(
				&format!("unsupported protocol version {}", hs.protocol_version),
				None,
			));
		}
		if hs.shm_key.is_empty() || hs.output_slots <= 0 || hs.slot_data_bytes <= 0 {
			return Some(error_message(
				"handshake missing output shared-memory geometry",
				None,
			));
		}

		// A re-handshake replaces the pools (worker.cpp resets the input
		// pool before attaching the output).
		self.input_pool = None;
		self.input_region = None;
		self.output_pool = None;
		self.output_region = None;

		let bytes =
			FrameSlotPool::bytes_needed(hs.output_slots as u32, hs.slot_data_bytes as usize);
		let mut output_region = SharedMemoryRegion::new();
		if !output_region.open(&hs.shm_key, bytes, ShmMode::Attach) {
			return Some(error_message(
				&format!("failed to attach shared memory: {}", output_region.error()),
				None,
			));
		}
		// SAFETY: `output_region` is a live mapping of at least `bytes`
		// bytes (checked above).
		let output_pool = unsafe { FrameSlotPool::attach(output_region.data()) };
		if !output_pool.is_valid() {
			return Some(error_message(
				"shared memory does not contain a frame slot pool",
				None,
			));
		}
		self.output_region = Some(output_region);
		self.output_pool = Some(output_pool);

		if hs.input_slots > 0 {
			if hs.input_shm_key.is_empty() || hs.input_slot_data_bytes <= 0 {
				return Some(error_message(
					"handshake missing input shared-memory geometry",
					None,
				));
			}
			let input_bytes = FrameSlotPool::bytes_needed(
				hs.input_slots as u32,
				hs.input_slot_data_bytes as usize,
			);
			let mut input_region = SharedMemoryRegion::new();
			if !input_region.open(&hs.input_shm_key, input_bytes, ShmMode::Attach) {
				return Some(error_message(
					&format!(
						"failed to attach input shared memory: {}",
						input_region.error()
					),
					None,
				));
			}
			// SAFETY: `input_region` is a live mapping of at least
			// `input_bytes` bytes (checked above).
			let input_pool = unsafe { FrameSlotPool::attach(input_region.data()) };
			if !input_pool.is_valid() {
				return Some(error_message(
					"input shared memory does not contain a frame slot pool",
					None,
				));
			}
			self.input_region = Some(input_region);
			self.input_pool = Some(input_pool);
		}

		// Success: protocol v2 answers the geometry handshake with the
		// capability announcement (worker.cpp left the response empty;
		// main now waits for hello_caps to mark the worker alive).
		Some(json!({
			"type": crate::ipc::TYPE_HELLO_CAPS,
			"protocol_version": PROTOCOL_VERSION,
			"formats": [PixelFormat::F32 as i32, SLOT_FORMAT_BGRA8, SLOT_FORMAT_AUDIO_F32],
			"max_slot_bytes": hs.slot_data_bytes,
		}))
	}

	/// `load_graph` (M15 S1): the file checks mirror worker.cpp; the
	/// payload is deserialized for real — an oaknode project XML
	/// ([`oak_node::serializer::load`]) or the minimal
	/// `{"project_copy":N}` identity payload written by the snapshot
	/// store before full graph uploads land in S2. Success answers
	/// nothing (v1 semantics); failures answer an `error` message.
	fn handle_load_graph(&mut self, msg: &Value) -> Option<Value> {
		let load: LoadGraphMsg = match serde_json::from_value(msg.clone()) {
			Ok(l) => l,
			Err(_) => return Some(error_message("invalid load_graph message", None)),
		};
		match std::fs::metadata(&load.path) {
			Err(_) => Some(error_message(
				&format!("graph file does not exist: {}", load.path),
				None,
			)),
			Ok(md) if md.len() == 0 => Some(error_message(
				&format!("graph file is empty: {}", load.path),
				None,
			)),
			Ok(md) => {
				log_error(&format!(
					"LoadGraph: loading {} ({} bytes)",
					load.path,
					md.len()
				));
				let content = match std::fs::read_to_string(&load.path) {
					Ok(c) => c,
					Err(e) => {
						return Some(error_message(&format!("graph file unreadable: {e}"), None))
					}
				};
				match oak_node::serializer::load_with_id_map(&content) {
					Ok((project, id_map)) => {
						let (working, output_spec) = {
							let guard = project.lock().unwrap_or_else(|e| e.into_inner());
							(guard.working_color_space(), guard.output_color_spec())
						};
						let project_uuid = project
							.lock()
							.unwrap_or_else(|e| e.into_inner())
							.uuid
							.clone();
						self.graph = Some(LoadedGraph {
							path: load.path.clone(),
							project: Some(project),
							project_uuid: Some(project_uuid),
							id_map,
							project_copy: 0,
						});
						// The project's color pipeline properties drive this
						// process's input/output transforms (the oakrender
						// process global read by eval + the output node).
						oak_core::color::set_pipeline_color_settings(working, output_spec);
						// A fresh graph snapshot can change what any viewer
						// identity renders; cached pixels from the previous
						// graph must not be served (M16 S2 frame cache).
						self.frame_cache.clear();
						log_error("LoadGraph: oaknode project deserialized");
						None
					}
					Err(graph_err) => {
						// Fallback: the snapshot store's minimal payload
						// `{"project_copy":N}` (identity-only graph context).
						if let Ok(v) = serde_json::from_str::<Value>(&content) {
							if let Some(pc) = v.get("project_copy").and_then(Value::as_u64) {
								self.graph = Some(LoadedGraph {
									path: load.path.clone(),
									project: None,
									project_uuid: None,
									id_map: std::collections::HashMap::new(),
									project_copy: pc,
								});
								// See the full-snapshot branch: any reload
								// invalidates the frame cache (M16 S2).
								self.frame_cache.clear();
								log_error(&format!(
									"LoadGraph: identity-only snapshot (project_copy {pc})"
								));
								return None;
							}
						}
						Some(error_message(
							&format!("graph deserialization failed: {graph_err}"),
							None,
						))
					}
				}
			}
		}
	}

	/// `render_frame` (v1 single-frame path, M15 S1 real): generate the
	/// frame through [`oak_render::eval`], write it into an acquired shm
	/// slot and answer `frame_ready`. The v1 message carries no montage
	/// or footage fields, so this path renders the pipeline's generated
	/// frame; montage/footage tickets arrive via `render_batch`.
	fn handle_render_frame(&mut self, msg: &Value) -> Option<Value> {
		let render: RenderFrameMsg = match serde_json::from_value(msg.clone()) {
			Ok(r) => r,
			Err(_) => return Some(error_message("invalid render_frame message", None)),
		};
		let pool = match self.output_pool.as_ref() {
			Some(p) if p.is_valid() => p,
			_ => {
				return Some(error_message(
					"render_frame: no shared-memory pool attached",
					Some(render.ticket),
				))
			}
		};
		let (w, h) = if render.width > 0 && render.height > 0 {
			(render.width, render.height)
		} else {
			(
				oak_core::frame::VideoParamsPod::DEFAULT_WIDTH,
				oak_core::frame::VideoParamsPod::DEFAULT_HEIGHT,
			)
		};
		let format = if render.format < 0 {
			PixelFormat::F32
		} else {
			match render.format {
				f if f == PixelFormat::U8 as i32 => PixelFormat::U8,
				f if f == PixelFormat::U10 as i32 => PixelFormat::U10,
				f if f == PixelFormat::U16 as i32 => PixelFormat::U16,
				f if f == PixelFormat::F16 as i32 => PixelFormat::F16,
				f if f == PixelFormat::F32 as i32 => PixelFormat::F32,
				other => {
					return Some(error_message(
						&format!("render_frame: unsupported format {other}"),
						Some(render.ticket),
					))
				}
			}
		};
		let time = Rational::new(render.time_num, render.time_den);
		let frame = match eval::generate_frame(time, (w, h), format) {
			Ok(f) => f,
			Err(e) => {
				return Some(error_message(
					&format!("render_frame: generation failed: {e}"),
					Some(render.ticket),
				))
			}
		};

		let slot = match self.acquire_slot(pool) {
			Some(s) => s,
			None => {
				return Some(error_message(
					"render_frame: no free shm slot",
					Some(render.ticket),
				))
			}
		};
		// Write meta + pixels into the slot, then publish.
		let data_size = frame.data.len();
		if data_size > pool.slot_data_bytes() {
			return Some(error_message(
				"render_frame: frame larger than the shm slot",
				Some(render.ticket),
			));
		}
		// SAFETY: `slot` was acquired; the slot block and meta are live
		// shared memory of the attached pool.
		unsafe {
			std::ptr::copy_nonoverlapping(frame.data.as_ptr(), pool.slot_data(slot), data_size);
			let meta = &mut *pool.meta(slot);
			*meta = FrameSlotMeta::default();
			meta.id = render.ticket;
			meta.time_num = render.time_num;
			meta.time_den = render.time_den;
			meta.width = w;
			meta.height = h;
			meta.format = format as i32;
			meta.channel_count = 4;
			meta.linesize = frame.linesize_bytes() as i32;
			meta.data_size = data_size as i32;
			if !pool.publish(slot) {
				return Some(error_message(
					"render_frame: ready ring full",
					Some(render.ticket),
				));
			}
		}
		Some(json!({
			"type": crate::ipc::TYPE_FRAME_READY,
			"ticket": render.ticket,
			"slot": slot,
		}))
	}

	/// `render_batch` (protocol v2; M15 S1): claim confirmation followed
	/// by in-order rendering of every ticket into its main-assigned shm
	/// slot. Responses stream to `out`: one `batch_accepted`, then one
	/// `frame_ready` or `frame_failed` per ticket. Crashes the process
	/// deliberately when the crash-mode environment asks for it (the
	/// crash-isolation test hook).
	fn handle_render_batch_stream(&mut self, line: &str, out: &mut impl Write) -> io::Result<()> {
		let batch: RenderBatchMsg = match serde_json::from_str(line) {
			Ok(b) => b,
			Err(_) => {
				return write_message(out, &error_message("invalid render_batch message", None))
			}
		};

		// Explicit claim confirmation (design §3.3): these tickets are
		// owned by this worker now — no work stealing. The `type` tag is
		// built by hand because [`BatchAcceptedMsg`] only carries the
		// payload fields.
		let accepted = json!({
			"type": crate::ipc::TYPE_BATCH_ACCEPTED,
			"batch_id": batch.batch_id,
			"tickets": batch.tickets.iter().map(|t| t.ticket).collect::<Vec<_>>(),
		});
		write_message(out, &accepted)?;
		out.flush()?;

		for spec in &batch.tickets {
			// Crash-isolation test hook: OAK_WORKER_CRASH_ON_TICKET=<n>
			// segfaults while rendering ticket n. A marker file (env
			// OAK_WORKER_CRASH_MARKER) makes the crash one-shot so the
			// restarted worker renders the frame for real.
			self.maybe_crash_for_testing(spec.ticket);

			let response = match self.render_ticket_to_slot(spec) {
				Ok(slot) => json!({
					"type": crate::ipc::TYPE_FRAME_READY,
					"ticket": spec.ticket,
					"slot": slot,
				}),
				Err(e) => {
					log_error(&format!("render_batch: ticket {} failed: {e}", spec.ticket));
					json!({
						"type": crate::ipc::TYPE_FRAME_FAILED,
						"ticket": spec.ticket,
						"error": e,
					})
				}
			};
			write_message(out, &response)?;
			out.flush()?;
		}
		Ok(())
	}

	/// The crash-mode test hook (see [`Self::handle_render_batch_stream`]).
	fn maybe_crash_for_testing(&self, ticket: i64) {
		let Ok(want) = std::env::var("OAK_WORKER_CRASH_ON_TICKET") else {
			return;
		};
		let Ok(crash_on) = want.parse::<i64>() else {
			return;
		};
		if crash_on != ticket {
			return;
		}
		let marker = std::env::var("OAK_WORKER_CRASH_MARKER").ok();
		let should_crash = match &marker {
			Some(path) => !std::path::Path::new(path).exists(),
			None => true,
		};
		if !should_crash {
			return;
		}
		if let Some(path) = &marker {
			let _ = std::fs::write(path, b"crashed");
		}
		log_error(&format!("crash mode: dying on ticket {ticket}"));
		// Raise SIGSEGV like a real plugin crash; abort as the fallback.
		unsafe { libc::raise(libc::SIGSEGV) };
		std::process::abort();
	}

	/// Acquire a free output slot, polling until one appears (the free
	/// ring is the only filler-side entry point; flow control). `None`
	/// on shutdown or after the 30 s safety deadline.
	fn acquire_slot(&self, pool: &FrameSlotPool) -> Option<u32> {
		let deadline = Instant::now() + Duration::from_secs(30);
		let mut slot = 0u32;
		loop {
			if self.shutdown_requested {
				return None;
			}
			// SAFETY: valid attached pool; the worker is the filler, so
			// popping the free ring is its SPSC role.
			if unsafe { pool.acquire(&mut slot) } {
				return Some(slot);
			}
			if Instant::now() > deadline {
				return None;
			}
			std::thread::sleep(Duration::from_millis(1));
		}
	}

	/// Render one batch ticket into its main-assigned slot (acquire,
	/// render, publish). Returns the published slot index.
	fn render_ticket_to_slot(&mut self, spec: &BatchTicketSpec) -> Result<u32, String> {
		// Clone the pool view (a cheap mapping-shared copy) so the render
		// path below can borrow `self` mutably (scratch buffer).
		let pool = match self.output_pool.clone() {
			Some(p) if p.is_valid() => p,
			_ => return Err("no shared-memory pool attached".to_string()),
		};

		// Flow control: acquire through the free ring. Main seeds and
		// releases slots in assignment order, so the pop yields exactly
		// the assigned slot — anything else is a protocol violation.
		let acquired = self
			.acquire_slot(&pool)
			.ok_or_else(|| "no free shm slot (shutdown or timeout)".to_string())?;
		if acquired != spec.slot as u32 {
			return Err(format!(
				"slot assignment mismatch: acquired {acquired}, assigned {}",
				spec.slot
			));
		}
		let slot = acquired;

		let result = self.render_spec_pixels(spec, &pool);
		match result {
			Ok(()) => {
				// SAFETY: `slot` was acquired above and rendered into.
				let published = unsafe { pool.publish(slot) };
				if !published {
					Err("ready ring full".to_string())
				} else {
					Ok(slot)
				}
			}
			// The slot was acquired but never published; main recycles it
			// when it sees frame_failed (the worker cannot push back to
			// the free ring — that is the drainer's SPSC role).
			Err(e) => Err(e),
		}
	}

	/// `render_audio_batch` (protocol v2, M15 S3): claim confirmation
	/// followed by in-order mixing of every audio range pull into its
	/// main-assigned shm slot (interleaved f32, wire format
	/// [`SLOT_FORMAT_AUDIO_F32`]). Responses stream to `out`: one
	/// `batch_accepted`, then one `frame_ready` or `frame_failed` per
	/// ticket — the same claim/credit/frame_ready flow as
	/// [`Self::handle_render_batch_stream`]. Crashes the process
	/// deliberately when the crash-mode environment asks for it (the
	/// audio crash-isolation test hook; the audio slot geometry check is
	/// below the video one in `handle_line`).
	fn handle_render_audio_batch_stream(
		&mut self,
		line: &str,
		out: &mut impl Write,
	) -> io::Result<()> {
		let batch: RenderAudioBatchMsg = match serde_json::from_str(line) {
			Ok(b) => b,
			Err(_) => {
				return write_message(
					out,
					&error_message("invalid render_audio_batch message", None),
				)
			}
		};

		let accepted = json!({
			"type": crate::ipc::TYPE_BATCH_ACCEPTED,
			"batch_id": batch.batch_id,
			"tickets": batch.tickets.iter().map(|t| t.ticket).collect::<Vec<_>>(),
		});
		write_message(out, &accepted)?;
		out.flush()?;

		for spec in &batch.tickets {
			self.maybe_crash_for_testing(spec.ticket);

			let response = match self.render_audio_ticket_to_slot(spec) {
				Ok(slot) => json!({
					"type": crate::ipc::TYPE_FRAME_READY,
					"ticket": spec.ticket,
					"slot": slot,
				}),
				Err(e) => {
					log_error(&format!(
						"render_audio_batch: ticket {} failed: {e}",
						spec.ticket
					));
					json!({
						"type": crate::ipc::TYPE_FRAME_FAILED,
						"ticket": spec.ticket,
						"error": e,
					})
				}
			};
			write_message(out, &response)?;
			out.flush()?;
		}
		Ok(())
	}

	/// Mix one audio range pull into its main-assigned slot (acquire,
	/// mix, publish). Returns the published slot index.
	fn render_audio_ticket_to_slot(&mut self, spec: &AudioTicketSpec) -> Result<u32, String> {
		let pool = match self.output_pool.clone() {
			Some(p) if p.is_valid() => p,
			_ => return Err("no shared-memory pool attached".to_string()),
		};

		let acquired = self
			.acquire_slot(&pool)
			.ok_or_else(|| "no free shm slot (shutdown or timeout)".to_string())?;
		if acquired != spec.slot as u32 {
			return Err(format!(
				"slot assignment mismatch: acquired {acquired}, assigned {}",
				spec.slot
			));
		}
		let slot = acquired;

		let params = self.audio_ticket_params(spec)?;
		let need = eval::audio_samples_byte_len(&params).map_err(|e| e.to_string())?;
		if need > pool.slot_data_bytes() {
			// The slot was acquired but never published; main recycles it on
			// frame_failed (see render_ticket_to_slot).
			return Err(format!(
				"audio range needs {need} bytes, slot holds {}",
				pool.slot_data_bytes()
			));
		}

		// SAFETY: `slot` was acquired above; the block is live shared memory
		// of the attached pool.
		let dst = unsafe {
			std::slice::from_raw_parts_mut(pool.slot_data(spec.slot as u32), pool.slot_data_bytes())
		};
		eval::render_audio_samples_into(&params, &mut dst[..need]).map_err(|e| e.to_string())?;

		// SAFETY: slot in range of the attached pool.
		unsafe {
			let meta = &mut *pool.meta(spec.slot as u32);
			*meta = FrameSlotMeta::default();
			meta.id = spec.ticket;
			meta.time_num = spec.time_num;
			meta.time_den = spec.time_den;
			// Audio slots reuse the video meta fields: `width` carries the
			// sample rate, `channel_count`/`linesize` describe the interleaved
			// layout, `data_size` is the sample bytes (SLOT_FORMAT_AUDIO_F32).
			meta.width = params.sample_rate;
			meta.height = 0;
			meta.format = SLOT_FORMAT_AUDIO_F32;
			meta.channel_count = params.channel_layout.count_ones().max(1) as i32;
			meta.linesize = meta.channel_count * 4;
			meta.data_size = need as i32;
		}

		let published = unsafe { pool.publish(slot) };
		if !published {
			Err("ready ring full".to_string())
		} else {
			Ok(slot)
		}
	}

	/// Map a wire audio ticket spec to the eval producer's audio params.
	fn audio_ticket_params(&self, spec: &AudioTicketSpec) -> Result<AudioTicketParams, String> {
		if spec.time_den <= 0 || spec.duration_den <= 0 || spec.sample_rate <= 0 {
			return Err(format!(
				"bad audio geometry: {}x{}, rate {}",
				spec.time_den, spec.duration_den, spec.sample_rate
			));
		}
		let start = Rational::new(spec.time_num, spec.time_den);
		let duration = Rational::new(spec.duration_num, spec.duration_den);
		let montage: Vec<MontageClip> = spec
			.montage
			.iter()
			.map(|c| MontageClip {
				filename: c.filename.clone(),
				stream_index: c.stream_index,
				in_time: Rational::new(c.in_num, c.in_den),
				out_time: Rational::new(c.out_num, c.out_den),
				media_in: Rational::new(c.media_in_num, c.media_in_den),
				gain: c.gain,
				effects: c
					.effects
					.iter()
					.map(crate::ipc::montage_effect_from)
					.collect(),
			})
			.collect();
		Ok(AudioTicketParams {
			viewer: self.graph.as_ref().map(|g| g.project_copy).unwrap_or(0),
			range: oak_core::TimeRange::new(start, start + duration),
			sample_rate: spec.sample_rate,
			channel_layout: spec.channel_layout,
			montage,
		})
	}

	/// Refresh the process-global pipeline color settings from the loaded
	/// project snapshot (the source of truth for what this worker renders).
	/// Returns true when the settings changed — the caller must drop the
	/// frame cache, whose F32 bytes were produced under the old settings
	/// (M16 S2). The resync keeps the global current via `load_graph`; this
	/// covers tickets in flight before that IPC lands, and mirrors
	/// [`handle_load_graph`]'s adopt step.
	fn sync_pipeline_color_from_graph(&mut self) -> bool {
		let Some(graph) = &self.graph else {
			return false;
		};
		let Some(project) = &graph.project else {
			return false;
		};
		let (working, output) = {
			let guard = project.lock().unwrap_or_else(|e| e.into_inner());
			(guard.working_color_space(), guard.output_color_spec())
		};
		if oak_core::color::pipeline_working_space() == working
			&& oak_core::color::pipeline_output_spec() == output
		{
			return false;
		}
		oak_core::color::set_pipeline_color_settings(working, output);
		true
	}

	/// Render `spec` into the slot's data block and fill the slot meta.
	fn render_spec_pixels(
		&mut self,
		spec: &BatchTicketSpec,
		pool: &FrameSlotPool,
	) -> Result<(), String> {
		// Derive the pipeline colors from the loaded project on every render:
		// eval's decode linearization and the output node below read the
		// process global, which the resync (load_graph) keeps current — but a
		// ticket in flight before that IPC lands must not render under stale
		// colors. A change invalidates the F32 frame cache: cached bytes were
		// produced under the previous settings (M16 S2).
		if self.sync_pipeline_color_from_graph() {
			self.frame_cache.clear();
		}
		let w = spec.width;
		let h = spec.height;
		if w <= 0 || h <= 0 {
			return Err(format!("bad render size {w}x{h}"));
		}
		let time = Rational::new(spec.time_num, spec.time_den);
		let params = self.ticket_params(spec, time);

		let bgra8 = spec.format == SLOT_FORMAT_BGRA8;
		let (dst_bpp, dst_linesize) = if bgra8 { (4, w * 4) } else { (16, w * 16) };
		let dst_need = (h as usize) * (dst_linesize as usize);
		if dst_need > pool.slot_data_bytes() {
			return Err(format!(
				"frame {}x{} needs {dst_need} bytes, slot holds {}",
				w,
				h,
				pool.slot_data_bytes()
			));
		}
		let _ = dst_bpp;

		// SAFETY: `spec.slot` was acquired by render_ticket_to_slot; the
		// block is live shared memory of the attached pool.
		let dst = unsafe {
			std::slice::from_raw_parts_mut(pool.slot_data(spec.slot as u32), pool.slot_data_bytes())
		};

		// M16 S2 frame cache: memoized F32 pipeline bytes keyed by the
		// render-deterministic spec subset (ticket/slot/format/channels are
		// delivery, not picture — an F32 and a BGRA8 request for the same
		// frame share one entry). A hit is a copy (plus the end-of-pipe
		// convert for BGRA8 slots); a miss renders as before and memoizes
		// the F32 bytes. See [`crate::framecache`].
		let f32_need = (w as usize) * (h as usize) * 16;
		let key = crate::framecache::spec_cache_key(spec);
		let mut cached = self.frame_cache.get(&key).map(|b| b.to_vec());
		if let Some(c) = &cached {
			if c.len() < f32_need {
				// Defensive: an entry smaller than this geometry is not for
				// this frame (keys are geometry-signed); re-render.
				cached = None;
			}
		}

		if !bgra8 {
			// F32 RGBA: render straight into the slot (no staging copy).
			match &cached {
				Some(c) => dst[..f32_need].copy_from_slice(&c[..f32_need]),
				None => {
					render_f32_into(
						spec,
						&params,
						&self.graph,
						time,
						(w, h),
						&mut dst[..dst_need],
					)?;
					self.frame_cache.insert(key, dst[..f32_need].to_vec());
				}
			}
		} else {
			// BGRA8: the F32 pipeline frame comes from the cache or the
			// session scratch, then converts into the slot (the end-of-pipe
			// format convert is not an extra frame copy, design §3.1).
			// Before quantization the output node runs: working space
			// (ACEScg) → the project's output colorspace, so the 8-bit
			// pixels carry gamma-encoded display values, not linear light.
			match cached.as_mut() {
				Some(c) => {
					apply_output_node(c, (w * h) as usize);
					convert_f32_rgba_to_bgra8(&c[..f32_need], &mut dst[..dst_need]);
				}
				None => {
					if self.f32_scratch.len() < f32_need {
						self.f32_scratch.resize(f32_need, 0);
					}
					render_f32_into(
						spec,
						&params,
						&self.graph,
						time,
						(w, h),
						&mut self.f32_scratch[..f32_need],
					)?;
					self.frame_cache
						.insert(key, self.f32_scratch[..f32_need].to_vec());
					apply_output_node(&mut self.f32_scratch, (w * h) as usize);
					convert_f32_rgba_to_bgra8(&self.f32_scratch[..f32_need], &mut dst[..dst_need]);
				}
			}
		}

		// Slot meta (fresh each publish).
		// SAFETY: slot in range of the attached pool.
		unsafe {
			let meta = &mut *pool.meta(spec.slot as u32);
			*meta = FrameSlotMeta::default();
			meta.id = spec.ticket;
			meta.time_num = spec.time_num;
			meta.time_den = spec.time_den;
			meta.width = w;
			meta.height = h;
			meta.format = spec.format;
			meta.channel_count = spec.channels.max(4);
			meta.linesize = dst_linesize;
			meta.data_size = dst_need as i32;
		}
		Ok(())
	}

	/// Map a wire ticket spec to the eval producer's ticket params.
	fn ticket_params(&self, spec: &BatchTicketSpec, time: Rational) -> VideoTicketParams {
		let footage = if spec.footage_file.is_empty() {
			None
		} else {
			Some((spec.footage_file.clone(), spec.footage_stream))
		};
		let montage: Vec<MontageClip> = spec
			.montage
			.iter()
			.map(|c| MontageClip {
				filename: c.filename.clone(),
				stream_index: c.stream_index,
				in_time: Rational::new(c.in_num, c.in_den),
				out_time: Rational::new(c.out_num, c.out_den),
				media_in: Rational::new(c.media_in_num, c.media_in_den),
				gain: c.gain,
				effects: c
					.effects
					.iter()
					.map(crate::ipc::montage_effect_from)
					.collect(),
			})
			.collect();
		let adjustments: Vec<AdjustmentSpan> = spec
			.adjustments
			.iter()
			.map(oak_render::ipc::adjustment_from_wire)
			.collect();
		VideoTicketParams {
			viewer: if spec.viewer_node != 0 {
				spec.viewer_node
			} else {
				self.graph.as_ref().map(|g| g.project_copy).unwrap_or(0)
			},
			project: spec.project_key.clone(),
			time,
			force_size: Some((spec.width, spec.height)),
			force_format: Some(PixelFormat::F32),
			cache: None,
			cache_dir: None,
			cache_id: None,
			cache_timebase: None,
			footage,
			montage,
			adjustments,
		}
	}
}

/// Render the F32 RGBA pipeline frame for `spec` into `dst`
/// (`(w*h*16)` bytes): graph-mode viewer frame, generated transparent
/// black, footage decode, or montage composite — through
/// [`oak_render::eval`].
fn render_f32_into(
	spec: &BatchTicketSpec,
	params: &VideoTicketParams,
	graph: &Option<LoadedGraph>,
	time: Rational,
	size: (i32, i32),
	dst: &mut [u8],
) -> Result<(), String> {
	let (w, h) = size;
	let stride = w * 16;
	// M16 S1 graph mode: render the ticket's viewer node from the loaded
	// snapshot — but only when the snapshot belongs to the ticket's own
	// project. The ticket carries its owning project's uuid; a stale graph
	// from a different project (fresh projects reuse small identity
	// numbers, so a foreign viewer identity can resolve successfully and
	// silently render the wrong picture) must never answer it — that ticket
	// falls through to the montage path instead, and is NOT logged (the
	// mismatch is the normal suite-order case, not an error). A missing
	// graph / viewer is logged once and also falls back.
	if spec.viewer_node != 0 {
		let project_matches = graph
			.as_ref()
			.and_then(|g| g.project_uuid.as_deref())
			.map(|uuid| uuid == spec.project_key.as_str())
			.unwrap_or(false);
		if project_matches {
			if let Some(project) = graph.as_ref().and_then(|g| g.project.as_ref()) {
				// The ticket's viewer is an identity in the *saved* project;
				// translate through the load map first, then fall back to the
				// raw packed id (identity-only snapshots, foreign files).
				let viewer_id = graph
					.as_ref()
					.and_then(|g| g.id_map.get(&spec.viewer_node).copied())
					.or_else(|| oak_node::id::NodeId::from_identity(spec.viewer_node));
				match viewer_id {
					Some(viewer_id) => {
						let rendered = eval::render_graph_frame(
							project,
							viewer_id,
							time,
							(w, h),
							PixelFormat::F32,
						);
						// M2: the graph renders all-GPU in-process; the worker's
						// wire format is a CPU shm slot, so this is the explicit
						// readback boundary of the process backend.
						let frame = match rendered {
							Ok(oak_core::texture::Texture::Cpu(frame)) => Some(frame),
							Ok(texture @ oak_core::texture::Texture::Gpu { .. }) => {
								match texture.to_frame() {
									Ok(frame) => Some(frame),
									Err(e) => {
										warn_graph_fallback(spec.viewer_node, &e.to_string());
										None
									}
								}
							}
							Ok(oak_core::texture::Texture::Planar(_)) => {
								// The footage path resolves imported planar
								// frames before returning; a planar texture
								// here is a bug, not a frame.
								warn_graph_fallback(
									spec.viewer_node,
									"unresolved planar texture",
								);
								None
							}
							Err(e) => {
								warn_graph_fallback(spec.viewer_node, &e.to_string());
								None
							}
						};
						if let Some(frame) = frame {
							let src_stride = frame.linesize_bytes();
							let row_bytes = (w as usize) * 16;
							if frame.data.len() < src_stride * (h as usize)
								|| dst.len() < row_bytes * (h as usize)
							{
								return Err("graph frame geometry mismatch".to_string());
							}
							for y in 0..h as usize {
								dst[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(
									&frame.data[y * src_stride..y * src_stride + row_bytes],
								);
							}
							return Ok(());
						}
					}
					None => warn_graph_fallback(spec.viewer_node, "viewer node not in graph"),
				}
			} else {
				warn_graph_fallback(spec.viewer_node, "no loaded graph");
			}
		}
	}
	if !params.montage.is_empty() {
		return eval::render_montage_frame_into(time, params, (w, h), dst, stride)
			.map_err(|e| format!("montage render: {e}"));
	}
	if !spec.footage_file.is_empty() {
		let decoded = eval::render_footage_frame(
			&spec.footage_file,
			spec.footage_stream,
			time,
			(w, h),
			PixelFormat::F32,
		)
		.map_err(|e| format!("footage decode: {e}"))?;
		// M2: decode stays CPU for now (M5 makes it GPU); an imported GPU
		// texture would still have to cross into the shm slot here.
		let frame = match &decoded {
			oak_core::texture::Texture::Cpu(frame) => frame.clone(),
			gpu @ oak_core::texture::Texture::Gpu { .. } => gpu
				.to_frame()
				.map_err(|e| format!("decode readback: {e}"))?,
			// Resolved by the footage path; a planar frame cannot cross
			// into the shm slot without a readback.
			oak_core::texture::Texture::Planar(_) => {
				return Err("unresolved planar decode texture".to_string())
			}
		};
		let src_stride = frame.linesize_bytes() as usize;
		let row_bytes = (w as usize) * 16;
		if frame.data.len() < src_stride * (h as usize) || dst.len() < row_bytes * (h as usize) {
			return Err("decoded frame geometry mismatch".to_string());
		}
		for y in 0..h as usize {
			dst[y * row_bytes..(y + 1) * row_bytes]
				.copy_from_slice(&frame.data[y * src_stride..y * src_stride + row_bytes]);
		}
		return Ok(());
	}
	// Generated frame: transparent black.
	dst[..(h as usize) * (stride as usize)].fill(0);
	Ok(())
}

/// Log (once per process) why a graph-mode ticket fell back to the
/// montage path — a missing snapshot, an absent viewer node, or a graph
/// render error. AtomicBool keeps a GPU-less or graph-less host from
/// spamming the worker log on every frame.
fn warn_graph_fallback(viewer: u64, why: &str) {
	static WARNED: AtomicBool = AtomicBool::new(false);
	if !WARNED.swap(true, Ordering::Relaxed) {
		log_error(&format!(
			"graph-mode ticket viewer {viewer}: {why}; falling back to montage"
		));
	}
}

/// Convert F32 RGBA (`src`, 16 bytes/px) to 8-bit BGRA (`dst`, 4
/// bytes/px) with clamping — the worker-side end-of-pipe convert for
/// BGRA8 preview slots.
fn convert_f32_rgba_to_bgra8(src: &[u8], dst: &mut [u8]) {
	let to_u8 = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 };
	let pixels = dst.len() / 4;
	for i in 0..pixels {
		let s = &src[i * 16..i * 16 + 16];
		let r = f32::from_le_bytes(s[0..4].try_into().unwrap());
		let g = f32::from_le_bytes(s[4..8].try_into().unwrap());
		let b = f32::from_le_bytes(s[8..12].try_into().unwrap());
		let a = f32::from_le_bytes(s[12..16].try_into().unwrap());
		let d = &mut dst[i * 4..i * 4 + 4];
		d[0] = to_u8(b);
		d[1] = to_u8(g);
		d[2] = to_u8(r);
		d[3] = to_u8(a);
	}
}

/// The output node for the BGRA8 delivery path: convert the first
/// `pixels` pixels of an F32 RGBA byte buffer from the pipeline working
/// space to the project's output colorspace (in place), so the 8-bit
/// quantization encodes display-referred values instead of linear light.
/// A no-op in the legacy sRGB working space (content already is
/// display-referred sRGB).
fn apply_output_node(bytes: &mut [u8], pixels: usize) {
	if oak_core::color::pipeline_working_space()
		== oak_core::colormath::WorkingColorSpace::SrgbLegacy
	{
		return;
	}
	let spec = oak_core::color::pipeline_output_spec();
	oak_core::colormath::acescg_to_output_bytes(bytes, pixels, spec);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Full render-worker main, transport-agnostic in the backend name.
///
/// Mirrors `oakengine_worker_main()` in worker.cpp: create the session
/// (which initializes the render backend), load the runtime config, write
/// the startup handshake, then serve the NDJSON control loop on
/// stdin/stdout until a `shutdown` message or EOF. Returns the process
/// exit code.
pub fn worker_main(backend: &str) -> i32 {
	// 1. Session creation initializes the render backend through the
	//    oakrender crate's direct Rust API
	//    (oakengine_worker_session_create()). The M15 "cpu" backend is
	//    headless: no renderer, CPU evaluation + decode only. M16 S1: a
	//    failed GPU init for any other backend (e.g. "auto" on a GPU-less
	//    host) is tolerated — the session continues headless.
	let mut session = match WorkerSession::create(backend) {
		Ok(s) => s,
		Err(msg) => {
			log_error(&msg);
			return 1;
		}
	};
	if !session.has_renderer() && is_no_backend(backend) {
		// Mirrors oakengine_worker_main(): an explicit no-renderer request
		// (""/"none") leaves nothing to evaluate, so the worker exits 1.
		// Any other backend failure already logged headless continuation.
		log_error("no renderer initialized");
		return 1;
	}

	// 2. Runtime services (config load, plugin executor install).
	if !session.initialize_runtime() {
		return 1;
	}

	// 3. Startup handshake before the loop (mirrors worker.cpp main).
	let handshake = session.startup_handshake();
	let stdout = io::stdout();
	let mut out = io::BufWriter::new(stdout.lock());
	if let Err(e) = write_message(&mut out, &handshake) {
		log_error(&format!("failed to write startup handshake: {e}"));
		return 1;
	}
	if let Err(e) = out.flush() {
		log_error(&format!("failed to flush startup handshake: {e}"));
		return 1;
	}

	// 4. NDJSON control loop until a shutdown message or EOF.
	let stdin = io::stdin();
	let mut reader = stdin.lock();
	let mut line = String::new();
	let mut exit_code = 0;
	while !session.shutdown_requested() {
		line.clear();
		match reader.read_line(&mut line) {
			Ok(0) => break, // EOF: the parent closed the control pipe.
			Ok(_) => {}
			Err(e) => {
				log_error(&format!("failed to read control line: {e}"));
				break;
			}
		}
		if line.trim().is_empty() {
			// Blank lines are skipped silently (read_message() semantics).
			continue;
		}
		// Protocol v2: render_batch streams its responses (one
		// batch_accepted + one frame_ready/frame_failed per ticket).
		let typ = serde_json::from_str::<Value>(&line)
			.ok()
			.and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string));
		match typ.as_deref() {
			Some(TYPE_RENDER_BATCH) => {
				if let Err(e) = session.handle_render_batch_stream(&line, &mut out) {
					log_error(&format!("failed to serve render_batch: {e}"));
					exit_code = 1;
					break;
				}
				// Plugin progress events buffered during the batch go out now.
				flush_worker_progress(&mut out);
				continue;
			}
			// M15 S3: render_audio_batch streams the same way (audio range
			// pulls into shm slots, interleaved f32).
			Some(TYPE_RENDER_AUDIO_BATCH) => {
				if let Err(e) = session.handle_render_audio_batch_stream(&line, &mut out) {
					log_error(&format!("failed to serve render_audio_batch: {e}"));
					exit_code = 1;
					break;
				}
				flush_worker_progress(&mut out);
				continue;
			}
			_ => {}
		}
		if let Some(response) = session.handle_line(&line) {
			if let Err(e) = write_message(&mut out, &response) {
				log_error(&format!("failed to write response: {e}"));
				exit_code = 1;
				break;
			}
			if let Err(e) = out.flush() {
				log_error(&format!("failed to flush response: {e}"));
				exit_code = 1;
				break;
			}
		}
		// Progress events buffered while serving the control message.
		flush_worker_progress(&mut out);
	}
	exit_code
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ipc::{
		FrameSlotPool, SharedMemoryRegion, ShmMode, WireEffectParam, WireMontageClip,
		WireMontageEffect, WireNodeValue,
	};
	use oak_core::colormath::{OutputColorSpec, WorkingColorSpace};
	use oak_node::id::NodeId;
	use oak_node::project::Project;
	use oak_node::sequence::SequenceBehavior;
	use oak_node::track::TrackListBehavior;
	use serde_json::json;
	use std::collections::HashMap;
	use std::ptr;

	/// Serializes the tests that mutate the process-global pipeline color
	/// settings (the two pre-existing adopt/sync tests included).
	static COLOR_TEST_LOCK: Mutex<()> = Mutex::new(());

	/// Saves the process-global pipeline color settings and restores them on
	/// drop, so a panicking assertion cannot leak the override into the next
	/// serialized test. All users hold [`COLOR_TEST_LOCK`].
	struct ColorSettingsGuard {
		working: oak_core::colormath::WorkingColorSpace,
		output: oak_core::colormath::OutputColorSpec,
	}

	impl ColorSettingsGuard {
		fn capture() -> ColorSettingsGuard {
			ColorSettingsGuard {
				working: oak_core::color::pipeline_working_space(),
				output: oak_core::color::pipeline_output_spec(),
			}
		}
	}

	impl Drop for ColorSettingsGuard {
		fn drop(&mut self) {
			oak_core::color::set_pipeline_color_settings(self.working, self.output);
		}
	}

	/// Test-only RAII environment override: restores the previous value (or
	/// absence) on drop, so a panicking assertion cannot leak a crash-mode
	/// override into the next serialized test.
	struct EnvGuard {
		name: &'static str,
		previous: Option<std::ffi::OsString>,
	}

	impl EnvGuard {
		fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
			let previous = std::env::var_os(name);
			std::env::set_var(name, value);
			Self { name, previous }
		}
	}

	impl Drop for EnvGuard {
		fn drop(&mut self) {
			match self.previous.take() {
				Some(value) => std::env::set_var(self.name, value),
				None => std::env::remove_var(self.name),
			}
		}
	}

	fn test_key(name: &str) -> String {
		static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
		let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		SharedMemoryRegion::make_key(i64::from(std::process::id()), (n & 0x7FFF) as i32)
			+ &format!("-w-{name}")
	}

	/// The "parent" side of a handshake: create an output segment holding a
	/// pool, optionally an input segment, and return the handshake message
	/// plus the owner regions (kept alive by the caller).
	fn parent_side(
		slots: i32,
		slot_bytes: i64,
		input: bool,
	) -> (Value, SharedMemoryRegion, Option<SharedMemoryRegion>) {
		let out_key = test_key("out");
		let out_bytes = FrameSlotPool::bytes_needed(slots as u32, slot_bytes as usize);
		let mut out_region = SharedMemoryRegion::new();
		let opened = out_region.open(&out_key, out_bytes, ShmMode::Create);
		let open_error = out_region.error();
		assert!(opened, "{open_error}");
		// SAFETY: live mapping sized by bytes_needed.
		let _pool =
			unsafe { FrameSlotPool::create(out_region.data(), slots as u32, slot_bytes as usize) };

		let (in_key, _in_bytes, in_region) = if input {
			let in_key = test_key("in");
			let in_bytes = FrameSlotPool::bytes_needed(slots as u32, slot_bytes as usize);
			let mut in_region = SharedMemoryRegion::new();
			assert!(in_region.open(&in_key, in_bytes, ShmMode::Create));
			// SAFETY: live mapping.
			let _ = unsafe {
				FrameSlotPool::create(in_region.data(), slots as u32, slot_bytes as usize)
			};
			(Some(in_key), Some(in_bytes), Some(in_region))
		} else {
			(None, None, None)
		};

		let hs = json!({
			"type": "handshake",
			"protocol_version": PROTOCOL_VERSION,
			"shm_key": out_key,
			"input_shm_key": in_key.unwrap_or_default(),
			"input_slots": if input { slots } else { 0 },
			"output_slots": slots,
			"slot_data_bytes": slot_bytes,
			"input_slot_data_bytes": if input { slot_bytes } else { 0 },
		});
		(hs, out_region, in_region)
	}

	#[test]
	fn no_backend_detection_matches_cpp() {
		assert!(is_no_backend(""));
		assert!(is_no_backend("none"));
		assert!(is_no_backend("NONE"));
		assert!(!is_no_backend("opengl"));
		assert!(!is_no_backend("vulkan"));
	}

	#[test]
	fn none_backend_session_has_no_renderer_but_serves_messages() {
		let mut s = WorkerSession::create("none").unwrap();
		assert!(!s.has_renderer());
		let resp = s.handle_line(r#"{"type":"shutdown"}"#);
		assert!(resp.is_none());
		assert!(s.shutdown_requested());
	}

	/// The M15 headless "cpu" backend: no renderer, but the session stays
	/// fully operational (generated frames render through CPU eval).
	#[test]
	fn cpu_backend_session_is_headless_but_operational() {
		assert!(is_cpu_backend("cpu"));
		assert!(is_cpu_backend("CPU"));
		assert!(!is_cpu_backend("auto"));
		let mut s = WorkerSession::create("cpu").unwrap();
		assert!(!s.has_renderer(), "cpu means no GPU renderer");
		assert!(!s.shutdown_requested());
		assert!(s.handle_line(r#"{"type":"shutdown"}"#).is_none());
		assert!(s.shutdown_requested());
	}

	#[test]
	fn startup_handshake_is_protocol_version_1_with_empty_geometry() {
		let s = WorkerSession::create("none").unwrap();
		let hs = s.startup_handshake();
		assert_eq!(
			hs,
			json!({
				"type": "handshake",
				"protocol_version": 1,
				"shm_key": "",
				"input_shm_key": "",
				"input_slots": 0,
				"output_slots": 0,
				"slot_data_bytes": 0,
				"input_slot_data_bytes": 0,
			})
		);
	}

	#[test]
	fn malformed_line_yields_error_response() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line("this is not json").unwrap();
		assert_eq!(resp["type"], "error");
		assert_eq!(resp["message"], "malformed control message");
	}

	#[test]
	fn unknown_message_type_yields_error_response() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line(r#"{"type":"frobnicate"}"#).unwrap();
		assert_eq!(resp["message"], "unknown message type: frobnicate");
	}

	#[test]
	fn cancel_and_shutdown_produce_no_response() {
		let mut s = WorkerSession::create("none").unwrap();
		assert!(s.handle_line(r#"{"type":"cancel","ticket":5}"#).is_none());
		assert!(s.handle_line(r#"{"type":"shutdown"}"#).is_none());
		assert!(s.shutdown_requested());
	}

	#[test]
	fn handshake_wrong_protocol_version() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s
			.handle_line(
				r#"{"type":"handshake","protocol_version":99,"shm_key":"k","output_slots":1,"slot_data_bytes":16}"#,
			)
			.unwrap();
		assert_eq!(resp["message"], "unsupported protocol version 99");
	}

	#[test]
	fn handshake_missing_geometry() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s
			.handle_line(r#"{"type":"handshake","protocol_version":1}"#)
			.unwrap();
		assert_eq!(
			resp["message"],
			"handshake missing output shared-memory geometry"
		);
	}

	#[test]
	fn handshake_attaches_real_output_pool() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, out_region, _in) = parent_side(4, 4096, false);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		// Protocol v2: a successful attach answers the geometry handshake
		// with the capability announcement.
		assert_eq!(resp["type"], crate::ipc::TYPE_HELLO_CAPS);
		assert_eq!(resp["protocol_version"], PROTOCOL_VERSION);
		let formats: Vec<i64> = resp["formats"]
			.as_array()
			.unwrap()
			.iter()
			.map(|v| v.as_i64().unwrap())
			.collect();
		assert!(formats.contains(&(PixelFormat::F32 as i64)));
		assert!(formats.contains(&(SLOT_FORMAT_BGRA8 as i64)));
		assert_eq!(resp["max_slot_bytes"], 4096);
		// The session now holds a real attached pool with the parent's
		// geometry.
		let out_pool = s.output_pool.as_ref().unwrap();
		assert_eq!(out_pool.slot_count(), 4);
		assert_eq!(out_pool.slot_data_bytes(), 4096);

		// The two views share the same rings, not copies: the parent pops a
		// free slot and the worker's pool sees the ring cursor move; the
		// parent's publish lands in the worker's ready ring.
		// SAFETY: `out_region` is a live mapping containing the pool the
		// session attached to.
		let parent_pool = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut parent_slot = 0;
		assert!(unsafe { parent_pool.acquire(&mut parent_slot) });
		assert_eq!(parent_slot, 0);
		let mut worker_slot = 0;
		assert!(unsafe { out_pool.acquire(&mut worker_slot) });
		assert_eq!(worker_slot, 1, "worker must see the parent's free-ring pop");

		// SAFETY: `parent_slot` was acquired by the parent; slot_bytes
		// writable.
		unsafe {
			ptr::write_bytes(parent_pool.slot_data(parent_slot), 0xAB, 64);
		}
		assert!(unsafe { parent_pool.publish(parent_slot) });
		let mut consumed = 0;
		assert!(unsafe { out_pool.consume(&mut consumed) });
		assert_eq!(consumed, parent_slot);
		// SAFETY: `consumed` was consumed by the worker's pool.
		assert_eq!(unsafe { *out_pool.slot_data_const(consumed) }, 0xAB);
		// Clean up so the region drop at test end unlinks cleanly.
		unsafe { out_pool.release(consumed) };
		unsafe { out_pool.release(worker_slot) };
	}

	#[test]
	fn handshake_attaches_input_pool_too() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(2, 256, true);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		assert_eq!(
			resp["type"],
			crate::ipc::TYPE_HELLO_CAPS,
			"handshake failed: {resp}"
		);
		assert!(s.input_pool.is_some());
		let in_pool = s.input_pool.as_ref().unwrap();
		assert_eq!(in_pool.slot_count(), 2);
		assert_eq!(in_pool.slot_data_bytes(), 256);
	}

	#[test]
	fn handshake_attach_failure_reports_error() {
		let mut s = WorkerSession::create("none").unwrap();
		// A key that was never created.
		let resp = s
			.handle_line(
				&json!({
					"type": "handshake",
					"protocol_version": 1,
					"shm_key": format!("olive-rw-{}-missing", std::process::id()),
					"output_slots": 4,
					"slot_data_bytes": 4096,
				})
				.to_string(),
			)
			.unwrap();
		assert_eq!(resp["type"], "error");
		assert!(resp["message"]
			.as_str()
			.unwrap()
			.starts_with("failed to attach shared memory: "));
		assert!(s.output_pool.is_none());
	}

	#[test]
	fn handshake_rejects_non_pool_segment() {
		let mut s = WorkerSession::create("none").unwrap();
		// A real segment of the right size that does not contain a pool
		// (zeroed memory → wrong magic). Sized so the attach size check
		// passes and the magic check fires.
		let key = test_key("nopool");
		let bytes = FrameSlotPool::bytes_needed(4, 4096);
		let mut region = SharedMemoryRegion::new();
		assert!(region.open(&key, bytes, ShmMode::Create));
		let resp = s
			.handle_line(
				&json!({
					"type": "handshake",
					"protocol_version": 1,
					"shm_key": key,
					"output_slots": 4,
					"slot_data_bytes": 4096,
				})
				.to_string(),
			)
			.unwrap();
		assert_eq!(
			resp["message"],
			"shared memory does not contain a frame slot pool"
		);
	}

	#[test]
	fn handshake_missing_input_geometry_is_an_error() {
		let mut s = WorkerSession::create("none").unwrap();
		let (mut hs, _out, _in) = parent_side(2, 256, false);
		// Ask for input slots without announcing their geometry.
		hs["input_slots"] = json!(2);
		let resp = s.handle_line(&hs.to_string()).unwrap();
		assert_eq!(
			resp["message"],
			"handshake missing input shared-memory geometry"
		);
	}

	#[test]
	fn load_graph_file_checks_then_real_deserialization() {
		let mut s = WorkerSession::create("none").unwrap();

		let missing = "/definitely/not/a/real/graph.ove";
		let resp = s
			.handle_line(&json!({ "type": "load_graph", "path": missing }).to_string())
			.unwrap();
		assert_eq!(
			resp["message"],
			format!("graph file does not exist: {missing}")
		);

		let empty = std::env::temp_dir().join("oak_worker_main_test_empty.ove");
		std::fs::write(&empty, b"").unwrap();
		let resp = s
			.handle_line(
				&json!({ "type": "load_graph", "path": empty.display().to_string() }).to_string(),
			)
			.unwrap();
		assert_eq!(
			resp["message"],
			format!("graph file is empty: {}", empty.display())
		);
		let _ = std::fs::remove_file(&empty);

		// A real oaknode project round-trips through the serializer.
		let project = oak_node::project::Project::new();
		let xml = oak_node::serializer::save(&project.lock().unwrap_or_else(|e| e.into_inner()))
			.expect("serialize empty project");
		let real = std::env::temp_dir().join("oak_worker_main_test_graph.ove");
		std::fs::write(&real, &xml).unwrap();
		let resp = s.handle_line(
			&json!({ "type": "load_graph", "path": real.display().to_string() }).to_string(),
		);
		assert!(resp.is_none(), "unexpected error: {resp:?}");
		let graph = s.graph.as_ref().expect("graph loaded");
		assert!(graph.project.is_some(), "full project deserialized");
		let _ = std::fs::remove_file(&real);

		// The minimal identity-only payload (`{"project_copy":N}`) loads as
		// a copied-project context.
		let ident = std::env::temp_dir().join("oak_worker_main_test_identity.ove");
		std::fs::write(&ident, r#"{"project_copy":7}"#).unwrap();
		let resp = s.handle_line(
			&json!({ "type": "load_graph", "path": ident.display().to_string() }).to_string(),
		);
		assert!(resp.is_none(), "unexpected error: {resp:?}");
		let graph = s.graph.as_ref().expect("graph loaded");
		assert!(graph.project.is_none());
		assert_eq!(graph.project_copy, 7);
		let _ = std::fs::remove_file(&ident);

		// Garbage that is neither project XML nor identity JSON fails
		// explainably.
		let bad = std::env::temp_dir().join("oak_worker_main_test_bad.ove");
		std::fs::write(&bad, b"definitely not a graph").unwrap();
		let resp = s
			.handle_line(
				&json!({ "type": "load_graph", "path": bad.display().to_string() }).to_string(),
			)
			.unwrap();
		assert!(resp["message"]
			.as_str()
			.unwrap()
			.starts_with("graph deserialization failed: "));
		let _ = std::fs::remove_file(&bad);
	}

	#[test]
	fn load_graph_adopts_pipeline_colors_from_snapshot() {
		use oak_core::colormath::{OutputColorSpec, WorkingColorSpace};
		let _guard = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let _colors = ColorSettingsGuard::capture();
		// Reset the process global to something different from the snapshot's
		// settings so the adopt step is observable.
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::SrgbLegacy,
			OutputColorSpec::default(),
		);
		let project = oak_node::project::Project::new();
		{
			let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
			guard.set_working_color_space(WorkingColorSpace::AcesCg);
		}
		let xml = oak_node::serializer::save(&project.lock().unwrap_or_else(|e| e.into_inner()))
			.expect("serialize project");
		let path = std::env::temp_dir().join("oak_worker_main_test_acescg.ove");
		std::fs::write(&path, &xml).unwrap();

		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line(
			&json!({ "type": "load_graph", "path": path.display().to_string() }).to_string(),
		);
		assert!(resp.is_none(), "unexpected error: {resp:?}");
		assert_eq!(
			oak_core::color::pipeline_working_space(),
			WorkingColorSpace::AcesCg,
			"load_graph must adopt the snapshot's working space"
		);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn sync_pipeline_color_from_graph_restores_stale_global() {
		use oak_core::colormath::WorkingColorSpace;
		let _guard = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let _colors = ColorSettingsGuard::capture();
		let project = oak_node::project::Project::new();
		{
			let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
			guard.set_working_color_space(WorkingColorSpace::AcesCg);
		}
		let xml = oak_node::serializer::save(&project.lock().unwrap_or_else(|e| e.into_inner()))
			.expect("serialize project");
		let path = std::env::temp_dir().join("oak_worker_main_test_sync.ove");
		std::fs::write(&path, &xml).unwrap();
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line(
			&json!({ "type": "load_graph", "path": path.display().to_string() }).to_string(),
		);
		assert!(resp.is_none(), "unexpected error: {resp:?}");

		// Simulate the app committing new project settings: the process
		// global flips immediately, but the resync (load_graph re-broadcast)
		// reaches this worker later. A render ticket in that window must not
		// run under the stale colors.
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::SrgbLegacy,
			oak_core::color::pipeline_output_spec(),
		);
		assert!(
			s.sync_pipeline_color_from_graph(),
			"stale global must be refreshed from the loaded project"
		);
		assert_eq!(
			oak_core::color::pipeline_working_space(),
			WorkingColorSpace::AcesCg,
			"sync must restore the snapshot's working space"
		);
		assert!(
			!s.sync_pipeline_color_from_graph(),
			"no change means no frame-cache invalidation"
		);
		let _ = std::fs::remove_file(&path);
	}

	#[test]
	fn acescg_pipeline_round_trip_preserves_srgb_encoded_midgray() {
		// The ACEScg overexposure bug: F32 bytes produced by the legacy sRGB
		// pass-through (already sRGB-encoded) are re-encoded by the ACEScg
		// output transform, applying the sRGB OETF a second time — 0.5
		// becomes srgb_oetf(0.5) ≈ 0.735. The correct pipeline linearizes
		// the code (sRGB EOTF) into ACEScg and then encodes for output, so
		// the mid-gray comes back near 0.5.
		use oak_core::colormath::{
			acescg_to_output_bytes, decode_to_acescg_bytes, srgb_oetf, OutputColorSpec,
			OutputGamut, OutputTransfer, SourcePrimaries, SourceTransfer, WorkingColorSpace,
		};
		let spec = OutputColorSpec {
			gamut: OutputGamut::Srgb,
			transfer: OutputTransfer::Srgb,
		};

		// Buggy path: treating the already-encoded code as ACEScg linear.
		let mut double_encoded = [0.5f32, 0.5, 0.5, 1.0];
		oak_core::colormath::working_to_display_target(
			&mut double_encoded,
			WorkingColorSpace::AcesCg,
			spec,
		);
		assert!(
			(double_encoded[0] - srgb_oetf(0.5)).abs() < 1e-4,
			"mis-encoding 0.5 as ACEScg-linear must yield srgb_oetf(0.5) ≈ 0.735, got {}",
			double_encoded[0]
		);

		// Correct path: decode (sRGB EOTF) → ACEScg → output encode.
		let mut bytes = [0.5f32, 0.5, 0.5, 1.0].map(f32::to_le_bytes).concat();
		decode_to_acescg_bytes(
			&mut bytes,
			1,
			SourcePrimaries::Bt709,
			SourceTransfer::SdrGamma,
		);
		acescg_to_output_bytes(&mut bytes, 1, spec);
		let out = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
		assert!(
			(out - 0.5).abs() < 1e-3,
			"correct pipeline must preserve mid-gray, got {out}"
		);
	}

	#[test]
	fn render_frame_without_pool_reports_error_with_ticket() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s
			.handle_line(r#"{"type":"render_frame","ticket":123,"node":"abc"}"#)
			.unwrap();
		assert_eq!(resp["type"], "error");
		assert_eq!(resp["ticket"], 123);
		assert_eq!(
			resp["message"],
			"render_frame: no shared-memory pool attached"
		);
	}

	#[test]
	fn render_frame_v1_renders_generated_frame_into_slot() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, out_region, _in) = parent_side(4, 4 * 4 * 16, false);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		assert_eq!(resp["type"], crate::ipc::TYPE_HELLO_CAPS);

		// The v1 single-frame path renders the pipeline's generated frame
		// (transparent black F32; format -1 = pipeline default) into an
		// acquired slot and reports it.
		let resp = s
			.handle_line(
				r#"{"type":"render_frame","ticket":123,"time_num":1,"time_den":2,"width":4,"height":4,"format":-1}"#,
			)
			.unwrap();
		assert_eq!(resp["type"], "frame_ready", "unexpected: {resp}");
		assert_eq!(resp["ticket"], 123);
		let slot = resp["slot"].as_i64().unwrap() as u32;

		// The parent (drainer) consumes the published slot and sees the
		// meta the worker wrote.
		let parent_pool = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut consumed = 0;
		assert!(unsafe { parent_pool.consume(&mut consumed) });
		assert_eq!(consumed, slot);
		let meta = unsafe { &*parent_pool.meta_const(consumed) };
		assert_eq!(meta.id, 123);
		assert_eq!(meta.width, 4);
		assert_eq!(meta.height, 4);
		assert_eq!(meta.format, PixelFormat::F32 as i32);
		assert_eq!(meta.data_size, 4 * 4 * 16);
		// Generated frame: transparent black.
		let data = unsafe {
			std::slice::from_raw_parts(parent_pool.slot_data_const(consumed), 4 * 4 * 16)
		};
		assert!(data.iter().all(|&b| b == 0));
		unsafe { parent_pool.release(consumed) };
	}

	#[test]
	fn render_batch_stream_renders_generated_frames_and_reports_failures() {
		// The crash-mode env vars are process-wide: serialize with the
		// crash-hook test that mutates them.
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let mut s = WorkerSession::create("none").unwrap();
		// Slots sized for 8x8 BGRA8.
		let (hs, out_region, _in) = parent_side(4, 8 * 8 * 4, false);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		assert_eq!(resp["type"], crate::ipc::TYPE_HELLO_CAPS);

		let batch = json!({
			"type": "render_batch",
			"batch_id": 9,
			"tickets": [
				{ "ticket": 1, "slot": 0, "time_num": 0, "time_den": 1, "width": 8, "height": 8, "format": SLOT_FORMAT_BGRA8, "channels": 4 },
				{ "ticket": 2, "slot": 1, "time_num": 0, "time_den": 1, "width": 0, "height": 8, "format": SLOT_FORMAT_BGRA8, "channels": 4 },
			],
		});
		let mut out: Vec<u8> = Vec::new();
		s.handle_render_batch_stream(&batch.to_string(), &mut out)
			.unwrap();
		let lines: Vec<Value> = String::from_utf8(out)
			.unwrap()
			.lines()
			.map(|l| serde_json::from_str(l).unwrap())
			.collect();
		assert_eq!(lines.len(), 3, "accepted + one reply per ticket");
		assert_eq!(lines[0]["type"], "batch_accepted");
		assert_eq!(lines[0]["batch_id"], 9);
		assert_eq!(lines[1]["type"], "frame_ready");
		assert_eq!(lines[1]["ticket"], 1);
		assert_eq!(lines[1]["slot"], 0);
		assert_eq!(lines[2]["type"], "frame_failed");
		assert_eq!(lines[2]["ticket"], 2);

		// The rendered slot holds opaque black BGRA8 (generated transparent
		// black F32 converted: alpha 0).
		let parent_pool = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut consumed = 0;
		assert!(unsafe { parent_pool.consume(&mut consumed) });
		assert_eq!(consumed, 0);
		let meta = unsafe { &*parent_pool.meta_const(consumed) };
		assert_eq!(meta.id, 1);
		assert_eq!(meta.format, SLOT_FORMAT_BGRA8);
		assert_eq!(meta.linesize, 8 * 4);
		assert_eq!(meta.data_size, 8 * 8 * 4);
		unsafe { parent_pool.release(consumed) };

		// The failed ticket acquired slot 1 but never published it, so it
		// is still owned by the filler side (not in the ready ring) — the
		// ready ring is empty now.
		assert!(!unsafe { parent_pool.consume(&mut consumed) });
	}

	#[test]
	fn render_audio_batch_stream_mixes_silence_into_slot() {
		// Serialize with the crash-hook env test (see render_batch_stream).
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		// M15 S3: an empty-montage audio range pull (1/24 s at 48 kHz
		// stereo = 2000 frames x 2 ch = 16000 bytes) renders total silence
		// into the assigned slot and reports frame_ready with the audio
		// slot meta.
		let mut s = WorkerSession::create("none").unwrap();
		let slot_bytes = 2000 * 2 * 4;
		let (hs, out_region, _in) = parent_side(4, slot_bytes as i64, false);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		assert_eq!(resp["type"], crate::ipc::TYPE_HELLO_CAPS);

		let batch = json!({
			"type": "render_audio_batch",
			"batch_id": 3,
			"tickets": [
				{
					"ticket": 11,
					"slot": 0,
					"time_num": 0,
					"time_den": 1,
					"duration_num": 1,
					"duration_den": 24,
					"sample_rate": 48000,
					"channel_layout": 0x3,
					"channels": 2,
				},
				{
					"ticket": 12,
					"slot": 1,
					"time_num": 0,
					"time_den": 1,
					"duration_num": 1,
					"duration_den": 24,
					"sample_rate": 0,
					"channel_layout": 0x3,
					"channels": 2,
				},
			],
		});
		let mut out: Vec<u8> = Vec::new();
		s.handle_render_audio_batch_stream(&batch.to_string(), &mut out)
			.unwrap();
		let lines: Vec<Value> = String::from_utf8(out)
			.unwrap()
			.lines()
			.map(|l| serde_json::from_str(l).unwrap())
			.collect();
		assert_eq!(lines.len(), 3, "accepted + one reply per ticket");
		assert_eq!(lines[0]["type"], "batch_accepted");
		assert_eq!(lines[0]["batch_id"], 3);
		assert_eq!(lines[1]["type"], "frame_ready");
		assert_eq!(lines[1]["ticket"], 11);
		assert_eq!(lines[1]["slot"], 0);
		assert_eq!(lines[2]["type"], "frame_failed");
		assert_eq!(lines[2]["ticket"], 12);

		// The rendered slot holds the audio meta + silent samples.
		let parent_pool = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut consumed = 0;
		assert!(unsafe { parent_pool.consume(&mut consumed) });
		assert_eq!(consumed, 0);
		let meta = unsafe { &*parent_pool.meta_const(consumed) };
		assert_eq!(meta.id, 11);
		assert_eq!(meta.format, SLOT_FORMAT_AUDIO_F32);
		assert_eq!(meta.width, 48000, "width carries the sample rate");
		assert_eq!(meta.height, 0);
		assert_eq!(meta.channel_count, 2);
		assert_eq!(meta.linesize, 2 * 4);
		assert_eq!(meta.data_size, slot_bytes as i32);
		let data = unsafe {
			std::slice::from_raw_parts(parent_pool.slot_data_const(consumed), slot_bytes)
		};
		assert!(data.iter().all(|&b| b == 0), "empty montage is silence");
		// The samples parse back as 2000 stereo frames.
		let parsed: Vec<f32> = data
			.as_chunks::<4>()
			.0
			.iter()
			.map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
			.collect();
		assert_eq!(parsed.len(), 2000 * 2);
		assert!(parsed.iter().all(|&v| v == 0.0));
		unsafe { parent_pool.release(consumed) };

		// The failed ticket (bad sample rate) acquired slot 1 but never
		// published it.
		assert!(!unsafe { parent_pool.consume(&mut consumed) });
	}

	#[test]
	fn render_audio_batch_rejects_oversized_range() {
		// Serialize with the crash-hook env test (see render_batch_stream).
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		// A range longer than the slot can hold must fail with frame_failed
		// (never a buffer overflow into the next slot).
		let mut s = WorkerSession::create("none").unwrap();
		// Slot sized for 1/48 s of stereo audio (2000 bytes).
		let (hs, out_region, _in) = parent_side(4, 2000, false);
		let resp = s.handle_line(&hs.to_string()).expect("hello_caps response");
		assert_eq!(resp["type"], crate::ipc::TYPE_HELLO_CAPS);

		let batch = json!({
			"type": "render_audio_batch",
			"batch_id": 4,
			"tickets": [
				{
					"ticket": 21,
					"slot": 0,
					"time_num": 0,
					"time_den": 1,
					"duration_num": 1,
					"duration_den": 24,
					"sample_rate": 48000,
					"channel_layout": 0x3,
					"channels": 2,
				},
			],
		});
		let mut out: Vec<u8> = Vec::new();
		s.handle_render_audio_batch_stream(&batch.to_string(), &mut out)
			.unwrap();
		let lines: Vec<Value> = String::from_utf8(out)
			.unwrap()
			.lines()
			.map(|l| serde_json::from_str(l).unwrap())
			.collect();
		assert_eq!(lines[0]["type"], "batch_accepted");
		assert_eq!(lines[1]["type"], "frame_failed");
		assert_eq!(lines[1]["ticket"], 21);
		let error_text = lines[1]["error"].as_str().unwrap().to_string();
		assert!(
			error_text.contains("needs 16000 bytes"),
			"oversized range reported: {error_text}"
		);
		// Slot 0 acquired but never published.
		let parent_pool = unsafe { FrameSlotPool::attach(out_region.data()) };
		let mut consumed = 0;
		assert!(!unsafe { parent_pool.consume(&mut consumed) });
	}

	// ---- oak-worker's in-process session tests (M14 R2: folded from the
	// ---- former src/session.rs mirror; the facade's production session
	// ---- tests above cover the rest) -------------------------------------

	#[test]
	fn session_starts_without_pools() {
		let s = WorkerSession::create("none").unwrap();
		assert!(s.output_pool.is_none());
		assert!(s.input_pool.is_none());
		assert!(!s.shutdown_requested());
	}

	#[test]
	fn non_object_json_yields_error_response() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line("[1,2,3]").unwrap();
		assert_eq!(resp["message"], "malformed control message");
	}

	#[test]
	fn missing_type_field_yields_unknown_error() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line(r#"{"hello":1}"#).unwrap();
		assert_eq!(resp["message"], "unknown message type: ");
	}

	#[test]
	fn handshake_bad_json_shape_is_invalid_handshake() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s
			.handle_line(r#"{"type":"handshake","protocol_version":"x"}"#)
			.unwrap();
		assert_eq!(resp["message"], "invalid handshake message");
	}

	// ---- M16 R2 branch-coverage additions ---------------------------------

	/// One render-batch ticket with the pipeline defaults for everything
	/// the tests do not care about.
	fn batch_spec(ticket: i64, slot: i32, width: i32, height: i32, format: i32) -> BatchTicketSpec {
		BatchTicketSpec {
			ticket,
			slot,
			time_num: 0,
			time_den: 1,
			width,
			height,
			format,
			channels: 4,
			..Default::default()
		}
	}

	/// A project holding one empty sequence viewer; returns the project,
	/// the viewer's loaded id and the project uuid.
	fn sequence_project() -> (Arc<Mutex<Project>>, NodeId, String) {
		let project = Project::new();
		let seq;
		{
			let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
			let (core, behavior) = SequenceBehavior::create();
			seq = guard.graph.add_node(core, behavior);
		}
		let uuid = project.lock().unwrap_or_else(|e| e.into_inner()).uuid.clone();
		(project, seq, uuid)
	}

	#[test]
	fn renderer_creation_falls_back_or_succeeds_headless() {
		// On a GPU-less host the dynamic -> direct-OpenGL fallback fails and
		// the session continues headless (M16 S1); on a GPU host it simply
		// initializes. Either is a valid production outcome — creation must
		// never error out of `WorkerSession::create`, and the session's
		// renderer flag must agree with what the factory can do on this host
		// (the second `create` is the availability oracle; the renderer flag
		// is not hand-rolled anywhere else).
		let session = WorkerSession::create("auto").expect("session creation is tolerant");
		assert_eq!(
			session.has_renderer(),
			Renderer::create("auto").is_ok(),
			"the session carries a renderer exactly when the factory can create one"
		);
	}

	#[test]
	fn renderer_is_open_gl_reports_the_backend_kind() {
		let gl = Renderer {
			inner: DisplayRenderer::new(BackendKind::Gl),
		};
		assert!(gl.is_open_gl());
		let vk = Renderer {
			inner: DisplayRenderer::new(BackendKind::Vulkan),
		};
		assert!(!vk.is_open_gl());
	}

	#[test]
	fn initialize_runtime_second_call_is_a_noop() {
		let mut s = WorkerSession::create("none").unwrap();
		s.runtime_initialized = true;
		assert!(s.initialize_runtime(), "already initialized");
	}

	#[test]
	fn worker_progress_events_flush_in_order_and_cancel_is_sticky() {
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		WORKER_PROGRESS_EVENTS
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.clear();
		WORKER_PLUGIN_CANCEL.store(false, Ordering::Relaxed);
		install_worker_progress_factory();
		assert!(
			oak_plugin::progress::has_reporter_factory(),
			"the worker factory must be installed for the plugin progress suite"
		);
		// Drive progressStart -> worker factory -> update -> progressEnd
		// through the plugin progress suite, exactly like a real render.
		oak_plugin::suites::progress::set_current(Some(
			oak_plugin::progress::ProgressReporter::silent(),
		));
		let v2 = oak_plugin::suites::progress::suite_v2();
		let v1 = oak_plugin::suites::progress::suite_v1();
		let label = std::ffi::CString::new("render").unwrap();
		let message = std::ffi::CString::new("frame 1").unwrap();
		// SAFETY: the suite takes the null handle by contract; the C strings
		// outlive the calls.
		unsafe {
			assert_eq!(
				(v2.start)(std::ptr::null_mut(), label.as_ptr(), message.as_ptr()),
				oak_plugin::suites::status::OK
			);
			assert_eq!(
				(v2.update)(std::ptr::null_mut(), 0.25),
				oak_plugin::suites::status::OK
			);
			// progressEnd is forwarded by the v1 suite (v2's end is a no-op).
			assert_eq!(
				(v1.end)(std::ptr::null_mut()),
				oak_plugin::suites::status::OK
			);
		}
		oak_plugin::suites::progress::set_current(None);

		let mut out: Vec<u8> = Vec::new();
		flush_worker_progress(&mut out);
		let events: Vec<Value> = String::from_utf8(out)
			.unwrap()
			.lines()
			.map(|line| serde_json::from_str(line).unwrap())
			.collect();
		assert_eq!(events.len(), 3, "start + update + end");
		assert_eq!(events[0]["type"], crate::ipc::TYPE_PLUGIN_PROGRESS);
		assert_eq!(events[0]["label"], "render");
		assert_eq!(events[0]["message"], "frame 1");
		assert_eq!(events[0]["fraction"], 0.0);
		assert_eq!(events[1]["fraction"], 0.25);
		assert_eq!(events[2]["fraction"], 1.0);

		// plugin_cancel is sticky until a fresh progressStart resets it.
		let mut s = WorkerSession::create("none").unwrap();
		assert!(s
			.handle_line(r#"{"type":"plugin_cancel"}"#)
			.is_none());
		assert!(WORKER_PLUGIN_CANCEL.load(Ordering::Relaxed));
		WORKER_PLUGIN_CANCEL.store(false, Ordering::Relaxed);
	}

	/// A writer whose pipe is already closed.
	struct FailingWriter;

	impl Write for FailingWriter {
		fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
			Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
		}
		fn flush(&mut self) -> io::Result<()> {
			Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
		}
	}

	#[test]
	fn flush_worker_progress_breaks_on_the_first_write_error() {
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		WORKER_PROGRESS_EVENTS
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.clear();
		push_worker_progress(0.1, "a", "b");
		push_worker_progress(0.2, "c", "d");
		let mut failing = FailingWriter;
		flush_worker_progress(&mut failing);
		// The buffer is drained even though the pipe failed.
		let mut out: Vec<u8> = Vec::new();
		flush_worker_progress(&mut out);
		assert!(out.is_empty());
	}

	#[test]
	fn crash_hook_env_error_paths_are_inert() {
		// The env vars are process-wide; the batch tests that call the hook
		// take this same lock. The `EnvGuard`s restore the environment even
		// if an assertion below fails (a leaked crash-mode override could
		// abort a later serialized test).
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = WorkerSession::create("none").unwrap();

		let marker = std::env::temp_dir().join(format!(
			"oak_worker_crash_marker_{}",
			std::process::id()
		));
		let _ = std::fs::remove_file(&marker);
		let _marker_env = EnvGuard::set("OAK_WORKER_CRASH_MARKER", &marker);

		// A non-numeric ticket id is ignored (no parse, no crash, and no
		// marker file side effect).
		let _ticket = EnvGuard::set("OAK_WORKER_CRASH_ON_TICKET", "not-a-number");
		session.maybe_crash_for_testing(1);
		assert!(
			!marker.exists(),
			"a non-numeric ticket never reaches the marker write"
		);
		drop(_ticket);

		// A matching ticket whose one-shot marker already exists does not
		// crash again (the restarted worker renders for real) and leaves the
		// marker untouched.
		std::fs::write(&marker, b"crashed").unwrap();
		let _ticket = EnvGuard::set("OAK_WORKER_CRASH_ON_TICKET", i64::MIN.to_string());
		session.maybe_crash_for_testing(i64::MIN);
		assert_eq!(
			std::fs::read(&marker).unwrap(),
			b"crashed".as_slice(),
			"the existing marker is left untouched"
		);
		session.maybe_crash_for_testing(7); // different ticket: early return
		assert_eq!(
			std::fs::read(&marker).unwrap(),
			b"crashed".as_slice(),
			"a mismatching ticket writes nothing"
		);

		let _ = std::fs::remove_file(&marker);
	}

	#[test]
	fn handshake_input_attach_failure_reports_error() {
		let mut s = WorkerSession::create("none").unwrap();
		let (mut hs, _out, _in) = parent_side(2, 256, false);
		hs["input_slots"] = json!(2);
		hs["input_slot_data_bytes"] = json!(256);
		hs["input_shm_key"] = json!(format!("olive-rw-{}-missing-in", std::process::id()));
		let resp = s.handle_line(&hs.to_string()).unwrap();
		assert_eq!(resp["type"], "error");
		assert!(
			resp["message"]
				.as_str()
				.unwrap()
				.starts_with("failed to attach input shared memory: "),
			"{resp}"
		);
	}

	#[test]
	fn handshake_rejects_non_pool_input_segment() {
		let mut s = WorkerSession::create("none").unwrap();
		let (mut hs, _out, _in) = parent_side(2, 256, false);
		let key = test_key("nopool-in");
		let bytes = FrameSlotPool::bytes_needed(2, 256);
		let mut region = SharedMemoryRegion::new();
		assert!(region.open(&key, bytes, ShmMode::Create));
		hs["input_slots"] = json!(2);
		hs["input_slot_data_bytes"] = json!(256);
		hs["input_shm_key"] = json!(key);
		let resp = s.handle_line(&hs.to_string()).unwrap();
		assert_eq!(
			resp["message"],
			"input shared memory does not contain a frame slot pool"
		);
	}

	#[test]
	fn load_graph_rejects_bad_shape_and_unreadable_files() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s.handle_line(r#"{"type":"load_graph","path":42}"#).unwrap();
		assert_eq!(resp["message"], "invalid load_graph message");

		// A non-empty file that is not valid UTF-8: it passes the metadata
		// checks but cannot be read as a project.
		let unreadable = std::env::temp_dir().join(format!(
			"oak_worker_unreadable_{}.ove",
			std::process::id()
		));
		std::fs::write(&unreadable, [0xFFu8, 0xFE, 0x00, 0x80]).unwrap();
		let resp = s
			.handle_line(
				&json!({ "type": "load_graph", "path": unreadable.display().to_string() })
					.to_string(),
			)
			.unwrap();
		assert!(
			resp["message"]
				.as_str()
				.unwrap()
				.starts_with("graph file unreadable: "),
			"{resp}"
		);
		let _ = std::fs::remove_file(&unreadable);

		// The identity-only payload still lands as a graph context.
		let ident = std::env::temp_dir().join(format!(
			"oak_worker_identity_{}.ove",
			std::process::id()
		));
		std::fs::write(&ident, r#"{"project_copy":11}"#).unwrap();
		assert!(s
			.handle_line(
				&json!({ "type": "load_graph", "path": ident.display().to_string() })
					.to_string(),
			)
			.is_none());
		let graph = s.graph.as_ref().expect("identity graph loaded");
		assert!(graph.project.is_none());
		assert_eq!(graph.project_copy, 11);
		let _ = std::fs::remove_file(&ident);
	}

	#[test]
	fn render_frame_maps_every_supported_pixel_format() {
		let mut s = WorkerSession::create("none").unwrap();
		// Five slots so each render can publish without draining.
		let (hs, _out, _in) = parent_side(5, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		for format in [
			PixelFormat::U8,
			PixelFormat::U10,
			PixelFormat::U16,
			PixelFormat::F16,
			PixelFormat::F32,
		] {
			let line = json!({
				"type": "render_frame",
				"ticket": 100 + format as i64,
				"time_num": 0,
				"time_den": 1,
				"width": 2,
				"height": 2,
				"format": format as i32,
			})
			.to_string();
			let resp = s.handle_line(&line).unwrap();
			assert_eq!(resp["type"], "frame_ready", "{format:?}: {resp}");
		}
	}

	#[test]
	fn render_frame_rejects_invalid_message_and_unsupported_format() {
		let mut s = WorkerSession::create("none").unwrap();
		let resp = s
			.handle_line(r#"{"type":"render_frame","ticket":"nope"}"#)
			.unwrap();
		assert_eq!(resp["message"], "invalid render_frame message");

		let (hs, _out, _in) = parent_side(2, 256, false);
		assert_eq!(
			s.handle_line(&hs.to_string()).unwrap()["type"],
			crate::ipc::TYPE_HELLO_CAPS
		);
		let resp = s
			.handle_line(
				r#"{"type":"render_frame","ticket":9,"time_num":0,"time_den":1,"width":4,"height":4,"format":7}"#,
			)
			.unwrap();
		assert_eq!(resp["message"], "render_frame: unsupported format 7");
		assert_eq!(resp["ticket"], 9);
	}

	#[test]
	fn render_frame_defaults_zero_size_and_rejects_the_oversized_frame() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		// 0x0 means "pipeline default" (1920x1080), which cannot fit the
		// 64-byte slot.
		let resp = s
			.handle_line(
				r#"{"type":"render_frame","ticket":10,"time_num":0,"time_den":1,"width":0,"height":0,"format":-1}"#,
			)
			.unwrap();
		assert_eq!(
			resp["message"],
			"render_frame: frame larger than the shm slot"
		);
	}

	#[test]
	fn render_frame_reports_no_free_slot_on_shutdown() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 256, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		s.shutdown_requested = true;
		let resp = s
			.handle_line(
				r#"{"type":"render_frame","ticket":11,"time_num":0,"time_den":1,"width":4,"height":4,"format":-1}"#,
			)
			.unwrap();
		assert_eq!(resp["message"], "render_frame: no free shm slot");
	}

	#[test]
	fn render_frame_reports_ready_ring_full() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 256, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		// Fill the single-entry ready ring, then recycle the slot through
		// the free ring so the render can acquire it again.
		let pool = s.output_pool.clone().unwrap();
		// SAFETY: live attached pool; single-threaded test.
		unsafe {
			let mut slot = 0u32;
			assert!(pool.acquire(&mut slot));
			assert!(pool.publish(slot));
			assert!(pool.release(slot));
		}
		let resp = s
			.handle_line(
				r#"{"type":"render_frame","ticket":12,"time_num":0,"time_den":1,"width":4,"height":4,"format":-1}"#,
			)
			.unwrap();
		assert_eq!(resp["message"], "render_frame: ready ring full");
	}

	#[test]
	fn batch_and_audio_batch_reject_invalid_messages() {
		let mut s = WorkerSession::create("none").unwrap();
		let mut out: Vec<u8> = Vec::new();
		s.handle_render_batch_stream(r#"{"type":"render_batch","batch_id":"x"}"#, &mut out)
			.unwrap();
		s.handle_render_audio_batch_stream(r#"{"type":"render_audio_batch","tickets":7}"#, &mut out)
			.unwrap();
		let lines: Vec<Value> = String::from_utf8(out)
			.unwrap()
			.lines()
			.map(|line| serde_json::from_str(line).unwrap())
			.collect();
		assert_eq!(lines.len(), 2);
		assert_eq!(lines[0]["type"], "error");
		assert_eq!(lines[0]["message"], "invalid render_batch message");
		assert_eq!(lines[1]["type"], "error");
		assert_eq!(lines[1]["message"], "invalid render_audio_batch message");
	}

	#[test]
	fn render_ticket_without_pool_and_with_wrong_slot_assignment() {
		let mut s = WorkerSession::create("none").unwrap();
		let spec = batch_spec(1, 0, 4, 4, PixelFormat::F32 as i32);
		assert_eq!(
			s.render_ticket_to_slot(&spec).unwrap_err(),
			"no shared-memory pool attached"
		);

		let (hs, _out, _in) = parent_side(2, 256, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let spec = batch_spec(1, 7, 4, 4, PixelFormat::F32 as i32);
		assert_eq!(
			s.render_ticket_to_slot(&spec).unwrap_err(),
			"slot assignment mismatch: acquired 0, assigned 7"
		);
	}

	#[test]
	fn render_ticket_reports_a_full_ready_ring() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 256, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();
		// SAFETY: live attached pool; single-threaded test.
		unsafe {
			let mut slot = 0u32;
			assert!(pool.acquire(&mut slot));
			assert!(pool.publish(slot));
			assert!(pool.release(slot));
		}
		let spec = batch_spec(2, 0, 4, 4, PixelFormat::F32 as i32);
		assert_eq!(s.render_ticket_to_slot(&spec).unwrap_err(), "ready ring full");
	}

	#[test]
	fn render_spec_pixels_rejects_oversized_and_rerenders_short_cache_entries() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();

		// 4x4 F32 needs 256 bytes; the slot holds 64.
		let spec = batch_spec(3, 0, 4, 4, PixelFormat::F32 as i32);
		let err = s.render_spec_pixels(&spec, &pool).unwrap_err();
		assert!(err.starts_with("frame 4x4 needs 256 bytes"), "{err}");

		// A 2x2 frame fits exactly; a stale entry smaller than this
		// geometry is defensively discarded and re-rendered.
		let spec = batch_spec(4, 0, 2, 2, PixelFormat::F32 as i32);
		let key = crate::framecache::spec_cache_key(&spec);
		s.frame_cache.insert(key.clone(), vec![0u8; 4]);
		s.render_spec_pixels(&spec, &pool).expect("2x2 fits");
		let cached = s.frame_cache.get(&key).expect("re-rendered and memoized");
		assert_eq!(cached.len(), 64);
	}

	#[test]
	fn render_spec_pixels_reports_footage_decode_failures() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(2, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();
		let missing = "/definitely/not/a/real/clip.mp4";

		// F32 slot: the decode error propagates out of `render_f32_into`.
		let spec = BatchTicketSpec {
			footage_file: missing.to_string(),
			..batch_spec(5, 0, 2, 2, PixelFormat::F32 as i32)
		};
		let err = s.render_spec_pixels(&spec, &pool).unwrap_err();
		assert!(err.starts_with("footage decode: "), "{err}");

		// BGRA8 slot: the same failure through the scratch-buffer path.
		let spec = BatchTicketSpec {
			footage_file: missing.to_string(),
			..batch_spec(6, 1, 2, 2, SLOT_FORMAT_BGRA8)
		};
		let err = s.render_spec_pixels(&spec, &pool).unwrap_err();
		assert!(err.starts_with("footage decode: "), "{err}");
	}

	#[test]
	fn render_audio_ticket_without_pool_wrong_slot_and_full_ring() {
		let mut s = WorkerSession::create("none").unwrap();
		assert_eq!(
			s.render_audio_ticket_to_slot(&AudioTicketSpec::default())
				.unwrap_err(),
			"no shared-memory pool attached"
		);

		let (hs, _out, _in) = parent_side(2, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let spec = AudioTicketSpec {
			slot: 5,
			..Default::default()
		};
		assert_eq!(
			s.render_audio_ticket_to_slot(&spec).unwrap_err(),
			"slot assignment mismatch: acquired 0, assigned 5"
		);

		// One sample frame of stereo audio needs 8 bytes; fill the
		// single-entry ready ring first so the publish fails.
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 8, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();
		// SAFETY: live attached pool; single-threaded test.
		unsafe {
			let mut slot = 0u32;
			assert!(pool.acquire(&mut slot));
			assert!(pool.publish(slot));
			assert!(pool.release(slot));
		}
		let spec = AudioTicketSpec {
			ticket: 2,
			slot: 0,
			time_num: 0,
			time_den: 1,
			duration_num: 1,
			duration_den: 48000,
			sample_rate: 48000,
			channel_layout: 0x3,
			channels: 2,
			montage: Vec::new(),
		};
		assert_eq!(
			s.render_audio_ticket_to_slot(&spec).unwrap_err(),
			"ready ring full"
		);
	}

	#[test]
	fn audio_ticket_params_maps_montage_and_effects() {
		let s = WorkerSession::create("none").unwrap();
		let spec = AudioTicketSpec {
			ticket: 1,
			slot: 0,
			time_num: 3,
			time_den: 2,
			duration_num: 1,
			duration_den: 4,
			sample_rate: 44100,
			channel_layout: 0x3,
			channels: 2,
			montage: vec![WireMontageClip {
				filename: "clip.mp4".to_string(),
				stream_index: 2,
				in_num: 1,
				in_den: 2,
				out_num: 3,
				out_den: 2,
				media_in_num: 0,
				media_in_den: 1,
				gain: 0.5,
				effects: vec![WireMontageEffect {
					type_id: "org.oak.gain".to_string(),
					enabled: true,
					effect_input_id: "Source".to_string(),
					params: vec![WireEffectParam {
						input: "gain".to_string(),
						value: WireNodeValue::Float(2.0),
					}],
				}],
			}],
		};
		let params = s.audio_ticket_params(&spec).expect("valid geometry");
		assert_eq!(params.viewer, 0);
		assert_eq!(params.range.in_(), Rational::new(3, 2));
		assert_eq!(params.range.out(), Rational::new(7, 4));
		assert_eq!(params.sample_rate, 44100);
		assert_eq!(params.channel_layout, 0x3);
		assert_eq!(params.montage.len(), 1);
		let clip = &params.montage[0];
		assert_eq!(clip.filename, "clip.mp4");
		assert_eq!(clip.stream_index, 2);
		assert_eq!(clip.in_time, Rational::new(1, 2));
		assert_eq!(clip.out_time, Rational::new(3, 2));
		assert_eq!(clip.media_in, Rational::new(0, 1));
		assert_eq!(clip.gain, 0.5);
		assert_eq!(clip.effects.len(), 1);
		assert_eq!(clip.effects[0].type_id, "org.oak.gain");
		assert!(clip.effects[0].enabled);
		assert_eq!(clip.effects[0].effect_input_id.as_deref(), Some("Source"));
		assert_eq!(clip.effects[0].params.len(), 1);
		assert_eq!(clip.effects[0].params[0].0, "gain");
		assert_eq!(
			clip.effects[0].params[0].1,
			oak_node::value::NodeValue::Float(2.0)
		);
	}

	#[test]
	fn sync_pipeline_color_without_a_project_is_a_noop() {
		let mut s = WorkerSession::create("none").unwrap();
		assert!(!s.sync_pipeline_color_from_graph(), "no graph at all");
		s.graph = Some(LoadedGraph {
			path: "identity".to_string(),
			project: None,
			project_uuid: None,
			id_map: HashMap::new(),
			project_copy: 3,
		});
		assert!(
			!s.sync_pipeline_color_from_graph(),
			"identity-only graphs carry no color settings"
		);
	}

	#[test]
	fn render_spec_pixels_graph_mode_copies_the_sequence_frame() {
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let (project, seq, uuid) = sequence_project();
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();

		let mut graph = LoadedGraph {
			path: "graph".to_string(),
			project: Some(project),
			project_uuid: Some(uuid.clone()),
			id_map: HashMap::new(),
			project_copy: 0,
		};
		graph.id_map.insert(7, seq);
		s.graph = Some(graph);

		let spec = BatchTicketSpec {
			viewer_node: 7,
			project_key: uuid,
			..batch_spec(41, 0, 2, 2, PixelFormat::F32 as i32)
		};
		s.render_spec_pixels(&spec, &pool)
			.expect("an empty sequence renders");
		// SAFETY: slot 0 of the attached pool is live and 64 bytes.
		let dst = unsafe { std::slice::from_raw_parts(pool.slot_data_const(0), 64) };
		assert!(
			dst.iter().all(|&b| b == 0),
			"an empty sequence is transparent black"
		);
	}

	#[test]
	fn graph_mode_falls_back_for_mismatched_or_broken_viewers() {
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let (project, seq, uuid) = sequence_project();
		// A second, non-sequence node: mapping a viewer to it makes
		// `render_graph_frame` fail and take the fallback path.
		let track_list;
		{
			let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
			let (core, behavior) = TrackListBehavior::create();
			track_list = guard.graph.add_node(core, behavior);
		}
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();

		let mut graph = LoadedGraph {
			path: "graph".to_string(),
			project: Some(project),
			project_uuid: Some(uuid.clone()),
			id_map: HashMap::new(),
			project_copy: 0,
		};
		graph.id_map.insert(7, seq);
		graph.id_map.insert(8, track_list);
		s.graph = Some(graph);

		// (a) A stale project key: the snapshot must not answer it.
		let spec = BatchTicketSpec {
			viewer_node: 7,
			project_key: "another-project".to_string(),
			..batch_spec(51, 0, 2, 2, PixelFormat::F32 as i32)
		};
		s.render_spec_pixels(&spec, &pool)
			.expect("stale identity falls back");

		// (b) A viewer identity that is not even a valid NodeId.
		let spec = BatchTicketSpec {
			viewer_node: u64::MAX,
			project_key: uuid.clone(),
			..spec.clone()
		};
		s.render_spec_pixels(&spec, &pool)
			.expect("absent viewer falls back");

		// (c) A viewer mapped to a non-sequence node: graph render errors.
		let spec = BatchTicketSpec {
			viewer_node: 8,
			project_key: uuid.clone(),
			..spec.clone()
		};
		s.render_spec_pixels(&spec, &pool)
			.expect("broken viewer falls back");

		// (d) A loaded identity-only graph (uuid matches, no project).
		s.graph = Some(LoadedGraph {
			path: "identity".to_string(),
			project: None,
			project_uuid: Some(uuid.clone()),
			id_map: HashMap::new(),
			project_copy: 0,
		});
		let spec = BatchTicketSpec {
			viewer_node: 7,
			project_key: uuid,
			..spec
		};
		s.render_spec_pixels(&spec, &pool)
			.expect("identity-only graph falls back");
	}

	#[test]
	fn warn_graph_fallback_is_one_shot() {
		// Smoke test: the only observable effect of the warning is a single
		// stderr line, and the worker has no test-installable log sink, so
		// the one-shot suppression itself cannot be asserted. What must hold
		// either way: repeated and concurrent calls all return without
		// corrupting shared state (the marker is an AtomicBool and the sink
		// is stderr, so no call may deadlock or panic).
		warn_graph_fallback(123, "first reason");
		warn_graph_fallback(123, "second reason");

		let completed = std::sync::atomic::AtomicUsize::new(0);
		std::thread::scope(|scope| {
			for viewer in 0..8u64 {
				let completed = &completed;
				scope.spawn(move || {
					warn_graph_fallback(viewer, "concurrent reason");
					completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
				});
			}
		});
		assert_eq!(
			completed.load(std::sync::atomic::Ordering::Relaxed),
			8,
			"every concurrent fallback warning must return"
		);
	}

	#[test]
	fn stale_pipeline_colors_are_refreshed_and_clear_the_frame_cache() {
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		// Loop over both starting states so the opposite-selection branch
		// runs both ways despite the shared process-global color state.
		for current in [WorkingColorSpace::SrgbLegacy, WorkingColorSpace::AcesCg] {
			oak_core::color::set_pipeline_color_settings(current, OutputColorSpec::default());
			let opposite = if current == WorkingColorSpace::SrgbLegacy {
				WorkingColorSpace::AcesCg
			} else {
				WorkingColorSpace::SrgbLegacy
			};
			let project = Project::new();
			{
				let mut guard = project.lock().unwrap_or_else(|e| e.into_inner());
				guard.set_working_color_space(opposite);
			}
			let uuid = project.lock().unwrap_or_else(|e| e.into_inner()).uuid.clone();
			let mut s = WorkerSession::create("none").unwrap();
			s.graph = Some(LoadedGraph {
				path: "graph".to_string(),
				project: Some(project),
				project_uuid: Some(uuid),
				id_map: HashMap::new(),
				project_copy: 0,
			});
			s.frame_cache.insert("stale".to_string(), vec![1u8; 8]);

			let (hs, _out, _in) = parent_side(1, 64, false);
			assert!(s.handle_line(&hs.to_string()).is_some());
			let pool = s.output_pool.clone().unwrap();
			let spec = batch_spec(61, 0, 2, 2, PixelFormat::F32 as i32);
			s.render_spec_pixels(&spec, &pool)
				.expect("renders under the refreshed colors");
			assert_eq!(
				oak_core::color::pipeline_working_space(),
				opposite,
				"the loaded graph's colors are adopted"
			);
			assert!(
				s.frame_cache.get("stale").is_none(),
				"a color change invalidates the frame cache"
			);
		}

		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::default(),
			OutputColorSpec::default(),
		);
	}

	#[test]
	fn apply_output_node_is_a_noop_in_the_legacy_working_space() {
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::SrgbLegacy,
			OutputColorSpec::default(),
		);
		let mut bytes: Vec<u8> = [0.25f32, 0.5, 0.75, 1.0]
			.iter()
			.flat_map(|value| value.to_le_bytes())
			.collect();
		let before = bytes.clone();
		apply_output_node(&mut bytes, 1);
		assert_eq!(bytes, before, "legacy sRGB is display-referred already");
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::default(),
			OutputColorSpec::default(),
		);
	}

	#[test]
	fn worker_main_without_renderer_exits_one() {
		// Mirrors oakengine_worker_main(): an explicit no-renderer backend
		// leaves nothing to evaluate, so the process exits 1 before the
		// control loop.
		assert_eq!(worker_main("none"), 1);
	}

	// ---- M16 R2 branch-coverage additions (runtime + renderer) ------------

	/// The full `initialize_runtime` body on a fresh session: color config,
	/// text backends, plugin executor, OFX scan/registration and the
	/// process-global progress factory. A plugin scan failure is tolerated,
	/// so either outcome is a valid production result.
	#[test]
	fn initialize_runtime_installs_the_full_stack() {
		// The reporter factory is process-global; serialize with every
		// other factory test (worker and ofx_host share this lock).
		let _guard = GLOBAL_FACTORY_TEST_LOCK
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let mut s = WorkerSession::create("none").unwrap();
		assert!(!s.runtime_initialized);
		assert!(s.initialize_runtime(), "the runtime always initializes");
		assert!(s.runtime_initialized);
		assert!(
			oak_plugin::progress::has_reporter_factory(),
			"the worker progress factory is installed"
		);
		// The second call short-circuits before re-installing anything.
		assert!(s.initialize_runtime());
	}

	/// The dynamic / direct-OpenGL renderer factories: a GPU host
	/// initializes one, a GPU-less host fails both — either outcome is a
	/// valid production result, so the assertions pin the invariants that
	/// must hold either way: a successful dynamic attempt reports the
	/// requested backend kind (init resolves adapters internally but never
	/// rewrites the kind), a failure names the stage that failed, and the
	/// chained `create` succeeds exactly when one of its two stages can,
	/// reporting OpenGL exactly when the fallback produced it.
	#[test]
	fn renderer_create_variants_are_tolerant() {
		match Renderer::create_dynamic("gl") {
			Ok(renderer) => assert!(renderer.is_open_gl(), "the gl request reports OpenGL"),
			Err(error) => assert!(
				error.starts_with("failed to initialize dynamic gl renderer"),
				"the gl failure names the stage: {error}"
			),
		}

		let dynamic_auto = Renderer::create_dynamic("auto");
		let dynamic_metal = Renderer::create_dynamic("metal");
		for (label, attempt) in [("auto", &dynamic_auto), ("metal", &dynamic_metal)] {
			match attempt {
				Ok(renderer) => assert!(
					!renderer.is_open_gl(),
					"{label} is not an OpenGL request"
				),
				Err(error) => assert!(
					error.starts_with(&format!("failed to initialize dynamic {label} renderer")),
					"the {label} failure names the stage: {error}"
				),
			}
		}

		let opengl = Renderer::create_opengl();
		match &opengl {
			Ok(renderer) => assert!(renderer.is_open_gl(), "direct OpenGL reports OpenGL"),
			Err(error) => assert!(
				error.starts_with("failed to initialize direct OpenGL renderer"),
				"the fallback failure names the stage: {error}"
			),
		}

		// `Renderer::create` = dynamic stage, then direct-OpenGL fallback:
		// its outcome is fully determined by the two stages observed above.
		for (request, dynamic) in [("bogus", &dynamic_auto), ("metal", &dynamic_metal)] {
			match Renderer::create(request) {
				Ok(renderer) => {
					assert_eq!(
						renderer.is_open_gl(),
						dynamic.is_err(),
						"{request}: OpenGL is reported exactly when the dynamic stage failed"
					);
					if dynamic.is_err() {
						assert!(
							opengl.is_ok(),
							"{request}: the fallback must have initialized"
						);
					}
				}
				Err(error) => {
					assert!(
						dynamic.is_err() && opengl.is_err(),
						"{request}: a chained error means both stages failed"
					);
					assert!(
						error.contains(&format!("dynamic {request}"))
							&& error.contains("direct OpenGL fallback also failed"),
						"{request}: the chained error names both stages: {error}"
					);
				}
			}
		}
	}

	/// An F32 cache hit copies the memoized bytes into the slot instead of
	/// re-rendering (a miss would produce transparent black).
	#[test]
	fn render_spec_pixels_f32_cache_hit_copies_without_render() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();
		let spec = batch_spec(71, 0, 2, 2, PixelFormat::F32 as i32);
		let key = crate::framecache::spec_cache_key(&spec);
		let known: Vec<u8> = (0..64).map(|i| i as u8).collect();
		s.frame_cache.insert(key.clone(), known.clone());
		s.render_spec_pixels(&spec, &pool).expect("cache hit");
		// SAFETY: slot 0 of the attached pool is live and 64 bytes.
		let dst = unsafe { std::slice::from_raw_parts(pool.slot_data_const(0), 64) };
		assert_eq!(dst, &known[..], "the cached F32 bytes land in the slot");
		assert!(s.frame_cache.get(&key).is_some(), "the entry stays memoized");
	}

	/// A BGRA8 cache hit runs the output node + format convert on the
	/// memoized F32 bytes (a miss would produce transparent black BGRA8).
	#[test]
	fn render_spec_pixels_bgra8_cache_hit_converts_the_memoized_frame() {
		let _color = COLOR_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::SrgbLegacy,
			OutputColorSpec::default(),
		);
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		let pool = s.output_pool.clone().unwrap();
		let spec = batch_spec(72, 0, 2, 2, SLOT_FORMAT_BGRA8);
		let key = crate::framecache::spec_cache_key(&spec);
		// Four opaque-white F32 pixels.
		let white: Vec<u8> = std::iter::repeat_n(1.0f32.to_le_bytes(), 2 * 2 * 4)
			.flatten()
			.collect();
		s.frame_cache.insert(key.clone(), white);
		s.render_spec_pixels(&spec, &pool).expect("cache hit");
		// SAFETY: slot 0 of the attached pool holds the 2x2 BGRA8 frame.
		let dst = unsafe { std::slice::from_raw_parts(pool.slot_data_const(0), 2 * 2 * 4) };
		assert!(
			dst.iter().all(|&b| b == 255),
			"cached white F32 converts to opaque white BGRA8: {dst:?}"
		);
		oak_core::color::set_pipeline_color_settings(
			WorkingColorSpace::default(),
			OutputColorSpec::default(),
		);
	}

	/// The batch path's acquire failure (shutdown) and a render error both
	/// surface as `Err` from `render_ticket_to_slot` (the slot is never
	/// published in either case).
	#[test]
	fn render_ticket_to_slot_reports_shutdown_and_render_errors() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(2, 256, false);
		assert!(s.handle_line(&hs.to_string()).is_some());

		// Shutdown: `acquire_slot` refuses before touching the rings.
		s.shutdown_requested = true;
		let spec = batch_spec(1, 0, 4, 4, PixelFormat::F32 as i32);
		assert_eq!(
			s.render_ticket_to_slot(&spec).unwrap_err(),
			"no free shm slot (shutdown or timeout)"
		);
		s.shutdown_requested = false;

		// A render failure (missing footage) propagates out of the batch
		// path unchanged.
		let spec = BatchTicketSpec {
			footage_file: "/definitely/not/a/real/clip.mp4".to_string(),
			..batch_spec(2, 0, 2, 2, PixelFormat::F32 as i32)
		};
		let err = s.render_ticket_to_slot(&spec).unwrap_err();
		assert!(err.starts_with("footage decode: "), "{err}");
	}

	/// The audio batch path's acquire failure: a shutdown session returns
	/// "no free shm slot" before mixing anything.
	#[test]
	fn render_audio_ticket_to_slot_reports_shutdown() {
		let mut s = WorkerSession::create("none").unwrap();
		let (hs, _out, _in) = parent_side(1, 64, false);
		assert!(s.handle_line(&hs.to_string()).is_some());
		s.shutdown_requested = true;
		let spec = AudioTicketSpec {
			ticket: 5,
			slot: 0,
			time_num: 0,
			time_den: 1,
			duration_num: 1,
			duration_den: 48000,
			sample_rate: 48000,
			channel_layout: 0x3,
			channels: 2,
			montage: Vec::new(),
		};
		assert_eq!(
			s.render_audio_ticket_to_slot(&spec).unwrap_err(),
			"no free shm slot (shutdown or timeout)"
		);
	}
}

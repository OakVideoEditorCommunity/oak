//! Plugin process lifecycle: spawn → handshake → ready → crash/stop, with
//! bounded restarts, heartbeat liveness and stdout discipline (OPP/1 §4).
//!
//! # The 11 states
//!
//! | state | meaning |
//! |---|---|
//! | `Discovered` | manifest parsed, not started yet |
//! | `Enabling` | start/enable accepted, argv+env resolved |
//! | `Spawning` | child spawned, waiting for the first message (10 s) |
//! | `Handshaking` | first message arrived, `session.hello` being validated |
//! | `Ready` | granted; heartbeat + request pump running |
//! | `Crashed` | EOF/exit/3× heartbeat miss, restart decision pending |
//! | `Restarting` | exponential backoff before re-spawn |
//! | `Stopping` | `session.shutdown` sent, 2 s + 2 s kill ladder running |
//! | `Stopped` | exited (cleanly or after the ladder) |
//! | `Failed` | terminal failure (spawn/handshake/stdout/max-restarts) |
//! | `Disabled` | user disabled the plugin; not running |
//!
//! The frozen spec lives at `~/.oak-reviews/opp-spec.md` (persistent; md5
//! 976f16a8…) — a `/tmp/oak-reviews/` copy also exists, but /tmp may be wiped
//! (it was, on 2026-11-10). Its §2.2 diagram draws all eleven states: the
//! middle nine, with `Enabling`/`Disabled` closing the lifecycle around the
//! user-enable / user-disable edges. Timings below
//! are the frozen defaults (10 s handshake, 2 s/5 s/3 heartbeat, 5 restarts
//! at 1 s doubling backoff, 2 s + 2 s shutdown ladder).

use crate::manifest::Manifest;
use crate::protocol::{
	encode_line, ping_request, shutdown_notification, FramedReader, FrameError, HelloParams,
	HelloResult, MsgId, Request, Response, API_VERSION, INVALID_PARAMS, INVALID_REQUEST,
};
use crate::session::{handle_host_request, HostSession, RpcError};
use serde::Serialize;
use serde_json::{json, Value};
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
	Discovered,
	Enabling,
	Spawning,
	Handshaking,
	Ready,
	Crashed,
	Restarting,
	Stopping,
	Stopped,
	Failed,
	Disabled,
}

impl PluginState {
	/// No process, no pending transition until the user acts.
	pub fn is_terminal(self) -> bool {
		matches!(
			self,
			PluginState::Stopped | PluginState::Failed | PluginState::Disabled
		)
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
	/// `Command::spawn` failed (bad argv, missing interpreter, …).
	SpawnFailed(String),
	/// No first message within `handshake_timeout` (§4.1: 10 s).
	HandshakeTimeout,
	/// First frame was not a `session.hello` request (§4.1).
	FirstMessageNotHello,
	/// `session.hello` rejected (api / capabilities / panels mismatch).
	HandshakeRejected(String),
	/// Non-protocol bytes on stdout (§1 stdout discipline).
	StdoutViolation(String),
	/// A single line exceeded 16 MiB (§1) — killed on the spot.
	MessageTooLarge { bytes: usize },
	/// Three consecutive heartbeat timeouts (§4.2).
	HeartbeatMissed,
	/// Child closed stdout / exited.
	Exited(Option<i32>),
	/// `restart.max` exhausted (§2.2).
	MaxRestartsExceeded,
	Io(String),
}

impl fmt::Display for FailureReason {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			FailureReason::SpawnFailed(e) => write!(f, "spawn failed: {e}"),
			FailureReason::HandshakeTimeout => write!(f, "handshake timed out"),
			FailureReason::FirstMessageNotHello => write!(f, "first message was not session.hello"),
			FailureReason::HandshakeRejected(e) => write!(f, "handshake rejected: {e}"),
			FailureReason::StdoutViolation(e) => write!(f, "stdout violation: {e}"),
			FailureReason::MessageTooLarge { bytes } => {
				write!(f, "message of {bytes} bytes exceeds the 16 MiB cap")
			}
			FailureReason::HeartbeatMissed => write!(f, "heartbeat missed 3 times"),
			FailureReason::Exited(None) => write!(f, "plugin exited"),
			FailureReason::Exited(Some(c)) => write!(f, "plugin exited with code {c}"),
			FailureReason::MaxRestartsExceeded => write!(f, "restart budget exhausted"),
			FailureReason::Io(e) => write!(f, "i/o error: {e}"),
		}
	}
}

/// How the current child process was (or is being) terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillMethod {
	/// It exited on its own (clean path).
	None,
	Sigterm,
	Sigkill,
}

/// Frozen timing defaults; override per test / per manifest limits (P1.2).
#[derive(Debug, Clone)]
pub struct LifecycleConfig {
	pub handshake_timeout: Duration,
	pub heartbeat_interval: Duration,
	pub heartbeat_timeout: Duration,
	pub heartbeat_misses: u32,
	pub shutdown_grace: Duration,
	pub shutdown_kill_wait: Duration,
	pub oak_version: String,
	/// `session.hello` result `features` (empty in P1: no shm/ui.pixel yet).
	pub features: Vec<String>,
}

impl Default for LifecycleConfig {
	fn default() -> Self {
		Self {
			handshake_timeout: Duration::from_secs(10),
			heartbeat_interval: Duration::from_secs(2),
			heartbeat_timeout: Duration::from_secs(5),
			heartbeat_misses: 3,
			shutdown_grace: Duration::from_secs(2),
			shutdown_kill_wait: Duration::from_secs(2),
			oak_version: env!("CARGO_PKG_VERSION").to_string(),
			features: Vec::new(),
		}
	}
}

#[derive(Debug)]
pub enum StartError {
	Manifest(String),
	Io(String),
}

impl fmt::Display for StartError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			StartError::Manifest(e) => write!(f, "invalid manifest: {e}"),
			StartError::Io(e) => write!(f, "cannot start supervisor: {e}"),
		}
	}
}

impl std::error::Error for StartError {}

// ---------------------------------------------------------------------------
// Internal event plumbing
// ---------------------------------------------------------------------------

enum Ev {
	Line(Message),
	Bad(BadKind),
	Eof,
}

use crate::protocol::Message;

enum BadKind {
	TooLarge { bytes: usize },
	Garbage(String),
}

enum Ctrl {
	Shutdown(String),
	Disable,
	Enable,
}

struct StateCell {
	state: PluginState,
	failure: Option<FailureReason>,
}

struct Inner {
	manifest: Manifest,
	plugin_dir: PathBuf,
	env_base: Vec<(String, String)>,
	config: LifecycleConfig,
	state: Mutex<StateCell>,
	session: Arc<HostSession>,
	/// `Some` only while a child lives; every outbound frame goes through it.
	stdin: Mutex<Option<std::process::ChildStdin>>,
	spawn_count: AtomicUsize,
	heartbeat_ok: AtomicUsize,
	ping_seq: AtomicU64,
	kill_method: Mutex<KillMethod>,
	/// Captured plugin stderr (bounded ring; P1.2 writes it to
	/// `$OAK_LOG_DIR`).
	stderr: Mutex<Vec<String>>,
	exit_summary: Mutex<Option<String>>,
	pid: Mutex<Option<u32>>,
	statesubs: Mutex<Vec<Sender<PluginState>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
	m.lock().unwrap_or_else(|p| p.into_inner())
}

fn transition(inner: &Inner, state: PluginState, failure: Option<FailureReason>) {
	{
		let mut cell = lock(&inner.state);
		cell.state = state;
		cell.failure = failure;
	}
	let mut subs = lock(&inner.statesubs);
	subs.retain(|tx| tx.send(state).is_ok());
}

// ---------------------------------------------------------------------------
// PluginHost — the public handle
// ---------------------------------------------------------------------------

/// Handle to one supervised plugin process.
///
/// All methods are safe to call from any thread; state transitions are
/// published through [`PluginHost::subscribe_states`].
pub struct PluginHost {
	inner: Arc<Inner>,
	ctrl: Option<Sender<Ctrl>>,
	supervisor: Option<JoinHandle<()>>,
}

impl PluginHost {
	/// Validate the manifest and start the supervisor thread; the child is
	/// spawned immediately (`Discovered → Enabling → Spawning → …`).
	pub fn start(
		manifest: Manifest,
		plugin_dir: impl Into<PathBuf>,
		config: LifecycleConfig,
	) -> Result<PluginHost, StartError> {
		manifest
			.validate()
			.map_err(|e| StartError::Manifest(e.to_string()))?;
		let inner = Arc::new(Inner {
			manifest,
			plugin_dir: plugin_dir.into(),
			env_base: std::env::vars().collect(),
			config,
			state: Mutex::new(StateCell {
				state: PluginState::Discovered,
				failure: None,
			}),
			session: Arc::new(HostSession::new()),
			stdin: Mutex::new(None),
			spawn_count: AtomicUsize::new(0),
			heartbeat_ok: AtomicUsize::new(0),
			ping_seq: AtomicU64::new(1),
			kill_method: Mutex::new(KillMethod::None),
			stderr: Mutex::new(Vec::new()),
			exit_summary: Mutex::new(None),
			pid: Mutex::new(None),
			statesubs: Mutex::new(Vec::new()),
		});
		let (ev_tx, ev_rx) = mpsc::channel::<Ev>();
		let (ctrl_tx, ctrl_rx) = mpsc::channel::<Ctrl>();
		let sup_inner = inner.clone();
		let supervisor = thread::Builder::new()
			.name("opp-supervisor".to_string())
			.spawn(move || supervise(sup_inner, ev_rx, ctrl_rx, ev_tx))
			.map_err(|e| StartError::Io(e.to_string()))?;
		Ok(PluginHost {
			inner,
			ctrl: Some(ctrl_tx),
			supervisor: Some(supervisor),
		})
	}

	pub fn state(&self) -> PluginState {
		lock(&self.inner.state).state
	}

	pub fn failure(&self) -> Option<FailureReason> {
		lock(&self.inner.state).failure.clone()
	}

	/// Total spawns across all lives of this handle (initial + restarts).
	pub fn spawn_count(&self) -> usize {
		self.inner.spawn_count.load(Ordering::SeqCst)
	}

	/// Heartbeat responses received since the last spawn.
	pub fn heartbeat_ok(&self) -> usize {
		self.inner.heartbeat_ok.load(Ordering::SeqCst)
	}

	pub fn kill_method(&self) -> KillMethod {
		*lock(&self.inner.kill_method)
	}

	pub fn stderr(&self) -> Vec<String> {
		lock(&self.inner.stderr).clone()
	}

	pub fn exit_summary(&self) -> Option<String> {
		lock(&self.inner.exit_summary).clone()
	}

	pub fn pid(&self) -> Option<u32> {
		*lock(&self.inner.pid)
	}

	pub fn session(&self) -> Arc<HostSession> {
		self.inner.session.clone()
	}

	/// Receiver that sees the current state first, then every transition.
	pub fn subscribe_states(&self) -> Receiver<PluginState> {
		let (tx, rx) = mpsc::channel();
		let current = lock(&self.inner.state).state;
		let _ = tx.send(current);
		lock(&self.inner.statesubs).push(tx);
		rx
	}

	/// Ask the plugin to exit (§4.4): `session.shutdown` + kill ladder.
	pub fn shutdown(&self, reason: &str) {
		if let Some(ctrl) = &self.ctrl {
			let _ = ctrl.send(Ctrl::Shutdown(reason.to_string()));
		}
	}

	/// User disable: same ladder, ends in [`PluginState::Disabled`].
	pub fn disable(&self) {
		if let Some(ctrl) = &self.ctrl {
			let _ = ctrl.send(Ctrl::Disable);
		}
	}

	/// Re-enable after a terminal state (restarts the crash budget).
	pub fn enable(&self) {
		if let Some(ctrl) = &self.ctrl {
			let _ = ctrl.send(Ctrl::Enable);
		}
	}

	/// Poll until `pred(state)` holds or `timeout` elapses.
	pub fn wait_state<F: Fn(PluginState) -> bool>(&self, pred: F, timeout: Duration) -> bool {
		let deadline = Instant::now() + timeout;
		loop {
			if pred(self.state()) {
				return true;
			}
			if Instant::now() >= deadline {
				return false;
			}
			thread::sleep(Duration::from_millis(5));
		}
	}

	pub fn wait_terminal(&self, timeout: Duration) -> bool {
		self.wait_state(|s| s.is_terminal(), timeout)
	}

	/// Host → plugin request (in-flight paired, `timeout` defaults to the
	/// protocol's 120 s — pass [`crate::session::DEFAULT_REQUEST_TIMEOUT`]).
	pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
		if self.state() != PluginState::Ready {
			return Err(RpcError::NotReady);
		}
		let id = self.inner.session.take_request_id();
		let rx = self.inner.session.register(&id);
		let req = Request::new(id.clone(), method, params);
		if let Err(e) = send_to_child(&self.inner, &req) {
			self.inner.session.cancel(&id);
			return Err(RpcError::Io(e));
		}
		HostSession::wait(rx, timeout)
	}
}

impl Drop for PluginHost {
	fn drop(&mut self) {
		// Closing the control channel (after an explicit shutdown request)
		// is what tells the supervisor to tear the child down and exit.
		if let Some(ctrl) = self.ctrl.take() {
			let _ = ctrl.send(Ctrl::Shutdown("host_drop".to_string()));
		}
		if let Some(handle) = self.supervisor.take() {
			let _ = handle.join();
		}
	}
}

// ---------------------------------------------------------------------------
// Supervisor thread
// ---------------------------------------------------------------------------

enum First {
	Msg(Message),
	Fail(FailureReason),
	/// Shutdown/disable arrived before the handshake completed.
	Abort(String),
}

enum Attempt {
	Ready,
	HandshakeFailed(FailureReason),
	Stopped(String),
}

enum ReadyExit {
	Crashed(FailureReason),
	Violation(BadKind),
	Stop(String),
}

fn supervise(inner: Arc<Inner>, ev_rx: Receiver<Ev>, ctrl_rx: Receiver<Ctrl>, ev_tx: Sender<Ev>) {
	let cfg = inner.config.clone();
	let mut restarts: u32 = 0;
	'outer: loop {
		// ---- Enabling: resolve argv + whitelisted env ----
		transition(&inner, PluginState::Enabling, None);
		let argv = inner.manifest.resolve_command(&inner.plugin_dir);
		let env = inner.manifest.resolve_env(&inner.env_base);

		// ---- Spawning ----
		transition(&inner, PluginState::Spawning, None);
		inner.session.reset_subscriptions();
		let mut child = match spawn_child(&argv, &env, &inner.plugin_dir) {
			Ok(c) => c,
			Err(e) => {
				transition(
					&inner,
					PluginState::Failed,
					Some(FailureReason::SpawnFailed(e)),
				);
				if reenable(&inner, &ctrl_rx) {
					restarts = 0;
					continue 'outer;
				}
				break 'outer;
			}
		};
		inner.spawn_count.fetch_add(1, Ordering::SeqCst);
		inner.heartbeat_ok.store(0, Ordering::SeqCst);
		*lock(&inner.pid) = Some(child.id());
		*lock(&inner.kill_method) = KillMethod::None;
		*lock(&inner.exit_summary) = None;

		let stdout = child.stdout.take().expect("stdout piped");
		let stderr_pipe = child.stderr.take().expect("stderr piped");
		let stdin = child.stdin.take().expect("stdin piped");
		*lock(&inner.stdin) = Some(stdin);
		let readers = vec![
			thread::spawn({
				let tx = ev_tx.clone();
				move || read_protocol(tx, stdout)
			}),
			thread::spawn({
				let inner = inner.clone();
				move || read_stderr(inner, stderr_pipe)
			}),
		];

		// ---- first message within handshake_timeout (10 s) ----
		let first = recv_first(&ev_rx, &ctrl_rx, cfg.handshake_timeout);
		let outcome = match first {
			First::Abort(reason) => Attempt::Stopped(reason),
			First::Fail(reason) => Attempt::HandshakeFailed(reason),
			First::Msg(m) => {
				transition(&inner, PluginState::Handshaking, None);
				match m {
					Message::Request(req) if req.method == "session.hello" => {
						match validate_hello(&inner, &req) {
							Ok((resp, hello)) => match send_to_child(&inner, &resp) {
								Ok(()) => {
									for ev in &hello.subscribe {
										inner.session.subscribe(ev);
									}
									transition(&inner, PluginState::Ready, None);
									Attempt::Ready
								}
								Err(e) => Attempt::HandshakeFailed(FailureReason::Io(e)),
							},
							Err((resp, reason)) => {
								// §4.1: answer with INVALID_PARAMS, then kill.
								let _ = send_to_child(&inner, &resp);
								Attempt::HandshakeFailed(reason)
							}
						}
					}
					Message::Request(req) => {
						let _ = send_to_child(
							&inner,
							&Response::failure(
								req.id,
								INVALID_REQUEST,
								"first message must be session.hello",
							),
						);
						Attempt::HandshakeFailed(FailureReason::FirstMessageNotHello)
					}
					_ => Attempt::HandshakeFailed(FailureReason::FirstMessageNotHello),
				}
			}
		};

		match outcome {
			Attempt::Ready => {}
			Attempt::HandshakeFailed(reason) => {
				reap_or_kill(&inner, &mut child, readers);
				inner.session.abort_all(RpcError::PluginDead);
				transition(&inner, PluginState::Failed, Some(reason));
				if reenable(&inner, &ctrl_rx) {
					restarts = 0;
					continue 'outer;
				}
				break 'outer;
			}
			Attempt::Stopped(reason) => {
				stop_sequence(&inner, &mut child, &ev_rx, readers, &cfg, &reason, false);
				inner.session.abort_all(RpcError::PluginDead);
				transition(&inner, final_state_for(&reason), None);
				if reenable(&inner, &ctrl_rx) {
					restarts = 0;
					continue 'outer;
				}
				break 'outer;
			}
		}

		// ---- Ready: heartbeat + bidirectional pump ----
		match ready_loop(&inner, &ev_rx, &ctrl_rx) {
			ReadyExit::Crashed(reason) => {
				transition(&inner, PluginState::Crashed, Some(reason));
				reap_or_kill(&inner, &mut child, readers);
				inner.session.abort_all(RpcError::PluginDead);
				if restarts < inner.manifest.restart.max {
					restarts += 1;
					transition(&inner, PluginState::Restarting, None);
					let shift = u32::from((restarts - 1).min(20));
					let backoff = Duration::from_millis(
						inner
							.manifest
							.restart
							.backoff_ms
							.saturating_mul(1u64 << shift),
					);
					match sleep_interruptible(backoff, &ctrl_rx) {
						None => continue 'outer,
						Some(reason) => {
							transition(&inner, final_state_for(&reason), None);
							if reenable(&inner, &ctrl_rx) {
								restarts = 0;
								continue 'outer;
							}
							break 'outer;
						}
					}
				} else {
					transition(
						&inner,
						PluginState::Failed,
						Some(FailureReason::MaxRestartsExceeded),
					);
					if reenable(&inner, &ctrl_rx) {
						restarts = 0;
						continue 'outer;
					}
					break 'outer;
				}
			}
			ReadyExit::Violation(bad) => {
				// §1: protocol error on stdout → kill, no restart.
				let reason = bad_reason(bad);
				reap_or_kill(&inner, &mut child, readers);
				inner.session.abort_all(RpcError::PluginDead);
				transition(&inner, PluginState::Failed, Some(reason));
				if reenable(&inner, &ctrl_rx) {
					restarts = 0;
					continue 'outer;
				}
				break 'outer;
			}
			ReadyExit::Stop(reason) => {
				stop_sequence(&inner, &mut child, &ev_rx, readers, &cfg, &reason, true);
				inner.session.abort_all(RpcError::PluginDead);
				transition(&inner, final_state_for(&reason), None);
				if reenable(&inner, &ctrl_rx) {
					restarts = 0;
					continue 'outer;
				}
				break 'outer;
			}
		}
	}
}

fn final_state_for(reason: &str) -> PluginState {
	if reason == "disabled" {
		PluginState::Disabled
	} else {
		PluginState::Stopped
	}
}

fn bad_reason(b: BadKind) -> FailureReason {
	match b {
		BadKind::TooLarge { bytes } => FailureReason::MessageTooLarge { bytes },
		BadKind::Garbage(s) => FailureReason::StdoutViolation(s),
	}
}

/// Block until the user re-enables (`true`) or the handle goes away / the
/// plugin is shut down (`false`). Also flips `Failed → Disabled` on a
/// user-disable while parked.
fn reenable(inner: &Arc<Inner>, ctrl_rx: &Receiver<Ctrl>) -> bool {
	match ctrl_rx.recv() {
		Ok(Ctrl::Enable) => true,
		Ok(Ctrl::Disable) => {
			transition(inner, PluginState::Disabled, None);
			false
		}
		Ok(Ctrl::Shutdown(_)) | Err(_) => false,
	}
}

// ---------------------------------------------------------------------------
// Phases
// ---------------------------------------------------------------------------

fn recv_first(
	ev_rx: &Receiver<Ev>,
	ctrl_rx: &Receiver<Ctrl>,
	timeout: Duration,
) -> First {
	let deadline = Instant::now() + timeout;
	loop {
		match ctrl_rx.try_recv() {
			Ok(Ctrl::Shutdown(r)) => return First::Abort(r),
			Ok(Ctrl::Disable) => return First::Abort("disabled".to_string()),
			Ok(Ctrl::Enable) => {}
			Err(TryRecvError::Empty) => {}
			Err(TryRecvError::Disconnected) => {
				return First::Abort("host_drop".to_string());
			}
		}
		let now = Instant::now();
		if now >= deadline {
			return First::Fail(FailureReason::HandshakeTimeout);
		}
		let slice = (deadline - now).min(Duration::from_millis(100));
		match ev_rx.recv_timeout(slice) {
			Ok(Ev::Line(m)) => return First::Msg(m),
			Ok(Ev::Bad(b)) => return First::Fail(bad_reason(b)),
			Ok(Ev::Eof) => return First::Fail(FailureReason::Exited(None)),
			Err(RecvTimeoutError::Timeout) => {}
			Err(RecvTimeoutError::Disconnected) => {
				return First::Fail(FailureReason::Exited(None));
			}
		}
	}
}

/// Validate `session.hello` against the manifest (§4.1): api, capability
/// subset, panel equality. Returns the response + parsed params on success.
fn validate_hello(
	inner: &Inner,
	req: &Request,
) -> Result<(Response, HelloParams), (Response, FailureReason)> {
	let params: HelloParams = match serde_json::from_value(req.params.clone()) {
		Ok(p) => p,
		Err(e) => {
			return Err((
				Response::failure_data(
					req.id.clone(),
					INVALID_PARAMS,
					"malformed session.hello",
					json!({ "detail": e.to_string() }),
				),
				FailureReason::HandshakeRejected("malformed session.hello".into()),
			));
		}
	};
	if params.api != API_VERSION {
		return Err((
			Response::failure_data(
				req.id.clone(),
				INVALID_PARAMS,
				"unsupported api",
				json!({ "supported_api": API_VERSION }),
			),
			FailureReason::HandshakeRejected(format!(
				"plugin api {} != host api {API_VERSION}",
				params.api
			)),
		));
	}
	if let Some(bad) = params
		.capabilities
		.iter()
		.find(|c| !inner.manifest.capabilities.contains(*c))
	{
		return Err((
			Response::failure_data(
				req.id.clone(),
				INVALID_PARAMS,
				"capability not declared in manifest",
				json!({ "capability": bad }),
			),
			FailureReason::HandshakeRejected(format!("undeclared capability {bad:?}")),
		));
	}
	if params.panels != inner.manifest.panels {
		return Err((
			Response::failure_data(
				req.id.clone(),
				INVALID_PARAMS,
				"panels differ from manifest",
				json!({ "manifest": inner.manifest.panels, "hello": params.panels }),
			),
			FailureReason::HandshakeRejected("panels differ from manifest".into()),
		));
	}
	let result = HelloResult {
		api: API_VERSION,
		oak_version: inner.config.oak_version.clone(),
		granted: params.capabilities.clone(),
		features: inner.config.features.clone(),
		shm: None,
	};
	let resp = Response::success(
		req.id.clone(),
		serde_json::to_value(result).expect("HelloResult serializes"),
	);
	Ok((resp, params))
}

/// Ready-state pump: pairs responses, dispatches plugin requests under the
/// gate, runs the 2 s/5 s/3 heartbeat, and watches control + stdout.
fn ready_loop(inner: &Arc<Inner>, ev_rx: &Receiver<Ev>, ctrl_rx: &Receiver<Ctrl>) -> ReadyExit {
	let cfg = &inner.config;
	let mut next_ping = Instant::now() + cfg.heartbeat_interval;
	let mut awaiting: Option<(MsgId, Instant)> = None;
	let mut misses: u32 = 0;
	loop {
		let now = Instant::now();
		let mut wait = next_ping.saturating_duration_since(now);
		if let Some((_, deadline)) = &awaiting {
			wait = wait.min(deadline.saturating_duration_since(now));
		}
		// Bound every sleep so shutdown/disable stay responsive.
		wait = wait.min(Duration::from_millis(100));
		match ev_rx.recv_timeout(wait) {
			Ok(Ev::Line(Message::Response(resp))) => {
				if let Some((ping_id, _)) = &awaiting
					&& resp.id == *ping_id
				{
					awaiting = None;
					misses = 0;
					inner.heartbeat_ok.fetch_add(1, Ordering::SeqCst);
					continue;
				}
				inner.session.complete(&resp);
			}
			Ok(Ev::Line(Message::Request(req))) => {
				// §2: dispatch serialized (arrival order); P1.2 extends the
				// gate across engine execution for mutations.
				let _gate = inner
					.session
					.dispatch_gate
					.lock()
					.unwrap_or_else(|p| p.into_inner());
				let resp = handle_host_request(&inner.session, &req);
				if let Err(e) = send_to_child(inner, &resp) {
					return ReadyExit::Crashed(FailureReason::Io(e));
				}
			}
			Ok(Ev::Line(Message::Notification(_))) => {
				// P2: ui.event routing. P1 has no consumer yet.
			}
			Ok(Ev::Bad(b)) => return ReadyExit::Violation(b),
			Ok(Ev::Eof) => return ReadyExit::Crashed(FailureReason::Exited(None)),
			Err(RecvTimeoutError::Timeout) => {}
			Err(RecvTimeoutError::Disconnected) => {
				return ReadyExit::Crashed(FailureReason::Exited(None));
			}
		}
		match ctrl_rx.try_recv() {
			Ok(Ctrl::Shutdown(r)) => return ReadyExit::Stop(r),
			Ok(Ctrl::Disable) => return ReadyExit::Stop("disabled".to_string()),
			Ok(Ctrl::Enable) => {}
			Err(TryRecvError::Empty) => {}
			Err(TryRecvError::Disconnected) => return ReadyExit::Stop("host_drop".to_string()),
		}
		let now = Instant::now();
		if let Some((_, deadline)) = &awaiting
			&& now >= *deadline
		{
			awaiting = None;
			misses += 1;
			if misses >= cfg.heartbeat_misses {
				return ReadyExit::Crashed(FailureReason::HeartbeatMissed);
			}
		}
		if awaiting.is_none() && now >= next_ping {
			let ping = ping_request(inner.ping_seq.fetch_add(1, Ordering::SeqCst));
			let id = ping.id.clone();
			if let Err(e) = send_to_child(inner, &ping) {
				return ReadyExit::Crashed(FailureReason::Io(e));
			}
			awaiting = Some((id, now + cfg.heartbeat_timeout));
			next_ping = now + cfg.heartbeat_interval;
		}
	}
}

// ---------------------------------------------------------------------------
// Child management
// ---------------------------------------------------------------------------

fn spawn_child(
	argv: &[String],
	env: &[(String, String)],
	cwd: &Path,
) -> Result<Child, String> {
	let (prog, rest) = argv
		.split_first()
		.ok_or_else(|| "empty process.command".to_string())?;
	let mut cmd = Command::new(prog);
	cmd.args(rest)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.env_clear();
	for (k, v) in env {
		cmd.env(k, v);
	}
	cmd.current_dir(cwd);
	cmd.spawn().map_err(|e| format!("spawn {prog}: {e}"))
}

fn send_to_child<M: Serialize>(inner: &Arc<Inner>, msg: &M) -> Result<(), String> {
	let bytes = encode_line(msg).map_err(|e| format!("encode: {e}"))?;
	let mut guard = lock(&inner.stdin);
	match guard.as_mut() {
		Some(s) => s
			.write_all(&bytes)
			.and_then(|()| s.flush())
			.map_err(|e| e.to_string()),
		None => Err("stdin closed".to_string()),
	}
}

/// stdout reader: frames → supervisor; anything non-protocol is a violation,
/// an over-long line aborts at the cap without buffering the whole message.
fn read_protocol(tx: Sender<Ev>, stdout: ChildStdout) {
	let mut reader = FramedReader::new(stdout);
	loop {
		match reader.read_message() {
			Ok(Some(msg)) => {
				if tx.send(Ev::Line(msg)).is_err() {
					return;
				}
			}
			Ok(None) => {
				let _ = tx.send(Ev::Eof);
				return;
			}
			Err(FrameError::Truncated) | Err(FrameError::Io(_)) => {
				let _ = tx.send(Ev::Eof);
				return;
			}
			Err(FrameError::TooLarge { bytes }) => {
				let _ = tx.send(Ev::Bad(BadKind::TooLarge { bytes }));
				return;
			}
			Err(e) => {
				let _ = tx.send(Ev::Bad(BadKind::Garbage(e.to_string())));
				return;
			}
		}
	}
}

/// stderr capture (§1: plugin logs never go to stdout).
fn read_stderr(inner: Arc<Inner>, stderr: ChildStderr) {
	let mut reader = std::io::BufReader::new(stderr);
	let mut line = String::new();
	loop {
		line.clear();
		match reader.read_line(&mut line) {
			Ok(0) | Err(_) => return,
			Ok(_) => {
				let mut logs = lock(&inner.stderr);
				if logs.len() >= 1000 {
					logs.remove(0);
				}
				logs.push(line.trim_end().to_string());
			}
		}
	}
}

fn is_dead(child: &mut Child) -> bool {
	matches!(child.try_wait(), Ok(Some(_)))
}

/// Kill ladder teardown for crash/handshake-failure paths: SIGKILL only
/// (the plugin is already misbehaving or gone), then reap + join.
fn reap_or_kill(inner: &Arc<Inner>, child: &mut Child, readers: Vec<JoinHandle<()>>) {
	match child.try_wait() {
		Ok(Some(st)) => *lock(&inner.exit_summary) = Some(st.to_string()),
		Ok(None) => {
			*lock(&inner.kill_method) = KillMethod::Sigkill;
			let _ = child.kill();
			let deadline = Instant::now() + Duration::from_millis(500);
			loop {
				match child.try_wait() {
					Ok(Some(st)) => {
						*lock(&inner.exit_summary) = Some(st.to_string());
						break;
					}
					Ok(None) if Instant::now() < deadline => {
						thread::sleep(Duration::from_millis(10));
					}
					_ => break,
				}
			}
		}
		Err(_) => {}
	}
	teardown(inner, child, readers);
}

/// §4.4 ladder: `session.shutdown` → 2 s grace → SIGTERM → 2 s → SIGKILL.
/// `notify=false` means the plugin never heard about the stop (cancel
/// before Ready), so the grace rung is skipped.
fn stop_sequence(
	inner: &Arc<Inner>,
	child: &mut Child,
	ev_rx: &Receiver<Ev>,
	readers: Vec<JoinHandle<()>>,
	cfg: &LifecycleConfig,
	reason: &str,
	notify: bool,
) {
	transition(inner, PluginState::Stopping, None);
	let mut exited = false;
	if notify {
		let _ = send_to_child(inner, &shutdown_notification(reason));
		exited = wait_for_exit(child, ev_rx, cfg.shutdown_grace);
	}
	if !exited {
		exited = is_dead(child);
	}
	if !exited {
		*lock(&inner.kill_method) = KillMethod::Sigterm;
		send_sigterm(child);
		exited = wait_for_exit(child, ev_rx, cfg.shutdown_kill_wait);
	}
	if !exited {
		*lock(&inner.kill_method) = KillMethod::Sigkill;
		let _ = child.kill();
		let _ = wait_for_exit(child, ev_rx, Duration::from_millis(500));
	}
	if lock(&inner.exit_summary).is_none()
		&& let Ok(Some(st)) = child.try_wait()
	{
		*lock(&inner.exit_summary) = Some(st.to_string());
	}
	teardown(inner, child, readers);
}

fn teardown(inner: &Arc<Inner>, child: &mut Child, readers: Vec<JoinHandle<()>>) {
	*lock(&inner.pid) = None;
	// Dropping stdin closes the pipe (§1: closing Oak→plugin stdin == exit
	// request) — safe here because the child is already gone.
	*lock(&inner.stdin) = None;
	let _ = child.try_wait();
	for h in readers {
		let _ = h.join();
	}
}

fn wait_for_exit(child: &mut Child, ev_rx: &Receiver<Ev>, dur: Duration) -> bool {
	let deadline = Instant::now() + dur;
	loop {
		if is_dead(child) {
			return true;
		}
		let now = Instant::now();
		if now >= deadline {
			return is_dead(child);
		}
		let slice = (deadline - now).min(Duration::from_millis(20));
		match ev_rx.recv_timeout(slice) {
			Ok(_) | Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
		}
	}
}

#[cfg(unix)]
fn send_sigterm(child: &Child) {
	// SAFETY: plain kill(2) against our own child pid.
	unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
}

#[cfg(not(unix))]
fn send_sigterm(child: &mut Child) {
	let _ = child.kill();
}

/// Backoff sleep that still honours shutdown/disable (Enable keeps waiting).
fn sleep_interruptible(total: Duration, ctrl_rx: &Receiver<Ctrl>) -> Option<String> {
	let deadline = Instant::now() + total;
	loop {
		let now = Instant::now();
		if now >= deadline {
			return None;
		}
		match ctrl_rx.recv_timeout((deadline - now).min(Duration::from_millis(20))) {
			Ok(Ctrl::Enable) => {}
			Ok(Ctrl::Shutdown(r)) => return Some(r),
			Ok(Ctrl::Disable) => return Some("disabled".to_string()),
			Err(RecvTimeoutError::Timeout) => {}
			Err(RecvTimeoutError::Disconnected) => return Some("host_drop".to_string()),
		}
	}
}

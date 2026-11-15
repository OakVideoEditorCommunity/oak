//! Host-side session: id-namespaced in-flight pairing, request timeouts,
//! dispatch serialization and the P1 protocol-level request handlers.
//!
//! Engine routing (`HostServices`) lands in P1.2 — until then unknown
//! plugin→host requests answer `METHOD_NOT_FOUND`.

use crate::protocol::{ErrorObject, MsgId, Request, Response, METHOD_NOT_FOUND};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

/// Protocol default request timeout (§2: SDK 默认 120 s).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Bound on the in-memory `session.log` capture (P1 keeps logs in memory;
/// the file sink / log panel arrives with P1.2's `$OAK_LOG_DIR` wiring).
const MAX_SESSION_LOG_LINES: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
	/// No response within the request timeout.
	Timeout,
	/// The channel closed under this call.
	Closed,
	/// The plugin is not in `Ready` state.
	NotReady,
	/// The plugin died while this call was in flight (design §2.2).
	PluginDead,
	/// Remote returned a JSON-RPC error object.
	Remote(ErrorObject),
	/// Local failure writing the request.
	Io(String),
}

impl fmt::Display for RpcError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			RpcError::Timeout => write!(f, "request timed out"),
			RpcError::Closed => write!(f, "channel closed"),
			RpcError::NotReady => write!(f, "plugin not ready"),
			RpcError::PluginDead => write!(f, "plugin dead (PLUGIN_DEAD)"),
			RpcError::Remote(e) => write!(f, "remote error {}: {}", e.code, e.message),
			RpcError::Io(e) => write!(f, "i/o error: {e}"),
		}
	}
}

impl std::error::Error for RpcError {}

impl From<ErrorObject> for RpcError {
	fn from(e: ErrorObject) -> Self {
		RpcError::Remote(e)
	}
}

/// One connection's session state (protocol §2 pairing + §4.3 subscriptions).
pub struct HostSession {
	/// Requests the **host** sent, keyed by the host's own id namespace.
	pending: Mutex<HashMap<MsgId, Sender<Result<Value, RpcError>>>>,
	next_id: AtomicU64,
	/// Serializes request dispatch: §2 requires same-transaction mutations to
	/// run in strict arrival order; P1.2 holds this gate across mutation
	/// execution. (The P1 receive loop is single-threaded already — the gate
	/// is the explicit enforcement point for the pool that comes later.)
	pub dispatch_gate: Mutex<()>,
	subscriptions: Mutex<HashSet<String>>,
	logs: Mutex<Vec<String>>,
}

impl Default for HostSession {
	fn default() -> Self {
		Self::new()
	}
}

impl HostSession {
	pub fn new() -> Self {
		Self {
			pending: Mutex::new(HashMap::new()),
			next_id: AtomicU64::new(1),
			dispatch_gate: Mutex::new(()),
			subscriptions: Mutex::new(HashSet::new()),
			logs: Mutex::new(Vec::new()),
		}
	}

	fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
		m.lock().unwrap_or_else(|p| p.into_inner())
	}

	/// Next id from the host's namespace (the plugin's ids never collide —
	/// each side only pairs responses against ids *it* sent, §2).
	pub fn take_request_id(&self) -> MsgId {
		MsgId::Number(self.next_id.fetch_add(1, Ordering::SeqCst) as i64)
	}

	/// Register an in-flight request; the receiver completes on
	/// [`HostSession::close`] of this id or [`HostSession::abort_all`].
	pub fn register(&self, id: &MsgId) -> Receiver<Result<Value, RpcError>> {
		let (tx, rx) = std::sync::mpsc::channel();
		HostSession::lock(&self.pending).insert(id.clone(), tx);
		rx
	}

	/// Drop a registration without completing it (local timeout path).
	pub fn cancel(&self, id: &MsgId) {
		HostSession::lock(&self.pending).remove(id);
	}

	/// Pair an incoming response with its pending call. Returns `false` when
	/// the id is unknown (already timed out, or a plugin-initiated id).
	pub fn complete(&self, resp: &Response) -> bool {
		let tx = HostSession::lock(&self.pending).remove(&resp.id);
		match tx {
			Some(tx) => {
				let out = match &resp.error {
					Some(e) => Err(RpcError::Remote(e.clone())),
					None => Ok(resp.result.clone().unwrap_or(Value::Null)),
				};
				let _ = tx.send(out);
				true
			}
			None => false,
		}
	}

	/// Fail every in-flight call (crash / restart: §4.2 「所有 id 与事务作废」).
	pub fn abort_all(&self, reason: RpcError) -> usize {
		let drained: Vec<Sender<Result<Value, RpcError>>> =
			HostSession::lock(&self.pending).drain().map(|(_, tx)| tx).collect();
		let n = drained.len();
		for tx in drained {
			let _ = tx.send(Err(reason.clone()));
		}
		n
	}

	pub fn pending_count(&self) -> usize {
		HostSession::lock(&self.pending).len()
	}

	/// Wait for one response with a timeout (§2 default 120 s).
	pub fn wait(
		rx: Receiver<Result<Value, RpcError>>,
		timeout: Duration,
	) -> Result<Value, RpcError> {
		match rx.recv_timeout(timeout) {
			Ok(r) => r,
			Err(RecvTimeoutError::Timeout) => Err(RpcError::Timeout),
			Err(RecvTimeoutError::Disconnected) => Err(RpcError::Closed),
		}
	}

	// ---- event subscriptions (§4.3) ----

	/// Returns `true` when the set changed.
	pub fn subscribe(&self, event: &str) -> bool {
		HostSession::lock(&self.subscriptions).insert(event.to_string())
	}

	/// Returns `true` when the set changed.
	pub fn unsubscribe(&self, event: &str) -> bool {
		HostSession::lock(&self.subscriptions).remove(event)
	}

	/// Sorted for deterministic tests / logs.
	pub fn subscriptions(&self) -> Vec<String> {
		let mut v: Vec<String> = HostSession::lock(&self.subscriptions)
			.iter()
			.cloned()
			.collect();
		v.sort();
		v
	}

	/// Clear subscriptions on (re)start: after a restart the plugin must
	/// re-subscribe through its fresh `session.hello` (§4.2 无状态恢复).
	pub fn reset_subscriptions(&self) {
		HostSession::lock(&self.subscriptions).clear();
	}

	// ---- session.log capture (§8.1) ----

	pub fn record_log(&self, params: &Value) {
		let mut logs = HostSession::lock(&self.logs);
		if logs.len() >= MAX_SESSION_LOG_LINES {
			logs.remove(0);
		}
		logs.push(params.to_string());
	}

	pub fn logs(&self) -> Vec<String> {
		HostSession::lock(&self.logs).clone()
	}
}

/// P1 dispatch of a plugin→host request. Protocol-level methods only;
/// `HostServices` routing (capability checks, engine calls) is P1.2.
pub fn handle_host_request(session: &HostSession, req: &Request) -> Response {
	match req.method.as_str() {
		"events.subscribe" | "events.unsubscribe" => {
			let unsubscribe = req.method == "events.unsubscribe";
			let arr = |key: &str| -> Vec<String> {
				req.params
					.get(key)
					.and_then(Value::as_array)
					.map(|a| {
						a.iter()
							.filter_map(Value::as_str)
							.map(str::to_string)
							.collect()
					})
					.unwrap_or_default()
			};
			let mut granted: Vec<String> = Vec::new();
			for e in arr("events") {
				if unsubscribe {
					session.unsubscribe(&e);
				} else {
					session.subscribe(&e);
					granted.push(e);
				}
			}
			for e in arr("unsubscribe") {
				session.unsubscribe(&e);
			}
			// P1: no event→capability catalog yet (§7.2 check arrives with
			// P1.2) — every requested event is granted.
			Response::success(req.id.clone(), json!({ "granted": granted }))
		}
		"session.log" => {
			session.record_log(&req.params);
			Response::success(req.id.clone(), json!({}))
		}
		other => Response::failure(
			req.id.clone(),
			METHOD_NOT_FOUND,
			format!("method not served by the P1 host: {other}"),
		),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::protocol::{Request, Response};
	use serde_json::json;
	use std::time::Duration;

	#[test]
	fn in_flight_pairing_uses_the_hosts_own_id_namespace() {
		let s = HostSession::new();
		let id = s.take_request_id();
		let id2 = s.take_request_id();
		assert_ne!(id, id2, "ids must be unique");
		let rx = s.register(&id);
		// A response for an unknown id must not complete anything.
		assert!(!s.complete(&Response::success(999u64, json!({}))));
		assert!(s.complete(&Response::success(id.clone(), json!({"ok": 1}))));
		let v = HostSession::wait(rx, Duration::from_secs(1)).unwrap();
		assert_eq!(v, json!({"ok": 1}));
		assert_eq!(s.pending_count(), 0);
	}

	#[test]
	fn remote_errors_and_timeouts_surface_as_rpc_errors() {
		let s = HostSession::new();
		let id = s.take_request_id();
		let rx = s.register(&id);
		s.complete(&Response::failure(id.clone(), -32001, "not in edit transaction"));
		match HostSession::wait(rx, Duration::from_secs(1)) {
			Err(RpcError::Remote(e)) => assert_eq!(e.code, -32001),
			other => panic!("expected Remote error, got {other:?}"),
		}
		// Timeout: register but never complete.
		let id = s.take_request_id();
		let rx = s.register(&id);
		match HostSession::wait(rx, Duration::from_millis(50)) {
			Err(RpcError::Timeout) => {}
			other => panic!("expected Timeout, got {other:?}"),
		}
		s.cancel(&id);
	}

	#[test]
	fn abort_all_fails_every_pending_call_with_plugin_dead() {
		let s = HostSession::new();
		let a = s.take_request_id();
		let b = s.take_request_id();
		let rxa = s.register(&a);
		let rxb = s.register(&b);
		assert_eq!(s.pending_count(), 2);
		assert_eq!(s.abort_all(RpcError::PluginDead), 2);
		assert_eq!(s.pending_count(), 0);
		for rx in [rxa, rxb] {
			match HostSession::wait(rx, Duration::from_secs(1)) {
				Err(RpcError::PluginDead) => {}
				other => panic!("expected PluginDead, got {other:?}"),
			}
		}
	}

	#[test]
	fn dispatch_gate_is_exclusive_and_serves_protocol_methods() {
		let s = HostSession::new();
		// P1.2 holds this gate across mutation execution; it must be
		// exclusive (std::sync::Mutex is non-reentrant → try_lock fails).
		let guard = s.dispatch_gate.lock().unwrap();
		assert!(s.dispatch_gate.try_lock().is_err(), "gate must be exclusive");
		drop(guard);
		// subscribe / unsubscribe bookkeeping
		let req = Request::new(
			1u64,
			"events.subscribe",
			json!({"events": ["timeline.structure_changed", "export.progress"]}),
		);
		let resp = handle_host_request(&s, &req);
		assert!(resp.is_success(), "{resp:?}");
		assert_eq!(
			resp.result,
			Some(json!({"granted": ["timeline.structure_changed", "export.progress"]}))
		);
		assert_eq!(s.subscriptions().len(), 2);
		let req = Request::new(2u64, "events.unsubscribe", json!({"events": ["export.progress"]}));
		let _ = handle_host_request(&s, &req);
		assert_eq!(s.subscriptions(), vec!["timeline.structure_changed"]);
		// session.log records
		let req = Request::new(3u64, "session.log", json!({"level": "info", "message": "hi"}));
		assert!(handle_host_request(&s, &req).is_success());
		assert_eq!(s.logs().len(), 1);
		// unknown method → METHOD_NOT_FOUND
		let req = Request::new(4u64, "ui.set_tree", json!({}));
		let resp = handle_host_request(&s, &req);
		assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
	}
}

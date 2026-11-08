//! Protocol-level acceptance: the **real binary** over real pipes.
//!
//! Nothing here links the server library — each test spawns
//! `target/<profile>/oak-ai-mcp --backend mock`, writes NDJSON frames to
//! its stdin and asserts on the frames it writes to stdout, so the framing,
//! the argument parsing and the process lifecycle are all covered.
//!
//! Chain: `initialize` → `notifications/initialized` → `tools/list` →
//! `tools/call` (`project.open`, `timeline.add_track`) → invalid arguments
//! → unknown tool → failing mid-call → clean EOF exit.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

/// Generous bound: a hung server must fail the test, not hang cargo.
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);

const BIN: &str = env!("CARGO_BIN_EXE_oak-ai-mcp");

struct Session {
	child: Child,
	stdin: ChildStdin,
	lines: Receiver<String>,
}

impl Session {
	/// Starts the server on the mock backend, with a reader thread pumping
	/// stdout frames.
	fn start(extra_args: &[&str]) -> Self {
		Self::start_with("mock", extra_args)
	}

	/// Starts the server on `backend`; the sdk-backend test uses this, every
	/// other test goes through [`Session::start`] (mock).
	fn start_with(backend: &str, extra_args: &[&str]) -> Self {
		let mut child = Command::new(BIN)
			.args(["--backend", backend])
			.args(extra_args)
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.spawn()
			.unwrap_or_else(|err| panic!("cannot spawn {BIN}: {err}"));

		let stdin = child.stdin.take().expect("child stdin");
		let stdout = child.stdout.take().expect("child stdout");
		let (tx, rx) = mpsc::channel();
		std::thread::spawn(move || {
			for line in BufReader::new(stdout).lines() {
				match line {
					Ok(line) => {
						if tx.send(line).is_err() {
							break;
						}
					}
					Err(_) => break,
				}
			}
		});

		Self {
			child,
			stdin,
			lines: rx,
		}
	}

	fn send_frame(&mut self, frame: &Value) {
		writeln!(self.stdin, "{frame}").expect("write frame");
		self.stdin.flush().expect("flush frame");
	}

	fn send_raw(&mut self, frame: &str) {
		writeln!(self.stdin, "{frame}").expect("write raw frame");
		self.stdin.flush().expect("flush raw frame");
	}

	fn request(&mut self, id: u64, method: &str, params: Value) {
		self.send_frame(&json!({
			"jsonrpc": "2.0",
			"id": id,
			"method": method,
			"params": params,
		}));
	}

	fn notify(&mut self, method: &str, params: Value) {
		self.send_frame(&json!({
			"jsonrpc": "2.0",
			"method": method,
			"params": params,
		}));
	}

	/// Reads one stdout line and checks it is a JSON-RPC 2.0 envelope.
	fn recv(&mut self) -> Value {
		let line = self
			.lines
			.recv_timeout(FRAME_TIMEOUT)
			.unwrap_or_else(|err| panic!("no response frame within {FRAME_TIMEOUT:?}: {err}"));
		assert!(!line.contains('\r'), "frames must be LF-terminated: {line:?}");
		let value: Value = serde_json::from_str(&line)
			.unwrap_or_else(|err| panic!("stdout frame is not JSON ({err}): {line:?}"));
		assert_eq!(value["jsonrpc"], "2.0", "bad envelope: {line}");
		value
	}

	/// Reads the next frame and asserts it answers `id` (proves the server
	/// sent no response for a preceding notification).
	fn recv_for(&mut self, id: u64) -> Value {
		let value = self.recv();
		assert_eq!(value["id"], id, "expected the response to id {id}: {value}");
		value
	}

	/// Closes stdin, waits for a clean exit and returns the frames nobody
	/// asked for (must be empty).
	fn finish(mut self) -> Vec<String> {
		drop(self.stdin);
		let status = self.child.wait().expect("wait for the server");
		assert!(
			status.success(),
			"server must exit 0 after stdin EOF, got {status:?}"
		);
		let mut leftover = Vec::new();
		// Small grace period so the reader thread can drain the pipe.
		while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(500)) {
			leftover.push(line);
		}
		leftover
	}
}

fn initialize_params() -> Value {
	json!({
		"protocolVersion": "2024-11-05",
		"capabilities": {},
		"clientInfo": { "name": "protocol-test", "version": "0" },
	})
}

fn tool_names(tools: &[Value]) -> BTreeSet<String> {
	tools
		.iter()
		.map(|tool| tool["name"].as_str().expect("tool name").to_string())
		.collect()
}

#[test]
fn full_protocol_chain_initialize_list_call_and_errors() {
	let mut session = Session::start(&[]);

	// --- initialize ---------------------------------------------------
	session.request(1, "initialize", initialize_params());
	let init = session.recv_for(1);
	assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
	assert_eq!(init["result"]["serverInfo"]["name"], "oak-ai-mcp");
	assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], false);
	assert!(
		init["result"]["instructions"].as_str().unwrap().contains("22 tools"),
		"instructions must describe the frozen tool face: {init}"
	);

	// --- notifications/initialized: no reply at all -------------------
	session.notify("notifications/initialized", json!({}));
	// The next frame must be the ping response, not an answer to the
	// notification.
	session.request(2, "ping", json!({}));
	let pong = session.recv_for(2);
	assert_eq!(pong["result"], json!({}));

	// --- tools/list ---------------------------------------------------
	session.request(3, "tools/list", json!({}));
	let list = session.recv_for(3);
	let tools = list["result"]["tools"]
		.as_array()
		.unwrap_or_else(|| panic!("tools/list must return an array: {list}"))
		.clone();
	assert_eq!(tools.len(), 22, "the frozen face has 22 tools");
	let names = tool_names(&tools);
	assert_eq!(names.len(), 22, "duplicate tool names in tools/list");
	for name in [
		"edit.undo",
		"project.open",
		"timeline.add_track",
		"timeline.split_clip",
		"render.get_frame",
	] {
		assert!(names.contains(name), "tools/list is missing '{name}'");
	}
	for tool in &tools {
		assert!(
			tool["inputSchema"].is_object(),
			"{} has no inputSchema object",
			tool["name"]
		);
		assert_eq!(
			tool["inputSchema"]["type"],
			"object",
			"{} inputSchema must be an object schema",
			tool["name"]
		);
	}

	// --- tools/call: project.open -------------------------------------
	session.request(
		4,
		"tools/call",
		json!({ "name": "project.open", "arguments": { "name": "protocol", "fps": 30 } }),
	);
	let opened = session.recv_for(4);
	assert_eq!(opened["result"]["isError"], false, "{opened}");
	assert_eq!(opened["result"]["structuredContent"]["created"], true);
	assert_eq!(opened["result"]["content"][0]["type"], "text");

	// --- tools/call: timeline.add_track -------------------------------
	session.request(
		5,
		"tools/call",
		json!({ "name": "timeline.add_track", "arguments": { "type": "video" } }),
	);
	let added = session.recv_for(5);
	assert_eq!(added["result"]["isError"], false, "{added}");
	let track = added["result"]["structuredContent"]["track"]
		.as_str()
		.unwrap_or_else(|| panic!("timeline.add_track must return a track id: {added}"));
	assert!(!track.is_empty());

	// --- invalid arguments: schema rejection, no transaction ----------
	session.request(
		6,
		"tools/call",
		json!({
			"name": "timeline.split_clip",
			"arguments": { "clip": "clip-1", "time": 1.5 },
		}),
	);
	let invalid = session.recv_for(6);
	assert_eq!(invalid["result"]["isError"], true, "{invalid}");
	let error = &invalid["result"]["structuredContent"];
	assert_eq!(error["code"], "SCHEMA_VALIDATION_FAILED");
	assert_eq!(error["step"], "validate");
	assert_eq!(error["tool"], "timeline.split_clip");
	assert!(
		error.get("txn").is_none(),
		"a schema rejection must not open a transaction: {error}"
	);
	let text = invalid["result"]["content"][0]["text"].as_str().unwrap();
	assert!(
		text.contains("SCHEMA_VALIDATION_FAILED"),
		"the text content must carry the code too: {text}"
	);

	// --- unknown tool -------------------------------------------------
	session.request(
		7,
		"tools/call",
		json!({ "name": "timeline.teleport", "arguments": {} }),
	);
	let unknown = session.recv_for(7);
	assert_eq!(unknown["result"]["isError"], true, "{unknown}");
	let error = &unknown["result"]["structuredContent"];
	assert_eq!(error["code"], "UNKNOWN_TOOL");
	assert_eq!(error["step"], "lookup");
	assert_eq!(error["tool"], "timeline.teleport");
	assert!(error.get("txn").is_none(), "{error}");

	// --- failure inside a transaction: aborted ------------------------
	session.request(
		8,
		"tools/call",
		json!({
			"name": "timeline.split_clip",
			"arguments": { "clip": "clip-999", "time": { "num": 1, "den": 1 } },
		}),
	);
	let failed = session.recv_for(8);
	assert_eq!(failed["result"]["isError"], true, "{failed}");
	let error = &failed["result"]["structuredContent"];
	assert_eq!(error["code"], "NOT_FOUND");
	assert_eq!(error["step"], "invoke");
	assert_eq!(error["txn"], "aborted");

	// --- clean EOF: exit 0, no stray frames ---------------------------
	let leftover = session.finish();
	assert!(leftover.is_empty(), "unexpected extra frames: {leftover:?}");
}

#[test]
fn requests_before_initialize_are_rejected_but_the_session_survives() {
	let mut session = Session::start(&[]);

	session.request(1, "tools/list", json!({}));
	let early = session.recv_for(1);
	assert_eq!(
		early["error"]["code"], -32002,
		"MCP `server not initialized` code: {early}"
	);

	// A notification before initialize is silently ignored.
	session.notify("notifications/initialized", json!({}));

	session.request(2, "initialize", initialize_params());
	let init = session.recv_for(2);
	assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

	session.request(3, "tools/list", json!({}));
	let list = session.recv_for(3);
	assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 22);

	assert!(session.finish().is_empty());
}

#[test]
fn bad_frames_do_not_kill_the_session() {
	let mut session = Session::start(&[]);

	// Non-JSON line -> -32700 with a null id, session continues.
	session.send_raw("this is not json");
	let parse_error = session.recv();
	assert_eq!(parse_error["error"]["code"], -32700, "{parse_error}");
	assert_eq!(parse_error["id"], Value::Null);

	// Batch array -> -32600 (MCP stdio carries one object per frame).
	session.send_raw(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
	let batch = session.recv();
	assert_eq!(batch["error"]["code"], -32600, "{batch}");
	assert_eq!(batch["id"], Value::Null);

	session.request(1, "initialize", initialize_params());
	assert_eq!(session.recv_for(1)["result"]["protocolVersion"], "2024-11-05");

	// Unknown request method -> -32601.
	session.request(2, "tools/teleport", json!({}));
	let unknown = session.recv_for(2);
	assert_eq!(unknown["error"]["code"], -32601, "{unknown}");

	// Wrong jsonrpc member -> -32600.
	session.send_frame(&json!({"jsonrpc": "1.0", "id": 3, "method": "ping"}));
	let wrong_version = session.recv_for(3);
	assert_eq!(wrong_version["error"]["code"], -32600, "{wrong_version}");

	// tools/call with a non-object `arguments` -> -32602, no tool run.
	session.request(
		4,
		"tools/call",
		json!({ "name": "project.open", "arguments": [] }),
	);
	let bad_arguments = session.recv_for(4);
	assert_eq!(bad_arguments["error"]["code"], -32602, "{bad_arguments}");

	// Missing tool name -> -32602 as well.
	session.request(5, "tools/call", json!({ "arguments": {} }));
	let missing_name = session.recv_for(5);
	assert_eq!(missing_name["error"]["code"], -32602, "{missing_name}");

	assert!(session.finish().is_empty());
}

/// Default build: `--backend sdk` must fail to start instead of pretending
/// the engine is available (no silent degradation to the mock backend).
#[cfg(not(feature = "sdk"))]
#[test]
fn sdk_backend_is_refused_when_built_without_the_feature() {
	let output = Command::new(BIN)
		.args(["--backend", "sdk"])
		.output()
		.unwrap_or_else(|err| panic!("cannot run {BIN}: {err}"));

	assert_eq!(
		output.status.code(),
		Some(2),
		"sdk without the feature must exit 2, got {:?}",
		output.status
	);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		stderr.contains("--features sdk"),
		"the refusal must say how to enable it: {stderr}"
	);
	assert!(
		stderr.contains("Refusing to fall back"),
		"the refusal must be explicit: {stderr}"
	);
	assert!(
		output.stdout.is_empty(),
		"a refused server must not speak on stdout: {:?}",
		String::from_utf8_lossy(&output.stdout)
	);
}

/// `--features sdk` build: the same flag must construct the real backend and
/// serve MCP, never fall back to the mock.
///
/// Only compiled in an sdk build (cfg-gated out of the default suite, which
/// cannot link the engine). It asserts the *wiring*: `SdkBackend::new()` only
/// builds empty state, so the server answers `initialize` before any engine
/// handle is touched — no project, media file or engine runtime is needed.
/// Running it needs the engine cdylib reachable at load time (the same
/// environment the T1 `--features sdk` suite uses).
#[cfg(feature = "sdk")]
#[test]
fn sdk_backend_starts_and_serves_when_built_with_the_feature() {
	let mut session = Session::start_with("sdk", &[]);

	session.request(1, "initialize", initialize_params());
	let init = session.recv_for(1);
	assert_eq!(init["result"]["serverInfo"]["name"], "oak-ai-mcp");
	assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

	// The tool face is backend-independent: 22 frozen tools either way.
	session.request(2, "tools/list", json!({}));
	let list = session.recv_for(2);
	assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 22);

	// Clean shutdown: stdin EOF, exit 0, no stray frames.
	assert!(session.finish().is_empty());
}

/// Pins protocol-version negotiation on the wire (the ablation of the echo
/// layer previously passed every test — nothing nailed it): a supported
/// version is echoed back, an unknown one falls back to the baseline
/// instead of failing the handshake.
#[test]
fn protocol_version_is_echoed_or_baselined() {
	// Supported (2025-06-18): echoed verbatim.
	let mut session = Session::start(&[]);
	session.request(
		1,
		"initialize",
		json!({
			"protocolVersion": "2025-06-18",
			"capabilities": {},
			"clientInfo": { "name": "protocol-test", "version": "0" },
		}),
	);
	let init = session.recv_for(1);
	assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
	assert!(session.finish().is_empty());

	// Unknown version: falls back to the baseline, handshake still succeeds.
	let mut session = Session::start(&[]);
	session.request(
		1,
		"initialize",
		json!({
			"protocolVersion": "1999-01-01",
			"capabilities": {},
			"clientInfo": { "name": "protocol-test", "version": "0" },
		}),
	);
	let init = session.recv_for(1);
	assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
	assert!(session.finish().is_empty());
}

#[test]
fn unknown_backend_and_help_exit_codes() {
	let unknown = Command::new(BIN)
		.args(["--backend", "engine"])
		.output()
		.expect("run binary");
	assert_eq!(unknown.status.code(), Some(2), "{:?}", unknown.status);
	assert!(
		String::from_utf8_lossy(&unknown.stderr).contains("unknown backend 'engine'"),
		"{}",
		String::from_utf8_lossy(&unknown.stderr)
	);

	let help = Command::new(BIN).arg("--help").output().expect("run binary");
	assert_eq!(help.status.code(), Some(0), "{:?}", help.status);
	let stdout = String::from_utf8_lossy(&help.stdout);
	assert!(stdout.contains("oak-ai-mcp"), "{stdout}");
	assert!(stdout.contains("--backend"), "{stdout}");
}

/// Every spawned server must be reaped by `Session::finish`; this test
/// exists so an early-returning test cannot leak a process silently.
#[test]
fn server_starts_and_exits_zero_on_immediate_eof() {
	let mut session = Session::start(&[]);
	session.request(1, "initialize", initialize_params());
	let init = session.recv_for(1);
	assert_eq!(init["result"]["serverInfo"]["name"], "oak-ai-mcp");
	let status: ExitStatus = {
		drop(session.stdin);
		session.child.wait().expect("wait")
	};
	assert!(status.success(), "immediate EOF must exit 0: {status:?}");
}

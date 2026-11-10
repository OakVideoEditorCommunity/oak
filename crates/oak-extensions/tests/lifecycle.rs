//! T7.1 acceptance: five lifecycle tests over fake plugins (no network).
//!
//! Each test writes a `plugin.toml` + `plugin.py` into its own temp dir and
//! drives a real child process through [`PluginHost`].

use oak_extensions::lifecycle::{
	FailureReason, KillMethod, LifecycleConfig, PluginHost, PluginState,
};
use oak_extensions::manifest::Manifest;
use oak_extensions::protocol::MAX_MESSAGE_BYTES;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Fresh per-test directory under $TMPDIR.
fn test_dir(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!(
		"opp-t19-{}-{}",
		tag,
		SEQ.fetch_add(1, Ordering::SeqCst)
	));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).expect("create test dir");
	dir
}

/// Manifest for `/usr/bin/python3 plugin.py` with the given restart policy.
fn manifest(_dir: &Path, tag: &str, max: u32, backoff_ms: u64) -> Manifest {
	let text = format!(
		r#"id = "test.{tag}"
name = "T {tag}"
version = "0.0.1"
api = 1
capabilities = ["project.read"]

[process]
command = ["/usr/bin/python3", "{{plugin_dir}}/plugin.py"]

[restart]
max = {max}
backoff_ms = {backoff_ms}
"#
	);
	Manifest::from_toml(&text).expect("valid manifest")
}

fn write_plugin(dir: &Path, source: &str) {
	std::fs::write(dir.join("plugin.py"), source).expect("write plugin.py");
}

/// Long-handshake config: no heartbeat interference, 500 ms first-message
/// budget (the frozen default is 10 s; tests compress it).
fn quiet_config() -> LifecycleConfig {
	LifecycleConfig {
		handshake_timeout: Duration::from_millis(500),
		heartbeat_interval: Duration::from_secs(30),
		heartbeat_timeout: Duration::from_secs(30),
		..LifecycleConfig::default()
	}
}

const HELLO: &str = r#"import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
send({"jsonrpc":"2.0","id":1,"method":"session.hello","params":{"api":1,"name":"fake","version":"0.0.1","capabilities":["project.read"],"panels":[],"subscribe":[]}})
"#;

/// ① No first message within the handshake budget → kill, `Failed`.
#[test]
fn handshake_timeout_kills_the_process() {
	let dir = test_dir("hs-timeout");
	write_plugin(&dir, "import time\ntime.sleep(60)\n");
	let host = PluginHost::start(
		manifest(&dir, "hs-timeout", 5, 10),
		&dir,
		quiet_config(),
	)
	.expect("start");
	assert!(
		host.wait_state(|s| s == PluginState::Failed, Duration::from_secs(10)),
		"expected Failed, got {:?}",
		host.state()
	);
	assert_eq!(
		host.failure(),
		Some(FailureReason::HandshakeTimeout),
		"failure reason must be the handshake timeout"
	);
	assert_eq!(
		host.kill_method(),
		KillMethod::Sigkill,
		"a hung child must be SIGKILLed"
	);
	assert!(
		host.exit_summary().is_some(),
		"the child must be reaped before Failed is observable"
	);
	assert_eq!(host.spawn_count(), 1, "no restart on handshake failure");
}

/// ② Non-protocol bytes on stdout after Ready → violation + kill, no restart.
#[test]
fn non_protocol_stdout_is_a_violation_and_kills() {
	let dir = test_dir("stdout");
	write_plugin(
		&dir,
		&format!(
			"{HELLO}import time, sys
sys.stdin.readline()
sys.stdout.write('this line is not jsonrpc\\n'); sys.stdout.flush()
time.sleep(60)
"
		),
	);
	let host = PluginHost::start(
		manifest(&dir, "stdout", 5, 10),
		&dir,
		quiet_config(),
	)
	.expect("start");
	assert!(
		host.wait_state(|s| s == PluginState::Failed, Duration::from_secs(10)),
		"expected Failed, got {:?}",
		host.state()
	);
	assert!(
		matches!(host.failure(), Some(FailureReason::StdoutViolation(_))),
		"expected StdoutViolation, got {:?}",
		host.failure()
	);
	assert_eq!(host.kill_method(), KillMethod::Sigkill);
	assert_eq!(host.spawn_count(), 1, "violations do not restart");
}

/// ③ A 17 MiB line exceeds the 16 MiB cap → rejected while reading + killed.
#[test]
fn seventeen_mib_line_is_rejected_and_kills() {
	let dir = test_dir("oversize");
	write_plugin(
		&dir,
		"import sys, time\nsys.stdout.write('x' * (17 * 1024 * 1024) + '\\n')\nsys.stdout.flush()\ntime.sleep(60)\n",
	);
	let host = PluginHost::start(
		manifest(&dir, "oversize", 5, 10),
		&dir,
		quiet_config(),
	)
	.expect("start");
	assert!(
		host.wait_state(|s| s == PluginState::Failed, Duration::from_secs(15)),
		"expected Failed, got {:?}",
		host.state()
	);
	match host.failure() {
		Some(FailureReason::MessageTooLarge { bytes }) => {
			assert!(
				bytes > MAX_MESSAGE_BYTES,
				"detected size {bytes} must exceed the cap"
			);
		}
		other => panic!("expected MessageTooLarge, got {other:?}"),
	}
	assert_eq!(host.kill_method(), KillMethod::Sigkill);
	assert_eq!(host.spawn_count(), 1, "oversize kills without restart");
}

/// ④ Crash loop stops after `restart.max = 5`: initial spawn + 5 restarts.
#[test]
fn crash_restarts_are_bounded_at_five() {
	let dir = test_dir("crash");
	write_plugin(
		&dir,
		&format!(
			"{HELLO}import sys
sys.stdin.readline()
sys.exit(0)
"
		),
	);
	let host = PluginHost::start(manifest(&dir, "crash", 5, 10), &dir, LifecycleConfig {
		handshake_timeout: Duration::from_secs(5),
		heartbeat_interval: Duration::from_secs(30),
		heartbeat_timeout: Duration::from_secs(30),
		..LifecycleConfig::default()
	})
	.expect("start");
	assert!(
		host.wait_state(|s| s == PluginState::Failed, Duration::from_secs(30)),
		"expected terminal Failed, got {:?}",
		host.state()
	);
	assert_eq!(
		host.failure(),
		Some(FailureReason::MaxRestartsExceeded),
		"the loop must stop on the restart budget"
	);
	assert_eq!(
		host.spawn_count(),
		6,
		"exactly initial spawn + 5 restarts"
	);
}

/// ⑤ Clean shutdown: notification → self-exit inside the grace window, so
/// the SIGTERM/SIGKILL rungs are never needed.
#[test]
fn clean_shutdown_needs_no_kill() {
	let dir = test_dir("shutdown");
	let src = String::from(HELLO)
		+ concat!(
			"import sys, json\n",
			"for line in sys.stdin:\n",
			"    line = line.strip()\n",
			"    if not line:\n",
			"        continue\n",
			"    m = json.loads(line)\n",
			"    if m.get('method') == 'session.shutdown':\n",
			"        sys.exit(0)\n",
			"    if 'method' in m and 'id' in m:\n",
			"        sys.stdout.write(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':{}}) + '\\n'); sys.stdout.flush()\n",
		);
	write_plugin(&dir, &src);
	let host = PluginHost::start(
		manifest(&dir, "shutdown", 5, 10),
		&dir,
		LifecycleConfig {
			handshake_timeout: Duration::from_secs(5),
			heartbeat_interval: Duration::from_millis(50),
			heartbeat_timeout: Duration::from_millis(500),
			heartbeat_misses: 3,
			shutdown_grace: Duration::from_secs(2),
			shutdown_kill_wait: Duration::from_secs(1),
			..LifecycleConfig::default()
		},
	)
	.expect("start");
	assert!(
		host.wait_state(|s| s == PluginState::Ready, Duration::from_secs(10)),
		"expected Ready, got {:?} ({:?})",
		host.state(),
		host.failure()
	);
	// Let a few 50 ms heartbeats round-trip (§4.2 liveness proof).
	std::thread::sleep(Duration::from_millis(300));
	assert!(
		host.heartbeat_ok() >= 1,
		"expected >= 1 heartbeat response, got {}",
		host.heartbeat_ok()
	);

	host.shutdown("app_quit");
	assert!(
		host.wait_state(|s| s == PluginState::Stopped, Duration::from_secs(10)),
		"expected Stopped, got {:?}",
		host.state()
	);
	assert_eq!(
		host.kill_method(),
		KillMethod::None,
		"clean shutdown must not need SIGTERM/SIGKILL"
	);
	let exit = host.exit_summary().expect("exit status recorded");
	assert!(
		exit.contains("exit status: 0"),
		"plugin must exit 0, got {exit:?}"
	);
	assert_eq!(host.failure(), None, "no failure on a clean stop");
}

/// Frozen default knobs (protocol §4 / spec §2.3) — guards accidental drift.
#[test]
fn default_knobs_match_the_frozen_protocol() {
	let c = LifecycleConfig::default();
	assert_eq!(c.handshake_timeout, Duration::from_secs(10), "10 s handshake");
	assert_eq!(c.heartbeat_interval, Duration::from_secs(2), "2 s ping");
	assert_eq!(c.heartbeat_timeout, Duration::from_secs(5), "5 s pong budget");
	assert_eq!(c.heartbeat_misses, 3, "3 misses = crash");
	assert_eq!(c.shutdown_grace, Duration::from_secs(2), "2 s self-exit grace");
	assert_eq!(c.shutdown_kill_wait, Duration::from_secs(2), "2 s SIGTERM grace");
}

/// The eleven states are distinct and only three are terminal.
#[test]
fn eleven_states_and_three_terminal() {
	use oak_extensions::lifecycle::PluginState as S;
	let all = [
		S::Discovered,
		S::Enabling,
		S::Spawning,
		S::Handshaking,
		S::Ready,
		S::Crashed,
		S::Restarting,
		S::Stopping,
		S::Stopped,
		S::Failed,
		S::Disabled,
	];
	let mut uniq: Vec<String> = all.iter().map(|s| format!("{s:?}")).collect();
	uniq.sort();
	uniq.dedup();
	assert_eq!(uniq.len(), 11, "the state machine has exactly 11 states");
	let terminal: Vec<S> = all.into_iter().filter(|s| s.is_terminal()).collect();
	assert_eq!(terminal.len(), 3, "Stopped/Failed/Disabled are terminal");
	assert!(S::Stopped.is_terminal() && S::Failed.is_terminal() && S::Disabled.is_terminal());
	assert!(!S::Ready.is_terminal() && !S::Spawning.is_terminal());
}

/// `PluginHost::call` refuses before Ready and times out on silence.
#[test]
fn call_requires_ready_and_times_out() {
	let dir = test_dir("call");
	// A plugin that handshakes, then answers nothing at all.
	write_plugin(
		&dir,
		&format!(
			"{HELLO}import time, sys
sys.stdin.readline()
time.sleep(60)
"
		),
	);
	let host = PluginHost::start(manifest(&dir, "call", 0, 10), &dir, LifecycleConfig {
		handshake_timeout: Duration::from_millis(500),
		heartbeat_interval: Duration::from_secs(30),
		heartbeat_timeout: Duration::from_secs(30),
		..LifecycleConfig::default()
	})
	.expect("start");
	// Before Ready: NotReady.
	let deadline = Instant::now() + Duration::from_secs(5);
	while host.state() != PluginState::Ready && Instant::now() < deadline {
		std::thread::sleep(Duration::from_millis(10));
	}
	assert_eq!(host.state(), PluginState::Ready, "plugin must reach Ready");
	let err = host
		.call("project.get_info", serde_json::json!({}), Duration::from_millis(50))
		.expect_err("plugin never answers → timeout");
	assert!(matches!(err, oak_extensions::session::RpcError::Timeout), "{err:?}");
	host.shutdown("test_done");
	assert!(host.wait_terminal(Duration::from_secs(10)), "terminal after shutdown");
}

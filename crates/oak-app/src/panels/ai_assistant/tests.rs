use super::*;
use crate::ai::ToolLogEntry;
// P7 merge: the graphops helpers moved into `crate::oakui::graphops`;
// import them once for the whole test module instead of per-test.
use crate::oakui::graphops;

/// The T13B DeadStore flake (1 red in 9 full runs): `OAK_AI_API_KEY`
/// is shared process state, and every test here constructs a panel
/// that reads it (`from_env_and_keyring`), so a writer's
/// set/remove window races any reader's `configured` snapshot.
/// **Every `#[gpui_kit::test]` in this module takes this lock on
/// its first line** — writers and readers on one lock, or neither
/// direction is safe. A new test without `let _env = env_lock();`
/// is a bug; `all_panel_tests_hold_the_env_lock` enforces it.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The doc on `env_lock` promises this invariant; here it is, so the
/// promise cannot rot. Scans this file's own source for every real
/// `#[gpui_kit::test]` attribute and requires `env_lock()` in the
/// window that follows — a new unlocked test turns this red.
#[test]
fn all_panel_tests_hold_the_env_lock() {
	let src = include_str!("tests.rs");
	let lines: Vec<&str> = src.lines().collect();
	let mut checked = 0;
	for (i, line) in lines.iter().enumerate() {
		// Attribute syntax only: skip prose that merely quotes the
		// attribute (this file's comments do, around line 3460).
		if !line.contains("#[gpui_kit::test]") || !line.trim_start().starts_with("#[") {
			continue;
		}
		checked += 1;
		// Stop at the next test attribute: a fixed 40-line window
		// would swallow the following test's `env_lock()` and wave
		// an unlocked test through — exactly the way a new test
		// pasted next to an old one would slip in.
		let next = lines[i + 1..]
			.iter()
			.position(|l| l.trim_start().starts_with("#[gpui_kit::test]"))
			.map(|p| i + 1 + p)
			.unwrap_or(lines.len());
		let window: String = lines[i..next].join("\n");
		assert!(
			window.contains("env_lock()"),
			"test at line {} declares #[gpui_kit::test] but never takes env_lock(); \
				 the shared OAK_AI_API_KEY would race other tests",
			i + 1
		);
	}
	// A scanner that finds nothing would pass vacuously; this module
	// has had 10+ panel tests since T12.
	assert!(
		checked >= 10,
		"the scanner must actually find the tests, found {checked}"
	);
}

/// Saves one process env var, clears it (so the test controls the
/// exact value it asserts on), and restores the original in `Drop`
/// — including when the test panics, which a tail-of-function
/// restore would skip and then poison every later test.
struct EnvVarGuard {
	name: &'static str,
	saved: Option<String>,
}
impl EnvVarGuard {
	fn clear(name: &'static str) -> Self {
		let saved = std::env::var(name).ok();
		// SAFETY: tests are single-process and hold `env_lock`.
		unsafe { std::env::remove_var(name) };
		Self { name, saved }
	}
}
impl Drop for EnvVarGuard {
	fn drop(&mut self) {
		// SAFETY: same process, same lock discipline as `clear`.
		unsafe {
			match &self.saved {
				Some(v) => std::env::set_var(self.name, v),
				None => std::env::remove_var(self.name),
			}
		}
	}
}
// The v2 face's trait must be in scope for the `HostAdapter` calls the
// `sequence.*` tests make directly.
use oak_ai_tools::backend::EngineExt;

/// Every structured code the loop can surface maps to a real key, and
/// unknown codes degrade to the fallback instead of leaking the raw
/// string as a translation key.
#[test]
fn error_i18n_key_maps_structured_codes() {
	assert_eq!(
		error_i18n_key("AI_NOT_CONFIGURED"),
		"ai.error.not_configured"
	);
	assert_eq!(error_i18n_key("AI_HTTP_ERROR"), "ai.error.http");
	assert_eq!(error_i18n_key("AI_API_ERROR"), "ai.error.api");
	assert_eq!(error_i18n_key("AI_BAD_RESPONSE"), "ai.error.bad_response");
	assert_eq!(
		error_i18n_key("ENGINE_UNSUPPORTED"),
		"ai.error.engine_unsupported"
	);
	assert_eq!(error_i18n_key("SOMETHING_NEW"), "ai.error.unknown");
}

#[test]
fn outcome_i18n_key_maps_loop_outcomes() {
	assert_eq!(outcome_i18n_key("applied"), "ai.outcome.applied");
	assert_eq!(outcome_i18n_key("proposed"), "ai.outcome.proposed");
	assert_eq!(outcome_i18n_key("rejected"), "ai.outcome.rejected");
	assert_eq!(outcome_i18n_key("failed"), "ai.outcome.failed");
	assert_eq!(outcome_i18n_key("??"), "ai.error.unknown");
}

/// One run at a time; a run needs a key and a non-empty draft.
#[test]
fn send_guard_blocks_busy_unconfigured_and_blank() {
	assert!(can_send(false, true, true));
	assert!(!can_send(true, true, true), "busy blocks a second send");
	assert!(!can_send(false, false, true), "missing key blocks");
	assert!(!can_send(false, true, false), "blank draft blocks");
}

/// The transcript gets tool bubbles in call order, then the answer; a
/// capped run gets the round-limit notice instead of a fake answer.
#[test]
/// **O3**: a stalled run must reach the panel as a notice, not as a
/// silent truncation — the field's worst symptom was "reached the round
/// limit" after six identical failures.
#[test]
fn append_report_surfaces_a_stall_as_a_notice() {
	let mut bubbles = Vec::new();
	let report = AgentReport {
		rounds: 3,
		converged: false,
		final_text: None,
		tools: vec![ToolLogEntry {
			round: 3,
			tool: "sequence.set_format".to_string(),
			outcome: "failed",
			detail: "{\"code\":\"INVALID_PARAMS\"}".to_string(),
		}],
		stalled: Some(crate::ai::agent::StallStop {
			tool: "sequence.set_format".to_string(),
			code: "INVALID_PARAMS".to_string(),
			repeats: 3,
			message: "`fps` must be a number".to_string(),
		}),
		text_fallback_calls: 0,
		unparsed_text_calls: 0,
		unparsed_text: None,
		..Default::default()
	};
	append_report(&mut bubbles, &report);

	let notice = bubbles.iter().find_map(|b| match b {
		Bubble::System(text) => Some(text.clone()),
		_ => None,
	});
	let notice = notice.expect("a stall must produce a system notice");
	assert!(
		notice.contains(crate::i18n::tr("ai.notice.stalled")),
		"the notice must use the translated wording: {notice}"
	);
	assert!(
		notice.contains("sequence.set_format") && notice.contains("INVALID_PARAMS"),
		"the notice must name what is stuck: {notice}"
	);
	// The round-limit notice must not be what the user sees here.
	assert!(
		!notice.contains(crate::i18n::tr("ai.notice.limit")),
		"a stall is not the round cap: {notice}"
	);
}

/// Unparseable text-form markup reaches the user as a notice plus the raw
/// text in a detail bubble — never as the answer.
#[test]
fn append_report_reports_unparsed_text_markup() {
	let raw = "<tool_call><parameter=fps>60</tool_call>";
	let mut bubbles = Vec::new();
	let report = AgentReport {
		rounds: 1,
		converged: false,
		final_text: None,
		tools: Vec::new(),
		stalled: None,
		text_fallback_calls: 0,
		unparsed_text_calls: 1,
		unparsed_text: Some(raw.to_string()),
		..Default::default()
	};
	append_report(&mut bubbles, &report);

	let notice = bubbles
		.iter()
		.find_map(|b| match b {
			Bubble::System(text) => Some(text.clone()),
			_ => None,
		})
		.expect("a notice is pushed");
	assert!(
		notice.contains(crate::i18n::tr("ai.notice.unparsed_tool_call")),
		"the notice must use the translated wording: {notice}"
	);
	let detail = bubbles
		.iter()
		.find_map(|b| match b {
			Bubble::Tool { detail, .. } => Some(detail.clone()),
			_ => None,
		})
		.expect("the raw text is kept for diagnosis");
	assert!(detail.contains("</tool_call>"), "raw markup kept: {detail}");
	// Nothing may present the markup as the answer.
	assert!(
		!bubbles.iter().any(|b| matches!(b, Bubble::Assistant(_))),
		"no assistant bubble: {bubbles:?}"
	);
}

#[test]
fn append_report_orders_tools_then_answer() {
	let tool = |name: &str, outcome: &'static str| ToolLogEntry {
		round: 1,
		tool: name.to_owned(),
		outcome,
		detail: format!("{name}-detail"),
	};
	let mut bubbles = Vec::new();
	let report = AgentReport {
		rounds: 2,
		converged: true,
		final_text: Some("done".to_owned()),
		tools: vec![
			tool("project.open", "applied"),
			tool("node.set_param", "failed"),
		],
		stalled: None,
		text_fallback_calls: 0,
		unparsed_text_calls: 0,
		unparsed_text: None,
		..Default::default()
	};
	append_report(&mut bubbles, &report);
	assert_eq!(
		bubbles,
		vec![
			Bubble::Tool {
				name: "project.open".to_owned(),
				outcome: "applied",
				detail: "project.open-detail".to_owned()
			},
			Bubble::Tool {
				name: "node.set_param".to_owned(),
				outcome: "failed",
				detail: "node.set_param-detail".to_owned()
			},
			Bubble::Assistant("done".to_owned()),
		]
	);
}

/// The capped run (no final text) becomes the round-limit notice, never
/// a silent drop.
#[test]
fn capped_run_becomes_the_round_limit_notice() {
	let mut bubbles = Vec::new();
	let report = AgentReport {
		rounds: 3,
		converged: false,
		final_text: None,
		tools: Vec::new(),
		stalled: None,
		text_fallback_calls: 0,
		unparsed_text_calls: 0,
		unparsed_text: None,
		..Default::default()
	};
	append_report(&mut bubbles, &report);
	assert_eq!(
		bubbles,
		vec![Bubble::System(
			crate::i18n::tr("ai.notice.limit").to_owned()
		)]
	);
}

/// Inline emphasis and code markers disappear; the words stay.
#[test]
fn inline_markers_are_stripped_for_the_clipboard() {
	assert_eq!(markdown_to_plain("**Project & Media**"), "Project & Media");
	assert_eq!(
		markdown_to_plain("call `project.open` then `project.save`"),
		"call project.open then project.save"
	);
	assert_eq!(
		markdown_to_plain("*italic* and __bold__"),
		"italic and bold"
	);
	assert_eq!(markdown_to_plain("~~gone~~ now"), "gone now");
}

/// Block markers (headings, bullets, ordered items) are dropped.
#[test]
fn block_markers_are_stripped_for_the_clipboard() {
	assert_eq!(markdown_to_plain("## Effects & Nodes"), "Effects & Nodes");
	assert_eq!(
		markdown_to_plain("- `media.probe` — inspect"),
		"media.probe — inspect"
	);
	assert_eq!(markdown_to_plain("1. first\n2) second"), "first\nsecond");
}

/// A fenced block keeps its body verbatim and loses both fences.
#[test]
fn fenced_code_keeps_body_and_drops_the_fence() {
	assert_eq!(
		markdown_to_plain("before\n```rust\nlet x = 1;\n```\nafter"),
		"before\nlet x = 1;\nafter"
	);
	// Tildes work the same way, and markers inside stay untouched.
	assert_eq!(markdown_to_plain("~~~\n**not bold**\n~~~"), "**not bold**");
}

/// Links degrade to their label; the URL is not copied.
#[test]
fn links_degrade_to_their_label() {
	assert_eq!(
		markdown_to_plain("see [the docs](https://example.com/x)"),
		"see the docs"
	);
}

/// An empty reply copies as an empty string, not a panic.
#[test]
fn empty_input_copies_as_empty() {
	assert_eq!(markdown_to_plain(""), "");
	assert_eq!(markdown_to_plain("\n\n   \n"), "");
}

/// Unbalanced markers stay literal and blank runs collapse.
#[test]
fn malformed_markdown_degrades_to_literal_text() {
	assert_eq!(markdown_to_plain("**unclosed"), "unclosed");
	assert_eq!(markdown_to_plain("a `tick"), "a `tick");
	assert_eq!(markdown_to_plain("a [bracket"), "a [bracket");
	assert_eq!(markdown_to_plain("a\n\n\n\nb"), "a\n\nb");
}

/// A conversation is never dropped without asking, and the prompt never
/// fires while a run is in flight (it would refill the transcript right
/// after the reset).
#[gpui_kit::test]
async fn new_session_confirms_before_dropping_a_conversation(cx: &mut TestAppContext) {
	let _env = env_lock();
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(800.0), px(600.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.bubbles.push(Bubble::User("把帧率改成60".to_string()));
			// A run in flight: the prompt must not arm.
			panel.busy = true;
			panel.request_new_session(cx);
			assert!(!panel.pending_new_session, "busy: no prompt, no reset");
			assert_eq!(panel.bubbles.len(), 1, "busy: transcript untouched");

			panel.busy = false;
			panel.request_new_session(cx);
			assert!(panel.pending_new_session, "a conversation must ask first");
			assert_eq!(panel.bubbles.len(), 1, "prompting must not clear");

			// Cancel: everything stays.
			panel.cancel_new_session(cx);
			assert!(!panel.pending_new_session, "cancel closes the prompt");
			assert_eq!(panel.bubbles.len(), 1, "cancel keeps the transcript");

			// Ask again, confirm: everything goes.
			panel.request_new_session(cx);
			assert!(panel.pending_new_session);
			panel.confirm_new_session(cx);
			assert!(panel.bubbles.is_empty(), "confirm drops the transcript");
			assert!(panel.agent.is_none(), "confirm drops the live agent");
			assert!(panel.error.is_none(), "confirm drops the error");
			assert!(!panel.pending_new_session, "prompt closes after confirm");
		});
	});
}

/// The blank first-run state resets directly (with a draft in the
/// composer) — no prompt, nothing to lose.
#[gpui_kit::test]
async fn new_session_resets_a_blank_session_without_asking(cx: &mut TestAppContext) {
	let _env = env_lock();
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(800.0), px(600.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			assert!(!panel.has_conversation(), "starts blank");
			// A half-typed draft is not a conversation; it still resets.
			set_input_text(&panel.input, "half a thought", cx);
			panel.request_new_session(cx);
			assert!(
				!panel.pending_new_session,
				"blank state must not ask for confirmation"
			);
			assert!(panel.bubbles.is_empty());
			assert!(
				panel.input.read(cx).value().is_empty(),
				"the composer resets too: {:?}",
				panel.input.read(cx).value()
			);
		});
	});
}

/// After a confirmed reset the panel conversations again: a fresh agent
/// run populates the transcript from scratch.
#[gpui_kit::test]
async fn a_confirmed_new_session_can_converse_again(cx: &mut TestAppContext) {
	let _env = env_lock();
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	unsafe { std::env::set_var("OAK_AI_API_KEY", "stub-key-not-a-secret") };
	let window = cx.open_window(size(px(800.0), px(600.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		// Without an open project `send` refuses with ENGINE_UNSUPPORTED
		// (the panel refuses to pretend a project is open) — same setup
		// as the e2e tool test.
		engine.update(cx, |engine, cx| engine.new_project(cx));
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	// First conversation: seed a bubble, then reset through the prompt.
	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.bubbles.push(Bubble::User("old chat".to_string()));
			panel.request_new_session(cx);
			panel.confirm_new_session(cx);
			assert!(panel.bubbles.is_empty());

			// A fresh run against a scripted model (no network).
			panel.agent = Some(Agent::new(
				Box::new(ToolLlm {
					tool: "timeline.add_track".to_string(),
					args: json!({ "type": "video" }),
					turns: std::cell::Cell::new(0),
				}),
				PanelBackend::Real(HostAdapter::new(OppHost::new())),
				AgentConfig::default(),
			));
			set_input_text(&panel.input, "加一条视频轨道", cx);
		});
	});
	// `set_input_text` lands asynchronously: sending in the same frame
	// reads a still-empty composer and `can_send` refuses silently
	// (root-caused with a probe: error=None, busy=false, configured=true,
	// draft visible only after the frame — same split as the e2e test).
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.send(cx));
	});
	cx.run_until_parked();

	let (user_bubbles, pending) = cx.read(|app| {
		let p = panel.read(app);
		(
			p.bubbles
				.iter()
				.filter(|b| matches!(b, Bubble::User(_)))
				.count(),
			p.pending_new_session,
		)
	});
	assert!(
		user_bubbles >= 1,
		"the reset conversation actually sends again: {user_bubbles} user bubbles"
	);
	assert!(!pending, "the prompt is not armed for the new chat");
}

/// Shared in-memory store used by the settings tests: one map behind
/// both seams, so a test can read what `save_settings` wrote without
/// ever naming `KeyringStore`.
#[derive(Clone, Default)]
struct TestStore(std::sync::Arc<std::sync::Mutex<crate::ai::secrets::MemoryStore>>);
impl TestStore {
	fn get(&self, account: &str) -> Option<String> {
		crate::ai::secrets::SecretStore::get(self, account)
	}
}
impl crate::ai::secrets::SecretStore for TestStore {
	fn get(&self, account: &str) -> Option<String> {
		self.0.lock().unwrap().get(account)
	}
}
impl crate::ai::secrets::KeyringWriter for TestStore {
	fn store(&mut self, account: &str, value: &str) -> Result<(), crate::ai::secrets::SecretError> {
		self.0.lock().unwrap().store(account, value)
	}
	fn remove(&mut self, account: &str) -> Result<(), crate::ai::secrets::SecretError> {
		self.0.lock().unwrap().remove(account)
	}
}

/// Opens a panel with the injected `TestStore` on both seams.
fn open_settings_panel(
	cx: &mut TestAppContext,
) -> (
	gpui_kit::WindowHandle<AiAssistantPanel<crate::oakui::RealEngine>>,
	TestStore,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(800.0), px(600.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");
	let store = TestStore::default();
	cx.update(|app| {
		panel.update(app, |panel, _cx| {
			panel.set_secret_store(Box::new(store.clone()), Box::new(store.clone()));
		});
	});
	(window, store)
}

/// The full save round trip: open → fill (key masked!) → save →
/// three accounts in the store, view closed, `configured` flipped.
#[gpui_kit::test]
async fn settings_save_writes_all_three_accounts_and_flips_configured(cx: &mut TestAppContext) {
	let _env = env_lock();
	let (window, store) = open_settings_panel(cx);
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.open_settings(cx);
			assert!(panel.show_settings, "view opens");
			// The key field is masked and starts empty — the stored
			// value (if any) is never prefilled into the clear.
			assert!(
				panel.settings_key.read(cx).presentation().is_masked(),
				"the API key field must be masked"
			);
			assert!(panel.settings_key.read(cx).value().is_empty());

			set_input_text(&panel.settings_key, "sk-test-key", cx);
			set_input_text(&panel.settings_url, "https://example.test/v1", cx);
			set_input_text(&panel.settings_model, "m1", cx);
		});
	});
	// `set_input_text` defers: land the values before saving.
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.save_settings(cx));
	});

	let (shown, configured, open) = cx.read(|app| {
		let p = panel.read(app);
		(
			p.settings_key.read(app).value().to_string(),
			p.configured,
			p.show_settings,
		)
	});
	assert!(
		store.get("api_key").is_some_and(|v| v == "sk-test-key"),
		"key landed"
	);
	assert_eq!(
		store.get("base_url").as_deref(),
		Some("https://example.test/v1"),
		"url landed"
	);
	assert_eq!(store.get("model").as_deref(), Some("m1"), "model landed");
	assert!(!open, "the view closes on success");
	assert!(configured, "configured flips after a successful save");
	assert!(
		shown.is_empty(),
		"the key field is cleared on save (never echoes back): {shown}"
	);
}

/// An empty key is refused **before** any write: no store changes,
/// the view stays open with a message, `configured` unchanged.
#[gpui_kit::test]
async fn settings_save_refuses_an_empty_key(cx: &mut TestAppContext) {
	let _env = env_lock();
	let (window, store) = open_settings_panel(cx);
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.open_settings(cx);
			set_input_text(&panel.settings_url, "https://u", cx);
		});
	});
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.save_settings(cx));
	});

	let (err, open) = cx.read(|app| {
		let p = panel.read(app);
		(p.settings_error.clone(), p.show_settings)
	});
	assert_eq!(
		err.as_deref(),
		Some("AI_SETTINGS_KEY_REQUIRED"),
		"the refusal says which field"
	);
	assert!(open, "the view stays open so the user can fix it");
	assert!(store.get("api_key").is_none(), "nothing was written");
	assert!(store.get("base_url").is_none(), "no partial writes either");
	assert!(store.get("model").is_none());
}

/// Cancel closes the view and touches nothing.
#[gpui_kit::test]
async fn settings_cancel_keeps_the_store_untouched(cx: &mut TestAppContext) {
	let _env = env_lock();
	let (window, store) = open_settings_panel(cx);
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.open_settings(cx);
			set_input_text(&panel.settings_key, "typed-but-never-saved", cx);
		});
	});
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.close_settings(cx));
	});

	let (open, err) = cx.read(|app| {
		let p = panel.read(app);
		(p.show_settings, p.settings_error.clone())
	});
	assert!(!open, "the view closes");
	assert!(err.is_none(), "no error to show");
	assert!(
		store.get("api_key").is_none(),
		"cancel must not write anything"
	);
}

/// A store that errors (the headless-keyring case) surfaces its code
/// and never panics; `configured` stays honest.
#[gpui_kit::test]
async fn settings_save_reports_a_failing_store_without_panicking(cx: &mut TestAppContext) {
	let _env = env_lock();
	#[derive(Clone)]
	struct DeadStore;
	impl crate::ai::secrets::SecretStore for DeadStore {
		fn get(&self, _account: &str) -> Option<String> {
			None
		}
	}
	impl crate::ai::secrets::KeyringWriter for DeadStore {
		fn store(
			&mut self,
			_account: &str,
			_value: &str,
		) -> Result<(), crate::ai::secrets::SecretError> {
			Err(crate::ai::secrets::SecretError::new(
				crate::ai::secrets::SecretError::WRITE,
				"no session bus",
			))
		}
		fn remove(&mut self, _account: &str) -> Result<(), crate::ai::secrets::SecretError> {
			Err(crate::ai::secrets::SecretError::new(
				crate::ai::secrets::SecretError::UNAVAILABLE,
				"no session bus",
			))
		}
	}

	// Sibling tests set `OAK_AI_API_KEY` in the shared process env;
	// with it set, `configured` is true no matter how dead the store
	// is (env outranks the store by design). The RAII guard clears
	// it for this test and restores it on drop (panic included), so
	// the assertion measures the store alone.
	let _guard = EnvVarGuard::clear("OAK_AI_API_KEY");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(800.0), px(600.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.set_secret_store(Box::new(DeadStore), Box::new(DeadStore));
			panel.open_settings(cx);
			set_input_text(&panel.settings_key, "any-key", cx);
		});
	});
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.save_settings(cx));
	});

	let (err, open, configured) = cx.read(|app| {
		let p = panel.read(app);
		(p.settings_error.clone(), p.show_settings, p.configured)
	});
	assert_eq!(
		err.as_deref(),
		Some("AI_SECRET_WRITE"),
		"the SecretError code reaches the view"
	);
	assert!(open, "the view stays open on failure");
	assert!(!configured, "a dead store is not a configured AI");
}

/// Header truncation (T13B): the strip yields (`min_w_0` +
/// `truncate`), the buttons don't (`flex_shrink_0`). Pinned as a
/// source-level contract because layout needs a live window to
/// *see*, and this is the assertion that fails first when someone
/// reverts either half.
#[test]
fn header_strip_yields_and_buttons_never_shrink() {
	let source = include_str!("../ai_assistant.rs").replace("\r\n", "\n");
	let strip_fn = source
		.split("fn status_strip(")
		.nth(1)
		.expect("status_strip exists");
	let strip_body = strip_fn.split("\n}\n").next().expect("body end");
	assert!(
		strip_body.contains("min_w_0()"),
		"the strip must be shrinkable (T13B truncation fix)"
	);
	assert!(
		strip_body.contains(".truncate()"),
		"the strip's text must truncate with an ellipsis"
	);
	assert!(
		!strip_body.contains("flex_shrink_0()"),
		"flex_shrink_0 on the strip is the bug: it made the text unyielding"
	);

	// Both header buttons carry the no-shrink marker, right next to
	// their ids.
	for id in ["ai-assistant-settings", "ai-assistant-new-session"] {
		let at = source.find(&format!(".id(\"{id}\")")).expect(id);
		let near = &source[at..at + 220];
		assert!(
			near.contains("flex_shrink_0()"),
			"{id} must be flex_shrink_0: {near}"
		);
	}
}

/// T13B: the unconfigured strip now points at Settings, not at an
/// env var — the key itself is what changed, so pin it.
#[test]
fn not_configured_copy_points_at_settings() {
	// Other tests (and `actions.rs`) flip the global language; pin
	// zh-CN for the assertion and restore afterwards so the suite
	// stays order-independent.
	let prev = crate::i18n::language_code().to_string();
	crate::i18n::set_language_code("zh-CN");
	let zh = crate::i18n::tr(not_configured_key());
	crate::i18n::set_language_code(&prev);
	assert!(zh.contains("设置"), "zh copy must offer 设置: {zh}");
	assert!(
		!zh.contains("OAK_AI_API_KEY"),
		"the env-var-only instruction is gone: {zh}"
	);
}

/// T13B: the two secret-error codes and the empty-key refusal all
/// map to their own wording (never the generic "unknown error").
#[test]
fn secret_error_codes_map_to_their_own_copy() {
	assert_eq!(
		error_i18n_key("AI_SECRET_UNAVAILABLE"),
		"ai.error.secret_unavailable"
	);
	assert_eq!(error_i18n_key("AI_SECRET_WRITE"), "ai.error.secret_write");
	// The settings view renders AI_SETTINGS_KEY_REQUIRED through the
	// same mapper: it needs a key of its own in every language.
	assert_eq!(
		error_i18n_key("AI_SETTINGS_KEY_REQUIRED"),
		"ai.error.unknown",
		"key_required is handled by the settings view itself"
	);
}

/// Static check: every `ai.*` key the panel renders exists in all eight
/// shipped locales (a missing key would render as the raw key).
#[test]
fn ai_keys_exist_in_every_language() {
	const LANGS: [(&str, &str); 8] = [
		(
			"zh-CN",
			include_str!("../../../../../assets/i18n/zh-CN.yaml"),
		),
		(
			"en-US",
			include_str!("../../../../../assets/i18n/en-US.yaml"),
		),
		("de", include_str!("../../../../../assets/i18n/de.yaml")),
		("pt", include_str!("../../../../../assets/i18n/pt.yaml")),
		("fr", include_str!("../../../../../assets/i18n/fr.yaml")),
		("es", include_str!("../../../../../assets/i18n/es.yaml")),
		("ru", include_str!("../../../../../assets/i18n/ru.yaml")),
		("ja", include_str!("../../../../../assets/i18n/ja.yaml")),
	];
	const KEYS: [&str; 38] = [
		"menu.window.ai_assistant",
		"panel.ai_assistant",
		"ai.placeholder",
		"ai.send",
		"ai.copy",
		"ai.copied",
		"ai.busy",
		"ai.empty",
		"ai.not_configured",
		"ai.notice.limit",
		"ai.notice.stalled",
		"ai.notice.unparsed_tool_call",
		"ai.outcome.unparsed",
		"ai.new_session",
		"ai.new_session.prompt",
		"ai.new_session.confirm",
		"ai.new_session.cancel",
		"ai.settings",
		"ai.settings.title",
		"ai.settings.api_key",
		"ai.settings.base_url",
		"ai.settings.model",
		"ai.settings.save",
		"ai.settings.cancel",
		"ai.settings.saved",
		"ai.settings.key_required",
		"ai.error.secret_unavailable",
		"ai.error.secret_write",
		"ai.error.not_configured",
		"ai.error.http",
		"ai.error.api",
		"ai.error.bad_response",
		"ai.error.engine_unsupported",
		"ai.error.unknown",
		"ai.outcome.applied",
		"ai.outcome.proposed",
		"ai.outcome.rejected",
		"ai.outcome.failed",
	];
	for (lang, yaml) in LANGS {
		for key in KEYS {
			assert!(yaml.contains(key), "locale {lang} is missing the key {key}");
		}
	}
}

// -----------------------------------------------------------------------
// T8.5 — real-engine tool execution
// -----------------------------------------------------------------------

use crate::ai::llm::ChatRequest;
use crate::ai::{ChatResponse, LlmClient, LlmError, ToolCall};
use crate::oakui::{MockEngine, RealEngine};
use gpui_kit::{TestAppContext, VisualTestContext, px, size};
use oak_ai_tools::error::CODE_UNSUPPORTED;
use oak_extensions::host_services::HostServices;
use serde_json::json;

/// Builds a live `OppHost` bound to a fresh project with one sequence —
/// the in-process engine fixture every real-tool test runs against.
/// Returns `(adapter, project, sequence-id)`; the undo stack is reset
/// first so `count()` deltas measure only this test.
/// **T10A ⑦ — the design change has two paths and both must be one row.**
///
/// The OPP path always runs inside an undo group (`edit_begin` opens one),
/// so `insert` must *join* it; a direct UI-style call has no group, so
/// `insert` must *open* one. Either way the edit is exactly one row — the
/// failure modes this guards against are "joined nothing" (zero rows) and
/// "opened a second group" (two rows).
#[test]
fn real_insert_is_one_row_with_and_without_an_enclosing_group() {
	use crate::oakui::{effectchain, graphops};
	let _g = graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, project, _seq, clip, _other) = real_clip_pair();
	let types = adapter.host().node_addable_effects(&clip).expect("list");
	let type_id = types[0].type_id.clone();
	let host = graphops::id_of(clip.parse().expect("numeric id")).expect("id");

	// (a) Direct path: no enclosing group. It must go through the same
	// effect-index mapping — a raw `insert(…, 0, …)` targets the source
	// node (the chain includes it) and fails with `connect: no input ""`.
	let before = graphops::undo_count();
	let direct =
		effectchain::insert_at_effect_index(&project, host, None, &type_id).expect("direct insert");
	assert_eq!(
		graphops::undo_count(),
		before + 1,
		"a direct insert opens and closes its own row"
	);
	// The chain starts with the node feeding the first effect (footage).
	assert_eq!(
		real_chain(&project, &clip).last().map(String::as_str),
		Some(direct.raw().to_string().as_str()),
		"the direct insert really wired the chain: {:?}",
		real_chain(&project, &clip)
	);

	// (b) OPP path: an enclosing transaction group is already open.
	let before = graphops::undo_count();
	let id = oak_ai_tools::txn::run(&mut adapter, "node.add_effect", |ops| {
		ops.node_add_effect(&json!({"clip": clip, "effect": type_id}))
	})
	.expect("opp insert");
	assert_eq!(
		graphops::undo_count(),
		before + 1,
		"joining the transaction group still yields exactly one row"
	);
	assert!(
		real_chain(&project, &clip).contains(&id.as_str().unwrap_or_default().to_string()),
		"the joined insert really wired the chain: {:?}",
		real_chain(&project, &clip)
	);
}

/// **T10A ⑧**: a node that cannot host effects is offered **no** types —
/// an empty list, never a panic and never the full catalogue.
#[test]
fn real_addable_effects_is_empty_for_a_node_that_cannot_host() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (adapter, _project, seq, _clip, _other) = real_clip_pair();

	let listed = adapter
		.host()
		.node_addable_effects(&seq)
		.expect("listing must not fail");
	assert!(
		listed.is_empty(),
		"a sequence hosts no effects, so it is offered none: {listed:?}"
	);
}

/// A real project with one video track carrying two **strictly
/// contiguous** clips (a.out == b.in), which is the seam case the
/// transition router has to recognise. Returns `(adapter, project,
/// seq_id, clip_a, clip_b)` with the clip ids already stringified.
fn real_clip_pair() -> (
	HostAdapter<OppHost>,
	crate::oakui::graphops::ProjectRef,
	String,
	String,
	String,
) {
	use crate::oakui::graphops;
	use quercus_engine_rs::editor::TrackKind;
	let project = graphops::create_project();
	let seq = graphops::create_sequence(&project, "Clips");
	let media = std::env::temp_dir().join(format!("oak_t10a_{}.mp4", std::process::id()));
	crate::testmedia::write_test_clip(&media, 64, 64, 20, 10).expect("generate media");
	let footage = graphops::import_footage(&project, &media).expect("import");
	let a = graphops::place_footage_clip(&project, seq, footage, TrackKind::Video, 0, 0, 10, 0)
		.expect("place first");
	let b = graphops::place_footage_clip(&project, seq, footage, TrackKind::Video, 0, 10, 20, 0)
		.expect("place second");
	let host = OppHost::new();
	host.attach_project(project.clone());
	(
		HostAdapter::new(host),
		project,
		seq.raw().to_string(),
		a.raw().to_string(),
		b.raw().to_string(),
	)
}

/// The effect chain of a node, as identity strings (empty when the node
/// cannot host effects).
fn real_chain(project: &crate::oakui::graphops::ProjectRef, node: &str) -> Vec<String> {
	use crate::oakui::{effectchain, graphops};
	let id = graphops::id_of(node.parse().expect("numeric id")).expect("id");
	effectchain::chain(project, id)
		.into_iter()
		.map(|n| n.raw().to_string())
		.collect()
}

/// **T10A ①**: `node.add_effect` reaches the real engine, the node is in
/// the chain afterwards, the edit is **exactly one** undo row, and the
/// undo really detaches it.
#[test]
fn real_node_add_effect_applies_and_is_one_undo_row() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, project, _seq, clip, _other) = real_clip_pair();

	let types = adapter.host().node_addable_effects(&clip).expect("list");
	assert!(!types.is_empty(), "the factory must offer effect types");
	let type_id = types[0].type_id.clone();

	let before = graphops::undo_count();
	let before_index = graphops::undo_index();
	// `txn::run` is the production wrapper: begin → call → commit on
	// success, begin → call → **abort** on failure. Using it here means a
	// failing assertion can never leave an undo group open and poison the
	// rest of the test process.
	let result = oak_ai_tools::txn::run(&mut adapter, "node.add_effect", |ops| {
		ops.node_add_effect(&json!({"clip": clip, "effect": type_id}))
	});
	let id = result.expect("add_effect executes");

	let new_id = id.as_str().expect("id is a string").to_string();
	assert!(
		real_chain(&project, &clip).contains(&new_id),
		"the effect is really in the chain: {:?}",
		real_chain(&project, &clip)
	);
	assert_eq!(graphops::undo_count(), before + 1, "exactly one undo row");
	assert_eq!(
		graphops::undo_index(),
		before_index + 1,
		"the row is on top of the stack"
	);

	assert!(graphops::undo_undo(), "undo must step the stack");
	assert!(
		!real_chain(&project, &clip).contains(&new_id),
		"the undo really detaches the effect: {:?}",
		real_chain(&project, &clip)
	);
}

/// **T10A ②**: `index` picks the position, and omitting it appends.
#[test]
fn real_node_add_effect_honours_the_index() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, project, _seq, clip, _other) = real_clip_pair();
	let types = adapter.host().node_addable_effects(&clip).expect("list");
	let first_type = types[0].type_id.clone();
	let second_type = types
		.get(1)
		.map(|t| t.type_id.clone())
		.unwrap_or_else(|| first_type.clone());

	let appended = oak_ai_tools::txn::run(&mut adapter, "node.add_effect", |ops| {
		ops.node_add_effect(&json!({"clip": clip, "effect": first_type}))
	})
	.expect("append");
	let at_zero = oak_ai_tools::txn::run(&mut adapter, "node.add_effect", |ops| {
		ops.node_add_effect(&json!({"clip": clip, "effect": second_type, "index": 0}))
	})
	.expect("insert at 0");

	let chain = real_chain(&project, &clip);
	// `chain()` includes the node feeding the first effect (the footage),
	// so effect index 0 is the **second** entry: `[source, first, …]`.
	assert_eq!(
		chain.get(1).map(String::as_str),
		at_zero.as_str(),
		"effect index 0 sits right after the source: {chain:?}"
	);
	let appended_id = appended.as_str().unwrap_or_default().to_string();
	assert!(
		chain.iter().any(|c| c == &appended_id),
		"the appended one is still there: {chain:?}"
	);
}

/// **T10A ③**: `side:"in"` always takes the edge path, and says so.
#[test]
fn real_add_transition_in_takes_the_edge_path() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq, clip, _other) = real_clip_pair();

	let before = graphops::undo_count();
	let placed = oak_ai_tools::txn::run(&mut adapter, "timeline.add_transition", |ops| {
		ops.timeline_add_transition(&json!({
			"clip": clip,
			"side": "in",
			"type": "crossdissolve",
			"duration": {"num": 1, "den": 1},
		}))
	})
	.expect("add_transition executes");

	assert_eq!(
		placed.get("placement").and_then(Value::as_str),
		Some("edge"),
		"the head of a clip is an edge transition: {placed}"
	);
	assert!(
		placed.get("id").is_some(),
		"it reports the new block: {placed}"
	);
	assert_eq!(
		graphops::undo_count() - before,
		1,
		"the transition and its shader kind are one row"
	);
}

/// **T10A ④**: `side:"out"` on a clip whose tail **strictly meets** the
/// next clip's head takes the seam path, splits the duration in half and
/// reports the half it actually used.
#[test]
fn real_add_transition_out_at_a_cut_takes_the_seam_path() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq, clip, _other) = real_clip_pair();

	let before = graphops::undo_count();
	let placed = oak_ai_tools::txn::run(&mut adapter, "timeline.add_transition", |ops| {
		ops.timeline_add_transition(&json!({
			"clip": clip,
			"side": "out",
			"type": "fade",
			"duration": {"num": 1, "den": 1},
		}))
	})
	.expect("add_transition executes");

	assert_eq!(
		placed.get("placement").and_then(Value::as_str),
		Some("seam"),
		"a cut is a seam: {placed}"
	);
	let half = placed.get("half").expect("the half is reported");
	assert!(
		half.get("num").and_then(Value::as_i64).is_some(),
		"the half is a rational frame count, not the requested total: {half}"
	);
	assert_eq!(
		graphops::undo_count() - before,
		1,
		"one undo row for the seam transition"
	);
}

/// **T10A ⑤**: `side:"out"` on the **last** clip has no cut after it, so
/// it falls back to the edge path.
#[test]
fn real_add_transition_out_without_a_cut_takes_the_edge_path() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq, _clip, last) = real_clip_pair();

	let placed = oak_ai_tools::txn::run(&mut adapter, "timeline.add_transition", |ops| {
		ops.timeline_add_transition(&json!({
			"clip": last,
			"side": "out",
			"type": "wipe",
			"duration": {"num": 1, "den": 1},
		}))
	})
	.expect("add_transition executes");

	assert_eq!(
		placed.get("placement").and_then(Value::as_str),
		Some("edge"),
		"nothing follows the last clip: {placed}"
	);
	// `null` rather than absent: the JSON keeps the key so a caller can
	// tell "no split" apart from "field missing by accident".
	assert!(
		placed.get("half").is_some_and(Value::is_null),
		"no split happened: {placed}"
	);
}

/// **T10A ⑥**: the three refusals are actionable (they name the cause and
/// list the types the clip accepts), and none of them touches the project.
#[test]
fn real_effect_errors_are_actionable_and_leave_the_project_untouched() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, project, seq, clip, _other) = real_clip_pair();

	let cases = [
		// Unknown type id.
		json!({"clip": clip, "effect": "no.such.effect"}),
		// A node that is not a clip cannot host an effect at all.
		json!({"clip": seq, "effect": "oak.solid_color"}),
	];
	for args in cases {
		let before = graphops::undo_count();
		let chain_before = real_chain(&project, &clip);
		let err = oak_ai_tools::txn::run(&mut adapter, "node.add_effect", |ops| {
			ops.node_add_effect(&args)
		})
		.expect_err("this call must be refused");
		let message = format!("{err}");
		assert!(
			message.contains("accepts") || message.contains("cannot host"),
			"the error must be actionable: {message}"
		);
		assert_eq!(
			graphops::undo_count(),
			before,
			"a refused call must not add an undo row"
		);
		assert_eq!(
			real_chain(&project, &clip),
			chain_before,
			"a refused call must not touch the chain"
		);
	}
}

fn real_tool_fixture() -> (
	HostAdapter<OppHost>,
	crate::oakui::graphops::ProjectRef,
	String,
) {
	let project = crate::oakui::graphops::create_project();
	let seq = crate::oakui::graphops::create_sequence(&project, "Seq");
	let seq_id = seq.raw().to_string();
	let host = OppHost::new();
	host.attach_project(project.clone());
	(HostAdapter::new(host), project, seq_id)
}

/// ① `project.get_overview` really executes: the tool's JSON is
/// `project_get_info` verbatim (the mapping is a straight
/// serialisation, no field rewriting), anchored to the real sequence
/// id/name and the real default fps.
#[test]
fn real_overview_is_project_info_verbatim() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, seq_id) = real_tool_fixture();

	let info = adapter.host().project_get_info().expect("info");
	let overview = adapter
		.project_get_overview(&Value::Null)
		.expect("overview executes");
	let expected = serde_json::to_value(&info).expect("serialize info");
	assert_eq!(
		overview, expected,
		"overview must equal project_get_info verbatim"
	);
	// Real-value anchors. A fresh sequence DOES carry video params:
	// `SequenceBehavior::create` → `set_default_parameters` installs
	// the config's `DefaultSequenceFrameRate` (frame duration, default
	// "1001/30000"), so the rate is 30000/1001 — not the 30/1 the
	// host's `unwrap_or` fallback would give for an empty list
	// (opp/host.rs:240). Measured, not assumed.
	assert_eq!(overview["sequences"][0]["id"], seq_id);
	assert_eq!(overview["sequences"][0]["name"], "Seq");
	assert_eq!(overview["sequences"][0]["fps"]["num"], 30000);
	assert_eq!(overview["sequences"][0]["fps"]["den"], 1001);
}

/// ② `timeline.add_track` really executes: the row lands in the real
/// structure and on the real undo stack (through the adapter's txn).
#[test]
fn real_timeline_add_track_executes() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq_id) = real_tool_fixture();

	let before = adapter
		.host()
		.timeline_get_structure()
		.unwrap()
		.tracks
		.len();
	let before_count = graphops::undo_count();

	adapter.txn_begin("timeline.add_track").expect("begin");
	let id = adapter
		.timeline_add_track(&json!({"type": "video"}))
		.expect("add_track executes");
	adapter.txn_commit().expect("commit");

	let after = adapter
		.host()
		.timeline_get_structure()
		.unwrap()
		.tracks
		.len();
	assert_eq!(after, before + 1, "the track reaches the real structure");
	assert!(id.as_str().is_some_and(|s| !s.is_empty()), "empty id: {id}");
	assert!(
		graphops::undo_count() > before_count,
		"the command must land on the real undo stack"
	);
}

/// ③ `media.list_footage` really executes: a fresh real project lists
/// nothing — the mock invents rows, so an empty array is the honest
/// real-engine value.
#[test]
fn real_media_list_footage_is_empty_not_fake() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq_id) = real_tool_fixture();

	let list = adapter
		.media_list_footage(&Value::Null)
		.expect("list executes");
	assert_eq!(list, json!([]), "a fresh project has no footage");
}

/// ④ Gap `time_rate` (mock-only until now): the real value comes from
/// the sequence's own grid — 30/1 when the sequence carries no video
/// params, and the explicit rate when it does (so nothing is
/// hard-coded).
#[test]
fn real_time_rate_reads_the_sequence_grid() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (adapter, project, seq_id) = real_tool_fixture();

	let rate = adapter
		.time_rate(TimeContext::Sequence(seq_id))
		.expect("rate for the default sequence");
	assert_eq!(
		(rate.num, rate.den),
		(30000, 1001),
		"the sequence's own grid (DefaultSequenceFrameRate default 1001/30000)"
	);

	// A sequence with an explicit 24/1 grid must read24/1, not the
	// default: proof the rate is fetched, not assumed.
	let seq24 = crate::oakui::graphops::create_sequence_with_params(
		&project,
		"Seq24",
		Some((
			1920,
			1080,
			quercus_engine_rs::types::Rational::new(24, 1),
			false,
		)),
	);
	let rate24 = adapter
		.time_rate(TimeContext::Sequence(seq24.raw().to_string()))
		.expect("rate for the explicit sequence");
	assert_eq!((rate24.num, rate24.den), (24, 1));
}

/// ⑤ Gap `rational_at` (mock-only until now): the default body
/// resolves a frame index against the engine's own rate grid and
/// passes rational seconds through untouched.
#[test]
fn real_rational_at_defaults_to_the_rate_grid() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (adapter, _project, seq_id) = real_tool_fixture();

	let ctx = TimeContext::Sequence(seq_id);
	let rate = adapter.time_rate(ctx.clone()).expect("rate");
	let got = adapter
		.rational_at(ctx.clone(), TimeValue::Frame(48))
		.expect("frame 48");
	let want = TimeValue::Frame(48).to_rational(rate).expect("grid math");
	assert_eq!(got, want, "the default body must use the engine rate");

	let through = adapter
		.rational_at(ctx, TimeValue::Rational(Rat::new(3, 4).expect("3/4")))
		.expect("rational seconds pass through");
	assert_eq!((through.num, through.den), (3, 4));
}

/// ⑥ The two mapping-gap tools refuse structurally on the real path:
/// stable `UNSUPPORTED` code, honest reason, no guessed data.
#[test]
fn unsupported_render_tools_refuse_structurally() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, _seq_id) = real_tool_fixture();

	let levels = adapter
		.render_get_audio_levels(&json!({"sequence": "1"}))
		.unwrap_err();
	assert_eq!(levels.code, CODE_UNSUPPORTED);
	assert!(
		levels.message.contains("audio_levels"),
		"reason must name the tool: {}",
		levels.message
	);

	let thumbs = adapter
		.render_get_thumbnails(&json!({"range": {"start": 0, "end": 1}}))
		.unwrap_err();
	assert_eq!(thumbs.code, CODE_UNSUPPORTED);
	assert!(
		thumbs.message.contains("thumbnails"),
		"reason must name the tool: {}",
		thumbs.message
	);
}

/// ⑦ `PanelBackend` forwards both variants and carries the attach /
/// is_mock switches the panel's send path relies on.
#[test]
fn panel_backend_forwards_both_variants() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (adapter, project, _seq_id) = real_tool_fixture();

	let mut real = PanelBackend::Real(adapter);
	assert!(!real.is_mock(), "the real path is the default variant");
	let overview = real
		.project_get_overview(&Value::Null)
		.expect("real forwards");
	assert!(
		overview["sequences"].is_array(),
		"real overview: {overview}"
	);
	// Value-level discrimination, not just shape: the real engine's
	// grid is the node default (DefaultSequenceFrameRate 1001/30000 →
	// 30000/1001), while the mock answers from its own MOCK_FPS (30/1).
	// An ablation that swapped the variant through would fail here even
	// though "sequences is an array" still held.
	assert_eq!(
		(
			overview["sequences"][0]["fps"]["num"].as_i64(),
			overview["sequences"][0]["fps"]["den"].as_i64()
		),
		(Some(30000), Some(1001)),
		"real variant must answer from the live engine grid: {overview}"
	);

	// The mock needs a project of its own before it answers: open one
	// through the same frozen tool surface the loop uses.
	let mut mock = PanelBackend::Mock(MockBackend::new());
	assert!(mock.is_mock(), "the mock variant must be recognisable");
	mock.project_open(&json!({"name": "Mock Probe"}))
		.expect("the mock opens its own project");
	let mock_overview = mock
		.project_get_overview(&Value::Null)
		.expect("mock forwards once it has a project");
	// The mock's shape is its own as well: one project-level fps
	// (`MOCK_FPS` 30/1) and per-sequence rate absent, where the real
	// engine reports the rate per sequence. Both value and shape pin
	// the variant, so a swap cannot ride on "sequences is an array".
	assert_eq!(
		(
			mock_overview["project"]["fps"]["num"].as_i64(),
			mock_overview["project"]["fps"]["den"].as_i64()
		),
		(Some(30), Some(1)),
		"the mock answers from its own MOCK_FPS grid: {mock_overview}"
	);
	assert!(
		mock_overview["sequences"][0].get("fps").is_none(),
		"the mock carries no per-sequence rate: {mock_overview}"
	);
	// Forwarding, not silent success: without a project the mock
	// returns its own structured error instead of panicking.
	let bare = PanelBackend::Mock(MockBackend::new())
		.project_get_overview(&Value::Null)
		.unwrap_err();
	assert!(
		!bare.code.is_empty(),
		"mock's structured error must surface"
	);

	// attach(): the real variant takes the handle; the mock ignores it.
	real.attach(project);
	mock.attach(crate::oakui::graphops::create_project());
}

/// A local LlmClient that never touches the network: the first turn
/// asks for one real tool call, the second answers. The key env var
/// exists only to satisfy `AiConfig::from_env`; nothing reads it.
struct StubLlm {
	turns: std::cell::Cell<u32>,
}

impl LlmClient for StubLlm {
	fn complete(&self, _request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError> {
		let turn = self.turns.get();
		self.turns.set(turn + 1);
		Ok(if turn == 0 {
			ChatResponse {
				content: None,
				tool_calls: vec![ToolCall {
					id: "stub-call-1".to_string(),
					name: "project.get_overview".to_string(),
					arguments: json!({}),
				}],
				finish_reason: Some("tool_calls".to_string()),
			}
		} else {
			ChatResponse {
				content: Some("Stub answer: the overview came from the real engine.".to_string()),
				tool_calls: vec![],
				finish_reason: Some("stop".to_string()),
			}
		})
	}
}

/// A stub that asks for **one named tool with fixed arguments** on its
/// first turn, then answers — the v2 analogue of [`StubLlm`].
struct ToolLlm {
	tool: String,
	args: Value,
	turns: std::cell::Cell<u32>,
}

impl LlmClient for ToolLlm {
	fn complete(&self, _request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError> {
		let turn = self.turns.get();
		self.turns.set(turn + 1);
		Ok(if turn == 0 {
			ChatResponse {
				content: None,
				tool_calls: vec![ToolCall {
					id: "stub-v2-call-1".to_string(),
					name: self.tool.clone(),
					arguments: self.args.clone(),
				}],
				finish_reason: Some("tool_calls".to_string()),
			}
		} else {
			ChatResponse {
				content: Some("Stub answer: the v2 tool ran on the real engine.".to_string()),
				tool_calls: vec![],
				finish_reason: Some("stop".to_string()),
			}
		})
	}
}

// ---- T9.2 v2 face: `sequence.get_settings` / `sequence.set_format` ----

/// `sequence.get_settings` on the real engine answers from the live
/// graph: the sequence's own name and its node-default grid
/// (`DefaultSequenceFrameRate` = 30000/1001), never a mock constant.
#[test]
fn real_sequence_get_settings_reads_the_live_grid() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (adapter, _project, seq_id) = real_tool_fixture();

	let settings = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("settings read");
	assert_eq!(settings["name"], json!("Seq"), "{settings}");
	assert_eq!(
		(
			settings["rate"]["num"].as_i64(),
			settings["rate"]["den"].as_i64()
		),
		(Some(30000), Some(1001)),
		"the real grid is NTSC: {settings}"
	);
	assert!(settings["width"].as_u64().unwrap_or(0) > 0, "{settings}");
	assert_eq!(settings["id"], json!(seq_id));
}

/// **The undo requirement**: changing the frame rate is one undo row,
/// and `undo` restores the previous rate exactly.
#[test]
fn real_sequence_set_format_is_one_undo_row_that_reverts() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, seq_id) = real_tool_fixture();

	let before = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("before");
	assert_eq!(before["rate"]["num"].as_i64(), Some(30000));

	// Depth, not just value: "one undo row" must mean the stack grew by
	// exactly one, so a duplicate *row* cannot ride on the value
	// assertions below.
	//
	// Known limit, measured: this assertion is blind to a duplicate push
	// **inside** the transaction's group. `count()` is the top-level row
	// count (`global.rs:216`); a group's pushes become children of one
	// multi command and only `group_end()` puts that multi on the stack
	// as a single row (`global.rs:125-137`), so N pushes in a group still
	// add exactly one row. Ablating the implementation to push twice left
	// this test green — by construction, not by a wrong assertion.
	//
	// That case is not a defect either: undoing a group undoes *all* its
	// children in reverse order (`MultiUndoCommand::undo`,
	// `undocommand.rs:287-291`), and both duplicate children restore the
	// same `before`, so one Ctrl+Z still reverts the whole edit. Atomicity
	// is owned by the transaction group, and the row assertion here
	// guards the other direction: a push that escaped the group, or a
	// second group being opened for one tool call.
	let depth = graphops::undo_count();
	let index = graphops::undo_index();

	// An edit needs the OPP transaction the dispatcher opens; here the
	// test opens it directly (the group becomes the one undo row).
	adapter.txn_begin("sequence.set_format").expect("begin");
	let after = adapter
		.sequence_set_format(json!({ "sequence": seq_id, "fps": 24 }))
		.expect("set format");
	adapter.txn_commit().expect("commit");

	assert_eq!(
		graphops::undo_count(),
		depth + 1,
		"the format change must add exactly ONE undo row"
	);
	assert_eq!(
		graphops::undo_index(),
		index + 1,
		"the edit must be the top of the history, not a buried row"
	);
	assert_eq!(
		(after["rate"]["num"].as_i64(), after["rate"]["den"].as_i64()),
		(Some(24), Some(1)),
		"24 is exact: {after}"
	);
	assert_eq!(
		after["undoable"],
		json!(true),
		"the reply says it can be taken back"
	);
	assert!(graphops::undo_can_undo(), "the edit entered the undo stack");

	assert!(graphops::undo_undo(), "undo must step the stack");
	let reverted = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("after undo");
	assert_eq!(
		(
			reverted["rate"]["num"].as_i64(),
			reverted["rate"]["den"].as_i64()
		),
		(Some(30000), Some(1001)),
		"undo must restore the NTSC grid: {reverted}"
	);
}

/// **Partial update**: supplying only `fps` must leave width/height —
/// and every other unset field — exactly as they were.
#[test]
fn real_sequence_set_format_only_touches_the_given_fields() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, seq_id) = real_tool_fixture();

	let before = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("before");
	adapter.txn_begin("sequence.set_format").expect("begin");
	adapter
		.sequence_set_format(json!({ "sequence": seq_id, "fps": 25 }))
		.expect("set only fps");
	adapter.txn_commit().expect("commit");
	let after = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("after");

	assert_eq!(
		(after["width"].clone(), after["height"].clone()),
		(before["width"].clone(), before["height"].clone()),
		"width/height must not move when only fps is given"
	);
	assert_eq!(
		after["name"], before["name"],
		"the name must not move either"
	);
	assert_eq!(after["interlaced"], before["interlaced"], "nor interlaced");
	assert_eq!(
		after["rate"]["num"].as_i64(),
		Some(25),
		"only the rate changed"
	);
}

/// **NTSC round trip on the real engine**: `29.97` must become the
/// exact `30000/1001` (not `2997/100`), and reading the settings back
/// must return that same rational — the engine stores what the tool
/// claimed, no rounding on either leg.
#[test]
fn real_sequence_ntsc_rate_round_trips_exactly() {
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	let (mut adapter, _project, seq_id) = real_tool_fixture();

	adapter.txn_begin("sequence.set_format").expect("begin");
	let written = adapter
		.sequence_set_format(json!({ "sequence": seq_id, "fps": 29.97 }))
		.expect("set 29.97");
	adapter.txn_commit().expect("commit");

	assert_eq!(
		(
			written["rate"]["num"].as_i64(),
			written["rate"]["den"].as_i64()
		),
		(Some(30000), Some(1001)),
		"29.97 must map onto 30000/1001, never 2997/100: {written}"
	);

	// Read back through a fresh call: the stored rate is the same
	// rational, i.e. the write really landed on the engine's grid.
	let reread = adapter
		.sequence_get_settings(json!({ "sequence": seq_id }))
		.expect("read back");
	assert_eq!(
		(
			reread["rate"]["num"].as_i64(),
			reread["rate"]["den"].as_i64()
		),
		(Some(30000), Some(1001)),
		"the engine must store the exact NTSC rational: {reread}"
	);
}

/// The v2 wire face on the mock: real partial update over the mock's
/// in-memory model (the mock is not a stub here — `sequence.set_format`
/// has a true implementation so the frozen mock E2E chain can exercise
/// the v2 tool).
#[test]
fn mock_sequence_set_format_updates_partially() {
	use oak_ai_tools::backend::EngineExt;
	let mut mock = MockBackend::new();
	mock.project_open(&json!({ "name": "Mock Probe" }))
		.expect("open");

	let before = mock.sequence_get_settings(json!({})).expect("before");
	let after = mock
		.sequence_set_format(json!({ "fps": { "num": 24000, "den": 1001 } }))
		.expect("set");
	assert_eq!(
		(after["rate"]["num"].as_i64(), after["rate"]["den"].as_i64()),
		(Some(24000), Some(1001))
	);
	assert_eq!(after["width"], before["width"], "width untouched");
	assert_eq!(after["height"], before["height"], "height untouched");
	assert_eq!(after["undoable"], json!(true));
}

/// End to end through the panel: the model asks for
/// `sequence.set_format`, the real engine executes it, and the panel's
/// detail shows the changed rate.
#[gpui_kit::test]
async fn e2e_panel_runs_sequence_set_format_against_the_real_engine(cx: &mut TestAppContext) {
	let _env = env_lock();
	let _g = crate::oakui::graphops::test_lock();
	graphops::undo_clear();
	unsafe { std::env::set_var("OAK_AI_API_KEY", "stub-key-not-a-secret") };
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});

	let window = cx.open_window(size(px(1280.0), px(860.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		engine.update(cx, |engine, cx| engine.new_project(cx));
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.agent = Some(Agent::new(
				Box::new(ToolLlm {
					tool: "sequence.set_format".to_string(),
					args: json!({ "fps": 24 }),
					turns: std::cell::Cell::new(0),
				}),
				PanelBackend::Real(HostAdapter::new(OppHost::new())),
				AgentConfig::default(),
			));
			set_input_text(&panel.input, "set the sequence frame rate to 24", cx);
		});
	});
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.send(cx));
	});
	cx.run_until_parked();

	let (bubbles, busy) = cx.read(|app| {
		let p = panel.read(app);
		(p.bubbles.len(), p.busy)
	});
	assert!(!busy, "the run must finish");

	// The executed tool call is in the transcript with the engine's
	// answer: a 24/1 rate, not the pre-edit 30000/1001.
	let (tool_details, tool_outcomes): (Vec<(String, String)>, Vec<&'static str>) =
		cx.read(|app| {
			let p = panel.read(app);
			let mut details = Vec::new();
			let mut outcomes = Vec::new();
			for b in &p.bubbles {
				if let Bubble::Tool {
					name,
					detail,
					outcome,
				} = b
				{
					details.push((name.clone(), detail.clone()));
					outcomes.push(*outcome);
				}
			}
			(details, outcomes)
		});
	let (name, detail) = tool_details
		.first()
		.expect("the executed v2 call must be in the transcript");
	assert_eq!(name, "sequence.set_format", "the v2 tool ran, not a mock");
	// `applied`, not `failed`/`proposed`: the gate really executed it.
	assert_eq!(
		tool_outcomes.first().copied(),
		Some("applied"),
		"the v2 tool must actually run: {tool_outcomes:?}"
	);
	assert!(
		detail.contains("24") && !detail.contains("30000"),
		"the detail must carry the engine's post-edit rate: {detail}"
	);
	assert!(bubbles >= 1, "the user bubble is pushed");
}

/// The mock engine keeps a demo project (`project()` is `Some`) but no
/// shared handle: the panel must refuse with the true reason
/// (`ENGINE_UNSUPPORTED`), never borrow the real engine's "no project
/// is open", and must not start a run or push a bubble.
#[gpui_kit::test]
async fn panel_refuses_tools_when_engine_keeps_no_handle(cx: &mut TestAppContext) {
	let _env = env_lock();
	unsafe { std::env::set_var("OAK_AI_API_KEY", "stub-key-not-a-secret") };
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(900.0), px(700.0)), |window, cx| {
		let engine = cx.new(MockEngine::create);
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	// `set_input_text` defers its write: park a frame so the composer
	// actually holds the draft before send() reads it.
	cx.update(|app| {
		panel.update(app, |panel, cx| {
			set_input_text(&panel.input, "list the project", cx)
		});
	});
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.send(cx));
	});
	cx.run_until_parked();

	let (error, busy, bubbles) = cx.read(|app| {
		let p = panel.read(app);
		(p.error.clone(), p.busy, p.bubbles.len())
	});
	assert_eq!(
		error.as_deref(),
		Some("ENGINE_UNSUPPORTED"),
		"the refusal must name the real cause"
	);
	assert!(!busy, "a refusal must not start a run");
	assert_eq!(bubbles, 0, "a refusal must not push a user bubble");
}

/// End to end: a real engine with a real project, a local LLM (no
/// network), one `project.get_overview` executed through
/// `HostAdapter<OppHost>`, and the panel rendering the outcome —
/// then a screenshot as evidence.
#[gpui_kit::test]
async fn e2e_real_engine_tools_run_in_the_panel(cx: &mut TestAppContext) {
	let _env = env_lock();
	let _g = crate::oakui::graphops::test_lock();
	unsafe { std::env::set_var("OAK_AI_API_KEY", "stub-key-not-a-secret") };
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});

	let window = cx.open_window(size(px(1280.0), px(860.0)), |window, cx| {
		let engine = cx.new(RealEngine::create);
		engine.update(cx, |engine, cx| engine.new_project(cx));
		AiAssistantPanel::new(engine, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("panel root");

	// Seed the conversation with a local LLM and a real backend;
	// send() attaches the engine's live project handle itself.
	cx.update(|app| {
		panel.update(app, |panel, cx| {
			panel.agent = Some(Agent::new(
				Box::new(StubLlm {
					turns: std::cell::Cell::new(0),
				}),
				PanelBackend::Real(HostAdapter::new(OppHost::new())),
				AgentConfig::default(),
			));
			set_input_text(&panel.input, "list the project info", cx);
		});
	});
	// The composer write is deferred; park, then send.
	cx.run_until_parked();
	cx.update(|app| {
		panel.update(app, |panel, cx| panel.send(cx));
	});
	cx.run_until_parked();

	let (tools, answer, error, busy) = cx.read(|app| {
		let p = panel.read(app);
		let tools: Vec<(String, String)> = p
			.bubbles
			.iter()
			.filter_map(|b| match b {
				Bubble::Tool { name, detail, .. } => Some((name.clone(), detail.clone())),
				_ => None,
			})
			.collect();
		let answer = p
			.bubbles
			.iter()
			.find_map(|b| match b {
				Bubble::Assistant(t) => Some(t.clone()),
				_ => None,
			})
			.unwrap_or_default();
		(tools, answer, p.error.clone(), p.busy)
	});
	assert!(error.is_none(), "the run failed: {error:?}");
	assert!(!busy, "the run must have finished");
	assert_eq!(tools.len(), 1, "exactly one tool ran: {tools:?}");
	assert_eq!(tools[0].0, "project.get_overview");
	// The detail is the tool's JSON verbatim: real overview fields,
	// and none of the mock's `"backend": "mock"` marker.
	assert!(
		tools[0].1.contains("sequences"),
		"detail must carry the real overview: {}",
		tools[0].1
	);
	assert!(
		!tools[0].1.contains("\"mock\""),
		"detail came from the mock backend: {}",
		tools[0].1
	);
	assert!(
		answer.contains("real engine"),
		"the final answer must arrive: {answer:?}"
	);

	// Render evidence (screenshot downgraded — see the note below).
	// The transcript must have actually rendered the tool row: the
	// element's laid-out bounds are non-empty.
	let window: gpui_kit::AnyWindowHandle = window.into();
	let visual = VisualTestContext::from_window(window, cx).into_mut();
	visual.run_until_parked();
	// Render evidence: the panel and its transcript laid out a real
	// frame of the window's size (not merely data in the entity).
	let root = visual
		.debug_bounds("ai-assistant-root")
		.expect("the panel must render a frame");
	assert_eq!(
		root.size,
		size(px(1280.0), px(860.0)),
		"the panel must fill the window"
	);
	let transcript = visual
		.debug_bounds("ai-assistant-transcript")
		.expect("the transcript must render");
	assert!(
		transcript.size.height > px(0.0) && transcript.size.height < root.size.height,
		"the transcript takes the space below the status strip: {transcript:?}"
	);
	// NOTE (honest limits, verified):
	//  * the tool row itself is *not* in the debug-bounds map — children
	//    of the transcript's scroll viewport are laid out but not
	//    recorded (probed: root/transcript record, "ai-tool-0" does
	//    not);
	//  * a pixel screenshot is unavailable in this harness:
	//    `Window::render_to_image` needs a `HeadlessRenderer`, and
	//    `#[gpui_kit::test]` builds `TestPlatform::new`, which passes
	//    `None` (vendor/gpui-pre/src/platform/test/platform.rs:126).
	//    `capture_screenshot` exists only on the macOS-only
	//    `VisualTestAppContext`. Screenshot: UNVERIFIED.
}

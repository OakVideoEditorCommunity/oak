use super::*;
use crate::ai::config::{AiConfig, ConfigError};
use crate::ai::llm::{ChatResponse, LlmError};
use oak_ai_tools::hooks::{Decision, FailureEvent, Feedback, ReadbackVerdict, RenderDoneEvent};
use oak_ai_tools::mock::MockBackend;
use std::cell::RefCell;
use std::collections::VecDeque;

/// A model whose next answers are scripted; the loop's conversation is
/// asserted through `agent.messages()`.
struct ScriptedLlm {
	script: RefCell<VecDeque<ChatResponse>>,
}

impl ScriptedLlm {
	fn new(script: Vec<ChatResponse>) -> Self {
		Self {
			script: RefCell::new(script.into()),
		}
	}

	fn tool_call(name: &str, args: Value) -> ChatResponse {
		ChatResponse {
			content: None,
			tool_calls: vec![crate::ai::llm::ToolCall {
				id: format!("call_{}", name.replace('.', "_")),
				name: name.to_string(),
				arguments: args,
			}],
			finish_reason: Some("tool_calls".into()),
		}
	}

	fn say(text: &str) -> ChatResponse {
		ChatResponse {
			content: Some(text.into()),
			tool_calls: Vec::new(),
			finish_reason: Some("stop".into()),
		}
	}
}

/// The field failure's worst symptom (T9.3): six rounds spent on one
/// call that could never succeed, ending in "reached the round limit".
/// The loop must stop at the third identical failure and say what it was
/// stuck on.
#[test]
fn repeated_identical_failures_stop_the_loop_early() {
	use oak_ai_tools::mock::MockBackend;

	// A schema failure is repeatable and never touches the backend.
	let bad = ScriptedLlm::tool_call("sequence.set_format", json!({ "frameRate": 60 }));
	let llm = ScriptedLlm::new(vec![bad.clone(), bad.clone(), bad.clone(), bad]);
	let mut agent = Agent::new(
		llm,
		MockBackend::new(),
		AgentConfig {
			max_rounds: 20,
			..Default::default()
		},
	);
	let report = agent
		.run("set the frame rate")
		.expect("the loop must report, not error");
	assert!(
		report.stalled.is_some(),
		"the loop must notice the repeated failure: {report:?}"
	);
	let stall = report.stalled.expect("stall");
	assert_eq!(stall.tool, "sequence.set_format");
	assert_eq!(stall.code, oak_ai_tools::error::CODE_SCHEMA_VALIDATION);
	assert_eq!(stall.repeats, MAX_IDENTICAL_FAILURES);
	assert!(
		stall.describe().contains("sequence.set_format")
			&& stall.describe().contains("stopped early"),
		"the stop must be explained to the user: {}",
		stall.describe()
	);
	assert!(
		report.tools.len() < 20,
		"the cap must not be burned: {} calls",
		report.tools.len()
	);
	assert!(!report.converged, "it did not converge; it stalled");
	// The report must not inflate the work: three calls are three rounds.
	assert_eq!(
		report.rounds, MAX_IDENTICAL_FAILURES,
		"the reported rounds must equal the turns consumed"
	);
}

/// A different error code resets the streak, so a model that changes its
/// approach is never cut off.
#[test]
fn a_different_failure_resets_the_streak() {
	use oak_ai_tools::mock::MockBackend;

	let bad = ScriptedLlm::tool_call("sequence.set_format", json!({ "frameRate": 60 }));
	// Three failures of two different codes never reach 3 in a row of one.
	let other = ScriptedLlm::tool_call("sequence.get_settings", json!({ "sequence": {} }));
	let llm = ScriptedLlm::new(vec![
		bad.clone(),
		bad.clone(),
		other.clone(),
		bad.clone(),
		bad.clone(),
		ScriptedLlm::say("done"),
	]);
	let mut agent = Agent::new(
		llm,
		MockBackend::new(),
		AgentConfig {
			max_rounds: 20,
			..Default::default()
		},
	);
	let report = agent.run("set the frame rate").expect("run");
	assert!(
		report.stalled.is_none(),
		"interleaved failures must not stall: {report:?}"
	);
}

/// The model's field output, verbatim (T9.4): a text-form call with a
/// missing `</parameter>` before `</tool_call>`.
const TEXT_CALL_FIELD: &str = "<tool_call>\n<function=sequence.set_format>\n<parameter=sequence>3\n<parameter=fps>60</tool_call>";

/// A text-form call is executed through the same path as a structured
/// one — not shown to the user as an answer.
#[test]
fn a_text_form_call_is_executed_not_displayed() {
	use oak_ai_tools::mock::MockBackend;

	let llm = ScriptedLlm::new(vec![
		ChatResponse {
			content: Some(TEXT_CALL_FIELD.to_string()),
			tool_calls: Vec::new(),
			finish_reason: Some("stop".into()),
		},
		ScriptedLlm::say("done"),
	]);
	let mut backend = MockBackend::new();
	use oak_ai_tools::backend::EngineBackend;
	EngineBackend::project_open(&mut backend, &json!({ "name": "Text Call" })).expect("open");
	let mut agent = Agent::new(llm, backend, AgentConfig::default());
	let report = agent.run("把帧率改成60").expect("run");

	assert_eq!(report.text_fallback_calls, 1, "the fallback counted it");
	assert_eq!(report.tools.len(), 1, "one tool ran: {:?}", report.tools);
	assert_eq!(report.tools[0].tool, "sequence.set_format");
	// The field sample names sequence `3`, which is the *real* engine's
	// id; the mock's is `sequence-1`. So the call must reach the backend
	// and fail on that id — which is exactly the proof that the parsed
	// arguments (not the markup) went down the normal path.
	assert_eq!(report.tools[0].outcome, "failed", "{:?}", report.tools[0]);
	assert!(
		report.tools[0].detail.contains("no sequence '3'"),
		"the parsed id reached the engine: {}",
		report.tools[0].detail
	);
	assert!(
		report
			.final_text
			.as_deref()
			.unwrap_or_default()
			.contains("done"),
		"the loop continued to the final answer: {:?}",
		report.final_text
	);
}

/// The same text form with the mock's own id really applies the edit.
#[test]
fn a_text_form_call_applies_against_the_mock_state() {
	use oak_ai_tools::mock::MockBackend;

	let content = "<tool_call><function=sequence.set_format>\n<parameter=sequence>sequence-1\n<parameter=fps>60</tool_call>";
	let llm = ScriptedLlm::new(vec![
		ChatResponse {
			content: Some(content.to_string()),
			tool_calls: Vec::new(),
			finish_reason: Some("stop".into()),
		},
		ScriptedLlm::say("done"),
	]);
	let mut backend = MockBackend::new();
	use oak_ai_tools::backend::EngineBackend;
	EngineBackend::project_open(&mut backend, &json!({ "name": "Text Apply" })).expect("open");
	let mut agent = Agent::new(llm, backend, AgentConfig::default());
	let report = agent.run("set the rate to 60").expect("run");

	assert_eq!(report.text_fallback_calls, 1);
	assert_eq!(report.tools[0].outcome, "applied", "{:?}", report.tools[0]);
	assert!(
		report.tools[0].detail.contains("\"num\":60"),
		"the rate really changed: {}",
		report.tools[0].detail
	);
}

/// **History fidelity**: the next request must carry the prose, never the
/// raw markup — feeding it back would teach the model this format works.
#[test]
fn the_history_never_carries_the_raw_markup() {
	use oak_ai_tools::mock::MockBackend;

	let content = format!("Setting it now.\n{TEXT_CALL_FIELD}");
	let llm = ScriptedLlm::new(vec![
		ChatResponse {
			content: Some(content),
			tool_calls: Vec::new(),
			finish_reason: Some("stop".into()),
		},
		ScriptedLlm::say("done"),
	]);
	let mut backend = MockBackend::new();
	use oak_ai_tools::backend::EngineBackend;
	EngineBackend::project_open(&mut backend, &json!({ "name": "History" })).expect("open");
	let mut agent = Agent::new(llm, backend, AgentConfig::default());
	let report = agent.run("set the rate").expect("run");
	assert_eq!(report.text_fallback_calls, 1);

	let history = agent.messages();
	let assistant_turns: Vec<&str> = history
		.iter()
		.filter_map(|m| (m.role == "assistant").then(|| m.content.as_deref().unwrap_or_default()))
		.collect();
	assert!(
		!assistant_turns.is_empty(),
		"the assistant turn is in history"
	);
	for turn in &assistant_turns {
		assert!(
			!turn.contains("<tool_call") && !turn.contains("<parameter="),
			"raw markup leaked into history: {turn}"
		);
	}
	assert!(
		assistant_turns.iter().any(|t| t.contains("Setting it now")),
		"the prose is kept: {assistant_turns:?}"
	);
	// tool_call ids and their results stay paired.
	let call_ids: Vec<&str> = history
		.iter()
		.filter_map(|m| m.tool_call_id.as_deref())
		.collect();
	assert_eq!(call_ids.len(), 1, "one tool result: {history:?}");
	assert!(
		call_ids[0].starts_with("text_"),
		"synthetic id: {:?}",
		call_ids[0]
	);
}

/// **Never double-execute**: a structured call must not also be read from
/// the text (the same call would run twice).
#[test]
fn a_structured_call_is_not_also_executed_from_text() {
	use oak_ai_tools::mock::MockBackend;

	// Both channels carry the same request.
	let response = ChatResponse {
		content: Some(TEXT_CALL_FIELD.to_string()),
		tool_calls: vec![ToolCall {
			id: "structured-1".to_string(),
			name: "sequence.set_format".to_string(),
			arguments: json!({ "sequence": 3, "fps": 60 }),
		}],
		finish_reason: Some("tool_calls".into()),
	};
	let llm = ScriptedLlm::new(vec![response, ScriptedLlm::say("done")]);
	let mut backend = MockBackend::new();
	use oak_ai_tools::backend::EngineBackend;
	EngineBackend::project_open(&mut backend, &json!({ "name": "No Double" })).expect("open");
	let mut agent = Agent::new(llm, backend, AgentConfig::default());
	let report = agent.run("set the rate").expect("run");

	assert_eq!(report.text_fallback_calls, 0, "the fallback must not fire");
	assert_eq!(
		report.tools.len(),
		1,
		"exactly one execution: {:?}",
		report.tools
	);
}

/// Unparseable markup stops the turn with a report, never with the XML
/// shown as the answer.
#[test]
fn unparseable_markup_is_reported_not_displayed() {
	use oak_ai_tools::mock::MockBackend;

	let llm = ScriptedLlm::new(vec![ChatResponse {
		content: Some("<tool_call><parameter=fps>60</tool_call>".to_string()),
		tool_calls: Vec::new(),
		finish_reason: Some("stop".into()),
	}]);
	let mut agent = Agent::new(llm, MockBackend::new(), AgentConfig::default());
	let report = agent.run("set the rate").expect("run");

	assert_eq!(report.unparsed_text_calls, 1);
	assert!(report.unparsed_text.is_some(), "the raw markup is kept");
	assert!(!report.converged, "the turn did not complete normally");
	assert!(
		report.final_text.is_none(),
		"the XML must never become the answer: {:?}",
		report.final_text
	);
	assert!(report.tools.is_empty(), "nothing was executed");
}

/// Ordinary prose with a `<` is still a normal answer.
#[test]
fn prose_with_a_less_than_sign_is_still_an_answer() {
	use oak_ai_tools::mock::MockBackend;

	let llm = ScriptedLlm::new(vec![ScriptedLlm::say("如果 width < 100 就裁掉边缘。")]);
	let mut agent = Agent::new(llm, MockBackend::new(), AgentConfig::default());
	let report = agent.run("advice").expect("run");
	assert!(report.converged);
	assert_eq!(report.unparsed_text_calls, 0, "no false positive");
	assert_eq!(report.text_fallback_calls, 0);
	assert!(
		report
			.final_text
			.unwrap_or_default()
			.contains("width < 100")
	);
}

impl LlmClient for ScriptedLlm {
	fn complete(&self, _request: &ChatRequest<'_>) -> Result<ChatResponse, LlmError> {
		self.script
			.borrow_mut()
			.pop_front()
			.ok_or_else(|| LlmError::Parse("script exhausted".into()))
	}
}

/// Hooks that refuse every mutating commit (the fail-closed path a
/// confirm-gate user would hit).
struct DenyHooks;

impl BeforeCommit for DenyHooks {
	fn before_commit(&mut self, _pending: &oak_ai_tools::hooks::PendingEdit<'_>) -> Decision {
		Decision::Reject {
			reason: "user declined this edit".into(),
		}
	}
}
impl RenderDone for DenyHooks {
	fn render_done(&mut self, _event: &RenderDoneEvent<'_>) -> ReadbackVerdict {
		ReadbackVerdict::Pass
	}
}
impl EditFailed for DenyHooks {
	fn edit_failed(&mut self, event: &FailureEvent<'_>) -> Feedback {
		Feedback {
			error: event.error.clone(),
			retryable: false,
			hint: "gate denied".into(),
		}
	}
}
// No `impl Hooks for DenyHooks`: the crate provides
// `impl<T: BeforeCommit + RenderDone + EditFailed> Hooks for T`, so the
// three impls above already make it a `Hooks`.
use oak_ai_tools::hooks::{BeforeCommit, EditFailed, RenderDone};

fn timeline_track_count(backend: &MockBackend) -> usize {
	backend
		.serialize_timeline()
		.get("tracks")
		.and_then(Value::as_array)
		.map(Vec::len)
		.unwrap_or(0)
}

fn agent(script: Vec<ChatResponse>, max_rounds: usize) -> Agent<ScriptedLlm, MockBackend> {
	Agent::new(
		ScriptedLlm::new(script),
		MockBackend::new(),
		AgentConfig {
			max_rounds,
			policy: Policy::Auto,
			model: "test-model".into(),
		},
	)
}

/// ① One tool call in round one, convergence in round two.
#[test]
fn single_tool_round_completes_and_converges() {
	let mut agent = agent(
		vec![
			ScriptedLlm::tool_call("node.list_types", json!({})),
			ScriptedLlm::say("there are 30+ node types"),
		],
		8,
	);
	let report = agent.run("what nodes exist?").expect("run");
	assert!(report.converged, "must converge: {report:?}");
	assert_eq!(report.rounds, 2, "one tool turn + one answer turn");
	assert_eq!(report.tools.len(), 1);
	assert_eq!(report.tools[0].tool, "node.list_types");
	assert_eq!(
		report.tools[0].outcome, "applied",
		"{}",
		report.tools[0].detail
	);
	assert!(report.final_text.is_some());
}

/// ② Two dependent rounds: open a project, then read its overview —
/// the second call only succeeds because the first one ran.
#[test]
fn multi_round_dependency_open_then_overview() {
	let mut agent = agent(
		vec![
			ScriptedLlm::tool_call("project.open", json!({ "name": "demo" })),
			ScriptedLlm::tool_call("project.get_overview", json!({})),
			ScriptedLlm::say("opened demo"),
		],
		8,
	);
	let report = agent.run("open demo and summarize it").expect("run");
	assert!(report.converged);
	assert_eq!(report.tools.len(), 2, "{report:?}");
	assert!(
		report.tools.iter().all(|t| t.outcome == "applied"),
		"{report:?}"
	);
	// The overview result (a `tool` message) must describe the project
	// the first call created.
	let tool_msgs: Vec<&ChatMessage> = agent
		.messages()
		.iter()
		.filter(|m| m.role == "tool")
		.collect();
	assert_eq!(tool_msgs.len(), 2);
	let overview = tool_msgs[1].content.as_deref().unwrap_or_default();
	assert!(
		overview.contains("demo"),
		"overview lost the dependency: {overview}"
	);
}

/// ③ A plain answer with no tool calls ends the loop immediately.
#[test]
fn no_tool_call_converges_in_one_round() {
	let mut agent = agent(vec![ScriptedLlm::say("just an answer")], 8);
	let report = agent.run("hello").expect("run");
	assert!(report.converged);
	assert_eq!(report.rounds, 1);
	assert!(report.tools.is_empty());
	assert_eq!(report.final_text.as_deref(), Some("just an answer"));
}

/// ④ A model that never stops asking is cut off at the cap — and the
/// report says so instead of pretending it converged.
#[test]
fn round_cap_stops_a_looping_model() {
	let mut agent = agent(
		vec![
			ScriptedLlm::tool_call("node.list_types", json!({})),
			ScriptedLlm::tool_call("node.list_types", json!({})),
			ScriptedLlm::tool_call("node.list_types", json!({})),
			ScriptedLlm::tool_call("node.list_types", json!({})),
		],
		3,
	);
	let report = agent.run("loop forever").expect("run");
	assert!(!report.converged, "cap must not look like convergence");
	assert_eq!(report.rounds, 3);
	assert_eq!(report.tools.len(), 3);
	assert!(report.final_text.is_none());
}

/// ⑤ Missing key → structured `AI_NOT_CONFIGURED`, no panic, and the
/// error travels through `AgentError` intact.
#[test]
fn missing_key_is_a_structured_agent_error() {
	let err = AiConfig::load(|_| None).expect_err("no key");
	assert_eq!(err, ConfigError::MissingApiKey);
	let agent_err = AgentError::from(err);
	assert_eq!(agent_err.code(), "AI_NOT_CONFIGURED");
	assert!(agent_err.to_string().contains("OAK_AI_API_KEY"));
}

/// ⑥ The confirm gate refuses a mutating call: the loop records the
/// rejection, hands the model the reason, and the engine is untouched.
///
/// Two runs: the first opens a project under the default `Auto` policy
/// (a `Confirm` gate would also block that call, and the mock backend
/// rejects `timeline.add_track` with `STATE_ERROR` when no project is
/// open — that failure never reaches the gate). The second run applies
/// `Confirm` + the denying gate to the track call itself.
#[test]
fn hooks_gate_rejection_reaches_the_model_and_leaves_engine_clean() {
	let mut agent = agent(
		vec![
			ScriptedLlm::tool_call("project.open", json!({ "name": "gate-test" })),
			ScriptedLlm::say("project opened"),
			ScriptedLlm::tool_call("timeline.add_track", json!({ "type": "video" })),
			ScriptedLlm::say("understood, not applied"),
		],
		8,
	);
	// Run 1: Auto + NoHooks → the project really opens.
	let first = agent.run("open the project").expect("first run");
	assert_eq!(
		first.tools[0].outcome, "applied",
		"{}",
		first.tools[0].detail
	);

	// The first run legitimately left its own undo group / sequence;
	// the rejection must not be judged against a zero baseline.
	let depth_after_open = agent.backend().undo_depth();
	let tracks_after_open = timeline_track_count(agent.backend());

	// Run 2: Confirm + DenyHooks → the track call hits the gate.
	let mut gate = DenyHooks;
	agent.config.policy = Policy::Confirm;
	let report = agent.run_with("add a video track", &mut gate).expect("run");

	assert!(report.converged);
	assert_eq!(report.tools.len(), 1, "{report:?}");
	assert_eq!(
		report.tools[0].outcome, "rejected",
		"{}",
		report.tools[0].detail
	);
	let detail = &report.tools[0].detail;
	assert!(
		detail.contains("USER_REJECTED"),
		"missing rejection code: {detail}"
	);
	assert!(
		detail.contains("user declined this edit"),
		"reason lost: {detail}"
	);

	// Nothing was applied: the rejection added no undo group and no
	// track on top of what run 1 established.
	let backend = agent.backend();
	assert_eq!(
		backend.undo_depth(),
		depth_after_open,
		"a rejected edit added an undo group"
	);
	assert_eq!(
		timeline_track_count(backend),
		tracks_after_open,
		"rejected track landed"
	);

	// The model saw the refusal as its tool result — the LAST tool
	// message (earlier runs appended their own).
	let tool_msg = agent
		.messages()
		.iter()
		.filter(|m| m.role == "tool")
		.last()
		.expect("tool message");
	let content = tool_msg.content.as_deref().unwrap_or_default();
	assert!(
		content.contains("\"ok\":false") || content.contains("\"ok\": false"),
		"{content}"
	);
}

/// T11 probe (censor-verify-2): what does `Policy::ProposalOnly` do to
/// a `memory.*` write? `execute_call` (agent.rs `if
/// super::memory::is_memory_tool(&call.name)`) diverts memory tools to
/// `memory::execute(&self.memory, …)` **before** hooks sees them — and
/// `self.memory` is not the Backend that `hooks` guards
/// (`oak_ai_tools::hooks` doc: "ProposalOnly never touches the
/// backend"). So the record lands directly in `~/.oak/memory` with no
/// confirmation gate.
///
/// This test ASSERTS THE CURRENT (bypassing) behaviour so the finding
/// is reproducible; **it is not an endorsement**. Whether a
/// `memory.*` write should be gated under `ProposalOnly` (it is a
/// persistent side effect outside the project document) is a design
/// question reserved for the Lead — see task-45. If the gate is later
/// enforced, this probe must be updated in the same change.
#[test]
fn zz_probe_memory_write_ignores_proposal_only() {
	use oak_ai_tools::hooks::Policy;
	use oak_ai_tools::mock::MockBackend;

	let root = std::env::temp_dir().join(format!("oak-mem-probe-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&root);
	let bin = super::super::memory::MemoryTools::from_env()
		.config()
		.bin()
		.to_path_buf();
	if !bin.is_file() {
		eprintln!("skipping: memlite binary not installed");
		return;
	}
	let memory = super::super::memory::MemoryTools::new(super::super::memory::MemoryConfig::new(
		bin,
		&root,
		Some("p".into()),
	));
	let llm = ScriptedLlm::new(vec![
		ScriptedLlm::tool_call(
			"memory.remember",
			json!({"content": "把这段的帧率改成 24", "title": "剪辑事实"}),
		),
		ScriptedLlm::say("done"),
	]);
	let mut agent = Agent::new(
		llm,
		MockBackend::new(),
		AgentConfig {
			max_rounds: 4,
			policy: Policy::ProposalOnly,
			..Default::default()
		},
	)
	.with_memory(memory);
	let report = agent.run("remember this").expect("run");
	println!("TOOLS={:?}", report.tools);
	let listed = agent.memory().list(&json!({"limit": 10})).expect("list");
	println!("LISTED={listed}");
	assert_eq!(
		listed["entries"].as_array().unwrap().len(),
		1,
		"the probe expects the write to have LANDED under ProposalOnly: {listed}"
	);
	assert_eq!(report.tools[0].outcome, "applied");
	let _ = std::fs::remove_dir_all(&root);
}

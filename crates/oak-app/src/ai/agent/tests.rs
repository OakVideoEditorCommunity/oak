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
	// `serialize_timeline` is `{footage, sequences}` — tracks live under
	// each sequence. (The old lookup hit a non-existent top-level
	// `tracks` and could only ever return 0; every existing use compared
	// 0 to 0, so only an absolute assertion exposes it.)
	backend
		.serialize_timeline()
		.get("sequences")
		.and_then(Value::as_array)
		.map(|sequences| {
			sequences
				.iter()
				.filter_map(|sequence| sequence.get("tracks"))
				.filter_map(Value::as_array)
				.map(Vec::len)
				.sum::<usize>()
		})
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
		confirm: ConfirmMode::Headless,
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

// ---------------------------------------------------------------------------
// Task J: the capability layer (confirm gate / readback / progress / plan /
// retry+rollback+cost+recipes) at the loop level.

use crate::ai::plan::RecipeBook;
use crate::ai::progress::FnSink;
use std::sync::{Arc, Mutex};

/// A backend with one open project (0 tracks), the usual plan sandbox.
fn seeded_for_plan() -> MockBackend {
	let mut backend = MockBackend::new();
	oak_ai_tools::dispatch::execute(&mut backend, "project.open", json!({"name": "plan", "fps": 30}))
		.expect("open");
	backend
}

fn plan_agent(backend: MockBackend) -> Agent<ScriptedLlm, MockBackend> {
	Agent::new(
		ScriptedLlm::new(vec![ScriptedLlm::say("unused")]),
		backend,
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Auto,
			model: "test-model".into(),
		confirm: ConfirmMode::Headless,
		},
	)
}

/// ④ A plan executes stepwise: plan + trajectory + progress + cost land
/// in the report, and every write is read back.
#[test]
fn run_plan_records_plan_trajectory_progress_and_cost() {
	let mut agent = plan_agent(seeded_for_plan());
	// `project.open` seeds default tracks: judge the plan against the
	// baseline it starts from, not against zero.
	let baseline_tracks = timeline_track_count(agent.backend());
	let plan = Plan::new("add two tracks")
		.push(PlanStep::new("video track", "timeline.add_track", json!({"type": "video"})))
		.push(PlanStep::new("audio track", "timeline.add_track", json!({"type": "audio"})));
	let report = agent.run_plan(plan).expect("plan runs");

	assert_eq!(
		report.plan.as_ref().expect("the plan is recorded").goal,
		"add two tracks"
	);
	assert_eq!(report.trajectory.len(), 2, "{:?}", report.trajectory);
	assert!(
		report
			.trajectory
			.iter()
			.all(|trace| trace.status == StepStatus::Done && trace.attempts == 1),
		"{:?}",
		report.trajectory
	);
	// Progress: one event per step, ending at 100%.
	assert_eq!(report.progress.len(), 2, "{:?}", report.progress);
	assert_eq!(report.progress[0].step, 1);
	assert_eq!(report.progress[1].percent(), 100);
	// Cost: one call per step; every write read back, none failed.
	assert_eq!(report.cost.tool_calls, 2);
	assert_eq!(report.readbacks, 2, "both writes were read back");
	assert_eq!(report.readback_failures, 0);
	assert!(report.rollback.is_none(), "nothing failed: {:?}", report.rollback);
	assert_eq!(
		timeline_track_count(agent.backend()),
		baseline_tracks + 2,
		"exactly the two planned writes landed"
	);
}

/// ⑤ The cost guard is a hard stop: the second step never runs and the
/// reason is reported (not merely counted).
#[test]
fn cost_guard_blocks_the_second_step_and_reports_the_reason() {
	let mut agent = plan_agent(seeded_for_plan()).with_cost_guard(CostGuard {
		max_tool_calls: Some(1),
		..CostGuard::default()
	});
	let baseline_tracks = timeline_track_count(agent.backend());
	let plan = Plan::new("two tracks")
		.push(PlanStep::new("video track", "timeline.add_track", json!({"type": "video"})))
		.push(PlanStep::new("audio track", "timeline.add_track", json!({"type": "audio"})));
	let report = agent.run_plan(plan).expect("plan runs");

	assert_eq!(report.trajectory[0].status, StepStatus::Done, "{:?}", report.trajectory);
	assert_eq!(report.trajectory[1].status, StepStatus::Blocked, "{:?}", report.trajectory);
	let stop = report.cost_stop.as_ref().expect("the stop must be reported");
	assert_eq!(stop.limit, "tool_calls");
	assert!(stop.detail.contains("1 of 1"), "{}", stop.detail);
	// Only the funded call touched the engine.
	assert_eq!(
		timeline_track_count(agent.backend()),
		baseline_tracks + 1,
		"the blocked step must not have run"
	);
}

/// ⑤ The same guard stops the *loop* mid-round: the second call of a
/// two-call reply is refused with `COST_LIMIT`, and the run says why.
#[test]
fn cost_guard_stops_the_loop_after_the_budget_is_spent() {
	let two_calls = ChatResponse {
		content: None,
		tool_calls: vec![
			ToolCall {
				id: "call_a".into(),
				name: "timeline.add_track".into(),
				arguments: json!({"type": "video"}),
			},
			ToolCall {
				id: "call_b".into(),
				name: "timeline.add_track".into(),
				arguments: json!({"type": "audio"}),
			},
		],
		finish_reason: Some("tool_calls".into()),
	};
	let seeded = seeded_for_plan();
	let baseline_tracks = timeline_track_count(&seeded);
	let mut agent = Agent::new(
		ScriptedLlm::new(vec![two_calls, ScriptedLlm::say("never reached")]),
		seeded,
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Auto,
			model: "test-model".into(),
		confirm: ConfirmMode::Headless,
		},
	)
	.with_cost_guard(CostGuard {
		max_tool_calls: Some(1),
		..CostGuard::default()
	});

	let report = agent.run("add both tracks").expect("the loop reports, not errors");
	assert_eq!(report.tools.len(), 2, "{:?}", report.tools);
	assert_eq!(report.tools[0].outcome, "applied", "{}", report.tools[0].detail);
	assert_eq!(report.tools[1].outcome, "cost-stopped", "{}", report.tools[1].detail);
	assert!(report.tools[1].detail.contains("COST_LIMIT"), "{}", report.tools[1].detail);
	let stop = report.cost_stop.as_ref().expect("a hard stop must be reported");
	assert_eq!(stop.limit, "tool_calls");
	assert!(!report.converged, "the loop must stop, not limp on");
	assert_eq!(
		timeline_track_count(agent.backend()),
		baseline_tracks + 1,
		"only the funded call ran"
	);
}

/// ④⑤ A failing step rolls the applied ones back to the baseline —
/// verified against the state, not against the undo bookkeeping.
#[test]
fn a_failed_plan_step_rolls_back_the_applied_steps() {
	let mut agent = plan_agent(seeded_for_plan());
	let baseline_tracks = timeline_track_count(agent.backend());
	let plan = Plan::new("track then an impossible placement")
		.push(PlanStep::new("add track", "timeline.add_track", json!({"type": "video"})))
		.push(PlanStep::new(
			"place",
			"timeline.place_clip",
			json!({"track": "no-such-track", "footage": "no-such-footage", "in": 0}),
		));
	let report = agent.run_plan(plan).expect("the plan reports, not errors");

	assert_eq!(report.trajectory[0].status, StepStatus::Done, "{:?}", report.trajectory);
	assert_eq!(report.trajectory[1].status, StepStatus::Failed, "{:?}", report.trajectory);
	let rollback = report.rollback.as_ref().expect("a failed plan must roll back");
	assert!(rollback.restored, "{}", rollback.detail);
	assert!(rollback.is_clean(), "{rollback:?}");
	assert!(
		rollback.triggered_by.contains("place"),
		"the trigger names the failing step: {}",
		rollback.triggered_by
	);
	assert_eq!(
		timeline_track_count(agent.backend()),
		baseline_tracks,
		"the applied step was undone — observable state matches the baseline"
	);
}

/// ⑤ Retry spends exactly `max_attempts` on a retryable failure, then
/// fails honestly (with the rollback record).
#[test]
fn retry_policy_spends_every_attempt_on_a_retryable_failure() {
	let mut agent = plan_agent(seeded_for_plan()).with_retry(RetryPolicy {
		max_attempts: 3,
		backoff_ms: 0,
	});
	let plan = Plan::new("impossible placement").push(PlanStep::new(
		"place",
		"timeline.place_clip",
		json!({"track": "nope", "footage": "nope", "in": 0}),
	));
	let report = agent.run_plan(plan).expect("the plan reports, not errors");

	assert_eq!(report.trajectory.len(), 1);
	assert_eq!(
		report.trajectory[0].attempts, 3,
		"all three attempts must be spent: {:?}",
		report.trajectory[0]
	);
	assert_eq!(report.trajectory[0].status, StepStatus::Failed);
	assert_eq!(report.cost.tool_calls, 3, "each attempt is a paid call");
	assert!(report.rollback.is_some(), "the failure is recorded with a rollback");
}

/// ⑥ The built-in `rough-cut` recipe runs end to end: template vars from
/// earlier results (`${track}`, `${footage.0.id}`) resolve, the clip is
/// placed, and the readback confirms it.
#[test]
fn rough_cut_recipe_executes_end_to_end() {
	// `env::temp_dir()` is %TEMP% on Windows and /tmp elsewhere — the
	// fixture must exist wherever CI runs.
	let media = std::env::temp_dir().join("oak58-rough.mp4");
	std::fs::write(&media, b"x").expect("fixture");
	let book = RecipeBook::builtin();
	let plan = book
		.plan("rough-cut", "cut one clip")
		.expect("the built-in recipe exists")
		.with_vars(json!({"media": media.to_string_lossy().to_string()}));
	let mut agent = plan_agent(MockBackend::new());
	let report = agent.run_plan(plan).expect("the plan runs");

	let statuses: Vec<StepStatus> = report
		.trajectory
		.iter()
		.map(|trace| trace.status)
		.collect();
	assert_eq!(
		statuses,
		vec![StepStatus::Done; 4],
		"open → track → import → place: {:?}",
		report.trajectory
	);
	let overview = readback::probe(agent.backend_mut()).expect("probe");
	assert!(
		overview.to_string().contains("clip-"),
		"a clip must be readable back: {overview}"
	);
	assert_eq!(report.readback_failures, 0);
}

/// ④ `plan_from_llm` turns a (scripted) model reply into an explicit
/// plan before anything executes — one LLM round, then the steps run.
#[test]
fn plan_from_llm_parses_the_reply_then_runs() {
	let reply = ScriptedLlm::say(
		r#"```json
{"goal": "open and add a track", "steps": [
  {"title": "open", "tool": "project.open", "args": {"name": "llm-plan", "fps": 30}},
  {"title": "track", "tool": "timeline.add_track", "args": {"type": "video"}}
]}
```"#,
	);
	let mut agent = agent(vec![reply], 4);
	let plan = agent.plan_from_llm("make a plan").expect("the plan parses");
	assert_eq!(plan.goal, "open and add a track");
	assert_eq!(plan.steps.len(), 2);
	assert_eq!(agent.cost_usage().llm_rounds, 1, "planning costs one round");

	let report = agent.run_plan(plan).expect("the plan runs");
	assert!(
		report
			.trajectory
			.iter()
			.all(|trace| trace.status == StepStatus::Done),
		"{:?}",
		report.trajectory
	);
}

/// ③ Progress reaches a subscriber as it happens (rounds here; plan
/// steps are asserted via `report.progress` in the plan test).
#[test]
fn progress_sink_sees_the_rounds() {
	let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
	let sink_seen = Arc::clone(&seen);
	let script = vec![
		ScriptedLlm::tool_call("timeline.add_track", json!({"type": "video"})),
		ScriptedLlm::say("done"),
	];
	let mut agent = Agent::new(
		ScriptedLlm::new(script),
		seeded_for_plan(),
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Auto,
			model: "test-model".into(),
		confirm: ConfirmMode::Headless,
		},
	)
	.with_progress_sink(FnSink(move |event: &ProgressEvent| {
		sink_seen
			.lock()
			.expect("poisoned")
			.push(event.to_string());
	}));

	let report = agent.run("add a track").expect("run");
	let events = seen.lock().expect("poisoned").clone();
	assert_eq!(
		events.len(),
		report.progress.len(),
		"the sink and the report must agree: {events:?} vs {:?}",
		report.progress
	);
	assert_eq!(events.len(), 2, "one event per round: {events:?}");
	assert!(events[0].contains("round"), "{events:?}");
	assert_eq!(report.progress[0].phase, "round");
	assert_eq!(report.progress[1].step, 2);
}

// ---------------------------------------------------------------------------
// Task-62: the confirm gate is wired into the production run path
// (`Agent::run` → `run_with` → `GatedHooks`).

/// ① Strict gate: the write never reaches the engine, the report marks
/// it `rejected` by the gate, and neither state nor undo stack moves.
#[test]
fn strict_confirm_gate_blocks_the_write_and_the_report_says_so() {
	let mut agent = Agent::new(
		ScriptedLlm::new(vec![
			ScriptedLlm::tool_call("timeline.add_track", json!({"type": "video"})),
			ScriptedLlm::say("done"),
		]),
		seeded_for_plan(),
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Auto,
			model: "test-model".into(),
			confirm: ConfirmMode::Strict,
		},
	);
	let tracks_before = timeline_track_count(agent.backend());
	let depth_before = agent.backend().undo_depth();

	let report = agent.run("add a track").expect("the run reports, not errors");

	assert_eq!(report.tools.len(), 1, "{:?}", report.tools);
	assert_eq!(report.tools[0].outcome, "rejected", "{}", report.tools[0].detail);
	assert!(
		report.tools[0].detail.contains("the confirmation gate refused"),
		"the refusal must come from the gate: {}",
		report.tools[0].detail
	);
	assert_eq!(
		timeline_track_count(agent.backend()),
		tracks_before,
		"the write must not have landed"
	);
	assert_eq!(
		agent.backend().undo_depth(),
		depth_before,
		"the gate must not have opened an undo group"
	);
}

/// ② Headless gate under `Policy::Confirm` (so the gate really is
/// asked): it auto-approves and the write runs for real.
#[test]
fn headless_confirm_gate_approves_and_the_write_runs() {
	let mut agent = Agent::new(
		ScriptedLlm::new(vec![
			ScriptedLlm::tool_call("timeline.add_track", json!({"type": "video"})),
			ScriptedLlm::say("done"),
		]),
		seeded_for_plan(),
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Confirm,
			model: "test-model".into(),
			confirm: ConfirmMode::Headless,
		},
	);
	let tracks_before = timeline_track_count(agent.backend());

	let report = agent.run("add a track").expect("the run reports, not errors");

	assert_eq!(report.tools[0].outcome, "applied", "{}", report.tools[0].detail);
	assert_eq!(
		timeline_track_count(agent.backend()),
		tracks_before + 1,
		"the approved write must have landed"
	);
}

/// ③ Reads stay exempt: the strict gate never sees them.
#[test]
fn strict_confirm_gate_still_lets_reads_through() {
	let mut agent = Agent::new(
		ScriptedLlm::new(vec![
			ScriptedLlm::tool_call("project.get_overview", json!({})),
			ScriptedLlm::say("done"),
		]),
		seeded_for_plan(),
		AgentConfig {
			max_rounds: 4,
			policy: Policy::Auto,
			model: "test-model".into(),
			confirm: ConfirmMode::Strict,
		},
	);

	let report = agent
		.run("what is on the timeline?")
		.expect("the run reports, not errors");

	assert_eq!(report.tools[0].outcome, "applied", "{}", report.tools[0].detail);
	assert!(
		report.tools[0].detail.contains("sequences"),
		"the read must return real state: {}",
		report.tools[0].detail
	);
}

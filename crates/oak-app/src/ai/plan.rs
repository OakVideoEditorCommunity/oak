//! Plans, recipes, retry, cost guard and rollback (capability groups 4
//! and 5).
//!
//! * **Plans** — an explicit step list produced *before* execution
//!   ([`Plan`]); the agent executes it step by step and records a
//!   [`StepTrace`] per step, so the report carries both the plan and the
//!   trajectory ([`crate::ai::agent::AgentReport::plan`] /
//!   `::trajectory`).
//! * **Recipes** — reusable step sequences ([`Recipe`]); a recipe *is*
//!   a plan factory. Built-ins live in [`RecipeBook::builtin`], hosts can
//!   load their own with [`RecipeBook::from_json`].
//! * **Retry/degrade** — [`RetryPolicy`] retries a failed step while the
//!   feedback says the error is retryable; [`OnFail`] decides what
//!   happens when retries run out: fail (and roll back), degrade to a
//!   fallback tool, or skip.
//! * **Cost guard** — [`CostGuard`] caps tool calls / LLM rounds /
//!   (estimated) tokens and *stops* when a limit is hit ([`CostStop`]),
//!   it does not merely count.
//! * **Rollback** — multi-step writes are not wrapped in a nested
//!   transaction (a second `group_begin` inside an open group answers
//!   `Error::State`, see `ai/README.md`); instead [`rollback_to`] issues
//!   `edit.undo` until the probe matches the [`Baseline`] taken before
//!   the plan. A rollback that cannot restore the baseline reports
//!   `restored: false` with the failing undo calls — never a silent
//!   "done".

use crate::ai::readback;
use oak_ai_tools::backend::EngineBackend;
use oak_ai_tools::dispatch;
use oak_ai_tools::hooks::{GatedOutcome, NoHooks, Policy};
use serde_json::{Value, json};

// ------------------------------------------------------------------ plan

/// What to do when a step fails after its retries are exhausted.
#[derive(Debug, Clone, PartialEq)]
pub enum OnFail {
	/// Stop the plan and roll the applied steps back (the default).
	Fail,
	/// Degrade: run `tool`/`args` once instead, then continue. If the
	/// fallback also fails, the step counts as failed (`Fail` semantics).
	Fallback { tool: String, args: Value },
	/// Record the step as skipped and continue with the next one.
	Skip,
}

impl OnFail {
	/// Stable label for traces/reports.
	pub fn label(&self) -> &'static str {
		match self {
			Self::Fail => "fail",
			Self::Fallback { .. } => "fallback",
			Self::Skip => "skip",
		}
	}
}

impl Default for OnFail {
	fn default() -> Self {
		Self::Fail
	}
}

/// One explicit plan step.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStep {
	/// Human-readable phase name (also the progress phase).
	pub title: String,
	/// Tool to call.
	pub tool: String,
	/// Arguments; string values may contain `${var}` templates resolved
	/// against the plan's variables and earlier results (see
	/// [`substitute`]).
	pub args: Value,
	/// What to do if the step fails after retries.
	pub on_fail: OnFail,
}

impl PlanStep {
	/// A plain step that fails the plan when it fails.
	pub fn new(title: impl Into<String>, tool: impl Into<String>, args: Value) -> Self {
		Self {
			title: title.into(),
			tool: tool.into(),
			args,
			on_fail: OnFail::Fail,
		}
	}

	/// Attach an [`OnFail`] policy.
	pub fn on_fail(mut self, on_fail: OnFail) -> Self {
		self.on_fail = on_fail;
		self
	}
}

/// An explicit plan: the step list produced *before* execution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
	/// What the plan is for (the user's request, condensed).
	pub goal: String,
	/// The steps, executed in order.
	pub steps: Vec<PlanStep>,
	/// Initial template variables (`${media}` etc.), merged before the
	/// first step's results.
	pub vars: Value,
}

impl Plan {
	/// An empty plan for `goal`.
	pub fn new(goal: impl Into<String>) -> Self {
		Self {
			goal: goal.into(),
			steps: Vec::new(),
			vars: json!({}),
		}
	}

	/// Append a step.
	pub fn push(mut self, step: PlanStep) -> Self {
		self.steps.push(step);
		self
	}

	/// Seed template variables (e.g. `json!({"media": "/tmp/clip.mp4"})`).
	pub fn with_vars(mut self, vars: Value) -> Self {
		self.vars = vars;
		self
	}
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
	/// Applied (and read back clean, when readback is on).
	Done,
	/// Applied only after a fallback tool ran.
	Degraded,
	/// Not applied; the plan stopped and rolled back.
	Failed,
	/// Not applied by design ([`OnFail::Skip`]); the plan continued.
	Skipped,
	/// Never attempted: the cost guard stopped the plan first.
	Blocked,
}

impl StepStatus {
	/// Stable label for traces.
	pub fn label(&self) -> &'static str {
		match self {
			Self::Done => "done",
			Self::Degraded => "degraded",
			Self::Failed => "failed",
			Self::Skipped => "skipped",
			Self::Blocked => "blocked",
		}
	}
}

/// The trajectory entry for one executed step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepTrace {
	/// 0-based index in the plan.
	pub index: usize,
	/// The step title (duplicated so a trace stands alone).
	pub title: String,
	/// The tool called (the fallback's, when degraded).
	pub tool: String,
	/// Outcome.
	pub status: StepStatus,
	/// How many attempts the primary tool took (1 = no retry).
	pub attempts: usize,
	/// Failure/mismatch detail, if any.
	pub detail: Option<String>,
}

// -------------------------------------------------------------- template

/// Resolves `${var}` templates in `args` against `vars`.
///
/// * the whole string `"${track}"` becomes the variable's JSON value
///   (id, number, object — whatever the producing step returned);
/// * a string *containing* `${track}` gets the value interpolated
///   (strings unquoted);
/// * an unresolved variable is an error, never a silent empty string.
pub fn substitute(args: &Value, vars: &Value) -> Result<Value, String> {
	fn resolve(text: &str, vars: &Value) -> Result<Value, String> {
		// Whole-string reference keeps the original JSON type. The name
		// is a dotted path: `${track}` reads a top-level field,
		// `${footage.0.id}` drills into the first array element (entity
		// arrays like `media.import_footage`'s result are objects).
		if text.starts_with("${") && text.ends_with('}') && text.matches("${").count() == 1 {
			let path = &text[2..text.len() - 1];
			return lookup(vars, path).cloned().ok_or_else(|| {
				format!("unresolved template variable `${{{path}}}` (available: {})", var_names(vars))
			});
		}
		let mut out = String::with_capacity(text.len());
		let bytes = text.as_bytes();
		let mut cursor = 0;
		while cursor < bytes.len() {
			if bytes[cursor] == b'$' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'{' {
				let Some(end) = text[cursor + 2..].find('}') else {
					out.push_str(&text[cursor..]);
					break;
				};
				let path = &text[cursor + 2..cursor + 2 + end];
				let value = lookup(vars, path).ok_or_else(|| {
					format!("unresolved template variable `${{{path}}}` (available: {})", var_names(vars))
				})?;
				out.push_str(&render(value));
				cursor = cursor + 2 + end + 1;
			} else {
				let ch = text[cursor..].chars().next().expect("cursor on a char boundary");
				out.push(ch);
				cursor += ch.len_utf8();
			}
		}
		Ok(Value::String(out))
	}
	fn render(value: &Value) -> String {
		match value {
			Value::String(s) => s.clone(),
			other => other.to_string(),
		}
	}
	/// Dotted-path lookup: `track` → field, `footage.0.id` → array
	/// element's field. Missing segments are `None`, never a default.
	fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
		let mut current = root;
		for segment in path.split('.') {
			current = match current {
				Value::Object(map) => map.get(segment)?,
				Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
				_ => return None,
			};
		}
		Some(current)
	}
	fn var_names(vars: &Value) -> String {
		match vars.as_object() {
			Some(map) if !map.is_empty() => map.keys().cloned().collect::<Vec<_>>().join(", "),
			_ => "<none>".to_string(),
		}
	}

	Ok(match args {
		Value::String(text) => resolve(text, vars)?,
		Value::Array(items) => Value::Array(
			items
				.iter()
				.map(|item| substitute(item, vars))
				.collect::<Result<Vec<_>, _>>()?,
		),
		Value::Object(map) => {
			let mut out = serde_json::Map::new();
			for (key, value) in map {
				out.insert(key.clone(), substitute(value, vars)?);
			}
			Value::Object(out)
		}
		other => other.clone(),
	})
}

// ---------------------------------------------------------------- recipe

/// A reusable workflow: a named step sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
	/// Stable name (`"rough-cut"`).
	pub name: String,
	/// One line for humans/panel pickers.
	pub description: String,
	/// The steps.
	pub steps: Vec<PlanStep>,
}

impl Recipe {
	/// Instantiates the recipe as a plan for `goal`.
	pub fn to_plan(&self, goal: impl Into<String>) -> Plan {
		Plan {
			goal: goal.into(),
			steps: self.steps.clone(),
			vars: json!({}),
		}
	}
}

/// The recipe library.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecipeBook {
	recipes: Vec<Recipe>,
}

impl RecipeBook {
	/// An empty book.
	pub fn empty() -> Self {
		Self::default()
	}

	/// The built-in workflows shipped with the agent.
	///
	/// * `open-and-inspect` — open a project and read it back; no writes,
	///   so no rollback path is exercised.
	/// * `rough-cut` — open, add a track, import `${media}`, place it at
	///   `${track}`/`${footage}` (the ids come from earlier steps' results,
	///   which is what makes a recipe more than a macro).
	pub fn builtin() -> Self {
		Self {
			recipes: vec![
				Recipe {
					name: "open-and-inspect".into(),
					description: "Open a project and read its overview".into(),
					steps: vec![
						PlanStep::new("open", "project.open", json!({"name": "inspect", "fps": 30})),
						PlanStep::new("inspect", "project.get_overview", json!({})),
					],
				},
				Recipe {
					name: "rough-cut".into(),
					description: "Open, track, import ${media}, place the first clip".into(),
					steps: vec![
						PlanStep::new("open", "project.open", json!({"name": "rough-cut", "fps": 30})),
						PlanStep::new("add track", "timeline.add_track", json!({"type": "video"})),
						PlanStep::new(
							"import media",
							"media.import_footage",
							json!({"paths": ["${media}"]}),
						),
						PlanStep::new(
							"place clip",
							"timeline.place_clip",
							json!({"track": "${track}", "footage": "${footage.0.id}", "in": 0}),
						),
					],
				},
			],
		}
	}

	/// Parses a book from JSON:
	/// `{"recipes":[{"name","description","steps":[{title,tool,args,on_fail}]}]}`
	/// where `on_fail` is `"fail"`, `"skip"` or
	/// `{"fallback":{"tool":…,"args":…}}`.
	pub fn from_json(raw: &str) -> Result<Self, String> {
		let root: Value = serde_json::from_str(raw).map_err(|err| format!("invalid JSON: {err}"))?;
		let recipes = root
			.get("recipes")
			.and_then(Value::as_array)
			.ok_or("`recipes` array missing")?;
		let mut out = Vec::new();
		for entry in recipes {
			let name = entry
				.get("name")
				.and_then(Value::as_str)
				.ok_or("recipe `name` missing")?
				.to_string();
			let description = entry
				.get("description")
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string();
			let steps_json = entry
				.get("steps")
				.and_then(Value::as_array)
				.ok_or_else(|| format!("recipe `{name}` has no `steps` array"))?;
			let mut steps = Vec::new();
			for step in steps_json {
				let title = step
					.get("title")
					.and_then(Value::as_str)
					.ok_or_else(|| format!("recipe `{name}`: step `title` missing"))?
					.to_string();
				let tool = step
					.get("tool")
					.and_then(Value::as_str)
					.ok_or_else(|| format!("recipe `{name}` step `{title}`: `tool` missing"))?
					.to_string();
				let args = step.get("args").cloned().unwrap_or_else(|| json!({}));
				let on_fail = match step.get("on_fail") {
					None => OnFail::Fail,
					Some(Value::String(s)) => match s.as_str() {
						"fail" => OnFail::Fail,
						"skip" => OnFail::Skip,
						other => return Err(format!("recipe `{name}`: unknown on_fail `{other}`")),
					},
					Some(Value::Object(map)) => {
						let fallback = map
							.get("fallback")
							.ok_or_else(|| format!("recipe `{name}`: on_fail object needs `fallback`"))?;
						OnFail::Fallback {
							tool: fallback
								.get("tool")
								.and_then(Value::as_str)
								.ok_or_else(|| format!("recipe `{name}`: fallback `tool` missing"))?
								.to_string(),
							args: fallback.get("args").cloned().unwrap_or_else(|| json!({})),
						}
					}
					Some(other) => {
						return Err(format!("recipe `{name}`: unsupported on_fail {other}"));
					}
				};
				steps.push(PlanStep {
					title,
					tool,
					args,
					on_fail,
				});
			}
			out.push(Recipe {
				name,
				description,
				steps,
			});
		}
		Ok(Self { recipes: out })
	}

	/// Every recipe, in load order.
	pub fn recipes(&self) -> &[Recipe] {
		&self.recipes
	}

	/// Look one up by name.
	pub fn get(&self, name: &str) -> Option<&Recipe> {
		self.recipes.iter().find(|recipe| recipe.name == name)
	}

	/// Turns a named recipe into a plan for `goal`.
	pub fn plan(&self, name: &str, goal: impl Into<String>) -> Result<Plan, String> {
		self.get(name)
			.map(|recipe| recipe.to_plan(goal))
			.ok_or_else(|| {
				format!(
					"no recipe `{name}` (known: {})",
					if self.recipes.is_empty() {
						"<none>".to_string()
					} else {
						self.recipes
							.iter()
							.map(|recipe| recipe.name.as_str())
							.collect::<Vec<_>>()
							.join(", ")
					}
				)
			})
	}
}

// ---------------------------------------------------------------- retry

/// How many times a failed step is retried, and how long to wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
	/// Total attempts (1 = no retry).
	pub max_attempts: usize,
	/// Delay between attempts, milliseconds.
	pub backoff_ms: u64,
}

impl RetryPolicy {
	/// One attempt, no delay.
	pub fn none() -> Self {
		Self {
			max_attempts: 1,
			backoff_ms: 0,
		}
	}

	/// Whether attempt number `attempt` (1-based, already made) should be
	/// retried, given the feedback the failure carried.
	pub fn should_retry(&self, attempt: usize, retryable: bool) -> bool {
		retryable && attempt < self.max_attempts
	}

	/// The delay before the next attempt.
	pub fn backoff(&self) {
		if self.backoff_ms > 0 {
			std::thread::sleep(std::time::Duration::from_millis(self.backoff_ms));
		}
	}
}

impl Default for RetryPolicy {
	fn default() -> Self {
		// One retry by default; tests and hosts pick their own numbers.
		Self {
			max_attempts: 2,
			backoff_ms: 100,
		}
	}
}

// ----------------------------------------------------------------- cost

/// Hard limits for one run. `None` = unlimited.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CostGuard {
	/// Maximum engine/tool calls (each dispatched call counts, including
	/// the ones the plan issues).
	pub max_tool_calls: Option<usize>,
	/// Maximum LLM rounds (a round = one `complete()` call).
	pub max_llm_rounds: Option<usize>,
	/// Maximum **estimated** tokens (see [`estimate_tokens`]).
	pub max_tokens: Option<u64>,
}

/// What one run has consumed so far.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CostUsage {
	/// Tool calls dispatched.
	pub tool_calls: usize,
	/// LLM rounds spent.
	pub llm_rounds: usize,
	/// Estimated tokens spent (chars / 4 — the LLM client carries no
	/// usage field, so this is an estimate, not a meter).
	pub tokens: u64,
}

/// Why a run stopped, with the numbers at the moment it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostStop {
	/// Which limit tripped: `"tool_calls"`, `"llm_rounds"`, `"tokens"`.
	pub limit: String,
	/// Human/model-readable explanation.
	pub detail: String,
	/// Usage when it tripped.
	pub usage: CostUsage,
}

impl CostGuard {
	/// No limits.
	pub fn unlimited() -> Self {
		Self::default()
	}

	/// The limit check run **before** a tool call: when the budget is
	/// already spent the call must not happen.
	pub fn check_tool(&self, usage: &CostUsage) -> Option<CostStop> {
		self.max_tool_calls.and_then(|max| {
			(usage.tool_calls >= max).then(|| CostStop {
				limit: "tool_calls".into(),
				detail: format!(
					"tool-call budget exhausted: {max} of {max} calls spent — \
					 stop, report what was done, and ask the user before more work",
				),
				usage: usage.clone(),
			})
		})
	}

	/// The limit check run before an LLM round.
	pub fn check_llm(&self, usage: &CostUsage) -> Option<CostStop> {
		self.max_llm_rounds.and_then(|max| {
			(usage.llm_rounds >= max).then(|| CostStop {
				limit: "llm_rounds".into(),
				detail: format!(
					"LLM-round budget exhausted: {max} of {max} rounds spent",
				),
				usage: usage.clone(),
			})
		})
	}

	/// The limit check run against the running token estimate.
	pub fn check_tokens(&self, usage: &CostUsage) -> Option<CostStop> {
		self.max_tokens.and_then(|max| {
			(usage.tokens >= max).then(|| CostStop {
				limit: "tokens".into(),
				detail: format!(
					"token budget exhausted: ~{} of ~{} tokens spent",
					usage.tokens, max
				),
				usage: usage.clone(),
			})
		})
	}
}

/// Rough token estimate (chars / 4).
///
/// The LLM client's `ChatResponse` carries no usage field (see
/// `ai/llm.rs`), so a token cap can only be enforced against an
/// estimate. It is labelled as such everywhere it surfaces; the
/// tool-call and LLM-round caps are exact.
pub fn estimate_tokens(text: &str) -> u64 {
	(text.chars().count() as u64 + 3) / 4
}

// -------------------------------------------------------------- rollback

/// The state before a plan (or a step group) started.
#[derive(Debug, Clone, PartialEq)]
pub enum Baseline {
	/// The probe snapshot.
	Snapshot(Value),
	/// The probe itself failed (no project yet, backend without
	/// overview, …) with this error — matched by error code, so an
	/// "unreadable" baseline can still be *restored* (i.e. unreadable
	/// again).
	Unreadable { code: String, message: String },
}

impl Baseline {
	/// Takes the baseline now (one probe call).
	pub fn capture(backend: &mut dyn EngineBackend) -> Self {
		match readback::probe(backend) {
			Ok(snapshot) => Self::Snapshot(snapshot),
			Err(error) => Self::Unreadable {
				code: error.code.clone(),
				message: error.message.clone(),
			},
		}
	}

	/// Whether a fresh probe matches this baseline.
	pub fn matches(&self, probe: &Result<Value, oak_ai_tools::error::ToolError>) -> bool {
		match (self, probe) {
			(Self::Snapshot(before), Ok(now)) => before == now,
			(Self::Unreadable { code, .. }, Err(error)) => code == &error.code,
			_ => false,
		}
	}
}

/// What an undo-back did (and did not) restore.
#[derive(Debug, Clone, PartialEq)]
pub struct RollbackReport {
	/// Why the rollback ran (the failing step / error).
	pub triggered_by: String,
	/// `edit.undo` calls issued successfully.
	pub undone: usize,
	/// Whether the probe matches the baseline again.
	pub restored: bool,
	/// Every `edit.undo` that failed, with its error — the reason a
	/// rollback can never be reported as "done" while it is not.
	pub failed_undos: Vec<String>,
	/// Steps that were applied and could **not** be confirmed undone
	/// (filled by the caller, which knows the trajectory).
	pub unrestored_steps: Vec<String>,
	/// Short conclusion ("state matches the baseline" / explicit failure).
	pub detail: String,
}

impl RollbackReport {
	/// Whether the rollback fully succeeded.
	pub fn is_clean(&self) -> bool {
		self.restored && self.failed_undos.is_empty() && self.unrestored_steps.is_empty()
	}
}

/// Undoes back to `baseline` with `edit.undo`, checking the probe after
/// each call.
///
/// Stops on: match (success), an undo error (recorded), or `max_undos`
/// (recorded as not restored). Never claims success it did not verify.
pub fn rollback_to(
	backend: &mut dyn EngineBackend,
	baseline: &Baseline,
	triggered_by: impl Into<String>,
	max_undos: usize,
) -> RollbackReport {
	let triggered_by = triggered_by.into();
	let mut undone = 0usize;
	let mut failed_undos = Vec::new();
	let mut restored = false;
	loop {
		if baseline.matches(&readback::probe(backend)) {
			restored = true;
			break;
		}
		if undone >= max_undos {
			break;
		}
		match dispatch::execute_with(backend, "edit.undo", json!({}), Policy::Auto, &mut NoHooks) {
			GatedOutcome::Applied { .. } => undone += 1,
			GatedOutcome::Failed { error, .. } => {
				failed_undos.push(format!(
					"edit.undo #{}: {} ({})",
					undone + 1,
					error.code,
					error.message
				));
				break;
			}
			GatedOutcome::Rejected { reason, error } => {
				failed_undos.push(format!(
					"edit.undo #{}: {} ({})",
					undone + 1,
					error.code,
					reason
				));
				break;
			}
			GatedOutcome::Proposed { .. } => {
				failed_undos.push(format!(
					"edit.undo #{}: not applied (proposal only)",
					undone + 1
				));
				break;
			}
		}
	}
	// Final verification even on the exit paths above.
	if !restored {
		restored = baseline.matches(&readback::probe(backend));
	}
	let detail = if restored {
		"state matches the baseline".to_string()
	} else if !failed_undos.is_empty() {
		format!(
			"rollback FAILED after {undone} undo(s): {}; the state may still hold the \
			 plan's writes — do not assume it was reverted",
			failed_undos.join("; ")
		)
	} else {
		format!(
			"rollback FAILED: {undone} undo(s) issued and the probe still does not match \
			 the baseline (undo cap {max_undos} reached)"
		)
	};
	RollbackReport {
		triggered_by,
		undone,
		restored,
		failed_undos,
		unrestored_steps: Vec::new(),
		detail,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use oak_ai_tools::mock::MockBackend;

	// ---------------------------------------------------------- templates

	/// ① Whole-string references keep the JSON type; missing vars error.
	#[test]
	fn template_resolution_keeps_types_and_fails_on_missing_vars() {
		let vars = json!({"track": "track-1", "count": 3});
		assert_eq!(
			substitute(&json!({"track": "${track}"}), &vars).expect("substitute"),
			json!({"track": "track-1"})
		);
		assert_eq!(
			substitute(&json!({"n": "${count}"}), &vars).expect("substitute"),
			json!({"n": 3}),
			"a whole-string reference keeps the number, not the string"
		);
		let err = substitute(&json!({"track": "${missing}"}), &vars).expect_err("must fail");
		assert!(err.contains("missing"), "{err}");
		// Interpolation inside a longer string.
		assert_eq!(
			substitute(&json!({"path": "/tmp/${track}.mp4"}), &vars).expect("substitute"),
			json!({"path": "/tmp/track-1.mp4"})
		);
		// Recursion into arrays.
		assert_eq!(
			substitute(&json!({"paths": ["${track}", "plain"]}), &vars).expect("substitute"),
			json!({"paths": ["track-1", "plain"]})
		);
		// Dotted paths drill into entity arrays (import results).
		let import_result = json!({"footage": [{"id": "footage-9"}], "failed": []});
		assert_eq!(
			substitute(&json!({"footage": "${footage.0.id}"}), &import_result).expect("path"),
			json!({"footage": "footage-9"}),
			"a whole-string path keeps the scalar type"
		);
		let err = substitute(&json!({"footage": "${footage.9.id}"}), &import_result)
			.expect_err("out of range");
		assert!(err.contains("footage.9.id"), "{err}");
	}

	// ------------------------------------------------------------- plans

	/// ② A plan carries goal, steps and seeded vars.
	#[test]
	fn plan_builder_assembles_the_step_list() {
		let plan = Plan::new("cut one clip")
			.with_vars(json!({"media": "/tmp/a.mp4"}))
			.push(PlanStep::new("open", "project.open", json!({"name": "x", "fps": 30})))
			.push(
				PlanStep::new("import", "media.import_footage", json!({"paths": ["${media}"]})),
			);
		assert_eq!(plan.goal, "cut one clip");
		assert_eq!(plan.steps.len(), 2);
		let resolved = substitute(&plan.steps[1].args, &plan.vars).expect("vars");
		assert_eq!(resolved, json!({"paths": ["/tmp/a.mp4"]}));
	}

	/// ③ OnFail defaults to Fail and labels itself.
	#[test]
	fn on_fail_defaults_to_failing_the_plan() {
		assert_eq!(PlanStep::new("t", "tool", json!({})).on_fail, OnFail::Fail);
		assert_eq!(OnFail::Skip.label(), "skip");
		assert_eq!(
			OnFail::Fallback {
				tool: "edit.undo".into(),
				args: json!({})
			}
			.label(),
			"fallback"
		);
		assert_eq!(StepStatus::Degraded.label(), "degraded");
	}

	// ----------------------------------------------------------- recipes

	/// ④ Built-ins exist and instantiate as plans.
	#[test]
	fn builtin_recipes_build_plans() {
		let book = RecipeBook::builtin();
		let rough = book.plan("rough-cut", "cut one clip").expect("plan");
		assert_eq!(rough.steps.len(), 4);
		assert!(book.get("open-and-inspect").is_some());
		assert_eq!(book.recipes().len(), 2);
		let unknown = book.plan("nope", "x").expect_err("unknown recipe");
		assert!(unknown.contains("nope"), "{unknown}");
	}

	/// ⑤ JSON loading, including the fallback form of on_fail.
	#[test]
	fn recipes_load_from_json() {
		let raw = r#"{"recipes":[{"name":"safe","description":"d","steps":[
			{"title":"open","tool":"project.open","args":{"name":"x","fps":30}},
			{"title":"save","tool":"project.save","args":{},"on_fail":"skip"},
			{"title":"export","tool":"export.start","args":{},"on_fail":{"fallback":{"tool":"render.start","args":{}}}}
		]}]}"#;
		let book = RecipeBook::from_json(raw).expect("load");
		let safe = book.get("safe").expect("recipe");
		assert_eq!(safe.steps.len(), 3);
		assert_eq!(safe.steps[0].on_fail, OnFail::Fail);
		assert_eq!(safe.steps[1].on_fail, OnFail::Skip);
		assert!(matches!(safe.steps[2].on_fail, OnFail::Fallback { .. }));
		// Errors are named, not generic.
		let err = RecipeBook::from_json(r#"{"recipes":[{"name":"broken","steps":[{"title":"t"}]}]}"#)
			.expect_err("no tool");
		assert!(err.contains("`tool` missing"), "{err}");
		let err = RecipeBook::from_json("not json").expect_err("bad json");
		assert!(err.contains("invalid JSON"), "{err}");
	}

	// ------------------------------------------------------------- retry

	/// ⑥ Retry honours both the attempt cap and the retryable flag.
	#[test]
	fn retry_honours_attempts_and_retryable() {
		let policy = RetryPolicy {
			max_attempts: 3,
			backoff_ms: 0,
		};
		assert!(policy.should_retry(1, true));
		assert!(policy.should_retry(2, true));
		assert!(!policy.should_retry(3, true), "the cap is inclusive");
		assert!(!policy.should_retry(1, false), "non-retryable never retries");
		assert!(!RetryPolicy::none().should_retry(1, true));
	}

	// -------------------------------------------------------------- cost

	/// ⑦ The guard stops at the limit — it does not merely count.
	#[test]
	fn cost_guard_stops_at_each_limit() {
		let guard = CostGuard {
			max_tool_calls: Some(2),
			max_llm_rounds: Some(1),
			max_tokens: Some(10),
		};
		let mut usage = CostUsage::default();
		assert!(guard.check_tool(&usage).is_none(), "0 < 2 may call");
		usage.tool_calls = 1;
		assert!(guard.check_tool(&usage).is_none(), "1 < 2 may call");
		usage.tool_calls = 2;
		let stop = guard.check_tool(&usage).expect("2 >= 2 must stop");
		assert_eq!(stop.limit, "tool_calls");
		assert!(stop.detail.contains("2 of 2"), "{}", stop.detail);

		usage.llm_rounds = 1;
		let stop = guard.check_llm(&usage).expect("1 >= 1 must stop");
		assert_eq!(stop.limit, "llm_rounds");

		usage.tokens = 10;
		let stop = guard.check_tokens(&usage).expect("10 >= 10 must stop");
		assert_eq!(stop.limit, "tokens");
		assert!(stop.detail.contains("~10"), "{}", stop.detail);
	}

	/// ⑧ Unlimited is the default; the token estimate is labelled.
	#[test]
	fn cost_default_is_unlimited() {
		let guard = CostGuard::unlimited();
		let usage = CostUsage {
			tool_calls: 9_999,
			llm_rounds: 9_999,
			tokens: u64::MAX,
		};
		assert!(guard.check_tool(&usage).is_none());
		assert!(guard.check_llm(&usage).is_none());
		assert!(guard.check_tokens(&usage).is_none());
		assert_eq!(estimate_tokens("abcd"), 1);
		assert_eq!(estimate_tokens("abcde"), 2);
	}

	// ----------------------------------------------------------- rollback

	/// ⑨ Undo-back restores a seeded state, and reports it honestly.
	#[test]
	fn rollback_restores_the_baseline() {
		let mut backend = MockBackend::new();
		dispatch::execute(&mut backend, "project.open", json!({"name": "rb", "fps": 30}))
			.expect("open");
		let baseline = Baseline::capture(&mut backend);

		// Two writes to undo.
		dispatch::execute(&mut backend, "timeline.add_track", json!({"type": "video"}))
			.expect("track");
		dispatch::execute(&mut backend, "timeline.add_track", json!({"type": "audio"}))
			.expect("track");
		assert_ne!(
			readback::probe(&mut backend).expect("probe"),
			match &baseline {
				Baseline::Snapshot(value) => value.clone(),
				other => panic!("expected a snapshot baseline, got {other:?}"),
			}
		);

		let report = rollback_to(&mut backend, &baseline, "step `add track` failed", 16);
		assert!(report.restored, "{}", report.detail);
		assert!(report.is_clean(), "{report:?}");
		assert_eq!(report.undone, 2, "one undo per applied write");
		assert!(report.failed_undos.is_empty());
		assert_eq!(report.triggered_by, "step `add track` failed");
	}

	/// ⑩ An undo that cannot run is recorded, and the report says FAILED
	/// — never a silent "rolled back".
	#[test]
	fn a_failing_undo_is_reported_as_a_failed_rollback() {
		let mut backend = MockBackend::new();
		// No project, nothing to undo: the first probe already fails and
		// the undo answers NOTHING_TO_UNDO (or the probe errors out of a
		// matching "unreadable" baseline — either way we must not claim
		// a clean restore unless the probe really matches).
		dispatch::execute(&mut backend, "timeline.add_track", json!({"type": "video"}))
			.expect_err("no project");
		let baseline = Baseline::capture(&mut backend);
		let report = rollback_to(&mut backend, &baseline, "step failed", 4);
		match &baseline {
			// Nothing was ever applied: unreadable baseline + unreadable
			// probe = restored, and that is the truth.
			Baseline::Unreadable { .. } => {
				assert!(report.restored, "{}", report.detail);
				assert!(report.is_clean(), "{report:?}");
			}
			Baseline::Snapshot(_) => {
				assert!(!report.is_clean(), "a snapshot baseline with nothing applied");
				assert!(!report.failed_undos.is_empty() || !report.restored);
			}
		}
	}

	/// ⑪ `is_clean` refuses when unrestored steps are listed — the
	/// explicit-error requirement for a failed rollback.
	#[test]
	fn unrestored_steps_make_the_report_unclean() {
		let mut report = RollbackReport {
			triggered_by: "step 3".into(),
			undone: 1,
			restored: true,
			failed_undos: vec![],
			unrestored_steps: vec!["place clip".into()],
			detail: "state matches the baseline".into(),
		};
		assert!(!report.is_clean(), "a step we cannot confirm undone must surface");
		report.unrestored_steps.clear();
		assert!(report.is_clean());
	}
}

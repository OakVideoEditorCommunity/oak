//! Post-write readback self-check (capability group 2).
//!
//! The rule: a write that reports success must be *read back* and the
//! state compared with what the call promised. A mismatch is a failure
//! with a diff — never a silent success.
//!
//! The probe is `project.get_overview` (a read class, so it runs under
//! every [`Policy`] and never touches the confirmation gate). It calls
//! the backend's `EngineBackend::project_get_overview` **directly** —
//! the same entry point the tool face dispatches to, minus the undo
//! wrapper: every dispatch call commits its own undo group (the mock's
//! `txn_commit` pushes even an empty one), and a probe that kept pushing
//! empty groups would bury the writes an [`crate::ai::plan::rollback_to`]
//! is trying to pop. No probe ever touches the undo stack.
//!
//! `export.start` is deliberately **not** read back: its artifact is a
//! file outside the engine, and the tool already reports the byte count
//! it wrote (see `ai/oak-ai-tools` README, "No fake success").

use oak_ai_tools::backend::EngineBackend;
use oak_ai_tools::error::ToolError;
use serde_json::{Value, json};

/// The read tool used as the probe.
pub const PROBE_TOOL: &str = "project.get_overview";

/// What the state must look like after the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
	/// The probe must contain every one of these ids (a write that mints
	/// an entity — `clip`, `track`, `footage`, …).
	IdsPresent(Vec<String>),
	/// The state must differ from the baseline taken before the write.
	Changed,
	/// The listed ids must be **gone from the observable state** (the
	/// overview no longer contains them) and — with a baseline — the
	/// state must have moved. This is how a delete is verified: the check
	/// runs against the probe, never against the tool's own return body.
	IdsGone(Vec<String>),
	/// The probe must still succeed (the document stays readable).
	ProbeOk,
	/// The probe must report at least one sequence (`project.open`).
	HasSequence,
}

impl Expect {
	/// Short label for reports.
	pub fn label(&self) -> &'static str {
		match self {
			Self::IdsPresent(_) => "ids-present",
			Self::IdsGone(_) => "ids-gone",
			Self::Changed => "changed",
			Self::ProbeOk => "probe-ok",
			Self::HasSequence => "has-sequence",
		}
	}
}

/// The verdict of one readback, with the evidence attached.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadbackReport {
	/// False ⇒ the write must be reported as failed.
	pub ok: bool,
	/// What was expected.
	pub expect: Expect,
	/// Human/model-readable description of the mismatch (`None` when ok).
	pub diff: Option<String>,
	/// Baseline probe (before the write), when one was taken.
	pub before: Option<Value>,
	/// Probe after the write.
	pub after: Option<Value>,
}

impl ReadbackReport {
	/// The model-facing payload for a **failed** readback: a structured
	/// failure the re-planning loop can act on.
	pub fn failure_payload(&self) -> Value {
		json!({
			"ok": false,
			"code": "READBACK_MISMATCH",
			"expect": self.expect.label(),
			"message": self.diff.clone().unwrap_or_else(|| "readback mismatch".into()),
			"hint": "the tool reported success but the state does not match; \
			         re-read project.get_overview, verify the entity, then retry \
			         or undo — do not assume the write landed",
		})
	}
}

/// Runs the probe on the backend directly.
///
/// Same method the `project.get_overview` tool dispatches to, but
/// without the undo-group wrapper (see the module docs): a probe must
/// not perturb the very state — or the undo stack — it is verifying.
pub fn probe(backend: &mut dyn EngineBackend) -> Result<Value, ToolError> {
	backend.project_get_overview(&json!({}))
}

/// Which probes to run around one mutating call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbePlan {
	/// Do not read this tool back at all.
	Skip,
	/// Probe after the call only (there is no usable baseline before it).
	AfterOnly,
	/// Probe before **and** after, so `Changed` can be judged.
	BeforeAndAfter,
}

/// The probe plan for a (mutating) tool.
///
/// * `export.start` — artifact outside the engine → skip;
/// * `project.open` — no project exists to probe beforehand → after only;
/// * `project.save` — the timeline does not move (the file does) → after
///   only, checking the document still reads;
/// * everything else — before and after.
pub fn probe_plan(tool: &str) -> ProbePlan {
	match tool {
		"export.start" => ProbePlan::Skip,
		"project.open" | "project.save" => ProbePlan::AfterOnly,
		_ => ProbePlan::BeforeAndAfter,
	}
}

/// The expectation for `tool`'s result, or `None` when the tool must not
/// be read back at all.
///
/// Generic rule first (ids the result mints ⇒ `IdsPresent`, otherwise
/// `Changed`), three named exceptions where the generic rule would lie:
///
/// * `project.open` has no baseline (there was no project to probe), so
///   "changed" is unprovable — the honest check is "a sequence exists";
/// * `project.save` writes a file, not the timeline: the state must stay
///   readable (`ProbeOk`), the bytes are the tool's own read-back;
/// * `export.start` writes outside the engine → skipped entirely.
pub fn expectation_for(tool: &str, result: &Value) -> Option<Expect> {
	match tool {
		"project.open" => return Some(Expect::HasSequence),
		"project.save" => return Some(Expect::ProbeOk),
		"export.start" => return None,
		_ => {}
	}
	// A delete reports the ids it *removed*: the verification is that
	// those ids are **gone from the observable state**, not that the
	// return body said so (`timeline.ripple_delete` → `IdsGone`).
	if result.get("removed").and_then(Value::as_bool) == Some(true) {
		let removed = collect_ids(result);
		if !removed.is_empty() {
			return Some(Expect::IdsGone(removed));
		}
		return Some(Expect::Changed);
	}
	let ids = collect_ids(result);
	if ids.is_empty() {
		Some(Expect::Changed)
	} else {
		Some(Expect::IdsPresent(ids))
	}
}

/// Entity ids named by a tool result.
///
/// Keys that name an entity, plus the generic `id`. Arrays are walked
/// (`media.import_footage` returns `footage: [{id, …}]`), objects one
/// level down; everything else is ignored — a *state* comparison is the
/// fallback when a result names nothing.
fn collect_ids(result: &Value) -> Vec<String> {
	const KEYS: [&str; 9] = [
		"id",
		"clip",
		"new_clip",
		"track",
		"footage",
		"node",
		"marker",
		"transition",
		"sequence",
	];
	let mut out: Vec<String> = Vec::new();
	fn walk(value: &Value, out: &mut Vec<String>) {
		match value {
			Value::Object(map) => {
				for (key, child) in map {
					if KEYS.contains(&key.as_str()) {
						if let Some(id) = child.as_str() {
							let id = id.to_string();
							if !out.contains(&id) {
								out.push(id);
							}
						}
					}
					// Entity containers keep going (`footage: [{id}]`).
					if child.is_array() || child.is_object() {
						walk(child, out);
					}
				}
			}
			Value::Array(items) => {
				for item in items {
					walk(item, out);
				}
			}
			_ => {}
		}
	}
	walk(result, &mut out);
	out
}

/// Compares the probes against the expectation. Pure: no backend, no I/O,
/// so every branch is unit-testable.
///
/// Strict on purpose: when the baseline is missing for a `Changed`
/// expectation the answer is `false` with an explanatory diff — "could
/// not prove the change" is not "the change happened".
pub fn verify(before: Option<&Value>, after: &Value, expect: Expect) -> ReadbackReport {
	let finish = |ok: bool, diff: Option<String>| ReadbackReport {
		ok,
		diff,
		expect: expect.clone(),
		before: before.cloned(),
		after: Some(after.clone()),
	};
	// Borrow (do not move): `finish` still clones `expect` for the report.
	match &expect {
		Expect::IdsPresent(ids) => {
			let missing: Vec<&str> = ids
				.iter()
				.filter(|id| !contains_string(after, id))
				.map(String::as_str)
				.collect();
			if missing.is_empty() {
				finish(true, None)
			} else {
				finish(
					false,
					Some(format!(
						"the write reported {} but the state does not contain: {}",
						ids.join(", "),
						missing.join(", ")
					)),
				)
			}
		}
		Expect::IdsGone(ids) => {
			let still_there: Vec<&str> = ids
				.iter()
				.filter(|id| contains_string(after, id))
				.map(String::as_str)
				.collect();
			if !still_there.is_empty() {
				return finish(
					false,
					Some(format!(
						"the delete reported {} but the state still contains: {}",
						ids.join(", "),
						still_there.join(", ")
					)),
				);
			}
			match before {
				// Ids gone but the state identical ⇒ the baseline never
				// held them either: nothing observable was deleted.
				Some(prev) if prev == after => finish(
					false,
					Some(
						"the reported ids are absent, but the state is identical to the \
						 baseline — nothing observable was deleted"
							.into(),
					),
				),
				_ => finish(true, None),
			}
		}
		Expect::Changed => match before {
			Some(prev) if prev == after => finish(
				false,
				Some("the tool reported success but the state is byte-identical to the baseline".into()),
			),
			Some(_) => finish(true, None),
			None => finish(
				false,
				Some(
					"no baseline probe before the write, so the change cannot be proven \
					 (the probe itself failed — see its error)"
						.into(),
				),
			),
		},
		Expect::ProbeOk => finish(true, None),
		Expect::HasSequence => {
			let sequences = after.get("sequences").and_then(Value::as_array);
			match sequences {
				Some(list) if !list.is_empty() => finish(true, None),
				Some(_) => finish(false, Some("the project opened but has no sequence".into())),
				None => finish(false, Some("the overview carries no `sequences` array".into())),
			}
		}
	}
}

/// Whether `haystack` contains `needle` as a string value anywhere.
fn contains_string(haystack: &Value, needle: &str) -> bool {
	match haystack {
		Value::String(s) => s == needle,
		Value::Array(items) => items.iter().any(|item| contains_string(item, needle)),
		Value::Object(map) => map.values().any(|child| contains_string(child, needle)),
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use oak_ai_tools::dispatch;
	use oak_ai_tools::mock::MockBackend;
	use serde_json::json;

	fn overview_with_clip() -> Value {
		json!({
			"sequences": [{
				"id": "sequence-1",
				"tracks": [{ "id": "track-1", "clips": [{ "id": "clip-7", "footage": "footage-2" }] }]
			}],
			"footage": [{ "id": "footage-2" }]
		})
	}

	/// ① A minted id that is readable back ⇒ ok.
	#[test]
	fn ids_present_passes_when_the_probe_reports_them() {
		let report = verify(
			None,
			&overview_with_clip(),
			Expect::IdsPresent(vec!["clip-7".into(), "footage-2".into()]),
		);
		assert!(report.ok, "{report:?}");
		assert!(report.diff.is_none());
	}

	/// ② A missing id ⇒ failure with a diff naming it (never silent).
	#[test]
	fn ids_present_fails_with_a_diff_naming_the_missing_entity() {
		let report = verify(
			None,
			&overview_with_clip(),
			Expect::IdsPresent(vec!["clip-7".into(), "clip-404".into()]),
		);
		assert!(!report.ok);
		let diff = report.diff.as_ref().expect("a diff must be reported");
		assert!(diff.contains("clip-404"), "{diff}");
		let payload = report.failure_payload();
		assert_eq!(payload["code"], "READBACK_MISMATCH");
		assert!(payload["hint"].is_string());
	}

	/// ③ "Changed" with an identical baseline ⇒ failure.
	#[test]
	fn unchanged_state_fails_the_changed_expectation() {
		let state = overview_with_clip();
		let report = verify(Some(&state), &state, Expect::Changed);
		assert!(!report.ok, "byte-identical state must not pass");
		assert!(report.diff.expect("diff").contains("byte-identical"));
	}

	/// ④ Changed with a real difference ⇒ ok.
	#[test]
	fn changed_state_passes() {
		let before = overview_with_clip();
		let mut after = before.clone();
		after["sequences"][0]["tracks"][0]["clips"]
			.as_array_mut()
			.expect("clips")
			.push(json!({"id": "clip-8"}));
		let report = verify(Some(&before), &after, Expect::Changed);
		assert!(report.ok, "{report:?}");
	}

	/// ⑤ Strict: no baseline ⇒ cannot prove the change ⇒ failure.
	#[test]
	fn a_missing_baseline_fails_instead_of_assuming_success() {
		let report = verify(None, &overview_with_clip(), Expect::Changed);
		assert!(!report.ok);
		assert!(report.diff.expect("diff").contains("no baseline"));
	}

	/// ⑥ `project.open` has no baseline; the honest check is the sequence.
	#[test]
	fn project_open_expects_a_sequence() {
		assert_eq!(
			expectation_for("project.open", &json!({"created": true})),
			Some(Expect::HasSequence)
		);
		let with = json!({"sequences": [{"id": "sequence-1"}]});
		assert!(verify(None, &with, Expect::HasSequence).ok);
		let empty = json!({"sequences": []});
		let report = verify(None, &empty, Expect::HasSequence);
		assert!(!report.ok, "an opened project must have a sequence");
	}

	/// ⑦ `export.start` writes outside the engine ⇒ no readback at all.
	#[test]
	fn export_is_not_read_back() {
		assert_eq!(
			expectation_for("export.start", &json!({"started": true, "bytes": 12})),
			None
		);
	}

	/// ⑧ `project.save` keeps the document readable; the bytes are the
	/// tool's own read-back.
	#[test]
	fn save_expects_a_readable_document() {
		assert_eq!(
			expectation_for("project.save", &json!({"saved": true, "bytes": 9})),
			Some(Expect::ProbeOk)
		);
		assert!(verify(None, &json!({}), Expect::ProbeOk).ok);
	}

	/// ⑨ The generic rule: an id-carrying result ⇒ ids; otherwise change.
	#[test]
	fn generic_expectation_rule() {
		assert_eq!(
			expectation_for("timeline.place_clip", &json!({"clip": "clip-3", "length": {"num": 1}})),
			Some(Expect::IdsPresent(vec!["clip-3".into()]))
		);
		assert_eq!(
			expectation_for("timeline.ripple_delete", &json!({"removed": true, "ripple": 3})),
			Some(Expect::Changed)
		);
		// Nested entity containers are walked.
		assert_eq!(
			expectation_for(
				"media.import_footage",
				&json!({"footage": [{"id": "footage-9"}], "failed": []})
			),
			Some(Expect::IdsPresent(vec!["footage-9".into()]))
		);
	}

	/// ⑩ End-to-end on the real mock: open → place a clip → the probe
	/// really contains the id the write reported.
	#[test]
	fn the_probe_reads_back_a_real_write() {
		let mut backend = MockBackend::new();
		dispatch::execute(&mut backend, "project.open", json!({"name": "rb", "fps": 30}))
			.expect("open");
		// The mock only checks that the file exists.
		std::fs::write("/tmp/oak-readback-fixture.mp4", b"x").expect("fixture");
		let footage = dispatch::execute(
			&mut backend,
			"media.import_footage",
			json!({"paths": ["/tmp/oak-readback-fixture.mp4"]}),
		)
		.unwrap_or_else(|err| panic!("import: {err}"));
		let footage_id = footage["footage"][0]["id"].as_str().expect("id").to_string();
		let track = dispatch::execute(&mut backend, "timeline.add_track", json!({"type": "video"}))
			.expect("track")["track"]
			.as_str()
			.expect("track id")
			.to_string();

		let placed = dispatch::execute(
			&mut backend,
			"timeline.place_clip",
			json!({"track": track, "footage": footage_id, "in": 0}),
		)
		.expect("place");
		let expect = expectation_for("timeline.place_clip", &placed).expect("expectation");

		let after = probe(&mut backend).expect("probe");
		let report = verify(None, &after, expect);
		assert!(report.ok, "{report:?}");
	}

	/// ⑪ A write that lies (result names an id the state lacks) fails.
	#[test]
	fn a_lying_result_is_caught() {
		let mut backend = MockBackend::new();
		dispatch::execute(&mut backend, "project.open", json!({"name": "rb", "fps": 30}))
			.expect("open");
		let after = probe(&mut backend).expect("probe");
		let report = verify(
			None,
			&after,
			Expect::IdsPresent(vec!["clip-does-not-exist".into()]),
		);
		assert!(!report.ok);
		assert!(report.diff.expect("diff").contains("clip-does-not-exist"));
	}

	/// ⑫ A delete is verified against **observable state**: after
	/// `timeline.ripple_delete` the clip id must be gone from the
	/// overview — the return body claiming `removed: true` proves nothing.
	#[test]
	fn a_delete_is_verified_against_the_overview() {
		let mut backend = MockBackend::new();
		dispatch::execute(&mut backend, "project.open", json!({"name": "rb", "fps": 30}))
			.expect("open");
		std::fs::write("/tmp/oak-readback-fixture.mp4", b"x").expect("fixture");
		let footage = dispatch::execute(
			&mut backend,
			"media.import_footage",
			json!({"paths": ["/tmp/oak-readback-fixture.mp4"]}),
		)
		.expect("import");
		let footage_id = footage["footage"][0]["id"].as_str().expect("id").to_string();
		let track = dispatch::execute(&mut backend, "timeline.add_track", json!({"type": "video"}))
			.expect("track")["track"]
			.as_str()
			.expect("track id")
			.to_string();
		let placed = dispatch::execute(
			&mut backend,
			"timeline.place_clip",
			json!({"track": track, "footage": footage_id, "in": 0}),
		)
		.expect("place");
		let clip_id = placed["clip"].as_str().expect("clip id").to_string();

		let before = probe(&mut backend).expect("probe");
		assert!(contains_string(&before, &clip_id), "the clip exists before the delete");
		let deleted = dispatch::execute(
			&mut backend,
			"timeline.ripple_delete",
			json!({"clip": clip_id}),
		)
		.expect("delete");
		assert_eq!(deleted["removed"], true, "the tool claims it deleted");

		let expect = expectation_for("timeline.ripple_delete", &deleted).expect("expectation");
		assert!(
			matches!(expect, Expect::IdsGone(_)),
			"a delete must be checked as gone, not merely changed: {expect:?}"
		);
		let after = probe(&mut backend).expect("probe");
		assert!(
			!contains_string(&after, &clip_id),
			"the observable state must no longer contain the clip"
		);
		let report = verify(Some(&before), &after, expect);
		assert!(report.ok, "{report:?}");
	}

	/// ⑬ The negative case (lead's requirement): the expectation is *not*
	/// met ⇒ the readback **fails**, whichever way it is missed.
	#[test]
	fn a_delete_that_left_the_clip_in_state_fails() {
		let before = json!({
			"sequences": [{
				"id": "sequence-1",
				"tracks": [{ "id": "track-1", "clips": [{ "id": "clip-7" }] }]
			}]
		});

		// (a) The state did not move at all: the id is still there.
		let unchanged = before.clone();
		let report = verify(Some(&before), &unchanged, Expect::IdsGone(vec!["clip-7".into()]));
		assert!(!report.ok, "an unmet expectation must fail");
		let diff = report.diff.as_ref().expect("a failure carries a diff");
		assert!(diff.contains("still contains") || diff.contains("identical"), "{diff}");
		assert_eq!(report.failure_payload()["code"], "READBACK_MISMATCH");

		// (b) The state moved for other reasons but the deleted clip is
		// still present: presence wins — the id must be gone.
		let mut moved = before.clone();
		moved["sequences"][0]["tracks"]
			.as_array_mut()
			.expect("tracks")
			.push(json!({"id": "track-2", "clips": []}));
		let report = verify(Some(&before), &moved, Expect::IdsGone(vec!["clip-7".into()]));
		assert!(!report.ok, "state change alone must not excuse a surviving id");
		assert!(
			report.diff.expect("diff").contains("still contains"),
			"the diff must name what survived"
		);
	}

}

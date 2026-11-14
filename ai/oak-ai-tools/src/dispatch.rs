//! Tool registry and dispatcher: frozen tool name → schema + handler.
//!
//! The registry is the single place where the 22 tool names of
//! `ai/schema/tools/*.json` are bound to a backend method. The
//! `registry_drift` test asserts a **bidirectional, one-to-one** match
//! against the schema directory, so adding or removing a schema without
//! touching this table (or the reverse) turns the test red.

use std::sync::OnceLock;

use serde_json::Value;

use crate::backend::EngineBackend;
use crate::error::{CODE_UNKNOWN_TOOL, ToolError};
use crate::schema::SchemaRegistry;
use crate::tools;

/// A backend method bound to a frozen tool name.
pub struct ToolDef {
	/// Tool name (= schema file name).
	pub name: &'static str,
	/// Whether the call is wrapped in one undo group (everything except
	/// `edit.undo`, which per OPP/1 section 6 has no transaction).
	pub transactional: bool,
	/// The backend entry point.
	pub call: BackendCall,
}

/// Backend entry point of a tool.
pub type BackendCall = fn(&mut dyn EngineBackend, &Value) -> Result<Value, ToolError>;

/// The frozen 22-tool registry, in schema-file alphabetical order.
pub fn registry() -> &'static [ToolDef] {
	static REGISTRY: [ToolDef; 22] = [
		ToolDef {
			name: "edit.undo",
			transactional: false,
			call: |backend, args| {
				tools::edit::parse_undo(args)?;
				backend.edit_undo(args)
			},
		},
		ToolDef {
			name: "export.start",
			transactional: true,
			call: |backend, args| {
				tools::export::parse_export(args)?;
				backend.export_start(args)
			},
		},
		ToolDef {
			name: "media.import_footage",
			transactional: true,
			call: |backend, args| {
				tools::media::parse_import(args)?;
				backend.media_import_footage(args)
			},
		},
		ToolDef {
			name: "media.list_footage",
			transactional: true,
			call: |backend, args| {
				tools::media::parse_list(args)?;
				backend.media_list_footage(args)
			},
		},
		ToolDef {
			name: "media.probe",
			transactional: true,
			call: |backend, args| {
				tools::media::parse_probe(args)?;
				backend.media_probe(args)
			},
		},
		ToolDef {
			name: "node.add_effect",
			transactional: true,
			call: |backend, args| {
				tools::node::parse_add_effect(args)?;
				backend.node_add_effect(args)
			},
		},
		ToolDef {
			name: "node.list_types",
			transactional: true,
			call: |backend, args| {
				tools::node::parse_list_types(args)?;
				backend.node_list_types(args)
			},
		},
		ToolDef {
			name: "node.set_param",
			transactional: true,
			call: |backend, args| {
				tools::node::parse_set_param(args)?;
				backend.node_set_param(args)
			},
		},
		ToolDef {
			name: "project.get_overview",
			transactional: true,
			call: |backend, args| {
				tools::project::parse_overview(args)?;
				backend.project_get_overview(args)
			},
		},
		ToolDef {
			name: "project.open",
			transactional: true,
			call: |backend, args| {
				tools::project::parse_open(args)?;
				backend.project_open(args)
			},
		},
		ToolDef {
			name: "project.save",
			transactional: true,
			call: |backend, args| {
				tools::project::parse_save(args)?;
				backend.project_save(args)
			},
		},
		ToolDef {
			name: "render.get_audio_levels",
			transactional: true,
			call: |backend, args| {
				tools::render_tools::parse_get_audio_levels(args)?;
				backend.render_get_audio_levels(args)
			},
		},
		ToolDef {
			name: "render.get_frame",
			transactional: true,
			call: |backend, args| {
				tools::render_tools::parse_get_frame(args)?;
				backend.render_get_frame(args)
			},
		},
		ToolDef {
			name: "render.get_thumbnails",
			transactional: true,
			call: |backend, args| {
				tools::render_tools::parse_get_thumbnails(args)?;
				backend.render_get_thumbnails(args)
			},
		},
		ToolDef {
			name: "timeline.add_marker",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_add_marker(args)?;
				backend.timeline_add_marker(args)
			},
		},
		ToolDef {
			name: "timeline.add_track",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_add_track(args)?;
				backend.timeline_add_track(args)
			},
		},
		ToolDef {
			name: "timeline.add_transition",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_add_transition(args)?;
				backend.timeline_add_transition(args)
			},
		},
		ToolDef {
			name: "timeline.move_clip",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_move(args)?;
				backend.timeline_move_clip(args)
			},
		},
		ToolDef {
			name: "timeline.place_clip",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_place_clip(args)?;
				backend.timeline_place_clip(args)
			},
		},
		ToolDef {
			name: "timeline.ripple_delete",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_ripple_delete(args)?;
				backend.timeline_ripple_delete(args)
			},
		},
		ToolDef {
			name: "timeline.split_clip",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_split(args)?;
				backend.timeline_split_clip(args)
			},
		},
		ToolDef {
			name: "timeline.trim_clip",
			transactional: true,
			call: |backend, args| {
				tools::timeline::parse_trim(args)?;
				backend.timeline_trim_clip(args)
			},
		},
	];
	&REGISTRY
}

/// Registry names, sorted (they are already alphabetical).
pub fn tool_names() -> Vec<&'static str> {
	registry().iter().map(|def| def.name).collect()
}

/// Looks a tool up.
pub fn find(tool: &str) -> Option<&'static ToolDef> {
	registry().iter().find(|def| def.name == tool)
}

/// Process-wide compiled schema registry (frozen files, loaded once).
pub fn default_schemas() -> &'static SchemaRegistry {
	static SCHEMAS: OnceLock<SchemaRegistry> = OnceLock::new();
	SCHEMAS.get_or_init(SchemaRegistry::load_default)
}

/// Validates `args` against `tool`'s frozen schema.
pub fn validate(tool: &str, args: &Value) -> Result<(), ToolError> {
	default_schemas()
		.validate(tool, args)
		.map_err(|err| err.with_tool(tool).with_step("validate"))
}

/// One tool call end to end:
///
/// 1. unknown tool → `UNKNOWN_TOOL` (step `lookup`);
/// 2. schema validation → `SCHEMA_VALIDATION_FAILED` (step `validate`),
///    the backend is never entered with invalid arguments;
/// 3. transactional tools run as one undo group (`begin`/`commit`/
///    `abort`), others run plain;
/// 4. failures come back annotated with `tool` / `step` / `txn`.
///
/// This is the hooks-free entry point: it is exactly [`execute_with`] with
/// [`crate::hooks::Policy::Auto`] and [`crate::hooks::NoHooks`], so its
/// behaviour is unchanged from before the adapter hooks existed (a caller
/// that installs no gate gets no gate).
pub fn execute(
	backend: &mut dyn EngineBackend,
	tool: &str,
	args: Value,
) -> Result<Value, ToolError> {
	execute_with(
		backend,
		tool,
		args,
		crate::hooks::Policy::Auto,
		&mut crate::hooks::NoHooks,
	)
	.into_result()
}

/// One tool call end to end, with the adapter-layer hooks of
/// [`crate::hooks`] (plan §4).
///
/// Same pipeline as [`execute`] plus three interception points:
///
/// * [`crate::hooks::BeforeCommit`] — the confirmation gate. The tool's
///   [`crate::hooks::OperationClass`] and the [`crate::hooks::Policy`]
///   decide whether, where and whether the gate is asked at all (reads are
///   always `auto`; `ProposalOnly` never enters the backend).
/// * [`crate::hooks::RenderDone`] — the readback assertion, run after a
///   frame-returning tool returned and before its undo group commits; a
///   failed assertion aborts the group (`READBACK_ASSERTION_FAILED`).
/// * [`crate::hooks::EditFailed`] — structured feedback for every failure,
///   called after the abort.
///
/// A tool that has no entry in [`crate::hooks::TOOL_CLASSES`] is refused
/// with `UNCLASSIFIED_TOOL` (fail-closed: a new tool must be graded).
pub fn execute_with(
	backend: &mut dyn EngineBackend,
	tool: &str,
	args: Value,
	policy: crate::hooks::Policy,
	hooks: &mut dyn crate::hooks::Hooks,
) -> crate::hooks::GatedOutcome {
	use crate::hooks::{GatedOutcome, ToolCall};

	let Some(def) = find(tool) else {
		let error = ToolError::new(CODE_UNKNOWN_TOOL, format!("unknown tool '{tool}'"))
			.with_tool(tool)
			.with_step("lookup");
		return GatedOutcome::failure(hooks, tool, None, error);
	};
	let Some(class) = crate::hooks::class_of(tool) else {
		let error = ToolError::new(
			crate::hooks::CODE_UNCLASSIFIED_TOOL,
			format!("tool '{tool}' has no entry in the hooks grading table"),
		)
		.with_tool(tool)
		.with_step("lookup");
		return GatedOutcome::failure(hooks, tool, None, error);
	};
	if let Err(error) = validate(tool, &args) {
		return GatedOutcome::failure(hooks, tool, Some(class), error);
	}

	let call = ToolCall {
		tool,
		args: &args,
		class,
	};
	if def.transactional {
		crate::hooks::gated_transactional(backend, &call, policy, hooks, |ops| {
			(def.call)(ops, &args)
		})
	} else {
		crate::hooks::gated_plain(backend, &call, policy, hooks, |ops| (def.call)(ops, &args))
	}
}

// ---------------------------------------------------------------------------
// v2 face (T9.2): tools added after the frozen 22
// ---------------------------------------------------------------------------

/// v2 backend entry point: same shape as [`BackendCall`] but on the
/// extension face.
pub type ExtCall = fn(&mut dyn crate::backend::EngineExt, &Value) -> Result<Value, ToolError>;

/// One v2 tool bound to an [`EngineExt`](crate::backend::EngineExt) method.
pub struct V2ToolDef {
	pub name: &'static str,
	/// Whether the call runs inside one undo group (`sequence.get_settings`
	/// is a read; `sequence.set_format` pushes its own undo row).
	pub transactional: bool,
	pub call: ExtCall,
}

/// The v2 registry: parse, then forward to the extension method.
pub fn v2_registry() -> &'static [V2ToolDef] {
	static REGISTRY: [V2ToolDef; 2] = [
		V2ToolDef {
			name: "sequence.get_settings",
			transactional: true,
			call: |backend, args| {
				tools::v2::sequence::parse_get_settings(args)?;
				backend.sequence_get_settings(args.clone())
			},
		},
		V2ToolDef {
			name: "sequence.set_format",
			transactional: true,
			call: |backend, args| {
				tools::v2::sequence::parse_set_format(args)?;
				backend.sequence_set_format(args.clone())
			},
		},
	];

	&REGISTRY
}

/// Looks a v2 tool up by name.
pub fn find_v2(tool: &str) -> Option<&'static V2ToolDef> {
	v2_registry().iter().find(|def| def.name == tool)
}

/// Executes a v2 tool with the same gate/transaction/hook flow the frozen
/// 22 use ([`execute_with`]), but on the extension face.
///
/// The class comes from [`crate::hooks::class_of_v2`] (fail-closed, like
/// the frozen table); a backend that does not implement the method returns
/// its default `UNSUPPORTED` with a gap id, which the gate records as a
/// structured failure rather than a silent success.
pub fn execute_v2_with(
	backend: &mut dyn crate::backend::EngineExt,
	tool: &str,
	args: Value,
	policy: crate::hooks::Policy,
	hooks: &mut dyn crate::hooks::Hooks,
) -> crate::hooks::GatedOutcome {
	use crate::hooks::{GatedOutcome, ToolCall};

	let Some(def) = find_v2(tool) else {
		let error = ToolError::new(CODE_UNKNOWN_TOOL, format!("unknown v2 tool '{tool}'"))
			.with_tool(tool)
			.with_step("lookup");
		return GatedOutcome::failure(hooks, tool, None, error);
	};
	let Some(class) = crate::hooks::class_of_v2(tool) else {
		let error = ToolError::new(
			crate::hooks::CODE_UNCLASSIFIED_TOOL,
			format!("v2 tool '{tool}' has no entry in the v2 grading table"),
		)
		.with_tool(tool)
		.with_step("lookup");
		return GatedOutcome::failure(hooks, tool, None, error);
	};

	// Runtime schema gate, the same position the frozen face's `validate`
	// takes: before the transaction opens and before any backend call.
	if let Err(error) = crate::schema_v2::validate(tool, &args) {
		return GatedOutcome::failure(hooks, tool, Some(class), error.with_tool(tool));
	}
	let call = ToolCall {
		tool,
		args: &args,
		class,
	};
	if def.transactional {
		crate::hooks::gated_transactional(backend, &call, policy, hooks, |ops| {
			(def.call)(ops, &args)
		})
	} else {
		crate::hooks::gated_plain(backend, &call, policy, hooks, |ops| (def.call)(ops, &args))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn registry_has_22_tools() {
		assert_eq!(registry().len(), 22);
		let mut names = tool_names();
		names.sort_unstable();
		let mut unique = names.clone();
		unique.dedup();
		assert_eq!(names.len(), unique.len(), "duplicate tool names");
	}

	#[test]
	fn every_tool_has_a_compiled_schema() {
		for def in registry() {
			assert!(
				default_schemas().get(def.name).is_some(),
				"no schema for {}",
				def.name
			);
		}
	}

	#[test]
	fn unknown_tool_is_rejected_before_any_backend_call() {
		let mut backend = crate::mock::MockBackend::new();
		let err = execute(&mut backend, "timeline.teleport", json!({})).unwrap_err();
		assert_eq!(err.code, CODE_UNKNOWN_TOOL);
		assert_eq!(err.step.as_deref(), Some("lookup"));
		assert!(err.txn.is_none());
	}

	#[test]
	fn schema_failure_never_reaches_the_backend() {
		let mut backend = crate::mock::MockBackend::new();
		let err = execute(
			&mut backend,
			"timeline.split_clip",
			serde_json::json!({"clip": "clip-1", "time": 1.5}),
		)
		.unwrap_err();
		assert_eq!(err.code, crate::error::CODE_SCHEMA_VALIDATION);
		assert_eq!(err.step.as_deref(), Some("validate"));
		// No project was opened, no transaction was started.
		assert!(!backend.can_undo());
	}
}

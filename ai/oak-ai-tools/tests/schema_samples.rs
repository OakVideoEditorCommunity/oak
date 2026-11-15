//! Frozen sample corpus gate: every instance under
//! `ai/schema/samples/valid/*` must pass the runtime schema validation,
//! and every instance under `ai/schema/samples/invalid/*` must be
//! rejected — before any backend is entered.
//!
//! Paths resolve through `CARGO_MANIFEST_DIR` (no machine-local absolute
//! paths). Sample file names carry the tool: `tool.json` or
//! `tool__variant.json`.

use std::path::{Path, PathBuf};

use oak_ai_tools::dispatch;
use oak_ai_tools::error::CODE_SCHEMA_VALIDATION;

fn samples_dir(kind: &str) -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ai/schema/samples").join(kind)
}

/// Tool name from a sample file stem: `timeline.split_clip__bare-float`
/// → `timeline.split_clip`.
fn tool_from_stem(stem: &str) -> &str {
	stem.split("__").next().unwrap_or(stem)
}

fn sample_files(kind: &str) -> Vec<(String, PathBuf)> {
	let dir = samples_dir(kind);
	let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
		.unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()))
		.map(|entry| entry.expect("sample entry").path())
		.filter(|path| path.extension().is_some_and(|ext| ext == "json"))
		.map(|path| {
			let stem = path.file_stem().expect("stem").to_string_lossy().to_string();
			(stem, path)
		})
		.collect();
	files.sort();
	files
}

#[test]
fn valid_corpus_is_27_and_all_pass() {
	let files = sample_files("valid");
	assert_eq!(files.len(), 27, "expected 27 valid samples, found {:?}", files);
	let registry = dispatch::default_schemas();
	for (stem, path) in &files {
		let tool = tool_from_stem(stem);
		let text = std::fs::read_to_string(path)
			.unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
		let args: serde_json::Value = serde_json::from_str(&text)
			.unwrap_or_else(|err| panic!("{stem} is not valid JSON: {err}"));
		// The sample must name a frozen tool.
		assert!(
			registry.get(tool).is_some(),
			"sample {stem} names unknown tool '{tool}'"
		);
		// Runtime validation must accept it.
		dispatch::validate(tool, &args)
			.unwrap_or_else(|err| panic!("valid sample {stem} rejected: {err:?}"));
	}
}

#[test]
fn invalid_corpus_is_13_and_all_rejected_with_schema_code() {
	let files = sample_files("invalid");
	assert_eq!(files.len(), 13, "expected 13 invalid samples, found {:?}", files);
	for (stem, path) in &files {
		let tool = tool_from_stem(stem);
		let text = std::fs::read_to_string(path)
			.unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
		let args: serde_json::Value = serde_json::from_str(&text)
			.unwrap_or_else(|err| panic!("{stem} is not valid JSON: {err}"));
		let err = match dispatch::validate(tool, &args) {
			Ok(()) => panic!("invalid sample {stem} was accepted"),
			Err(err) => err,
		};
		assert_eq!(
			err.code, CODE_SCHEMA_VALIDATION,
			"invalid sample {stem} must fail with {CODE_SCHEMA_VALIDATION}, got {:?}",
			err.code
		);
	}
}

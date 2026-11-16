use super::*;
use crate::dispatch::execute;
use serde_json::json;

/// Two real files on disk (the mock only checks existence).
fn temp_file(tag: &str) -> String {
	let path = std::env::temp_dir().join(format!("oak_mock_{tag}.mp4"));
	std::fs::write(&path, b"mock media fixture").expect("write fixture");
	path.to_string_lossy().to_string()
}

#[test]
fn new_backend_has_no_project() {
	let mut backend = MockBackend::new();
	let err = backend.project_get_overview(&json!({})).unwrap_err();
	assert_eq!(err.code, CODE_STATE);
	assert!(!backend.can_undo());
}

#[test]
fn open_save_overview_round_trip() {
	let mut backend = MockBackend::new();
	let out = execute(
		&mut backend,
		"project.open",
		json!({"fps": 30, "name": "cut"}),
	)
	.expect("open");
	assert_eq!(out["created"], true);
	assert_eq!(out["project"]["fps"]["num"], 30);
	let overview = execute(&mut backend, "project.get_overview", json!({})).expect("overview");
	assert_eq!(overview["sequences"].as_array().unwrap().len(), 1);
	assert_eq!(
		overview["sequences"][0]["tracks"].as_array().unwrap().len(),
		2
	);
	let saved = execute(
		&mut backend,
		"project.save",
		json!({"path": "/tmp/oak_mock_roundtrip.ove"}),
	)
	.expect("save");
	assert_eq!(saved["saved"], true);
}

#[test]
fn import_requires_existing_files_and_reports_per_path() {
	let mut backend = MockBackend::new();
	execute(&mut backend, "project.open", json!({})).expect("open");
	let good = temp_file("import_ok");
	let out = execute(
		&mut backend,
		"media.import_footage",
		json!({"paths": [good, "/no/such/file.mp4"]}),
	)
	.expect("import");
	assert_eq!(out["footage"].as_array().unwrap().len(), 1);
	assert_eq!(out["failed"].as_array().unwrap().len(), 1);
	assert!(out["footage"][0]["synthetic"] == true);
	let _ = std::fs::remove_file(&good);
}

#[test]
fn unknown_tool_and_schema_failures_stay_structured() {
	let mut backend = MockBackend::new();
	let err = execute(&mut backend, "nope.nope", json!({})).unwrap_err();
	assert_eq!(err.tool.as_deref(), Some("nope.nope"));
	assert_eq!(err.step.as_deref(), Some("lookup"));
	let err = execute(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": "clip-1", "time": 1.5}),
	)
	.unwrap_err();
	assert_eq!(err.step.as_deref(), Some("validate"));
	assert!(
		err.txn.is_none(),
		"validation failures start no transaction"
	);
}

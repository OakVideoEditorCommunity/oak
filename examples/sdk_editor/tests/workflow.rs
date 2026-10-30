// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! P8 acceptance: a consumer with only `quercus-engine-rs` completes the
//! whole editing workflow (import → edit → effect/keyframe → preview/audio →
//! undo/redo → save/reopen → export).

use std::sync::Mutex;

static ENGINE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn headless_workflow_completes_on_the_public_sdk() {
	let _guard = ENGINE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
	let work_dir = std::env::temp_dir().join(format!(
		"oak-sdk-editor-test-{}",
		std::process::id()
	));
	let report = sdk_editor::run(&work_dir).expect("the SDK workflow completes");

	assert!(report.explorer_entries >= 3, "project tree has root/sequence/footage");
	assert_eq!(report.keyframes, 2);
	assert!(report.reopened_clips > 0, "the reopened project keeps its clips");
	assert!(report.preview_bytes > 0);
	assert!(report.audio_samples > 0);
	assert!(report.audio_peak > 0.0, "the test clip's tone is audible");
	assert!(report.export_bytes > 0);
	assert!(report.export_path.is_file());
	assert!(report.undo_rows > 0);
	let _ = std::fs::remove_dir_all(work_dir);
}

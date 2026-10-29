// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Headless SDK workflow by default; `--gui` opens the dependency-free
//! terminal shell driving the same SDK session.

fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let gui = args.iter().any(|arg| arg == "--gui");
	let work_dir = args
		.iter()
		.find(|arg| !arg.starts_with("--"))
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| {
			std::env::temp_dir().join(format!("oak-sdk-editor-{}", std::process::id()))
		});

	if gui {
		if let Err(error) = sdk_editor::shell::run(&work_dir) {
			eprintln!("sdk_editor gui: {error}");
			std::process::exit(1);
		}
		return;
	}

	match sdk_editor::run(&work_dir) {
		Ok(report) => {
			println!("{}", sdk_editor::summary(&report));
		}
		Err(error) => {
			eprintln!("sdk_editor: {error}");
			std::process::exit(1);
		}
	}
}

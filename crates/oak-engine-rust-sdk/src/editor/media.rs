//! Media lifecycle integration tests for replacement, rename and deletion.

#[cfg(test)]
mod tests {
	use crate::edit::EditCommand;
	use crate::editor::EditorSession;

	#[test]
	fn replace_footage_preserves_identity_and_supports_undo_and_reload() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let media_a = crate::test_support::make_test_clip("replace_a");
		let media_b = crate::test_support::make_test_clip("replace_b");
		let footage = session.project().import_footage(&media_a).expect("footage");
		session
			.execute(EditCommand::ReplaceFootage {
				footage,
				path: media_b.clone(),
			})
			.expect("replace");
		let snapshot = session.snapshot();
		let current = snapshot.footages.iter().find(|item| item.id == footage).expect("footage row");
		assert_eq!(current.filename.as_deref(), Some(media_b.as_str()));
		assert!(current.video_streams > 0);
		assert!(crate::node::undo());
		assert_eq!(
			session.snapshot().footages[0].filename.as_deref(),
			Some(media_a.as_str())
		);
		assert!(crate::node::redo());
		assert_eq!(
			session.snapshot().footages[0].filename.as_deref(),
			Some(media_b.as_str())
		);
		assert!(session
			.execute(EditCommand::ReplaceFootage {
				footage,
				path: "/nonexistent/replacement.mp4".into(),
			})
			.is_err());
		assert_eq!(
			session.snapshot().footages[0].filename.as_deref(),
			Some(media_b.as_str()),
			"failed probe must not change the current media"
		);

		let path = std::env::temp_dir()
			.join(format!("oak_p4_replace_{}.ove", std::process::id()))
			.to_string_lossy()
			.into_owned();
		session.save(&path).expect("save");
		let opened = EditorSession::open(&path).expect("reload");
		let reloaded = opened.snapshot();
		assert_eq!(reloaded.footages[0].id, footage);
		assert_eq!(reloaded.footages[0].filename.as_deref(), Some(media_b.as_str()));
		let _ = std::fs::remove_file(path);
		let _ = std::fs::remove_file(media_a);
		let _ = std::fs::remove_file(media_b);
	}

	#[test]
	fn project_entry_rename_and_delete_use_semantic_commands() {
		let _guard = crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let session = EditorSession::new().expect("session");
		let media = crate::test_support::make_test_clip("entry_delete");
		let footage = session.project().import_footage(&media).expect("footage");
		session
			.execute(EditCommand::RenameBlocks {
				clips: vec![footage],
				name: "After".into(),
			})
			.expect("rename");
		assert_eq!(session.snapshot().root.children[0].name, "After");
		session
			.execute(EditCommand::DeleteEntry { node: footage })
			.expect("delete");
		assert!(session.snapshot().root.children.is_empty());
		assert!(session.project().node_kind(footage).is_none());
		assert!(crate::node::undo());
		assert!(session.project().node_kind(footage).is_some());
		assert_eq!(session.snapshot().root.children[0].name, "After");
		let _ = std::fs::remove_file(media);
	}
}

//! Shared command-test fixtures and media/project validation scenarios.

use super::*;


	/// Missing and undecodable media are rejected before any node lands.
	#[test]
	pub(crate) fn import_footage_rejects_missing_and_corrupt_media() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let missing = temp_path("import_missing", "mp4");
		let _ = std::fs::remove_file(&missing);
		assert!(import_footage(&project, &missing).is_err());
		let junk = temp_path("import_corrupt", "mp4");
		std::fs::write(&junk, b"definitely not a video").unwrap();
		assert!(
			import_footage(&project, &junk).is_err(),
			"the probe rejects non-media"
		);
		assert_eq!(footage_ids(&lock(&project)).len(), 0, "no orphan landed");
		let _ = std::fs::remove_file(&junk);
	}

	/// `reprobe_unprobed_footage` resolves absolute and (project-file
	/// relative) paths, skips missing files and leaves probed footage alone.
	#[test]
	pub(crate) fn reprobe_unprobed_footage_resolves_and_probes() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let media_path = media_file("reprobe");
		let project = create_project();
		let add_footage = |filename: &str| -> NodeId {
			let mut g = lock(&project);
			let (core, behavior) = FootageBehavior::create();
			let id = g.graph.add_node(core, behavior);
			if let Some(f) = g
				.graph
				.get_mut(id)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<FootageBehavior>())
			{
				f.filename = filename.to_string();
			}
			id
		};

		// No project filename yet: the relative path cannot resolve.
		let relative = add_footage("relative_gap.mp4");
		let absolute = add_footage(&media_path.to_string_lossy());
		let missing = add_footage("/nonexistent/oak_gap_media.mp4");
		reprobe_unprobed_footage(&project);
		{
			let g = lock(&project);
			assert!(
				footage_behavior(&g.graph, relative)
					.map(|f| f.streams.is_empty())
					.unwrap_or(false),
				"a relative path without a project dir is left alone"
			);
			assert!(
				!footage_behavior(&g.graph, absolute)
					.map(|f| f.streams.is_empty())
					.unwrap_or(true),
				"an absolute path probes"
			);
			assert!(
				footage_behavior(&g.graph, missing)
					.map(|f| f.streams.is_empty())
					.unwrap_or(false),
				"a missing file is left unprobed"
			);
			assert!(footage_duration_seconds(&g.graph, missing).is_none());
			assert!(footage_duration_seconds(&g.graph, project_root(&g)).is_none());
			assert!(footage_duration_seconds(&g.graph, absolute).is_some());
		}

		// With a project file, a relative name resolves against its dir.
		let project_path = temp_path("reprobe_project", "ove");
		save_ove(&project, &project_path).expect("save project");
		let media_name = media_path.file_name().unwrap().to_string_lossy().into_owned();
		let relative_ok = add_footage(&media_name);
		reprobe_unprobed_footage(&project);
		assert!(
			!footage_behavior(&lock(&project).graph, relative_ok)
				.map(|f| f.streams.is_empty())
				.unwrap_or(true),
			"the relative path resolved against the project dir and probed"
		);

		let _ = std::fs::remove_file(&media_path);
		let _ = std::fs::remove_file(&project_path);
		oak_undo::global::clear().unwrap();
	}

	pub(crate) fn project_root(g: &Project) -> NodeId {
		g.root
	}

	/// Marker-list and workarea handles: null handles are safe, live handles
	/// round-trip, and the undoable setters restore their previous state.
	#[test]
	pub(crate) fn marker_and_workarea_handle_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();

		let null = CHandle::default();
		assert!(markers_of(&null).is_empty());
		assert_eq!(marker_index_at(&null, Rational::new(1, 1)), None);
		assert_eq!(workarea_state(&null), None);
		workarea_set(
			&null,
			true,
			TimeRange::new(Rational::new(0, 1), Rational::new(1, 1)),
		);
		let mut dead = CHandle::default();
		release_handle(&mut dead);

		let mut markers = marker_list_create();
		assert!(markers_of(&markers).is_empty());
		marker_add(&markers, Rational::new(2, 1), "M1", 2).expect("add marker");
		marker_add(&markers, Rational::new(5, 1), "M2", 4).expect("add marker 2");
		assert_eq!(markers_of(&markers).len(), 2);
		assert_eq!(marker_index_at(&markers, Rational::new(5, 1)), Some(1));
		assert_eq!(marker_index_at(&markers, Rational::new(9, 1)), None);
		assert!(
			marker_add(&markers, Rational::new(2, 1), "dup", 1).is_err(),
			"a duplicate timestamp is rejected"
		);
		assert!(
			marker_remove(&markers, Rational::new(9, 1)).is_err(),
			"no marker at that time"
		);
		marker_remove(&markers, Rational::new(2, 1)).expect("remove marker");
		assert_eq!(
			markers_of(&markers),
			vec![(Rational::new(5, 1), "M2".to_string(), 4)]
		);
		oak_undo::global::undo().unwrap();
		assert_eq!(markers_of(&markers).len(), 2);
		oak_undo::global::redo().unwrap();
		assert_eq!(markers_of(&markers).len(), 1);

		let mut wa = workarea_create();
		let range = TimeRange::new(Rational::new(1, 1), Rational::new(4, 1));
		workarea_set(&wa, true, range);
		assert_eq!(workarea_state(&wa), Some((true, range)));
		let new = TimeRange::new(Rational::new(2, 1), Rational::new(3, 1));
		workarea_set_undoable(&wa, false, new, range).expect("set workarea");
		assert_eq!(workarea_state(&wa), Some((false, new)));
		oak_undo::global::undo().unwrap();
		assert_eq!(workarea_state(&wa), Some((true, range)));
		oak_undo::global::redo().unwrap();
		assert_eq!(workarea_state(&wa), Some((false, new)));

		release_handle(&mut markers);
		release_handle(&mut wa);
		oak_undo::global::clear().unwrap();
	}

	/// The library setters validate their arguments before touching storage,
	/// and the list is empty (not an error) without a configured library.
	#[test]
	pub(crate) fn library_argument_validation_and_disabled_list() {
		let _g = test_lock();
		if !oak_storage::writethrough::storage_enabled() {
			assert!(library_list().expect("disabled list").is_empty());
			assert!(library().is_err(), "the library URI is unavailable");
		}
		assert!(library_create("   ").is_err(), "blank name rejected");
		assert!(library_delete("").is_err(), "blank uuid rejected");
		assert!(library_rename("", "x").is_err(), "blank uuid rejected");
		assert!(library_rename("u", "   ").is_err(), "blank name rejected");
		assert!(library_duplicate("").is_err(), "blank uuid rejected");
		assert!(
			library_export("", Path::new("out.ove")).is_err(),
			"blank uuid rejected"
		);
		assert!(library_open("").is_err(), "blank uuid rejected");
	}

	/// `connect_command` / `disconnect_command` validate nodes, inputs and
	/// existing edges; their closures connect and disconnect on redo/undo.
	#[test]
	pub(crate) fn connect_and_disconnect_commands_validate_and_undo() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("connect");
		let clip = place_clip(&project, seq, footage, 0, 10);
		let bogus = id_of(9_000_001).unwrap();
		let input = oak_node::block::clip_input::TEXTURE_INPUT;

		assert!(connect_command(&project, bogus, clip, input).is_err());
		assert!(connect_command(&project, footage, bogus, input).is_err());
		assert!(connect_command(&project, footage, clip, "no_such_input").is_err());
		assert!(
			connect_command(&project, clip, footage, "file_in").is_err(),
			"the footage's file_in text input is not connectable"
		);
		assert!(
			connect_command(&project, footage, clip, input).is_err(),
			"the texture input is already connected"
		);

		let clip2 = {
			let mut g = lock(&project);
			create_footage_clip(
				&mut g.graph,
				footage,
				Rational::new(0, 1),
				Rational::new(10, 25),
			)
		};
		let mut cmd = connect_command(&project, footage, clip2, input).expect("connect");
		cmd.redo_now();
		assert_eq!(
			lock(&project).graph.connected_output(clip2, input, -1),
			Some(footage)
		);
		cmd.undo_now();
		assert_eq!(
			lock(&project).graph.connected_output(clip2, input, -1),
			None
		);

		assert!(disconnect_command(&project, clip2, "no_such_input").is_err());
		lock(&project)
			.graph
			.connect(footage, clip2, input, -1)
			.expect("wire it back");
		let mut cmd = disconnect_command(&project, clip2, input).expect("disconnect");
		cmd.redo_now();
		assert_eq!(
			lock(&project).graph.connected_output(clip2, input, -1),
			None
		);
		cmd.undo_now();
		assert_eq!(
			lock(&project).graph.connected_output(clip2, input, -1),
			Some(footage),
			"the undo restores the captured edge"
		);

		// Constructed while nothing is connected: the undo's source is None
		// and both directions are safe no-ops.
		lock(&project).graph.disconnect_input(clip2, input, -1);
		let mut cmd = disconnect_command(&project, clip2, input).expect("disconnect");
		cmd.redo_now();
		cmd.undo_now();
		assert_eq!(
			lock(&project).graph.connected_output(clip2, input, -1),
			None
		);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// `set_context_position_command` creates an entry on redo, removes it on
	/// undo when it created it, and restores the previous slot otherwise.
	#[test]
	pub(crate) fn context_position_command_covers_create_update_and_remove() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("context_pos");
		let clip = place_clip(&project, seq, footage, 0, 10);
		let bogus = id_of(9_000_002).unwrap();
		assert!(set_context_position_command(&project, bogus, seq, 1.0, 2.0).is_err());
		assert!(set_context_position_command(&project, clip, bogus, 1.0, 2.0).is_err());

		let positions = |p: &ProjectRef, node: NodeId| -> Vec<(NodeId, (f64, f64), bool)> {
			lock(p)
				.graph
				.get(node)
				.map(|e| e.core.context_positions.clone())
				.unwrap_or_default()
		};

		let mut first =
			set_context_position_command(&project, clip, seq, 5.0, 6.0).expect("first set");
		first.redo_now();
		assert_eq!(positions(&project, clip), vec![(seq, (5.0, 6.0), false)]);
		first.undo_now();
		assert!(
			positions(&project, clip).is_empty(),
			"the created entry is removed on undo"
		);

		let mut create = set_context_position_command(&project, clip, seq, 5.0, 6.0).unwrap();
		create.redo_now();
		let mut update =
			set_context_position_command(&project, clip, seq, 30.0, 40.0).expect("update");
		update.redo_now();
		assert_eq!(positions(&project, clip), vec![(seq, (30.0, 40.0), false)]);
		update.undo_now();
		assert_eq!(
			positions(&project, clip),
			vec![(seq, (5.0, 6.0), false)],
			"the previous position returns"
		);

		let _ = std::fs::remove_file(&media);
	}

	/// Borrow helpers and node queries answer `None`/empty for stale ids and
	/// for behaviors that do not expose `as_any`.
	#[test]
	pub(crate) fn behavior_borrows_and_queries_on_missing_nodes() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let root = lock(&project).root;
		let seq = create_sequence(&project, "Borrows");
		let opaque = add_opaque(&project);
		let bogus = id_of(9_000_003).unwrap();
		let g = lock(&project);

		assert!(is_folder(&g.graph, root));
		assert!(!is_folder(&g.graph, seq));
		assert!(!is_folder(&g.graph, opaque));
		assert!(!is_folder(&g.graph, bogus));

		assert!(sequence_behavior(&g.graph, root).is_none());
		assert!(sequence_behavior(&g.graph, opaque).is_none());
		assert!(sequence_behavior(&g.graph, seq).is_some());
		assert!(track_list_behavior(&g.graph, bogus).is_none());
		assert!(track_behavior(&g.graph, bogus).is_none());
		assert!(clip_behavior(&g.graph, bogus).is_none());
		assert!(footage_behavior(&g.graph, bogus).is_none());
		assert!(clip_behavior(&g.graph, opaque).is_none());
		assert!(footage_behavior(&g.graph, opaque).is_none());
		assert_eq!(node_label(&g.graph, bogus), "");
		assert_eq!(node_type_id(&g.graph, opaque), "test.opaque");
		assert_eq!(node_type_id(&g.graph, bogus), "");
		assert!(multicam_nodes(&g.graph).is_empty());
		assert!(sequence_video_params(&g.graph, bogus).is_none());
		assert!(sequence_time_base(&g.graph, bogus).is_none());
		assert_eq!(sequence_length(&g.graph, bogus), Rational::new(0, 1));
		assert_eq!(sequence_playhead(&g.graph, bogus), Rational::new(0, 1));
		assert!(track_list_of(&g.graph, bogus, TrackType::Video).is_none());
		assert!(track_ids(&g.graph, bogus, TrackType::Video).is_empty());
		assert!(clip_ids(&g.graph, bogus).is_empty());
		assert!(clip_range(&g.graph, bogus).is_none());
		assert!(clip_track(&g.graph, bogus).is_none());
		assert!(find_input_footage(&g.graph, bogus).is_none());
		assert!(clip_media_filename(&g.graph, bogus).is_none());
		assert!(block_core_of(&g.graph, opaque).is_none());
		assert!(block_core_of(&g.graph, seq).is_none());
		assert!(!is_timeline_clip(&g.graph, bogus));
		assert!(!is_timeline_clip(&g.graph, opaque));
		drop(g);

		// `is_timeline_clip` filters opaque entries out of a track's blocks.
		let track = video_track_of(&project, seq);
		{
			let mut g = lock(&project);
			if let Some(t) = g
				.graph
				.get_mut(track)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackBehavior>())
			{
				t.blocks.push(opaque);
			}
		}
		assert!(clip_ids(&lock(&project).graph, track).is_empty());

		// The mutable core helpers reject stale and non-block nodes.
		let bogus2 = id_of(9_000_004).unwrap();
		let mut g = lock(&project);
		assert!(!with_block_core_mut(&mut g.graph, bogus2, |_| {}));
		assert!(!with_block_core_mut(&mut g.graph, opaque, |_| {}));
		oak_undo::global::clear().unwrap();
	}

	/// `sequence_length` skips dangling track-list / track / block references
	/// and non-block entries instead of failing.
	#[test]
	pub(crate) fn sequence_length_skips_dangling_and_non_block_entries() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("seq_len");
		let clip = place_clip(&project, seq, footage, 0, 10);
		let list = video_list_of(&project, seq);
		let track = video_track_of(&project, seq);
		let bogus = id_of(9_000_005).unwrap();
		let opaque = add_opaque(&project);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let expected = ts_to_rational(10, tb);
		assert_eq!(sequence_length(&lock(&project).graph, seq), expected);

		{
			let mut g = lock(&project);
			if let Some(s) = g
				.graph
				.get_mut(seq)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<SequenceBehavior>())
			{
				s.track_lists.push(bogus);
			}
			if let Some(l) = g
				.graph
				.get_mut(list)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackListBehavior>())
			{
				l.tracks.push(bogus);
			}
			if let Some(t) = g
				.graph
				.get_mut(track)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackBehavior>())
			{
				t.blocks.push(bogus);
				t.blocks.push(opaque);
			}
		}
		let g = lock(&project);
		assert_eq!(sequence_length(&g.graph, seq), expected);
		assert_eq!(
			clip_range(&g.graph, clip),
			Some((Rational::new(0, 1), expected, Rational::new(0, 1)))
		);
		drop(g);
		let _ = std::fs::remove_file(&media);
	}


//! Validation matrices for transitions, links and composite editing commands.

use super::*;
	#[test]

	/// The transition seam/edge commands and the default-transition action
	/// cover their whole validation matrix plus the wedge clamp.
	pub(crate) fn transition_error_matrix_and_edge_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("transitions");
		let track = video_track_of(&project, seq);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let half = ts_to_rational(2, tb);
		let bogus = id_of(9_000_012).unwrap();
		let a = place_clip(&project, seq, footage, 0, 10);
		let b = place_clip(&project, seq, footage, 10, 20);

		// The seam command creates, undoes and redoes.
		let t = add_transition_at_seam(&project, a, b, half).expect("seam transition");
		assert_eq!(transition_of_clip(&lock(&project).graph, b, true), Some(t));
		oak_undo::global::undo().unwrap();
		assert!(transition_of_clip(&lock(&project).graph, b, true).is_none());
		oak_undo::global::redo().unwrap();
		assert_eq!(transition_of_clip(&lock(&project).graph, b, true), Some(t));
		assert!(
			add_transition_at_seam(&project, a, b, half).is_err(),
			"the seam already carries a transition"
		);

		// Validation errors: length, non-clips, non-contiguous, off-track
		// outgoing clip, mixed tracks and a track that is not a track.
		assert!(add_transition_at_seam(&project, a, b, Rational::new(0, 1)).is_err());
		assert!(add_transition_at_seam(&project, footage, b, half).is_err());
		assert!(add_transition_at_seam(&project, a, footage, half).is_err());
		let c = place_clip(&project, seq, footage, 100, 110);
		let d = place_clip(&project, seq, footage, 200, 210);
		assert!(
			add_transition_at_seam(&project, c, d, half).is_err(),
			"the clips are not contiguous"
		);
		let e = place_clip(&project, seq, footage, 300, 310);
		let f = place_clip(&project, seq, footage, 310, 320);
		oak_timeline::util::track_ripple_remove_block(
			&node_ref(&project, track),
			&node_ref(&project, e),
		);
		assert!(
			add_transition_at_seam(&project, e, f, half).is_err(),
			"the outgoing clip is off-track"
		);
		let index = add_track(&project, seq, TrackType::Video).expect("third video track");
		let track2 = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)[index]
		};
		let g1 = place_clip(&project, seq, footage, 400, 410);
		let h = place_footage_clip(&project, seq, footage, TrackType::Video, index, 400, 410, 0)
			.expect("clip on the second track");
		assert!(
			add_transition_at_seam(&project, g1, h, half).is_err(),
			"the clips are on different tracks"
		);
		let folder = create_folder(&project, "Not A Track").expect("folder");
		set_block_track(&project, g1, Some(folder));
		set_block_track(&project, h, Some(folder));
		assert!(
			add_transition_at_seam(&project, g1, h, half).is_err(),
			"the named track is not in the project"
		);

		// Non-adjacent blocks with touching ranges.
		let i = place_clip(&project, seq, footage, 500, 510);
		let j = place_clip(&project, seq, footage, 510, 520);
		let opaque = add_opaque(&project);
		{
			let mut g = lock(&project);
			if let Some(t) = g
				.graph
				.get_mut(track)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackBehavior>())
			{
				let position = t
					.blocks
					.iter()
					.position(|&block| block == i)
					.expect("i on the track");
				t.blocks.insert(position + 1, opaque);
			}
		}
		assert!(
			add_transition_at_seam(&project, i, j, half).is_err(),
			"the clips are not adjacent on the track"
		);

		// Edge transitions: length, non-clip, stale, off-track, a clip that
		// names a track it is not on, and an already-transitioned edge.
		assert!(add_transition_at_edge(&project, a, true, Rational::new(0, 1)).is_err());
		assert!(add_transition_at_edge(&project, footage, true, half).is_err());
		assert!(add_transition_at_edge(&project, bogus, true, half).is_err());
		assert!(add_transition_at_edge(&project, e, true, half).is_err());
		set_block_track(&project, i, Some(track2));
		assert!(add_transition_at_edge(&project, i, true, half).is_err());
		set_block_track(&project, i, Some(track));
		assert!(add_transition_at_edge(&project, b, true, half).is_err());

		// A lone clip takes a head and a tail transition; the length clamp
		// bottoms out at one frame, and the wedge map reports both sides.
		let lone = place_clip(&project, seq, footage, 600, 610);
		let head = add_transition_at_edge(&project, lone, true, half).expect("head transition");
		assert_eq!(transition_of_clip(&lock(&project).graph, lone, true), Some(head));
		assert!(add_transition_at_edge(&project, lone, true, half).is_err());
		let tail = add_transition_at_edge(&project, lone, false, half).expect("tail transition");
		assert_eq!(transition_of_clip(&lock(&project).graph, lone, false), Some(tail));
		let frame = ts_to_rational(1, tb);
		let applied = set_transition_length(&project, lone, true, frame, Rational::new(0, 1))
			.expect("clamp to one frame");
		assert_eq!(applied, frame);
		assert!(set_transition_length(&project, bogus, true, frame, half).is_err());
		assert!(clip_transition_widths(&lock(&project).graph, bogus).is_empty());
		{
			let g = lock(&project);
			let widths = clip_transition_widths(&g.graph, track);
			let lone_widths = widths.get(&lone).expect("lone clip widths");
			assert!(lone_widths.0.is_some(), "the head wedge");
			assert!(lone_widths.1.is_some(), "the tail wedge");
		}

		// The default-transition batch skips everything it cannot use.
		assert!(
			add_default_transition(&project, bogus, &[lone], half).is_err(),
			"the sequence is not in the project"
		);
		let opaque2 = add_opaque(&project);
		assert!(
			add_default_transition(&project, seq, &[opaque2, footage], half).is_err(),
			"opaque and trackless entries contribute nothing"
		);
		let other = create_sequence(&project, "Other Seq");
		let other_clip = place_clip(&project, other, footage, 0, 10);
		assert!(
			add_default_transition(&project, seq, &[other_clip], half).is_err(),
			"a clip of another sequence is ignored"
		);
		let lone2 = place_clip(&project, seq, footage, 700, 710);
		let lone2_track = {
			let g = lock(&project);
			clip_track(&g.graph, lone2).expect("lone2 on a track")
		};
		let saved_list = {
			let g = lock(&project);
			track_behavior(&g.graph, lone2_track).and_then(|t| t.track_list)
		};
		set_track_list(&project, lone2_track, None);
		assert!(
			add_default_transition(&project, seq, &[lone2], half).is_err(),
			"a listless track is skipped"
		);
		set_track_list(&project, lone2_track, saved_list);
		{
			let mut g = lock(&project);
			if let Some(t) = g
				.graph
				.get_mut(lone2_track)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackBehavior>())
			{
				t.blocks.retain(|&block| block != lone2);
			}
		}
		assert!(
			add_default_transition(&project, seq, &[lone2], half).is_err(),
			"a clip missing from its track is skipped"
		);
		let adj = create_adjustment_layer(&project, seq, track, 800, 820).expect("adjustment");
		assert!(
			add_default_transition(&project, seq, &[adj], half).is_err(),
			"an adjustment layer is not a clip"
		);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// `remove_node` is undoable and an empty multi command leaves no row.
	#[test]
	pub(crate) fn remove_node_and_empty_multi_command_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("remove_node");
		let _a = place_clip(&project, seq, footage, 0, 10);
		let b = place_clip(&project, seq, footage, 10, 20);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let before = oak_undo::global::count().unwrap();
		push_multi_command(Vec::new(), "Empty Multi").expect("an empty multi is a no-op");
		assert_eq!(oak_undo::global::count().unwrap(), before, "no empty row");

		remove_node(&project, b).expect("remove node");
		assert!(!lock(&project).graph.is_valid(b));
		oak_undo::global::undo().unwrap();
		assert!(lock(&project).graph.is_valid(b));
		assert_eq!(
			clip_range(&lock(&project).graph, b).unwrap().0,
			ts_to_rational(10, tb)
		);
		oak_undo::global::redo().unwrap();
		assert!(!lock(&project).graph.is_valid(b));

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// Rational <-> timestamp conversion: rounding, invalid timebases and
	/// the null sentinel.
	#[test]
	pub(crate) fn timestamp_conversion_edge_cases() {
		let _g = test_lock();
		// Round half away from zero.
		assert_eq!(rational_to_ts(Rational::new(-1, 2), (1, 1)), -1);
		assert_eq!(rational_to_ts(Rational::new(-1, 3), (1, 1)), 0);
		assert_eq!(rational_to_ts(Rational::new(1, 2), (1, 1)), 1);
		assert_eq!(rational_to_ts(Rational::new(1, 3), (1, 1)), 0);
		// Degenerate timebases and the null rational answer zero.
		assert_eq!(rational_to_ts(Rational::new(1, 2), (0, 1)), 0);
		assert_eq!(rational_to_ts(Rational::new(1, 2), (1, 0)), 0);
		assert_eq!(rational_to_ts(Rational::NULL, (1, 1)), 0);
		// Timestamp -> reduced rational.
		assert_eq!(ts_to_rational(2, (1, 25)), Rational::new(2, 25));
		assert_eq!(ts_to_rational(0, (1001, 30000)), Rational::new(0, 1));
		assert_eq!(
			ts_to_rational(30000, (1001, 30000)),
			Rational::new(1001, 1)
		);
		// Whole-frame round trips are exact.
		let tb = (1001, 30000);
		for frame in [0i64, 1, 7, 300, -5] {
			assert_eq!(rational_to_ts(ts_to_rational(frame, tb), tb), frame);
		}
		// The identity -> NodeId bridge rejects the sentinel.
		assert!(id_of(999_999).is_some());
		assert!(id_of(NodeId::INVALID.identity()).is_none());
	}

	/// `find_input_footage` terminates on a cyclic input graph.
	#[test]
	pub(crate) fn find_input_footage_handles_cycles() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("cycle");
		let _ = seq;
		// A source whose output feeds TWO inputs of one node: the BFS meets
		// the source twice in the next frontier, and the second visit takes
		// the already-visited bail-out.
		let (source, keymix) = {
			let mut g = lock(&project);
			let source = create_footage_clip(
				&mut g.graph,
				footage,
				Rational::new(0, 1),
				Rational::new(1, 1),
			);
			let (core, behavior) = oak_node::factory::Factory::global()
				.create_any("org.olivevideoeditor.Olive.keymix")
				.expect("keymix is registered");
			let keymix = g.graph.add_node(core, behavior);
			(source, keymix)
		};
		lock(&project)
			.graph
			.connect(source, keymix, "tex_in", -1)
			.expect("source -> tex_in");
		lock(&project)
			.graph
			.connect(source, keymix, "mask_in", -1)
			.expect("source -> mask_in");
		assert!(find_input_footage(&lock(&project).graph, keymix).is_none());
		let _ = std::fs::remove_file(&media);
	}

	/// `set_clips_linked` links and unlinks the set as one undoable row and
	/// ignores a set with fewer than two blocks.
	#[test]
	pub(crate) fn set_clips_linked_links_and_unlinks_the_set() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("set_links");
		let a = place_clip(&project, seq, footage, 0, 10);
		let b = place_clip(&project, seq, footage, 10, 20);
		let before = oak_undo::global::count().unwrap();
		set_clips_linked(&project, &[a], true).expect("a single block is a no-op");
		assert_eq!(oak_undo::global::count().unwrap(), before);

		set_clips_linked(&project, &[a, b], true).expect("link the pair");
		assert!(lock(&project).graph.are_linked(a, b));
		oak_undo::global::undo().unwrap();
		assert!(!lock(&project).graph.are_linked(a, b));
		oak_undo::global::redo().unwrap();
		assert!(lock(&project).graph.are_linked(a, b));

		set_clips_linked(&project, &[a, b], false).expect("unlink the pair");
		assert!(!lock(&project).graph.are_linked(a, b));
		oak_undo::global::undo().unwrap();
		assert!(
			lock(&project).graph.are_linked(a, b),
			"the undo restores the prior topology"
		);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// A relative OVE path is resolved against the working directory and
	/// recorded absolute.
	#[test]
	pub(crate) fn load_ove_resolves_relative_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let _ = create_sequence(&project, "Relative");
		let name = "oak_graphops_gap_relative_load.ove";
		let absolute = std::env::current_dir()
			.expect("the test cwd")
			.join(name);
		save_ove(&project, &absolute).expect("save");
		let loaded = load_ove(Path::new(name)).expect("relative load");
		{
			let g = lock(&loaded);
			assert_eq!(sequence_ids(&g).len(), 1);
			assert!(
				Path::new(g.filename()).is_absolute(),
				"the filename is normalized to an absolute path"
			);
		}
		let _ = std::fs::remove_file(&absolute);
	}


//! Undo/redo topology, persistence and incremental-link regression coverage.

use super::*;


	/// Incremental link-undo regression: undoing the A/V drop removes ONLY
	/// the links the drop created — a link added afterwards (here: the
	/// video clip to a third clip) survives (the old undo restored the
	/// whole links vector from a construction-time snapshot and clobbered
	/// it).
	#[test]
	pub(crate) fn drop_undo_keeps_links_created_after_the_drop() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Drop Undo Links");
		let media = std::env::temp_dir().join(format!("oak_drop_undo_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate test media");
		let footage = import_footage(&project, &media).expect("import");
		let dropped = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			0,
			10,
			0,
		)
		.expect("linked placement");
		let (video, audio) = (dropped[0], dropped[1]);

		// A third clip, linked to the dropped video clip AFTER the drop.
		let third = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 20, 30, 0)
			.expect("place third");
		{
			let mut g = lock(&project);
			assert!(g.graph.link(video, third), "the third link is established");
		}

		// Undo twice: the third clip's placement, then the drop. Neither
		// may touch the video<->third link.
		oak_undo::global::undo().expect("undo the third placement");
		oak_undo::global::undo().expect("undo the drop");
		{
			let g = lock(&project);
			assert!(
				g.graph.links_of(video).contains(&third),
				"the later link survives the drop's undo"
			);
			assert!(g.graph.links_of(third).contains(&video));
			assert!(
				!g.graph.links_of(video).contains(&audio),
				"the drop's own link is removed"
			);
			assert!(!g.graph.links_of(audio).contains(&video));
		}
		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// Off-track linked clip regression: a linked clip that was
	/// ripple-removed from its track (track = None) is SKIPPED by a group
	/// move instead of failing the whole move — the on-track clip moves
	/// and the off-track one keeps its range.
	#[test]
	pub(crate) fn moving_a_linked_clip_skips_the_off_track_partner() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Off Track Move");
		let media = std::env::temp_dir().join(format!("oak_off_track_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate test media");
		let footage = import_footage(&project, &media).expect("import");
		let dropped = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			0,
			10,
			0,
		)
		.expect("linked placement");
		let (video, audio) = (dropped[0], dropped[1]);

		// Ripple-remove the audio clip from its track: the node stays in
		// the graph, its links intact, but it is off any track.
		let audio_track = {
			let g = lock(&project);
			clip_track(&g.graph, audio).expect("audio starts on a track")
		};
		oak_timeline::util::track_ripple_remove_block(
			&node_ref(&project, audio_track),
			&node_ref(&project, audio),
		);
		{
			let g = lock(&project);
			assert!(
				clip_track(&g.graph, audio).is_none(),
				"audio is off-track now"
			);
			assert!(g.graph.are_linked(video, audio), "the link itself survives");
		}

		// Moving the video clip succeeds and moves ONLY the on-track clip.
		move_clip_with_links(&project, video, None, 40, &[audio]).expect("the move succeeds");
		{
			let g = lock(&project);
			let tb = sequence_time_base(&g.graph, seq).expect("a valid frame rate");
			let (v_in, _, _) = clip_range(&g.graph, video).expect("video is a clip");
			assert_eq!(rational_to_ts(v_in, tb), 40, "the on-track clip moved");
			let (a_in, _, _) = clip_range(&g.graph, audio).expect("audio is a clip");
			assert_eq!(rational_to_ts(a_in, tb), 0, "the off-track clip stayed put");
		}
		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}


	use super::*;

	/// add_track undo/redo cycles must converge (the track count AND the
	/// created track's identity stay stable; the C++ remove-last undo must
	/// not eat a default track).
	#[test]
	pub(crate) fn add_track_undo_redo_cycles_converge() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Add Track Cycles");
		let count_of = |p: &ProjectRef, kind: TrackType| {
			let g = lock(p);
			track_ids(&g.graph, seq, kind).len()
		};
		assert_eq!(
			count_of(&project, TrackType::Video),
			2,
			"default 2 video tracks"
		);

		let index = add_track(&project, seq, TrackType::Video).expect("add a track");
		assert_eq!(index, 2, "the new track is the third video track");
		let track_id = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)[index]
		};

		for cycle in 0..3 {
			oak_undo::global::undo().expect("undo");
			assert_eq!(
				count_of(&project, TrackType::Video),
				2,
				"cycle {cycle}: undo removes only the added track"
			);
			oak_undo::global::redo().expect("redo");
			assert_eq!(
				count_of(&project, TrackType::Video),
				3,
				"cycle {cycle}: redo restores the added track"
			);
			let g = lock(&project);
			assert!(
				g.graph.is_valid(track_id),
				"cycle {cycle}: the same track node is back in the graph"
			);
			assert_eq!(
				track_ids(&g.graph, seq, TrackType::Video)[index],
				track_id,
				"cycle {cycle}: the track keeps its identity and position"
			);
			drop(g);
		}
		oak_undo::global::clear().unwrap();
	}


	use super::*;

	/// Full-graph snapshot for convergence checks: node count plus, for
	/// every track, the ordered block ids and each block's range.
	pub(crate) fn snapshot(p: &ProjectRef, seq: NodeId) -> (usize, Vec<(NodeId, Vec<(NodeId, i128, i128)>)>) {
		let g = lock(p);
		let mut tracks = Vec::new();
		for kind in [TrackType::Video, TrackType::Audio] {
			for t in track_ids(&g.graph, seq, kind) {
				let blocks: Vec<(NodeId, i128, i128)> = track_behavior(&g.graph, t)
					.map(|t| {
						t.blocks
							.iter()
							.map(|&b| {
								let (in_r, out_r, _) = clip_range(&g.graph, b).unwrap_or_default();
								(
									b,
									in_r.numerator() as i128 * 1_000_000
										/ in_r.denominator().max(1) as i128,
									out_r.numerator() as i128 * 1_000_000
										/ out_r.denominator().max(1) as i128,
								)
							})
							.collect()
					})
					.unwrap_or_default();
				tracks.push((t, blocks));
			}
		}
		(g.graph.node_count(), tracks)
	}

	pub(crate) fn cycle_assert(
		p: &ProjectRef,
		seq: NodeId,
		post: &(usize, Vec<(NodeId, Vec<(NodeId, i128, i128)>)>),
		what: &str,
	) {
		for cycle in 0..3 {
			oak_undo::global::undo().unwrap_or_else(|e| panic!("{what}: undo failed: {e:?}"));
			oak_undo::global::redo().unwrap_or_else(|e| panic!("{what}: redo failed: {e:?}"));
			let state = snapshot(p, seq);
			assert_eq!(&state, post, "{what}: cycle {cycle} diverged");
		}
	}

	pub(crate) fn project_with_two_clips(media: &std::path::Path) -> (ProjectRef, NodeId, NodeId) {
		let project = create_project();
		let seq = create_sequence(&project, "Cycle Ops");
		let footage = import_footage(&project, media).expect("import");
		place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			0,
			10,
			0,
		)
		.expect("linked placement")
		.into_iter()
		.next()
		.map(|_| (project.clone(), seq, footage))
		.expect("one clip")
	}

	/// Move / trim / delete / split undo-redo cycles must all converge.
	#[test]
	pub(crate) fn move_trim_delete_split_cycles_converge() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let media = std::env::temp_dir().join(format!("oak_cycle_ops_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate");

		// --- move ---
		let (project, seq, _footage) = project_with_two_clips(&media);
		let clip = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)
				.iter()
				.find_map(|t| track_behavior(&g.graph, *t).and_then(|t| t.blocks.first().copied()))
				.expect("a clip")
		};
		move_clip(&project, clip, 20).expect("move");
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "move");
		oak_undo::global::clear().unwrap();

		// --- trim ---
		trim_clip(&project, clip, 22, 28).expect("trim");
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "trim");
		oak_undo::global::clear().unwrap();

		// --- split ---
		split_clip(&project, clip, 25).expect("split");
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "split");
		oak_undo::global::clear().unwrap();

		// --- delete ---
		delete_clip(&project, clip).expect("delete");
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "delete");
		oak_undo::global::clear().unwrap();

		// --- ripple delete (on a fresh project, then check) ---
		let (project, seq, _footage) = project_with_two_clips(&media);
		let clip = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)
				.iter()
				.find_map(|t| track_behavior(&g.graph, *t).and_then(|t| t.blocks.first().copied()))
				.expect("a clip")
		};
		ripple_delete_clip(&project, clip).expect("ripple delete");
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "ripple delete");
		oak_undo::global::clear().unwrap();

		let _ = std::fs::remove_file(&media);
	}

	// ---- sequence folder mounting ------------------------------------------

	/// The root folder's direct children.
	pub(crate) fn root_children(p: &ProjectRef) -> Vec<NodeId> {
		let guard = lock(p);
		guard
			.graph
			.get(guard.root)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<FolderBehavior>())
			.map(|f| f.children.clone())
			.unwrap_or_default()
	}

	/// Folders and sequences created through the module helpers mount under
	/// the root folder, so the project explorer lists them.
	#[test]
	pub(crate) fn create_folder_and_sequence_mount_under_root() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let folder = create_folder(&project, "Folder 1").expect("create folder");
		let seq = create_sequence_with_params(
			&project,
			"Seq 4K",
			Some((3840, 2160, Rational::new(24000, 1001), true)),
		);
		let children = root_children(&project);
		assert!(children.contains(&folder), "the folder mounts under root");
		assert!(children.contains(&seq), "the sequence mounts under root");
		assert_eq!(
			folder_ids(&lock(&project)).len(),
			2,
			"the root folder itself plus the new folder"
		);
		oak_undo::global::clear().unwrap();
	}

	/// `create_sequence_with_params` writes the format into the sequence's
	/// first video stream, including the new `interlaced` flag.
	#[test]
	pub(crate) fn create_sequence_with_params_sets_first_stream_format() {
		let _g = test_lock();
		let project = create_project();
		let seq = create_sequence_with_params(
			&project,
			"Interlaced",
			Some((1280, 720, Rational::new(30000, 1001), true)),
		);
		let guard = lock(&project);
		let (width, height, rate) = sequence_video_params(&guard.graph, seq).expect("video params");
		assert_eq!((width, height), (1280, 720));
		assert_eq!((rate.numerator(), rate.denominator()), (30000, 1001));
		let interlaced = guard
			.graph
			.get(seq)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<SequenceBehavior>())
			.expect("sequence behavior")
			.video_params
			.first()
			.expect("a video stream")
			.interlaced;
		assert!(interlaced, "the interlaced flag lands in the first stream");
		oak_undo::global::clear().unwrap();
	}

	/// Sequences saved before they mounted under the root load free-floating;
	/// `ensure_sequences_mounted` reattaches them (the open path's migration).
	#[test]
	pub(crate) fn ensure_sequences_mounted_reattaches_orphans() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Legacy");
		// Detach the sequence from the root: a legacy project loaded without
		// the mount migration.
		{
			let mut guard = lock(&project);
			let root = guard.root;
			let folder = guard
				.graph
				.get_mut(root)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<FolderBehavior>())
				.expect("root folder behavior");
			folder.children.retain(|&c| c != seq);
		}
		assert!(
			!root_children(&project).contains(&seq),
			"the sequence floats free after the detach"
		);
		ensure_sequences_mounted(&project);
		assert!(
			root_children(&project).contains(&seq),
			"the orphan sequence reattaches under root"
		);
		oak_undo::global::clear().unwrap();
	}

	/// `set_sequence_parameters` rewrites the display name and the first video
	/// stream's format in one call (the properties dialog's commit path).
	#[test]
	pub(crate) fn set_sequence_parameters_updates_name_and_format() {
		let _g = test_lock();
		let project = create_project();
		let seq = create_sequence_with_params(
			&project,
			"Before",
			Some((1280, 720, Rational::new(25, 1), false)),
		);
		set_sequence_parameters(
			&project,
			seq,
			"After",
			1920,
			1080,
			Rational::new(30000, 1001),
			true,
		)
		.expect("update parameters");
		let guard = lock(&project);
		assert_eq!(node_label(&guard.graph, seq), "After");
		let (width, height, rate) = sequence_video_params(&guard.graph, seq).expect("video params");
		assert_eq!((width, height), (1920, 1080));
		assert_eq!((rate.numerator(), rate.denominator()), (30000, 1001));
		let interlaced = guard
			.graph
			.get(seq)
			.and_then(|e| e.behavior.as_any())
			.and_then(|a| a.downcast_ref::<SequenceBehavior>())
			.expect("sequence behavior")
			.video_params
			.first()
			.expect("a video stream")
			.interlaced;
		assert!(interlaced, "the interlaced flag updates too");
		oak_undo::global::clear().unwrap();
	}

//! Seam, edge and default-transition geometry plus undo/redo coverage.

use super::*;
	#[test]

	/// A LONE clip (no junction on either side) takes the default
	/// transition on BOTH ends (PR's apply-to-a-lone-clip semantics): a
	/// head fade-in wired `in_block_in` only and a tail fade-out wired
	/// `out_block_in` only, each spanning `2*half` into the clip, as ONE
	/// undo row — and the action is idempotent (a second run has nothing
	/// left to build and leaves no empty row).
	pub(crate) fn default_transition_covers_both_ends_of_a_lone_clip() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let media =
			std::env::temp_dir().join(format!("oak_lone_transition_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate");
		let project = create_project();
		let seq = create_sequence(&project, "Lone Transition");
		let footage = import_footage(&project, &media).expect("import");
		let clip = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 10, 30, 0)
			.expect("place the clip");
		let (tb, track) = {
			let g = lock(&project);
			(
				sequence_time_base(&g.graph, seq).expect("time base"),
				track_ids(&g.graph, seq, TrackType::Video)[0],
			)
		};
		let half = ts_to_rational(3, tb);

		let before = oak_undo::global::count().unwrap();
		let created =
			add_default_transition(&project, seq, &[clip], half).expect("add default transition");
		assert_eq!(created, 2, "a lone clip takes both ends");
		assert_eq!(
			oak_undo::global::count().unwrap(),
			before + 1,
			"one undo row"
		);

		let edge = half + half;
		{
			let g = lock(&project);
			let blocks = track_behavior(&g.graph, track)
				.map(|t| t.blocks.clone())
				.unwrap_or_default();
			let index = blocks
				.iter()
				.position(|&b| b == clip)
				.expect("clip on track");
			// (A leading gap when the clip starts past zero,) the head
			// transition, the clip, the tail transition.
			let (head, tail) = (blocks[index - 1], blocks[index + 1]);
			let head_b = g
				.graph
				.get(head)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<oak_node::block::TransitionBlockBehavior>())
				.expect("head transition");
			assert_eq!(
				(head_b.in_offset, head_b.out_offset),
				(Rational::new(0, 1), edge),
				"the head transition eats only into the clip"
			);
			assert_eq!(
				(head_b.core.in_(), head_b.core.out()),
				(ts_to_rational(10, tb), ts_to_rational(10, tb) + edge),
				"the head transition spans the clip's head"
			);
			assert_eq!(
				g.graph
					.connected_output(head, oak_node::block::transition_input::IN_BLOCK, -1),
				Some(clip),
				"the head transition wires only the clip in"
			);
			assert_eq!(
				g.graph
					.connected_output(head, oak_node::block::transition_input::OUT_BLOCK, -1),
				None,
				"the head transition has no outgoing side"
			);
			let tail_b = g
				.graph
				.get(tail)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<oak_node::block::TransitionBlockBehavior>())
				.expect("tail transition");
			assert_eq!(
				(tail_b.in_offset, tail_b.out_offset),
				(edge, Rational::new(0, 1)),
				"the tail transition eats only into the clip"
			);
			assert_eq!(
				(tail_b.core.in_(), tail_b.core.out()),
				(ts_to_rational(30, tb) - edge, ts_to_rational(30, tb)),
				"the tail transition spans the clip's tail"
			);
			assert_eq!(
				g.graph
					.connected_output(tail, oak_node::block::transition_input::OUT_BLOCK, -1),
				Some(clip),
				"the tail transition wires only the clip out"
			);
			assert_eq!(
				g.graph
					.connected_output(tail, oak_node::block::transition_input::IN_BLOCK, -1),
				None,
				"the tail transition has no incoming side"
			);
			// The wedges: a head transition draws on the clip's start edge, a
			// tail transition on its end edge.
			assert_eq!(transition_of_clip(&g.graph, clip, true), Some(head));
			assert_eq!(transition_of_clip(&g.graph, clip, false), Some(tail));
		}

		// Idempotent: every edge already carries a transition, so a second
		// run builds nothing and leaves no empty row.
		let rows = oak_undo::global::count().unwrap();
		assert!(add_default_transition(&project, seq, &[clip], half).is_err());
		assert_eq!(oak_undo::global::count().unwrap(), rows);

		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// 编辑 → 设为默认转场: one transition per contiguous seam around the
	/// selection — the single seam here, built once even though both clips
	/// are selected — wired onto both clips as ONE undo row.
	#[test]
	pub(crate) fn default_transition_covers_the_selected_seams() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let media =
			std::env::temp_dir().join(format!("oak_default_transition_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate");
		let (project, seq, track, a, b, tb, half) = two_touching_clips(&media);

		let before = oak_undo::global::count().unwrap();
		let created = add_default_transition(&project, seq, &[a, b], half).expect("add transition");
		assert_eq!(
			created, 1,
			"both clips name the same seam; it is built once"
		);
		assert_eq!(
			oak_undo::global::count().unwrap(),
			before + 1,
			"the whole batch is one undo row"
		);

		let transition = {
			let g = lock(&project);
			let blocks = track_behavior(&g.graph, track)
				.map(|t| t.blocks.clone())
				.unwrap_or_default();
			assert_eq!(
				blocks.len(),
				3,
				"the transition joins the track: {blocks:?}"
			);
			let t = blocks[1];
			let seam = ts_to_rational(10, tb);
			let core = block_core_of(&g.graph, t).expect("transition core");
			assert_eq!(
				(core.in_(), core.out()),
				(seam - half, seam + half),
				"the transition straddles the seam"
			);
			let behavior = g
				.graph
				.get(t)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<oak_node::block::TransitionBlockBehavior>())
				.expect("transition behavior");
			assert_eq!(
				(behavior.in_offset, behavior.out_offset),
				(half, half),
				"a default transition is symmetric"
			);
			assert_eq!(
				g.graph
					.connected_output(t, oak_node::block::transition_input::OUT_BLOCK, -1),
				Some(a),
				"the previous clip feeds out_block_in"
			);
			assert_eq!(
				g.graph
					.connected_output(t, oak_node::block::transition_input::IN_BLOCK, -1),
				Some(b),
				"the next clip feeds in_block_in"
			);
			// The wedges the widget paints: the earlier clip's tail is the
			// transition's in_offset, the later clip's head its out_offset.
			let widths = clip_transition_widths(&g.graph, track);
			assert_eq!(
				widths.get(&a),
				Some(&(None, Some(half))),
				"the head clip's tail wedge"
			);
			assert_eq!(
				widths.get(&b),
				Some(&(Some(half), None)),
				"the tail clip's head wedge"
			);
			assert_eq!(transition_of_clip(&g.graph, b, true), Some(t));
			assert_eq!(transition_of_clip(&g.graph, a, false), Some(t));
			assert_eq!(
				transition_of_clip(&g.graph, a, true),
				None,
				"the first clip has nothing before it"
			);
			assert_eq!(
				transition_of_clip(&g.graph, b, false),
				None,
				"the last clip has nothing after it"
			);
			t
		};

		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "add default transition");

		// The seam carries a transition now: nothing left to build, and the
		// refused batch leaves no empty undo row behind.
		let rows = oak_undo::global::count().unwrap();
		assert!(
			add_default_transition(&project, seq, &[a, b], half).is_err(),
			"a seam that already carries a transition is skipped"
		);
		assert_eq!(oak_undo::global::count().unwrap(), rows);
		{
			let g = lock(&project);
			assert!(
				g.graph.is_valid(transition),
				"the refused batch changed nothing"
			);
		}

		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// The wedge drag (`TransitionChanged`): the later clip's head wedge
	/// resizes the transition's out_offset, the earlier clip's tail wedge its
	/// in_offset, the seam stays put, and a too-long request is clamped to
	/// the dragged clip's length.
	#[test]
	pub(crate) fn transition_length_drag_moves_one_wedge_and_keeps_the_seam() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let media =
			std::env::temp_dir().join(format!("oak_transition_drag_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate");
		let (project, seq, _track, a, b, tb, half) = two_touching_clips(&media);
		add_default_transition(&project, seq, &[a, b], half).expect("add transition");

		let frame = ts_to_rational(1, tb);
		let seam = ts_to_rational(10, tb);
		let offsets = |p: &ProjectRef, t: NodeId| {
			let g = lock(p);
			g.graph
				.get(t)
				.and_then(|e| e.behavior.as_any())
				.and_then(|a| a.downcast_ref::<oak_node::block::TransitionBlockBehavior>())
				.map(|t| (t.in_offset, t.out_offset))
				.expect("transition behavior")
		};
		let transition = {
			let g = lock(&project);
			transition_of_clip(&g.graph, b, true).expect("the head transition of b")
		};

		// Growing the head wedge of the later clip (edge = Start) only
		// touches the out offset.
		let longer = ts_to_rational(5, tb);
		let applied = set_transition_length(&project, b, true, frame, longer).expect("grow");
		assert_eq!(applied, longer);
		assert_eq!(offsets(&project, transition), (half, longer));
		{
			let g = lock(&project);
			let core = block_core_of(&g.graph, transition).expect("transition core");
			assert_eq!((core.in_(), core.out()), (seam - half, seam + longer));
			assert_eq!(
				clip_range(&g.graph, a).map(|r| r.1),
				Some(seam),
				"the seam does not move"
			);
		}
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "transition length");

		// A request longer than the dragged clip is clamped to its length.
		let applied = set_transition_length(&project, b, true, frame, ts_to_rational(9_999, tb))
			.expect("clamp");
		assert_eq!(
			applied,
			ts_to_rational(10, tb),
			"clamped to the later clip's length"
		);

		// The other side: the earlier clip's tail wedge (edge = End) drives
		// the in offset.
		let applied = set_transition_length(&project, a, false, frame, ts_to_rational(6, tb))
			.expect("grow in");
		assert_eq!(applied, ts_to_rational(6, tb));
		assert_eq!(
			offsets(&project, transition),
			(ts_to_rational(6, tb), ts_to_rational(10, tb))
		);
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "transition length (in side)");

		// A clip without a transition on that edge is rejected, not silently
		// rewired.
		assert!(
			set_transition_length(&project, b, false, frame, half).is_err(),
			"the last clip has no tail transition"
		);
		assert!(
			set_transition_length(&project, a, true, frame, half).is_err(),
			"the first clip has no head transition"
		);

		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}


	use super::*;
	use std::path::PathBuf;

	/// A unique temp path per test tag (the tests share the process but not
	/// the tag namespace).
	pub(crate) fn temp_path(tag: &str, ext: &str) -> PathBuf {
		std::env::temp_dir().join(format!("oak_graphops_gap_{tag}.{ext}"))
	}

	/// A decodable test clip on disk.
	pub(crate) fn media_file(tag: &str) -> PathBuf {
		let path = temp_path(tag, "mp4");
		oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("write test media");
		path
	}

	/// A project with one sequence and one imported footage node.
	pub(crate) fn project_with_footage(tag: &str) -> (ProjectRef, NodeId, NodeId, PathBuf) {
		let project = create_project();
		let seq = create_sequence(&project, "Gap Seq");
		let path = media_file(tag);
		let footage = import_footage(&project, &path).expect("import footage");
		(project, seq, footage, path)
	}

	pub(crate) fn video_track_of(project: &ProjectRef, seq: NodeId) -> NodeId {
		let g = lock(project);
		track_ids(&g.graph, seq, TrackType::Video)[0]
	}

	pub(crate) fn video_list_of(project: &ProjectRef, seq: NodeId) -> NodeId {
		let g = lock(project);
		track_list_of(&g.graph, seq, TrackType::Video).expect("video track list")
	}

	pub(crate) fn place_clip(
		project: &ProjectRef,
		seq: NodeId,
		footage: NodeId,
		in_ts: i64,
		out_ts: i64,
	) -> NodeId {
		place_footage_clip(project, seq, footage, TrackType::Video, 0, in_ts, out_ts, 0)
			.expect("place a clip")
	}

	/// A behavior that does not override `as_any` — the timeline helpers must
	/// treat it as opaque.
	struct OpaqueBehavior;
	impl oak_node::node::NodeBehavior for OpaqueBehavior {
		fn name(&self) -> &str {
			"Opaque"
		}
		fn type_id(&self) -> &str {
			"test.opaque"
		}
		fn duplicate(
			&self,
			_core: &oak_node::node::NodeCore,
		) -> Option<Box<dyn oak_node::node::NodeBehavior>> {
			None
		}
	}

	pub(crate) fn add_opaque(project: &ProjectRef) -> NodeId {
		let mut g = lock(project);
		g.graph
			.add_node(oak_node::node::NodeCore::new(), Box::new(OpaqueBehavior))
	}

	/// Load/save error paths and the round trip through the OVE serializer.
	#[test]
	pub(crate) fn project_lifecycle_round_trip_and_errors() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();

		let missing = temp_path("missing_project", "ove");
		let _ = std::fs::remove_file(&missing);
		assert!(
			load_ove(&missing).is_err(),
			"a missing project file is rejected"
		);

		let project = create_project();
		assert_eq!(project_name(&lock(&project)), "(untitled)");
		let _seq = create_sequence(&project, "Round Trip");
		let path = temp_path("round_trip", "ove");
		save_ove(&project, &path).expect("save");
		assert_eq!(project_name(&lock(&project)), "oak_graphops_gap_round_trip");
		assert!(!lock(&project).is_modified(), "save clears the modified flag");

		let loaded = load_ove(&path).expect("load");
		{
			let g = lock(&loaded);
			assert_eq!(project_name(&g), "oak_graphops_gap_round_trip");
			assert!(!g.is_modified(), "a loaded project starts unmodified");
			let seqs = sequence_ids(&g);
			assert_eq!(seqs.len(), 1, "the sequence survives the round trip");
			assert_eq!(node_label(&g.graph, seqs[0]), "Round Trip");
		}

		// Writing over a directory fails.
		assert!(
			save_ove(&project, &std::env::temp_dir()).is_err(),
			"writing to a directory path fails"
		);

		let _ = std::fs::remove_file(&path);
		oak_undo::global::clear().unwrap();
	}

	/// A bare project (no root folder) rejects every node-creating entry.
	#[test]
	pub(crate) fn bare_project_without_root_rejects_creations() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let bare = Project::new();
		let media = media_file("bare_root");
		assert!(create_folder(&bare, "F").is_err());
		assert!(create_text_footage_node(&bare).is_err());
		assert!(import_footage(&bare, &media).is_err());
		let _ = std::fs::remove_file(&media);
	}

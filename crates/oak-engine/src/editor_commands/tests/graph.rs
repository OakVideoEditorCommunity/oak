//! Graph behavior access, block-core mutation and node-connection regressions.

use super::*;
	#[test]

	/// The mutable block-core helper reaches every block kind and rejects
	/// non-block nodes.
	pub(crate) fn block_core_mut_covers_gap_transition_and_adjustment() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("block_cores");
		let a = place_clip(&project, seq, footage, 0, 10);
		let b = place_clip(&project, seq, footage, 10, 20);
		delete_clip(&project, a).expect("delete leaves a gap");
		let track = video_track_of(&project, seq);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let is_gap = |g: &Graph, id: NodeId| {
			g.get(id)
				.and_then(|e| e.behavior.as_any())
				.map(|a| a.is::<oak_node::block::GapBlockBehavior>())
				.unwrap_or(false)
		};
		let gap = {
			let g = lock(&project);
			track_behavior(&g.graph, track)
				.map(|t| t.blocks.clone())
				.unwrap_or_default()
				.into_iter()
				.find(|&block| is_gap(&g.graph, block))
		}
		.expect("a gap block replaced the deleted clip");
		let c = place_clip(&project, seq, footage, 20, 30);
		let transition =
			add_transition_at_seam(&project, b, c, ts_to_rational(2, tb)).expect("transition");
		let adj = create_adjustment_layer(&project, seq, track, 40, 60).expect("adjustment");
		{
			let mut g = lock(&project);
			assert!(with_block_core_mut(&mut g.graph, gap, |core| core.speed = 1.25));
			assert!(with_block_core_mut(&mut g.graph, transition, |core| {
				core.speed = 1.5
			}));
			assert!(with_block_core_mut(&mut g.graph, adj, |core| core.speed = 1.75));
			assert!(!with_block_core_mut(
				&mut g.graph,
				id_of(9_000_006).unwrap(),
				|_| {}
			));
			assert!(!with_block_core_mut(&mut g.graph, seq, |_| {}));
			assert_eq!(block_core_of(&g.graph, gap).map(|c| c.speed), Some(1.25));
			assert_eq!(
				block_core_of(&g.graph, transition).map(|c| c.speed),
				Some(1.5)
			);
			assert_eq!(block_core_of(&g.graph, adj).map(|c| c.speed), Some(1.75));
		}
		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// Track-list find-or-create, track setters and track removal.
	#[test]
	pub(crate) fn track_list_creation_and_track_setter_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let bogus = id_of(9_000_007).unwrap();
		assert!(
			find_or_create_track_list(&project, bogus, TrackType::Video).is_none(),
			"a bogus sequence cannot own a list"
		);
		let seq = create_sequence(&project, "Lists");
		let list = find_or_create_track_list(&project, seq, TrackType::Video).expect("the list");
		assert_eq!(
			find_or_create_track_list(&project, seq, TrackType::Video),
			Some(list),
			"an existing list is reused"
		);
		assert!(
			find_or_create_track_list(&project, seq, TrackType::Subtitle).is_some(),
			"a new kind creates its list"
		);
		assert!(track_ids(&lock(&project).graph, seq, TrackType::Subtitle).is_empty());
		assert!(add_track(&project, bogus, TrackType::Video).is_err());

		let track = video_track_of(&project, seq);
		let initial_height = track_height(&project, track).expect("a live track");
		set_track_height(&project, track, -5.0);
		set_track_height(&project, track, 0.0);
		assert_eq!(
			track_height(&project, track),
			Some(initial_height),
			"non-positive heights are ignored"
		);
		set_track_height(&project, track, 4.5);
		assert_eq!(track_height(&project, track), Some(4.5));
		set_track_height(&project, bogus, 4.5);
		assert_eq!(track_height(&project, bogus), None);
		assert_eq!(track_muted(&project, bogus), None);
		assert_eq!(track_locked(&project, bogus), None);

		let before = track_ids(&lock(&project).graph, seq, TrackType::Video).len();
		remove_track(&project, track).expect("remove track");
		assert_eq!(
			track_ids(&lock(&project).graph, seq, TrackType::Video).len(),
			before - 1
		);
		oak_undo::global::undo().unwrap();
		assert_eq!(
			track_ids(&lock(&project).graph, seq, TrackType::Video).len(),
			before
		);
		assert!(lock(&project).graph.is_valid(track));
		oak_undo::global::clear().unwrap();
	}

	/// Clip placement validates ranges, kinds, footage, track indices and the
	/// missing track list; a label-less footage falls back to its filename.
	#[test]
	pub(crate) fn clip_placement_validation_and_footage_label_fallback() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("clip_place");
		let bogus = id_of(9_000_008).unwrap();
		let input = oak_node::block::clip_input::TEXTURE_INPUT;
		assert!(place_footage_clip(&project, seq, footage, TrackType::Video, 0, -1, 10, 0).is_err());
		assert!(place_footage_clip(&project, seq, footage, TrackType::Video, 0, 10, 10, 0).is_err());
		assert!(place_footage_clip(&project, seq, footage, TrackType::Video, 0, 0, 10, -2).is_err());
		assert!(
			place_footage_clip(&project, seq, footage, TrackType::Subtitle, 0, 0, 10, 0).is_err()
		);
		assert!(place_footage_clip(&project, seq, bogus, TrackType::Video, 0, 0, 10, 0).is_err());
		assert!(
			place_footage_clip(&project, seq, footage, TrackType::Video, 99, 0, 10, 0).is_err(),
			"the track index must exist"
		);
		assert!(
			place_footage_clip(&project, bogus, footage, TrackType::Video, 0, 0, 10, 0).is_err(),
			"the sequence needs a valid frame rate"
		);
		assert!(
			place_footage_clips_linked(
				&project,
				seq,
				footage,
				&[(TrackType::Video, 0)],
				0,
				10,
				0
			)
			.is_err(),
			"a linked drop needs at least two targets"
		);
		assert!(
			place_footage_clips_linked(
				&project,
				seq,
				bogus,
				&[(TrackType::Video, 0), (TrackType::Audio, 0)],
				0,
				10,
				0
			)
			.is_err()
		);
		assert!(
			place_footage_clips_linked(
				&project,
				seq,
				footage,
				&[(TrackType::Video, 0), (TrackType::Subtitle, 0)],
				0,
				10,
				0
			)
			.is_err(),
			"a kind without a track list is rejected"
		);

		lock(&project)
			.graph
			.get_mut(footage)
			.expect("the footage")
			.core
			.label
			.clear();
		let clip = place_clip(&project, seq, footage, 0, 10);
		let expected_name = media.file_name().unwrap().to_string_lossy().into_owned();
		{
			let g = lock(&project);
			let tb = sequence_time_base(&g.graph, seq).expect("time base");
			assert_eq!(node_label(&g.graph, clip), expected_name);
			assert_eq!(g.graph.connected_output(clip, input, -1), Some(footage));
			assert_eq!(
				clip_range(&g.graph, clip),
				Some((Rational::new(0, 1), ts_to_rational(10, tb), Rational::new(0, 1)))
			);
			assert_eq!(
				clip_media_filename(&g.graph, clip).as_deref(),
				Some(media.to_string_lossy().as_ref())
			);
		}

		// Removing the video list rejects later placements.
		let list = video_list_of(&project, seq);
		{
			let mut g = lock(&project);
			if let Some(s) = g
				.graph
				.get_mut(seq)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<SequenceBehavior>())
			{
				s.track_lists.retain(|&l| l != list);
			}
		}
		assert!(
			place_footage_clip(&project, seq, footage, TrackType::Video, 0, 20, 30, 0).is_err(),
			"no video track list rejects the placement"
		);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// Nested-sequence and generator clip placement, including their
	/// validation errors.
	#[test]
	pub(crate) fn nested_and_generator_clip_placement_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, host, footage, media) = project_with_footage("nested");
		let source = create_sequence(&project, "Source Seq");
		let text = create_text_footage_node(&project).expect("text node");
		let bogus = id_of(9_000_009).unwrap();
		let input = oak_node::block::clip_input::TEXTURE_INPUT;

		assert!(place_nested_sequence_clip(&project, host, source, 0, -1, 10).is_err());
		assert!(place_nested_sequence_clip(&project, host, source, 0, 10, 10).is_err());
		assert!(place_nested_sequence_clip(&project, host, bogus, 0, 0, 10).is_err());
		assert!(place_nested_sequence_clip(&project, host, source, 9, 0, 10).is_err());
		let nested =
			place_nested_sequence_clip(&project, host, source, 0, 0, 25).expect("nested clip");
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, host).expect("time base")
		};
		{
			let g = lock(&project);
			assert!(node_label(&g.graph, nested).contains("序列"));
			assert_eq!(g.graph.connected_output(nested, input, -1), Some(source));
			assert_eq!(
				clip_range(&g.graph, nested).map(|r| r.1),
				Some(ts_to_rational(25, tb))
			);
		}

		assert!(place_generator_clip(&project, host, text, 0, 5, 5).is_err());
		assert!(place_generator_clip(&project, host, bogus, 0, 0, 10).is_err());
		assert!(place_generator_clip(&project, host, text, 9, 0, 10).is_err());
		let generator =
			place_generator_clip(&project, host, text, 0, 30, 55).expect("generator clip");
		{
			let g = lock(&project);
			assert_eq!(g.graph.connected_output(generator, input, -1), Some(text));
			assert_eq!(
				clip_range(&g.graph, generator).map(|r| r.1),
				Some(ts_to_rational(55, tb))
			);
		}
		assert!(
			place_text_clip(&project, host, bogus, 0, 0, 10).is_err(),
			"a stale text id is rejected"
		);

		let _ = footage;
		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	pub(crate) fn set_clip_track(project: &ProjectRef, clip: NodeId, track: Option<NodeId>) {
		let mut g = lock(project);
		if let Some(c) = g
			.graph
			.get_mut(clip)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
		{
			c.core.track = track;
		}
	}

	pub(crate) fn set_block_track(project: &ProjectRef, block: NodeId, track: Option<NodeId>) {
		let mut g = lock(project);
		with_block_core_mut(&mut g.graph, block, |core| core.track = track);
	}

	pub(crate) fn set_track_list(project: &ProjectRef, track: NodeId, list: Option<NodeId>) {
		let mut g = lock(project);
		if let Some(t) = g
			.graph
			.get_mut(track)
			.and_then(|e| e.behavior.as_any_mut())
			.and_then(|a| a.downcast_mut::<TrackBehavior>())
		{
			t.track_list = list;
		}
	}


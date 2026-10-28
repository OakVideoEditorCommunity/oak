//! Adjustment-layer placement, range validation and undo regression coverage.

use super::*;


	// ---- adjustment layers ------------------------------------------------

	/// Drop a fresh adjustment-layer block onto `track` spanning
	/// `[in_ts, out_ts)` (timeline frames) and return its node id.
	pub(crate) fn add_adjustment_layer(
		p: &ProjectRef,
		seq: NodeId,
		track: NodeId,
		in_ts: i64,
		out_ts: i64,
	) -> NodeId {
		let tb = {
			let g = lock(p);
			sequence_time_base(&g.graph, seq).expect("the sequence has a timebase")
		};
		let in_r = ts_to_rational(in_ts, tb);
		let block = {
			let mut g = lock(p);
			let (core, behavior) = oak_node::block::adjustment_create();
			let id = g.graph.add_node(core, behavior);
			let entry = g.graph.get_mut(id).expect("the new block");
			let a = entry
				.behavior
				.as_any_mut()
				.and_then(|a| a.downcast_mut::<AdjustmentBlockBehavior>())
				.expect("an adjustment behavior");
			a.core.range = TimeRange::new(in_r, ts_to_rational(out_ts, tb));
			id
		};
		let (list, index) = {
			let g = lock(p);
			let list = track_behavior(&g.graph, track)
				.and_then(|t| t.track_list)
				.expect("the track has a list");
			let index = track_list_behavior(&g.graph, list)
				.and_then(|l| l.tracks.iter().position(|&t| t == track))
				.expect("the track is in its list") as i32;
			(list, index)
		};
		push(
			oak_timeline::undopointer::TrackPlaceBlockCommand::new(
				node_ref(p, list),
				index,
				node_ref(p, block),
				in_r,
			)
			.to_command(),
			"Add Adjustment Layer",
		)
		.expect("place the adjustment layer");
		block
	}

	/// An adjustment layer is a timeline block: the generic clip queries
	/// see it, its length counts toward the track, and the generic trim /
	/// delete operations apply to it with converging undo-redo cycles.
	#[test]
	pub(crate) fn adjustment_layer_trim_and_delete_cycles_converge() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Adjustment Ops");
		add_track(&project, seq, TrackType::Video).expect("add video track");
		let track = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)[0]
		};
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("timebase")
		};
		let adj = add_adjustment_layer(&project, seq, track, 50, 125);

		{
			let g = lock(&project);
			assert!(
				clip_ids(&g.graph, track).contains(&adj),
				"an adjustment layer counts as a timeline block"
			);
			assert_eq!(
				clip_range(&g.graph, adj),
				Some((
					ts_to_rational(50, tb),
					ts_to_rational(125, tb),
					Rational::new(0, 1)
				)),
				"the block reports its timeline range and a zero media in"
			);
			assert_eq!(
				clip_track(&g.graph, adj),
				Some(track),
				"the block knows its track"
			);
		}
		assert_eq!(
			oak_timeline::util::track_length(&node_ref(&project, track)),
			ts_to_rational(125, tb),
			"the track length counts the leading gap plus the adjustment layer"
		);

		// Out-trim: the in point stays, the out point moves.
		trim_clip(&project, adj, 50, 100).expect("trim the adjustment layer");
		{
			let g = lock(&project);
			let (in_r, out_r, _) = clip_range(&g.graph, adj).expect("range");
			assert_eq!(in_r, ts_to_rational(50, tb));
			assert_eq!(
				out_r,
				ts_to_rational(100, tb),
				"the out-trim lands on the adjustment layer"
			);
		}
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "adjustment trim");
		oak_undo::global::clear().unwrap();

		// Delete: the block leaves the track (and the clip list), and the
		// replaced span is a gap, not a block.
		delete_clip(&project, adj).expect("delete the adjustment layer");
		{
			let g = lock(&project);
			assert!(
				!clip_ids(&g.graph, track).contains(&adj),
				"the deleted block leaves the clip list"
			);
			assert_eq!(
				clip_track(&g.graph, adj),
				None,
				"the removed block no longer names a track"
			);
			let blocks = track_behavior(&g.graph, track)
				.map(|t| t.blocks.clone())
				.unwrap_or_default();
			assert_eq!(
				blocks.len(),
				0,
				"the trailing block and the gap that preceded it are gone: {blocks:?}"
			);
		}
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "adjustment delete");
		oak_undo::global::clear().unwrap();
	}

	/// The production entry point ([`create_adjustment_layer`]) lands the
	/// block on the video track with the requested span (nothing wired to
	/// it), the undo detaches it without removing the node and the redo
	/// re-places the same block.
	#[test]
	pub(crate) fn create_adjustment_layer_places_on_video_track_and_undoes() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Adjustment UI");
		add_track(&project, seq, TrackType::Video).expect("add video track");
		let track = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)[0]
		};
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("timebase")
		};

		let adj = create_adjustment_layer(&project, seq, track, 50, 125).expect("create");
		{
			let g = lock(&project);
			assert_eq!(
				clip_ids(&g.graph, track),
				vec![adj],
				"the adjustment layer is the track's only block"
			);
			assert_eq!(
				clip_range(&g.graph, adj),
				Some((
					ts_to_rational(50, tb),
					ts_to_rational(125, tb),
					Rational::new(0, 1)
				)),
				"the block spans the requested range with a zero media in"
			);
			assert_eq!(
				clip_track(&g.graph, adj),
				Some(track),
				"the block knows its track"
			);
			assert!(
				g.graph
					.get(adj)
					.and_then(|e| e.behavior.as_any())
					.map(|a| a.is::<AdjustmentBlockBehavior>())
					.unwrap_or(false),
				"the new block is an adjustment layer, not a clip"
			);
		}
		assert_eq!(
			oak_timeline::util::track_length(&node_ref(&project, track)),
			ts_to_rational(125, tb),
			"the track length counts the leading gap plus the layer"
		);
		let post = snapshot(&project, seq);
		cycle_assert(&project, seq, &post, "create adjustment");

		oak_undo::global::undo().unwrap();
		{
			let g = lock(&project);
			assert_eq!(
				clip_track(&g.graph, adj),
				None,
				"the undo detaches the block from its track"
			);
			assert!(
				g.graph.is_valid(adj),
				"the detached block stays in the graph as an orphan"
			);
		}
		oak_undo::global::redo().unwrap();
		{
			let g = lock(&project);
			assert_eq!(
				clip_track(&g.graph, adj),
				Some(track),
				"the redo re-places the same block"
			);
		}
		oak_undo::global::clear().unwrap();
	}

	/// Only video tracks take adjustment layers, and the span must be a
	/// forward range of non-negative frames; a rejected call creates no
	/// node and leaves the undo stack untouched.
	#[test]
	pub(crate) fn create_adjustment_layer_rejects_audio_tracks_and_bad_ranges() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Adjustment Rejects");
		add_track(&project, seq, TrackType::Video).expect("add video track");
		add_track(&project, seq, TrackType::Audio).expect("add audio track");
		let (video, audio) = {
			let g = lock(&project);
			(
				track_ids(&g.graph, seq, TrackType::Video)[0],
				track_ids(&g.graph, seq, TrackType::Audio)[0],
			)
		};
		let before = oak_undo::global::count().unwrap();

		assert!(
			create_adjustment_layer(&project, seq, audio, 0, 25).is_err(),
			"an audio track takes no adjustment layer"
		);
		assert!(
			create_adjustment_layer(&project, seq, video, 25, 25).is_err(),
			"an empty range is rejected"
		);
		assert!(
			create_adjustment_layer(&project, seq, video, 50, 25).is_err(),
			"a reversed range is rejected"
		);
		assert!(
			create_adjustment_layer(&project, seq, video, -1, 25).is_err(),
			"a negative in point is rejected"
		);
		assert_eq!(
			oak_undo::global::count().unwrap(),
			before,
			"a rejected call leaves the undo stack untouched"
		);
		{
			let g = lock(&project);
			assert!(
				clip_ids(&g.graph, video).is_empty(),
				"no block was created for the rejected calls"
			);
		}
		oak_undo::global::clear().unwrap();
	}

	/// 添加文本素材: the helper creates a text generator mounted under the
	/// root as ONE undoable row. The undo detaches it from the folder AND
	/// removes the node from the graph (not a bare unmount), and the redo
	/// restores it under the same id.
	#[test]
	pub(crate) fn create_text_footage_mounts_under_root_and_undoes() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let before = oak_undo::global::count().unwrap();
		let text = create_text_footage_node(&project).expect("create text footage");
		assert_eq!(
			oak_undo::global::count().unwrap(),
			before + 1,
			"the whole creation is one undo row"
		);
		let root = {
			let g = lock(&project);
			assert_eq!(node_type_id(&g.graph, text), TEXT_FOOTAGE_TYPE_ID);
			assert_eq!(node_label(&g.graph, text), TEXT_FOOTAGE_LABEL);
			assert_eq!(
				g.graph.get(text).map(|e| e.core.bin_folder),
				Some(Some(g.root)),
				"the text node names the root as its bin folder"
			);
			g.root
		};
		assert!(
			root_children(&project).contains(&text),
			"the text entry mounts under the root folder"
		);

		oak_undo::global::undo().unwrap();
		assert!(
			!root_children(&project).contains(&text),
			"the undo unmounts the entry"
		);
		assert!(
			!lock(&project).graph.is_valid(text),
			"the undo removes the node itself — no orphan outlives it"
		);

		oak_undo::global::redo().unwrap();
		assert!(
			root_children(&project).contains(&text),
			"the redo remounts the entry"
		);
		let g = lock(&project);
		assert_eq!(
			node_type_id(&g.graph, text),
			TEXT_FOOTAGE_TYPE_ID,
			"the restored node keeps its id and type"
		);
		assert_eq!(
			g.graph.get(text).map(|e| e.core.bin_folder),
			Some(Some(root)),
			"the restored node names the root again"
		);
		drop(g);
		oak_undo::global::clear().unwrap();
	}

	/// The text drop places a GENERATOR clip: the block spans the dropped
	/// range on the target track and its `tex_in` reads the text node
	/// instead of a decode. Non-text entries and out-of-range tracks are
	/// rejected.
	#[test]
	pub(crate) fn place_text_clip_wires_the_generator_and_spans_the_range() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Text Drop");
		let text = create_text_footage_node(&project).expect("create text footage");
		let (tb, track) = {
			let g = lock(&project);
			let tb = sequence_time_base(&g.graph, seq).expect("time base");
			let track = track_ids(&g.graph, seq, TrackType::Video)[0];
			(tb, track)
		};
		let clip = place_text_clip(&project, seq, text, 0, 12, 137).expect("place the text clip");
		{
			let g = lock(&project);
			let (in_r, out_r, _) = clip_range(&g.graph, clip).expect("clip range");
			assert_eq!(in_r, ts_to_rational(12, tb));
			assert_eq!(
				out_r,
				ts_to_rational(137, tb),
				"the clip spans the dropped frames"
			);
			assert_eq!(
				g.graph
					.connected_output(clip, oak_node::block::clip_input::TEXTURE_INPUT, -1),
				Some(text),
				"the clip reads the generator through tex_in"
			);
			assert_eq!(
				clip_track(&g.graph, clip),
				Some(track),
				"the clip lands on the target video track"
			);
		}
		// Only the text generator takes the generator-clip path: a folder
		// entry on the same call is rejected.
		let folder = create_folder(&project, "Not Text").expect("create folder");
		assert!(
			place_text_clip(&project, seq, folder, 0, 0, 10).is_err(),
			"a non-text node is not placeable as a text clip"
		);
		assert!(
			place_text_clip(&project, seq, text, 9, 0, 10).is_err(),
			"a track index past the video tracks is rejected"
		);
		oak_undo::global::clear().unwrap();
	}

	// ---- default transitions (Ctrl+Shift+D / clip menu) ---------------------

	/// Two touching clips on the video track of a fresh project, plus the
	/// track, the time base and the symmetric half length the transition
	/// tests use.
	pub(crate) fn two_touching_clips(
		media: &std::path::Path,
	) -> (
		ProjectRef,
		NodeId,
		NodeId,
		NodeId,
		NodeId,
		(i64, i64),
		Rational,
	) {
		let project = create_project();
		let seq = create_sequence(&project, "Default Transition");
		let footage = import_footage(&project, media).expect("import");
		let a = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 0, 10, 0)
			.expect("place a");
		let b = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 10, 20, 0)
			.expect("place b");
		let (tb, track) = {
			let g = lock(&project);
			(
				sequence_time_base(&g.graph, seq).expect("time base"),
				track_ids(&g.graph, seq, TrackType::Video)[0],
			)
		};
		let half = ts_to_rational(3, tb);
		(project, seq, track, a, b, tb, half)
	}


//! Track lifecycle, flags, placement validation and track-state undo tests.

use super::*;


	use super::*;

	/// A project with one sequence holding one track of `kind`; returns the
	/// project, the sequence and the track's node id.
	pub(crate) fn project_with_track(kind: TrackType) -> (ProjectRef, NodeId, NodeId) {
		let project = create_project();
		let seq = create_sequence(&project, "Seq");
		let index = add_track(&project, seq, kind).expect("add a track");
		let track = {
			let g = lock(&project);
			track_ids(&g.graph, seq, kind)[index]
		};
		(project, seq, track)
	}

	/// The track flag setters flip the flag as ONE undoable entry each;
	/// undo restores the previous value and redo re-applies.
	#[test]
	pub(crate) fn track_flag_setters_toggle_and_undo() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, _seq, track) = project_with_track(TrackType::Video);

		assert_eq!(track_muted(&project, track), Some(false));
		assert_eq!(track_locked(&project, track), Some(false));

		set_track_muted(&project, track, true).expect("mute the track");
		assert_eq!(track_muted(&project, track), Some(true));
		set_track_locked(&project, track, true).expect("lock the track");
		assert_eq!(track_locked(&project, track), Some(true));

		oak_undo::global::undo().unwrap();
		assert_eq!(track_locked(&project, track), Some(false));
		oak_undo::global::undo().unwrap();
		assert_eq!(track_muted(&project, track), Some(false));
		oak_undo::global::redo().unwrap();
		assert_eq!(track_muted(&project, track), Some(true));
		oak_undo::global::clear().unwrap();
	}

	/// Setting a flag to its current value pushes no undo row.
	#[test]
	pub(crate) fn track_flag_setter_noop_when_unchanged() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, _seq, track) = project_with_track(TrackType::Audio);
		let before = oak_undo::global::count().unwrap();

		set_track_muted(&project, track, false).expect("mute already false");
		set_track_locked(&project, track, false).expect("lock already false");
		assert_eq!(oak_undo::global::count().unwrap(), before);
		oak_undo::global::clear().unwrap();
	}

	/// Undo/redo stability: undo → redo → undo → redo must converge to the
	/// SAME graph state every cycle (the user's "撤销再前进再撤销再前进，
	/// 结果居然变了" regression). Snapshot the sequence's track blocks and
	/// the graph node count around two full cycles.
	#[test]
	pub(crate) fn undo_redo_cycles_converge_to_the_same_state() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Undo Cycles");
		let media = std::env::temp_dir().join(format!("oak_undo_cycle_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate test media");
		let footage = import_footage(&project, &media).expect("import");
		place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			0,
			10,
			0,
		)
		.expect("linked placement");

		let snapshot = |p: &ProjectRef| -> (usize, Vec<(TrackType, Vec<usize>)>) {
			let g = lock(p);
			let node_count = g.graph.node_count();
			let mut tracks = Vec::new();
			for kind in [TrackType::Video, TrackType::Audio] {
				let blocks: Vec<usize> = track_ids(&g.graph, seq, kind)
					.iter()
					.map(|t| {
						track_behavior(&g.graph, *t)
							.map(|t| t.blocks.len())
							.unwrap_or(0)
					})
					.collect();
				tracks.push((kind, blocks));
			}
			(node_count, tracks)
		};

		let before = snapshot(&project);
		// Two full undo/redo cycles; the state must be identical after
		// every redo and match the pre-undo state after every undo.
		for cycle in 0..2 {
			oak_undo::global::undo().expect("undo");
			let g = lock(&project);
			let video_blocks: usize = track_ids(&g.graph, seq, TrackType::Video)
				.iter()
				.map(|t| {
					track_behavior(&g.graph, *t)
						.map(|t| t.blocks.len())
						.unwrap_or(0)
				})
				.sum();
			assert_eq!(video_blocks, 0, "cycle {cycle}: undo removes the clips");
			drop(g);
			oak_undo::global::redo().expect("redo");
			let state = snapshot(&project);
			assert_eq!(
				state, before,
				"cycle {cycle}: redo must restore the exact state"
			);
		}
		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// A stale (non-track) id is rejected, not silently ignored.
	#[test]
	pub(crate) fn track_flag_setters_reject_non_tracks() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, _track) = project_with_track(TrackType::Video);
		assert!(set_track_muted(&project, seq, true).is_err());
		assert!(set_track_locked(&project, seq, true).is_err());
		oak_undo::global::clear().unwrap();
	}

	/// Every clip is stamped at creation with the footage's name as its
	/// label and a color index fixed for its whole life — adding or
	/// removing other clips must not reshuffle it (the old
	/// track-position-derived color did).
	#[test]
	pub(crate) fn clips_get_a_creation_time_label_and_stable_color() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Clip Color");
		let media = std::env::temp_dir().join(format!("oak_clip_color_{}.mp4", std::process::id()));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate test media");
		let footage = import_footage(&project, &media).expect("import");
		let name = media.file_name().unwrap().to_string_lossy().into_owned();

		let color_of = |p: &ProjectRef, id: NodeId| -> i32 {
			let g = lock(p);
			g.graph
				.get(id)
				.expect("the clip node exists")
				.core
				.override_color
		};

		// Label = the footage name; the color is locked in at creation.
		let first = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 0, 10, 0)
			.expect("place first");
		let color1 = color_of(&project, first);
		assert_ne!(color1, -1, "the clip color is locked in at creation");
		{
			let g = lock(&project);
			assert_eq!(
				g.graph.get(first).unwrap().core.label,
				name,
				"the clip is named after its footage"
			);
		}

		// Adding another clip must not reshuffle the first one's color.
		let _second = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 10, 20, 0)
			.expect("place second");
		assert_eq!(
			color_of(&project, first),
			color1,
			"the color survives later edits"
		);

		// The linked A/V drop names both clips after the footage too.
		let linked = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			20,
			30,
			0,
		)
		.expect("linked placement");
		for clip in linked {
			let g = lock(&project);
			assert_eq!(g.graph.get(clip).unwrap().core.label, name);
		}

		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// Paste regression: the pasted group's links must land on
	/// NodeCore.links (where links_of/are_linked read them), not on the
	/// parallel BlockCore.links mirror the graph API never sees. Paste a
	/// copied linked A/V pair and require links_of to find the link; undo
	/// clears it, redo restores it.
	#[test]
	pub(crate) fn paste_links_the_group_in_node_core_links() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Paste Links");
		let media =
			std::env::temp_dir().join(format!("oak_paste_links_{}.mp4", std::process::id()));
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

		let items = copy_clips(&project, &dropped);
		assert_eq!(items.len(), 2, "both clips copied");
		let pasted = paste_clips(&project, seq, &items, 20).expect("paste");
		assert_eq!(pasted.len(), 2);
		{
			let g = lock(&project);
			assert!(
				g.graph.links_of(pasted[0]).contains(&pasted[1]),
				"links_of finds the pasted link (the wrong-field regression)"
			);
			assert!(g.graph.are_linked(pasted[1], pasted[0]));
		}

		// Undo unlinks the group; redo relinks it.
		oak_undo::global::undo().expect("undo paste");
		{
			let g = lock(&project);
			assert!(
				!g.graph.are_linked(pasted[0], pasted[1]),
				"undo unlinks the pair"
			);
		}
		oak_undo::global::redo().expect("redo paste");
		{
			let g = lock(&project);
			assert!(
				g.graph.are_linked(pasted[0], pasted[1]),
				"redo relinks the pair"
			);
		}
		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}


	/// Edit-menu operations: enable/disable, speed, rename, duplicate,
	/// paste-insert (tail shifts right), area delete (gap vs ripple) and
	/// roll-to-playhead — each undoable.
	#[test]
	pub(crate) fn edit_menu_operations_roundtrip() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "EditOps");
		let media = std::env::temp_dir().join(format!(
			"oak_edit_ops_{}.mp4",
			std::process::id()
		));
		oak_codec::testmedia::write_test_clip(&media, 64, 64, 10, 10).expect("generate test media");
		let footage = import_footage(&project, &media).expect("import");

		let clip_a = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 0, 10, 0)
			.expect("clip A");
		let clip_b = place_footage_clip(&project, seq, footage, TrackType::Video, 0, 10, 20, 0)
			.expect("clip B");

		let range_of = |p: &ProjectRef, c: NodeId| {
			let g = lock(p);
			let core = block_core_of(&g.graph, c).expect("block core");
			let tb = sequence_time_base(&g.graph, seq).unwrap_or((1, 25));
			(
				rational_to_ts(core.range.in_(), tb),
				rational_to_ts(core.range.out(), tb),
			)
		};

		// Enable/disable toggles, and undoes.
		set_blocks_enabled(&project, &[clip_a], false).expect("disable");
		{
			let g = lock(&project);
			assert!(!block_core_of(&g.graph, clip_a).unwrap().enabled);
		}
		oak_undo::global::undo().expect("undo disable");
		{
			let g = lock(&project);
			assert!(block_core_of(&g.graph, clip_a).unwrap().enabled);
		}

		// Speed lands on the block core; the span is unchanged.
		set_clips_speed(&project, &[clip_a], 2.0).expect("speed");
		{
			let g = lock(&project);
			let core = block_core_of(&g.graph, clip_a).unwrap();
			assert_eq!(core.speed, 2.0);
		}
		assert_eq!(range_of(&project, clip_a), (0, 10), "span unchanged");
		oak_undo::global::undo().expect("undo speed");
		{
			let g = lock(&project);
			assert_eq!(block_core_of(&g.graph, clip_a).unwrap().speed, 1.0);
		}

		// Rename sets the label, undo restores it.
		let old_label = {
			let g = lock(&project);
			g.graph.get(clip_a).unwrap().core.label.clone()
		};
		rename_blocks(&project, &[clip_a], "新名字").expect("rename");
		{
			let g = lock(&project);
			assert_eq!(g.graph.get(clip_a).unwrap().core.label, "新名字");
		}
		oak_undo::global::undo().expect("undo rename");
		{
			let g = lock(&project);
			assert_eq!(g.graph.get(clip_a).unwrap().core.label, old_label);
		}

		// Duplicate lands right after the selection's end.
		let dup = duplicate_clips(&project, seq, &[clip_a, clip_b]).expect("duplicate");
		assert_eq!(dup.len(), 2);
		assert_eq!(range_of(&project, dup[0]), (20, 30));
		assert_eq!(range_of(&project, dup[1]), (30, 40));
		oak_undo::global::undo().expect("undo duplicate");

		// Paste-insert at 5: A straddles the playhead so it splits (front
		// stays [0,5), rear shifts right past the pasted span); B shifts.
		let items = copy_clips(&project, &[clip_a]);
		let inserted = paste_clips_insert(&project, seq, &items, 5).expect("paste insert");
		assert_eq!(range_of(&project, inserted[0]), (5, 15));
		assert_eq!(range_of(&project, clip_b), (20, 30), "B shifted right");
		assert_eq!(range_of(&project, clip_a), (0, 5), "A's front half stays");
		oak_undo::global::undo().expect("undo paste insert");
		assert_eq!(range_of(&project, clip_b), (10, 20), "undo restores B");
		assert_eq!(range_of(&project, clip_a), (0, 10), "undo restores A");

		// Roll A's out boundary (A|B at 10) to 12: A extends, B shrinks.
		roll_clip_edge_to(&project, clip_a, false, 12).expect("roll out");
		assert_eq!(range_of(&project, clip_a), (0, 12));
		assert_eq!(range_of(&project, clip_b), (12, 20));
		oak_undo::global::undo().expect("undo roll");
		assert_eq!(range_of(&project, clip_a), (0, 10));
		assert_eq!(range_of(&project, clip_b), (10, 20));

		// Non-ripple area delete of [2, 6): A shrinks to [0, 2), B stays.
		delete_area(&project, seq, 2, 6, false).expect("delete area");
		assert_eq!(range_of(&project, clip_a), (0, 2));
		assert_eq!(range_of(&project, clip_b), (10, 20), "gap left behind");
		oak_undo::global::undo().expect("undo delete area");
		assert_eq!(range_of(&project, clip_a), (0, 10));

		// Ripple area delete of [2, 6): A shrinks, B pulls left by 4.
		delete_area(&project, seq, 2, 6, true).expect("ripple delete area");
		assert_eq!(range_of(&project, clip_a), (0, 2));
		assert_eq!(range_of(&project, clip_b), (6, 16), "tail rippled left");
		oak_undo::global::undo().expect("undo ripple delete");
		assert_eq!(range_of(&project, clip_b), (10, 20));

		// Nest: the two clips collapse into one nested clip spanning them,
		// and a new sequence appears in the project.
		let nested = nest_clips(&project, seq, &[clip_a, clip_b]).expect("nest");
		assert_eq!(range_of(&project, nested), (0, 20));
		{
			let g = lock(&project);
			assert!(!g.graph.is_valid(clip_a), "the originals are gone");
			assert!(!g.graph.is_valid(clip_b));
		}

		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

	/// Delete-one-of-a-pair regression: deleting a linked clip removes the
	/// survivor's back-reference (take_node cleans up symmetrically), so
	/// moving the survivor afterwards neither trips over a stale id nor
	/// fails the whole move.
	#[test]
	pub(crate) fn deleting_one_of_a_linked_pair_clears_the_partner_link() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let project = create_project();
		let seq = create_sequence(&project, "Delete Linked");
		let media =
			std::env::temp_dir().join(format!("oak_delete_linked_{}.mp4", std::process::id()));
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

		delete_clip(&project, video).expect("delete the video clip");
		{
			let g = lock(&project);
			assert!(
				!g.graph.links_of(audio).contains(&video),
				"the survivor holds no stale reference to the deleted clip"
			);
		}

		// Dragging the survivor works — even when the stale id is still
		// handed in as a linked partner (the old whole-move failure).
		move_clip_with_links(&project, audio, None, 30, &[video]).expect("move the survivor");
		{
			let g = lock(&project);
			let tb = sequence_time_base(&g.graph, seq).expect("a valid frame rate");
			let (in_r, _, _) = clip_range(&g.graph, audio).expect("audio is still a clip");
			assert_eq!(rational_to_ts(in_r, tb), 30, "the survivor moved");
		}
		oak_undo::global::clear().unwrap();
		let _ = std::fs::remove_file(&media);
	}

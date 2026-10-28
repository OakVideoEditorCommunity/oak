//! Sequence setters, document paths and miscellaneous off-track error cases.

use super::*;
	#[test]

	/// Remaining query/setter paths: playhead, sequence-parameter rejection
	/// and the empty video-stream push, plus the listless/off-track edits.
	pub(crate) fn sequence_and_move_misc_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("misc_paths");
		let clip = place_clip(&project, seq, footage, 0, 10);
		let track = video_track_of(&project, seq);
		let list = video_list_of(&project, seq);
		let bogus = id_of(9_000_020).unwrap();

		// Playhead get/set.
		assert_eq!(
			sequence_playhead(&lock(&project).graph, seq),
			Rational::new(0, 1)
		);
		sequence_set_playhead(&project, seq, Rational::new(3, 2));
		assert_eq!(
			sequence_playhead(&lock(&project).graph, seq),
			Rational::new(3, 2)
		);
		sequence_set_playhead(&project, bogus, Rational::new(1, 1));

		// Sequence parameters: stale id and non-sequence entries are
		// rejected.
		assert!(set_sequence_parameters(
			&project,
			bogus,
			"x",
			1,
			1,
			Rational::new(1, 1),
			false
		)
		.is_err());
		let folder = create_folder(&project, "Not A Sequence").expect("folder");
		assert!(set_sequence_parameters(
			&project,
			folder,
			"x",
			1,
			1,
			Rational::new(1, 1),
			false
		)
		.is_err());
		// An empty video-stream list takes the push branch.
		{
			let mut g = lock(&project);
			if let Some(s) = g
				.graph
				.get_mut(seq)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<SequenceBehavior>())
			{
				s.video_params.clear();
			}
		}
		set_sequence_parameters(
			&project,
			seq,
			"Recreated",
			640,
			360,
			Rational::new(24, 1),
			true,
		)
		.expect("set parameters on an empty stream list");
		assert_eq!(
			sequence_video_params(&lock(&project).graph, seq),
			Some((640, 360, Rational::new(24, 1)))
		);

		// A clip on a listless track cannot move.
		let saved_list = {
			let g = lock(&project);
			track_behavior(&g.graph, track).and_then(|t| t.track_list)
		};
		set_track_list(&project, track, None);
		assert!(move_clip(&project, clip, 20).is_err());
		set_track_list(&project, track, saved_list);
		move_clip(&project, clip, 20).expect("the restored list moves again");
		oak_undo::global::undo().unwrap();

		// A trackless clip cannot be the moved anchor of a linked group.
		oak_timeline::util::track_ripple_remove_block(
			&node_ref(&project, track),
			&node_ref(&project, clip),
		);
		assert!(move_clip_with_links(&project, clip, None, 5, &[]).is_err());

		// The clipboard falls back to 25fps when the track list has no
		// sequence.
		let b = place_clip(&project, seq, footage, 30, 40);
		{
			let mut g = lock(&project);
			if let Some(l) = g
				.graph
				.get_mut(list)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackListBehavior>())
			{
				l.sequence = None;
			}
		}
		let items = copy_clips(&project, &[b]);
		assert_eq!(items.len(), 1, "the fallback timebase still copies");
		let in_r = clip_range(&lock(&project).graph, b)
			.expect("b is a clip")
			.0;
		assert_eq!(
			items[0].start_ts,
			(in_r.numerator() * 25) / in_r.denominator(),
			"the fallback treats the rational as 25fps seconds"
		);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// Dropping a new clip whose in-point lands in EMPTY track space still
	/// overwrites the clip its tail covers (the start-on-a-clip behavior):
	/// the covered head is removed and the placed clip lands before the
	/// overlapped clip in track order. The stored model allows holes between
	/// blocks; the place command used to leave the overlapped clip untouched
	/// and insert the new one before it, sliding it UNDER the clip it
	/// covered.
	#[test]
	pub(crate) fn place_clip_from_empty_space_overwrites_the_overlapped_clip() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("place_from_empty");
		let track = video_track_of(&project, seq);
		let head = place_clip(&project, seq, footage, 0, 40);
		let tail = place_clip(&project, seq, footage, 100, 200);

		// Drop a 100-frame clip at 50: its tail covers the tail clip's head.
		let placed = place_clip(&project, seq, footage, 50, 150);

		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let span = |id: NodeId| {
			let g = lock(&project);
			clip_range(&g.graph, id).map(|r| (r.0, r.1))
		};
		assert_eq!(
			clip_ids(&lock(&project).graph, track),
			vec![head, placed, tail],
			"the placed clip lands before the clip it covers"
		);
		assert_eq!(
			span(head),
			Some((Rational::new(0, 1), ts_to_rational(40, tb)))
		);
		assert_eq!(
			span(placed),
			Some((ts_to_rational(50, tb), ts_to_rational(150, tb)))
		);
		assert_eq!(
			span(tail),
			Some((ts_to_rational(150, tb), ts_to_rational(200, tb))),
			"the covered head is removed"
		);

		oak_undo::global::undo().unwrap();
		assert_eq!(
			span(tail),
			Some((ts_to_rational(100, tb), ts_to_rational(200, tb)))
		);
		assert_eq!(clip_ids(&lock(&project).graph, track), vec![head, tail]);

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

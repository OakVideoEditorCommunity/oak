//! Clipboard, placement, splitting, trimming and linked-move behavior tests.

use super::*;
	#[test]

	/// `copy_clips` skips non-clips, stale ids, trackless clips, non-track
	/// owners and listless tracks; `paste_clips` validates its arguments and
	/// installs the pasted group's links.
	pub(crate) fn copy_and_paste_cover_skip_and_error_branches() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("copy_paste");
		let dropped = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			0,
			10,
			0,
		)
		.expect("linked drop");
		let (video, audio) = (dropped[0], dropped[1]);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};

		// The clipboard captures the clip's speed.
		{
			let mut g = lock(&project);
			if let Some(c) = g
				.graph
				.get_mut(video)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<ClipBlockBehavior>())
			{
				c.core.speed = 2.0;
			}
		}
		let track = video_track_of(&project, seq);
		let adj = create_adjustment_layer(&project, seq, track, 50, 60).expect("adjustment");
		let bogus = id_of(9_000_010).unwrap();
		let items = copy_clips(&project, &[adj, bogus, video, audio]);
		assert_eq!(items.len(), 2, "only the two real clips are captured");
		assert!(
			items.iter().any(|i| i.speed == 2.0 && i.kind == TrackType::Video),
			"the speed travels with the clipboard clip"
		);

		// A trackless clip is skipped.
		let audio_track = {
			let g = lock(&project);
			clip_track(&g.graph, audio).expect("the audio track")
		};
		oak_timeline::util::track_ripple_remove_block(
			&node_ref(&project, audio_track),
			&node_ref(&project, audio),
		);
		assert!(
			copy_clips(&project, &[audio]).is_empty(),
			"a trackless clip is skipped"
		);

		// A clip whose owner is not a track is skipped.
		let folder = create_folder(&project, "Fake Track").expect("folder");
		set_clip_track(&project, video, Some(folder));
		assert!(
			copy_clips(&project, &[video]).is_empty(),
			"a non-track owner is skipped"
		);
		set_clip_track(&project, video, Some(track));

		// A track without a list, and a dangling list reference, are skipped.
		set_track_list(&project, track, None);
		assert!(
			copy_clips(&project, &[video]).is_empty(),
			"a listless track is skipped"
		);
		set_track_list(&project, track, Some(bogus));
		assert!(
			copy_clips(&project, &[video]).is_empty(),
			"a dangling list is skipped"
		);
		set_track_list(&project, track, Some(video_list_of(&project, seq)));

		// paste argument errors.
		assert!(paste_clips(&project, seq, &[], 0).is_err());
		let zero = create_sequence_with_params(
			&project,
			"Zero Paste",
			Some((10, 10, Rational::new(0, 1), false)),
		);
		assert!(
			paste_clips(&project, zero, &items, 0).is_err(),
			"a sequence without a valid frame rate is rejected"
		);
		let mut subtitle = items[0].clone();
		subtitle.kind = TrackType::Subtitle;
		assert!(
			paste_clips(&project, seq, &[subtitle], 0).is_err(),
			"no subtitle track list"
		);
		let list = video_list_of(&project, seq);
		let saved_tracks = {
			let mut g = lock(&project);
			let l = g
				.graph
				.get_mut(list)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackListBehavior>())
				.expect("list behavior");
			let saved = l.tracks.clone();
			l.tracks.clear();
			saved
		};
		let mut video_item = items[0].clone();
		video_item.kind = TrackType::Video;
		assert!(
			paste_clips(&project, seq, &[video_item], 0).is_err(),
			"no track of the clip's kind to paste onto"
		);
		{
			let mut g = lock(&project);
			if let Some(l) = g
				.graph
				.get_mut(list)
				.and_then(|e| e.behavior.as_any_mut())
				.and_then(|a| a.downcast_mut::<TrackListBehavior>())
			{
				l.tracks = saved_tracks;
			}
		}

		// A stale footage id fails the connect step.
		let mut bad = items[0].clone();
		bad.footage = bogus;
		assert!(paste_clips(&project, seq, &[bad], 0).is_err());

		// Success: the group is linked, the speed survives, and an
		// out-of-range track index clamps.
		let mut clamped = items[0].clone();
		clamped.track_index = 99;
		let pasted =
			paste_clips(&project, seq, &[clamped, items[1].clone()], 100).expect("paste");
		assert_eq!(pasted.len(), 2);
		{
			let g = lock(&project);
			assert!(
				g.graph.links_of(pasted[0]).contains(&pasted[1]),
				"the pasted group is linked"
			);
			assert_eq!(
				clip_behavior(&g.graph, pasted[0]).map(|c| c.core.speed),
				Some(2.0)
			);
			assert_eq!(
				rational_to_ts(clip_range(&g.graph, pasted[0]).unwrap().0, tb),
				100,
				"the paste lands at the playhead"
			);
			let video_tracks = track_ids(&g.graph, seq, TrackType::Video);
			assert_eq!(
				clip_track(&g.graph, pasted[0]),
				Some(*video_tracks.last().unwrap()),
				"the out-of-range track index clamps to the last track"
			);
		}
		oak_undo::global::undo().unwrap();
		assert!(!lock(&project).graph.are_linked(pasted[0], pasted[1]));
		oak_undo::global::redo().unwrap();
		assert!(lock(&project).graph.are_linked(pasted[0], pasted[1]));

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}

	/// Split / split-preserving-links, trim, ripple trim, roll, slide, slip
	/// and the same- and cross-track moves cover both their validation errors
	/// and their no-op clamps.
	#[test]
	pub(crate) fn split_trim_ripple_roll_slide_slip_and_move_paths() {
		let _g = test_lock();
		oak_undo::global::clear().unwrap();
		let (project, seq, footage, media) = project_with_footage("edit_ops");
		let track = video_track_of(&project, seq);
		let a = place_clip(&project, seq, footage, 0, 10);
		let b = place_clip(&project, seq, footage, 10, 20);
		let c = place_clip(&project, seq, footage, 20, 30);
		let tb = {
			let g = lock(&project);
			sequence_time_base(&g.graph, seq).expect("time base")
		};
		let bogus = id_of(9_000_011).unwrap();

		// split: stale / edge frames / success.
		assert!(split_clip(&project, bogus, 5).is_err());
		assert!(split_clip(&project, a, 0).is_err());
		assert!(split_clip(&project, a, 10).is_err());
		split_clip(&project, a, 5).expect("split");
		assert_eq!(clip_ids(&lock(&project).graph, track).len(), 4);
		oak_undo::global::undo().unwrap();
		assert_eq!(clip_ids(&lock(&project).graph, track).len(), 3);

		// split preserving links: empty input is a no-op, a linked pair
		// splits as one row.
		assert!(split_clips_preserving_links(&project, &[], 5).is_ok());
		let pair = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			40,
			50,
			0,
		)
		.expect("linked pair");
		split_clips_preserving_links(&project, &pair, 45).expect("split the pair");
		oak_undo::global::undo().unwrap();

		// trim: invalid, unchanged, both ends, in-only and out-only.
		assert!(trim_clip(&project, a, -1, 5).is_err());
		assert!(trim_clip(&project, a, 5, 5).is_err());
		assert!(trim_clip(&project, bogus, 0, 5).is_err());
		trim_clip(&project, a, 0, 10).expect("unchanged trim");
		trim_clip(&project, a, 2, 8).expect("trim both ends");
		assert_eq!(
			clip_range(&lock(&project).graph, a),
			Some((
				ts_to_rational(2, tb),
				ts_to_rational(8, tb),
				ts_to_rational(2, tb),
			)),
			"the trim-in shifts the media in-point with the edge"
		);
		oak_undo::global::undo().unwrap();
		assert_eq!(
			clip_range(&lock(&project).graph, a),
			Some((Rational::new(0, 1), ts_to_rational(10, tb), Rational::new(0, 1)))
		);
		trim_clip(&project, a, 2, 10).expect("in-only trim");
		oak_undo::global::undo().unwrap();
		trim_clip(&project, a, 0, 8).expect("out-only trim");
		oak_undo::global::undo().unwrap();

		// ripple trim: null delta, trim-in (tail follows right), trim-out
		// (tail follows left).
		assert!(ripple_trim_clip(&project, bogus, true, 5).is_err());
		ripple_trim_clip(&project, b, true, 10).expect("null delta");
		ripple_trim_clip(&project, b, true, 12).expect("ripple in");
		assert_eq!(
			clip_range(&lock(&project).graph, b).unwrap().0,
			ts_to_rational(12, tb)
		);
		assert_eq!(
			clip_range(&lock(&project).graph, c).unwrap().0,
			ts_to_rational(22, tb),
			"the tail shifted right"
		);
		oak_undo::global::undo().unwrap();
		ripple_trim_clip(&project, b, false, 18).expect("ripple out");
		assert_eq!(
			clip_range(&lock(&project).graph, b).unwrap().1,
			ts_to_rational(18, tb)
		);
		assert_eq!(
			clip_range(&lock(&project).graph, c).unwrap().0,
			ts_to_rational(18, tb),
			"the tail followed the out edge"
		);
		oak_undo::global::undo().unwrap();

		// roll edit: non-clip, non-adjacent, escaping and unmoved boundaries,
		// then the real roll.
		assert!(roll_edit(&project, track, footage, b, 15).is_err());
		assert!(roll_edit(&project, track, a, c, 15).is_err());
		assert!(roll_edit(&project, track, a, b, 0).is_err());
		assert!(roll_edit(&project, track, a, b, 10).is_ok(), "unmoved boundary");
		roll_edit(&project, track, a, b, 12).expect("roll");
		{
			// The roll moves the shared seam to 12: the left clip keeps its
			// in point and grows at its out (media window untouched), the
			// follower keeps its out point and its head is consumed (its
			// media in advances by the rolled amount).
			let g = lock(&project);
			assert_eq!(
				clip_range(&g.graph, a).unwrap(),
				(
					Rational::new(0, 1),
					ts_to_rational(12, tb),
					Rational::new(0, 1)
				)
			);
			assert_eq!(
				clip_range(&g.graph, b).unwrap(),
				(
					ts_to_rational(12, tb),
					ts_to_rational(20, tb),
					ts_to_rational(2, tb)
				),
				"the follower's head loses the 2 rolled frames from its media"
			);
		}
		oak_undo::global::undo().unwrap();

		// slide: unmoved clip, right into the neighbor, over a collapsing
		// left neighbor, and onto the right neighbor without trimming it.
		assert!(slide_clip(&project, bogus, 5).is_err());
		slide_clip(&project, b, 10).expect("unmoved slide");
		slide_clip(&project, b, 15).expect("slide right");
		{
			let g = lock(&project);
			assert_eq!(clip_range(&g.graph, b).unwrap().0, ts_to_rational(15, tb));
			assert_eq!(clip_range(&g.graph, a).unwrap().1, ts_to_rational(15, tb));
			assert_eq!(clip_range(&g.graph, c).unwrap().0, ts_to_rational(25, tb));
		}
		oak_undo::global::undo().unwrap();
		slide_clip(&project, b, 0).expect("slide onto zero");
		{
			let g = lock(&project);
			assert_eq!(clip_range(&g.graph, b).unwrap().0, Rational::new(0, 1));
			assert_eq!(
				clip_range(&g.graph, a).unwrap().1,
				ts_to_rational(10, tb),
				"the collapsing left neighbor is left alone"
			);
		}
		oak_undo::global::undo().unwrap();
		slide_clip(&project, b, 20).expect("slide onto the right neighbor");
		{
			let g = lock(&project);
			assert_eq!(clip_range(&g.graph, b).unwrap().0, ts_to_rational(20, tb));
			assert_eq!(
				clip_range(&g.graph, c).unwrap().0,
				ts_to_rational(20, tb),
				"the right neighbor keeps its length"
			);
		}
		oak_undo::global::undo().unwrap();

		// slip: a negative request clamps to the old media in (a no-op),
		// otherwise only the media in-point moves.
		assert!(slip_clip(&project, bogus, 5).is_err());
		slip_clip(&project, b, -5).expect("clamped slip is a no-op");
		slip_clip(&project, b, 7).expect("slip");
		{
			let g = lock(&project);
			let (in_r, _, media_in) = clip_range(&g.graph, b).expect("b is a clip");
			assert_eq!(media_in, ts_to_rational(7, tb));
			assert_eq!(in_r, ts_to_rational(10, tb), "the range stays put");
		}
		oak_undo::global::undo().unwrap();

		// move: stale id, negative clamp, real move.
		assert!(move_clip(&project, bogus, 3).is_err());
		move_clip(&project, b, -50).expect("move clamps to zero");
		assert_eq!(
			clip_range(&lock(&project).graph, b).unwrap().0,
			Rational::new(0, 1)
		);
		oak_undo::global::undo().unwrap();
		move_clip(&project, b, 40).expect("move");
		assert_eq!(
			clip_range(&lock(&project).graph, b).unwrap().0,
			ts_to_rational(40, tb)
		);
		oak_undo::global::undo().unwrap();

		// move to track: stale clip, destination without a list, trackless
		// clip, then the cross-track move and its undo.
		let index = add_track(&project, seq, TrackType::Video).expect("third video track");
		let track2 = {
			let g = lock(&project);
			track_ids(&g.graph, seq, TrackType::Video)[index]
		};
		let folder = create_folder(&project, "Dest Folder").expect("folder");
		assert!(move_clip_to_track(&project, bogus, track2, 0).is_err());
		assert!(move_clip_to_track(&project, b, folder, 0).is_err());
		let d = place_clip(&project, seq, footage, 60, 70);
		let d_track = {
			let g = lock(&project);
			clip_track(&g.graph, d).expect("d on a track")
		};
		oak_timeline::util::track_ripple_remove_block(
			&node_ref(&project, d_track),
			&node_ref(&project, d),
		);
		assert!(move_clip_to_track(&project, d, track2, 0).is_err());
		move_clip_to_track(&project, b, track2, 30).expect("cross-track move");
		{
			let g = lock(&project);
			assert_eq!(clip_track(&g.graph, b), Some(track2));
			assert_eq!(clip_range(&g.graph, b).unwrap().0, ts_to_rational(30, tb));
		}
		oak_undo::global::undo().unwrap();

		// move with links: group to another track, the group-wide clamp at
		// zero, and ignoring self / stale / off-track linked ids.
		let pair = place_footage_clips_linked(
			&project,
			seq,
			footage,
			&[(TrackType::Video, 0), (TrackType::Audio, 0)],
			80,
			90,
			0,
		)
		.expect("linked pair");
		let (v2, a2) = (pair[0], pair[1]);
		move_clip_with_links(&project, v2, Some(track2), 100, &[a2]).expect("group move");
		{
			let g = lock(&project);
			assert_eq!(clip_track(&g.graph, v2), Some(track2));
			assert_eq!(clip_range(&g.graph, v2).unwrap().0, ts_to_rational(100, tb));
			assert_eq!(clip_range(&g.graph, a2).unwrap().0, ts_to_rational(100, tb));
		}
		oak_undo::global::undo().unwrap();
		move_clip_with_links(&project, v2, None, -50, &[a2]).expect("clamped group move");
		{
			let g = lock(&project);
			assert_eq!(clip_range(&g.graph, v2).unwrap().0, Rational::new(0, 1));
			assert_eq!(clip_range(&g.graph, a2).unwrap().0, Rational::new(0, 1));
		}
		oak_undo::global::undo().unwrap();
		move_clip_with_links(&project, v2, None, 20, &[v2, bogus, d])
			.expect("self, stale and off-track linked ids are ignored");
		oak_undo::global::undo().unwrap();

		let _ = std::fs::remove_file(&media);
		oak_undo::global::clear().unwrap();
	}


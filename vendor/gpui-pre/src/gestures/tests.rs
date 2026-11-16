    use super::*;
    use crate::point;

    #[test]
    fn ongoing_scroll_locks_to_dominant_axis() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Started, now);
        assert_eq!(ongoing_scroll.axis, Some(Axis::Horizontal));
        assert_eq!(horizontal_delta, point(px(10.), px(0.)));

        let mut continued_delta = point(px(3.), px(2.));
        ongoing_scroll.filter_at(
            &mut continued_delta,
            TouchPhase::Moved,
            now + Duration::from_millis(1),
        );
        assert_eq!(ongoing_scroll.axis, Some(Axis::Horizontal));
        assert_eq!(continued_delta, point(px(3.), px(0.)));
    }

    #[test]
    fn ongoing_scroll_unlocks_when_direction_changes() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Started, now);

        let mut vertical_delta = point(px(2.), px(10.));
        ongoing_scroll.filter_at(
            &mut vertical_delta,
            TouchPhase::Moved,
            now + Duration::from_millis(1),
        );
        assert_eq!(ongoing_scroll.axis, None);
        assert_eq!(vertical_delta, point(px(2.), px(10.)));
    }

    #[test]
    fn ongoing_scroll_starts_new_gesture_at_timeout_boundary() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Moved, now);

        let mut vertical_delta = point(px(2.), px(10.));
        ongoing_scroll.filter_at(
            &mut vertical_delta,
            TouchPhase::Moved,
            now + SCROLL_EVENT_SEPARATION,
        );
        assert_eq!(ongoing_scroll.axis, Some(Axis::Vertical));
        assert_eq!(vertical_delta, point(px(0.), px(10.)));
    }

    #[test]
    fn ongoing_scroll_ignores_zero_delta_and_resets_when_ended() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Started, now);

        let mut zero_delta = Point::default();
        ongoing_scroll.filter_at(
            &mut zero_delta,
            TouchPhase::Ended,
            now + Duration::from_millis(1),
        );
        assert_eq!(ongoing_scroll.axis, None);

        let mut vertical_delta = point(px(2.), px(3.));
        ongoing_scroll.filter_at(
            &mut vertical_delta,
            TouchPhase::Moved,
            now + Duration::from_millis(2),
        );
        assert_eq!(ongoing_scroll.axis, Some(Axis::Vertical));
        assert_eq!(vertical_delta, point(px(0.), px(3.)));
    }

    #[test]
    fn ongoing_scroll_ignores_zero_delta_movement() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Started, now);

        let mut zero_delta = Point::default();
        ongoing_scroll.filter_at(
            &mut zero_delta,
            TouchPhase::Moved,
            now + SCROLL_EVENT_SEPARATION,
        );

        let mut vertical_delta = point(px(2.), px(10.));
        ongoing_scroll.filter_at(
            &mut vertical_delta,
            TouchPhase::Moved,
            now + SCROLL_EVENT_SEPARATION,
        );
        assert_eq!(ongoing_scroll.axis, Some(Axis::Vertical));
        assert_eq!(vertical_delta, point(px(0.), px(10.)));
    }

    #[test]
    fn ongoing_scroll_supports_moved_only_platforms() {
        let now = Instant::now();
        let mut ongoing_scroll = OngoingScroll::default();
        let mut horizontal_delta = point(px(10.), px(2.));
        ongoing_scroll.filter_at(&mut horizontal_delta, TouchPhase::Moved, now);
        assert_eq!(ongoing_scroll.axis, Some(Axis::Horizontal));
        assert_eq!(horizontal_delta, point(px(10.), px(0.)));
    }

    #[test]
    fn touch_within_slop_resolves_to_tap() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        let recognized =
            recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 10., 10.), now);
        assert!(recognized.is_empty());
        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 12., 11.),
            now + Duration::from_millis(20),
        );
        assert!(recognized.is_empty());

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 12., 11.),
            now + Duration::from_millis(60),
        );
        let [RecognizedTouchGesture::Tap { down, up }] = recognized.as_slice() else {
            panic!("expected tap, got {recognized:?}");
        };
        assert_eq!(down.click_count, 1);
        assert_eq!(down.position, point(px(12.), px(11.)));
        assert_eq!(up.click_count, 1);
    }

    #[test]
    fn consecutive_taps_accumulate_tap_count() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();

        recognizer.handle_event_at(&touch_event(TouchId(1), TouchPhase::Started, 10., 10.), now);
        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Ended, 10., 10.),
            now + Duration::from_millis(40),
        );

        let second_down = now + Duration::from_millis(200);
        recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Started, 14., 10.),
            second_down,
        );
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Ended, 14., 10.),
            second_down + Duration::from_millis(40),
        );
        let [RecognizedTouchGesture::Tap { down, .. }] = recognized.as_slice() else {
            panic!("expected tap, got {recognized:?}");
        };
        assert_eq!(down.click_count, 2);

        let late_down = second_down + Duration::from_secs(2);
        recognizer.handle_event_at(
            &touch_event(TouchId(3), TouchPhase::Started, 14., 10.),
            late_down,
        );
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(3), TouchPhase::Ended, 14., 10.),
            late_down + Duration::from_millis(40),
        );
        let [RecognizedTouchGesture::Tap { down, .. }] = recognized.as_slice() else {
            panic!("expected tap, got {recognized:?}");
        };
        assert_eq!(down.click_count, 1);
    }

    #[test]
    fn touch_beyond_slop_resolves_to_pan() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 100., 120.),
            now + Duration::from_millis(16),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Started);
        assert_eq!(scroll.position, point(px(100.), px(100.)));
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(20.)));

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 100., 135.),
            now + Duration::from_millis(32),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Moved);
        assert_eq!(scroll.position, point(px(100.), px(100.)));
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(15.)));

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 100., 135.),
            now + Duration::from_millis(48),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Ended);
    }

    #[test]
    fn touch_pan_stays_locked_to_its_initial_dominant_axis() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 104., 120.),
            now + Duration::from_millis(16),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(20.)));

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 134., 125.),
            now + Duration::from_millis(32),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(5.)));
    }

    #[test]
    fn touch_pan_locks_to_horizontal_axis() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);

        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 120., 104.),
            now + Duration::from_millis(16),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(20.), px(0.)));
    }

    #[test]
    fn predicted_positions_lead_the_pan_but_totals_converge_on_release() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);

        // The first pan step scrolls to the predicted position, not the raw one.
        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 120.);
        moved.predicted_position = Some(point(px(106.), px(128.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(16));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(28.)));

        // The next step is measured from where the previous prediction left
        // the content, so an overshoot is paid back here.
        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 130.);
        moved.predicted_position = Some(point(px(104.), px(134.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(32));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(6.)));

        // A release without a fling (the finger stopped long before lifting)
        // targets the raw position: the total scrolled distance equals the
        // finger's actual travel despite the predictions.
        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 100., 130.),
            now + Duration::from_millis(120),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Ended);
        assert!(!recognizer.has_momentum());
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(-4.)));
    }

    #[test]
    fn predicted_positions_do_not_emit_false_reversals() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);

        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 120.);
        moved.predicted_position = Some(point(px(100.), px(130.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(16));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(30.)));

        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 125.);
        moved.predicted_position = Some(point(px(100.), px(127.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(32));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(
            scroll.delta.pixel_delta(px(16.)),
            Point::<Pixels>::default()
        );

        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 125.);
        moved.predicted_position = Some(point(px(100.), px(126.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(40));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(
            scroll.delta.pixel_delta(px(16.)),
            Point::<Pixels>::default()
        );

        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 132.);
        moved.predicted_position = Some(point(px(100.), px(136.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(48));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(6.)));

        let mut moved = touch_event(touch, TouchPhase::Moved, 100., 124.);
        moved.predicted_position = Some(point(px(100.), px(140.)));
        let recognized = recognizer.handle_event_at(&moved, now + Duration::from_millis(64));
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(-12.)));
    }

    #[test]
    fn predicted_overshoot_folds_into_the_fling_without_scrolling_backwards() {
        let now = Instant::now();
        let mut total_with_prediction = 0f32;
        let mut total_without_prediction = 0f32;
        for use_prediction in [true, false] {
            let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
            let mut total = 0f32;
            let mut drain = |recognized: &[RecognizedTouchGesture], upward_only: bool| {
                for gesture in recognized {
                    let RecognizedTouchGesture::Scroll(scroll) = gesture else {
                        panic!("expected scroll, got {gesture:?}");
                    };
                    let delta = scroll.delta.pixel_delta(px(16.)).y;
                    if upward_only {
                        assert!(
                            delta <= px(0.),
                            "content moved backwards by {delta:?} during an upward gesture"
                        );
                    }
                    total += f32::from(delta);
                }
            };

            recognizer.handle_event_at(
                &touch_event(TouchId(1), TouchPhase::Started, 100., 500.),
                now,
            );
            for step in 1..=5u64 {
                let raw_y = 500. - step as f32 * 40.;
                let mut moved = touch_event(TouchId(1), TouchPhase::Moved, 100., raw_y);
                if use_prediction {
                    moved.predicted_position = Some(point(px(100.), px(raw_y - 25.)));
                }
                let recognized =
                    recognizer.handle_event_at(&moved, now + Duration::from_millis(step * 16));
                drain(&recognized, use_prediction);
            }
            // The release leaves the emitted position 25px ahead of the raw
            // one; with prediction the correction must not scroll backwards.
            let recognized = recognizer.handle_event_at(
                &touch_event(TouchId(1), TouchPhase::Ended, 100., 300.),
                now + Duration::from_millis(90),
            );
            drain(&recognized, use_prediction);
            assert!(recognizer.has_momentum());
            let mut tick = now + Duration::from_millis(91);
            while recognizer.has_momentum() {
                if let Some(gesture) = recognizer.tick_momentum_at(tick) {
                    drain(&[gesture], use_prediction);
                }
                tick += Duration::from_millis(16);
            }

            if use_prediction {
                total_with_prediction = total;
            } else {
                total_without_prediction = total;
            }
        }
        // Folding the overshoot into the fling redistributes the travel but
        // must not change where the content comes to rest.
        assert!(
            (total_with_prediction - total_without_prediction).abs() < 0.01,
            "totals diverged: {total_with_prediction} vs {total_without_prediction}"
        );
    }

    #[test]
    fn fast_release_starts_momentum_that_decays_to_a_stop() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 300.), now);
        for step in 1..=5 {
            recognizer.handle_event_at(
                &touch_event(touch, TouchPhase::Moved, 100., 300. - step as f32 * 20.),
                now + Duration::from_millis(step * 16),
            );
        }
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 100., 200.),
            now + Duration::from_millis(6 * 16),
        );
        assert!(recognizer.has_momentum());

        let tick = now + Duration::from_millis(6 * 16 + 16);
        let recognized = recognizer.tick_momentum_at(tick);
        let Some(RecognizedTouchGesture::Scroll(scroll)) = recognized else {
            panic!("expected momentum scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Moved);
        assert_eq!(scroll.position, point(px(100.), px(300.)));
        let delta = scroll.delta.pixel_delta(px(16.));
        assert!(
            delta.y < px(0.),
            "momentum should continue upward, got {delta:?}"
        );
        // The least-squares fit may leave float residue on the motionless axis.
        assert!(
            delta.x.abs() < px(0.001),
            "expected no x motion, got {delta:?}"
        );

        let mut last_phase = TouchPhase::Moved;
        let mut ticks = 0;
        let mut time = tick;
        while recognizer.has_momentum() {
            time += Duration::from_millis(16);
            ticks += 1;
            assert!(ticks < 1000, "momentum never stopped");
            if let Some(RecognizedTouchGesture::Scroll(scroll)) = recognizer.tick_momentum_at(time)
            {
                last_phase = scroll.touch_phase;
            }
        }
        assert_eq!(last_phase, TouchPhase::Ended);
        assert!(recognizer.tick_momentum_at(time).is_none());
    }

    #[test]
    fn diagonal_release_flings_only_on_locked_axis() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 300.), now);
        for step in 1..=5 {
            let recognized = recognizer.handle_event_at(
                &touch_event(
                    touch,
                    TouchPhase::Moved,
                    100. + step as f32 * 3.,
                    300. - step as f32 * 20.,
                ),
                now + Duration::from_millis(step * 16),
            );
            let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
                panic!("expected scroll, got {recognized:?}");
            };
            assert_eq!(scroll.delta.pixel_delta(px(16.)).x, px(0.));
        }
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 115., 200.),
            now + Duration::from_millis(6 * 16),
        );
        assert!(recognizer.has_momentum());

        let mut time = now + Duration::from_millis(6 * 16);
        while recognizer.has_momentum() {
            time += Duration::from_millis(16);
            if let Some(RecognizedTouchGesture::Scroll(scroll)) = recognizer.tick_momentum_at(time)
            {
                let delta = scroll.delta.pixel_delta(px(16.));
                assert_eq!(delta.x, px(0.));
                assert!(delta.y <= px(0.));
            }
        }
    }

    #[test]
    fn slow_release_does_not_start_momentum() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 300.), now);
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 100., 280.),
            now + Duration::from_millis(16),
        );
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 100., 279.),
            now + Duration::from_millis(500),
        );
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 100., 279.),
            now + Duration::from_millis(600),
        );
        assert!(!recognizer.has_momentum());
    }

    #[test]
    fn new_touch_interrupts_momentum() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();

        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Started, 100., 300.),
            now,
        );
        for step in 1..=3 {
            recognizer.handle_event_at(
                &touch_event(
                    TouchId(1),
                    TouchPhase::Moved,
                    100.,
                    300. - step as f32 * 33.,
                ),
                now + Duration::from_millis(step * 16),
            );
        }
        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Ended, 100., 200.),
            now + Duration::from_millis(64),
        );
        assert!(recognizer.has_momentum());

        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Started, 100., 200.),
            now + Duration::from_millis(200),
        );
        assert!(!recognizer.has_momentum());
        let [
            RecognizedTouchGesture::Scroll(closing),
            RecognizedTouchGesture::Scroll(opening),
        ] = recognized.as_slice()
        else {
            panic!("expected closing and opening scrolls, got {recognized:?}");
        };
        assert_eq!(closing.touch_phase, TouchPhase::Ended);
        assert!(closing.delta.pixel_delta(px(16.)).is_zero());
        assert_eq!(opening.touch_phase, TouchPhase::Started);
        assert!(opening.delta.pixel_delta(px(16.)).is_zero());
    }

    #[test]
    fn catching_a_fling_pans_immediately_and_never_taps() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();

        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Started, 100., 300.),
            now,
        );
        for step in 1..=3 {
            recognizer.handle_event_at(
                &touch_event(
                    TouchId(1),
                    TouchPhase::Moved,
                    100.,
                    300. - step as f32 * 33.,
                ),
                now + Duration::from_millis(step * 16),
            );
        }
        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Ended, 100., 200.),
            now + Duration::from_millis(64),
        );
        assert!(recognizer.has_momentum());

        recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Started, 100., 200.),
            now + Duration::from_millis(200),
        );

        // A movement well within the slop scrolls immediately.
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Moved, 100., 197.),
            now + Duration::from_millis(216),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Moved);
        assert_eq!(scroll.delta.pixel_delta(px(16.)), point(px(0.), px(-3.)));

        // Releasing the catch is not a tap.
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Ended, 100., 197.),
            now + Duration::from_millis(232),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Ended);
    }

    #[test]
    fn cancelled_pan_emits_cancelled_scroll_and_no_tap() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let touch = TouchId(1);

        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);
        recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 100., 150.),
            now + Duration::from_millis(16),
        );
        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Cancelled, 100., 150.),
            now + Duration::from_millis(32),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected cancelled scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Cancelled);
        assert!(!recognizer.has_momentum());

        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 100., 100.), now);
        let recognized = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Cancelled, 100., 102.),
            now + Duration::from_millis(16),
        );
        assert!(recognized.is_empty(), "cancelled tap must not click");
    }

    #[test]
    fn concurrent_touches_are_ignored_while_one_is_active() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();

        recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Started, 100., 100.),
            now,
        );
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Started, 200., 200.),
            now + Duration::from_millis(8),
        );
        assert!(recognized.is_empty());
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Moved, 200., 300.),
            now + Duration::from_millis(16),
        );
        assert!(recognized.is_empty());
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(2), TouchPhase::Ended, 200., 300.),
            now + Duration::from_millis(24),
        );
        assert!(recognized.is_empty());

        // The first touch still resolves normally.
        let recognized = recognizer.handle_event_at(
            &touch_event(TouchId(1), TouchPhase::Moved, 100., 150.),
            now + Duration::from_millis(32),
        );
        let [RecognizedTouchGesture::Scroll(scroll)] = recognized.as_slice() else {
            panic!("expected scroll, got {recognized:?}");
        };
        assert_eq!(scroll.touch_phase, TouchPhase::Started);
    }

    #[test]
    fn spline_position_table_matches_the_bezier_curve() {
        let samples = friction_spline::spline_position_samples();
        // AOSP's initializer solves sample 0 numerically like every other
        // sample, so it lands within solver tolerance of zero, not at zero.
        assert!(samples[0].abs() < 1e-4);
        assert_eq!(samples[100], 1.);
        for window in samples.windows(2) {
            assert!(window[0] < window[1], "table must be strictly increasing");
        }
        // Each table entry must lie on the defining parametric Bezier: for
        // sample i there must be a curve parameter whose time component is
        // i/100 and whose position component is the stored value.
        for (i, &stored_position) in samples.iter().enumerate().take(100) {
            let alpha = i as f32 / 100.;
            let (mut lower, mut upper) = (0f32, 1f32);
            for _ in 0..50 {
                let middle = (lower + upper) / 2.;
                let (time, _) = friction_spline::bezier_time_and_position(middle);
                if time > alpha {
                    upper = middle;
                } else {
                    lower = middle;
                }
            }
            let (time, position) = friction_spline::bezier_time_and_position((lower + upper) / 2.);
            assert!(
                (time - alpha).abs() < 1e-4,
                "sample {i}: time {time} != {alpha}"
            );
            assert!(
                (position - stored_position).abs() < 1e-3,
                "sample {i}: position {position} != stored {stored_position}"
            );
        }
    }

    #[test]
    fn fling_curves_are_sane_for_both_physics() {
        for physics in [ScrollPhysics::ios(), ScrollPhysics::android()] {
            let slow = physics.fling_duration(500.);
            let fast = physics.fling_duration(4000.);
            assert!(slow > Duration::ZERO, "{physics:?}");
            assert!(fast > slow, "faster flings must coast longer: {physics:?}");

            let halfway = physics.fling_distance(4000., fast / 2);
            let total = physics.fling_distance(4000., fast);
            assert!(halfway > 0. && halfway < total, "{physics:?}");
            assert!(
                physics.fling_distance(4000., fast * 2) == total,
                "distance must not grow past the fling duration: {physics:?}"
            );
            assert!(
                physics.fling_distance(4000., fast) > physics.fling_distance(500., slow),
                "faster flings must travel further: {physics:?}"
            );
        }
    }

    #[test]
    fn momentum_is_frame_rate_independent() {
        // The same fling ticked at 60Hz and as one huge stalled frame must
        // cover identical ground.
        let total_distance_with_tick_length = |tick: Duration| -> f32 {
            let mut recognizer = TouchGestureRecognizer::new(GestureTuning {
                scroll_physics: ScrollPhysics::android(),
                ..GestureTuning::default()
            });
            let now = Instant::now();
            recognizer.handle_event_at(
                &touch_event(TouchId(1), TouchPhase::Started, 100., 500.),
                now,
            );
            for step in 1..=3 {
                recognizer.handle_event_at(
                    &touch_event(
                        TouchId(1),
                        TouchPhase::Moved,
                        100.,
                        500. - step as f32 * 40.,
                    ),
                    now + Duration::from_millis(step * 16),
                );
            }
            recognizer.handle_event_at(
                &touch_event(TouchId(1), TouchPhase::Ended, 100., 380.),
                now + Duration::from_millis(64),
            );
            assert!(recognizer.has_momentum());

            let mut total = 0f32;
            let mut time = now + Duration::from_millis(64);
            let mut guard = 0;
            while recognizer.has_momentum() {
                time += tick;
                guard += 1;
                assert!(guard < 10_000, "momentum never stopped");
                if let Some(RecognizedTouchGesture::Scroll(scroll)) =
                    recognizer.tick_momentum_at(time)
                {
                    total += f32::from(scroll.delta.pixel_delta(px(16.)).y);
                }
            }
            total
        };

        let smooth = total_distance_with_tick_length(Duration::from_millis(16));
        let stalled = total_distance_with_tick_length(Duration::from_secs(10));
        assert!(
            (smooth - stalled).abs() < 0.01,
            "expected identical fling distance, got {smooth} vs {stalled}"
        );
    }

    #[test]
    fn flick_velocity_reflects_release_speed_not_window_average() {
        // A uniformly accelerating flick: position grows quadratically, so
        // the speed at the newest sample (2·k·t) is twice the window
        // average (k·t). The estimator must report the former.
        let mut velocity_tracker = VelocityTracker::default();
        let start = Instant::now();
        for step in 0..=6 {
            let t = step as f32 * 0.016;
            velocity_tracker.push(
                start + Duration::from_millis(step * 16),
                point(px(0.), px(1000. * t * t)),
            );
        }
        let velocity = velocity_tracker.velocity();
        let release_speed = 2. * 1000. * 0.096;
        assert!(
            (velocity.y - release_speed).abs() < 1.,
            "expected ≈{release_speed} px/s at release, got {} px/s",
            velocity.y
        );
        assert_eq!(velocity.x, 0.);
    }

    #[test]
    fn samples_before_a_pause_do_not_contribute_velocity() {
        // Fast motion, then a hold longer than the stopped-finger gap, then
        // a slow nudge: only the motion after the pause describes the
        // release.
        let mut velocity_tracker = VelocityTracker::default();
        let start = Instant::now();
        velocity_tracker.push(start, point(px(0.), px(0.)));
        velocity_tracker.push(start + Duration::from_millis(16), point(px(0.), px(50.)));
        velocity_tracker.push(start + Duration::from_millis(80), point(px(0.), px(52.)));
        velocity_tracker.push(start + Duration::from_millis(96), point(px(0.), px(54.)));
        let velocity = velocity_tracker.velocity();
        assert!(
            velocity.y < 200.,
            "pre-pause motion leaked into the estimate: {} px/s",
            velocity.y
        );
    }

    #[test]
    fn claimed_touch_drag_emits_phased_stream_without_pan_or_tap() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        let now = Instant::now();
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 10., 20.), now);
        let Some(RecognizedTouchGesture::TouchDrag(started)) = recognizer.offer_touch_drag(touch)
        else {
            panic!("expected touch drag");
        };
        assert_eq!(started.phase, TouchPhase::Started);
        assert_eq!(started.start_position, point(px(10.), px(20.)));
        recognizer.resolve_touch_drag(true);

        let moved = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 40., 50.),
            now + Duration::from_millis(10),
        );
        let [RecognizedTouchGesture::TouchDrag(moved)] = moved.as_slice() else {
            panic!("expected moved touch drag, got {moved:?}");
        };
        assert_eq!(moved.phase, TouchPhase::Moved);
        assert_eq!(moved.position, point(px(40.), px(50.)));

        let ended = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 45., 55.),
            now + Duration::from_millis(20),
        );
        let [RecognizedTouchGesture::TouchDrag(ended)] = ended.as_slice() else {
            panic!("expected ended touch drag, got {ended:?}");
        };
        assert_eq!(ended.phase, TouchPhase::Ended);
        assert_eq!(ended.position, point(px(45.), px(55.)));
    }

    #[test]
    fn unclaimed_touch_drag_remains_a_pan_candidate() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        let now = Instant::now();
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 0., 0.), now);
        assert!(recognizer.offer_touch_drag(touch).is_some());
        recognizer.resolve_touch_drag(false);

        let moved = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 20., 0.),
            now + Duration::from_millis(10),
        );
        assert!(matches!(
            moved.as_slice(),
            [RecognizedTouchGesture::Scroll(ScrollWheelEvent {
                touch_phase: TouchPhase::Started,
                ..
            })]
        ));
    }

    #[test]
    fn claimed_long_press_emits_phased_stream_without_tap() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        let now = Instant::now();
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 10., 20.), now);
        let Some(RecognizedTouchGesture::LongPress(started)) = recognizer.offer_long_press(touch)
        else {
            panic!("expected long press");
        };
        assert_eq!(started.phase, TouchPhase::Started);
        assert_eq!(started.start_position, point(px(10.), px(20.)));
        recognizer.resolve_long_press(true);

        let moved = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 12., 21.),
            now + Duration::from_millis(510),
        );
        let [RecognizedTouchGesture::LongPress(moved)] = moved.as_slice() else {
            panic!("expected moved long press, got {moved:?}");
        };
        assert_eq!(moved.phase, TouchPhase::Moved);

        let ended = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 12., 21.),
            now + Duration::from_millis(520),
        );
        let [RecognizedTouchGesture::LongPress(ended)] = ended.as_slice() else {
            panic!("expected ended long press, got {ended:?}");
        };
        assert_eq!(ended.phase, TouchPhase::Ended);
    }

    #[test]
    fn unclaimed_long_press_remains_a_tap_candidate() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        let now = Instant::now();
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 10., 20.), now);
        assert!(recognizer.offer_long_press(touch).is_some());
        recognizer.resolve_long_press(false);

        let ended = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Ended, 10., 20.),
            now + Duration::from_millis(510),
        );
        assert!(matches!(
            ended.as_slice(),
            [RecognizedTouchGesture::Tap { .. }]
        ));
    }

    #[test]
    fn unclaimed_long_press_can_still_become_a_pan() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        let now = Instant::now();
        recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 0., 0.), now);
        assert!(recognizer.offer_long_press(touch).is_some());
        recognizer.resolve_long_press(false);

        let moved = recognizer.handle_event_at(
            &touch_event(touch, TouchPhase::Moved, 20., 0.),
            now + Duration::from_millis(510),
        );
        assert!(matches!(
            moved.as_slice(),
            [RecognizedTouchGesture::Scroll(ScrollWheelEvent {
                touch_phase: TouchPhase::Started,
                ..
            })]
        ));
    }

    #[test]
    fn long_press_offer_is_one_shot_and_specific_to_pending_touch() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        recognizer.handle_event(&touch_event(touch, TouchPhase::Started, 0., 0.));

        assert!(
            recognizer
                .handle_event(&touch_event(TouchId(2), TouchPhase::Moved, 20., 0.))
                .is_empty()
        );
        assert!(recognizer.offer_long_press(TouchId(2)).is_none());
        assert!(recognizer.offer_long_press(touch).is_some());
        assert!(recognizer.offer_long_press(touch).is_none());
    }

    #[test]
    fn long_press_cannot_be_offered_after_pending_touch_resolves() {
        for phase in [TouchPhase::Ended, TouchPhase::Cancelled, TouchPhase::Moved] {
            let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
            let touch = TouchId(1);
            let now = Instant::now();
            recognizer.handle_event_at(&touch_event(touch, TouchPhase::Started, 0., 0.), now);
            let position = if phase == TouchPhase::Moved { 20. } else { 0. };
            recognizer.handle_event_at(
                &touch_event(touch, phase, position, 0.),
                now + Duration::from_millis(10),
            );
            assert!(recognizer.offer_long_press(touch).is_none());
        }
    }

    #[test]
    fn claimed_long_press_emits_cancelled_for_its_touch_only() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let touch = TouchId(1);
        recognizer.handle_event(&touch_event(touch, TouchPhase::Started, 4., 5.));
        assert!(recognizer.offer_long_press(touch).is_some());
        recognizer.resolve_long_press(true);

        assert!(
            recognizer
                .handle_event(&touch_event(TouchId(2), TouchPhase::Cancelled, 9., 9.))
                .is_empty()
        );
        let cancelled = recognizer.handle_event(&touch_event(touch, TouchPhase::Cancelled, 6., 7.));
        let [RecognizedTouchGesture::LongPress(cancelled)] = cancelled.as_slice() else {
            panic!("expected cancelled long press, got {cancelled:?}");
        };
        assert_eq!(cancelled.phase, TouchPhase::Cancelled);
        assert_eq!(cancelled.start_position, point(px(4.), px(5.)));
        assert_eq!(cancelled.position, point(px(6.), px(7.)));
    }

    #[test]
    fn unrelated_touch_cannot_end_or_cancel_pending_touch() {
        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
            let touch = TouchId(1);
            recognizer.handle_event(&touch_event(touch, TouchPhase::Started, 4., 5.));

            assert!(
                recognizer
                    .handle_event(&touch_event(TouchId(2), phase, 9., 9.))
                    .is_empty()
            );
            assert!(recognizer.offer_long_press(touch).is_some());
        }
    }

    #[test]
    fn completed_touch_id_cannot_claim_replacement_touch() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let completed_touch = TouchId(1);
        let replacement_touch = TouchId(2);
        recognizer.handle_event(&touch_event(completed_touch, TouchPhase::Started, 0., 0.));
        recognizer.handle_event(&touch_event(completed_touch, TouchPhase::Cancelled, 0., 0.));
        recognizer.handle_event(&touch_event(replacement_touch, TouchPhase::Started, 5., 5.));

        assert!(recognizer.offer_long_press(completed_touch).is_none());
        assert!(recognizer.offer_long_press(replacement_touch).is_some());
    }

    fn touch_event(id: TouchId, phase: TouchPhase, x: f32, y: f32) -> TouchEvent {
        TouchEvent {
            id,
            phase,
            position: point(px(x), px(y)),
            predicted_position: None,
            force: None,
        }
    }

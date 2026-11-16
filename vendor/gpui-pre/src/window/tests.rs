    use std::{
        cell::{Cell, RefCell},
        path::PathBuf,
        rc::Rc,
        time::Duration,
    };

    use crate::{
        AnyWindowHandle, AppContext as _, Bounds, ContentMask, Context, DispatchPhase,
        DragMoveEvent, Empty, ExternalDragPayload, ExternalPaths, FileDragPaths, FileDropEvent,
        FocusHandle, InputEvent as _, InteractiveElement as _, IntoElement, KeyDownEvent,
        Keystroke, LongPressEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement,
        Pixels, PlatformInput, Point, Render, RequestFrameOptions, ScaledPixels,
        StatefulInteractiveElement as _, Styled, TestAppContext, TouchDragEvent, TouchEvent,
        TouchId, TouchPhase, Underline, UnderlineStyle, Window, WindowAppearance, WindowOptions,
        canvas, div, hsla, point, px, size,
    };

    #[cfg(feature = "profiler")]
    #[gpui::test]
    fn test_frame_requests_and_skips_are_journaled(cx: &mut TestAppContext) {
        use crate::FrameRequestSource;
        use crate::profiler::journal::{
            self, ForegroundJournalEntry, FrameSkipReason, IntervalBoundary,
        };
        use scheduler::Instant;

        cx.app.borrow_mut().mode = crate::app::GpuiMode::Production;
        let window = cx.add_window(|_, _| EmptyView);
        let platform_window = cx.test_window(window.into());
        platform_window.simulate_frame_request(RequestFrameOptions::default());
        let (journal, _guard) = journal::install_test_foreground_journal(64, 4);
        let mut collector = journal.collector();

        for (signal_at, source) in [
            (Some(Instant::now()), FrameRequestSource::NativeCallback),
            (Some(Instant::now()), FrameRequestSource::LocalSchedule),
            (None, FrameRequestSource::NativeCallback),
        ] {
            platform_window.simulate_frame_request(RequestFrameOptions {
                signal_at,
                signal_source: source,
                require_presentation: true,
                ..Default::default()
            });
            let entries = collector.collect_unseen().entries;
            let signal_index = entries
                .iter()
                .position(|entry| {
                    matches!(
                        entry,
                        ForegroundJournalEntry::PlatformSignal(signal)
                            if signal.window_id == window.window_id()
                                && signal.signal_at == signal_at
                                && signal.source == source
                                && signal_at.is_none_or(|at| signal.handled_at >= at)
                    )
                })
                .expect("frame request metadata");
            let skip_index = entries
                .iter()
                .position(|entry| {
                    matches!(
                        entry,
                        ForegroundJournalEntry::Boundary(IntervalBoundary::FrameSkipped(skipped))
                            if skipped.window_id == window.window_id()
                                && skipped.reason == FrameSkipReason::NoRenderNeeded
                    )
                })
                .expect("completed no-render boundary");
            assert!(signal_index < skip_index);
        }
    }

    #[cfg(feature = "profiler")]
    #[gpui::test]
    fn test_frame_demand_survives_throttling(cx: &mut TestAppContext) {
        use crate::profiler::journal::{
            self, ForegroundJournalEntry, FrameSkipReason, IntervalBoundary,
        };
        use scheduler::Instant;

        // Test mode draws during effect flushing, bypassing platform frame throttling.
        cx.app.borrow_mut().mode = crate::app::GpuiMode::Production;
        let (journal, _guard) = journal::install_test_foreground_journal(256, 4);
        let mut collector = journal.collector();
        for initially_dirty in [true, false] {
            // Keep the throttle closed without depending on short wall-clock deadlines.
            let window = cx
                .app
                .borrow_mut()
                .open_window(
                    WindowOptions {
                        inactive_frame_interval: Some(Duration::from_secs(3600)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| EmptyView),
                )
                .expect("open inactive test window");
            let platform_window = cx.test_window(window.into());
            platform_window.simulate_active_status_change(false);
            platform_window.simulate_frame_request(RequestFrameOptions::default());
            let callback_ran = Rc::new(Cell::new(false));
            let draws_before = window
                .update(cx, |_, window, _| {
                    let draws_before = window
                        .frame_duration_snapshot()
                        .draw_duration_histogram
                        .len();
                    if initially_dirty {
                        window.refresh();
                    }
                    window.on_next_frame({
                        let callback_ran = callback_ran.clone();
                        move |window, _| {
                            callback_ran.set(true);
                            window.refresh();
                        }
                    });
                    draws_before
                })
                .expect("schedule throttled frame");
            // Skips only seal retained work, so give the throttled skip some.
            let input_at = Instant::now();
            journal::record_input(journal::InputTiming {
                kind: "test",
                start: input_at,
                end: input_at,
                caused_invalidation: false,
            });
            collector.collect_unseen();
            platform_window.simulate_frame_request(RequestFrameOptions {
                signal_at: Some(Instant::now()),
                ..Default::default()
            });
            window
                .update(cx, |_, window, _| {
                    assert_eq!(
                        window
                            .frame_duration_snapshot()
                            .draw_duration_histogram
                            .len(),
                        draws_before,
                    );
                })
                .expect("inspect deferred frame");
            assert!(!callback_ran.get());
            assert!(collector.collect_unseen().entries.iter().any(|entry| {
                matches!(
                    entry,
                    ForegroundJournalEntry::Boundary(IntervalBoundary::FrameSkipped(skipped))
                        if skipped.window_id == window.window_id()
                            && skipped.reason == FrameSkipReason::InactiveFrameRateLimit
                )
            }));
            platform_window.simulate_frame_request(RequestFrameOptions {
                signal_at: Some(Instant::now()),
                require_presentation: true,
                ..Default::default()
            });
            window
                .update(cx, |_, window, _| {
                    assert_eq!(
                        window
                            .frame_duration_snapshot()
                            .draw_duration_histogram
                            .len(),
                        draws_before + 1,
                    );
                })
                .expect("inspect presented frame");
            assert!(callback_ran.get());
        }
    }

    /// Visibility transitions reach observers exactly once each, with the new
    /// state already stored on the window, and never wake the platform for a
    /// frame: the platform requests one itself when it resumes presenting.
    #[gpui::test]
    fn test_window_visibility(cx: &mut TestAppContext) {
        use crate::WindowVisibility;

        let window = cx.add_window(|_, _| EmptyView);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let _subscription = window
            .update(cx, {
                let observed = observed.clone();
                move |_, window, _| {
                    assert_eq!(window.visibility(), WindowVisibility::Visible);
                    assert!(window.is_visible());
                    window.observe_window_visibility(move |visibility, window, _| {
                        assert_eq!(window.visibility(), visibility);
                        observed.borrow_mut().push(visibility);
                    })
                }
            })
            .unwrap();
        let test_window = cx.test_window(window.into());
        let frame_wake_count = test_window.frame_wake_count();

        test_window.simulate_visibility_change(WindowVisibility::Hidden);
        assert_eq!(*observed.borrow(), [WindowVisibility::Hidden]);
        window
            .update(cx, |_, window, _| assert!(!window.is_visible()))
            .unwrap();

        // Platforms may report the same state again; observers only see changes.
        test_window.simulate_visibility_change(WindowVisibility::Hidden);
        assert_eq!(observed.borrow().len(), 1);

        test_window.simulate_visibility_change(WindowVisibility::Visible);
        assert_eq!(
            *observed.borrow(),
            [WindowVisibility::Hidden, WindowVisibility::Visible]
        );
        window
            .update(cx, |_, window, _| assert!(window.is_visible()))
            .unwrap();
        assert_eq!(test_window.frame_wake_count(), frame_wake_count);
    }

    /// App learns display state from the platform's display notifications,
    /// and a window from the platform's report of which display it is on.
    #[gpui::test]
    fn test_display_tracking(cx: &mut TestAppContext) {
        use crate::{DisplayChanges, DisplayEvent, DisplayId, Subscription};

        let sixty_hertz = Duration::from_secs(1) / 60;
        let one_hundred_twenty_hertz = Duration::from_secs(1) / 120;
        let one_hundred_forty_four_hertz = Duration::from_secs(1) / 144;

        let display_events = Rc::new(RefCell::new(Vec::new()));
        let _display_subscription = cx.update({
            let display_events = display_events.clone();
            move |cx| cx.observe_displays(move |event, _| display_events.borrow_mut().push(event))
        });

        let window = cx.add_window(|window, cx| DisplayTracker {
            refresh_intervals: Vec::new(),
            _subscription: cx.observe_window_display(
                window,
                |tracker: &mut DisplayTracker, window, cx| {
                    tracker.refresh_intervals.push(window.refresh_interval(cx));
                },
            ),
        });
        let window_notifications = Rc::new(RefCell::new(Vec::new()));
        let _window_subscription = window
            .update(cx, {
                let window_notifications = window_notifications.clone();
                move |_, window, cx| {
                    assert_eq!(window.refresh_interval(cx), Some(sixty_hertz));
                    window.observe_window_display(move |window, cx| {
                        window_notifications
                            .borrow_mut()
                            .push((window.display_id, window.refresh_interval(cx)));
                    })
                }
            })
            .unwrap();
        let test_window = cx.test_window(window.into());

        // The window may hear that it moved before App hears the display was
        // connected. App learns about the display before the window's
        // observers run, and the platform's later notification changes
        // nothing.
        cx.test_platform()
            .connect_display(DisplayId(2), Some(one_hundred_twenty_hertz));
        test_window.simulate_move_to_display(DisplayId(2));
        assert_eq!(
            window_notifications.take(),
            [(Some(DisplayId(2)), Some(one_hundred_twenty_hertz))]
        );
        assert_eq!(display_events.take(), [DisplayEvent::Added(DisplayId(2))]);
        cx.simulate_display_refresh_interval_change(DisplayId(2), Some(one_hundred_twenty_hertz));
        assert!(window_notifications.borrow().is_empty());

        // Platforms may report a move that isn't one.
        test_window.simulate_move_to_display(DisplayId(2));
        assert!(window_notifications.borrow().is_empty());

        cx.simulate_display_refresh_interval_change(
            DisplayId(2),
            Some(one_hundred_forty_four_hertz),
        );
        assert_eq!(
            window_notifications.take(),
            [(Some(DisplayId(2)), Some(one_hundred_forty_four_hertz))]
        );

        // Changes to a display the window isn't on don't notify it.
        cx.simulate_display_refresh_interval_change(DisplayId(1), Some(one_hundred_twenty_hertz));
        cx.simulate_display_removed(DisplayId(1));
        assert!(window_notifications.borrow().is_empty());

        assert_eq!(
            display_events.take(),
            [
                DisplayEvent::Changed {
                    id: DisplayId(2),
                    changes: DisplayChanges::REFRESH_INTERVAL,
                },
                DisplayEvent::Changed {
                    id: DisplayId(1),
                    changes: DisplayChanges::REFRESH_INTERVAL,
                },
                DisplayEvent::Removed(DisplayId(1)),
            ]
        );
        window
            .update(cx, |tracker, _, _| {
                assert_eq!(
                    tracker.refresh_intervals,
                    [
                        Some(one_hundred_twenty_hertz),
                        Some(one_hundred_forty_four_hertz)
                    ]
                );
            })
            .unwrap();

        struct DisplayTracker {
            refresh_intervals: Vec<Option<Duration>>,
            _subscription: Subscription,
        }

        impl Render for DisplayTracker {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                Empty
            }
        }
    }

    #[gpui::test]
    fn test_fully_visible_bounds_preserve_layout_viewport(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        let mut platform_window = cx.test_window(window.into());
        platform_window.simulate_resize(size(px(400.), px(800.)));
        window
            .update(cx, |_, window, _| {
                assert_eq!(
                    window.visual_viewport_bounds(),
                    Bounds::new(Point::default(), size(px(400.), px(800.)))
                );
                assert_eq!(
                    window.fully_visible_bounds(),
                    window.visual_viewport_bounds()
                );
            })
            .unwrap();

        platform_window.simulate_frame_request(RequestFrameOptions::default());
        let wakes = platform_window.frame_wake_count();
        let visual_bounds = Bounds::new(point(px(10.), px(40.)), size(px(380.), px(460.)));
        platform_window.simulate_visual_viewport_change(visual_bounds);
        assert!(platform_window.frame_wake_count() > wakes);
        platform_window.simulate_frame_request(RequestFrameOptions::default());
        let wakes = platform_window.frame_wake_count();
        platform_window.simulate_insets_change(crate::WindowInsets {
            safe_area: crate::Edges {
                top: px(60.),
                right: px(20.),
                bottom: px(30.),
                left: px(-10.),
            },
            ime: crate::Edges {
                bottom: px(350.),
                ..Default::default()
            },
        });
        assert!(platform_window.frame_wake_count() > wakes);
        window
            .update(cx, |_, window, _| {
                assert_eq!(window.viewport_size(), size(px(400.), px(800.)));
                assert_eq!(window.visual_viewport_bounds(), visual_bounds);
                assert_eq!(
                    window.fully_visible_bounds(),
                    Bounds::new(point(px(10.), px(60.)), size(px(370.), px(390.)))
                );
                window.request_virtual_keyboard();
                window.dismiss_virtual_keyboard();
                assert_eq!(window.viewport_size(), size(px(400.), px(800.)));
            })
            .unwrap();
        assert_eq!(platform_window.virtual_keyboard_requests(), 1);
        assert_eq!(platform_window.virtual_keyboard_dismissals(), 1);

        platform_window.simulate_insets_change(crate::WindowInsets {
            safe_area: crate::Edges {
                top: px(900.),
                left: px(500.),
                ..Default::default()
            },
            ..Default::default()
        });
        window
            .update(cx, |_, window, _| {
                assert_eq!(
                    window.fully_visible_bounds().size,
                    size(Pixels::ZERO, Pixels::ZERO)
                );
            })
            .unwrap();
    }

    struct EmptyView;

    impl Render for EmptyView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct OpensWindowOnPaint {
        opened: Rc<Cell<bool>>,
    }

    impl Render for OpensWindowOnPaint {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            div()
                .size_full()
                .child(canvas(
                    |_, _, _| {},
                    move |_, _, _window, cx| {
                        if !opened.replace(true) {
                            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyView))
                                .unwrap();
                        }
                    },
                ))
                // Siblings painted after the canvas: their elements were
                // allocated in the arena before the nested draw, so they detect
                // a mid-draw arena clear when painted afterwards.
                .child(div().child("after"))
        }
    }

    /// Opening a window synchronously draws it and requests an element arena
    /// clear. When that happens from within another window's draw (here: from
    /// an element's paint), the clear must be deferred until the outer draw
    /// finishes, or the outer draw's arena-allocated elements would be freed
    /// out from under it.
    #[test]
    fn test_window_opened_during_draw_defers_arena_clear() {
        let mut cx = TestAppContext::single();

        let opened = Rc::new(Cell::new(false));
        // add_window draws once, which runs the nested open_window mid-draw.
        let window = cx.add_window({
            let opened = opened.clone();
            move |_, _| OpensWindowOnPaint { opened }
        });

        assert!(opened.get());
        assert_eq!(cx.windows().len(), 2);

        // The deferred clear must actually run once the outer draw unwinds:
        // subsequent draws of both windows work against a fresh arena.
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    #[test]
    fn test_scale_factor_change_preserves_bounds_and_survives_resize() {
        let mut cx = TestAppContext::single();
        let window = cx.add_window(|_, _| EmptyView);
        let handle: AnyWindowHandle = window.into();
        let window_state = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, _| {
                (
                    window.scale_factor(),
                    window.bounds(),
                    window.viewport_size(),
                )
            })
            .unwrap()
        };

        let (scale_factor, mut expected_bounds, _) = window_state(&mut cx);
        assert_eq!(scale_factor, 2.0);

        for (scale_factor, resized_size) in [
            (1.0, size(px(800.), px(600.))),
            (1.25, size(px(640.), px(480.))),
            (2.0, size(px(1024.), px(768.))),
        ] {
            cx.simulate_window_scale_factor_change(handle, scale_factor);
            assert_eq!(
                window_state(&mut cx),
                (scale_factor, expected_bounds, expected_bounds.size)
            );

            cx.simulate_window_resize(handle, resized_size);
            expected_bounds.size = resized_size;
            assert_eq!(
                window_state(&mut cx),
                (scale_factor, expected_bounds, resized_size)
            );
        }
    }

    /// Platforms that stop requesting frames for idle windows (currently web)
    /// rely on the frame waker firing whenever frame demand arises; a demand
    /// source that skips the waker shows up there as a window that silently
    /// stops repainting until unrelated activity wakes it.
    #[gpui::test]
    fn test_frame_waker_fires_on_frame_demand(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        let test_window = cx.test_window(window.into());

        // Windows start dirty, and that can predate waker installation;
        // installing the waker must deliver the pending wake or the first
        // frame would never be requested.
        assert!(
            test_window.frame_wake_count() >= 1,
            "opening a window must wake the frame source for the initial frame"
        );

        // Serve outstanding demand (present the frame drawn by `add_window`).
        test_window.simulate_frame_request(RequestFrameOptions::default());

        // An idle window must not wake on clean frames or plain updates, or
        // the frame source could never stop.
        let baseline = test_window.frame_wake_count();
        test_window.simulate_frame_request(RequestFrameOptions::default());
        window.update(cx, |_, _, _| {}).unwrap();
        assert_eq!(
            test_window.frame_wake_count(),
            baseline,
            "clean frames and non-notifying updates must not wake the frame source"
        );

        // Notifying a view in an idle window is the core demand signal.
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        assert!(
            test_window.frame_wake_count() > baseline,
            "notifying a view in an idle window must wake the frame source"
        );

        // Serving that demand returns to idle without further wakes.
        test_window.simulate_frame_request(RequestFrameOptions::default());
        let baseline = test_window.frame_wake_count();
        test_window.simulate_frame_request(RequestFrameOptions::default());
        assert_eq!(
            test_window.frame_wake_count(),
            baseline,
            "serving demand must return the window to idle"
        );

        // Next-frame callbacks create demand without dirtying the window.
        window
            .update(cx, |_, window, _| window.on_next_frame(|_, _| {}))
            .unwrap();
        assert!(
            test_window.frame_wake_count() > baseline,
            "scheduling a next-frame callback in an idle window must wake the frame source"
        );
    }

    /// A frame request that arrives while next-frame callbacks are pending
    /// must never strand them: either the frame runs them, or (when the
    /// inactive-window frame-rate throttle defers the frame) the waker fires
    /// so another request is delivered.
    #[gpui::test]
    fn test_pending_next_frame_callbacks_are_not_stranded(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        let test_window = cx.test_window(window.into());
        // Establish a recent last-frame time so the inactive-window throttle
        // can engage on the next request.
        test_window.simulate_frame_request(RequestFrameOptions::default());

        let callback_ran = Rc::new(Cell::new(false));
        window
            .update(cx, {
                let callback_ran = callback_ran.clone();
                move |_, window, _| {
                    window.on_next_frame(move |_, _| callback_ran.set(true));
                }
            })
            .unwrap();

        let baseline = test_window.frame_wake_count();
        test_window.simulate_frame_request(RequestFrameOptions::default());
        // The test window is inactive, so this request throttles to ~30fps
        // when it lands within the throttle interval of the previous frame
        // (the common case here, but timing-dependent): the callback is
        // deferred and the waker must re-arm the frame source. On a slow run
        // the request instead lands outside the interval and runs the
        // callback directly.
        assert!(
            test_window.frame_wake_count() > baseline || callback_ran.get(),
            "a frame request with pending next-frame callbacks must either run them or re-arm the frame source"
        );
    }

    #[gpui::test]
    fn test_window_reports_no_raw_handle_instead_of_panicking(cx: &mut TestAppContext) {
        use raw_window_handle::{HandleError, HasDisplayHandle as _, HasWindowHandle as _};

        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                assert!(matches!(
                    window.window_handle(),
                    Err(HandleError::NotSupported)
                ));
                assert!(matches!(
                    window.display_handle(),
                    Err(HandleError::NotSupported)
                ));
            })
            .unwrap();
    }

    #[gpui::test]
    fn test_appearance_change_runs_after_app_update(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        let observed_appearance = Rc::new(Cell::new(None));
        let _subscription = window
            .update(cx, {
                let observed_appearance = observed_appearance.clone();
                move |_, window, _| {
                    window.observe_window_appearance(move |window, _| {
                        observed_appearance.set(Some(window.appearance()));
                    })
                }
            })
            .unwrap();
        let test_window = cx.test_window(window.into());

        cx.update(|_| {
            test_window.simulate_appearance_change(WindowAppearance::Dark);
            assert_eq!(observed_appearance.get(), None);
        });
        cx.run_until_parked();

        assert_eq!(observed_appearance.get(), Some(WindowAppearance::Dark));
    }

    #[gpui::test]
    fn queued_frame_callback_wakes_a_parked_render_loop(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        let test_window = cx.test_window(window.into());

        assert!(test_window.simulate_scheduled_frame());
        assert!(test_window.simulate_scheduled_frame());
        assert!(!test_window.frame_scheduled());

        cx.update_window(window.into(), |_, window, _| {
            window.active.set(true);
            window.on_next_frame(|_, _| {});
        })
        .unwrap();
        assert!(
            test_window.frame_scheduled(),
            "queuing work on a parked window must wake the render loop"
        );

        assert!(test_window.simulate_scheduled_frame());
        assert!(
            test_window.frame_scheduled(),
            "presenting the frame must await one compositor callback"
        );
        assert!(test_window.simulate_scheduled_frame());
        assert!(!test_window.frame_scheduled());
    }

    #[gpui::test]
    fn pending_presentation_wakes_a_parked_render_loop(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        let test_window = cx.test_window(window.into());

        assert!(test_window.simulate_scheduled_frame());
        assert!(test_window.simulate_scheduled_frame());
        assert!(!test_window.frame_scheduled());

        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        assert!(
            test_window.frame_scheduled(),
            "a rendered scene awaiting presentation must wake the render loop"
        );
    }

    #[gpui::test]
    fn callback_queued_during_a_frame_requests_a_follow_up(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        let test_window = cx.test_window(window.into());

        let callback_ran = Rc::new(Cell::new(false));
        cx.update_window(window.into(), |_, window, _| {
            // Inactive windows are frame-rate throttled, which would defer the
            // ticks this test drives manually.
            window.active.set(true);
            let callback_ran = callback_ran.clone();
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |_, _| callback_ran.set(true));
            });
        })
        .unwrap();

        assert!(test_window.simulate_scheduled_frame());
        assert!(!callback_ran.get());
        assert!(
            test_window.frame_scheduled(),
            "a callback queued mid-frame must schedule a follow-up before the loop parks"
        );

        assert!(test_window.simulate_scheduled_frame());
        assert!(callback_ran.get());
    }

    struct RootView {
        explicit_size: bool,
        child_bounds: Rc<Cell<Bounds<Pixels>>>,
    }

    impl Render for RootView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let child_bounds = self.child_bounds.clone();
            let root = div().flex().flex_col().child(
                canvas(
                    move |bounds, _, _| child_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .size_full(),
            );
            if self.explicit_size {
                root.w(px(300.)).h(px(200.))
            } else {
                root
            }
        }
    }

    #[test]
    fn auto_sized_window_root_fills_the_window() {
        let mut cx = TestAppContext::single();
        let child_bounds = Rc::new(Cell::new(Bounds::default()));
        let window = cx.add_window({
            let child_bounds = child_bounds.clone();
            move |_, _| RootView {
                explicit_size: false,
                child_bounds,
            }
        });

        let viewport_size = cx
            .update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                window.viewport_size()
            })
            .unwrap();

        assert_eq!(child_bounds.get().size, viewport_size);
    }

    #[test]
    fn explicitly_sized_window_root_keeps_its_size() {
        let mut cx = TestAppContext::single();
        let child_bounds = Rc::new(Cell::new(Bounds::default()));
        let window = cx.add_window({
            let child_bounds = child_bounds.clone();
            move |_, _| RootView {
                explicit_size: true,
                child_bounds,
            }
        });

        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();

        assert_eq!(child_bounds.get().size, size(px(300.), px(200.)));
    }

    struct FileDragView {
        path: PathBuf,
        observed_drag_moves: Rc<RefCell<Vec<Point<Pixels>>>>,
        observed_drops: Rc<RefCell<Vec<PathBuf>>>,
    }

    struct FileDropExitView(Rc<Cell<usize>>);

    impl Render for FileDropExitView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().on_file_drop_exit({
                let observed_file_drop_exit = self.0.clone();
                move |_, _, _| observed_file_drop_exit.set(observed_file_drop_exit.get() + 1)
            })
        }
    }

    impl Render for FileDragView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("file-drag")
                .size_full()
                .on_drag(self.path.clone(), |_, _, _, cx| cx.new(|_| Empty))
                .external_drag_payload(|path: &PathBuf, _, _| {
                    Some(ExternalDragPayload::Files(FileDragPaths::new([(
                        path.clone(),
                        true,
                    )])))
                })
                .on_drag_move({
                    let observed_drag_moves = self.observed_drag_moves.clone();
                    move |event: &DragMoveEvent<PathBuf>, _, _| {
                        observed_drag_moves.borrow_mut().push(event.event.position);
                    }
                })
                .on_drop({
                    let observed_drops = self.observed_drops.clone();
                    move |path: &PathBuf, _, _| observed_drops.borrow_mut().push(path.clone())
                })
        }
    }

    #[gpui::test]
    fn file_drag_is_promoted_once_and_restored_in_source_window(cx: &mut TestAppContext) {
        struct Drag {
            window: AnyWindowHandle,
            observed_drag_moves: Rc<RefCell<Vec<Point<Pixels>>>>,
            observed_drops: Rc<RefCell<Vec<PathBuf>>>,
        }

        fn start_drag(cx: &mut TestAppContext, path: PathBuf, platform_result: bool) -> Drag {
            let observed_drag_moves = Rc::new(RefCell::new(Vec::new()));
            let observed_drops = Rc::new(RefCell::new(Vec::new()));
            let window: AnyWindowHandle = cx
                .add_window({
                    let observed_drag_moves = observed_drag_moves.clone();
                    let observed_drops = observed_drops.clone();
                    move |_, _| FileDragView {
                        path,
                        observed_drag_moves,
                        observed_drops,
                    }
                })
                .into();
            cx.test_window(window)
                .set_start_external_drag_result(platform_result);

            let update_result = cx.update_window(window, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    MouseDownEvent {
                        position: point(px(10.), px(10.)),
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(20.), px(20.)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                assert!(cx.active_drag.is_some());
            });
            assert!(
                update_result.is_ok(),
                "failed to start drag: {update_result:?}"
            );

            assert!(cx.test_window(window).external_drag_files().is_empty());
            Drag {
                window,
                observed_drag_moves,
                observed_drops,
            }
        }

        let successful_path = PathBuf::from("/tmp/successful-drag");
        let successful = start_drag(cx, successful_path.clone(), true);
        let outside_position = point(px(-1.), px(20.));
        let update_result = cx.update_window(successful.window, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: outside_position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(cx.active_drag.is_none());
        });
        assert!(
            update_result.is_ok(),
            "failed to promote drag: {update_result:?}"
        );
        assert_eq!(
            cx.test_window(successful.window).external_drag_files(),
            [(successful_path.clone(), true)]
        );
        // Views must still see the move that leaves the window, otherwise they never learn to tear
        // down the drag state they built up while the pointer was inside.
        assert_eq!(
            successful.observed_drag_moves.borrow().last(),
            Some(&outside_position)
        );

        let first_destination_exit_count = Rc::new(Cell::new(0));
        let first_destination: AnyWindowHandle = cx
            .add_window({
                let first_destination_exit_count = first_destination_exit_count.clone();
                move |_, _| FileDropExitView(first_destination_exit_count)
            })
            .into();
        let second_destination_exit_count = Rc::new(Cell::new(0));
        let second_destination: AnyWindowHandle = cx
            .add_window({
                let second_destination_exit_count = second_destination_exit_count.clone();
                move |_, _| FileDropExitView(second_destination_exit_count)
            })
            .into();
        let reentry_position = point(px(30.), px(30.));
        let external_paths = || ExternalPaths([successful_path.clone()].into_iter().collect());
        let update_result = cx.update_window(first_destination, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: external_paths(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<ExternalPaths>().is_some())
            );
            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
            assert!(cx.active_drag.is_none());
            assert_eq!(first_destination_exit_count.get(), 1);
            assert_eq!(second_destination_exit_count.get(), 0);
        });
        assert!(
            update_result.is_ok(),
            "failed to handle drag in first destination window: {update_result:?}"
        );

        let update_result = cx.update_window(second_destination, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse("down").expect("valid keystroke"),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: external_paths(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<ExternalPaths>().is_some())
            );
            assert_eq!(first_destination_exit_count.get(), 1);
            assert_eq!(second_destination_exit_count.get(), 0);

            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
            assert!(cx.active_drag.is_none());
            assert_eq!(first_destination_exit_count.get(), 1);
            assert_eq!(second_destination_exit_count.get(), 1);
        });
        assert!(
            update_result.is_ok(),
            "failed to handle drag in second destination window: {update_result:?}"
        );

        let update_result = cx.update_window(successful.window, |_, window, cx| {
            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: external_paths(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<PathBuf>().is_some())
            );
            assert_eq!(
                successful.observed_drag_moves.borrow().last(),
                Some(&reentry_position)
            );

            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
            assert!(cx.active_drag.is_none());

            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: external_paths(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<PathBuf>().is_some())
            );

            window.dispatch_event(
                FileDropEvent::Submit {
                    position: reentry_position,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(
                successful.observed_drops.borrow().as_slice(),
                std::slice::from_ref(&successful_path)
            );
            assert!(cx.active_drag.is_none());

            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
            assert!(cx.active_drag.is_none());
            window.dispatch_event(FileDropEvent::Ended.to_platform_input(), cx);
            assert!(cx.active_drag.is_none());

            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: external_paths(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<ExternalPaths>().is_some())
            );
            window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
        });
        assert!(
            update_result.is_ok(),
            "failed to restore drag in source window: {update_result:?}"
        );

        let cancelled_path = PathBuf::from("/tmp/cancelled-drag");
        let cancelled = start_drag(cx, cancelled_path.clone(), true);
        let update_result = cx.update_window(cancelled.window, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: outside_position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(cx.active_drag.is_none());

            window.dispatch_event(
                FileDropEvent::Entered {
                    position: reentry_position,
                    paths: ExternalPaths([cancelled_path].into_iter().collect()),
                }
                .to_platform_input(),
                cx,
            );
            assert!(
                cx.active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.value.downcast_ref::<PathBuf>().is_some())
            );
            assert!(cx.stop_active_drag(window));
            assert!(cx.active_drag.is_none());
        });
        assert!(
            update_result.is_ok(),
            "failed to cancel restored drag: {update_result:?}"
        );
        assert!(!cx.update(|cx| cx.end_platform_drag(cancelled.window.window_id())));

        let removed_path = PathBuf::from("/tmp/removed-window-drag");
        let removed = start_drag(cx, removed_path, true);
        let removed_window_id = removed.window.window_id();
        let update_result = cx.update_window(removed.window, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: outside_position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert!(cx.active_drag.is_none());
            window.remove_window();
        });
        assert!(
            update_result.is_ok(),
            "failed to remove drag source window: {update_result:?}"
        );
        assert!(!cx.update(|cx| cx.end_platform_drag(removed_window_id)));

        let failed_path = PathBuf::from("/tmp/failed-drag");
        let failed = start_drag(cx, failed_path.clone(), false);
        let update_result = cx.update_window(failed.window, |_, window, cx| {
            for x_position in [-1., -2.] {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(x_position), px(20.)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            }
            assert!(cx.active_drag.is_some());
        });
        assert!(
            update_result.is_ok(),
            "failed to retain drag after platform failure: {update_result:?}"
        );
        assert_eq!(
            cx.test_window(failed.window).external_drag_files(),
            [(failed_path, true)]
        );
    }

    struct FocusForwarder {
        a: FocusHandle,
        b: FocusHandle,
    }

    impl Render for FocusForwarder {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().w(px(50.)).h(px(50.)).track_focus(&self.a))
                .child(div().w(px(50.)).h(px(50.)).track_focus(&self.b))
        }
    }

    /// When a focus listener moves focus again (e.g. a dock forwarding focus to its
    /// active panel), the resulting focus events must be dispatched without waiting
    /// for an unrelated redraw of the window.
    #[gpui::test]
    fn test_focus_moved_by_focus_listener_is_dispatched(cx: &mut TestAppContext) {
        let b_focus_count = Rc::new(Cell::new(0));
        let window = cx.add_window({
            let b_focus_count = b_focus_count.clone();
            move |window, cx| {
                let a = cx.focus_handle();
                let b = cx.focus_handle();
                cx.on_focus(&a, window, |this: &mut FocusForwarder, window, cx| {
                    let b = this.b.clone();
                    window.focus(&b, cx);
                })
                .detach();
                cx.on_focus(&b, window, move |_, _, _| {
                    b_focus_count.set(b_focus_count.get() + 1);
                })
                .detach();
                FocusForwarder { a, b }
            }
        });

        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.executor().run_until_parked();

        window
            .update(cx, |this, window, cx| {
                let a = this.a.clone();
                window.focus(&a, cx);
            })
            .unwrap();
        cx.executor().run_until_parked();

        window
            .update(cx, |this, window, _| {
                assert!(this.b.is_focused(window));
            })
            .unwrap();
        assert_eq!(b_focus_count.get(), 1);
    }

    #[gpui::test]
    fn claimed_touch_drag_receives_movement_and_release(cx: &mut TestAppContext) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let events = events.clone();
            move |_, _| TouchDragListener { events }
        });
        let touch = TouchId(1);

        dispatch_touch(window, cx, touch, TouchPhase::Started, 10.);
        dispatch_touch(window, cx, touch, TouchPhase::Moved, 30.);
        dispatch_touch(window, cx, touch, TouchPhase::Ended, 40.);

        assert_eq!(
            events.borrow().as_slice(),
            [
                (TouchPhase::Started, px(10.)),
                (TouchPhase::Moved, px(30.)),
                (TouchPhase::Ended, px(40.)),
            ]
        );
    }

    struct TouchDragListener {
        events: Rc<RefCell<Vec<(TouchPhase, Pixels)>>>,
    }

    impl Render for TouchDragListener {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let events = self.events.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.on_mouse_event(move |event: &TouchDragEvent, phase, window, _cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        events.borrow_mut().push((event.phase, event.position.x));
                        if event.phase == TouchPhase::Started {
                            window.prevent_default();
                        }
                    });
                },
            )
        }
    }

    #[gpui::test]
    fn long_press_is_claimed_only_when_started_prevents_default(cx: &mut TestAppContext) {
        for response in [
            LongPressResponse::PreventDefault,
            LongPressResponse::StopPropagation,
            LongPressResponse::None,
        ] {
            let phases = Rc::new(RefCell::new(Vec::new()));
            let window = cx.add_window({
                let phases = phases.clone();
                move |_, _| LongPressListener { phases, response }
            });
            dispatch_touch(window, cx, TouchId(1), TouchPhase::Started, 0.);
            cx.executor().advance_clock(Duration::from_millis(501));
            cx.executor().run_until_parked();
            window
                .update(cx, |_, window, _| {
                    assert_eq!(
                        window.long_press_capture.is_some(),
                        response == LongPressResponse::PreventDefault
                    );
                })
                .unwrap();
            dispatch_touch(window, cx, TouchId(1), TouchPhase::Moved, 2.);
            dispatch_touch(window, cx, TouchId(1), TouchPhase::Ended, 2.);
            window
                .update(cx, |_, window, _| {
                    assert!(window.long_press_capture.is_none());
                })
                .unwrap();

            let phases = phases.borrow();
            if response == LongPressResponse::PreventDefault {
                assert_eq!(
                    phases.as_slice(),
                    [TouchPhase::Started, TouchPhase::Moved, TouchPhase::Ended]
                );
            } else {
                assert_eq!(phases.as_slice(), [TouchPhase::Started]);
            }
        }
    }

    #[gpui::test]
    fn stale_default_prevention_does_not_claim_long_press(cx: &mut TestAppContext) {
        let phases = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let phases = phases.clone();
            move |_, _| LongPressListener {
                phases,
                response: LongPressResponse::None,
            }
        });
        window
            .update(cx, |_, window, _| {
                window.prevent_default();
            })
            .unwrap();

        dispatch_touch(window, cx, TouchId(1), TouchPhase::Started, 0.);
        cx.executor().advance_clock(Duration::from_millis(501));
        cx.executor().run_until_parked();
        dispatch_touch(window, cx, TouchId(1), TouchPhase::Moved, 2.);

        assert_eq!(phases.borrow().as_slice(), [TouchPhase::Started]);
    }

    #[gpui::test]
    fn resolved_touch_cancels_scheduled_long_press(cx: &mut TestAppContext) {
        for (phase, position) in [
            (TouchPhase::Ended, 0.),
            (TouchPhase::Cancelled, 0.),
            (TouchPhase::Moved, 20.),
        ] {
            let phases = Rc::new(RefCell::new(Vec::new()));
            let window = cx.add_window({
                let phases = phases.clone();
                move |_, _| LongPressListener {
                    phases,
                    response: LongPressResponse::PreventDefault,
                }
            });
            dispatch_touch(window, cx, TouchId(1), TouchPhase::Started, 0.);
            dispatch_touch(window, cx, TouchId(1), phase, position);
            cx.executor().advance_clock(Duration::from_millis(501));
            cx.executor().run_until_parked();

            assert!(phases.borrow().is_empty(), "{phase:?} allowed long press");
        }
    }

    #[gpui::test]
    fn stale_long_press_timer_cannot_affect_replacement_touch(cx: &mut TestAppContext) {
        let phases = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let phases = phases.clone();
            move |_, _| LongPressListener {
                phases,
                response: LongPressResponse::PreventDefault,
            }
        });
        let first_touch = TouchId(1);
        dispatch_touch(window, cx, first_touch, TouchPhase::Started, 0.);
        cx.executor().advance_clock(Duration::from_millis(250));
        dispatch_touch(window, cx, first_touch, TouchPhase::Cancelled, 0.);
        dispatch_touch(window, cx, TouchId(2), TouchPhase::Started, 10.);

        cx.executor().advance_clock(Duration::from_millis(251));
        cx.executor().run_until_parked();
        assert!(phases.borrow().is_empty());

        cx.executor().advance_clock(Duration::from_millis(250));
        cx.executor().run_until_parked();
        assert_eq!(phases.borrow().as_slice(), [TouchPhase::Started]);
    }

    #[gpui::test]
    fn test_underline_exclusions_preserve_device_geometry(cx: &mut TestAppContext) {
        test_underline_paint_at_scales(cx, |window| {
            let (numerator, denominator) = match window.scale_factor() {
                1.0 => (1, 1),
                1.25 => (5, 4),
                1.5 => (3, 2),
                2.0 => (2, 1),
                3.0 => (3, 1),
                scale => panic!("unexpected scale {scale}"),
            };
            let snap = |quarters: i32| {
                let scaled = quarters * numerator;
                let divisor = 4 * denominator;
                let rounded =
                    scaled.abs() / divisor + i32::from(2 * (scaled.abs() % divisor) > divisor);
                ScaledPixels((scaled.signum() * rounded) as f32)
            };
            let stroke = |quarters| {
                if quarters == 0 {
                    ScaledPixels(0.)
                } else {
                    snap(quarters).max(ScaledPixels(1.))
                }
            };

            for origin in [-101, -3, -2, -1, 0, 1, 2, 3, 101] {
                for width in [0, 1, 3, 84] {
                    for thickness in [0, 1, 4, 7] {
                        for wavy in [false, true] {
                            let style = UnderlineStyle {
                                thickness: px(thickness as f32 / 4.),
                                color: Some(hsla(0.25, 0.5, 0.75, 0.5)),
                                wavy,
                            };
                            let bounds = Bounds::new(
                                point(snap(origin), snap(-origin)),
                                size(
                                    stroke(width),
                                    stroke(thickness) * if wavy { 3. } else { 1. },
                                ),
                            );
                            let origin = point(px(origin as f32 / 4.), px(-origin as f32 / 4.));
                            let width = px(width as f32 / 4.);
                            assert_eq!(window.underline_bounds(origin, width, &style), bounds);
                            let original = paint_test_underlines(window, |window| {
                                window.paint_underline(origin, width, &style);
                            })
                            .to_vec();
                            assert_eq!(original.len(), usize::from(!bounds.is_empty()));
                            if let Some(underline) = original.first() {
                                assert_eq!(underline.bounds, bounds);
                                assert_eq!(underline.thickness, stroke(thickness));
                                assert_eq!(underline.wavy, wavy.into());
                                assert_eq!(underline.color, hsla(0.25, 0.5, 0.75, 0.5));
                            }

                            let underlines = paint_test_underlines(window, |window| {
                                window.paint_underline_with_exclusions(origin, width, &style, &[]);
                            });
                            assert_eq!(underlines.len(), original.len());
                            for (actual, original) in underlines.iter().zip(&original) {
                                assert_same_underline_geometry(actual, original);
                                assert_eq!(actual.content_mask, original.content_mask);
                            }

                            let left = bounds.left().0 as i32;
                            let right = bounds.right().0 as i32;
                            let exclusions = [
                                (15, 25),
                                (3, 7),
                                (5, 10),
                                (10, 12),
                                (0, 0),
                                (4, 2),
                                (-50, -1),
                                (70, 100),
                            ]
                            .map(|(start, end)| {
                                ScaledPixels((left + start) as f32)
                                    ..ScaledPixels((left + end) as f32)
                            });
                            let underlines = paint_test_underlines(window, |window| {
                                window.paint_underline_with_exclusions(
                                    origin,
                                    width,
                                    &style,
                                    &exclusions,
                                );
                            });
                            let expected = (left..right)
                                .filter(|column| {
                                    !bounds.is_empty()
                                        && !exclusions.iter().any(|span| {
                                            span.contains(&ScaledPixels(*column as f32))
                                        })
                                })
                                .collect::<Vec<_>>();
                            assert_eq!(underline_device_columns(underlines), expected);
                            for underline in underlines {
                                assert_same_underline_geometry(underline, &original[0]);
                            }
                        }
                    }
                }
            }
        });
    }

    #[gpui::test]
    fn test_underline_exclusions_preserve_helvetica_warning_edge(cx: &mut TestAppContext) {
        test_underline_paint_at_scales(cx, |window| {
            if window.scale_factor() != 2.0 {
                return;
            }
            let style = UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: true,
            };
            let origin = point(px(2.888_183_8), px(15.357_917));
            let width = px(23.848_145) - origin.x;
            let original = paint_test_underlines(window, |window| {
                window.paint_underline(origin, width, &style);
            })[0];
            assert_eq!(original.bounds.left(), ScaledPixels(6.));
            assert_eq!(original.bounds.right(), ScaledPixels(48.));
            let point_bounds = window.underline_bounds(origin, px(10.829_102), &style);
            assert_eq!(point_bounds.left(), ScaledPixels(6.));
            assert_eq!(point_bounds.right(), ScaledPixels(28.));

            let underlines = paint_test_underlines(window, |window| {
                window.paint_underline_with_exclusions(
                    origin,
                    width,
                    &style,
                    &[point_bounds.left()..point_bounds.right()],
                );
            });
            assert_eq!(underlines.len(), 1);
            assert_same_underline_geometry(&underlines[0], &original);
            assert_eq!(
                underline_device_columns(underlines),
                (28..48).collect::<Vec<_>>()
            );
        });
    }

    #[gpui::test]
    fn test_underline_exclusions_preserve_content_mask_and_opacity(cx: &mut TestAppContext) {
        test_underline_paint_at_scales(cx, |window| {
            let scale = window.scale_factor();
            let original_mask = window.content_mask();
            let original_opacity = window.element_opacity();
            let mask = ContentMask {
                bounds: Bounds::from_corners(point(px(4.), px(10.)), point(px(16.), px(12.))),
            };
            let style = UnderlineStyle {
                thickness: px(2.),
                color: Some(hsla(0.25, 0.5, 0.75, 0.5)),
                wavy: true,
            };
            let origin = point(px(-1.25), px(9.25));
            let width = px(24.);
            window.with_content_mask(Some(mask), |window| {
                window.with_element_opacity(Some(0.5), |window| {
                    window.with_element_opacity(Some(0.5), |window| {
                        let original = paint_test_underlines(window, |window| {
                            window.paint_underline(origin, width, &style);
                        })[0];
                        assert_eq!(original.color, hsla(0.25, 0.5, 0.75, 0.125));
                        let underlines = paint_test_underlines(window, |window| {
                            window.paint_underline_with_exclusions(
                                origin,
                                width,
                                &style,
                                &[ScaledPixels(6. * scale)..ScaledPixels(8. * scale)],
                            );
                        });
                        assert_eq!(underlines.len(), 2);
                        let expected = [(4. * scale, 6. * scale), (8. * scale, 16. * scale)].map(
                            |(left, right)| {
                                Bounds::from_corners(
                                    point(ScaledPixels(left), ScaledPixels((10. * scale).floor())),
                                    point(ScaledPixels(right), ScaledPixels((12. * scale).ceil())),
                                )
                            },
                        );
                        assert_eq!(
                            underlines
                                .iter()
                                .map(|underline| underline.content_mask.bounds)
                                .collect::<Vec<_>>(),
                            expected
                        );
                        for underline in underlines {
                            assert_same_underline_geometry(underline, &original);
                        }
                    });
                });
            });
            assert_eq!(window.content_mask(), original_mask);
            assert_eq!(window.element_opacity(), original_opacity);
        });
    }

    #[gpui::test]
    fn test_underline_exclusions_adversarial_permutations(cx: &mut TestAppContext) {
        test_underline_paint_at_scales(cx, |window| {
            let style = UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: true,
            };
            let origin = point(px(-4.), px(-1.));
            let width = px(8.);
            let left = (-4. * window.scale_factor()) as i32;
            let right = (4. * window.scale_factor()) as i32;
            let endpoints = [-16, -8, -1, 0, 1, 8, 16];
            for first_start in endpoints {
                for first_end in endpoints {
                    for second_start in endpoints {
                        for second_end in endpoints {
                            let exclusions = [first_start..first_end, second_start..second_end];
                            let expected = (left..right)
                                .filter(|column| {
                                    !exclusions.iter().any(|span| span.contains(column))
                                })
                                .collect::<Vec<_>>();
                            let exclusions = exclusions.map(|span| {
                                ScaledPixels(span.start as f32)..ScaledPixels(span.end as f32)
                            });
                            let underlines = paint_test_underlines(window, |window| {
                                window.paint_underline_with_exclusions(
                                    origin,
                                    width,
                                    &style,
                                    &exclusions,
                                );
                            });
                            assert_eq!(
                                underline_device_columns(underlines),
                                expected,
                                "{exclusions:?}"
                            );
                        }
                    }
                }
            }
        });
    }

    #[derive(Clone, Copy, PartialEq)]
    enum LongPressResponse {
        PreventDefault,
        StopPropagation,
        None,
    }

    struct LongPressListener {
        phases: Rc<RefCell<Vec<TouchPhase>>>,
        response: LongPressResponse,
    }

    impl Render for LongPressListener {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let entity = cx.entity();
            let phases = self.phases.clone();
            let response = self.response;
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        phases.borrow_mut().push(event.phase);
                        match response {
                            LongPressResponse::PreventDefault => {
                                window.capture_long_press(&entity);
                                window.prevent_default();
                            }
                            LongPressResponse::StopPropagation => cx.stop_propagation(),
                            LongPressResponse::None => {}
                        }
                    });
                },
            )
        }
    }

    struct UnderlineTestView(Rc<dyn Fn(&mut Window)>);

    impl Render for UnderlineTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paint = self.0.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.content_mask_stack.push(ContentMask {
                        bounds: Bounds::from_corners(
                            point(px(-1000.), px(-1000.)),
                            point(px(1000.), px(1000.)),
                        ),
                    });
                    paint(window);
                    window.content_mask_stack.pop();
                },
            )
            .size_full()
        }
    }

    fn test_underline_paint_at_scales(
        cx: &mut TestAppContext,
        paint: impl Fn(&mut Window) + 'static,
    ) {
        let window = cx.add_window(move |_, _| UnderlineTestView(Rc::new(paint)));
        for scale in [1., 1.25, 1.5, 2., 3.] {
            cx.simulate_window_scale_factor_change(window.into(), scale);
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
    }

    fn paint_test_underlines(window: &mut Window, paint: impl FnOnce(&mut Window)) -> &[Underline] {
        window.next_frame.scene.clear();
        paint(window);
        &window.next_frame.scene.underlines
    }

    fn assert_same_underline_geometry(actual: &Underline, expected: &Underline) {
        assert_eq!(actual.bounds, expected.bounds);
        assert_eq!(actual.thickness, expected.thickness);
        assert_eq!(actual.wavy, expected.wavy);
        assert_eq!(actual.color, expected.color);
        assert_eq!(actual.pad, expected.pad);
    }

    fn underline_device_columns(underlines: &[Underline]) -> Vec<i32> {
        underlines
            .iter()
            .flat_map(|underline| {
                let bounds = underline.bounds.intersect(&underline.content_mask.bounds);
                bounds.left().0 as i32..bounds.right().0 as i32
            })
            .collect()
    }

    fn dispatch_touch<T: 'static>(
        window: crate::WindowHandle<T>,
        cx: &mut TestAppContext,
        id: TouchId,
        phase: TouchPhase,
        x: f32,
    ) {
        window
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    TouchEvent {
                        id,
                        phase,
                        position: point(px(x), px(0.)),
                        predicted_position: None,
                        force: None,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
    }

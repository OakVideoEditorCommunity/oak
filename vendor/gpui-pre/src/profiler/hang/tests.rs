    use std::cell::Cell;
    use std::rc::Rc;
    use std::thread;
    use std::time::Duration;

    use proptest::prelude::*;
    use rand::prelude::*;
    use scheduler::SpawnTime;

    use crate::{
        self as gpui, AnyView, AppContext as _, Context, FocusHandle, InteractiveElement,
        IntoElement, Modifiers, MouseButton, ParentElement as _, Render, Styled, TestAppContext,
        VisualTestContext, Window, WindowId, div, point, px,
    };

    use crate::profiler::{
        ActionTiming, FrameTiming, PresentTiming, TaskTiming, ViewPhase, YieldTime,
    };

    use super::super::journal::{
        FRAME_DEADLINE, ForegroundEvent, ForegroundJournalEntry, FrameSkipReason, FrameSkipped,
        FrameSnapshot, FrameStateChange, InputTiming, IntervalBoundary, IntervalSealer,
        PlatformSignal, PollSummary, PresentedFrame, SmallPollFlush,
        install_test_foreground_journal, record_present,
    };
    use super::{
        HangDetector, HangIncident, HangTrigger, SerializedHangContributor, SerializedHangIncident,
    };

    actions!(hang_test, [HangyAction]);

    // Well above legitimate per-event work in a test app (layout of one div,
    // empty polls), well below the injected hangs.
    const HANG_THRESHOLD: Duration = Duration::from_millis(10);
    // Equal to the threshold, mirroring how production wires the detector
    // today.
    const FRAME_BUDGET: Duration = HANG_THRESHOLD;

    #[test]
    fn completed_skips_preserve_real_hangs_and_serialize_their_boundary() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(1);
        for discontinuous in [false, true] {
            for trigger in [HangTrigger::Threshold, HangTrigger::BlockingWork] {
                let mut sealer = IntervalSealer::new(startup);
                let mut entries = Vec::new();
                match trigger {
                    HangTrigger::Threshold => entries.push(ForegroundJournalEntry::Event(
                        task_poll_event(at(10), at(30)),
                    )),
                    _ => entries.extend([
                        ForegroundJournalEntry::Event(task_poll_event(at(10), at(16))),
                        ForegroundJournalEntry::Event(task_poll_event(at(20), at(26))),
                    ]),
                }
                if discontinuous {
                    entries.push(ForegroundJournalEntry::Discontinuity { lost: 1 });
                }
                entries.push(ForegroundJournalEntry::Boundary(
                    IntervalBoundary::FrameSkipped(FrameSkipped {
                        window_id,
                        at: at(60),
                        reason: FrameSkipReason::NoRenderNeeded,
                    }),
                ));
                let snapshots = sealer.push_entries(entries);
                let [snapshot] = snapshots.as_slice() else {
                    panic!("expected one snapshot, got {snapshots:?}");
                };
                let incident = HangIncident::detect(snapshot.clone(), HANG_THRESHOLD, FRAME_BUDGET);
                if trigger == HangTrigger::BlockingWork {
                    assert!(incident.is_none(), "short work isn't blocking");
                    continue;
                }
                let incident = incident.expect("real work still qualifies across a skip");
                assert_eq!(incident.trigger, trigger);
                let serialized = SerializedHangIncident::convert(startup, &incident, 10, None);
                assert_eq!(serialized.sealed_by, "frame_skipped");
                assert_eq!(serialized.dirty_to_present_ms, None);
                assert_eq!(serialized.start_ms, 10.0);
                assert_eq!(serialized.active_ms, 50.0);
                assert_eq!(serialized.journal_discontinuous, discontinuous);
                assert_eq!(serialized.event_count, incident.contributors.len());
                let json = serde_json::to_value(&serialized).expect("serialize incident");
                assert_eq!(json["sealed_by"], "frame_skipped");
                assert!(json["dirty_to_present_ms"].is_null());
            }
        }
    }

    #[test]
    fn delayed_platform_signals_without_work_are_not_hangs() {
        let start = scheduler::Instant::now();
        let window_id = WindowId::from(1);
        let mut sealer = IntervalSealer::new(start);
        let snapshots = sealer.push_entries([
            ForegroundJournalEntry::PlatformSignal(PlatformSignal {
                window_id,
                signal_at: Some(start),
                handled_at: start + Duration::from_secs(5),
                source: crate::FrameRequestSource::NativeCallback,
            }),
            ForegroundJournalEntry::Boundary(IntervalBoundary::FrameSkipped(FrameSkipped {
                window_id,
                at: start + Duration::from_secs(6),
                reason: FrameSkipReason::NoRenderNeeded,
            })),
        ]);
        assert!(
            snapshots.is_empty(),
            "a late signal without work seals nothing, got {snapshots:?}"
        );
    }

    /// A hang that outlives the frame deadline stays in one incident with
    /// the frame it starved: the deadline only unblocks idle boundaries, so
    /// the eventual presentation seals the hang together with its
    /// dirty-to-present association.
    #[test]
    fn a_hang_outliving_the_frame_deadline_keeps_its_frame_association() {
        let start = scheduler::Instant::now();
        let window_id = WindowId::from(0x51E17);
        let hang_end = start + FRAME_DEADLINE * 5;
        let presented_at = hang_end + Duration::from_millis(16);
        let (journal, _journal_guard) = install_test_foreground_journal(64, 8);
        let mut detector = HangDetector::new(journal, HANG_THRESHOLD, FRAME_BUDGET);

        let snapshots = detector.sealer.push_entries([
            ForegroundJournalEntry::FrameState(FrameStateChange::Pending {
                window_id,
                dirty_at: start,
            }),
            ForegroundJournalEntry::Event(task_poll_event(start, hang_end)),
        ]);
        assert!(snapshots.is_empty(), "nothing seals mid-hang");

        let snapshots = detector
            .sealer
            .push_entries([ForegroundJournalEntry::Boundary(
                IntervalBoundary::Presented(PresentedFrame {
                    frame: frame(window_id, presented_at),
                    presentation: PresentTiming {
                        window_id,
                        present_start: presented_at - Duration::from_millis(1),
                        present_end: presented_at,
                        animation_interval: None,
                    },
                }),
            )]);
        let [snapshot] = snapshots.as_slice() else {
            panic!("expected one presented snapshot, got {snapshots:?}");
        };
        let incident = HangIncident::detect(snapshot.clone(), HANG_THRESHOLD, FRAME_BUDGET)
            .expect("the hang qualifies");
        assert!(matches!(
            incident.contributors[0],
            ForegroundEvent::TaskPoll(timing) if timing.end.0 == hang_end
        ));
        assert!(matches!(
            incident.snapshot.boundary,
            IntervalBoundary::Presented(_)
        ));
    }

    #[test]
    fn serialized_incident_reports_presented_seal_fields() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(0xF1E1D);

        let presentation = PresentTiming {
            window_id,
            present_start: at(380),
            present_end: at(400),
            animation_interval: None,
        };
        let frame = FrameTiming {
            refresh_interval: None,
            signal_at: None,
            window_id,
            dirty_at: Some(at(100)),
            invalidations: 3,
            draw_start: at(350),
            draw_end: at(380),
            phases: Default::default(),
        };
        let snapshot = FrameSnapshot {
            interval_start: at(150),
            boundary: IntervalBoundary::Presented(PresentedFrame {
                frame,
                presentation,
            }),
            events: vec![
                task_poll_event(at(150), at(300)),
                ForegroundEvent::Action(ActionTiming {
                    name: "test::SlowAction",
                    start: at(300),
                    end: at(340),
                }),
                ForegroundEvent::Input(InputTiming {
                    kind: "test",
                    start: at(340),
                    end: at(345),
                    caused_invalidation: false,
                }),
                ForegroundEvent::Present(presentation),
            ],
            small_polls: vec![SmallPollFlush {
                summary: PollSummary {
                    count: 2,
                    total: Duration::from_millis(1),
                },
                since: at(105),
                until: at(145),
            }],
            dropped_events: 0,
            journal_discontinuous: false,
        };

        let incident =
            HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET).expect("has contributors");
        let serialized = SerializedHangIncident::convert(startup, &incident, 1, Some(at(50)));

        assert_eq!(serialized.phase, "steady");
        assert_eq!(serialized.trigger, HangTrigger::Threshold);
        // The frame's first invalidation anchors the active window, not the
        // interval start or the first contributor.
        assert_eq!(serialized.start_ms, 100.0);
        assert_eq!(serialized.active_ms, 300.0);
        assert_eq!(serialized.stall_ms, 150.0);
        assert_eq!(serialized.dirty_to_present_ms, Some(300.0));
        assert_eq!(serialized.sealed_by, "present");
        // Occupancy: poll 150ms + action 40ms + input 5ms + present 20ms +
        // folded polls 1ms = 216ms of the 300ms window.
        assert_eq!(serialized.busy_fraction, 0.72);
        assert_eq!(serialized.event_count, 4);
        assert_eq!(serialized.small_poll_count, 2);
        assert_eq!(serialized.small_poll_total_ms, 1.0);
        assert_eq!(serialized.dropped_events, 0);
        // Three contributors qualify (poll, action, present); the cap keeps
        // the longest and counts the rest.
        assert_eq!(serialized.contributors.len(), 1);
        assert_eq!(serialized.contributors_elided, 2);
        assert!(matches!(
            serialized.contributors[0],
            SerializedHangContributor::TaskPoll {
                start_ms,
                duration_ms,
                ..
            } if start_ms == 150.0 && duration_ms == 150.0
        ));
    }

    #[test]
    fn hang_monitor_polls_on_its_interval_and_on_flush() {
        use super::{HangMonitor, HangMonitorPollReason};

        let (journal, _guard) = install_test_foreground_journal(64, 4);
        // Each monitor reports on its own channel, which disconnects once the
        // monitor's thread has exited and dropped its callback.
        let spawn_monitor = |journal, interval| {
            let (sender, receiver) = std::sync::mpsc::channel();
            let monitor = HangMonitor::spawn(
                HangDetector::new(journal, HANG_THRESHOLD, FRAME_BUDGET),
                interval,
                move |poll| {
                    if !poll.incidents.is_empty() {
                        sender.send((poll.reason, poll.incidents.len())).ok();
                    }
                },
            )
            .expect("spawn monitor thread");
            (monitor, receiver)
        };

        let (monitor, receiver) = spawn_monitor(journal.clone(), Duration::from_millis(10));
        simulate_blocked_foreground_poll(HANG_THRESHOLD * 2);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(10)).ok(),
            Some((HangMonitorPollReason::Interval, 1)),
        );
        drop(monitor);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(10)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
        );

        // An interval longer than the test isolates the flush path.
        let (monitor, receiver) = spawn_monitor(journal, Duration::from_secs(3600));
        simulate_blocked_foreground_poll(HANG_THRESHOLD * 2);
        let flushed = monitor.request_flush().expect("monitor thread is running");
        futures::executor::block_on(flushed).expect("flush completes");
        assert_eq!(
            receiver.try_recv().ok(),
            Some((HangMonitorPollReason::Flush, 1)),
        );

        drop(monitor);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(10)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
        );
    }

    #[gpui::test]
    fn app_flushes_its_hang_monitor_on_shutdown(cx: &mut TestAppContext) {
        use super::{HangMonitorConfig, HangMonitorPollReason};

        // The monitor runs on a real OS thread, so shutdown has to park until it
        // finishes the flush, and its completion wakes the scheduler from that thread.
        cx.executor().allow_parking();

        let (sender, receiver) = std::sync::mpsc::channel();
        cx.update(|cx| {
            cx.start_hang_monitor(
                HangMonitorConfig {
                    threshold: HANG_THRESHOLD,
                    frame_budget: FRAME_BUDGET,
                    // Longer than the test, so only shutdown can poll.
                    interval: Duration::from_secs(3600),
                },
                move |poll| {
                    sender.send(poll.reason).ok();
                },
            )
            .expect("start hang monitor");
        });
        assert!(receiver.try_recv().is_err());

        cx.update(|cx| cx.shutdown());
        assert_eq!(receiver.try_recv().ok(), Some(HangMonitorPollReason::Flush));
    }

    #[gpui::test]
    fn starting_the_hang_monitor_twice_is_rejected(cx: &mut TestAppContext) {
        use super::{HangMonitorConfig, HangMonitorError};

        let config = HangMonitorConfig {
            threshold: HANG_THRESHOLD,
            frame_budget: FRAME_BUDGET,
            interval: Duration::from_secs(3600),
        };
        cx.update(|cx| {
            cx.start_hang_monitor(config, |_| {})
                .expect("start hang monitor");
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cx.start_hang_monitor(config, |_| {})
            }));
            // Debug builds also assert, so misuse is caught during development.
            if cfg!(debug_assertions) {
                assert!(second.is_err());
            } else {
                assert!(matches!(second, Ok(Err(HangMonitorError::AlreadyStarted))));
            }
        });
    }

    #[test]
    fn serialized_incident_reports_idle_seal_fields() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);

        let idle = FrameSnapshot {
            interval_start: at(200),
            boundary: IntervalBoundary::Idle { ended_at: at(260) },
            events: vec![task_poll_event(at(200), at(260))],
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };
        let incident =
            HangIncident::detect(idle, HANG_THRESHOLD, FRAME_BUDGET).expect("has contributors");
        let serialized = SerializedHangIncident::convert(startup, &incident, 8, None);
        assert_eq!(serialized.phase, "startup");
        assert_eq!(serialized.sealed_by, "idle");
        assert_eq!(serialized.dirty_to_present_ms, None);
        // With no frame, the earliest contributor anchors the active window.
        assert_eq!(serialized.start_ms, 200.0);
        assert_eq!(serialized.active_ms, 60.0);
        assert_eq!(serialized.stall_ms, 60.0);
        assert_eq!(serialized.busy_fraction, 1.0);
    }

    #[test]
    fn phase_is_startup_until_the_first_present() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let snapshot = FrameSnapshot {
            interval_start: at(500),
            boundary: IntervalBoundary::Idle { ended_at: at(600) },
            events: vec![task_poll_event(at(500), at(600))],
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };
        let incident =
            HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET).expect("has contributors");

        let phase = |first_present_at| {
            SerializedHangIncident::convert(startup, &incident, 8, first_present_at).phase
        };
        assert_eq!(phase(None), "startup", "nothing has been presented yet");
        assert_eq!(
            phase(Some(at(700))),
            "startup",
            "the incident began before the first presentation"
        );
        assert_eq!(
            phase(Some(at(500))),
            "steady",
            "an incident beginning exactly at first presentation is steady"
        );
        assert_eq!(phase(Some(at(100))), "steady");
    }

    /// The detector must latch the first presentation it observes and never
    /// move it.
    #[test]
    fn first_present_at_latches_on_the_first_observed_presentation() {
        let (journal, _journal_guard) = install_test_foreground_journal(64, 8);
        let mut detector = HangDetector::new(journal, HANG_THRESHOLD, FRAME_BUDGET);
        let window_id = WindowId::from(0x1A7C4);

        let first_present_end = scheduler::Instant::now();
        record_present(
            presentation(window_id, first_present_end),
            Some(frame(window_id, first_present_end)),
        );
        detector.poll();
        let latched = detector
            .first_present_at()
            .expect("a presentation was recorded");
        assert_eq!(latched, first_present_end);

        record_present(
            presentation(window_id, first_present_end + Duration::from_millis(16)),
            Some(frame(
                window_id,
                first_present_end + Duration::from_millis(16),
            )),
        );
        detector.poll();
        assert_eq!(detector.first_present_at(), Some(latched));
    }

    /// The only work inside the 100ms active window here is the 50ms poll,
    /// so the busy fraction is 0.5: the folded polls' flush span lies
    /// entirely before the window and must contribute nothing to it
    /// (PR #62779 review finding 5).
    #[test]
    fn busy_fraction_excludes_small_polls_outside_the_active_window() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let snapshot = FrameSnapshot {
            interval_start: at(0),
            boundary: IntervalBoundary::Idle { ended_at: at(1000) },
            events: vec![task_poll_event(at(900), at(950))],
            // Folded polls that ran during at(0)..at(800), long before the
            // active window opens at the contributor's start.
            small_polls: vec![SmallPollFlush {
                summary: PollSummary {
                    count: 500,
                    total: Duration::from_millis(400),
                },
                since: at(0),
                until: at(800),
            }],
            dropped_events: 0,
            journal_discontinuous: false,
        };

        let incident =
            HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET).expect("has contributors");
        let serialized = SerializedHangIncident::convert(startup, &incident, 8, None);

        assert_eq!(serialized.busy_fraction, 0.5);
    }

    /// Many sub-threshold pieces of work can drop a frame as thoroughly as
    /// one long stall: an interval whose total foreground spend reaches the
    /// budget is an incident even when no single event crosses the hang
    /// threshold, and every event becomes a contributor so the report shows
    /// what filled the interval.
    #[test]
    fn an_interval_of_small_work_over_budget_is_an_incident() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(0xB0D6E7);
        let snapshot = FrameSnapshot {
            interval_start: at(0),
            boundary: IntervalBoundary::Presented(PresentedFrame {
                frame: FrameTiming {
                    refresh_interval: None,
                    signal_at: Some(at(0)),
                    window_id,
                    dirty_at: Some(at(0)),
                    invalidations: 1,
                    draw_start: at(140),
                    draw_end: at(145),
                    phases: Default::default(),
                },
                presentation: PresentTiming {
                    window_id,
                    present_start: at(145),
                    present_end: at(150),
                    animation_interval: None,
                },
            }),
            events: vec![
                task_poll_event(at(0), at(5)),
                task_poll_event(at(5), at(13)),
                task_poll_event(at(13), at(18)),
            ],
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };

        let incident = HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET)
            .expect("the frame missed the 60 Hz budget used for unknown displays");
        assert_eq!(incident.trigger, HangTrigger::LateFrame);
        assert_eq!(incident.contributors.len(), 3);
        assert_eq!(
            incident.contributors[0].duration(),
            Duration::from_millis(8)
        );
        let serialized = SerializedHangIncident::convert(startup, &incident, 8, Some(startup));
        assert_eq!(serialized.trigger, HangTrigger::LateFrame);
        assert_eq!(serialized.stall_ms, 8.0);
        assert_eq!(serialized.dirty_to_present_ms, Some(150.0));
        assert_eq!(serialized.sealed_by, "present");
    }

    #[test]
    fn a_journal_gap_suppresses_budget_inference_but_retains_observed_hangs() {
        let start = scheduler::Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut sealer = super::super::journal::IntervalSealer::new(start);
        let snapshots = sealer.push_entries([
            ForegroundJournalEntry::Event(task_poll_event(at(0), at(5))),
            ForegroundJournalEntry::Discontinuity { lost: 1 },
            ForegroundJournalEntry::Event(task_poll_event(at(10), at(15))),
            ForegroundJournalEntry::Boundary(IntervalBoundary::Idle { ended_at: at(15) }),
            ForegroundJournalEntry::Discontinuity { lost: 1 },
            ForegroundJournalEntry::Event(task_poll_event(at(20), at(32))),
            ForegroundJournalEntry::Boundary(IntervalBoundary::Idle { ended_at: at(32) }),
            ForegroundJournalEntry::Event(task_poll_event(at(40), at(52))),
            ForegroundJournalEntry::Boundary(IntervalBoundary::Idle { ended_at: at(52) }),
        ]);
        let [budget_only, observed_hang, clean] = snapshots.as_slice() else {
            panic!("expected two discontinuous snapshots followed by a clean one");
        };

        assert!(budget_only.journal_discontinuous);
        assert_eq!(budget_only.dropped_events, 1);
        assert!(HangIncident::detect(budget_only.clone(), HANG_THRESHOLD, FRAME_BUDGET).is_none());

        let observed_hang =
            HangIncident::detect(observed_hang.clone(), HANG_THRESHOLD, FRAME_BUDGET)
                .expect("the directly observed threshold-qualified hang remains valid");
        assert!(observed_hang.snapshot.journal_discontinuous);
        assert_eq!(observed_hang.contributors.len(), 1);
        let serialized = SerializedHangIncident::convert(start, &observed_hang, 8, None);
        assert!(serialized.journal_discontinuous);

        assert!(!clean.journal_discontinuous);
        assert!(HangIncident::detect(clean.clone(), HANG_THRESHOLD, FRAME_BUDGET).is_some());
    }

    /// A frame that reaches the screen late with almost no foreground work
    /// behind it — pure scheduling or presentation delay — is not a hang:
    /// the budget measures foreground spend, not dirty-to-present time.
    #[test]
    fn a_slow_frame_with_little_foreground_spend_is_not_an_incident() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(0xFA57);
        let snapshot = FrameSnapshot {
            interval_start: at(0),
            boundary: IntervalBoundary::Presented(PresentedFrame {
                frame: FrameTiming {
                    refresh_interval: None,
                    signal_at: None,
                    window_id,
                    dirty_at: Some(at(0)),
                    invalidations: 1,
                    draw_start: at(145),
                    draw_end: at(147),
                    phases: Default::default(),
                },
                presentation: PresentTiming {
                    window_id,
                    present_start: at(149),
                    present_end: at(150),
                    animation_interval: None,
                },
            }),
            events: vec![task_poll_event(at(0), at(5))],
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };

        assert!(HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET).is_none());
    }

    #[test]
    fn active_use_counts_time_within_a_minute_of_deliberate_input() {
        let start = scheduler::Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let input_of_kind = |kind: &'static str, seconds: u64| {
            ForegroundJournalEntry::Event(ForegroundEvent::Input(InputTiming {
                kind,
                start: at(seconds),
                end: at(seconds),
                caused_invalidation: true,
            }))
        };
        let input = |seconds: u64| input_of_kind("key_down", seconds);
        let max = Duration::from_secs(10);
        let mut activity = super::ActiveUse::default();
        assert_eq!(activity.take(at(0), max), Duration::ZERO);
        // The pointer passing over a window isn't use.
        activity.observe(&input_of_kind("mouse_move", 1));
        activity.observe(&input_of_kind("mouse_exited", 2));
        assert_eq!(activity.take(at(4), max), Duration::ZERO);
        // Idle until input at 5 s.
        activity.observe(&input(5));
        assert_eq!(activity.take(at(8), max), Duration::from_secs(3));
        // Active until 65 s, a minute after the input.
        assert_eq!(activity.take(at(16), max), Duration::from_secs(8));
        for second in (24..=64).step_by(8) {
            activity.take(at(second), max);
        }
        assert_eq!(activity.take(at(70), max), Duration::from_secs(1));
        assert_eq!(activity.take(at(78), max), Duration::ZERO);
        // A suspend discards the poll it was observed in.
        activity.observe(&input(80));
        activity.observe(&ForegroundJournalEntry::Boundary(
            IntervalBoundary::PowerTransition { ended_at: at(81) },
        ));
        assert_eq!(activity.take(at(84), max), Duration::ZERO);
        assert_eq!(activity.take(at(88), max), Duration::from_secs(4));
        // Time the monitor itself was suspended isn't counted.
        assert_eq!(activity.take(at(130), max), max);

        // Incidents spanning input or within a minute after it are during
        // active use; ones later than that aren't.
        assert!(activity.was_active_during(at(79), at(80)));
        assert!(activity.was_active_during(at(100), at(101)));
        assert!(!activity.was_active_during(at(141), at(142)));
        assert!(!activity.was_active_during(at(66), at(70)));
    }

    /// Without a measured frame, only a single event as long as the frame
    /// budget is blocking work, however much short work the interval holds.
    #[test]
    fn only_single_events_over_the_frame_budget_are_blocking_work() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let snapshot = |longest_ms: u64| FrameSnapshot {
            interval_start: at(0),
            boundary: IntervalBoundary::Idle { ended_at: at(200) },
            events: (0..10)
                .map(|i| task_poll_event(at(i * 20), at(i * 20 + 5)))
                .chain([task_poll_event(at(200), at(200 + longest_ms))])
                .collect(),
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };
        let frame_budget = Duration::from_millis(7);
        assert!(HangIncident::detect(snapshot(6), HANG_THRESHOLD, frame_budget).is_none());
        let incident = HangIncident::detect(snapshot(7), HANG_THRESHOLD, frame_budget)
            .expect("one event as long as the frame budget");
        assert_eq!(incident.trigger, HangTrigger::BlockingWork);
        assert_eq!(incident.contributors.len(), 1);
    }

    /// With a known refresh interval, a frame is a budget hang when its work,
    /// from its frame signal to its submission, exceeded one refresh interval or
    /// the 120 Hz floor, whichever is longer.
    #[test]
    fn late_frames_are_budget_hangs() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(0xB0D6E7);
        let detect = |refresh_interval: Duration,
                      signal_at: Option<scheduler::Instant>,
                      submitted_ms: u64| {
            let snapshot = FrameSnapshot {
                interval_start: at(0),
                boundary: IntervalBoundary::Presented(PresentedFrame {
                    frame: FrameTiming {
                        window_id,
                        dirty_at: Some(at(0)),
                        invalidations: 1,
                        draw_start: at(1),
                        draw_end: at(submitted_ms - 1),
                        refresh_interval: Some(refresh_interval),
                        signal_at,
                        phases: Default::default(),
                    },
                    presentation: PresentTiming {
                        window_id,
                        present_start: at(submitted_ms),
                        // Time spent inside the submission doesn't make
                        // the frame late.
                        present_end: at(submitted_ms + 50),
                        animation_interval: None,
                    },
                }),
                events: vec![
                    task_poll_event(at(1), at(6)),
                    task_poll_event(at(10), at(15)),
                ],
                small_polls: Vec::new(),
                dropped_events: 0,
                journal_discontinuous: false,
            };
            HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET)
        };

        let sixty_hertz = Duration::from_secs(1) / 60;
        let incident = detect(sixty_hertz, Some(at(0)), 25).expect("work exceeded one refresh");
        assert_eq!(incident.trigger, HangTrigger::LateFrame);
        assert_eq!(incident.contributors.len(), 2);
        // Work from before the frame signal didn't delay the frame.
        let incident = detect(sixty_hertz, Some(at(8)), 25).expect("17 ms of work at 60 Hz");
        assert_eq!(incident.contributors.len(), 1);
        assert!(detect(sixty_hertz, Some(at(0)), 16).is_none());
        assert!(detect(sixty_hertz, None, 25).is_none());

        let three_hundred_sixty_hertz = Duration::from_secs(1) / 360;
        assert!(detect(three_hundred_sixty_hertz, Some(at(0)), 8).is_none());
        assert!(detect(three_hundred_sixty_hertz, Some(at(0)), 9).is_some());
    }

    /// Contributors serialize in start order with their nesting depth: an
    /// input dispatch that synchronously drew a window reads as the input at
    /// depth 0 followed by the draw at depth 1, rather than two unrelated
    /// blocks of equal wall time.
    #[test]
    fn serialized_contributors_are_chronological_with_nesting_depths() {
        let startup = scheduler::Instant::now();
        let at = |ms: u64| startup + Duration::from_millis(ms);
        let window_id = WindowId::from(0x2E57ED);
        let snapshot = FrameSnapshot {
            interval_start: at(0),
            boundary: IntervalBoundary::Idle { ended_at: at(80) },
            events: vec![
                task_poll_event(at(50), at(70)),
                ForegroundEvent::Input(InputTiming {
                    kind: "mouse_move",
                    start: at(0),
                    end: at(40),
                    caused_invalidation: true,
                }),
                ForegroundEvent::Draw(FrameTiming {
                    refresh_interval: None,
                    signal_at: None,
                    window_id,
                    dirty_at: Some(at(0)),
                    invalidations: 1,
                    draw_start: at(1),
                    draw_end: at(39),
                    phases: Default::default(),
                }),
            ],
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        };

        let incident =
            HangIncident::detect(snapshot, HANG_THRESHOLD, FRAME_BUDGET).expect("has contributors");
        let serialized = SerializedHangIncident::convert(startup, &incident, 8, Some(startup));

        assert_eq!(serialized.stall_ms, 40.0);
        assert!(matches!(
            serialized.contributors.as_slice(),
            [
                SerializedHangContributor::Input {
                    input_kind: "mouse_move",
                    depth: 0,
                    ..
                },
                SerializedHangContributor::Draw { depth: 1, .. },
                SerializedHangContributor::TaskPoll { depth: 0, .. },
            ]
        ));
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn detection_matches_threshold_and_blocking_work_model_at_boundaries(
            durations_ms in prop::collection::vec(0u16..=50, 0..=16),
            threshold_ms in 0u16..=60,
            frame_budget_ms in 0u16..=60,
            small_poll_total_ms in 0u16..=100,
        ) {
            let origin = scheduler::Instant::now();
            let mut cursor_ms = 0u64;
            let events = durations_ms
                .iter()
                .map(|duration_ms| {
                    let start = origin + Duration::from_millis(cursor_ms);
                    cursor_ms += u64::from(*duration_ms);
                    let end = origin + Duration::from_millis(cursor_ms);
                    cursor_ms += 1;
                    ForegroundEvent::Input(InputTiming {
                        kind: "test",
                        start,
                        end,
                        caused_invalidation: false,
                    })
                })
                .collect::<Vec<_>>();
            let interval_end_ms = cursor_ms.max(1);
            let interval_end = origin + Duration::from_millis(interval_end_ms);
            // Without a frame signal, the frame's work can't be measured.
            let window_id = WindowId::from(1);
            let snapshot = FrameSnapshot {
                interval_start: origin,
                boundary: IntervalBoundary::Presented(PresentedFrame {
                    frame: FrameTiming {
                        window_id,
                        dirty_at: Some(origin),
                        invalidations: 1,
                        draw_start: interval_end,
                        draw_end: interval_end,
                        refresh_interval: None,
                        signal_at: None,
                        phases: Default::default(),
                    },
                    presentation: PresentTiming {
                        window_id,
                        present_start: interval_end,
                        present_end: interval_end,
                        animation_interval: None,
                    },
                }),
                events,
                small_polls: vec![SmallPollFlush {
                    summary: PollSummary {
                        count: u64::from(small_poll_total_ms > 0),
                        total: Duration::from_millis(u64::from(small_poll_total_ms)),
                    },
                    since: origin,
                    until: origin + Duration::from_millis(interval_end_ms),
                }],
                dropped_events: 0,
                journal_discontinuous: false,
            };
            let qualifying_durations = durations_ms
                .iter()
                .copied()
                .filter(|duration| *duration >= threshold_ms)
                .collect::<Vec<_>>();
            let blocking_durations = durations_ms
                .iter()
                .copied()
                .filter(|duration| *duration >= frame_budget_ms)
                .collect::<Vec<_>>();
            let expected_incident =
                !qualifying_durations.is_empty() || !blocking_durations.is_empty();

            let incident = HangIncident::detect(
                snapshot,
                Duration::from_millis(u64::from(threshold_ms)),
                Duration::from_millis(u64::from(frame_budget_ms)),
            );
            prop_assert_eq!(incident.is_some(), expected_incident);

            if let Some(incident) = incident {
                let mut expected_contributors = if qualifying_durations.is_empty() {
                    blocking_durations
                } else {
                    qualifying_durations
                };
                expected_contributors.sort_unstable_by(|first, second| second.cmp(first));
                let observed_contributors = incident
                    .contributors
                    .iter()
                    .map(|event| event.duration().as_millis() as u16)
                    .collect::<Vec<_>>();
                prop_assert_eq!(observed_contributors, expected_contributors);
            }
        }
    }

    #[derive(Clone, Default)]
    struct HangControls {
        render: Rc<Cell<Option<Duration>>>,
        input: Rc<Cell<Option<Duration>>>,
        action: Rc<Cell<Option<Duration>>>,
    }

    struct HangyView {
        controls: HangControls,
        focus_handle: FocusHandle,
    }

    impl Render for HangyView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if let Some(duration) = self.controls.render.take() {
                thread::sleep(duration);
            }
            let action_controls = self.controls.clone();
            let input_controls = self.controls.clone();
            div()
                .size_full()
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(move |_, _: &HangyAction, _, _| {
                    if let Some(duration) = action_controls.action.take() {
                        thread::sleep(duration);
                    }
                }))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    if let Some(duration) = input_controls.input.take() {
                        thread::sleep(duration);
                    }
                })
        }
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    enum HangKind {
        Render,
        Input,
        Action,
        Poll,
    }

    /// Renders a real element tree and injects randomly ordered, randomly
    /// sized hangs through the production dispatch paths: view rendering
    /// (window draw), platform input dispatch, action dispatch, and a
    /// foreground task poll. Asserts the detector reports every one of them.
    #[gpui::test(iterations = 5)]
    fn detects_randomly_placed_foreground_hangs(mut rng: StdRng, cx: &mut TestAppContext) {
        let (journal, _journal_guard) = install_test_foreground_journal(1024, 16);
        let mut detector = HangDetector::new(journal, HANG_THRESHOLD, FRAME_BUDGET);

        let controls = HangControls::default();
        let (view, cx) = cx.add_window_view(|_, cx| HangyView {
            controls: controls.clone(),
            focus_handle: cx.focus_handle(),
        });
        view.update_in(cx, |view, window, cx| {
            window.focus(&view.focus_handle, cx);
        });
        draw_window(cx);

        let mut injections: Vec<Injection> = Vec::new();
        for _ in 0..rng.random_range(1..=4) {
            let duration = HANG_THRESHOLD + Duration::from_millis(rng.random_range(5..25));
            let started = scheduler::Instant::now();
            let kind = match rng.random_range(0..4) {
                0 => {
                    controls.render.set(Some(duration));
                    view.update_in(cx, |_, _, cx| cx.notify());
                    draw_window(cx);
                    HangKind::Render
                }
                1 => {
                    controls.input.set(Some(duration));
                    cx.simulate_mouse_down(
                        point(px(5.), px(5.)),
                        MouseButton::Left,
                        Modifiers::none(),
                    );
                    HangKind::Input
                }
                2 => {
                    controls.action.set(Some(duration));
                    cx.dispatch_action(HangyAction);
                    HangKind::Action
                }
                _ => {
                    simulate_blocked_foreground_poll(duration);
                    HangKind::Poll
                }
            };
            injections.push(Injection {
                kind,
                duration,
                window: started..scheduler::Instant::now(),
            });

            // Innocent interleaved activity that must not confuse detection.
            if rng.random_bool(0.5) {
                draw_window(cx);
            }
            if rng.random_bool(0.5) {
                cx.simulate_mouse_up(point(px(5.), px(5.)), MouseButton::Left, Modifiers::none());
            }
        }

        // A final presentation seals whatever interval is still open.
        draw_window(cx);

        let incidents = detector.poll();
        let contributors: Vec<ForegroundEvent> = incidents
            .iter()
            .flat_map(|incident| incident.contributors.iter().copied())
            .collect();

        for injection in &injections {
            assert_detected_once(injection, &contributors);
        }
    }

    /// A slow view's draw is journaled under its type name with its own time,
    /// excluding a slow view nested inside it, even when it's embedded through
    /// an `AnyView`; fast phases aren't journaled, and the draw records how
    /// long each of its phases took.
    #[gpui::test]
    fn slow_views_are_journaled_with_their_own_time(cx: &mut TestAppContext) {
        let (journal, _journal_guard) = install_test_foreground_journal(1024, 16);
        let mut collector = journal.collector();
        let (_view, cx) = cx.add_window_view(|_, cx| SlowParent {
            child: cx.new(|_| SlowChild).into(),
        });
        draw_window(cx);

        let events: Vec<ForegroundEvent> = collector
            .collect_unseen()
            .entries
            .into_iter()
            .filter_map(|entry| match entry {
                ForegroundJournalEntry::Event(event) => Some(event),
                _ => None,
            })
            .collect();
        let view_timing = |suffix: &str, phase: ViewPhase| {
            events
                .iter()
                .rev()
                .find_map(|event| match event {
                    ForegroundEvent::View(timing)
                        if timing.name.ends_with(suffix) && timing.phase == phase =>
                    {
                        Some(*timing)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("expected a {phase:?} timing for {suffix}"))
        };
        let parent = view_timing("::SlowParent", ViewPhase::RequestLayout);
        let child = view_timing("::SlowChild", ViewPhase::RequestLayout);
        let parent_duration = parent.end.duration_since(parent.start);
        let child_duration = child.end.duration_since(child.start);
        assert!(child.self_duration >= SLOW_CHILD_RENDER);
        assert!(parent.self_duration >= SLOW_PARENT_RENDER);
        assert!(parent_duration >= child_duration + parent.self_duration);
        assert!(!events.iter().any(|event| matches!(
            event,
            ForegroundEvent::View(timing) if timing.phase == ViewPhase::Paint
        )));

        let draw = events
            .iter()
            .rev()
            .find_map(|event| match event {
                ForegroundEvent::Draw(timing) => Some(*timing),
                _ => None,
            })
            .expect("a draw");
        assert!(draw.phases.request_layout >= SLOW_PARENT_RENDER + SLOW_CHILD_RENDER);
        assert!(
            draw.phases.request_layout + draw.phases.prepaint + draw.phases.paint
                <= draw.draw_duration()
        );
    }

    const SLOW_PARENT_RENDER: Duration = Duration::from_millis(3);
    const SLOW_CHILD_RENDER: Duration = Duration::from_millis(2);

    struct SlowParent {
        child: AnyView,
    }

    impl Render for SlowParent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            thread::sleep(SLOW_PARENT_RENDER);
            div().size_full().child(self.child.clone())
        }
    }

    struct SlowChild;

    impl Render for SlowChild {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            thread::sleep(SLOW_CHILD_RENDER);
            div().size_full()
        }
    }

    struct Injection {
        kind: HangKind,
        duration: Duration,
        window: std::ops::Range<scheduler::Instant>,
    }

    /// Exactly one contributor of the injected kind must fall inside the
    /// injection's window, and it must be at least as long as the injected
    /// sleep (sleeps never wake early). Contributors outside every window are
    /// ignored: on a loaded machine, uninjected work can legitimately exceed
    /// the threshold, and reporting it is correct.
    fn assert_detected_once(injection: &Injection, contributors: &[ForegroundEvent]) {
        let Injection {
            kind,
            duration,
            window,
        } = injection;
        let matches: Vec<&ForegroundEvent> = contributors
            .iter()
            .filter(|event| {
                matches_kind(event, *kind)
                    && window.start <= event.start_time()
                    && event.end_time() <= window.end
            })
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected injected {kind:?} hang of {duration:?} to be reported exactly once; \
             observed {:?}",
            matches
                .iter()
                .map(|event| event.duration())
                .collect::<Vec<_>>(),
        );
        if let Some(event) = matches.first() {
            assert!(
                event.duration() >= *duration,
                "injected {kind:?} hang of {duration:?} was reported as {:?}",
                event.duration(),
            );
        }
    }

    fn matches_kind(event: &ForegroundEvent, kind: HangKind) -> bool {
        match (event, kind) {
            (ForegroundEvent::Draw(_), HangKind::Render) => true,
            (ForegroundEvent::Input(_), HangKind::Input) => true,
            (ForegroundEvent::Action(timing), HangKind::Action) => {
                timing.name.ends_with("HangyAction")
            }
            (ForegroundEvent::TaskPoll(timing), HangKind::Poll) => {
                timing.location.file() == file!()
            }
            _ => false,
        }
    }

    fn draw_window(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let arena_clear = window.draw(cx);
            window.present_if_needed();
            arena_clear.clear(cx);
        });
    }

    /// The deterministic test scheduler does not bracket runnables with the
    /// profiler hooks, so drive the same public hooks the platform
    /// dispatchers call around a poll that blocks the foreground.
    fn simulate_blocked_foreground_poll(duration: Duration) {
        let location = std::panic::Location::caller();
        crate::profiler::update_running_task(SpawnTime(scheduler::Instant::now()), location);
        thread::sleep(duration);
        crate::profiler::save_task_timing();
    }

    fn task_poll_event(start: scheduler::Instant, end: scheduler::Instant) -> ForegroundEvent {
        ForegroundEvent::TaskPoll(TaskTiming {
            location: std::panic::Location::caller(),
            spawned: SpawnTime(start),
            start,
            end: YieldTime(end),
        })
    }

    fn presentation(window_id: WindowId, present_end: scheduler::Instant) -> PresentTiming {
        PresentTiming {
            window_id,
            present_start: present_end - Duration::from_millis(1),
            present_end,
            animation_interval: None,
        }
    }

    fn frame(window_id: WindowId, draw_end: scheduler::Instant) -> FrameTiming {
        FrameTiming {
            refresh_interval: None,
            signal_at: None,
            window_id,
            dirty_at: Some(draw_end - Duration::from_millis(2)),
            invalidations: 1,
            draw_start: draw_end - Duration::from_millis(1),
            draw_end,
            phases: Default::default(),
        }
    }

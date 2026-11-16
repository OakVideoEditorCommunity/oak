    use std::{
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use super::*;
    use crate::profiler::journal::install_test_foreground_journal;

    /// Runs `benchmark` under a wall-time primary with a fake counter as the
    /// secondary metric and returns the per-iteration values the report
    /// recorded for it.
    fn secondary_values_per_iteration(
        counter: Arc<AtomicU64>,
        benchmark: impl Fn(&mut BenchAppContext, &Arc<AtomicU64>),
    ) -> Vec<u64> {
        let platform = bench_platform(None, Arc::new(crate::NoopTextSystem::new()));
        let report = BenchReport::default();
        let mut criterion = criterion::Criterion::default()
            .with_measurement(
                BenchMeasurement::new(criterion::measurement::WallTime).with_secondary(
                    "fake counter",
                    bench_metrics::FakeCounter::new(counter.clone()),
                ),
            )
            .without_plots()
            .sample_size(10)
            .warm_up_time(Duration::from_millis(1))
            .measurement_time(Duration::from_millis(1));

        criterion.bench_function("secondary_values_per_iteration", |bencher| {
            let mut cx = BenchAppContext::new_with_platform_and_report(
                platform.clone(),
                Some("secondary_values_per_iteration"),
                bencher,
                report.clone(),
            );
            // Fixture work outside the measured loop must not be attributed
            // to any iteration.
            counter.fetch_add(1_000_000, Ordering::SeqCst);
            benchmark(&mut cx, &counter);
            cx.teardown();
        });

        report
            .metrics()
            .values("fake counter")
            .expect("the fake counter was recorded")
            .into_iter()
            .map(|value| value as u64)
            .collect()
    }

    #[test]
    fn secondary_metric_is_normalized_per_iteration_for_bench_iter() {
        let values = secondary_values_per_iteration(Arc::new(AtomicU64::new(0)), |cx, counter| {
            cx.bench_iter(|_| {
                counter.fetch_add(11, Ordering::SeqCst);
            });
        });
        assert!(!values.is_empty());
        assert!(
            values.iter().all(|value| *value == 11),
            "each sample's total should be divided by its iteration count: {values:?}"
        );
    }

    #[test]
    fn secondary_metric_is_accumulated_across_per_iteration_batches() {
        let values = secondary_values_per_iteration(Arc::new(AtomicU64::new(0)), |cx, counter| {
            cx.bench_batched_task(
                |_| {
                    // Setup runs outside every measured interval.
                    counter.fetch_add(1_000, Ordering::SeqCst);
                },
                |(), cx| {
                    counter.fetch_add(11, Ordering::SeqCst);
                    cx.background_spawn(async {})
                },
            );
        });
        assert!(!values.is_empty());
        assert!(
            values.iter().all(|value| *value == 11),
            "PerIteration batches should sum to one per-iteration value: {values:?}"
        );
    }

    #[test]
    fn foreground_work_reports_long_task_without_window_draw() {
        let (journal, _journal_guard) = install_test_foreground_journal(1024, 64);
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground_executor = ForegroundExecutor::new(dispatcher);

        let trace_scope = TraceScope::start(journal.collector());

        // A single foreground task poll that never touches a window, akin
        // to the stall a debounced background computation can cause.
        let task = foreground_executor.spawn(async move {
            std::thread::sleep(Duration::from_millis(60));
        });
        run_task_to_completion(&foreground_executor, task);

        let events = trace_scope.finish();
        assert!(
            events.frame_events.is_empty(),
            "no window was involved, so no frame events should be recorded"
        );

        let report = BenchReport::default();
        report.record_foreground_events(events.foreground_events());

        let summary = report
            .foreground_work()
            .expect("a long task poll should be reported even without a window draw");
        // The spawned task's own poll is one sample; the tiny wrapper poll
        // that observes its completion in `run_task_to_completion` folds
        // into a second, near-zero sample rather than being dropped.
        assert!(summary.count >= 1, "expected at least one recorded item");
        assert!(
            summary.max >= Duration::from_millis(55),
            "expected the long poll's duration to be recorded, got {:?}",
            summary.max
        );
        // `total` is an exact sum, while `max` may be rounded up to its
        // histogram bucket's boundary, so compare each against the expected
        // floor directly instead of against each other.
        assert!(
            summary.total >= Duration::from_millis(55),
            "expected the long poll's duration to be included in the total, got {:?}",
            summary.total
        );
    }

    #[test]
    fn foreground_work_excludes_setup_before_trace_scope_starts() {
        let (journal, _journal_guard) = install_test_foreground_journal(1024, 64);
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground_executor = ForegroundExecutor::new(dispatcher);

        // Fixture/setup work that must not be attributed to the measurement:
        // a long poll recorded before the trace scope (and its journal
        // collector) is created.
        let setup_task = foreground_executor.spawn(async move {
            std::thread::sleep(Duration::from_millis(80));
        });
        run_task_to_completion(&foreground_executor, setup_task);

        let trace_scope = TraceScope::start(journal.collector());

        let measured_task = foreground_executor.spawn(async move {
            std::thread::sleep(Duration::from_millis(10));
        });
        run_task_to_completion(&foreground_executor, measured_task);

        let events = trace_scope.finish();
        let report = BenchReport::default();
        report.record_foreground_events(events.foreground_events());

        let summary = report
            .foreground_work()
            .expect("the measured task's poll should be reported");
        assert!(
            summary.max < Duration::from_millis(40),
            "setup work's 80ms poll must not leak into the measured summary, got {:?}",
            summary.max
        );
        assert!(
            summary.total < Duration::from_millis(40),
            "setup work's 80ms poll must not leak into the measured total, got {:?}",
            summary.total
        );
    }

    #[test]
    fn bench_task_reports_long_task_without_window() {
        let platform = bench_platform(None, Arc::new(crate::NoopTextSystem::new()));
        let report = BenchReport::default();
        let name = "bench_task_reports_long_task_without_window";

        let mut criterion = criterion::Criterion::default()
            .with_measurement(BenchMeasurement::new(criterion::measurement::WallTime))
            .without_plots()
            .sample_size(10)
            .warm_up_time(Duration::from_millis(1))
            .measurement_time(Duration::from_millis(1));

        criterion.bench_function(name, |bencher| {
            let mut cx = BenchAppContext::new_with_platform_and_report(
                platform.clone(),
                Some(name),
                bencher,
                report.clone(),
            );
            cx.bench_task(|cx| {
                cx.foreground_executor().spawn(async move {
                    std::thread::sleep(Duration::from_millis(20));
                })
            });
            cx.teardown();
        });

        let summary = report
            .foreground_work()
            .expect("bench_task should report foreground work with no window involved");
        assert!(
            summary.max >= Duration::from_millis(15),
            "expected a ~20ms task poll to be recorded, got {:?}",
            summary.max
        );
    }

    #[test]
    fn task_completion_supports_non_send_foreground_output() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);
        let (sender, receiver) = futures::channel::oneshot::channel();

        background_executor
            .spawn(async move {
                sender
                    .send(())
                    .expect("foreground receiver should remain alive");
            })
            .detach();
        let expected_output = Rc::new(42);
        let task_output = expected_output.clone();
        let task = foreground_executor.spawn(async move {
            receiver.await.expect("background task should send a value");
            task_output
        });

        let output = run_task_to_completion(&foreground_executor, task);
        assert!(
            Rc::ptr_eq(&output, &expected_output),
            "task runner should preserve non-Send foreground output"
        );
    }

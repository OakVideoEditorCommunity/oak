    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::{BackgroundExecutor, ForegroundExecutor};

    #[test]
    fn is_idle_tracks_queued_work_but_ignores_undue_timers() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground = ForegroundExecutor::new(dispatcher.clone());
        assert!(dispatcher.is_idle());

        foreground.spawn(async {}).detach();
        assert!(!dispatcher.is_idle());
        dispatcher.run_until_idle();
        assert!(dispatcher.is_idle());

        let background = BackgroundExecutor::new(dispatcher.clone());
        let timer = background.timer(Duration::from_secs(60));
        // The timer future's initial poll runs on a worker thread; wait for
        // it so only the armed, not-yet-due timer remains.
        dispatcher.run_until_idle();
        assert!(
            dispatcher.is_idle(),
            "a timer that is not due yet should not count as pending work"
        );
        drop(timer);
        dispatcher.cancel_pending_timers();
        dispatcher.run_until_idle();
        assert!(dispatcher.is_idle());
    }

    #[test]
    fn run_ready_main_tasks_does_not_wait_for_background_handoffs() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let background = BackgroundExecutor::new(dispatcher.clone());
        let foreground = ForegroundExecutor::new(dispatcher.clone());

        let (sender, receiver) = futures::channel::oneshot::channel();
        background
            .spawn(async move {
                thread::sleep(Duration::from_millis(10));
                sender.send(()).ok();
            })
            .detach();

        let completed = Arc::new(AtomicBool::new(false));
        foreground
            .spawn({
                let completed = completed.clone();
                async move {
                    receiver.await.ok();
                    completed.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        assert!(dispatcher.run_ready_main_tasks());
        assert!(!completed.load(Ordering::SeqCst));

        dispatcher.run_until_idle();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn run_until_idle_completes_background_to_main_handoffs() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let background = BackgroundExecutor::new(dispatcher.clone());
        let foreground = ForegroundExecutor::new(dispatcher.clone());

        let (sender, receiver) = futures::channel::oneshot::channel();
        background
            .spawn(async move {
                thread::sleep(Duration::from_millis(10));
                sender.send(()).ok();
            })
            .detach();

        let completed = Arc::new(AtomicBool::new(false));
        foreground
            .spawn({
                let completed = completed.clone();
                async move {
                    receiver.await.ok();
                    completed.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        dispatcher.run_until_idle();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn run_until_waits_for_untracked_external_wakes() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground = ForegroundExecutor::new(dispatcher.clone());
        let (sender, receiver) = futures::channel::oneshot::channel();
        let sender_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            sender
                .send(())
                .expect("foreground receiver should remain alive");
        });

        let completed = Arc::new(AtomicBool::new(false));
        foreground
            .spawn({
                let completed = completed.clone();
                async move {
                    receiver
                        .await
                        .expect("external sender should deliver its wake");
                    completed.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        dispatcher.run_until(|| completed.load(Ordering::SeqCst).then_some(()));
        sender_thread.join().expect("sender thread should finish");
        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn run_until_returns_at_readiness_despite_requeuing_main_work() {
        const REQUEUE_LIMIT: usize = 10_000;

        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground = ForegroundExecutor::new(dispatcher.clone());

        // Mirrors main-thread work that yields and immediately re-queues
        // itself (e.g. an idle-time sweep): the main queue never drains until
        // such work finishes every iteration, so readiness must be observed
        // between runnables rather than only at quiescence.
        let iterations = Arc::new(AtomicUsize::new(0));
        foreground
            .spawn({
                let iterations = iterations.clone();
                async move {
                    for _ in 0..REQUEUE_LIMIT {
                        iterations.fetch_add(1, Ordering::SeqCst);
                        yield_once().await;
                    }
                }
            })
            .detach();

        let completed = Arc::new(AtomicBool::new(false));
        foreground
            .spawn({
                let completed = completed.clone();
                async move {
                    completed.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        dispatcher.run_until(|| completed.load(Ordering::SeqCst).then_some(()));
        assert!(
            iterations.load(Ordering::SeqCst) < REQUEUE_LIMIT,
            "run_until should return at readiness instead of draining re-queued main work"
        );
    }

    /// Completes after one re-schedule: the poll returns `Pending` and wakes
    /// immediately, so the runnable re-enters the main queue.
    fn yield_once() -> impl Future<Output = ()> {
        let mut yielded = false;
        std::future::poll_fn(move |poll_context| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                poll_context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
    }

    #[test]
    fn run_ready_main_tasks_advances_requeuing_work_one_batch_per_call() {
        const REQUEUE_LIMIT: usize = 10_000;

        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let foreground = ForegroundExecutor::new(dispatcher.clone());

        let iterations = Arc::new(AtomicUsize::new(0));
        foreground
            .spawn({
                let iterations = iterations.clone();
                async move {
                    for _ in 0..REQUEUE_LIMIT {
                        iterations.fetch_add(1, Ordering::SeqCst);
                        yield_once().await;
                    }
                }
            })
            .detach();

        assert!(dispatcher.run_ready_main_tasks());
        assert_eq!(iterations.load(Ordering::SeqCst), 1);
        assert!(dispatcher.run_ready_main_tasks());
        assert_eq!(iterations.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn timers_fire_in_real_time() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let background = BackgroundExecutor::new(dispatcher);

        let fired = Arc::new(AtomicBool::new(false));
        let timer = background.timer(Duration::from_millis(10));
        background
            .spawn({
                let fired = fired.clone();
                async move {
                    timer.await;
                    fired.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !fired.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_pending_timers_wakes_waiters_without_waiting_for_deadline() {
        let dispatcher = Arc::new(ThreadedDispatcher::new());
        let background = BackgroundExecutor::new(dispatcher.clone());

        let fired = Arc::new(AtomicBool::new(false));
        let timer = background.timer(Duration::from_secs(10));
        background
            .spawn({
                let fired = fired.clone();
                async move {
                    timer.await;
                    fired.store(true, Ordering::SeqCst);
                }
            })
            .detach();

        dispatcher.run_until_idle();
        assert_eq!(dispatcher.cancel_pending_timers(), 1);
        dispatcher.run_until_idle();

        assert!(fired.load(Ordering::SeqCst));
        assert_eq!(dispatcher.cancel_pending_timers(), 0);
    }

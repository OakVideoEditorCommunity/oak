    use super::*;
    use gpui::{Point, px};

    #[cfg(any(feature = "wayland", feature = "x11"))]
    #[test]
    fn rejects_null_xkb_context() {
        let context = unsafe {
            // libxkbcommon permits unref on null, matching the value returned by Context::new on failure.
            xkb::Context::from_raw_ptr(std::ptr::null_mut())
        };
        let error = validate_xkb_context(context)
            .err()
            .expect("null XKB context should be rejected");

        assert_eq!(
            error.to_string(),
            "libxkbcommon failed to create an XKB context"
        );
    }

    #[test]
    fn test_is_within_click_distance() {
        let zero = Point::new(px(0.0), px(0.0));
        assert!(is_within_click_distance(zero, Point::new(px(5.0), px(5.0))));
        assert!(is_within_click_distance(
            zero,
            Point::new(px(-4.9), px(5.0))
        ));
        assert!(is_within_click_distance(
            Point::new(px(3.0), px(2.0)),
            Point::new(px(-2.0), px(-2.0))
        ));
        assert!(!is_within_click_distance(
            zero,
            Point::new(px(5.0), px(5.1))
        ),);
    }

    #[cfg(any(feature = "wayland", feature = "x11"))]
    mod idle_sleep_prevention {
        use super::super::await_idle_sleep_prevention;
        use anyhow::{Result, anyhow};
        use futures::channel::oneshot;
        use gpui::{ActivityGuard, TestAppContext};
        use std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering::SeqCst},
            },
            time::Duration,
        };

        #[gpui::test]
        async fn guard_passes_through_and_releases_on_drop(cx: &mut TestAppContext) {
            let released = Arc::new(AtomicUsize::new(0));
            let (guard_tx, guard_rx) = oneshot::channel();
            assert!(guard_tx.send(Ok(release_guard(released.clone()))).is_ok());
            let guard = await_idle_sleep_prevention(guard_rx, &cx.background_executor)
                .await
                .expect("acquisition should succeed");

            assert_eq!(released.load(SeqCst), 0);
            drop(guard);
            assert_eq!(released.load(SeqCst), 1);
        }

        #[gpui::test]
        async fn acquisition_error_passes_through(cx: &mut TestAppContext) {
            let (guard_tx, guard_rx) = oneshot::channel::<Result<ActivityGuard>>();
            assert!(guard_tx.send(Err(anyhow!("inhibition rejected"))).is_ok());
            assert_eq!(
                await_idle_sleep_prevention(guard_rx, &cx.background_executor)
                    .await
                    .err()
                    .expect("acquisition should fail")
                    .to_string(),
                "inhibition rejected"
            );
        }

        #[gpui::test]
        async fn abandoned_acquisition_is_an_error(cx: &mut TestAppContext) {
            let (guard_tx, guard_rx) = oneshot::channel::<Result<ActivityGuard>>();
            drop(guard_tx);
            assert_eq!(
                await_idle_sleep_prevention(guard_rx, &cx.background_executor)
                    .await
                    .err()
                    .expect("acquisition should fail")
                    .to_string(),
                "Idle sleep prevention request was abandoned"
            );
        }

        #[gpui::test]
        async fn late_guard_after_timeout_is_released(cx: &mut TestAppContext) {
            let released = Arc::new(AtomicUsize::new(0));
            let (guard_tx, guard_rx) = oneshot::channel();
            let task = cx.background_executor.spawn({
                let executor = cx.background_executor.clone();
                async move { await_idle_sleep_prevention(guard_rx, &executor).await }
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_secs(9));
            cx.run_until_parked();
            assert!(!guard_tx.is_canceled());
            cx.executor().advance_clock(Duration::from_secs(1));
            assert_eq!(
                task.await
                    .err()
                    .expect("acquisition should time out")
                    .to_string(),
                "Idle sleep prevention acquisition timed out after 10 seconds"
            );

            assert!(guard_tx.is_canceled());
            if let Err(Ok(guard)) = guard_tx.send(Ok(release_guard(released.clone()))) {
                assert_eq!(released.load(SeqCst), 0);
                drop(guard);
            }
            assert_eq!(released.load(SeqCst), 1);
        }

        fn release_guard(released: Arc<AtomicUsize>) -> ActivityGuard {
            ActivityGuard::new(move || {
                released.fetch_add(1, SeqCst);
            })
        }
    }

    #[cfg(any(feature = "wayland", feature = "x11"))]
    mod read_fd_with_timeout {
        use super::super::{PIPE_READ_TIMEOUT, read_fd_with_timeout};
        use std::io::Write as _;
        use std::time::{Duration, Instant};

        #[test]
        fn reads_data_written_before_close() {
            let mut pipe = filedescriptor::Pipe::new().unwrap();
            pipe.write.write_all(b"hello clipboard").unwrap();
            drop(pipe.write);

            let bytes = read_fd_with_timeout(pipe.read, PIPE_READ_TIMEOUT).unwrap();
            assert_eq!(bytes, b"hello clipboard");
        }

        #[test]
        fn returns_empty_when_writer_closes_without_writing() {
            let pipe = filedescriptor::Pipe::new().unwrap();
            drop(pipe.write);

            let bytes = read_fd_with_timeout(pipe.read, PIPE_READ_TIMEOUT).unwrap();
            assert!(bytes.is_empty());
        }

        #[test]
        fn times_out_when_writer_never_writes() {
            let pipe = filedescriptor::Pipe::new().unwrap();
            let _open_writer = pipe.write;

            let timeout = Duration::from_millis(50);
            let started = Instant::now();
            let result = read_fd_with_timeout(pipe.read, timeout);
            let elapsed = started.elapsed();

            let err = result.unwrap_err();
            assert!(
                err.to_string().contains("timed out"),
                "unexpected error: {err}"
            );
            assert!(elapsed >= timeout, "returned before the timeout elapsed");
        }

        #[test]
        fn times_out_when_writer_stalls_after_partial_write() {
            let mut pipe = filedescriptor::Pipe::new().unwrap();
            pipe.write.write_all(b"partial").unwrap();
            let _open_writer = pipe.write;

            let err = read_fd_with_timeout(pipe.read, Duration::from_millis(50)).unwrap_err();
            assert!(
                err.to_string().contains("timed out"),
                "unexpected error: {err}"
            );
        }

        #[test]
        fn slow_writer_resets_deadline_between_chunks() {
            let pipe = filedescriptor::Pipe::new().unwrap();
            let chunks = 12;
            let gap = Duration::from_millis(40);
            let timeout = Duration::from_millis(400);

            let writer = std::thread::spawn({
                let mut write = pipe.write;
                move || {
                    for _ in 0..chunks {
                        std::thread::sleep(gap);
                        write.write_all(&[b'x'; 1000]).unwrap();
                    }
                }
            });
            // The total transfer (~480ms) exceeds the timeout; this only
            // passes because the timeout is re-armed per chunk.
            let bytes = read_fd_with_timeout(pipe.read, timeout).unwrap();
            writer.join().unwrap();
            assert_eq!(bytes, vec![b'x'; 1000 * chunks]);
        }

        #[test]
        fn reads_payload_larger_than_pipe_capacity() {
            let pipe = filedescriptor::Pipe::new().unwrap();
            // Exceeds the 64 KiB pipe capacity, forcing the writer to block.
            let payload = vec![b'z'; 1024 * 1024];

            let writer = std::thread::spawn({
                let mut write = pipe.write;
                let payload = payload.clone();
                move || write.write_all(&payload).unwrap()
            });
            let bytes = read_fd_with_timeout(pipe.read, PIPE_READ_TIMEOUT).unwrap();
            writer.join().unwrap();
            assert_eq!(bytes, payload);
        }
    }

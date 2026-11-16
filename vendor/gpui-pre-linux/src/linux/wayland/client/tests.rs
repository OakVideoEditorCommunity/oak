    use std::cell::Cell;

    use super::*;

    #[test]
    fn rejects_required_globals_below_supported_versions() {
        assert!(wl_seat_version(4).is_err());
        assert!(wl_output_version(1).is_err());
        assert_eq!(wl_seat_version(5).expect("minimum seat version"), 5);
        assert_eq!(wl_output_version(2).expect("minimum output version"), 2);
    }

    #[test]
    fn frame_callback_timestamp_preserves_queue_delay() {
        let received_at = Instant::now();
        assert_eq!(
            frame_callback_instant(1_000, 1_025, received_at),
            received_at - Duration::from_millis(25),
        );
        assert_eq!(
            frame_callback_instant(1_000, 1_000, received_at),
            received_at,
        );
    }

    #[test]
    fn frame_callback_timestamp_wraps() {
        let received_at = Instant::now();
        assert_eq!(
            frame_callback_instant(u32::MAX - 9, 15, received_at),
            received_at - Duration::from_millis(25),
        );
    }

    #[test]
    fn frame_callback_timestamp_rejects_implausible_clock() {
        let received_at = Instant::now();
        for (callback_data, monotonic_millis) in [(1_001, 1_000), (0, 60_001), (0, u32::MAX)] {
            assert_eq!(
                frame_callback_instant(callback_data, monotonic_millis, received_at),
                received_at,
            );
        }
        assert_eq!(
            frame_callback_instant(0, 60_000, received_at),
            received_at - Duration::from_secs(60),
        );
    }

    #[derive(Clone)]
    struct FakeDataOffer {
        name: &'static str,
        finished: Rc<Cell<bool>>,
        destroyed: Rc<Cell<bool>>,
    }

    impl FakeDataOffer {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                finished: Rc::new(Cell::new(false)),
                destroyed: Rc::new(Cell::new(false)),
            }
        }
    }

    impl FileDragDataOffer for FakeDataOffer {
        fn finish(&self) {
            self.finished.set(true);
        }

        fn destroy(&self) {
            self.destroyed.set(true);
        }
    }

    fn drag_state() -> DragState<FakeDataOffer, &'static str> {
        DragState {
            data_offer: None,
            window: None,
            position: Point::default(),
            uri_read_generation: 0,
        }
    }

    fn complete_drag_uri_read(
        drag: &mut DragState<FakeDataOffer, &'static str>,
        generation: u64,
        data_offer: FakeDataOffer,
        window: &'static str,
    ) -> Option<(&'static str, PlatformInput)> {
        drag.complete_uri_read(
            generation,
            data_offer,
            window,
            Point::default(),
            gpui::ExternalPaths(SmallVec::from_vec(vec![PathBuf::from("/tmp/file")])),
        )
    }

    fn record_entered(
        input: Option<(&'static str, PlatformInput)>,
        entered_windows: &mut Vec<&'static str>,
    ) {
        if let Some((window, PlatformInput::FileDrop(FileDropEvent::Entered { .. }))) = input {
            entered_windows.push(window);
        }
    }

    #[test]
    fn leave_discards_pending_drag_uri_read() {
        let mut drag = drag_state();
        let data_offer = FakeDataOffer::new("A");
        let generation = drag.begin_uri_read();
        let mut entered_windows = Vec::new();

        record_entered(drag.handle_leave(), &mut entered_windows);
        record_entered(
            complete_drag_uri_read(&mut drag, generation, data_offer.clone(), "window A"),
            &mut entered_windows,
        );

        assert!(entered_windows.is_empty());
        assert!(data_offer.destroyed.get());
        assert!(drag.data_offer.is_none());
        assert!(drag.window.is_none());
    }

    #[test]
    fn drop_then_leave_discards_pending_drag_uri_read() {
        let mut drag = drag_state();
        let data_offer = FakeDataOffer::new("A");
        let generation = drag.begin_uri_read();
        let mut entered_windows = Vec::new();

        record_entered(drag.handle_drop(), &mut entered_windows);
        assert_ne!(drag.uri_read_generation, generation);
        record_entered(drag.handle_leave(), &mut entered_windows);
        record_entered(
            complete_drag_uri_read(&mut drag, generation, data_offer.clone(), "window A"),
            &mut entered_windows,
        );

        assert!(entered_windows.is_empty());
        assert!(!data_offer.finished.get());
        assert!(data_offer.destroyed.get());
        assert!(drag.data_offer.is_none());
        assert!(drag.window.is_none());
    }

    #[test]
    fn stale_completion_preserves_newer_drag_destination() {
        let mut drag = drag_state();
        let data_offer_a = FakeDataOffer::new("A");
        let generation_a = drag.begin_uri_read();
        let mut entered_windows = Vec::new();
        record_entered(drag.handle_leave(), &mut entered_windows);

        let data_offer_b = FakeDataOffer::new("B");
        let generation_b = drag.begin_uri_read();
        record_entered(
            complete_drag_uri_read(&mut drag, generation_b, data_offer_b.clone(), "window B"),
            &mut entered_windows,
        );
        record_entered(
            complete_drag_uri_read(&mut drag, generation_a, data_offer_a.clone(), "window A"),
            &mut entered_windows,
        );

        assert_eq!(entered_windows, ["window B"]);
        assert!(data_offer_a.destroyed.get());
        assert!(!data_offer_b.destroyed.get());
        assert_eq!(drag.data_offer.as_ref().map(|offer| offer.name), Some("B"));
        assert_eq!(drag.window, Some("window B"));
    }

    #[derive(Default)]
    struct FakeImeCursorRectangleSink {
        cursor_rectangles: RefCell<Vec<(i32, i32, i32, i32)>>,
        commit_count: Cell<usize>,
    }

    impl ImeCursorRectangleSink for FakeImeCursorRectangleSink {
        fn set_ime_cursor_rectangle(&self, x: i32, y: i32, width: i32, height: i32) {
            self.cursor_rectangles
                .borrow_mut()
                .push((x, y, width, height));
        }

        fn commit_ime_state(&self) {
            self.commit_count.set(self.commit_count.get() + 1);
        }
    }

    #[test]
    fn builds_terminated_uri_list_for_each_dragged_path() {
        let paths = FileDragPaths::new([
            (PathBuf::from("/tmp/first"), false),
            (PathBuf::from("/tmp/nested directory"), true),
        ]);

        assert_eq!(
            file_uri_list(&paths),
            "file:///tmp/first\r\nfile:///tmp/nested%20directory\r\n"
        );
    }

    #[test]
    fn skips_paths_that_have_no_file_url() {
        let paths = FileDragPaths::new([
            (PathBuf::from("relative/path"), false),
            (PathBuf::from("/tmp/absolute"), false),
        ]);

        assert_eq!(file_uri_list(&paths), "file:///tmp/absolute\r\n");
    }

    #[test]
    fn builds_empty_uri_list_without_convertible_paths() {
        assert!(file_uri_list(&FileDragPaths::default()).is_empty());
        assert!(
            file_uri_list(&FileDragPaths::new([(PathBuf::from("relative"), false)])).is_empty()
        );
    }

    fn ime_cursor_bounds(x: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(20.25)), size(px(1.0), px(18.75)))
    }

    #[test]
    fn caches_cursor_rectangle_committed_after_done() {
        let text_input = FakeImeCursorRectangleSink::default();
        let mut last_ime_cursor_rectangle = None;
        let initial_bounds = ime_cursor_bounds(10.0);
        let updated_bounds = ime_cursor_bounds(20.0);

        update_ime_cursor_rectangle(&text_input, &mut last_ime_cursor_rectangle, initial_bounds);
        set_ime_cursor_rectangle_after_done(
            &text_input,
            &mut last_ime_cursor_rectangle,
            updated_bounds,
            true,
        );
        update_ime_cursor_rectangle(&text_input, &mut last_ime_cursor_rectangle, updated_bounds);

        assert_eq!(text_input.commit_count.get(), 2);
        assert_eq!(text_input.cursor_rectangles.borrow().len(), 2);
    }

    #[test]
    fn skips_unchanged_cursor_rectangle_after_done() {
        let text_input = FakeImeCursorRectangleSink::default();
        let mut last_ime_cursor_rectangle = None;
        let bounds = ime_cursor_bounds(10.0);

        update_ime_cursor_rectangle(&text_input, &mut last_ime_cursor_rectangle, bounds);
        set_ime_cursor_rectangle_after_done(
            &text_input,
            &mut last_ime_cursor_rectangle,
            bounds,
            true,
        );

        assert_eq!(text_input.commit_count.get(), 1);
        assert_eq!(text_input.cursor_rectangles.borrow().len(), 1);
    }

    #[test]
    fn skips_cursor_rectangles_with_unchanged_protocol_coordinates() {
        let text_input = FakeImeCursorRectangleSink::default();
        let mut last_ime_cursor_rectangle = None;

        update_ime_cursor_rectangle(
            &text_input,
            &mut last_ime_cursor_rectangle,
            ime_cursor_bounds(10.25),
        );
        update_ime_cursor_rectangle(
            &text_input,
            &mut last_ime_cursor_rectangle,
            ime_cursor_bounds(10.75),
        );

        assert_eq!(text_input.commit_count.get(), 1);
        assert_eq!(
            text_input.cursor_rectangles.borrow().as_slice(),
            &[(10, 20, 1, 18)]
        );
    }

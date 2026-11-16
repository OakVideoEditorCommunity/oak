    use super::*;
    use crate::{
        AnyWindowHandle, AppContext as _, Context, GestureTuning, InputEvent, Keystroke,
        MouseMoveEvent, TestAppContext, TouchEvent, TouchId, canvas, util::FluentBuilder as _,
    };
    use std::{cell::Cell, rc::Weak};

    struct GroupHoverTestView {
        render_count: Rc<Cell<usize>>,
        anonymous_paint_count: Rc<Cell<usize>>,
        stateful_width: Rc<Cell<Pixels>>,
    }

    impl Render for GroupHoverTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            self.render_count.set(self.render_count.get() + 1);
            let anonymous_paint_count = self.anonymous_paint_count.clone();
            let stateful_width = self.stateful_width.clone();
            div().size_full().child(
                div()
                    .ml(px(20.))
                    .mt(px(20.))
                    .size(px(50.))
                    .relative()
                    .group("hover-group")
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .invisible()
                            .group_hover("hover-group", |style| style.visible())
                            .child(canvas(
                                |_, _, _| {},
                                move |_, _, _, _| {
                                    anonymous_paint_count.set(anonymous_paint_count.get() + 1)
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("stateful-group-hover-target")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size(px(10.))
                            .group_hover("hover-group", |style| style.size(px(20.)))
                            .child(canvas(
                                move |bounds, _, _| stateful_width.set(bounds.size.width),
                                |_, _, _, _| {},
                            )),
                    ),
            )
        }
    }

    #[gpui::test]
    fn group_hover_styles_update_only_on_transitions(cx: &mut TestAppContext) {
        let render_count = Rc::new(Cell::new(0));
        let anonymous_paint_count = Rc::new(Cell::new(0));
        let stateful_width = Rc::new(Cell::new(px(0.)));
        let window = cx.add_window({
            let render_count = render_count.clone();
            let anonymous_paint_count = anonymous_paint_count.clone();
            let stateful_width = stateful_width.clone();
            move |_, _| GroupHoverTestView {
                render_count,
                anonymous_paint_count,
                stateful_width,
            }
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(anonymous_paint_count.get(), 0);
        assert_eq!(stateful_width.get(), px(10.));

        let move_mouse = |cx: &mut TestAppContext, position| {
            cx.update_window(window, |_, window, cx| {
                window.simulate_mouse_move(position, cx)
            })
            .unwrap();
        };

        let initial_render_count = render_count.get();
        move_mouse(cx, point(px(25.), px(25.)));
        assert_eq!(render_count.get(), initial_render_count + 1);
        assert_eq!(anonymous_paint_count.get(), 1);
        assert_eq!(stateful_width.get(), px(20.));

        move_mouse(cx, point(px(30.), px(30.)));
        assert_eq!(render_count.get(), initial_render_count + 1);
        assert_eq!(anonymous_paint_count.get(), 1);
        assert_eq!(stateful_width.get(), px(20.));

        move_mouse(cx, point(px(5.), px(5.)));
        assert_eq!(render_count.get(), initial_render_count + 2);
        assert_eq!(anonymous_paint_count.get(), 1);
        assert_eq!(stateful_width.get(), px(10.));

        move_mouse(cx, point(px(10.), px(10.)));
        assert_eq!(render_count.get(), initial_render_count + 2);
        assert_eq!(anonymous_paint_count.get(), 1);
        assert_eq!(stateful_width.get(), px(10.));
    }

    struct HoverListenerLayoutTestView {
        target_left: Pixels,
        hover_transitions: Rc<RefCell<Vec<bool>>>,
    }

    impl Render for HoverListenerLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let hover_transitions = self.hover_transitions.clone();
            div().relative().size_full().child(
                div()
                    .id("hover-target")
                    .absolute()
                    .left(self.target_left)
                    .top_0()
                    .size(px(20.))
                    .on_click(|_, _, _| {})
                    .on_hover(move |is_hovered, _, _| {
                        hover_transitions.borrow_mut().push(*is_hovered);
                    }),
            )
        }
    }

    #[gpui::test]
    fn default_hover_listener_updates_when_layout_changes_under_stationary_mouse(
        cx: &mut TestAppContext,
    ) {
        let hover_transitions = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let hover_transitions = hover_transitions.clone();
            move |_, _| HoverListenerLayoutTestView {
                target_left: px(40.),
                hover_transitions,
            }
        });
        let any_window = AnyWindowHandle::from(window);

        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(point(px(10.), px(10.)), cx);
        })
        .unwrap();
        assert!(hover_transitions.borrow().is_empty());

        window
            .update(cx, |view, _, cx| {
                view.target_left = px(0.);
                cx.notify();
            })
            .unwrap();
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        window
            .update(cx, |view, _, cx| {
                view.target_left = px(40.);
                cx.notify();
            })
            .unwrap();
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true, false]);
    }

    #[gpui::test]
    fn default_hover_listener_ends_after_key_press(cx: &mut TestAppContext) {
        let hover_transitions = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let hover_transitions = hover_transitions.clone();
            move |_, _| HoverListenerLayoutTestView {
                target_left: px(0.),
                hover_transitions,
            }
        });
        let any_window = AnyWindowHandle::from(window);

        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(point(px(10.), px(10.)), cx);
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        key_down(cx, any_window, "a");
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true, false]);
    }

    struct HoverListenerModeLayoutTestView {
        target_left: Pixels,
        hover_transitions: Rc<RefCell<Vec<bool>>>,
    }

    impl Render for HoverListenerModeLayoutTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let hover_transitions = self.hover_transitions.clone();
            div().relative().size_full().child(
                div()
                    .id("hover-target")
                    .absolute()
                    .left(self.target_left)
                    .top_0()
                    .size(px(20.))
                    .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
                    .on_hover(move |is_hovered, _, _| {
                        hover_transitions.borrow_mut().push(*is_hovered);
                    }),
            )
        }
    }

    #[gpui::test]
    fn input_modality_independent_hover_listener_updates_after_key_press(cx: &mut TestAppContext) {
        let hover_transitions = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let hover_transitions = hover_transitions.clone();
            move |_, _| HoverListenerModeLayoutTestView {
                target_left: px(40.),
                hover_transitions,
            }
        });
        let any_window = AnyWindowHandle::from(window);
        let pointer_position = point(px(10.), px(10.));

        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(pointer_position, cx);
        })
        .unwrap();
        assert!(hover_transitions.borrow().is_empty());

        key_down(cx, any_window, "a");
        window
            .update(cx, |view, _, cx| {
                view.target_left = px(0.);
                cx.notify();
            })
            .unwrap();
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        key_down(cx, any_window, "b");
        cx.update_window(any_window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        cx.update_window(any_window, |_, window, cx| {
            window.simulate_mouse_move(point(px(30.), px(10.)), cx);
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true, false]);

        cx.update_window(any_window, |_, window, cx| {
            window.simulate_mouse_move(pointer_position, cx);
            window.dispatch_event(
                MouseExitEvent {
                    position: pointer_position,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true, false, true, false]);
    }

    #[gpui::test]
    fn default_hover_listener_remains_hovered_during_stationary_mouse_press(
        cx: &mut TestAppContext,
    ) {
        let hover_transitions = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let hover_transitions = hover_transitions.clone();
            move |_, _| HoverListenerLayoutTestView {
                target_left: px(0.),
                hover_transitions,
            }
        });
        let any_window = AnyWindowHandle::from(window);
        let mouse_position = point(px(10.), px(10.));

        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(mouse_position, cx);
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        cx.update_window(any_window, |_, window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: mouse_position,
                    button: MouseButton::Left,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);

        cx.update_window(any_window, |_, window, cx| {
            window.dispatch_event(
                MouseUpEvent {
                    position: mouse_position,
                    button: MouseButton::Left,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(*hover_transitions.borrow(), [true]);
    }

    struct TestTooltipView;

    impl Render for TestTooltipView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(20.)).h(px(20.)).child("tooltip")
        }
    }

    type CapturedActiveTooltip = Rc<RefCell<Option<Weak<RefCell<Option<ActiveTooltip>>>>>>;

    struct TooltipCaptureElement {
        child: AnyElement,
        captured_active_tooltip: CapturedActiveTooltip,
    }

    impl IntoElement for TooltipCaptureElement {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl Element for TooltipCaptureElement {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            (self.child.request_layout(window, cx), ())
        }

        fn prepaint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            self.child.prepaint(window, cx);
        }

        fn paint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut Self::RequestLayoutState,
            _prepaint: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.child.paint(window, cx);
            window.with_global_id("target".into(), |global_id, window| {
                window.with_element_state::<InteractiveElementState, _>(
                    global_id,
                    |state, _window| {
                        let state = state.unwrap();
                        *self.captured_active_tooltip.borrow_mut() =
                            state.active_tooltip.as_ref().map(Rc::downgrade);
                        ((), state)
                    },
                )
            });
        }
    }

    struct TooltipOwner {
        captured_active_tooltip: CapturedActiveTooltip,
        show_delay_override: Option<Duration>,
    }

    impl Render for TooltipOwner {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            TooltipCaptureElement {
                child: div()
                    .size_full()
                    .child(
                        div()
                            .id("target")
                            .w(px(50.))
                            .h(px(50.))
                            .tooltip(|_, cx| cx.new(|_| TestTooltipView).into())
                            .when_some(self.show_delay_override, |this, delay| {
                                this.tooltip_show_delay(delay)
                            }),
                    )
                    .into_any_element(),
                captured_active_tooltip: self.captured_active_tooltip.clone(),
            }
        }
    }

    #[test]
    fn scroll_handle_aligns_wide_children_to_left_edge() {
        let handle = ScrollHandle::new();
        {
            let mut state = handle.0.borrow_mut();
            state.bounds = Bounds::new(point(px(0.), px(0.)), size(px(80.), px(20.)));
            state.child_bounds = vec![Bounds::new(point(px(25.), px(0.)), size(px(200.), px(20.)))];
            state.overflow.x = Overflow::Scroll;
            state.active_item = Some(ScrollActiveItem {
                index: 0,
                strategy: ScrollStrategy::default(),
            });
        }

        handle.scroll_to_active_item();

        assert_eq!(handle.offset().x, px(-25.));
    }

    #[test]
    fn scroll_handle_aligns_tall_children_to_top_edge() {
        let handle = ScrollHandle::new();
        {
            let mut state = handle.0.borrow_mut();
            state.bounds = Bounds::new(point(px(0.), px(0.)), size(px(20.), px(80.)));
            state.child_bounds = vec![Bounds::new(point(px(0.), px(25.)), size(px(20.), px(200.)))];
            state.overflow.y = Overflow::Scroll;
            state.active_item = Some(ScrollActiveItem {
                index: 0,
                strategy: ScrollStrategy::default(),
            });
        }

        handle.scroll_to_active_item();

        assert_eq!(handle.offset().y, px(-25.));
    }

    #[test]
    fn long_press_shows_tooltip_until_touch_ends() {
        let (mut test_app, any_window, captured_active_tooltip) = create_tooltip_owner_test(None);
        let active_tooltip = captured_active_tooltip
            .borrow()
            .clone()
            .unwrap()
            .upgrade()
            .unwrap();
        let touch_id = TouchId(1);
        let touch_position = point(px(49.), px(10.));

        test_app
            .update_window(any_window, |_, window, cx| {
                window.dispatch_event(
                    TouchEvent {
                        id: touch_id,
                        phase: TouchPhase::Started,
                        position: touch_position,
                        predicted_position: None,
                        force: None,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        test_app
            .dispatcher
            .advance_clock(GestureTuning::default().long_press_duration + Duration::from_millis(1));
        test_app.run_until_parked();

        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::Visible { .. })
        ));

        let moved_position = point(px(55.), px(10.));
        test_app
            .update_window(any_window, |_, window, cx| {
                window.dispatch_event(
                    TouchEvent {
                        id: touch_id,
                        phase: TouchPhase::Moved,
                        position: moved_position,
                        predicted_position: None,
                        force: None,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.draw(cx).clear(cx);
            })
            .unwrap();
        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::Visible { .. })
        ));

        test_app
            .update_window(any_window, |_, window, cx| {
                window.dispatch_event(
                    TouchEvent {
                        id: touch_id,
                        phase: TouchPhase::Ended,
                        position: moved_position,
                        predicted_position: None,
                        force: None,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();

        assert!(active_tooltip.borrow().is_none());
    }

    fn setup_tooltip_owner_test(
        show_delay_override: Option<Duration>,
    ) -> (
        TestAppContext,
        crate::AnyWindowHandle,
        CapturedActiveTooltip,
    ) {
        let (mut test_app, any_window, captured_active_tooltip) =
            create_tooltip_owner_test(show_delay_override);

        test_app
            .update_window(any_window, |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(10.), px(10.)),
                        modifiers: Default::default(),
                        pressed_button: None,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();

        test_app
            .update_window(any_window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();

        (test_app, any_window, captured_active_tooltip)
    }

    fn create_tooltip_owner_test(
        show_delay_override: Option<Duration>,
    ) -> (
        TestAppContext,
        crate::AnyWindowHandle,
        CapturedActiveTooltip,
    ) {
        let mut test_app = TestAppContext::single();
        let captured_active_tooltip: CapturedActiveTooltip = Rc::new(RefCell::new(None));
        let window = test_app.add_window({
            let captured_active_tooltip = captured_active_tooltip.clone();
            move |_, _| TooltipOwner {
                captured_active_tooltip,
                show_delay_override,
            }
        });
        let any_window = window.into();

        test_app
            .update_window(any_window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();

        (test_app, any_window, captured_active_tooltip)
    }

    #[test]
    fn tooltip_waiting_for_show_is_released_when_its_owner_disappears() {
        let (mut test_app, any_window, captured_active_tooltip) = setup_tooltip_owner_test(None);

        let weak_active_tooltip = captured_active_tooltip.borrow().clone().unwrap();
        let active_tooltip = weak_active_tooltip.upgrade().unwrap();
        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::WaitingForShow { .. })
        ));

        test_app
            .update_window(any_window, |_, window, _| {
                window.remove_window();
            })
            .unwrap();
        test_app.run_until_parked();
        drop(active_tooltip);

        assert!(weak_active_tooltip.upgrade().is_none());
    }

    #[test]
    fn tooltip_respects_custom_show_delay() {
        let extra_delay = Duration::from_secs(1);
        let show_delay_override = DEFAULT_TOOLTIP_SHOW_DELAY + extra_delay;
        let (mut test_app, _any_window, captured_active_tooltip) =
            setup_tooltip_owner_test(Some(show_delay_override));

        let weak_active_tooltip = captured_active_tooltip.borrow().clone().unwrap();
        let active_tooltip = weak_active_tooltip.upgrade().unwrap();

        test_app
            .dispatcher
            .advance_clock(DEFAULT_TOOLTIP_SHOW_DELAY);
        test_app.run_until_parked();

        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::WaitingForShow { .. })
        ));

        test_app.dispatcher.advance_clock(extra_delay);
        test_app.run_until_parked();

        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::Visible { .. })
        ));
    }

    #[test]
    fn tooltip_is_released_when_its_owner_disappears() {
        let (mut test_app, any_window, captured_active_tooltip) = setup_tooltip_owner_test(None);

        let weak_active_tooltip = captured_active_tooltip.borrow().clone().unwrap();
        let active_tooltip = weak_active_tooltip.upgrade().unwrap();

        test_app
            .dispatcher
            .advance_clock(DEFAULT_TOOLTIP_SHOW_DELAY);
        test_app.run_until_parked();

        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::Visible { .. })
        ));

        test_app
            .update_window(any_window, |_, window, _| {
                window.remove_window();
            })
            .unwrap();
        test_app.run_until_parked();
        drop(active_tooltip);

        assert!(weak_active_tooltip.upgrade().is_none());
    }

    #[test]
    fn tooltip_hides_after_mouse_leaves_origin() {
        let (mut test_app, any_window, captured_active_tooltip) = setup_tooltip_owner_test(None);

        let weak_active_tooltip = captured_active_tooltip.borrow().clone().unwrap();
        let active_tooltip = weak_active_tooltip.upgrade().unwrap();

        test_app
            .dispatcher
            .advance_clock(DEFAULT_TOOLTIP_SHOW_DELAY);
        test_app.run_until_parked();

        assert!(matches!(
            active_tooltip.borrow().as_ref(),
            Some(ActiveTooltip::Visible { .. })
        ));

        test_app
            .update_window(any_window, |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(75.), px(75.)),
                        modifiers: Default::default(),
                        pressed_button: None,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();

        assert!(active_tooltip.borrow().is_none());
    }

    struct MouseDownOutOwner {
        mouse_down_out_count: Rc<RefCell<usize>>,
    }

    impl Render for MouseDownOutOwner {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mouse_down_out_count = self.mouse_down_out_count.clone();
            div()
                .size_full()
                .child(div().id("target").w(px(50.)).h(px(50.)).on_mouse_down_out(
                    move |_, _, _| {
                        *mouse_down_out_count.borrow_mut() += 1;
                    },
                ))
        }
    }

    #[test]
    fn mouse_down_out_is_suppressed_while_window_prompt_is_active() {
        let mut test_app = TestAppContext::single();
        let mouse_down_out_count = Rc::new(RefCell::new(0));
        let window = test_app.add_window({
            let mouse_down_out_count = mouse_down_out_count.clone();
            move |_, _| MouseDownOutOwner {
                mouse_down_out_count,
            }
        });
        let any_window: AnyWindowHandle = window.into();

        fn dispatch_mouse_down_outside_target(
            test_app: &mut TestAppContext,
            any_window: AnyWindowHandle,
        ) {
            test_app
                .update_window(any_window, |_, window, cx| {
                    window.dispatch_event(
                        MouseDownEvent {
                            position: point(px(75.), px(75.)),
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                            first_mouse: false,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        }

        test_app
            .update_window(any_window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();

        dispatch_mouse_down_outside_target(&mut test_app, any_window);
        assert_eq!(
            *mouse_down_out_count.borrow(),
            1,
            "mouse down outside the element should fire mouse-down-out listeners"
        );

        test_app
            .update_window(any_window, |_, window, cx| {
                cx.set_prompt_builder(crate::fallback_prompt_renderer);
                let _receiver =
                    window.prompt(crate::PromptLevel::Warning, "message", None, &["Ok"], cx);
                assert!(window.has_active_prompt());
                window.draw(cx).clear(cx);
            })
            .unwrap();

        dispatch_mouse_down_outside_target(&mut test_app, any_window);
        assert_eq!(
            *mouse_down_out_count.borrow(),
            1,
            "mouse down over an active prompt should not fire mouse-down-out listeners"
        );
    }

    #[test]
    fn test_accessibility_id_builder_writes_author_id() {
        let mut element = div()
            .id("buffer-font-size")
            .accessibility_id("settings.buffer-font-size");
        let mut node = accesskit::Node::new(accesskit::Role::SpinButton);

        element.interactivity().write_a11y_info(&mut node);

        assert_eq!(node.author_id(), Some("settings.buffer-font-size"));
    }

    #[test]
    fn test_write_a11y_info_string_and_numeric_properties() {
        let mut interactivity = Interactivity::default();
        interactivity.aria.author_id = Some("settings.buffer-font-size".into());
        interactivity.aria.label = Some("Buffer Font Size".into());
        interactivity.aria.value = Some("15".into());
        interactivity.aria.placeholder = Some("Search".into());
        interactivity.aria.numeric_value = Some(15.0);
        interactivity.aria.min_numeric_value = Some(6.0);
        interactivity.aria.max_numeric_value = Some(72.0);
        interactivity.aria.numeric_value_step = Some(1.0);

        let mut node = accesskit::Node::new(accesskit::Role::SpinButton);
        interactivity.write_a11y_info(&mut node);

        assert_eq!(node.author_id(), Some("settings.buffer-font-size"));
        assert_eq!(node.label(), Some("Buffer Font Size"));
        assert_eq!(node.value(), Some("15"));
        assert_eq!(node.placeholder(), Some("Search"));
        assert_eq!(node.numeric_value(), Some(15.0));
        assert_eq!(node.min_numeric_value(), Some(6.0));
        assert_eq!(node.max_numeric_value(), Some(72.0));
        assert_eq!(node.numeric_value_step(), Some(1.0));
    }

    /// Two focusable, clickable elements ("a" and "b") used to exercise the
    /// Enter/Space -> synthesized click press/release pairing.
    struct KeyboardActivationTest {
        focus_a: FocusHandle,
        focus_b: FocusHandle,
        clicks: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for KeyboardActivationTest {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let clicks_a = self.clicks.clone();
            let clicks_b = self.clicks.clone();
            div()
                .size_full()
                .child(
                    div()
                        .id("a")
                        .w(px(50.))
                        .h(px(50.))
                        .track_focus(&self.focus_a)
                        .on_click(move |_, _, _| clicks_a.borrow_mut().push("a")),
                )
                .child(
                    div()
                        .id("b")
                        .w(px(50.))
                        .h(px(50.))
                        .track_focus(&self.focus_b)
                        .on_click(move |_, _, _| clicks_b.borrow_mut().push("b")),
                )
        }
    }

    fn setup_keyboard_activation_test() -> (
        TestAppContext,
        AnyWindowHandle,
        Rc<RefCell<Vec<&'static str>>>,
        FocusHandle,
        FocusHandle,
    ) {
        let mut cx = TestAppContext::single();
        let (focus_a, focus_b) = cx.update(|cx| (cx.focus_handle(), cx.focus_handle()));
        let clicks: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let focus_a = focus_a.clone();
            let focus_b = focus_b.clone();
            let clicks = clicks.clone();
            move |_, _| KeyboardActivationTest {
                focus_a,
                focus_b,
                clicks,
            }
        });
        (cx, window.into(), clicks, focus_a, focus_b)
    }

    /// Move focus to `handle`, flush effects, then paint so the newly focused
    /// element registers its key handlers for the next dispatched event.
    fn focus_and_draw(cx: &mut TestAppContext, window: AnyWindowHandle, handle: &FocusHandle) {
        cx.update_window(window, |_, window, cx| window.focus(handle, cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    fn key_down(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                KeyDownEvent {
                    keystroke,
                    is_held: false,
                    prefer_character_input: false,
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    }

    fn key_up(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        })
        .unwrap();
    }

    /// Pressing and releasing Enter on the same focused element fires a click.
    #[test]
    fn keyboard_activation_fires_click_on_same_element() {
        let (mut cx, window, clicks, focus_a, _focus_b) = setup_keyboard_activation_test();

        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "enter");
        key_up(&mut cx, window, "enter");

        assert_eq!(*clicks.borrow(), vec!["a"]);
    }

    /// A key-down whose key-up lands on a *different* element (because focus
    /// moved in between) must not leak a synthesized click onto the newly
    /// focused element. This is the core regression: previously the key-up
    /// handler fired unconditionally on whatever was focused at key-up time.
    #[test]
    fn keyboard_activation_does_not_leak_across_focus_change() {
        let (mut cx, window, clicks, focus_a, focus_b) = setup_keyboard_activation_test();

        // Enter pressed while "a" is focused...
        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "enter");

        // ...focus moves to "b" before the release (as a confirm action would)...
        focus_and_draw(&mut cx, window, &focus_b);
        key_up(&mut cx, window, "enter");

        // ...so neither element is clicked: "a" never saw the up, and "b"
        // never saw the down.
        assert!(clicks.borrow().is_empty(), "clicks: {:?}", clicks.borrow());
    }

    /// A keydown whose flag is left pending because focus moved away before
    /// the keyup must not fire a click when focus later *returns* to the same
    /// element (the menu trigger reopening case). The stamped focus generation
    /// no longer matches, so the stale pending state is ignored.
    #[test]
    fn keyboard_activation_does_not_leak_when_focus_returns() {
        let (mut cx, window, clicks, focus_a, focus_b) = setup_keyboard_activation_test();

        // Enter pressed on "a"...
        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "enter");

        // ...focus leaves "a" before its keyup (so the pending state is never
        // consumed), then comes back to "a"...
        focus_and_draw(&mut cx, window, &focus_b);
        focus_and_draw(&mut cx, window, &focus_a);
        key_up(&mut cx, window, "enter");

        // ...and the now-stale pending keydown must not fire a click.
        assert!(clicks.borrow().is_empty(), "clicks: {:?}", clicks.borrow());
    }

    /// A non-activation key *released* during the press must cancel the pending
    /// activation. For the sequence escape-down, space-down, escape-up,
    /// space-up the space forms a clean down/up pair, but the intervening
    /// escape-up means this isn't a plain space activation, so no click fires.
    #[test]
    fn keyboard_activation_cleared_by_intervening_key_release() {
        let (mut cx, window, clicks, focus_a, _focus_b) = setup_keyboard_activation_test();

        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "escape");
        key_down(&mut cx, window, "space");
        key_up(&mut cx, window, "escape");
        key_up(&mut cx, window, "space");

        assert!(clicks.borrow().is_empty(), "clicks: {:?}", clicks.borrow());
    }

    /// The flag is a single activation marker, not keyed by which activation
    /// key was used, so a Space down paired with an Enter up on the same
    /// element still fires a click.
    #[test]
    fn keyboard_activation_does_not_distinguish_space_and_enter() {
        let (mut cx, window, clicks, focus_a, _focus_b) = setup_keyboard_activation_test();

        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "space");
        key_up(&mut cx, window, "enter");

        assert_eq!(*clicks.borrow(), vec!["a"]);
    }

    /// A non-activation key pressed between the activation down and up clears
    /// the pending flag, suppressing the click.
    #[test]
    fn keyboard_activation_cleared_by_intervening_keydown() {
        let (mut cx, window, clicks, focus_a, _focus_b) = setup_keyboard_activation_test();

        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "enter");
        key_down(&mut cx, window, "a");
        key_up(&mut cx, window, "enter");

        assert!(clicks.borrow().is_empty(), "clicks: {:?}", clicks.borrow());
    }

    /// A modified Enter (e.g. cmd-enter) is not treated as an activation key,
    /// so it neither sets the pending flag nor fires a click on release.
    #[test]
    fn keyboard_activation_ignores_modified_keys() {
        let (mut cx, window, clicks, focus_a, _focus_b) = setup_keyboard_activation_test();

        focus_and_draw(&mut cx, window, &focus_a);
        key_down(&mut cx, window, "cmd-enter");
        key_up(&mut cx, window, "cmd-enter");

        assert!(clicks.borrow().is_empty(), "clicks: {:?}", clicks.borrow());
    }

    /// Two sibling tab groups, each a focusable container that is *not* itself a
    /// tab stop and holds a single tab stop. Mirrors how the title bar and
    /// status bar expose their controls as ARIA toolbars.
    struct TabGroupFocus {
        group_a: FocusHandle,
        item_a: FocusHandle,
        group_b: FocusHandle,
        item_b: FocusHandle,
    }

    impl Render for TabGroupFocus {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            fn group(container: &FocusHandle, item: &FocusHandle) -> Div {
                div()
                    .track_focus(container)
                    .tab_group()
                    .child(div().track_focus(item))
            }
            div()
                .child(group(&self.group_a, &self.item_a))
                .child(group(&self.group_b, &self.item_b))
        }
    }

    /// Focusing a tab-group container and pressing Tab (`focus_next`) must move
    /// focus to the first tab stop *inside that container*, as documented on
    /// [`InteractiveElement::tab_stop`].
    #[test]
    fn focus_next_from_tab_group_container_enters_that_group() {
        let mut cx = TestAppContext::single();
        let (group_a, item_a, group_b, item_b) = cx.update(|cx| {
            (
                cx.focus_handle(),
                cx.focus_handle().tab_stop(true),
                cx.focus_handle(),
                cx.focus_handle().tab_stop(true),
            )
        });
        let window: AnyWindowHandle = cx
            .add_window({
                let (group_a, item_a, group_b, item_b) =
                    (group_a, item_a, group_b.clone(), item_b.clone());
                move |_, _| TabGroupFocus {
                    group_a,
                    item_a,
                    group_b,
                    item_b,
                }
            })
            .into();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        // Focus the *second* group's container, then advance like Tab would.
        let focused = cx
            .update_window(window, |_, window, cx| {
                window.focus(&group_b, cx);
                window.focus_next(cx);
                window.focused(cx).map(|handle| handle.id)
            })
            .unwrap();

        assert_eq!(focused, Some(item_b.id));
    }

    #[gpui::test]
    fn test_fractional_padding_does_not_make_a_fitting_container_scrollable(
        cx: &mut TestAppContext,
    ) {
        struct PaddedContainer {
            scroll_handle: ScrollHandle,
        }

        impl Render for PaddedContainer {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                // 4.25px of padding snaps to 4px in layout, so a 42px child
                // fits the 50px box exactly.
                div().size_full().child(
                    div()
                        .id("container")
                        .h(px(50.))
                        .w(px(100.))
                        .py(px(4.25))
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll_handle)
                        .child(div().w_full().h(px(42.))),
                )
            }
        }

        let scroll_handle = ScrollHandle::new();
        let window: AnyWindowHandle = cx
            .add_window({
                let scroll_handle = scroll_handle.clone();
                move |_, _| PaddedContainer { scroll_handle }
            })
            .into();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        assert_eq!(scroll_handle.max_offset().y, px(0.));
    }

    struct ContentSizedGrid;

    impl Render for ContentSizedGrid {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let widths = [px(100.), px(200.), px(50.)];
            div().size_full().child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols_max_content(widths.len() as u16)
                    .children(widths.into_iter().enumerate().map(|(index, width)| {
                        div()
                            .debug_selector(move || format!("cell-{index}"))
                            .w(width)
                            .h(px(10.))
                    })),
            )
        }
    }

    #[gpui::test]
    fn grid_cols_max_content_sizes_columns_to_their_content(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| ContentSizedGrid);
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();

        let mut bounds = |selector: &'static str| {
            cx.update_window(window.into(), |_, window, _| {
                window.rendered_frame.debug_bounds.get(selector).copied()
            })
            .unwrap()
            .unwrap_or_else(|| panic!("{selector} was not rendered"))
        };

        assert_eq!(bounds("cell-0").origin.x, px(0.));
        assert_eq!(bounds("cell-1").origin.x, px(100.));
        assert_eq!(bounds("cell-2").origin.x, px(300.));
    }

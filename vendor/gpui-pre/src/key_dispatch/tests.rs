    use crate::{
        self as gpui, AppContext, DispatchResult, Element, ElementId, GlobalElementId,
        InspectorElementId, Keystroke, LayoutId, Style,
    };
    use core::panic;
    use smallvec::SmallVec;
    use std::{
        cell::{Cell, RefCell},
        ops::Range,
        rc::Rc,
    };

    use crate::{
        Action, ActionRegistry, App, Bounds, Context, DispatchEventResult, DispatchPhase,
        DispatchTree, Entity, FocusHandle, InputHandler, InputPreference, IntoElement, KeyBinding,
        KeyContext, KeyDownEvent, Keymap, Modifiers, Pixels, PlatformInput, PlatformWindow, Point,
        Render, Subscription, TestAppContext, UTF16Selection, Unbind, VisualContext,
        VisualTestContext, Window,
    };

    actions!(dispatch_test, [TestAction, SecondaryTestAction]);

    fn test_dispatch_tree(bindings: Vec<KeyBinding>) -> DispatchTree {
        let registry = ActionRegistry::default();

        DispatchTree::new(
            Rc::new(RefCell::new(Keymap::new(bindings))),
            Rc::new(registry),
        )
    }

    struct PendingInputTestView {
        focus_handle: FocusHandle,
        action_count: Rc<Cell<usize>>,
        secondary_action_count: Rc<Cell<usize>>,
    }

    #[derive(Clone)]
    struct PendingTextInputTestView {
        focus_handle: FocusHandle,
        text: Rc<RefCell<String>>,
        action_count: Rc<Cell<usize>>,
        accepts_text_input: Rc<Cell<bool>>,
    }

    impl PendingTextInputTestView {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                text: Rc::default(),
                action_count: Rc::default(),
                accepts_text_input: Rc::new(Cell::new(true)),
            }
        }
    }

    impl Element for PendingTextInputTestView {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            Some("pending-text-input-test".into())
        }

        fn source_location(&self) -> Option<&'static panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            (window.request_layout(Style::default(), [], cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            window.set_focus_handle(&self.focus_handle, cx);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            _: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            let mut key_context = KeyContext::default();
            key_context.add("Terminal");
            window.set_key_context(key_context);
            window.handle_input(&self.focus_handle, self.clone(), cx);
            let action_count = self.action_count.clone();
            window.on_action(
                std::any::TypeId::of::<TestAction>(),
                move |_, phase, _, _| {
                    if phase == DispatchPhase::Bubble {
                        action_count.set(action_count.get() + 1);
                    }
                },
            );
        }
    }

    impl IntoElement for PendingTextInputTestView {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl InputHandler for PendingTextInputTestView {
        fn accepts_text_input(&mut self, _: &mut Window, _: &mut App) -> bool {
            self.accepts_text_input.get()
        }

        fn selected_text_range(
            &mut self,
            _: bool,
            _: &mut Window,
            _: &mut App,
        ) -> Option<UTF16Selection> {
            None
        }

        fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
            None
        }

        fn text_for_range(
            &mut self,
            _: Range<usize>,
            _: &mut Option<Range<usize>>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<String> {
            None
        }

        fn replace_text_in_range(
            &mut self,
            replacement_range: Option<Range<usize>>,
            text: &str,
            _: &mut Window,
            _: &mut App,
        ) {
            if replacement_range.is_some() {
                unimplemented!()
            }
            self.text.borrow_mut().push_str(text)
        }

        fn replace_and_mark_text_in_range(
            &mut self,
            replacement_range: Option<Range<usize>>,
            new_text: &str,
            _: Option<Range<usize>>,
            _: &mut Window,
            _: &mut App,
        ) {
            if replacement_range.is_some() {
                unimplemented!()
            }
            self.text.borrow_mut().push_str(new_text)
        }

        fn unmark_text(&mut self, _: &mut Window, _: &mut App) {}

        fn prefers_ime_for_printable_keys(&mut self, _: &mut Window, _: &mut App) -> bool {
            true
        }

        fn bounds_for_range(
            &mut self,
            _: Range<usize>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<Bounds<Pixels>> {
            None
        }

        fn character_index_for_point(
            &mut self,
            _: Point<Pixels>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<usize> {
            None
        }
    }

    impl Render for PendingTextInputTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.clone()
        }
    }

    struct PendingInputTimeoutPauseOwner;

    impl Render for PendingInputTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use crate::{InteractiveElement as _, Styled as _};
            let action_count = self.action_count.clone();
            let secondary_action_count = self.secondary_action_count.clone();
            crate::div()
                .key_context("Terminal")
                .track_focus(&self.focus_handle)
                .size_full()
                .on_action(move |_: &TestAction, _, _| {
                    action_count.set(action_count.get() + 1);
                })
                .on_action(move |_: &SecondaryTestAction, _, _| {
                    secondary_action_count.set(secondary_action_count.get() + 1);
                })
        }
    }

    fn setup_pending_input_test(
        cx: &mut TestAppContext,
        bindings: impl IntoIterator<Item = KeyBinding>,
    ) -> (&mut VisualTestContext, Rc<Cell<usize>>, Rc<Cell<usize>>) {
        cx.update(|cx| cx.bind_keys(bindings));

        let action_count = Rc::new(Cell::new(0));
        let secondary_action_count = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, cx| PendingInputTestView {
            focus_handle: cx.focus_handle(),
            action_count: action_count.clone(),
            secondary_action_count: secondary_action_count.clone(),
        });
        let focus_handle = cx.update(|_, cx| view.read(cx).focus_handle.clone());
        cx.update(|window, cx| {
            window.focus(&focus_handle, cx);
            window.activate_window();
        });

        (cx, action_count, secondary_action_count)
    }

    #[derive(Debug, PartialEq)]
    struct ObservedKeystroke {
        keystroke: Keystroke,
        action_name: Option<&'static str>,
        context_stack: Vec<KeyContext>,
    }

    fn capture_observed_keystrokes(
        cx: &mut VisualTestContext,
    ) -> (Rc<RefCell<Vec<ObservedKeystroke>>>, Subscription) {
        let observed_keystrokes = Rc::new(RefCell::new(Vec::new()));
        let subscription = cx.update(|_, cx| {
            cx.observe_keystrokes({
                let observed_keystrokes = observed_keystrokes.clone();
                move |event, _, _| {
                    observed_keystrokes.borrow_mut().push(ObservedKeystroke {
                        keystroke: event.keystroke.clone(),
                        action_name: event.action.as_ref().map(|action| action.name()),
                        context_stack: event.context_stack.clone(),
                    });
                }
            })
        });
        (observed_keystrokes, subscription)
    }

    fn simulate_modifier_tap(cx: &mut VisualTestContext, modifiers: Modifiers) {
        cx.simulate_modifiers_change(modifiers);
        cx.simulate_modifiers_change(Modifiers::none());
    }

    fn terminal_context() -> KeyContext {
        KeyContext::parse("Terminal").expect("valid key context")
    }

    fn setup_pending_input_timeout_test(
        cx: &mut TestAppContext,
    ) -> (&mut VisualTestContext, Rc<Cell<usize>>, Rc<Cell<usize>>) {
        setup_pending_input_test(
            cx,
            [
                KeyBinding::new("ctrl-b", TestAction, Some("Terminal")),
                KeyBinding::new("ctrl-b h", SecondaryTestAction, Some("Terminal")),
                KeyBinding::new("ctrl-b h j", TestAction, Some("Terminal")),
            ],
        )
    }

    fn query_prefers_ime_for_printable_keys(cx: &mut VisualTestContext) -> Option<bool> {
        let mut platform_window = cx.test_window(cx.window_handle());
        let mut input_handler = platform_window.take_input_handler()?;
        let prefers_ime = input_handler.query_prefers_ime_for_printable_keys();
        platform_window.set_input_handler(input_handler);
        Some(prefers_ime)
    }

    fn setup_altgr_test(
        cx: &mut TestAppContext,
    ) -> (Entity<PendingTextInputTestView>, &mut VisualTestContext) {
        let (test, cx) = cx.add_window_view(|_, cx| PendingTextInputTestView::new(cx));
        test.update_in(cx, |test, window, cx| {
            window.focus(&test.focus_handle, cx);
            window.activate_window();
            cx.bind_keys([KeyBinding::new("ctrl-alt-a", TestAction, Some("Terminal"))]);
        });
        (test, cx)
    }

    fn dispatch_altgr_key_down(
        cx: &mut VisualTestContext,
        prefer_character_input: bool,
    ) -> DispatchEventResult {
        cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke {
                        key_char: Some("ą".into()),
                        ..Keystroke::parse("ctrl-alt-a").expect("valid keystroke")
                    },
                    is_held: false,
                    prefer_character_input,
                }),
                cx,
            )
        })
    }

    fn simulate_pending_binding(cx: &mut VisualTestContext) {
        cx.simulate_modifiers_change(crate::Modifiers::control());
        cx.simulate_keystrokes("ctrl-b");
        cx.simulate_modifiers_change(crate::Modifiers::default());
    }

    #[test]
    fn test_keybinding_for_action_bounds() {
        let tree = test_dispatch_tree(vec![KeyBinding::new(
            "cmd-n",
            TestAction,
            Some("ProjectPanel"),
        )]);

        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("ProjectPanel").unwrap(),
        ];

        let keybinding = tree.bindings_for_action(&TestAction, &contexts);

        assert!(keybinding[0].action.partial_eq(&TestAction))
    }

    #[test]
    fn test_bindings_for_action_hides_targeted_unbind_in_active_context() {
        let tree = test_dispatch_tree(vec![
            KeyBinding::new("tab", TestAction, Some("Editor")),
            KeyBinding::new(
                "tab",
                Unbind("dispatch_test::TestAction".into()),
                Some("Editor && edit_prediction"),
            ),
            KeyBinding::new(
                "tab",
                SecondaryTestAction,
                Some("Editor && showing_completions"),
            ),
        ]);

        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Editor showing_completions edit_prediction").unwrap(),
        ];

        let bindings = tree.bindings_for_action(&TestAction, &contexts);
        assert!(bindings.is_empty());

        let highest = tree.highest_precedence_binding_for_action(&TestAction, &contexts);
        assert!(highest.is_none());

        let fallback_bindings = tree.bindings_for_action(&SecondaryTestAction, &contexts);
        assert_eq!(fallback_bindings.len(), 1);
        assert!(fallback_bindings[0].action.partial_eq(&SecondaryTestAction));
    }

    #[test]
    fn test_bindings_for_action_keeps_targeted_binding_outside_unbind_context() {
        let tree = test_dispatch_tree(vec![
            KeyBinding::new("tab", TestAction, Some("Editor")),
            KeyBinding::new(
                "tab",
                Unbind("dispatch_test::TestAction".into()),
                Some("Editor && edit_prediction"),
            ),
            KeyBinding::new(
                "tab",
                SecondaryTestAction,
                Some("Editor && showing_completions"),
            ),
        ]);

        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Editor").unwrap(),
        ];

        let bindings = tree.bindings_for_action(&TestAction, &contexts);
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].action.partial_eq(&TestAction));

        let highest = tree.highest_precedence_binding_for_action(&TestAction, &contexts);
        assert!(highest.is_some_and(|binding| binding.action.partial_eq(&TestAction)));
    }

    /// Models the picker preview footer scenario: a picker action is bound in
    /// `Picker > Editor`, but a base keymap binds the same chord to an editor
    /// action in `Editor`. `Picker > Editor` and `Editor` resolve at the same
    /// context depth, so at equal depth precedence is decided purely by load
    /// order (later wins). Because base keymaps load after the default keymap,
    /// the picker binding is shadowed unless it is (re)bound by an overlay that
    /// loads after the base keymap - which is exactly what
    /// `keymaps/specific-overrides*.json` does.
    #[test]
    fn test_overlay_after_base_restores_shadowed_picker_binding() {
        // SecondaryTestAction stands in for the editor/base action (e.g.
        // editor::AddSelectionBelow), TestAction for the picker action.
        let contexts = vec![
            KeyContext::parse("Picker").unwrap(),
            KeyContext::parse("Editor").unwrap(),
        ];

        // Default keymap (picker binding) followed by a base keymap that binds
        // the same chord to an editor action: the base binding wins and the
        // picker action is shadowed, so its footer tooltip renders no shortcut.
        let shadowed = test_dispatch_tree(vec![
            KeyBinding::new("ctrl-alt-down", TestAction, Some("Picker > Editor")),
            KeyBinding::new("ctrl-alt-down", SecondaryTestAction, Some("Editor")),
        ]);
        let highest = shadowed.highest_precedence_binding_for_action(&TestAction, &contexts);
        assert!(
            highest.is_none(),
            "picker binding should be shadowed by the base editor binding"
        );

        // Re-binding the picker action in an overlay loaded after the base keymap
        // restores it as the resolved binding.
        let fixed = test_dispatch_tree(vec![
            KeyBinding::new("ctrl-alt-down", TestAction, Some("Picker > Editor")),
            KeyBinding::new("ctrl-alt-down", SecondaryTestAction, Some("Editor")),
            // overlay loaded last:
            KeyBinding::new("ctrl-alt-down", TestAction, Some("Picker > Editor")),
        ]);
        let highest = fixed.highest_precedence_binding_for_action(&TestAction, &contexts);
        assert!(
            highest.is_some_and(|binding| binding.action.partial_eq(&TestAction)),
            "overlay loaded after base should restore the picker binding"
        );

        // Conversely, putting the override in the default keymap (i.e. before the
        // base keymap) does NOT help: the later base binding still wins at equal
        // depth. This is why the overlay must be loaded after the base keymap.
        let override_before_base = test_dispatch_tree(vec![
            KeyBinding::new("ctrl-alt-down", TestAction, Some("Picker > Editor")),
            KeyBinding::new("ctrl-alt-down", TestAction, Some("Picker > Editor")),
            KeyBinding::new("ctrl-alt-down", SecondaryTestAction, Some("Editor")),
        ]);
        let highest =
            override_before_base.highest_precedence_binding_for_action(&TestAction, &contexts);
        assert!(
            highest.is_none(),
            "an override loaded before the base binding cannot win the equal-depth tie"
        );
    }

    #[test]
    fn test_pending_has_binding_state() {
        let bindings = vec![
            KeyBinding::new("ctrl-b h", TestAction, None),
            KeyBinding::new("space", TestAction, Some("ContextA")),
            KeyBinding::new("space f g", TestAction, Some("ContextB")),
        ];
        let mut tree = test_dispatch_tree(bindings);

        type DispatchPath = SmallVec<[super::DispatchNodeId; 32]>;
        fn dispatch(
            tree: &mut DispatchTree,
            pending: SmallVec<[Keystroke; 1]>,
            key: &str,
            path: &DispatchPath,
        ) -> DispatchResult {
            tree.dispatch_key(pending, Keystroke::parse(key).unwrap(), path)
        }

        let dispatch_path: DispatchPath = SmallVec::new();
        let result = dispatch(&mut tree, SmallVec::new(), "ctrl-b", &dispatch_path);
        assert_eq!(result.pending.len(), 1);
        assert!(!result.pending_has_binding);

        let result = dispatch(&mut tree, result.pending, "h", &dispatch_path);
        assert_eq!(result.pending.len(), 0);
        assert_eq!(result.bindings.len(), 1);
        assert!(!result.pending_has_binding);

        let node_id = tree.push_node();
        tree.set_key_context(KeyContext::parse("ContextB").unwrap());
        tree.pop_node();

        let dispatch_path = tree.dispatch_path(node_id);
        let result = dispatch(&mut tree, SmallVec::new(), "space", &dispatch_path);

        assert_eq!(result.pending.len(), 1);
        assert!(!result.pending_has_binding);
    }

    #[crate::test]
    fn test_pending_input_observers_notified_on_focus_change_and_blur(cx: &mut TestAppContext) {
        #[derive(Clone)]
        struct CustomElement {
            focus_handle: FocusHandle,
            text: Rc<RefCell<String>>,
        }

        impl CustomElement {
            fn new(cx: &mut Context<Self>) -> Self {
                Self {
                    focus_handle: cx.focus_handle(),
                    text: Rc::default(),
                }
            }
        }

        impl Element for CustomElement {
            type RequestLayoutState = ();

            type PrepaintState = ();

            fn id(&self) -> Option<ElementId> {
                Some("custom".into())
            }

            fn source_location(&self) -> Option<&'static panic::Location<'static>> {
                None
            }

            fn request_layout(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                window: &mut Window,
                cx: &mut App,
            ) -> (LayoutId, Self::RequestLayoutState) {
                (window.request_layout(Style::default(), [], cx), ())
            }

            fn prepaint(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                _: Bounds<Pixels>,
                _: &mut Self::RequestLayoutState,
                window: &mut Window,
                cx: &mut App,
            ) -> Self::PrepaintState {
                window.set_focus_handle(&self.focus_handle, cx);
            }

            fn paint(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                _: Bounds<Pixels>,
                _: &mut Self::RequestLayoutState,
                _: &mut Self::PrepaintState,
                window: &mut Window,
                cx: &mut App,
            ) {
                let mut key_context = KeyContext::default();
                key_context.add("Terminal");
                window.set_key_context(key_context);
                window.handle_input(&self.focus_handle, self.clone(), cx);
                window.on_action(std::any::TypeId::of::<TestAction>(), |_, _, _, _| {});
            }
        }

        impl IntoElement for CustomElement {
            type Element = Self;

            fn into_element(self) -> Self::Element {
                self
            }
        }

        impl InputHandler for CustomElement {
            fn selected_text_range(
                &mut self,
                _: bool,
                _: &mut Window,
                _: &mut App,
            ) -> Option<UTF16Selection> {
                None
            }

            fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
                None
            }

            fn text_for_range(
                &mut self,
                _: Range<usize>,
                _: &mut Option<Range<usize>>,
                _: &mut Window,
                _: &mut App,
            ) -> Option<String> {
                None
            }

            fn replace_text_in_range(
                &mut self,
                replacement_range: Option<Range<usize>>,
                text: &str,
                _: &mut Window,
                _: &mut App,
            ) {
                if replacement_range.is_some() {
                    unimplemented!()
                }
                self.text.borrow_mut().push_str(text)
            }

            fn replace_and_mark_text_in_range(
                &mut self,
                replacement_range: Option<Range<usize>>,
                new_text: &str,
                _: Option<Range<usize>>,
                _: &mut Window,
                _: &mut App,
            ) {
                if replacement_range.is_some() {
                    unimplemented!()
                }
                self.text.borrow_mut().push_str(new_text)
            }

            fn unmark_text(&mut self, _: &mut Window, _: &mut App) {}

            fn bounds_for_range(
                &mut self,
                _: Range<usize>,
                _: &mut Window,
                _: &mut App,
            ) -> Option<Bounds<Pixels>> {
                None
            }

            fn character_index_for_point(
                &mut self,
                _: Point<Pixels>,
                _: &mut Window,
                _: &mut App,
            ) -> Option<usize> {
                None
            }
        }

        impl Render for CustomElement {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.clone()
            }
        }

        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("ctrl-b", TestAction, Some("Terminal"))]);
            cx.bind_keys([KeyBinding::new("ctrl-b h", TestAction, Some("Terminal"))]);
            cx.bind_keys([KeyBinding::new("ctrl-d", TestAction, None)]);
            cx.bind_keys([KeyBinding::new("ctrl-d h", TestAction, None)]);
        });

        let (test, cx) = cx.add_window_view(|_, cx| CustomElement::new(cx));
        let focus_handle = test.update(cx, |test, _| test.focus_handle.clone());

        let pending_input_changed_count = Rc::new(RefCell::new(0usize));
        let pending_input_changed_count_for_observer = pending_input_changed_count.clone();

        struct PendingInputObserver {
            _subscription: Subscription,
        }

        let _observer = cx.update(|window, cx| {
            cx.new(|cx| PendingInputObserver {
                _subscription: cx.observe_pending_input(window, move |_, _, _| {
                    *pending_input_changed_count_for_observer.borrow_mut() += 1;
                }),
            })
        });

        cx.update(|window, cx| {
            window.focus(&focus_handle, cx);
            window.activate_window();
        });

        cx.simulate_keystrokes("ctrl-b");

        let count_after_pending = Rc::new(RefCell::new(0usize));
        let count_after_pending_for_assertion = count_after_pending.clone();

        cx.update(|window, cx| {
            assert!(window.has_pending_keystrokes());
            *count_after_pending.borrow_mut() = *pending_input_changed_count.borrow();
            assert!(*count_after_pending.borrow() > 0);

            window.focus(&cx.focus_handle(), cx);

            assert!(!window.has_pending_keystrokes());
        });

        // Focus-triggered pending-input notifications are deferred to the end of the current
        // effect cycle, so the observer callback should run after the focus update completes.
        cx.update(|_, _| {
            let count_after_focus_change = *pending_input_changed_count.borrow();
            assert!(count_after_focus_change > *count_after_pending_for_assertion.borrow());
        });

        cx.update(|window, cx| window.focus(&focus_handle, cx));
        cx.simulate_keystrokes("ctrl-b");
        let count_before_blur = *pending_input_changed_count.borrow();

        cx.update(|window, cx| {
            assert!(window.has_pending_keystrokes());
            window.blur(cx);
            assert!(!window.has_pending_keystrokes());
            assert!(window.pending_input_is_none());
        });

        cx.update(|_, _| {
            assert!(*pending_input_changed_count.borrow() > count_before_blur);
        });

        cx.update(|window, cx| window.disable_focus(cx));
        cx.simulate_keystrokes("ctrl-d");

        cx.update(|window, cx| {
            assert!(window.has_pending_keystrokes());
            window.blur(cx);
            assert!(window.pending_input_is_none());
        });
    }

    #[crate::test]
    fn test_standalone_modifier_dispatch(cx: &mut TestAppContext) {
        let (cx, action_count, _) =
            setup_pending_input_test(cx, [KeyBinding::new("shift", TestAction, Some("Terminal"))]);
        let intercepted_keystrokes = Rc::new(RefCell::new(Vec::new()));
        let should_consume = Rc::new(Cell::new(true));
        let terminal_context = terminal_context();
        let (observed_keystrokes, _observer) = capture_observed_keystrokes(cx);
        let _interceptor = cx.update(|_, cx| {
            cx.intercept_keystrokes({
                let intercepted_keystrokes = intercepted_keystrokes.clone();
                let should_consume = should_consume.clone();
                let terminal_context = terminal_context.clone();
                move |event, _, cx| {
                    assert!(event.action.is_none());
                    assert_eq!(event.context_stack, vec![terminal_context.clone()]);
                    intercepted_keystrokes
                        .borrow_mut()
                        .push(event.keystroke.clone());
                    if should_consume.get() {
                        cx.stop_propagation();
                    }
                }
            })
        });
        let shift = Keystroke::parse("shift").expect("valid keystroke");

        // Pressing Shift is not a keystroke. Consuming its recognized release suppresses
        // the binding.
        cx.simulate_modifiers_change(Modifiers::shift());
        assert!(intercepted_keystrokes.borrow().is_empty());
        cx.simulate_modifiers_change(Modifiers::none());
        assert_eq!(
            intercepted_keystrokes.borrow().as_slice(),
            std::slice::from_ref(&shift)
        );
        assert_eq!(action_count.get(), 0);
        assert!(observed_keystrokes.borrow().is_empty());

        // Without consumption, the same recognized release reaches keymap dispatch.
        should_consume.set(false);
        intercepted_keystrokes.borrow_mut().clear();
        cx.simulate_modifiers_change(Modifiers::shift());
        assert!(intercepted_keystrokes.borrow().is_empty());
        cx.simulate_modifiers_change(Modifiers::none());
        assert_eq!(
            intercepted_keystrokes.borrow().as_slice(),
            std::slice::from_ref(&shift)
        );
        assert_eq!(action_count.get(), 1);
        assert_eq!(
            observed_keystrokes.borrow().as_slice(),
            &[ObservedKeystroke {
                keystroke: shift,
                action_name: Some(TestAction.name()),
                context_stack: vec![terminal_context],
            }]
        );
    }

    #[crate::test]
    fn test_unbound_standalone_modifier_observation(cx: &mut TestAppContext) {
        let (cx, _, _) = setup_pending_input_test(cx, []);
        let terminal_context = terminal_context();
        let (observed_keystrokes, _observer) = capture_observed_keystrokes(cx);

        for (modifiers, key) in [
            (Modifiers::shift(), "shift"),
            (Modifiers::control(), "ctrl"),
            (Modifiers::alt(), "alt"),
            (Modifiers::command(), "cmd"),
            (Modifiers::function(), "fn"),
        ] {
            observed_keystrokes.borrow_mut().clear();
            cx.simulate_modifiers_change(modifiers);
            assert!(
                observed_keystrokes.borrow().is_empty(),
                "modifier press must not notify observers"
            );
            cx.simulate_modifiers_change(Modifiers::none());
            assert_eq!(
                observed_keystrokes.borrow().as_slice(),
                &[ObservedKeystroke {
                    keystroke: Keystroke::parse(key).expect("valid modifier"),
                    action_name: None,
                    context_stack: vec![terminal_context.clone()],
                }]
            );
        }
    }

    #[crate::test]
    fn test_raw_modifier_handler_can_suppress_observation(cx: &mut TestAppContext) {
        struct ModifierListenerTestView {
            focus_handle: FocusHandle,
            consume: Rc<Cell<bool>>,
            releases: Rc<Cell<usize>>,
        }

        impl Render for ModifierListenerTestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                use crate::{InteractiveElement as _, Styled as _};
                let consume = self.consume.clone();
                let releases = self.releases.clone();
                crate::div()
                    .key_context("Terminal")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .on_action(|_: &TestAction, _, cx| cx.propagate())
                    .on_modifiers_changed(move |event, _, cx| {
                        if !event.modifiers.modified() {
                            releases.set(releases.get() + 1);
                            if consume.get() {
                                cx.stop_propagation();
                            }
                        }
                    })
            }
        }

        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("shift", TestAction, Some("Terminal"))]);
        });
        let consume = Rc::new(Cell::new(true));
        let releases = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, cx| ModifierListenerTestView {
            focus_handle: cx.focus_handle(),
            consume: consume.clone(),
            releases: releases.clone(),
        });
        cx.update(|window, cx| {
            let focus_handle = view.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
            window.activate_window();
        });
        let (observed_keystrokes, _observer) = capture_observed_keystrokes(cx);

        simulate_modifier_tap(cx, Modifiers::shift());
        assert_eq!(releases.get(), 1);
        assert!(observed_keystrokes.borrow().is_empty());

        consume.set(false);
        simulate_modifier_tap(cx, Modifiers::shift());
        assert_eq!(releases.get(), 2);
        assert_eq!(
            observed_keystrokes.borrow().as_slice(),
            &[ObservedKeystroke {
                keystroke: Keystroke::parse("shift").expect("valid modifier"),
                action_name: None,
                context_stack: vec![terminal_context()],
            }]
        );
    }

    #[crate::test]
    fn test_pending_modifier_observed_when_binding_resolves(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_test(
            cx,
            [
                KeyBinding::new("shift", TestAction, Some("Terminal")),
                KeyBinding::new("shift f1", SecondaryTestAction, Some("Terminal")),
            ],
        );
        let (observed_keystrokes, _observer) = capture_observed_keystrokes(cx);

        simulate_modifier_tap(cx, Modifiers::shift());
        assert!(observed_keystrokes.borrow().is_empty());
        assert_eq!(action_count.get(), 0);

        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();
        assert_eq!(action_count.get(), 1);
        assert_eq!(
            observed_keystrokes.borrow().as_slice(),
            &[ObservedKeystroke {
                keystroke: Keystroke::parse("shift").expect("valid modifier"),
                action_name: Some(TestAction.name()),
                context_stack: Vec::new(),
            }]
        );

        observed_keystrokes.borrow_mut().clear();
        simulate_modifier_tap(cx, Modifiers::shift());
        assert!(observed_keystrokes.borrow().is_empty());
        cx.simulate_keystrokes("f1");
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 1);
        assert_eq!(
            observed_keystrokes.borrow().as_slice(),
            &[ObservedKeystroke {
                keystroke: Keystroke::parse("f1").expect("valid key"),
                action_name: Some(SecondaryTestAction.name()),
                context_stack: vec![terminal_context()],
            }]
        );
    }

    #[crate::test]
    fn test_combined_shortcut_has_no_standalone_modifier(cx: &mut TestAppContext) {
        let (cx, action_count, _) = setup_pending_input_test(
            cx,
            [
                KeyBinding::new("shift", TestAction, Some("Terminal")),
                KeyBinding::new("shift-f1", TestAction, Some("Terminal")),
            ],
        );
        let intercepted_keystrokes = Rc::new(RefCell::new(Vec::new()));
        let (observed_keystrokes, _observer) = capture_observed_keystrokes(cx);
        let _interceptor = cx.update(|_, cx| {
            cx.intercept_keystrokes({
                let intercepted_keystrokes = intercepted_keystrokes.clone();
                move |event, _, _| {
                    intercepted_keystrokes
                        .borrow_mut()
                        .push(event.keystroke.clone());
                }
            })
        });

        // Shift-F1 is the only keystroke. Releasing Shift must not emit a second one.
        cx.simulate_modifiers_change(Modifiers::shift());
        assert!(intercepted_keystrokes.borrow().is_empty());
        cx.simulate_keystrokes("shift-f1");
        cx.simulate_modifiers_change(Modifiers::none());

        let shift_f1 = Keystroke::parse("shift-f1").expect("valid keystroke");
        assert_eq!(
            intercepted_keystrokes.borrow().as_slice(),
            std::slice::from_ref(&shift_f1)
        );
        assert_eq!(
            observed_keystrokes.borrow().as_slice(),
            &[ObservedKeystroke {
                keystroke: shift_f1,
                action_name: Some(TestAction.name()),
                context_stack: vec![terminal_context()],
            }]
        );
        assert_eq!(action_count.get(), 1);
    }

    #[crate::test]
    fn test_keystroke_input_preference(cx: &mut TestAppContext) {
        let (test, cx) = setup_altgr_test(cx);
        let intercepted_preferences = Rc::new(RefCell::new(Vec::new()));
        let observed_preferences = Rc::new(RefCell::new(Vec::new()));
        let _interceptor = cx.update(|_, cx| {
            cx.intercept_keystrokes({
                let intercepted_preferences = intercepted_preferences.clone();
                move |event, _, _| {
                    intercepted_preferences
                        .borrow_mut()
                        .push(event.input_preference);
                }
            })
        });
        let _observer = cx.update(|_, cx| {
            cx.observe_keystrokes({
                let observed_preferences = observed_preferences.clone();
                move |event, _, _| {
                    observed_preferences
                        .borrow_mut()
                        .push(event.input_preference);
                }
            })
        });

        for (prefer_character_input, accepts_text_input, input_preference) in [
            (true, true, InputPreference::CharacterInput),
            (false, true, InputPreference::KeyBindings),
            (true, false, InputPreference::KeyBindings),
            (false, false, InputPreference::KeyBindings),
        ] {
            test.update(cx, |test, _| {
                test.accepts_text_input.set(accepts_text_input);
                test.action_count.set(0);
            });
            intercepted_preferences.borrow_mut().clear();
            observed_preferences.borrow_mut().clear();
            let result = dispatch_altgr_key_down(cx, prefer_character_input);
            assert_eq!(
                result.propagate,
                input_preference == InputPreference::CharacterInput
            );
            assert_eq!(
                intercepted_preferences.borrow().as_slice(),
                &[input_preference]
            );
            assert_eq!(
                observed_preferences.borrow().as_slice(),
                &[input_preference]
            );
            test.read_with(cx, |test, _| {
                assert_eq!(
                    test.action_count.get(),
                    usize::from(input_preference == InputPreference::KeyBindings)
                );
            });
        }
    }

    #[crate::test]
    fn test_input_preference_rechecked_after_interception(cx: &mut TestAppContext) {
        let (test, cx) = setup_altgr_test(cx);
        let accepts_text_input = test.read_with(cx, |test, _| test.accepts_text_input.clone());
        let _interceptor = cx.update(|_, cx| {
            cx.intercept_keystrokes({
                let accepts_text_input = accepts_text_input.clone();
                move |event, _, _| {
                    assert_eq!(
                        event.input_preference == InputPreference::CharacterInput,
                        accepts_text_input.get()
                    );
                    accepts_text_input.set(!accepts_text_input.get());
                }
            })
        });
        let observed_preferences = Rc::new(RefCell::new(Vec::new()));
        let _observer = cx.update(|_, cx| {
            cx.observe_keystrokes({
                let observed_preferences = observed_preferences.clone();
                move |event, _, _| {
                    observed_preferences
                        .borrow_mut()
                        .push(event.input_preference);
                }
            })
        });

        for (initially_accepts_text_input, observed_preference) in [
            (true, InputPreference::KeyBindings),
            (false, InputPreference::CharacterInput),
        ] {
            accepts_text_input.set(initially_accepts_text_input);
            test.update(cx, |test, _| test.action_count.set(0));
            observed_preferences.borrow_mut().clear();
            let result = dispatch_altgr_key_down(cx, true);
            assert_eq!(result.propagate, !initially_accepts_text_input);
            assert_eq!(
                observed_preferences.borrow().as_slice(),
                &[observed_preference]
            );
            test.read_with(cx, |test, _| {
                assert_eq!(
                    test.action_count.get(),
                    usize::from(initially_accepts_text_input)
                );
            });
        }
    }

    #[crate::test]
    fn test_keystroke_interceptors_can_consume_preferred_character_input(cx: &mut TestAppContext) {
        let (test, cx) = setup_altgr_test(cx);
        let intercepted = Rc::new(Cell::new(false));
        let _interceptor = cx.update(|_, cx| {
            cx.intercept_keystrokes({
                let intercepted = intercepted.clone();
                move |event, _, cx| {
                    assert_eq!(event.input_preference, InputPreference::CharacterInput);
                    intercepted.set(true);
                    cx.stop_propagation();
                }
            })
        });

        let result = dispatch_altgr_key_down(cx, true);
        assert!(intercepted.get());
        assert!(!result.propagate);
        test.read_with(cx, |test, _| assert_eq!(test.action_count.get(), 0));
    }

    #[crate::test]
    fn test_consumed_modifier_preserves_pending_input(cx: &mut TestAppContext) {
        let (test, cx) = cx.add_window_view(|_, cx| PendingTextInputTestView::new(cx));
        test.update_in(cx, |test, window, cx| {
            window.focus(&test.focus_handle, cx);
            window.activate_window();
        });
        let _subscription = cx.update(|_, cx| {
            cx.intercept_keystrokes(|event, _, cx| {
                if event.keystroke.key == "shift" {
                    cx.stop_propagation();
                }
            })
        });

        // Exercise both paths that processing Shift would take: completing `j shift`, and
        // replaying `j` when `j k` no longer matches.
        for binding in ["j shift", "j k"] {
            cx.update(|_, cx| {
                cx.clear_key_bindings();
                cx.bind_keys([KeyBinding::new(binding, TestAction, Some("Terminal"))]);
            });
            test.update(cx, |test, _| test.text.borrow_mut().clear());
            cx.simulate_keystrokes("j");
            cx.executor()
                .advance_clock(crate::PENDING_INPUT_TIMEOUT / 2);
            cx.run_until_parked();
            let pending_keystrokes = cx.update(|window, _| {
                window
                    .pending_input()
                    .expect("pending input")
                    .keystrokes()
                    .to_vec()
            });

            // Consuming Shift must not complete or replay the pending `j`.
            cx.simulate_modifiers_change(Modifiers::shift());
            cx.simulate_modifiers_change(Modifiers::none());
            cx.update(|window, _| {
                let pending_input = window.pending_input().expect("pending input");
                assert_eq!(pending_input.keystrokes(), pending_keystrokes);
                assert!(pending_input.timeout().is_some());
            });
            test.update(cx, |test, _| {
                assert_eq!(test.action_count.get(), 0);
                assert_eq!(test.text.borrow().as_str(), "");
            });

            // The unchanged timeout should replay `j` after its remaining half.
            cx.executor()
                .advance_clock(crate::PENDING_INPUT_TIMEOUT / 2);
            cx.run_until_parked();
            cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
            test.update(cx, |test, _| {
                assert_eq!(test.action_count.get(), 0);
                assert_eq!(test.text.borrow().as_str(), "j");
            });
        }
    }

    #[crate::test]
    fn test_printable_pending_input_replays_on_timeout(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("j k", TestAction, Some("Terminal"))]);
        });
        let (test, cx) = cx.add_window_view(|_, cx| PendingTextInputTestView::new(cx));
        let focus_handle = test.update(cx, |test, _| test.focus_handle.clone());
        cx.update(|window, cx| {
            window.focus(&focus_handle, cx);
            window.activate_window();
        });

        cx.simulate_keystrokes("j");

        cx.update(|window, _| {
            let pending_input = window.pending_input().expect("pending input");
            assert_eq!(pending_input.keystrokes().len(), 1);
            assert!(pending_input.timeout().is_some());
        });
        test.update(cx, |test, _| {
            assert_eq!(test.action_count.get(), 0);
            assert_eq!(test.text.borrow().as_str(), "");
        });

        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        test.update(cx, |test, _| {
            assert_eq!(test.action_count.get(), 0);
            assert_eq!(test.text.borrow().as_str(), "j");
        });
    }

    #[crate::test]
    fn test_pending_input_timeout_dispatches_shorter_binding(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);
        cx.update(|window, _| {
            assert_eq!(
                window
                    .pending_input()
                    .map(|pending_input| pending_input.keystrokes().len()),
                Some(1)
            );
            assert_eq!(
                window
                    .pending_input()
                    .and_then(|pending_input| pending_input.timeout())
                    .map(|timeout| timeout.duration()),
                Some(crate::PENDING_INPUT_TIMEOUT)
            );
        });
        assert_eq!(action_count.get(), 0);

        // Emulate a countdown indicator re-rendering the window while waiting for the timeout.
        for _ in 0..10 {
            cx.executor()
                .advance_clock(crate::PENDING_INPUT_TIMEOUT / 10);
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 0);
    }

    #[crate::test]
    fn test_running_pending_input_timeout_resets_when_binding_advances(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 4 / 5);
        cx.run_until_parked();
        cx.simulate_keystrokes("h");
        cx.run_until_parked();

        cx.update(|window, cx| {
            let pending_input = window.pending_input().expect("pending input");
            let timeout = pending_input.timeout().expect("pending input timeout");
            assert_eq!(pending_input.keystrokes().len(), 2);
            assert!(!timeout.is_paused());
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT);
        });

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT / 5);
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 0);

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 4 / 5);
        cx.run_until_parked();
        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 1);
    }

    #[crate::test]
    fn test_pending_input_timeout_starts_when_binding_becomes_ambiguous(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_test(
            cx,
            [
                KeyBinding::new("ctrl-b h", SecondaryTestAction, Some("Terminal")),
                KeyBinding::new("ctrl-b h j", TestAction, Some("Terminal")),
            ],
        );
        simulate_pending_binding(cx);

        cx.update(|window, _| {
            let pending_input = window.pending_input().expect("pending input");
            assert_eq!(pending_input.keystrokes().len(), 1);
            assert!(pending_input.timeout().is_none());
        });

        cx.simulate_keystrokes("h");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let pending_input = window.pending_input().expect("pending input");
            let timeout = pending_input.timeout().expect("pending input timeout");
            assert_eq!(pending_input.keystrokes().len(), 2);
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT);
        });

        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();
        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 1);
    }

    #[crate::test]
    fn test_invalid_continuation_while_timeout_paused_replays_pending_input(
        cx: &mut TestAppContext,
    ) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_test(
            cx,
            [
                KeyBinding::new("ctrl-b", TestAction, Some("Terminal")),
                KeyBinding::new("ctrl-b h", SecondaryTestAction, Some("Terminal")),
                KeyBinding::new("x", SecondaryTestAction, Some("Terminal")),
            ],
        );
        simulate_pending_binding(cx);
        let pause_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, true, cx));
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("x");
        cx.run_until_parked();

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 1);

        drop(pause_owner);
        cx.update(|_, _| {});
        cx.run_until_parked();
        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();

        cx.update(|window, _| assert!(window.pending_input_is_none()));
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 1);
    }

    #[crate::test]
    fn test_pending_input_timeout_pauses_and_resumes(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 7 / 10);
        cx.run_until_parked();

        let pause_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        let other_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, true, cx));
            assert!(!window.set_pending_input_timeout_paused(&pause_owner, true, cx));
            assert!(!window.set_pending_input_timeout_paused(&other_owner, false, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let timeout = window
                .pending_input()
                .and_then(|pending_input| pending_input.timeout())
                .expect("pending input timeout");
            assert!(timeout.is_paused());
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT * 3 / 10);
        });

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 2);
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);

        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, false, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let timeout = window
                .pending_input()
                .and_then(|pending_input| pending_input.timeout())
                .expect("pending input timeout");
            assert!(!timeout.is_paused());
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT * 3 / 10);
        });

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 3 / 10);
        cx.run_until_parked();

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 0);
    }

    #[crate::test]
    fn test_pending_input_timeout_resumes_when_owner_is_released(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 7 / 10);
        cx.run_until_parked();

        let pause_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, true, cx));
        });
        cx.run_until_parked();

        drop(pause_owner);
        cx.update(|_, _| {});
        cx.run_until_parked();
        cx.update(|window, cx| {
            let timeout = window
                .pending_input()
                .and_then(|pending_input| pending_input.timeout())
                .expect("pending input timeout");
            assert!(!timeout.is_paused());
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT * 3 / 10);
        });

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 3 / 10);
        cx.run_until_parked();

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 1);
        assert_eq!(secondary_action_count.get(), 0);
    }

    #[crate::test]
    fn test_pending_input_timeout_resets_when_binding_advances(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);
        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT / 2);
        cx.run_until_parked();
        let pause_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, true, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let timeout = window
                .pending_input()
                .and_then(|pending_input| pending_input.timeout())
                .expect("pending input timeout");
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT / 2);
        });

        cx.simulate_keystrokes("h");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let pending_input = window.pending_input().expect("pending input");
            let timeout = pending_input.timeout().expect("pending input timeout");
            assert_eq!(pending_input.keystrokes().len(), 2);
            assert!(timeout.is_paused());
            assert_eq!(timeout.remaining(cx), crate::PENDING_INPUT_TIMEOUT);
        });

        cx.executor()
            .advance_clock(crate::PENDING_INPUT_TIMEOUT * 2);
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 0);

        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, false, cx));
        });
        cx.run_until_parked();
        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();

        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 1);
    }

    #[crate::test]
    fn test_clearing_pending_input_invalidates_timeout_pause(cx: &mut TestAppContext) {
        let (cx, action_count, secondary_action_count) = setup_pending_input_timeout_test(cx);
        simulate_pending_binding(cx);
        let pause_owner = cx.update(|_, cx| cx.new(|_| PendingInputTimeoutPauseOwner));
        cx.update(|window, cx| {
            assert!(window.set_pending_input_timeout_paused(&pause_owner, true, cx));
            window.focus(&cx.focus_handle(), cx);
            assert!(!window.has_pending_keystrokes());
        });

        drop(pause_owner);
        cx.update(|_, _| {});
        cx.run_until_parked();
        cx.executor().advance_clock(crate::PENDING_INPUT_TIMEOUT);
        cx.run_until_parked();

        cx.update(|window, _| assert!(window.pending_input_is_none()));
        assert_eq!(action_count.get(), 0);
        assert_eq!(secondary_action_count.get(), 0);
    }

    #[crate::test]
    fn test_input_handler_pending(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("ctrl-b", TestAction, Some("Terminal"))]);
            cx.bind_keys([KeyBinding::new("ctrl-b h", TestAction, Some("Terminal"))]);
            cx.bind_keys([KeyBinding::new("ctrl-x k", TestAction, Some("Terminal"))]);
        });
        let (test, cx) = cx.add_window_view(|_, cx| PendingTextInputTestView::new(cx));
        let focus_handle = test.update(cx, |test, _| test.focus_handle.clone());
        cx.update(|window, cx| {
            window.focus(&focus_handle, cx);
            window.activate_window();
        });

        assert_eq!(query_prefers_ime_for_printable_keys(cx), Some(true));
        cx.simulate_keystrokes("ctrl-x");
        cx.update(|window, _| assert!(window.has_pending_keystrokes()));
        assert_eq!(query_prefers_ime_for_printable_keys(cx), Some(false));

        let prefers_ime_after_blur = {
            let mut platform_window = cx.test_window(cx.window_handle());
            let mut input_handler = platform_window.take_input_handler();
            cx.update(|window, cx| {
                window.blur(cx);
                assert!(!window.has_pending_keystrokes());
                assert!(window.pending_input_is_none());
            });
            let prefers_ime = input_handler
                .as_mut()
                .map(|input_handler| input_handler.query_prefers_ime_for_printable_keys());
            if let Some(input_handler) = input_handler {
                platform_window.set_input_handler(input_handler);
            }
            prefers_ime
        };
        assert_eq!(prefers_ime_after_blur, Some(true));
        cx.update(|window, cx| window.focus(&focus_handle, cx));

        cx.simulate_keystrokes("ctrl-x");
        assert_eq!(query_prefers_ime_for_printable_keys(cx), Some(false));
        cx.simulate_keystrokes("k");
        cx.update(|window, _| assert!(!window.has_pending_keystrokes()));
        assert_eq!(query_prefers_ime_for_printable_keys(cx), Some(true));
        test.update(cx, |test, _| {
            assert_eq!(test.action_count.get(), 1);
            assert_eq!(test.text.borrow().as_str(), "");
        });

        cx.simulate_keystrokes("ctrl-b [");
        test.update(cx, |test, _| assert_eq!(test.text.borrow().as_str(), "["))
    }

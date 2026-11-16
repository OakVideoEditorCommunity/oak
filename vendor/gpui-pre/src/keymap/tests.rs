    use super::*;
    use crate as gpui;
    use gpui::{NoAction, Unbind};

    actions!(
        test_only,
        [ActionAlpha, ActionBeta, ActionGamma, ActionDelta,]
    );

    #[test]
    fn test_keymap() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, None),
            KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
            KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor && mode==full")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings.clone());

        // global bindings are enabled in all contexts
        assert_eq!(keymap.binding_enabled(&bindings[0], &[]), Some(0));
        assert_eq!(
            keymap.binding_enabled(&bindings[0], &[KeyContext::parse("terminal").unwrap()]),
            Some(1)
        );

        // contextual bindings are enabled in contexts that match their predicate
        assert_eq!(
            keymap.binding_enabled(&bindings[1], &[KeyContext::parse("barf x=y").unwrap()]),
            None
        );
        assert_eq!(
            keymap.binding_enabled(&bindings[1], &[KeyContext::parse("pane x=y").unwrap()]),
            Some(1)
        );

        assert_eq!(
            keymap.binding_enabled(&bindings[2], &[KeyContext::parse("editor").unwrap()]),
            None
        );
        assert_eq!(
            keymap.binding_enabled(
                &bindings[2],
                &[KeyContext::parse("editor mode=full").unwrap()]
            ),
            Some(1)
        );
    }

    #[test]
    fn test_depth_precedence() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
            KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-a").unwrap()],
            &[
                KeyContext::parse("pane").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );

        assert!(!pending);
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionGamma {}));
        assert!(result[1].action.partial_eq(&ActionBeta {}));
    }

    #[test]
    fn test_keymap_disabled() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-b", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-a", NoAction {}, Some("editor && mode==full")),
            KeyBinding::new("ctrl-b", NoAction {}, None),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // binding is only enabled in a specific context
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("barf").unwrap()],
                )
                .0
                .is_empty()
        );
        assert!(
            !keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("editor").unwrap()],
                )
                .0
                .is_empty()
        );

        // binding is disabled in a more specific context
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("editor mode=full").unwrap()],
                )
                .0
                .is_empty()
        );

        // binding is globally disabled
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-b").unwrap()],
                    &[KeyContext::parse("barf").unwrap()],
                )
                .0
                .is_empty()
        );
    }

    #[test]
    /// Tests for https://github.com/zed-industries/zed/issues/30259
    fn test_multiple_keystroke_binding_disabled() {
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space = || Keystroke::parse("space").unwrap();
        let w = || Keystroke::parse("w").unwrap();

        let space_w = [space(), w()];
        let space_w_w = [space(), w(), w()];

        let workspace_context = || [KeyContext::parse("workspace").unwrap()];

        let editor_workspace_context = || {
            [
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ]
        };

        // Ensure `space` results in pending input on the workspace, but not editor
        let space_workspace = keymap.bindings_for_input(&[space()], &workspace_context());
        assert!(space_workspace.0.is_empty());
        assert!(space_workspace.1);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(!space_editor.1);

        // Ensure `space w` results in pending input on the workspace, but not editor
        let space_w_workspace = keymap.bindings_for_input(&space_w, &workspace_context());
        assert!(space_w_workspace.0.is_empty());
        assert!(space_w_workspace.1);

        let space_w_editor = keymap.bindings_for_input(&space_w, &editor_workspace_context());
        assert!(space_w_editor.0.is_empty());
        assert!(!space_w_editor.1);

        // Ensure `space w w` results in the binding in the workspace, but not in the editor
        let space_w_w_workspace = keymap.bindings_for_input(&space_w_w, &workspace_context());
        assert!(!space_w_w_workspace.0.is_empty());
        assert!(!space_w_w_workspace.1);

        let space_w_w_editor = keymap.bindings_for_input(&space_w_w, &editor_workspace_context());
        assert!(space_w_w_editor.0.is_empty());
        assert!(!space_w_w_editor.1);

        // Now test what happens if we have another binding defined AFTER the NoAction
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);

        // Now test what happens if we have another binding defined BEFORE the NoAction
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);

        // Now test what happens if we have another binding defined at a higher context
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);
    }

    #[test]
    fn test_override_multikey() {
        let bindings = [
            KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-w", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-w").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert!(result.is_empty());
        assert!(pending);

        let bindings = [
            KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-w", ActionBeta {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-w").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert_eq!(result.len(), 1);
        assert!(!pending);
    }

    #[test]
    fn test_simple_disable() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert!(result.is_empty());
        assert!(!pending);
    }

    #[test]
    fn test_disable_weaker_sources_only() {
        const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
        const VIM: KeyBindingMetaIndex = KeyBindingMetaIndex(1);
        const BASE: KeyBindingMetaIndex = KeyBindingMetaIndex(2);
        const DEFAULT: KeyBindingMetaIndex = KeyBindingMetaIndex(3);

        let editor_context = || [KeyContext::parse("editor").unwrap()];
        let ctrl_x = || [Keystroke::parse("ctrl-x").unwrap()];

        // A base keymap null disables a default binding in the same context.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert!(result.is_empty());

        // A user binding is not affected by base keymap or default nulls.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionBeta {}, None).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));

        // A user binding at a shallower context is not disabled by a deeper
        // base keymap null.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionBeta {}, Some("workspace")).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(
            &ctrl_x(),
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));

        // A vim binding survives a base keymap null, and a user null disables
        // everything.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionGamma {}, Some("editor")).with_meta(VIM),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionGamma {}));

        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", ActionGamma {}, Some("editor")).with_meta(VIM),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert!(result.is_empty());
    }

    #[test]
    fn test_fail_to_disable() {
        // disabled at the wrong level
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("workspace")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 1);
        assert!(!pending);
    }

    #[test]
    fn test_disable_deeper() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 0);
        assert!(!pending);
    }

    #[test]
    fn test_pending_match_enabled() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
            KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(matched.1);
    }

    #[test]
    fn test_pending_match_enabled_extended() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
            KeyBinding::new("ctrl-x 0", NoAction, Some("Workspace")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("Workspace")),
            KeyBinding::new("ctrl-x 0", NoAction, Some("vim_mode == normal")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
    }

    #[test]
    fn test_overriding_prefix() {
        let bindings = [
            KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
    }

    #[test]
    fn test_context_precedence_with_same_source() {
        // Test case: User has both Workspace and Editor bindings for the same key
        // Editor binding should take precedence over Workspace binding
        let bindings = [
            KeyBinding::new("cmd-r", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Test with context stack: [Workspace, Editor] (Editor is deeper)
        let (result, _) = keymap.bindings_for_input(
            &[Keystroke::parse("cmd-r").unwrap()],
            &[
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Editor").unwrap(),
            ],
        );

        // Both bindings should be returned, but Editor binding should be first (highest precedence)
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionBeta {})); // Editor binding first
        assert!(result[1].action.partial_eq(&ActionAlpha {})); // Workspace binding second
    }

    #[test]
    fn test_bindings_for_action() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, Some("pane")),
            KeyBinding::new("ctrl-b", ActionBeta {}, Some("editor && mode == full")),
            KeyBinding::new("ctrl-c", ActionGamma {}, Some("workspace")),
            KeyBinding::new("ctrl-a", NoAction {}, Some("pane && active")),
            KeyBinding::new("ctrl-b", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert_bindings(&keymap, &ActionAlpha {}, &["ctrl-a"]);
        assert_bindings(&keymap, &ActionBeta {}, &[]);
        assert_bindings(&keymap, &ActionGamma {}, &["ctrl-c"]);

        #[track_caller]
        fn assert_bindings(keymap: &Keymap, action: &dyn Action, expected: &[&str]) {
            let actual = keymap
                .bindings_for_action(action)
                .map(|binding| binding.keystrokes[0].inner().unparse())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{:?}", action);
        }
    }

    #[test]
    fn test_targeted_unbind_ignores_target_context() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("tab", ActionBeta {}, Some("Editor && showing_completions")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor && edit_prediction"),
            ),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("tab").unwrap()],
            &[KeyContext::parse("Editor showing_completions edit_prediction").unwrap()],
        );

        assert!(!pending);
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));
    }

    #[test]
    fn test_bindings_for_action_keeps_binding_for_narrower_targeted_unbind() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor && edit_prediction"),
            ),
            KeyBinding::new("tab", ActionBeta {}, Some("Editor && showing_completions")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert_bindings(&keymap, &ActionAlpha {}, &["tab"]);
        assert_bindings(&keymap, &ActionBeta {}, &["tab"]);

        #[track_caller]
        fn assert_bindings(keymap: &Keymap, action: &dyn Action, expected: &[&str]) {
            let actual = keymap
                .bindings_for_action(action)
                .map(|binding| binding.keystrokes[0].inner().unparse())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{:?}", action);
        }
    }

    #[test]
    fn test_bindings_for_action_removes_binding_for_broader_targeted_unbind() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor && edit_prediction")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor"),
            ),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert!(keymap.bindings_for_action(&ActionAlpha {}).next().is_none());
    }

    #[test]
    fn test_source_precedence_sorting() {
        // KeybindSource precedence: User (0) > Vim (1) > Base (2) > Default (3)
        // Test that user keymaps take precedence over default keymaps at the same context depth
        let mut keymap = Keymap::default();

        // Add a default keymap binding first
        let mut default_binding = KeyBinding::new("cmd-r", ActionAlpha {}, Some("Editor"));
        default_binding.set_meta(KeyBindingMetaIndex(3)); // Default source
        keymap.add_bindings([default_binding]);

        // Add a user keymap binding
        let mut user_binding = KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor"));
        user_binding.set_meta(KeyBindingMetaIndex(0)); // User source
        keymap.add_bindings([user_binding]);

        // Test with Editor context stack
        let (result, _) = keymap.bindings_for_input(
            &[Keystroke::parse("cmd-r").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );

        // User binding should take precedence over default binding
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionBeta {}));
        assert!(result[1].action.partial_eq(&ActionAlpha {}));
    }

    #[test]
    fn test_possible_next_bindings_exclude_null_bindings() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionBeta {}, Some("Editor")),
            KeyBinding::new("ctrl-k a", NoAction {}, Some("Editor")),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k b".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_exclude_unbound_bindings() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionBeta {}, Some("Editor")),
            KeyBinding::new(
                "ctrl-k a",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor"),
            ),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k b".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_keep_user_binding_over_base_keymap_null() {
        const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
        const BASE: KeyBindingMetaIndex = KeyBindingMetaIndex(2);
        const DEFAULT: KeyBindingMetaIndex = KeyBindingMetaIndex(3);

        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-k a", NoAction {}, Some("Editor")).with_meta(BASE),
            KeyBinding::new("ctrl-k a", ActionBeta {}, Some("Editor")).with_meta(USER),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k a".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_apply_disabling_by_context() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("ctrl-k a", ActionBeta {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionGamma {}, Some("Editor")),
            // Only removes the editor binding, so the workspace binding still dispatches.
            KeyBinding::new(
                "ctrl-k a",
                Unbind("test_only::ActionBeta".into()),
                Some("Editor"),
            ),
            // Doesn't apply because `Terminal` isn't in the context stack.
            KeyBinding::new("ctrl-k b", NoAction {}, Some("Terminal")),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Editor").unwrap(),
            ],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![
                ("ctrl-k b".to_string(), "test_only::ActionGamma"),
                ("ctrl-k a".to_string(), "test_only::ActionAlpha"),
            ]
        );
    }

    #[test]
    fn test_possible_next_bindings_exclude_longer_chord_disabled_in_deeper_context() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("space w w", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("space w x", ActionBeta {}, Some("Workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("AgentPanel")),
        ]);
        let context_stack = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("AgentPanel").unwrap(),
        ];

        for input in ["space", "space w"] {
            let input = input
                .split(' ')
                .map(|keystroke| Keystroke::parse(keystroke).unwrap())
                .collect::<Vec<_>>();
            let bindings = keymap.possible_next_bindings_for_input(&input, &context_stack);
            assert_eq!(
                describe_bindings(&bindings),
                vec![("space w x".to_string(), "test_only::ActionBeta")]
            );
        }
    }

    #[test]
    fn test_possible_next_bindings_exclude_disabled_bindings_with_typed_input() {
        // Option-S reports both its physical key and the character it produced, so typed input
        // matches bindings written with either.
        let typed_input = [Keystroke::parse("alt-s->ß").unwrap()];
        let keymap = Keymap::new(vec![
            KeyBinding::new("ß a", ActionAlpha {}, None),
            KeyBinding::new("alt-s a", NoAction {}, None),
        ]);
        assert!(
            keymap
                .possible_next_bindings_for_input(&typed_input, &[])
                .is_empty()
        );

        // Cross-check against dispatch of the same typed input.
        let typed_then_a = [
            Keystroke::parse("alt-s->ß").unwrap(),
            Keystroke::parse("a").unwrap(),
        ];
        let (dispatched, _) = keymap.bindings_for_input(&typed_then_a, &[]);
        assert!(dispatched.is_empty());
    }

    fn describe_bindings(bindings: &[KeyBinding]) -> Vec<(String, &'static str)> {
        bindings
            .iter()
            .map(|binding| {
                (
                    binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| keystroke.inner().unparse())
                        .collect::<Vec<_>>()
                        .join(" "),
                    binding.action().name(),
                )
            })
            .collect()
    }

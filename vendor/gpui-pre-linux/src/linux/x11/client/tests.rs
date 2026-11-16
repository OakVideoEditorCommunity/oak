    use super::*;

    fn test_keymap(layouts: &str) -> xkbc::Keymap {
        test_keymap_with_variant(layouts, "")
    }

    fn test_keymap_with_variant(layouts: &str, variant: &str) -> xkbc::Keymap {
        // These fixtures compile layout names from local files, unlike server keymaps.
        let context = xkbc::Context::new(xkbc::CONTEXT_NO_FLAGS);
        assert!(!context.get_raw_ptr().is_null());
        xkbc::Keymap::new_from_names(
            &context,
            "",
            "pc105",
            layouts,
            variant,
            None,
            xkbc::COMPILE_NO_FLAGS,
        )
        .expect("test keymap should compile")
    }

    // Returns a state where the second layout is active via a temporary
    // mechanism (holding a key down or one-shot), not a permanent toggle.
    fn state_with_non_locked_layout(keymap: &xkbc::Keymap) -> xkbc::State {
        let mut depressed_layout_state = xkbc::State::new(keymap);
        depressed_layout_state.update_mask(0, 0, 0, 1, 0, 0);
        if depressed_layout_state.serialize_layout(STATE_LAYOUT_EFFECTIVE) == 1 {
            return depressed_layout_state;
        }

        let mut latched_layout_state = xkbc::State::new(keymap);
        latched_layout_state.update_mask(0, 0, 0, 0, 1, 0);
        if latched_layout_state.serialize_layout(STATE_LAYOUT_EFFECTIVE) == 1 {
            return latched_layout_state;
        }

        panic!("test keymap should support a non-locked secondary layout");
    }

    #[test]
    fn key_event_state_uses_event_modifiers_without_mutating_server_state() {
        let keymap = test_keymap("us");
        let server_state = xkbc::State::new(&keymap);
        // The "9" key on a US keyboard.
        let keycode = keymap
            .key_by_name("AE09")
            .expect("test key should exist in the keymap");

        // Simulate pressing Shift+9 (which should produce "(").
        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::SHIFT);
        let keystroke = keystroke_from_xkb(
            &key_event_state,
            modifiers_from_state(xproto::KeyButMask::SHIFT),
            keycode,
        );

        // Assert Shift+9 produces "(" on US layout.
        assert_eq!(keystroke.key, "(");
        assert_eq!(keystroke.key_char.as_deref(), Some("("));
        // Assert the long-lived server state was not mutated by the key event.
        assert_eq!(server_state.key_get_utf8(keycode), "9");
    }

    #[test]
    fn key_event_state_ignores_pointer_button_bits() {
        let keymap = test_keymap("us");
        let server_state = xkbc::State::new(&keymap);
        // The "9" key on a US keyboard.
        let keycode = keymap
            .key_by_name("AE09")
            .expect("test key should exist in the keymap");

        // Simulate Shift held down.
        let shifted_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::SHIFT);
        // Simulate Shift held down while also clicking the left mouse button.
        let shifted_with_button_state = xkb_state_for_key_event(
            &server_state,
            xproto::KeyButMask::SHIFT | xproto::KeyButMask::BUTTON1,
        );

        // Assert the mouse button has no effect on modifier state.
        assert_eq!(
            shifted_with_button_state.serialize_mods(xkbc::STATE_MODS_EFFECTIVE),
            shifted_state.serialize_mods(xkbc::STATE_MODS_EFFECTIVE)
        );
        // Assert both cases produce the same character.
        assert_eq!(
            shifted_with_button_state.key_get_utf8(keycode),
            shifted_state.key_get_utf8(keycode)
        );
    }

    #[test]
    fn key_event_state_preserves_non_locked_layout_components() {
        // US + Russian dual-layout keyboard.
        let keymap = test_keymap("us,ru");
        // Simulate the Russian layout being active via a temporary layout
        // switch (holding a key), not a permanent toggle.
        let server_state = state_with_non_locked_layout(&keymap);
        // The "Q" key position, which produces a Cyrillic character in Russian layout.
        let keycode = keymap
            .key_by_name("AD01")
            .expect("test key should exist in the keymap");

        let expected_text = server_state.key_get_utf8(keycode);
        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::default());

        // Assert the temporary layout switch is preserved.
        assert_eq!(
            key_event_state.serialize_layout(STATE_LAYOUT_EFFECTIVE),
            server_state.serialize_layout(STATE_LAYOUT_EFFECTIVE)
        );
        // Assert the key produces the same character as expected from the
        // Russian layout.
        assert_eq!(key_event_state.key_get_utf8(keycode), expected_text);
    }

    // https://github.com/zed-industries/zed/issues/14282
    #[test]
    fn capslock_toggle_produces_uppercase() {
        let keymap = test_keymap("us");
        let mut server_state = xkbc::State::new(&keymap);
        // The "A" key position on a US keyboard.
        let keycode = keymap
            .key_by_name("AC01")
            .expect("'a' key should exist in the keymap");

        // Simulate the user having toggled CapsLock on (it's now permanently
        // active until pressed again).
        let lock_mod = u16::from(xproto::KeyButMask::LOCK) as xkbc::ModMask;
        server_state.update_mask(0, 0, lock_mod, 0, 0, 0);

        // Simulate pressing the "a" key while CapsLock is on.
        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::LOCK);

        // Assert CapsLock is treated as a toggle (locked), not as a held key
        // (depressed). This distinction matters because XKB only applies
        // capitalization when CapsLock is in the "locked" state.
        assert_eq!(
            key_event_state.serialize_mods(xkbc::STATE_MODS_LOCKED) & lock_mod,
            lock_mod,
        );
        // Assert typing "a" with CapsLock on produces "A".
        assert_eq!(key_event_state.key_get_utf8(keycode), "A");
    }

    // https://github.com/zed-industries/zed/issues/14282
    #[test]
    fn neo2_level3_via_capslock_produces_ellipsis() {
        // Neo 2 is a German keyboard layout that repurposes CapsLock as a
        // "level 3" modifier key for accessing additional characters.
        let keymap = test_keymap_with_variant("de", "neo");
        let server_state = xkbc::State::new(&keymap);
        // The key in the "Q" position, which produces "x" on Neo 2 base layer.
        let keycode = keymap
            .key_by_name("AD01")
            .expect("test key should exist in the keymap");

        // Simulate holding CapsLock, which in Neo 2 activates the "level 3"
        // layer (mapped to the Mod5 modifier internally).
        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::MOD5);

        // Assert holding CapsLock + pressing the "x" key produces "..."
        // (ellipsis), which is the level 3 character on that key in Neo 2.
        assert_eq!(key_event_state.key_get_utf8(keycode), "\u{2026}");
    }

    // https://github.com/zed-industries/zed/issues/14282
    #[test]
    fn neo2_latched_mod5_preserved() {
        // Neo 2 also supports "latching" the level 3 modifier (via Caps+Tab),
        // which activates it for only the next keypress and then deactivates.
        let keymap = test_keymap_with_variant("de", "neo");
        let mut server_state = xkbc::State::new(&keymap);
        let keycode = keymap
            .key_by_name("AD01")
            .expect("test key should exist in the keymap");

        // Simulate the level 3 modifier being latched (one-shot active).
        let mod5 = u16::from(xproto::KeyButMask::MOD5) as xkbc::ModMask;
        server_state.update_mask(0, mod5, 0, 0, 0, 0);

        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::MOD5);

        // Assert the modifier stays classified as "latched" (one-shot) rather
        // than being reclassified as "depressed" (held down). This matters
        // because latched modifiers auto-deactivate after one keypress.
        assert_eq!(
            key_event_state.serialize_mods(xkbc::STATE_MODS_LATCHED) & mod5,
            mod5,
        );
        // Assert the latched level 3 still produces the ellipsis character.
        assert_eq!(key_event_state.key_get_utf8(keycode), "\u{2026}");
    }

    // https://github.com/zed-industries/zed/pull/31193
    #[test]
    fn german_layout_correct_key_resolution() {
        // Standard German keyboard layout.
        let keymap = test_keymap("de");
        let server_state = xkbc::State::new(&keymap);
        // The "7" key on the number row.
        let keycode = keymap
            .key_by_name("AE07")
            .expect("'7' key should exist in the keymap");

        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::default());

        // Assert pressing the "7" key on a German layout produces "7".
        assert_eq!(key_event_state.key_get_utf8(keycode), "7");
    }

    // https://github.com/zed-industries/zed/issues/26468
    // https://github.com/zed-industries/zed/issues/16667
    #[test]
    fn space_works_with_cyrillic_layout_active() {
        // US + Russian dual-layout keyboard.
        let keymap = test_keymap("us,ru");
        let mut server_state = xkbc::State::new(&keymap);
        let space = keymap
            .key_by_name("SPCE")
            .expect("space key should exist in the keymap");

        // Simulate the user having switched to the Russian layout
        // (e.g. via a keyboard shortcut like Super+Space).
        server_state.update_mask(0, 0, 0, 0, 0, 1);

        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::default());

        // Assert the Russian layout is still active after constructing the
        // key event state (not accidentally reset to US).
        assert_eq!(key_event_state.serialize_layout(STATE_LAYOUT_EFFECTIVE), 1);
        // Assert pressing space while on the Russian layout still types a space.
        assert_eq!(key_event_state.key_get_utf8(space), " ");
    }

    // https://github.com/zed-industries/zed/issues/40678
    #[test]
    fn macro_shift_bracket_produces_brace() {
        let keymap = test_keymap("us");
        let server_state = xkbc::State::new(&keymap);
        // The "]" key on a US keyboard.
        let bracket = keymap
            .key_by_name("AD12")
            .expect("']' key should exist in the keymap");

        // Simulate a keyboard macro (e.g. from a ZMK/QMK firmware keyboard)
        // that sends Shift + "]" very rapidly. The modifier state notification
        // for Shift hasn't reached us yet, so the server state has no
        // modifiers. But the key event itself carries the correct Shift state.
        assert_eq!(server_state.serialize_mods(xkbc::STATE_MODS_EFFECTIVE), 0);
        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::SHIFT);

        // Assert Shift+"]" produces "}" even when the Shift notification
        // arrived late.
        assert_eq!(key_event_state.key_get_utf8(bracket), "}");
    }

    // https://github.com/zed-industries/zed/issues/49329
    #[test]
    fn sequential_key_events_do_not_corrupt_state() {
        let keymap = test_keymap("us");
        let server_state = xkbc::State::new(&keymap);

        // Simulate typing "a s d" with spaces in between, all without any
        // modifier keys held.
        let keys: &[(&str, &str)] = &[
            ("AC01", "a"),
            ("SPCE", " "),
            ("AC02", "s"),
            ("SPCE", " "),
            ("AC03", "d"),
        ];

        for &(key_name, expected_utf8) in keys {
            let keycode = keymap
                .key_by_name(key_name)
                .expect("test key should exist in the keymap");

            let key_event_state =
                xkb_state_for_key_event(&server_state, xproto::KeyButMask::default());

            // Assert each key in the sequence produces the expected character
            // (no dropped or garbled input from state corruption).
            assert_eq!(
                key_event_state.key_get_utf8(keycode),
                expected_utf8,
                "key {key_name} should produce {expected_utf8:?}",
            );
        }

        // Assert the server state is completely untouched after processing
        // all key events.
        assert_eq!(server_state.serialize_mods(xkbc::STATE_MODS_EFFECTIVE), 0);
        assert_eq!(server_state.serialize_layout(STATE_LAYOUT_EFFECTIVE), 0);
    }

    // https://github.com/zed-industries/zed/issues/26468
    #[test]
    fn space_works_with_czech_layout_active() {
        // US + Czech dual-layout keyboard.
        let keymap = test_keymap("us,cz");
        let mut server_state = xkbc::State::new(&keymap);
        let space = keymap
            .key_by_name("SPCE")
            .expect("space key should exist in the keymap");

        // Simulate the user having switched to the Czech layout.
        server_state.update_mask(0, 0, 0, 0, 0, 1);

        let key_event_state = xkb_state_for_key_event(&server_state, xproto::KeyButMask::default());

        // Assert pressing space while on the Czech layout still types a space.
        assert_eq!(key_event_state.key_get_utf8(space), " ");
    }

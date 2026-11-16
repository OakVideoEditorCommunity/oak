    use super::*;
    use crate::{Font, FontFeatures, FontStyle, FontWeight, TestAppContext, TestDispatcher, font};
    #[cfg(target_os = "macos")]
    use crate::{TextRun, WindowTextSystem, WrapBoundary};

    fn build_wrapper() -> LineWrapper {
        let dispatcher = TestDispatcher::new(0);
        let cx = TestAppContext::build(dispatcher, None);
        let id = cx.text_system().resolve_font(&font(".ZedMono"));
        LineWrapper::new(id, px(16.), cx.text_system().clone())
    }

    fn generate_test_runs(input_run_len: &[usize]) -> Vec<TextRun> {
        input_run_len
            .iter()
            .map(|run_len| TextRun {
                len: *run_len,
                font: Font {
                    family: "Dummy".into(),
                    features: FontFeatures::default(),
                    fallbacks: None,
                    weight: FontWeight::default(),
                    style: FontStyle::Normal,
                },
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn test_wrap_line() {
        let mut wrapper = build_wrapper();

        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("aa bbb cccc ddddd eeee")],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(7, 0),
                Boundary::new(12, 0),
                Boundary::new(18, 0)
            ],
        );
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("aaa aaaaaaaaaaaaaaaaaa")],
                    px(72.0),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(4, 0),
                Boundary::new(11, 0),
                Boundary::new(18, 0)
            ],
        );
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("     aaaaaaa")],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(7, 5),
                Boundary::new(9, 5),
                Boundary::new(11, 5),
            ]
        );
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("                            ")],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(7, 0),
                Boundary::new(14, 0),
                Boundary::new(21, 0)
            ]
        );
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("          aaaaaaaaaaaaaa")],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(7, 0),
                Boundary::new(14, 3),
                Boundary::new(18, 3),
                Boundary::new(22, 3),
            ]
        );

        // Test wrapping multiple text fragments
        assert_eq!(
            wrapper
                .wrap_line(
                    &[
                        LineFragment::text("aa bbb "),
                        LineFragment::text("cccc ddddd eeee")
                    ],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(7, 0),
                Boundary::new(12, 0),
                Boundary::new(18, 0)
            ],
        );

        // Test wrapping with a mix of text and element fragments
        assert_eq!(
            wrapper
                .wrap_line(
                    &[
                        LineFragment::text("aa "),
                        LineFragment::element(px(20.), 1),
                        LineFragment::text(" bbb "),
                        LineFragment::element(px(30.), 1),
                        LineFragment::text(" cccc")
                    ],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(5, 0),
                Boundary::new(9, 0),
                Boundary::new(11, 0)
            ],
        );

        // Test with element at the beginning and text afterward
        assert_eq!(
            wrapper
                .wrap_line(
                    &[
                        LineFragment::element(px(50.), 1),
                        LineFragment::text(" aaaa bbbb cccc dddd")
                    ],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(2, 0),
                Boundary::new(7, 0),
                Boundary::new(12, 0),
                Boundary::new(17, 0)
            ],
        );

        // Test with a large element that forces wrapping by itself
        assert_eq!(
            wrapper
                .wrap_line(
                    &[
                        LineFragment::text("short text "),
                        LineFragment::element(px(100.), 1),
                        LineFragment::text(" more text")
                    ],
                    px(72.),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(6, 0),
                Boundary::new(11, 0),
                Boundary::new(12, 0),
                Boundary::new(18, 0)
            ],
        );

        // Test with non-breaking glue characters
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("a\u{202F}b\u{00A0}c\u{2011}d e")],
                    px(72.0),
                    IndentAdjustment::default()
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(12, 0),], // special chars above take up 3, 2 and 3 bytes, so boundary ends up at 12
        );
    }

    #[test]
    fn test_truncate_line_end() {
        let mut wrapper = build_wrapper();

        fn perform_test(
            wrapper: &mut LineWrapper,
            text: &'static str,
            expected: &'static str,
            ellipsis: &str,
        ) {
            let dummy_run_lens = vec![text.len()];
            let dummy_runs = generate_test_runs(&dummy_run_lens);
            let (result, dummy_runs) = wrapper.truncate_line(
                text.into(),
                px(220.),
                ellipsis,
                &dummy_runs,
                TruncateFrom::End,
            );
            assert_eq!(result, expected);
            assert_eq!(dummy_runs.first().unwrap().len, result.len());
        }

        perform_test(
            &mut wrapper,
            "aa bbb cccc ddddd eeee ffff gggg",
            "aa bbb cccc ddddd eeee",
            "",
        );
        perform_test(
            &mut wrapper,
            "aa bbb cccc ddddd eeee ffff gggg",
            "aa bbb cccc ddddd eee…",
            "…",
        );
        perform_test(
            &mut wrapper,
            "aa bbb cccc ddddd eeee ffff gggg",
            "aa bbb cccc dddd......",
            "......",
        );
        perform_test(
            &mut wrapper,
            "aa bbb cccc 🦀🦀🦀🦀🦀 eeee ffff gggg",
            "aa bbb cccc 🦀🦀🦀🦀…",
            "…",
        );
    }

    #[test]
    fn test_truncate_line_start() {
        let mut wrapper = build_wrapper();

        #[track_caller]
        fn perform_test(
            wrapper: &mut LineWrapper,
            text: &'static str,
            expected: &'static str,
            ellipsis: &str,
        ) {
            let dummy_run_lens = vec![text.len()];
            let dummy_runs = generate_test_runs(&dummy_run_lens);
            let (result, dummy_runs) = wrapper.truncate_line(
                text.into(),
                px(220.),
                ellipsis,
                &dummy_runs,
                TruncateFrom::Start,
            );
            assert_eq!(result, expected);
            assert_eq!(dummy_runs.first().unwrap().len, result.len());
        }

        perform_test(
            &mut wrapper,
            "aaaa bbbb cccc ddddd eeee fff gg",
            "cccc ddddd eeee fff gg",
            "",
        );
        perform_test(
            &mut wrapper,
            "aaaa bbbb cccc ddddd eeee fff gg",
            "…ccc ddddd eeee fff gg",
            "…",
        );
        perform_test(
            &mut wrapper,
            "aaaa bbbb cccc ddddd eeee fff gg",
            "......dddd eeee fff gg",
            "......",
        );
        perform_test(
            &mut wrapper,
            "aaaa bbbb cccc 🦀🦀🦀🦀🦀 eeee fff gg",
            "…🦀🦀🦀🦀 eeee fff gg",
            "…",
        );
    }

    #[test]
    fn test_truncate_multiple_runs_end() {
        let mut wrapper = build_wrapper();

        fn perform_test(
            wrapper: &mut LineWrapper,
            text: &'static str,
            expected: &str,
            run_lens: &[usize],
            result_run_len: &[usize],
            line_width: Pixels,
        ) {
            let dummy_runs = generate_test_runs(run_lens);
            let (result, dummy_runs) =
                wrapper.truncate_line(text.into(), line_width, "…", &dummy_runs, TruncateFrom::End);
            assert_eq!(result, expected);
            for (run, result_len) in dummy_runs.iter().zip(result_run_len) {
                assert_eq!(run.len, *result_len);
            }
        }
        // Case 0: Normal
        // Text: abcdefghijkl
        // Runs: Run0 { len: 12, ... }
        //
        // Truncate res: abcd… (truncate_at = 4)
        // Run res: Run0 { string: abcd…, len: 7, ... }
        perform_test(&mut wrapper, "abcdefghijkl", "abcd…", &[12], &[7], px(50.));
        // Case 1: Drop some runs
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: abcdef… (truncate_at = 6)
        // Runs res: Run0 { string: abcd, len: 4, ... }, Run1 { string: ef…, len:
        // 5, ... }
        perform_test(
            &mut wrapper,
            "abcdefghijkl",
            "abcdef…",
            &[4, 4, 4],
            &[4, 5],
            px(70.),
        );
        // Case 2: Truncate at start of some run
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: abcdefgh… (truncate_at = 8)
        // Runs res: Run0 { string: abcd, len: 4, ... }, Run1 { string: efgh, len:
        // 4, ... }, Run2 { string: …, len: 3, ... }
        perform_test(
            &mut wrapper,
            "abcdefghijkl",
            "abcdefgh…",
            &[4, 4, 4],
            &[4, 4, 3],
            px(90.),
        );
    }

    #[test]
    fn test_truncate_multiple_runs_start() {
        let mut wrapper = build_wrapper();

        #[track_caller]
        fn perform_test(
            wrapper: &mut LineWrapper,
            text: &'static str,
            expected: &str,
            run_lens: &[usize],
            result_run_len: &[usize],
            line_width: Pixels,
        ) {
            let dummy_runs = generate_test_runs(run_lens);
            let (result, dummy_runs) = wrapper.truncate_line(
                text.into(),
                line_width,
                "…",
                &dummy_runs,
                TruncateFrom::Start,
            );
            assert_eq!(result, expected);
            for (run, result_len) in dummy_runs.iter().zip(result_run_len) {
                assert_eq!(run.len, *result_len);
            }
        }
        // Case 0: Normal
        // Text: abcdefghijkl
        // Runs: Run0 { len: 12, ... }
        //
        // Truncate res: …ijkl (truncate_at = 9)
        // Run res: Run0 { string: …ijkl, len: 7, ... }
        perform_test(&mut wrapper, "abcdefghijkl", "…ijkl", &[12], &[7], px(50.));
        // Case 1: Drop some runs
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: …ghijkl (truncate_at = 7)
        // Runs res: Run0 { string: …gh, len: 5, ... }, Run1 { string: ijkl, len:
        // 4, ... }
        perform_test(
            &mut wrapper,
            "abcdefghijkl",
            "…ghijkl",
            &[4, 4, 4],
            &[5, 4],
            px(70.),
        );
        // Case 2: Truncate at start of some run
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: abcdefgh… (truncate_at = 3)
        // Runs res: Run0 { string: …, len: 3, ... }, Run1 { string: efgh, len:
        // 4, ... }, Run2 { string: ijkl, len: 4, ... }
        perform_test(
            &mut wrapper,
            "abcdefghijkl",
            "…efghijkl",
            &[4, 4, 4],
            &[3, 4, 4],
            px(90.),
        );
    }

    #[test]
    fn test_update_run_after_truncation_end() {
        fn perform_test(result: &str, run_lens: &[usize], result_run_lens: &[usize]) {
            let mut dummy_runs = generate_test_runs(run_lens);
            update_runs_after_truncation(result, "…", &mut dummy_runs, TruncateFrom::End);
            for (run, result_len) in dummy_runs.iter().zip(result_run_lens) {
                assert_eq!(run.len, *result_len);
            }
        }
        // Case 0: Normal
        // Text: abcdefghijkl
        // Runs: Run0 { len: 12, ... }
        //
        // Truncate res: abcd… (truncate_at = 4)
        // Run res: Run0 { string: abcd…, len: 7, ... }
        perform_test("abcd…", &[12], &[7]);
        // Case 1: Drop some runs
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: abcdef… (truncate_at = 6)
        // Runs res: Run0 { string: abcd, len: 4, ... }, Run1 { string: ef…, len:
        // 5, ... }
        perform_test("abcdef…", &[4, 4, 4], &[4, 5]);
        // Case 2: Truncate at start of some run
        // Text: abcdefghijkl
        // Runs: Run0 { len: 4, ... }, Run1 { len: 4, ... }, Run2 { len: 4, ... }
        //
        // Truncate res: abcdefgh… (truncate_at = 8)
        // Runs res: Run0 { string: abcd, len: 4, ... }, Run1 { string: efgh, len:
        // 4, ... }, Run2 { string: …, len: 3, ... }
        perform_test("abcdefgh…", &[4, 4, 4], &[4, 4, 3]);
    }

    #[test]
    fn test_is_word_char() {
        #[track_caller]
        fn assert_word(word: &str) {
            for c in word.chars() {
                assert!(
                    LineWrapper::is_word_char(c),
                    "assertion failed for '{}' (unicode 0x{:x})",
                    c,
                    c as u32
                );
            }
        }

        #[track_caller]
        fn assert_not_word(word: &str) {
            let found = word.chars().any(|c| !LineWrapper::is_word_char(c));
            assert!(found, "assertion failed for '{}'", word);
        }

        assert_word("Hello123");
        assert_word("non-English");
        assert_word("var_name");
        assert_word("123456");
        assert_word("3.1415");
        assert_word("10^2");
        assert_word("1~2");
        assert_word("100%");
        assert_word("@mention");
        assert_word("#hashtag");
        assert_word("$variable");
        assert_word("a=1");
        assert_word("Self::is_word_char");
        assert_word("on;");
        assert_word("more⋯");
        assert_word("won’t");
        assert_word("‘twas");
        assert_word("plz!");
        assert_word("see)");
        assert_word("quoted”");
        assert_word("well…");

        // Space
        assert_not_word("foo bar");

        // URL case
        assert_word("github.com");
        assert_not_word("zed-industries/zed");
        assert_not_word("zed-industries\\zed");
        assert_not_word("a=1&b=2");
        assert_not_word("foo?b=2");

        // Latin-1 Supplement
        assert_word("ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏ");
        // Latin Extended-A
        assert_word("ĀāĂăĄąĆćĈĉĊċČčĎď");
        // Latin Extended-B
        assert_word("ƀƁƂƃƄƅƆƇƈƉƊƋƌƍƎƏ");
        // Cyrillic
        assert_word("АБВГДЕЖЗИЙКЛМНОП");
        // Vietnamese (https://github.com/zed-industries/zed/issues/23245)
        assert_word("ThậmchíđếnkhithuachạychúngcònnhẫntâmgiếtnốtsốđôngtùchínhtrịởYênBáivàCaoBằng");
        // Bengali
        assert_word("গিয়েছিলেন");
        assert_word("ছেলে");
        assert_word("হচ্ছিল");

        // non-word characters
        assert_not_word("你好");
        assert_not_word("안녕하세요");
        assert_not_word("こんにちは");
        assert_not_word("😀😁😂");
        assert_not_word("()[]{}<>");

        // Non-breaking ("Glue") characters, see https://www.unicode.org/reports/tr14/
        // (https://github.com/zed-industries/zed/issues/59664)
        assert_word("\u{202F}"); // NNBSP " "
        assert_word("\u{00A0}"); // NBSP " "
        assert_word("\u{2011}"); // NBH "‑"
    }

    // For compatibility with the test macro
    #[cfg(target_os = "macos")]
    use crate as gpui;

    // These seem to vary wildly based on the text system.
    #[cfg(target_os = "macos")]
    #[crate::test]
    fn test_wrap_shaped_line(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let text_system = WindowTextSystem::new(cx.text_system().clone());

            let normal = TextRun {
                len: 0,
                font: font("Helvetica"),
                color: Default::default(),
                underline: Default::default(),
                ..Default::default()
            };
            let bold = TextRun {
                len: 0,
                font: font("Helvetica").bold(),
                ..Default::default()
            };

            let text = "aa bbb cccc ddddd eeee".into();
            let lines = text_system
                .shape_text(
                    text,
                    px(16.),
                    &[
                        normal.with_len(4),
                        bold.with_len(5),
                        normal.with_len(6),
                        bold.with_len(1),
                        normal.with_len(7),
                    ],
                    Some(px(72.)),
                    None,
                )
                .unwrap();

            assert_eq!(
                lines[0].layout.wrap_boundaries(),
                &[
                    WrapBoundary {
                        run_ix: 0,
                        glyph_ix: 7
                    },
                    WrapBoundary {
                        run_ix: 0,
                        glyph_ix: 12
                    },
                    WrapBoundary {
                        run_ix: 0,
                        glyph_ix: 18
                    }
                ],
            );
        });
    }

    #[test]
    fn test_multiline_truncation_fits_within_wrapped_lines() {
        let mut wrapper = build_wrapper();

        // With .ZedMono at 16px, each char is 9.6px wide.
        // wrap_width = 72px fits ~7 chars per line.
        //
        // "aa bbbbbb cccccc dddddd eeee ffff" with wrap_width=72px wraps as:
        //   Line 1: "aa "       (28.8px, wraps because "bbbbbb" won't fit)
        //   Line 2: "bbbbbb "   (67.2px)
        //   Line 3: "cccccc "   (67.2px)
        //   ...
        //
        // truncate_wrapped_line should wrap first to find line 2 starts at
        // "bbbbbb...", then truncate only that line to fit with ellipsis.
        let text: &str = "aa bbbbbb cccccc dddddd eeee ffff";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (truncated, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        // The truncated text, when wrapped, must fit within max_lines lines.
        let wrap_count = wrapper
            .wrap_line(
                &[LineFragment::text(&truncated)],
                wrap_width,
                IndentAdjustment::default(),
            )
            .count();

        assert!(
            wrap_count < max_lines,
            "Truncated text '{}' wraps into {} visual lines, expected at most {}",
            truncated,
            wrap_count + 1,
            max_lines
        );

        // The truncated text should end with the ellipsis.
        assert!(
            truncated.ends_with('\u{2026}'),
            "Truncated text '{}' should end with ellipsis",
            truncated
        );
    }

    #[test]
    fn test_multiline_truncation_no_truncation_needed() {
        let mut wrapper = build_wrapper();

        // Text that fits in 2 lines shouldn't be truncated.
        // Line 1: "aa bbb " (67.2px), Line 2: "cccccc" (57.6px)
        let text: &str = "aa bbb cccccc";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (result, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        assert_eq!(
            result.as_ref(),
            text,
            "Text that fits should not be modified"
        );
    }

    #[test]
    fn test_multiline_truncation_three_lines() {
        let mut wrapper = build_wrapper();

        let text: &str = "aa bbb cccc ddddd eeee ffff gggg hhhh iiii jjjj";
        let wrap_width = px(72.);
        let max_lines: usize = 3;

        let runs = generate_test_runs(&[text.len()]);
        let (truncated, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        let wrap_count = wrapper
            .wrap_line(
                &[LineFragment::text(&truncated)],
                wrap_width,
                IndentAdjustment::default(),
            )
            .count();

        assert!(
            wrap_count < max_lines,
            "Truncated text '{}' wraps into {} visual lines, expected at most {}",
            truncated,
            wrap_count + 1,
            max_lines
        );

        assert!(
            truncated.ends_with('\u{2026}'),
            "Truncated text '{}' should end with ellipsis",
            truncated
        );
    }

    #[test]
    fn test_multiline_truncation_with_newlines() {
        let mut wrapper = build_wrapper();

        // "hello\nworld foo bar baz" with line_clamp(2):
        // shape_text splits on \n, giving physical lines "hello" and
        // "world foo bar baz". The newline consumes line 1, so the
        // second physical line should be truncated on line 2.
        let text: &str = "hello\nworld foo bar baz";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (truncated, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        // The newline should be preserved.
        let parts: Vec<&str> = truncated.splitn(2, '\n').collect();
        assert_eq!(
            parts.len(),
            2,
            "Newline should be preserved: '{}'",
            truncated
        );
        assert_eq!(parts[0], "hello");

        // The second line should fit within wrap_width and end with ellipsis.
        let second_line_width: Pixels = parts[1].chars().map(|c| wrapper.width_for_char(c)).sum();
        assert!(
            second_line_width <= wrap_width,
            "Second line '{}' ({}px) exceeds wrap_width ({}px)",
            parts[1],
            second_line_width,
            wrap_width
        );
        assert!(
            truncated.ends_with('\u{2026}'),
            "Should end with ellipsis: '{}'",
            truncated
        );
    }

    #[test]
    fn test_multiline_truncation_newline_on_last_line() {
        let mut wrapper = build_wrapper();

        // "hello\nworld\nmore" with line_clamp(2):
        // Line 1: "hello", Line 2: "world" — but there's a third line,
        // so line 2 should be truncated with ellipsis.
        let text: &str = "hello\nworld\nmore";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (truncated, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        let parts: Vec<&str> = truncated.splitn(2, '\n').collect();
        assert_eq!(parts[0], "hello");
        assert!(
            truncated.ends_with('\u{2026}'),
            "Should end with ellipsis since there's more content: '{}'",
            truncated
        );
    }

    #[test]
    fn test_truncate_line_middle() {
        let mut wrapper = build_wrapper();

        // No truncation when text fits within a very wide budget.
        let short_text = "hello world";
        let runs = generate_test_runs(&[short_text.len()]);
        let (result, result_runs) = wrapper.truncate_line(
            short_text.into(),
            px(10000.),
            "…",
            &runs,
            TruncateFrom::Middle,
        );
        assert_eq!(result.as_ref(), short_text);
        assert_eq!(result_runs.len(), 1);
        assert_eq!(result_runs[0].len, short_text.len());

        // Basic middle truncation: long string with px(100.) budget.
        let long_text = "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz";
        let runs = generate_test_runs(&[long_text.len()]);
        let (result, _result_runs) =
            wrapper.truncate_line(long_text.into(), px(100.), "…", &runs, TruncateFrom::Middle);
        assert!(
            result.contains('…'),
            "Middle-truncated result should contain '…', got: '{}'",
            result
        );
        assert!(
            result.chars().count() < long_text.chars().count(),
            "Middle-truncated result should be shorter than original"
        );
        assert_eq!(
            result.chars().next(),
            long_text.chars().next(),
            "Result should start with the same first character as original"
        );
        assert_eq!(
            result.chars().last(),
            long_text.chars().last(),
            "Result should end with the same last character as original"
        );

        // Degenerate case: budget so narrow that middle truncation cannot find a valid split.
        // Still show the truncation affix instead of returning the original overflowing text.
        let text = "abcdef";
        let runs = generate_test_runs(&[text.len()]);
        let (result, result_runs) =
            wrapper.truncate_line(text.into(), px(1.), "…", &runs, TruncateFrom::Middle);
        assert_eq!(result.as_ref(), "…");
        assert_eq!(result_runs.len(), 1);
        assert_eq!(result_runs[0].len, "…".len());

        // Run adjustment correctness: multiple runs across the string.
        // Verify that the returned runs' lengths sum to result.len().
        let multi_run_text = "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz";
        let run_lens = [20, 20, multi_run_text.len() - 40];
        let runs = generate_test_runs(&run_lens);
        let (result, result_runs) = wrapper.truncate_line(
            multi_run_text.into(),
            px(100.),
            "…",
            &runs,
            TruncateFrom::Middle,
        );
        let total_run_len: usize = result_runs.iter().map(|r| r.len).sum();
        assert_eq!(
            total_run_len,
            result.len(),
            "Sum of run lengths ({}) should equal result byte length ({})",
            total_run_len,
            result.len()
        );
    }

    #[test]
    fn test_multiline_truncation_trailing_newline() {
        let mut wrapper = build_wrapper();

        // "hello\nworld\n" with line_clamp(2):
        // The trailing newline has no content after it, so no ellipsis.
        let text: &str = "hello\nworld\n";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (result, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        assert!(
            !result.ends_with('\u{2026}'),
            "Trailing newline with no content should not add ellipsis: '{}'",
            result
        );
    }

    #[test]
    fn test_multiline_truncation_newline_fits_exactly() {
        let mut wrapper = build_wrapper();

        // "hello\nworld" with line_clamp(2):
        // Exactly 2 lines, no truncation needed.
        let text: &str = "hello\nworld";
        let wrap_width = px(72.);
        let max_lines: usize = 2;

        let runs = generate_test_runs(&[text.len()]);
        let (result, _) = wrapper.truncate_wrapped_line(
            text.into(),
            wrap_width,
            max_lines,
            "\u{2026}",
            &runs,
            TruncateFrom::End,
        );

        assert_eq!(
            result.as_ref(),
            text,
            "Text that fits exactly should not be modified: '{}'",
            result
        );
    }

    #[test]
    fn test_extra_columns_overflow_guard() {
        let mut wrapper = build_wrapper();
        let space_width = wrapper.width_for_char(' ');

        // 6 spaces indent, wrap width 10 columns.
        let text = "      ab cd ef gh";
        let wrap_width = space_width * 10.0;

        // When base_indent + extra overflows wrap width (6 + 8 + 2 > 10),
        // indent must fall back to 0 instead of degrading to one character per row.
        //
        // Expected wrapped lines (10 columns):
        //   |      ab  |  (row 0: 6 spaces + "ab ", len 9)
        //   |cd ef gh  |  (row 1: 0 indent + "cd ef gh", len 8)
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(text)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(8),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(9, 0)]
        );

        // When base_indent + extra fits within wrap width (6 + 2 + 2 <= 10),
        // the extra indent is applied and not clamped.
        //
        // Expected wrapped lines (10 columns):
        //   |      ab  |  (row 0: 6 spaces + "ab ", len 9)
        //   |        cd|  (row 1: 8 spaces + "cd", len 10)
        //   |        ef|  (row 2: 8 spaces + "ef", len 10)
        //   |        gh|  (row 3: 8 spaces + "gh", len 10)
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(text)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(2),
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(9, 8),
                Boundary::new(11, 8),
                Boundary::new(13, 8),
                Boundary::new(15, 8),
            ]
        );

        // When text contains full-width (two-column) glyphs, reserving headroom for a
        // two-column character (candidate + 2 > wrap_width) ensures that candidate
        // indents leaving only 1 column cannot accept the indent and overflow.
        //
        // 1 space indent, wrap width 10 columns.
        // " 🦀🦀🦀🦀🦀🦀🦀🦀" with extra=8 (extra_two with tab_size=4):
        // candidate = 1 + 8 = 9.
        // With +2 headroom (9 + 2 > 10), indent falls back to 0.
        //
        // Expected wrapped lines (10 columns):
        //   | 🦀🦀🦀🦀 |  (row 0: 1 space + 4 two-column glyphs, 9 cols)
        //   |🦀🦀🦀🦀  |  (row 1: 0 indent + 4 two-column glyphs, 8 cols)
        let full_width_text = " 🦀🦀🦀🦀🦀🦀🦀🦀";
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(full_width_text)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(8),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(17, 0)]
        );

        // When the carried suffix fits within wrap width alongside extra indent
        // (1 indent + 9 carried suffix = 10 <= 10), the extra indent is applied.
        //
        // Expected wrapped lines (10 columns):
        //   |a         |  (row 0: "a ", len 2)
        //   | abcdefghi|  (row 1: 1 indent + "abcdefghi", len 10)
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("a abcdefghi")],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(1),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(2, 1)]
        );

        // When extra indent exceeds wrap width by even 1 column
        // (2 indent + 9 carried suffix = 11 > 10), indent falls back to 0.
        //
        // Expected wrapped lines (10 columns):
        //   |a         |  (row 0: "a ", len 2)
        //   |abcdefghi |  (row 1: 0 indent + "abcdefghi", len 9)
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("a abcdefghi")],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(2),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(2, 0)]
        );

        // When text wraps at an earlier word boundary, the carried suffix
        // must be accounted for so indent + carried_suffix <= wrap_width.
        //
        // "a abcdefghij" with wrap_width 10 columns, ExtraColumns(8):
        // candidate = 0 + 8 = 8.
        // carried suffix = "abcdefghi" (9 columns).
        // 8 indent + 9 carried suffix = 17 > 10, so indent falls back to 0.
        //
        // Expected wrapped lines (10 columns):
        //   |a         |  (row 0: "a ", len 2)
        //   |abcdefghij|  (row 1: 0 indent + "abcdefghij", len 10)
        let carried_text = "a abcdefghij";
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(carried_text)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(8),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(2, 0)]
        );

        // When a wider inline element is encountered after the first wrap,
        // the overflow guard must evaluate whether the element fits with the extra indent.
        // If text continues after the element row and wraps again, subsequent continuation
        // lines resume the extra indent if their carried content fits.
        //
        // "abcdefghijk " followed by an 8-column element and "z 12":
        // Row 0: "abcdefghij" (len 10)
        // Row 1: "k " (2 cols) with 8 indent (len 10)
        // Row 2: element (8 cols) cannot fit with 8 indent (8 + 8 = 16 > 10),
        //        so indent falls back to 0. Element (8 cols) + "z " (2 cols) = len 10.
        // Row 3: "12" (2 cols) fits with 8 indent (8 + 2 = 10 <= 10).
        //
        // Expected wrapped lines (10 columns):
        //   |abcdefghij|  (row 0: 10 cols)
        //   |        k |  (row 1: 8 indent + "k ", len 10)
        //   |[ELEMENT]z|  (row 2: 0 indent + [ELEMENT (8)] + "z ", len 10)
        //   |        12|  (row 3: 8 indent + "12", len 10)
        let element_fragments = [
            LineFragment::text("abcdefghijk "),
            LineFragment::element(space_width * 8.0, 1),
            LineFragment::text("z 12"),
        ];
        assert_eq!(
            wrapper
                .wrap_line(
                    &element_fragments,
                    wrap_width,
                    IndentAdjustment::ExtraColumns(8),
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(10, 8),
                Boundary::new(12, 0),
                Boundary::new(15, 8),
            ]
        );

        // A line of only whitespace wrapping before any non-whitespace character
        // must not have extra columns added to subsequent rows.
        let spaces = "                    "; // 20 spaces
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(spaces)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(8),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(10, 0)]
        );

        // When leading whitespace wraps before the first non-whitespace character,
        // base_indent should reflect the leading whitespace on the row where non-whitespace begins.
        // 14 spaces followed by "ab cd ef gh", wrap width 10:
        // Row 0: 10 spaces (len 10) -> wraps at ix 10 with indent 0
        // Row 1: 4 spaces + "ab cd" (len 9) -> wraps at ix 20
        //        base_indent is 14 - 10 = 4.
        //        With ExtraColumns(2), candidate = 4 + 2 = 6. 6 + 2 (headroom) = 8 <= 10.
        //        Row 2 and subsequent rows get indent 6.
        let multiline_indent_text = "              ab cd ef gh";
        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text(multiline_indent_text)],
                    wrap_width,
                    IndentAdjustment::ExtraColumns(2),
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(10, 0),
                Boundary::new(20, 6),
                Boundary::new(23, 6),
            ]
        );

        // When a word boundary precedes an oversized carried suffix that itself
        // exceeds wrap_width (e.g. text followed by an inline element), wrapping
        // at the earlier candidate would force the continuation row to immediately
        // overflow even with 0 indent. The wrapper must reject the candidate and
        // break before the overflowing item instead.
        //
        // "aaaaaaaaaaaaaaaaaaa a" (21 chars: 19 'a's, space, 'a') followed by 10-column element:
        // Row 0: "aaaaaaaaaa" (10 chars, len 10) -> wraps at ix 10
        // Row 1: "aaaaaaaa" (8 chars) with 2 indent (len 10) -> wraps at ix 18
        // Row 2: "a a" (3 chars) with 2 indent (len 5)
        //        candidate boundary at ix 20 (space)
        //        Then element (10 cols): width becomes 5 + 10 = 15 > wrap_width (10).
        //        Candidate would carry 'a' (1 col) + element (10 cols) = 11 cols > 10.
        //        Because carried suffix (11) > wrap_width (10), candidate is rejected.
        //        Wrapper breaks at ix 21 (before element) with indent 0.
        // Row 3: element (10 cols, len 10)
        let oversized_suffix_fragments = [
            LineFragment::text("aaaaaaaaaaaaaaaaaaa a"),
            LineFragment::element(space_width * 10.0, 1),
        ];
        assert_eq!(
            wrapper
                .wrap_line(
                    &oversized_suffix_fragments,
                    wrap_width,
                    IndentAdjustment::ExtraColumns(2),
                )
                .collect::<Vec<_>>(),
            &[
                Boundary::new(10, 2),
                Boundary::new(18, 2),
                Boundary::new(21, 0),
            ]
        );

        assert_eq!(
            wrapper
                .wrap_line(
                    &[LineFragment::text("  aaaaaaaa")],
                    space_width * 4.0,
                    IndentAdjustment::ExtraColumns(u32::MAX),
                )
                .collect::<Vec<_>>(),
            &[Boundary::new(4, 0), Boundary::new(8, 0)]
        );
    }

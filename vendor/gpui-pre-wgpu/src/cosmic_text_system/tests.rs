    use super::*;
    #[cfg(feature = "test-support")]
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn all_font_names_tracks_available_families() -> Result<()> {
        let text_system = gpui::TextSystem::new(Arc::new(
            CosmicTextSystem::new_without_system_fonts("IBM Plex Sans"),
        ));
        assert!(text_system.all_font_names().is_empty());

        text_system.add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/lilex/Lilex-Regular.ttf"
        ))])?;
        assert_eq!(text_system.all_font_names(), ["Lilex"]);

        text_system.add_fonts(vec![
            Cow::Borrowed(IBM_PLEX),
            Cow::Borrowed(include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf")),
        ])?;
        assert_eq!(text_system.all_font_names(), ["IBM Plex Sans", "Lilex"]);
        Ok(())
    }

    fn fid(i: usize) -> FontId {
        FontId(i)
    }

    fn chain(ids: &[usize]) -> SmallVec<[(FontId, SharedString); 4]> {
        ids.iter()
            .map(|&i| (fid(i), SharedString::from(format!("fb{i}"))))
            .collect()
    }

    fn span(start: usize, end: usize, slot: Option<usize>, font_id: FontId) -> RunSpan {
        RunSpan {
            start,
            end,
            slot,
            font_id,
        }
    }

    const IBM_PLEX: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
    const LILEX: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");

    /// Every code point of `Bidi_Class=B`, each of which starts a new bidi
    /// paragraph and so can split one line into mixed-direction paragraphs.
    const SEPARATORS: &[char] = &[
        '\u{000a}', '\u{000d}', '\u{001c}', '\u{001d}', '\u{001e}', '\u{0085}', '\u{2029}',
    ];

    fn text_system() -> Result<CosmicTextSystem> {
        let text_system = CosmicTextSystem::new_without_system_fonts("IBM Plex Sans");
        text_system.add_fonts(vec![Cow::Borrowed(IBM_PLEX)])?;
        Ok(text_system)
    }

    #[test]
    fn font_properties_describe_the_selected_face() -> Result<()> {
        let text_system = text_system()?;
        let regular = gpui::font("IBM Plex Sans");
        let regular_id = text_system.font_id(&regular)?;
        for (weight, style) in [
            (gpui::FontWeight::MEDIUM, gpui::FontStyle::Normal),
            (gpui::FontWeight::BOLD, gpui::FontStyle::Italic),
            (gpui::FontWeight::NORMAL, gpui::FontStyle::Oblique),
        ] {
            let requested = Font {
                weight,
                style,
                ..regular.clone()
            };
            let font_id = text_system.font_id(&requested)?;
            assert_eq!(font_id, regular_id);
            assert_eq!(
                text_system.font_weight_and_style(font_id)?,
                (gpui::FontWeight::NORMAL, gpui::FontStyle::Normal)
            );
        }
        Ok(())
    }

    fn layout_text(text_system: &CosmicTextSystem, text: &str) -> Result<LineLayout> {
        let font_id = text_system.font_id(&gpui::font("IBM Plex Sans"))?;
        let runs = [FontRun {
            len: text.len(),
            font_id,
        }];
        Ok(text_system.layout_line(text, gpui::px(14.0), &runs))
    }

    /// Mirrors the original crash: mixed-direction text reaching the shaper
    /// through `shape_text`, which only splits lines on `\n`.
    #[test]
    fn shape_text_with_mixed_direction_paragraphs() -> Result<()> {
        let platform_text_system = Arc::new(text_system()?);
        let text_system = Arc::new(gpui::TextSystem::new(platform_text_system));
        let window_text_system = gpui::WindowTextSystem::new(text_system);

        let text: SharedString = "first line\n\u{05d0}\u{001c}A".into();
        let runs = [gpui::TextRun {
            len: text.len(),
            font: gpui::font("IBM Plex Sans"),
            ..Default::default()
        }];

        let lines = window_text_system.shape_text(text, gpui::px(14.0), &runs, None, None)?;

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].len(), "\u{05d0}\u{001c}A".len());
        assert!(lines[1].width() > Pixels::ZERO);
        Ok(())
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn reports_graphemes_that_exhaust_font_fallback() -> Result<()> {
        let platform_text_system = Arc::new(text_system()?);
        let dispatcher = gpui::TestDispatcher::new(0);
        let cx =
            gpui::TestAppContext::build_with_text_system(dispatcher, None, platform_text_system);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|cx| {
            let observed = observed.clone();
            cx.on_missing_glyphs(move |missing_glyphs, _| {
                observed.borrow_mut().extend_from_slice(missing_glyphs);
            })
        });
        let text: SharedString = "界".into();

        cx.update(|cx| {
            let text_system = gpui::WindowTextSystem::new(cx.text_system().clone());
            let runs = [gpui::TextRun {
                len: text.len(),
                font: gpui::font("IBM Plex Sans"),
                ..Default::default()
            }];
            text_system.shape_line(text, gpui::px(14.0), &runs, None);
        });
        cx.run_until_parked();

        let observed = observed.borrow();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].grapheme(), "界");
        assert_eq!(
            observed[0].font_class(),
            gpui::FallbackFontClass::Proportional
        );
        Ok(())
    }

    #[test]
    fn combines_missing_glyphs_from_one_grapheme() -> Result<()> {
        let text_system = text_system()?;
        let font_id = text_system.font_id(&gpui::font("IBM Plex Sans"))?;
        let text = "x\u{0301}";
        let runs = [FontRun {
            len: text.len(),
            font_id,
        }];

        let missing_glyphs = text_system
            .0
            .read()
            .missing_glyphs(text, &runs, [0, "x".len()]);

        assert_eq!(missing_glyphs.len(), 1);
        assert_eq!(missing_glyphs[0].grapheme(), text);
        Ok(())
    }

    #[test]
    fn adding_fonts_invalidates_cached_line_layouts() -> Result<()> {
        let platform_text_system = Arc::new(text_system()?);
        let text_system = Arc::new(gpui::TextSystem::new(platform_text_system.clone()));
        let window_text_system = gpui::WindowTextSystem::new(text_system.clone());
        let text: SharedString = "cached text".into();
        let runs = [gpui::TextRun {
            len: text.len(),
            font: gpui::font("IBM Plex Sans"),
            ..Default::default()
        }];

        let first_layout = window_text_system.shape_line(text.clone(), gpui::px(14.0), &runs, None);
        let cached_layout =
            window_text_system.shape_line(text.clone(), gpui::px(14.0), &runs, None);
        assert!(std::ptr::eq::<LineLayout>(
            &**first_layout,
            &**cached_layout
        ));
        let loaded_font_count = platform_text_system.0.read().loaded_fonts.len();

        text_system.add_fonts(vec![Cow::Borrowed(LILEX)])?;

        let refreshed_layout = window_text_system.shape_line(text, gpui::px(14.0), &runs, None);
        assert!(!std::ptr::eq::<LineLayout>(
            &**first_layout,
            &**refreshed_layout
        ));
        assert_eq!(
            platform_text_system.0.read().loaded_fonts.len(),
            loaded_font_count
        );
        Ok(())
    }

    #[test]
    fn layout_line_with_mixed_direction_paragraphs() -> Result<()> {
        let text_system = text_system()?;

        for separator in SEPARATORS {
            for text in [
                format!("\u{05d0}{separator}A"),
                format!("A{separator}\u{05d0}"),
            ] {
                let layout = layout_text(&text_system, &text)?;

                assert_eq!(layout.len, text.len(), "{text:?}");
                assert!(layout.width > Pixels::ZERO, "{text:?}");
                assert!(
                    layout.runs.iter().any(|run| !run.glyphs.is_empty()),
                    "{text:?}"
                );
            }
        }

        Ok(())
    }

    #[test]
    fn layout_line_with_separators_at_line_edges() -> Result<()> {
        let text_system = text_system()?;

        for text in [
            "\u{001c}",
            "\u{001c}\u{001c}",
            "\u{001c}\u{05d0}",
            "\u{05d0}\u{001c}",
            "\u{05d0}\u{001c}\u{001c}A",
            "\u{001c}\u{05d0}\u{001c}A\u{001c}",
        ] {
            let layout = layout_text(&text_system, text)?;
            assert_eq!(layout.len, text.len(), "{text:?}");
        }

        Ok(())
    }

    /// Glyph indices must stay absolute and positions ordered across segment
    /// boundaries, otherwise cursor placement and hit testing desync. Uses
    /// single-direction text so visual order matches logical order.
    #[test]
    fn layout_line_keeps_indices_and_positions_ordered_across_paragraphs() -> Result<()> {
        let text_system = text_system()?;
        let text = "ab\u{001c}cd\u{2029}ef";
        let layout = layout_text(&text_system, text)?;

        let glyphs: Vec<_> = layout.runs.iter().flat_map(|run| &run.glyphs).collect();
        assert!(!glyphs.is_empty());

        for glyph in &glyphs {
            assert!(glyph.index < text.len(), "{:?}", glyph.index);
            assert!(text.is_char_boundary(glyph.index), "{:?}", glyph.index);
        }
        for pair in glyphs.windows(2) {
            assert!(pair[0].index < pair[1].index);
            assert!(pair[0].position.x <= pair[1].position.x);
        }

        // Every segment contributes width, so the whole line is wider than its
        // leading paragraph alone.
        assert!(layout.width > layout_text(&text_system, "ab")?.width);
        Ok(())
    }

    /// A font run boundary that does not line up with a paragraph boundary must
    /// still be clipped to the right segments.
    #[test]
    fn layout_line_with_font_run_straddling_a_separator() -> Result<()> {
        let text_system = text_system()?;
        let font_id = text_system.font_id(&gpui::font("IBM Plex Sans"))?;
        let text = "ab\u{001c}\u{05d0}\u{05d1}";

        // The run boundary falls inside the trailing RTL paragraph.
        let runs = [
            FontRun {
                len: "ab\u{001c}\u{05d0}".len(),
                font_id,
            },
            FontRun {
                len: "\u{05d1}".len(),
                font_id,
            },
        ];
        let layout = text_system.layout_line(text, gpui::px(14.0), &runs);

        assert_eq!(layout.len, text.len());
        assert!(layout.width > Pixels::ZERO);
        Ok(())
    }

    /// Lines with no separator take the fast path and must be shaped exactly as
    /// they were before paragraph splitting existed.
    #[test]
    fn layout_line_without_separators_takes_fast_path() -> Result<()> {
        let text_system = text_system()?;

        for text in [
            "hello world",
            "\u{05d0}\u{05d1}\u{05d2}",
            "mixed \u{05d0}\u{05d1}",
        ] {
            assert!(!contains_paragraph_separator(text), "{text:?}");
            let layout = layout_text(&text_system, text)?;
            assert_eq!(layout.len, text.len(), "{text:?}");
            assert!(layout.width > Pixels::ZERO, "{text:?}");
        }

        Ok(())
    }

    /// cosmic-text sums word widths to get a line's width but accumulates glyph
    /// advances to position glyphs, so a trailing zero-advance glyph (here a
    /// zero-width space) can land a few ulps past the width. When that glyph is a
    /// wrap boundary, the row before it extends past the line's width, and hit
    /// testing in that sliver used to panic (ZED-BW8, ZED-75K, ZED-81Z).
    #[test]
    fn index_for_position_past_line_width() -> Result<()> {
        let text_system = Arc::new(gpui::TextSystem::new(Arc::new(text_system()?)));
        let window_text_system = gpui::WindowTextSystem::new(text_system);
        let text: SharedString = "Warning: this will delete files\u{200b}".into();
        let runs = [gpui::TextRun {
            len: text.len(),
            font: gpui::font("IBM Plex Sans"),
            ..Default::default()
        }];
        let lines =
            window_text_system.shape_text(text, gpui::px(14.), &runs, Some(gpui::px(4.)), None)?;
        let line = &lines[0];
        let width = line.unwrapped_layout.width;
        let boundary_glyph = |row: usize| {
            let boundary = line.wrap_boundaries()[row];
            &line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix]
        };
        let row = (1..line.wrap_boundaries().len())
            .find(|row| boundary_glyph(*row).position.x > width)
            .expect("trailing zero-width space should be a wrap boundary past the line width");
        let row_start_x = f32::from(boundary_glyph(row - 1).position.x);
        let row_end = boundary_glyph(row);

        let mut x = f32::from(width) - row_start_x;
        while x + row_start_x < f32::from(width) {
            x = x.next_up();
        }
        assert!(gpui::px(x + row_start_x) < row_end.position.x);

        let line_height = gpui::px(20.);
        let position = gpui::point(gpui::px(x), line_height * row as f32 + gpui::px(1.));
        assert_eq!(
            line.index_for_position(position, line_height),
            Err(row_end.index)
        );
        Ok(())
    }

    #[test]
    fn paragraph_separator_detection() {
        for separator in SEPARATORS {
            assert!(is_paragraph_separator(*separator), "{separator:?}");
            assert!(contains_paragraph_separator(&format!("a{separator}b")));
        }

        for text in [
            "",
            "plain ascii",
            "\u{05d0}",
            "tab\there",
            "emoji \u{1f600}",
        ] {
            assert!(!contains_paragraph_separator(text), "{text:?}");
        }
    }

    #[test]
    fn font_runs_are_clipped_to_segment() {
        let runs = [
            FontRun {
                len: 3,
                font_id: fid(1),
            },
            FontRun {
                len: 4,
                font_id: fid(2),
            },
        ];

        assert_eq!(clip_font_runs(&runs, 0..7).as_slice(), &runs);
        assert_eq!(
            clip_font_runs(&runs, 2..5).as_slice(),
            &[
                FontRun {
                    len: 1,
                    font_id: fid(1)
                },
                FontRun {
                    len: 2,
                    font_id: fid(2)
                },
            ]
        );
        assert_eq!(
            clip_font_runs(&runs, 3..7).as_slice(),
            &[FontRun {
                len: 4,
                font_id: fid(2)
            }]
        );
        assert!(clip_font_runs(&runs, 5..5).is_empty());
    }

    #[test]
    fn primary_wins_over_current_fallback_when_primary_covers() {
        let primary = fid(0);
        let fb = chain(&[1, 2]);
        let covers = |id: FontId, _: char| id == fid(0) || id == fid(1);
        assert_eq!(
            pick_covering_slot('a', Some(0), primary, &fb, &covers),
            None
        );
    }

    #[test]
    fn primary_preferred_over_fallback_when_both_cover() {
        let primary = fid(0);
        let fb = chain(&[1]);
        let covers = |_: FontId, _: char| true;
        assert_eq!(pick_covering_slot('a', None, primary, &fb, &covers), None);
    }

    #[test]
    fn falls_through_chain_in_order() {
        let primary = fid(0);
        let fb = chain(&[1, 2, 3]);
        // only fallback 2 at index 1 covers.
        let covers = |id: FontId, _: char| id == fid(2);
        assert_eq!(
            pick_covering_slot('字', None, primary, &fb, &covers),
            Some(1)
        );
    }

    #[test]
    fn no_coverage_returns_primary() {
        let primary = fid(0);
        let fb = chain(&[1, 2]);
        let covers = |_: FontId, _: char| false;
        // nothing covers. return `None` so the `cosmic-text` built in script
        // fallback can take over during shaping.
        assert_eq!(
            pick_covering_slot('\u{1F600}', Some(1), primary, &fb, &covers),
            None
        );
    }

    #[test]
    fn empty_chain_always_returns_primary() {
        let primary = fid(0);
        let fb: SmallVec<[(FontId, SharedString); 4]> = SmallVec::new();
        let covers = |_: FontId, _: char| false;
        assert_eq!(pick_covering_slot('a', None, primary, &fb, &covers), None);
    }

    #[test]
    fn slot_font_id_resolution() {
        let primary = fid(7);
        let fb = chain(&[10, 20]);
        assert_eq!(slot_font_id(None, primary, &fb), fid(7));
        assert_eq!(slot_font_id(Some(0), primary, &fb), fid(10));
        assert_eq!(slot_font_id(Some(1), primary, &fb), fid(20));
    }

    #[test]
    fn run_spans_with_no_chain_emit_one_primary_span() {
        let primary = fid(0);
        let fb: SmallVec<[(FontId, SharedString); 4]> = SmallVec::new();
        let covers = |_: FontId, _: char| false;
        let text = "hello";
        let spans = compute_run_spans(text, 0, text.len(), primary, &fb, &covers);
        assert_eq!(spans.as_slice(), &[span(0, text.len(), None, primary)]);
    }

    #[test]
    fn run_spans_use_byte_offsets_for_multibyte_chars() {
        let primary = fid(0);
        let fb = chain(&[1]);
        // primary covers ascii. fallback covers cjk.
        let covers = |id: FontId, ch: char| {
            if id == primary {
                ch.is_ascii()
            } else {
                !ch.is_ascii()
            }
        };
        let text = "a字b";
        let spans = compute_run_spans(text, 0, text.len(), primary, &fb, &covers);
        // '字' is 3 bytes so split is at 1 then 4.
        assert_eq!(
            spans.as_slice(),
            &[
                span(0, 1, None, primary),
                span(1, 4, Some(0), fid(1)),
                span(4, 5, None, primary),
            ]
        );
    }

    #[test]
    fn run_spans_respect_run_offset() {
        let primary = fid(0);
        let fb = chain(&[1]);
        let covers = |id: FontId, ch: char| {
            if id == primary {
                ch.is_ascii()
            } else {
                !ch.is_ascii()
            }
        };
        // outer text has a prefix that is not part of this run.
        let text = "xx字y";
        let run_offset = 2;
        let run_len = text.len() - run_offset;
        let spans = compute_run_spans(text, run_offset, run_len, primary, &fb, &covers);
        assert_eq!(
            spans.as_slice(),
            &[span(2, 5, Some(0), fid(1)), span(5, 6, None, primary)]
        );
    }

    #[test]
    fn run_spans_keep_combining_marks_with_base_in_fallback() {
        let primary = fid(0);
        let fb = chain(&[1]);
        // primary covers ascii only. fallback covers the base char.
        // combining mark must stay in the fallback span even when fallback
        // does not advertise coverage of it.
        let covers = |id: FontId, ch: char| {
            if id == primary {
                ch.is_ascii()
            } else {
                ch == '\u{0905}'
            }
        };
        // \u{0905} devanagari short a + \u{0902} candrabindu mark.
        let text = "\u{0905}\u{0902}";
        let spans = compute_run_spans(text, 0, text.len(), primary, &fb, &covers);
        assert_eq!(spans.as_slice(), &[span(0, text.len(), Some(0), fid(1))]);
    }

    #[test]
    fn run_spans_keep_zwj_inside_emoji_cluster() {
        let primary = fid(0);
        let fb = chain(&[1]);
        // only fallback covers the emoji codepoints. zwj must not split.
        let covers = |id: FontId, ch: char| id == fid(1) && ch != '\u{200D}';
        // family zwj sequence woman zwj girl.
        let text = "\u{1F469}\u{200D}\u{1F467}";
        let spans = compute_run_spans(text, 0, text.len(), primary, &fb, &covers);
        assert_eq!(spans.as_slice(), &[span(0, text.len(), Some(0), fid(1))]);
    }

    #[test]
    fn run_spans_collapse_adjacent_same_slot() {
        let primary = fid(0);
        let fb = chain(&[1]);
        let covers = |id: FontId, ch: char| {
            if id == primary {
                ch.is_ascii()
            } else {
                !ch.is_ascii()
            }
        };
        let text = "字字字";
        let spans = compute_run_spans(text, 0, text.len(), primary, &fb, &covers);
        assert_eq!(spans.as_slice(), &[span(0, text.len(), Some(0), fid(1))]);
    }

    #[test]
    fn run_spans_empty_run_returns_no_spans() {
        let primary = fid(0);
        let fb = chain(&[1]);
        let covers = |_: FontId, _: char| true;
        let spans = compute_run_spans("anything", 3, 0, primary, &fb, &covers);
        assert!(spans.is_empty());
    }

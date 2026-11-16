    use super::*;
    use crate::{
        AppContext as _, Context, FontId, GlyphId, IntoElement, Render, ShapedGlyph, ShapedRun,
        Styled, TestAppContext, TextRun, Underline, canvas, font, hsla,
    };
    use std::rc::Rc;

    /// Helper: build a ShapedLine from glyph descriptors without the platform text system.
    /// Each glyph is described as (byte_index, x_position).
    fn make_shaped_line(
        text: &str,
        glyphs: &[(usize, f32)],
        width: f32,
        decorations: &[DecorationRun],
    ) -> ShapedLine {
        let shaped_glyphs: Vec<ShapedGlyph> = glyphs
            .iter()
            .map(|&(index, x)| ShapedGlyph {
                id: GlyphId(0),
                position: point(px(x), px(0.0)),
                index,
                is_emoji: false,
            })
            .collect();

        ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(width),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![ShapedRun {
                    font_id: FontId(0),
                    glyphs: shaped_glyphs,
                }],
                len: text.len(),
            }),
            text: SharedString::new(text),
            decoration_runs: SmallVec::from(decorations.to_vec()),
        }
    }

    #[test]
    fn test_aligned_line_paint_bounds() {
        let line = make_shaped_line("abcd", &[(0, 0.), (1, 10.), (2, 20.), (3, 30.)], 40., &[]);
        let origin = point(px(10.), px(20.));
        let line_height = px(16.);
        for (wrapped, align_width, expected_edges) in [
            (false, None, [(10., 50.), (10., 50.), (10., 50.)]),
            (true, None, [(10., 50.), (10., 50.), (10., 50.)]),
            (false, Some(40.), [(10., 50.), (10., 50.), (10., 50.)]),
            (true, Some(40.), [(10., 50.), (10., 50.), (10., 50.)]),
            (false, Some(100.), [(10., 50.), (10., 80.), (10., 110.)]),
            (true, Some(100.), [(10., 50.), (10., 70.), (10., 110.)]),
            (false, Some(10.), [(10., 50.), (-5., 50.), (-20., 50.)]),
            (true, Some(10.), [(10., 50.), (5., 50.), (0., 50.)]),
            (false, Some(0.), [(10., 50.), (-10., 50.), (-30., 50.)]),
            (true, Some(0.), [(10., 50.), (0., 50.), (-10., 50.)]),
        ] {
            let boundaries = if wrapped {
                vec![WrapBoundary {
                    run_ix: 0,
                    glyph_ix: 2,
                }]
            } else {
                Vec::new()
            };
            for (align, (left, right)) in [TextAlign::Left, TextAlign::Center, TextAlign::Right]
                .into_iter()
                .zip(expected_edges)
            {
                assert_eq!(
                    line_paint_bounds(
                        origin,
                        &line.layout,
                        line_height,
                        align,
                        align_width.map(px),
                        &boundaries,
                    ),
                    Bounds::new(
                        point(px(left), origin.y),
                        size(px(right - left), px(if wrapped { 32. } else { 16. })),
                    ),
                    "wrapped={wrapped}, align={align:?}, align_width={align_width:?}",
                );
            }
        }
        let empty = make_shaped_line("", &[], 0., &[]);
        for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
            assert_eq!(
                line_paint_bounds(
                    origin,
                    &empty.layout,
                    line_height,
                    align,
                    Some(px(100.)),
                    &[]
                ),
                Bounds::new(origin, size(Pixels::ZERO, line_height)),
            );
        }
    }

    #[gpui::test]
    fn test_underline_handler_matches_default_paint(cx: &mut TestAppContext) {
        test_underline_handler_at_scales(cx, |window, cx| {
            let first_style = UnderlineStyle {
                thickness: px(1.),
                color: Some(hsla(0., 1., 0.5, 1.)),
                wavy: true,
            };
            let last_style = UnderlineStyle {
                color: Some(hsla(0.5, 1., 0.5, 1.)),
                wavy: false,
                ..first_style
            };
            let fallback_style = UnderlineStyle {
                color: Some(black()),
                ..first_style
            };
            let decoration = DecorationRun {
                len: 1,
                color: black(),
                background_color: None,
                underline: Some(first_style),
                strikethrough: None,
            };
            let line = underline_test_line(
                "aébcde",
                &[
                    decoration.clone(),
                    DecorationRun {
                        len: 2,
                        ..decoration.clone()
                    },
                    DecorationRun {
                        underline: None,
                        ..decoration.clone()
                    },
                    DecorationRun {
                        underline: Some(UnderlineStyle {
                            color: None,
                            ..first_style
                        }),
                        ..decoration.clone()
                    },
                    DecorationRun {
                        underline: Some(last_style),
                        ..decoration.clone()
                    },
                    DecorationRun {
                        underline: Some(last_style),
                        ..decoration
                    },
                ],
                false,
                window,
            );
            assert_eq!(line.width(), px(48.));
            for origin_x in [-3.25, 0., 4.25] {
                for (align, align_width, offset) in [
                    (TextAlign::Left, None, 0.),
                    (TextAlign::Center, None, 0.),
                    (TextAlign::Right, None, 0.),
                    (TextAlign::Left, Some(px(96.)), 0.),
                    (TextAlign::Center, Some(px(96.)), 24.),
                    (TextAlign::Right, Some(px(96.)), 48.),
                ] {
                    let origin = point(px(origin_x), px(12.25));
                    let line_height = px(20.);
                    window.next_frame.scene.clear();
                    line.paint(origin, line_height, align, align_width, window, cx)
                        .unwrap();
                    let original = window.next_frame.scene.underlines.clone();
                    window.next_frame.scene.clear();
                    line.layout
                        .paint(
                            origin,
                            line_height,
                            align,
                            align_width,
                            &line.decoration_runs,
                            window,
                            cx,
                        )
                        .unwrap();
                    assert_underline_primitives_eq(&window.next_frame.scene.underlines, &original);

                    window.next_frame.scene.clear();
                    let mut strokes = Vec::new();
                    line.paint_with_underline_handler(
                        origin,
                        line_height,
                        align,
                        align_width,
                        window,
                        cx,
                        |range, origin, width, style, window| {
                            strokes.push((range, origin, width, *style));
                            window.paint_underline(origin, width, style);
                        },
                    )
                    .unwrap();
                    let start = px(origin_x + offset);
                    let y = origin.y + underline_y_offset(line_height, line.ascent, line.descent);
                    assert_eq!(
                        strokes,
                        [
                            (0..3, point(start, y), px(16.), first_style),
                            (4..5, point(start + px(24.), y), px(8.), fallback_style),
                            (5..7, point(start + px(32.), y), px(16.), last_style),
                        ]
                    );
                    assert_underline_primitives_eq(&window.next_frame.scene.underlines, &original);

                    window.next_frame.scene.clear();
                    let mut captured = Vec::new();
                    line.paint_with_underline_handler(
                        origin,
                        line_height,
                        align,
                        align_width,
                        window,
                        cx,
                        |range, origin, width, style, _| {
                            captured.push((range, origin, width, *style))
                        },
                    )
                    .unwrap();
                    assert_eq!(captured, strokes);
                    assert_eq!(window.next_frame.scene.underlines.len(), 0);
                }
            }
            for text in ["", "abc"] {
                let line = underline_test_line(text, &[], false, window);
                let mut calls = 0;
                line.paint_with_underline_handler(
                    point(px(4.), px(10.)),
                    px(20.),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                    |_, _, _, _, _| calls += 1,
                )
                .unwrap();
                assert_eq!(calls, 0);
            }
        });
    }

    #[gpui::test]
    fn test_underline_handler_reports_zero_advance_geometry(cx: &mut TestAppContext) {
        test_underline_handler_at_scales(cx, |window, cx| {
            let first_style = UnderlineStyle {
                thickness: px(1.),
                color: Some(black()),
                wavy: true,
            };
            let last_style = UnderlineStyle {
                wavy: false,
                ..first_style
            };
            let decoration = DecorationRun {
                len: 1,
                color: black(),
                background_color: None,
                underline: Some(first_style),
                strikethrough: None,
            };
            let line = underline_test_line(
                "ab",
                &[
                    decoration.clone(),
                    DecorationRun {
                        underline: Some(last_style),
                        ..decoration
                    },
                ],
                true,
                window,
            );
            let half_width = cx
                .text_system()
                .bounding_box(line.runs[0].font_id, line.font_size)
                .size
                .width
                / 2.;
            let origin = point(px(40.25), px(10.25));
            let line_height = px(20.);
            let y = origin.y + underline_y_offset(line_height, line.ascent, line.descent);
            for (align, offset) in [
                (TextAlign::Left, 0.),
                (TextAlign::Center, 16.),
                (TextAlign::Right, 32.),
            ] {
                window.next_frame.scene.clear();
                line.paint(origin, line_height, align, Some(px(32.)), window, cx)
                    .unwrap();
                let original = window.next_frame.scene.underlines.clone();
                window.next_frame.scene.clear();
                let mut strokes = Vec::new();
                line.paint_with_underline_handler(
                    origin,
                    line_height,
                    align,
                    Some(px(32.)),
                    window,
                    cx,
                    |range, origin, width, style, window| {
                        strokes.push((range, origin, width, *style));
                        window.paint_underline(origin, width, style);
                    },
                )
                .unwrap();
                let end = origin.x + px(offset);
                let start = point(end - half_width, y);
                let width = end - start.x;
                assert_eq!(
                    strokes,
                    [
                        (0..1, start, width, first_style),
                        (1..2, start, width, last_style),
                    ]
                );
                assert_underline_primitives_eq(&window.next_frame.scene.underlines, &original);
            }
        });
    }

    #[gpui::test]
    fn test_underline_handler_matches_wrapped_paint(cx: &mut TestAppContext) {
        test_underline_handler_at_scales(cx, |window, cx| {
            let style = UnderlineStyle {
                thickness: px(1.),
                color: Some(black()),
                wavy: true,
            };
            for zero_advance in [false, true] {
                let line = underline_test_line(
                    "abcd",
                    &[DecorationRun {
                        len: 4,
                        color: black(),
                        background_color: None,
                        underline: Some(style),
                        strikethrough: None,
                    }],
                    zero_advance,
                    window,
                );
                let half_width = cx
                    .text_system()
                    .bounding_box(line.runs[0].font_id, line.font_size)
                    .size
                    .width
                    / 2.;
                let origin = point(px(40.25), px(10.25));
                let line_height = px(20.);
                let y = origin.y + underline_y_offset(line_height, line.ascent, line.descent);
                let wrapped = WrappedLine {
                    layout: Arc::new(WrappedLineLayout {
                        unwrapped_layout: line.layout,
                        wrap_boundaries: SmallVec::from_buf([WrapBoundary {
                            run_ix: 0,
                            glyph_ix: 2,
                        }]),
                        wrap_width: Some(px(16.)),
                    }),
                    text: line.text,
                    decoration_runs: line.decoration_runs.into_vec(),
                };
                window.next_frame.scene.clear();
                wrapped
                    .paint(origin, line_height, TextAlign::Left, None, window, cx)
                    .unwrap();
                let original = window.next_frame.scene.underlines.clone();
                window.next_frame.scene.clear();
                let mut strokes = Vec::new();
                paint_line(
                    origin,
                    &wrapped.unwrapped_layout,
                    line_height,
                    TextAlign::Left,
                    Some(px(16.)),
                    &wrapped.decoration_runs,
                    &wrapped.wrap_boundaries,
                    window,
                    cx,
                    &mut |range, origin, width, style, window| {
                        strokes.push((range, origin, width, *style));
                        window.paint_underline(origin, width, style);
                    },
                )
                .unwrap();
                let (start, width) = if zero_advance {
                    let start = origin.x - half_width;
                    (start, origin.x - start)
                } else {
                    (origin.x, px(16.))
                };
                assert_eq!(
                    strokes,
                    [
                        (0..4, point(start, y), width, style),
                        (0..4, point(start, y + line_height), width, style),
                    ]
                );
                assert_underline_primitives_eq(&window.next_frame.scene.underlines, &original);
            }
        });
    }

    #[test]
    fn test_split_at_invariants() {
        // Split "abcdef" at every possible byte index and verify structural invariants.
        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[],
        );

        for i in 0..=6 {
            let (left, right) = line.split_at(i);

            assert_eq!(
                left.width() + right.width(),
                line.width(),
                "widths must sum at split={i}"
            );
            assert_eq!(
                left.len() + right.len(),
                line.len(),
                "lengths must sum at split={i}"
            );
            assert_eq!(
                format!("{}{}", left.text.as_ref(), right.text.as_ref()),
                "abcdef",
                "text must concatenate at split={i}"
            );
            assert_eq!(left.font_size, line.font_size, "font_size at split={i}");
            assert_eq!(right.ascent, line.ascent, "ascent at split={i}");
            assert_eq!(right.descent, line.descent, "descent at split={i}");
        }

        // Edge: split at 0 produces no left runs, full content on right
        let (left, right) = line.split_at(0);
        assert_eq!(left.runs.len(), 0);
        assert_eq!(right.runs[0].glyphs.len(), 6);

        // Edge: split at end produces full content on left, no right runs
        let (left, right) = line.split_at(6);
        assert_eq!(left.runs[0].glyphs.len(), 6);
        assert_eq!(right.runs.len(), 0);
    }

    #[test]
    fn test_split_at_glyph_rebasing() {
        // Two font runs (simulating a font fallback boundary at byte 3):
        //   run A (FontId 0): glyphs at bytes 0,1,2  positions 0,10,20
        //   run B (FontId 1): glyphs at bytes 3,4,5  positions 30,40,50
        // Successive splits simulate the incremental splitting done during wrap.
        let line = ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(60.0),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![
                    ShapedRun {
                        font_id: FontId(0),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(0.0), px(0.0)),
                                index: 0,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(10.0), px(0.0)),
                                index: 1,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(20.0), px(0.0)),
                                index: 2,
                                is_emoji: false,
                            },
                        ],
                    },
                    ShapedRun {
                        font_id: FontId(1),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(30.0), px(0.0)),
                                index: 3,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(40.0), px(0.0)),
                                index: 4,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(50.0), px(0.0)),
                                index: 5,
                                is_emoji: false,
                            },
                        ],
                    },
                ],
                len: 6,
            }),
            text: "abcdef".into(),
            decoration_runs: SmallVec::new(),
        };

        // First split at byte 2 — mid-run in run A
        let (first, remainder) = line.split_at(2);
        assert_eq!(first.text.as_ref(), "ab");
        assert_eq!(first.runs.len(), 1);
        assert_eq!(first.runs[0].font_id, FontId(0));

        // Remainder "cdef" should have two runs: tail of A (1 glyph) + all of B (3 glyphs)
        assert_eq!(remainder.text.as_ref(), "cdef");
        assert_eq!(remainder.runs.len(), 2);
        assert_eq!(remainder.runs[0].font_id, FontId(0));
        assert_eq!(remainder.runs[0].glyphs.len(), 1);
        assert_eq!(remainder.runs[0].glyphs[0].index, 0);
        assert_eq!(remainder.runs[0].glyphs[0].position.x, px(0.0));
        assert_eq!(remainder.runs[1].font_id, FontId(1));
        assert_eq!(remainder.runs[1].glyphs[0].index, 1);
        assert_eq!(remainder.runs[1].glyphs[0].position.x, px(10.0));

        // Second split at byte 2 within remainder — crosses the run boundary
        let (second, final_part) = remainder.split_at(2);
        assert_eq!(second.text.as_ref(), "cd");
        assert_eq!(final_part.text.as_ref(), "ef");
        assert_eq!(final_part.runs[0].glyphs[0].index, 0);
        assert_eq!(final_part.runs[0].glyphs[0].position.x, px(0.0));

        // Widths must sum across all three pieces
        assert_eq!(
            first.width() + second.width() + final_part.width(),
            line.width()
        );
    }

    #[test]
    fn test_split_at_decorations() {
        // Three decoration runs: red [0..2), green [2..5), blue [5..6).
        // Split at byte 3 — red goes entirely left, green straddles, blue goes entirely right.
        let red = Hsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let green = Hsla {
            h: 0.3,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let blue = Hsla {
            h: 0.6,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };

        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[
                DecorationRun {
                    len: 2,
                    color: red,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
                DecorationRun {
                    len: 3,
                    color: green,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
                DecorationRun {
                    len: 1,
                    color: blue,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
            ],
        );

        let (left, right) = line.split_at(3);

        // Left: red(2) + green(1) — green straddled, left portion has len 1
        assert_eq!(left.decoration_runs.len(), 2);
        assert_eq!(left.decoration_runs[0].len, 2);
        assert_eq!(left.decoration_runs[0].color, red);
        assert_eq!(left.decoration_runs[1].len, 1);
        assert_eq!(left.decoration_runs[1].color, green);

        // Right: green(2) + blue(1) — green straddled, right portion has len 2
        assert_eq!(right.decoration_runs.len(), 2);
        assert_eq!(right.decoration_runs[0].len, 2);
        assert_eq!(right.decoration_runs[0].color, green);
        assert_eq!(right.decoration_runs[1].len, 1);
        assert_eq!(right.decoration_runs[1].color, blue);
    }

    struct UnderlineHandlerTestView(Rc<dyn Fn(&mut Window, &mut App)>);

    impl Render for UnderlineHandlerTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paint = self.0.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    window.with_element_opacity(Some(0.5), |window| paint(window, cx));
                },
            )
            .size_full()
        }
    }

    fn test_underline_handler_at_scales(
        cx: &mut TestAppContext,
        paint: impl Fn(&mut Window, &mut App) + 'static,
    ) {
        let window = cx.add_window(move |_, _| UnderlineHandlerTestView(Rc::new(paint)));
        for scale in [1., 1.25, 1.5, 2., 3.] {
            cx.simulate_window_scale_factor_change(window.into(), scale);
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
    }

    fn underline_test_line(
        text: &str,
        decorations: &[DecorationRun],
        zero_advance: bool,
        window: &Window,
    ) -> ShapedLine {
        let mut line = window.text_system().shape_line(
            SharedString::new(text),
            px(16.),
            &[TextRun {
                len: text.len(),
                font: font(".ZedMono"),
                color: black(),
                ..TextRun::default()
            }],
            None,
        );
        line.decoration_runs = SmallVec::from(decorations.to_vec());
        let layout = &line.layout;
        let mut runs = layout.runs.clone();
        let advance = if zero_advance { px(0.) } else { px(8.) };
        let mut width = px(0.);
        for glyph in runs.iter_mut().flat_map(|run| &mut run.glyphs) {
            glyph.position.x = width;
            width += advance;
        }
        line.layout = Arc::new(LineLayout {
            font_size: layout.font_size,
            width,
            ascent: layout.ascent,
            descent: layout.descent,
            runs,
            len: layout.len,
        });
        line
    }

    fn assert_underline_primitives_eq(actual: &[Underline], expected: &[Underline]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.bounds, expected.bounds);
            assert_eq!(actual.content_mask, expected.content_mask);
            assert_eq!(actual.color, expected.color);
            assert_eq!(actual.thickness, expected.thickness);
            assert_eq!(actual.wavy, expected.wavy);
            assert_eq!(actual.order, expected.order);
            assert_eq!(actual.pad, expected.pad);
        }
    }

    #[test]
    fn test_cursor_preserves_shaping_metadata_across_runs() {
        let line = ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(50.0),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![
                    ShapedRun {
                        font_id: FontId(3),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(11),
                                position: point(px(0.0), px(1.0)),
                                index: 0,
                                is_emoji: true,
                            },
                            ShapedGlyph {
                                id: GlyphId(12),
                                position: point(px(17.0), px(1.0)),
                                index: 1,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(13),
                                position: point(px(19.0), px(-1.0)),
                                index: 1,
                                is_emoji: false,
                            },
                        ],
                    },
                    ShapedRun {
                        font_id: FontId(8),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(21),
                                position: point(px(25.0), px(1.0)),
                                index: 5,
                                is_emoji: true,
                            },
                            ShapedGlyph {
                                id: GlyphId(22),
                                position: point(px(41.0), px(1.0)),
                                index: 7,
                                is_emoji: false,
                            },
                        ],
                    },
                ],
                len: 10,
            }),
            text: "a😀bcdef".into(),
            decoration_runs: SmallVec::new(),
        };
        let mut cursor = line.cursor();
        let first = cursor.take_until(5);
        assert_eq!(first.text.as_ref(), "a😀");
        assert_eq!(first.runs[0].font_id, FontId(3));
        assert_eq!(first.runs[0].glyphs[0].id, GlyphId(11));
        assert!(first.runs[0].glyphs[0].is_emoji);
        assert_eq!(first.runs[0].glyphs[1].index, 1);
        assert_eq!(first.runs[0].glyphs[1].position, point(px(17.0), px(1.0)));
        assert_eq!(first.runs[0].glyphs.len(), 3);
        assert_eq!(first.runs[0].glyphs[2].index, 1);
        assert_eq!(first.runs[0].glyphs[2].position, point(px(19.0), px(-1.0)));
        assert_eq!(cursor.x_offset(), px(25.0));

        let second = cursor.take_until(7);
        assert_eq!(second.text.as_ref(), "bc");
        assert_eq!(second.runs[0].font_id, FontId(8));
        assert_eq!(second.runs[0].glyphs[0].id, GlyphId(21));
        assert_eq!(second.runs[0].glyphs[0].index, 0);
        assert_eq!(second.runs[0].glyphs[0].position, point(px(0.0), px(1.0)));
        assert_eq!(cursor.x_offset(), px(41.0));

        let final_part = cursor.take_until(10);
        assert_eq!(final_part.text.as_ref(), "def");
        assert_eq!(final_part.runs[0].font_id, FontId(8));
        assert_eq!(final_part.runs[0].glyphs[0].id, GlyphId(22));
        assert_eq!(final_part.runs[0].glyphs[0].index, 0);
        assert_eq!(
            final_part.runs[0].glyphs[0].position,
            point(px(0.0), px(1.0))
        );
    }

    #[test]
    fn test_cursor_preserves_existing_visual_order_splitting() {
        let line = make_shaped_line("abc", &[(0, 0.0), (2, 10.0), (1, 20.0)], 30.0, &[]);
        let mut cursor = line.cursor();
        let mut remainder = line.clone();
        let mut previous_boundary = 0;
        for boundary in [0, 1, 2, 3] {
            let (expected, rest) = remainder.split_at(boundary - previous_boundary);
            let actual = cursor.take_until(boundary);
            assert_eq!(actual.text, expected.text);
            assert_eq!(actual.width(), expected.width());
            assert_eq!(actual.runs.len(), expected.runs.len());
            for (actual, expected) in actual.runs.iter().zip(&expected.runs) {
                assert_eq!(actual.font_id, expected.font_id);
                assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                for (actual, expected) in actual.glyphs.iter().zip(&expected.glyphs) {
                    assert_eq!(actual.id, expected.id);
                    assert_eq!(actual.index, expected.index);
                    assert_eq!(actual.position, expected.position);
                }
            }
            assert_eq!(cursor.x_offset(), line.x_for_index(boundary));
            remainder = rest;
            previous_boundary = boundary;
        }
    }

    #[test]
    fn test_cursor_partitions_one_decoration_across_three_chunks() {
        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[DecorationRun {
                len: 6,
                color: Hsla {
                    h: 0.2,
                    s: 0.4,
                    l: 0.6,
                    a: 1.0,
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
        );
        let mut cursor = line.cursor();
        assert_eq!(cursor.take_until(2).decoration_runs[0].len, 2);
        assert_eq!(cursor.take_until(4).decoration_runs[0].len, 2);
        assert_eq!(cursor.take_until(6).decoration_runs[0].len, 2);
    }

    #[test]
    fn test_cursor_matches_successive_splits_at_ordered_boundaries() {
        let decorations: Vec<_> = [2, 0, 3, 1]
            .into_iter()
            .map(|len| DecorationRun {
                len,
                color: Hsla {
                    h: len as f32 / 10.0,
                    s: 0.5,
                    l: 0.5,
                    a: 1.0,
                },
                background_color: Some(black()),
                underline: None,
                strikethrough: None,
            })
            .collect();
        let line = make_shaped_line(
            "abcdef",
            &[(0, 5.0), (0, 5.0), (2, 15.0), (4, 25.0), (5, 35.0)],
            45.0,
            &decorations,
        );
        for first in 0..=line.len() {
            for second in first..=line.len() {
                let mut cursor = line.cursor();
                let mut remainder = line.clone();
                let mut previous_boundary = 0;
                let mut total_width = px(0.0);
                let mut text = String::new();
                for boundary in [first, second, line.len(), line.len()] {
                    let (expected, rest) = remainder.split_at(boundary - previous_boundary);
                    let actual = cursor.take_until(boundary);
                    assert_eq!(actual.text, expected.text);
                    assert_eq!(actual.len(), expected.len());
                    assert_eq!(actual.width(), expected.width());
                    assert_eq!(actual.runs.len(), expected.runs.len());
                    for (actual, expected) in actual.runs.iter().zip(&expected.runs) {
                        assert_eq!(actual.font_id, expected.font_id);
                        assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                        for (actual, expected) in actual.glyphs.iter().zip(&expected.glyphs) {
                            assert_eq!(actual.id, expected.id);
                            assert_eq!(actual.index, expected.index);
                            assert_eq!(actual.position, expected.position);
                        }
                    }
                    assert_eq!(actual.decoration_runs.len(), expected.decoration_runs.len());
                    for (actual, expected) in
                        actual.decoration_runs.iter().zip(&expected.decoration_runs)
                    {
                        assert_eq!(actual.len, expected.len);
                        assert_eq!(actual.color, expected.color);
                        assert_eq!(actual.background_color, expected.background_color);
                    }
                    total_width += actual.width();
                    text.push_str(&actual.text);
                    remainder = rest;
                    previous_boundary = boundary;
                }
                assert_eq!(total_width, line.width());
                assert_eq!(text, line.text.as_ref());
            }
        }
    }

    #[test]
    fn test_cursor_empty_chunks_and_repeated_boundaries() {
        let line = make_shaped_line("ab", &[(0, 5.0), (1, 15.0)], 20.0, &[]);
        let mut cursor = line.cursor();
        assert_eq!(cursor.take_until(0).text.as_ref(), "");
        assert_eq!(cursor.take_until(0).text.as_ref(), "");
        assert_eq!(cursor.take_until(1).text.as_ref(), "a");
        assert_eq!(cursor.take_until(2).text.as_ref(), "b");
        assert_eq!(cursor.take_until(2).text.as_ref(), "");
        let empty = make_shaped_line("", &[], 0.0, &[]);
        let piece = empty.cursor().take_until(0);
        assert!(piece.text.is_empty());
        assert!(piece.runs.is_empty());
        assert_eq!(piece.width(), px(0.0));
    }

    #[test]
    fn test_cursor_rejects_invalid_boundaries() {
        let line = make_shaped_line("é", &[(0, 0.0)], 10.0, &[]);
        let mut cursor = line.cursor();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(1);
            }))
            .is_err()
        );
        let mut cursor = line.cursor();
        cursor.take_until(2);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(0);
            }))
            .is_err()
        );
        let mut cursor = line.cursor();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(3);
            }))
            .is_err()
        );
    }

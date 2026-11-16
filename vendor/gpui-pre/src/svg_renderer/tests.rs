    use super::*;
    use usvg::fontdb::{Database, Family, Query};

    const IBM_PLEX_REGULAR: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
    const LILEX_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");

    #[test]
    fn renders_parsed_svg_at_requested_size() -> Result<()> {
        let renderer = SvgRenderer::new(Arc::new(()));
        let svg = renderer.parse_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24pt" height="12pt"></svg>"#,
        )?;
        let requested_size = Size::new(DevicePixels(24), DevicePixels(12));
        let image = renderer.render_parsed(&svg, SvgSize::ExactSize(requested_size))?;

        assert_eq!(image.size(0), requested_size);
        Ok(())
    }

    #[test]
    fn preserves_aspect_ratio_for_width_constrained_size() -> Result<()> {
        let renderer = SvgRenderer::new(Arc::new(()));
        let svg = renderer.parse_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24pt" height="12pt"></svg>"#,
        )?;
        let image = renderer.render_parsed(
            &svg,
            SvgSize::Size(Size::new(DevicePixels(24), DevicePixels(24))),
        )?;

        assert_eq!(image.size(0), Size::new(DevicePixels(24), DevicePixels(12)));
        Ok(())
    }

    fn db_with_bundled_fonts() -> Database {
        let mut db = Database::new();
        db.load_font_data(IBM_PLEX_REGULAR.to_vec());
        db.load_font_data(LILEX_REGULAR.to_vec());
        db
    }

    #[test]
    fn text_with_split_glyph_clusters_in_mixed_fonts_does_not_panic() {
        let mut db = Database::new();
        db.load_font_data(IBM_PLEX_REGULAR.to_vec());
        db.load_font_data(LILEX_REGULAR.to_vec());
        let options = usvg::Options {
            fontdb: std::sync::Arc::new(db),
            ..Default::default()
        };

        // A base letter followed by a stack of combining marks. Under HarfBuzz's
        // default cluster merging every mark glyph shares the base's byte index,
        // which is the "glyph splitting" condition that triggered the panic. The
        // chunk must use two different fonts so the buggy merge path runs.
        let zalgo = "e\u{0301}\u{0302}\u{0303}\u{0304}\u{0306}\u{0307}\u{0308}\u{030a}";
        let svg = format!(
            r#"<svg viewBox="0 0 200 200" xmlns="http://www.w3.org/2000/svg"><text font-family="Lilex" font-size="32">{zalgo}<tspan font-family="IBM Plex Sans">{zalgo}</tspan></text></svg>"#
        );

        // Before the fix this aborts via panic with a message like
        // "removal index (is 5) should be < len (is 5)".
        usvg::Tree::from_data(svg.as_bytes(), &options)
            .expect("SVG with mixed-font text should parse");
    }

    #[test]
    fn test_is_emoji_presentation() {
        let cases = [
            ("a", false),
            ("Z", false),
            ("1", false),
            ("#", false),
            ("*", false),
            ("漢", false),
            ("中", false),
            ("カ", false),
            ("©", false),
            ("♥", false),
            ("😀", true),
            ("✅", true),
            ("🇺🇸", true),
            // SVG fallback is not cluster-aware yet
            ("©️", false),
            ("♥️", false),
            ("1️⃣", false),
        ];
        for (s, expected) in cases {
            assert_eq!(
                is_emoji_presentation(s.chars().next().unwrap()),
                expected,
                "for char {:?}",
                s
            );
        }
    }

    #[test]
    fn fix_generic_font_families_sets_all_families() {
        let mut db = db_with_bundled_fonts();
        fix_generic_font_families(&mut db);

        let families = [
            Family::SansSerif,
            Family::Serif,
            Family::Monospace,
            Family::Cursive,
            Family::Fantasy,
        ];

        for family in families {
            let query = Query {
                families: &[family],
                ..Default::default()
            };
            assert!(
                db.query(&query).is_some(),
                "Expected generic family {family:?} to resolve after fix_generic_font_families"
            );
        }
    }

    #[test]
    fn test_select_emoji_font_skips_family_without_glyph() {
        let mut db = db_with_bundled_fonts();

        let ibm_plex_sans = db
            .query(&usvg::fontdb::Query {
                families: &[usvg::fontdb::Family::Name("IBM Plex Sans")],
                weight: usvg::fontdb::Weight(400),
                stretch: usvg::fontdb::Stretch::Normal,
                style: usvg::fontdb::Style::Normal,
            })
            .unwrap();
        let lilex = db
            .query(&usvg::fontdb::Query {
                families: &[usvg::fontdb::Family::Name("Lilex")],
                weight: usvg::fontdb::Weight(400),
                stretch: usvg::fontdb::Stretch::Normal,
                style: usvg::fontdb::Style::Normal,
            })
            .unwrap();
        let selected = select_emoji_font('│', &[], &db, &["IBM Plex Sans", "Lilex"]).unwrap();

        assert_eq!(selected, lilex);
        assert!(!font_has_char(&db, ibm_plex_sans, '│'));
        assert!(font_has_char(&db, selected, '│'));
    }

    #[test]
    fn fix_generic_font_families_monospace_resolves_to_lilex() {
        let mut db = db_with_bundled_fonts();
        fix_generic_font_families(&mut db);

        let query = Query {
            families: &[Family::Monospace],
            ..Default::default()
        };
        let id = db.query(&query).expect("Monospace should resolve");
        let face = db.face(id).expect("Face should exist");
        assert!(
            face.families.iter().any(|(name, _)| name.contains("Lilex")),
            "Monospace should map to Lilex, got {:?}",
            face.families
        );
    }

    use super::*;
    use crate::{
        AnyWindowHandle, Background, BorderStyle, Bounds, ContentMask, Corners, Edges, Pixels,
        ScaledPixels, Size, TestAppContext, size,
    };
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    const ELEMENT_COUNT: usize = 64;
    const MUTATION_COUNT: usize = 32;
    const WINDOW_SIZE: Size<Pixels> = size(px(1200.0), px(900.0));

    #[derive(Debug, PartialEq)]
    struct SceneSnapshot {
        quads: Vec<QuadSnapshot>,
        element_ids: BTreeSet<u64>,
    }

    impl SceneSnapshot {
        fn from_window(window: &Window) -> Self {
            Self {
                quads: window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .map(QuadSnapshot::from)
                    .collect(),
                element_ids: window
                    .rendered_frame
                    .element_states
                    .keys()
                    .filter_map(|(global_id, _)| {
                        let ElementId::NamedInteger(name, element_id) = global_id.last()? else {
                            return None;
                        };
                        (name.as_ref() == "randomized-element").then_some(*element_id)
                    })
                    .collect(),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct QuadSnapshot {
        order: u32,
        border_style: BorderStyle,
        bounds: Bounds<ScaledPixels>,
        content_mask: ContentMask<ScaledPixels>,
        background: Background,
        border_color: Hsla,
        corner_radii: Corners<ScaledPixels>,
        border_widths: Edges<ScaledPixels>,
    }

    impl From<&crate::Quad> for QuadSnapshot {
        fn from(quad: &crate::Quad) -> Self {
            Self {
                order: quad.order,
                border_style: quad.border_style,
                bounds: quad.bounds,
                content_mask: quad.content_mask,
                background: quad.background,
                border_color: quad.border_color,
                corner_radii: quad.corner_radii,
                border_widths: quad.border_widths,
            }
        }
    }

    #[gpui::test]
    fn localized_recoloring_notifies_one_entity_until_its_subtree_is_exhausted(
        cx: &mut TestAppContext,
    ) {
        // Wide: every element is a root child, so every entity's subtree is exactly one
        // element and localized change of k elements must touch k boundaries only when it
        // spills; a mixed tree has larger subtrees to stay inside.
        let config = RandomizedElementTreeConfig::new(11, 200)
            .with_topology(RandomizedElementTreeTopology::Mixed)
            .with_entity_density(0.1);
        let window = cx.open_window(WINDOW_SIZE, move |_, cx| {
            RandomizedElementTree::new_with_config(config, cx)
        });
        let root = window.root(cx).expect("window has a root");
        let (localized, spread) = root.update(cx, |tree, cx| {
            let localized = tree.recolor_children(20, ChangeLocality::Localized, cx);
            let spread = tree.recolor_children(20, ChangeLocality::Spread, cx);
            (localized, spread)
        });
        assert_eq!(localized.recolored, 20);
        assert_eq!(spread.recolored, 20);
        assert!(
            localized.notified <= spread.notified,
            "localized change touches no more boundaries than spread change: {localized:?} vs {spread:?}"
        );
        assert!(localized.notified >= 1);
    }

    #[test]
    fn bounds_sample_within_range_and_reproduce_from_the_seed() {
        let bounds = RandomizedElementTreeBounds::new(32..=256)
            .with_topologies([
                RandomizedElementTreeTopology::Deep,
                RandomizedElementTreeTopology::Narrow,
            ])
            .with_entity_density(0.1..=0.5)
            .with_handler_density(0.25..=0.25);
        let mut topologies = BTreeSet::new();
        let sample = |seed| bounds.sample(&mut StdRng::seed_from_u64(seed));
        for seed in 0..64 {
            let config = sample(seed);
            assert!((32..=256).contains(&config.element_count()), "{config:?}");
            assert!(config.topology().is_tall(), "{config:?}");
            assert!((0.1..=0.5).contains(&config.entity_density()), "{config:?}");
            assert_eq!(config.handler_density(), 0.25);
            assert_eq!(sample(seed), config, "a seed draws the same config twice");
            topologies.insert(format!("{:?}", config.topology()));
        }
        assert_eq!(topologies.len(), 2, "64 seeds pick both allowed topologies");
        assert_ne!(sample(0), sample(1));
    }

    #[test]
    fn generated_topologies_include_wide_deep_narrow_and_empty_trees() {
        let wide = RandomizedElementTree::new(0, ELEMENT_COUNT).snapshot();
        assert_eq!(wide.children.len(), ELEMENT_COUNT);
        assert_eq!(wide.max_depth(), 1);

        let deep = RandomizedElementTree::new(1, ELEMENT_COUNT).snapshot();
        assert_eq!(deep.children.len(), 1);
        assert_eq!(deep.max_depth(), ELEMENT_COUNT);

        let narrow = RandomizedElementTree::new(3, ELEMENT_COUNT).snapshot();
        assert_eq!(narrow.descendant_count(), ELEMENT_COUNT);
        assert!(narrow.max_depth() > 1);

        let empty = RandomizedElementTree::new(2, 0).snapshot();
        assert_eq!(empty.descendant_count(), 0);
        assert_eq!(empty.max_depth(), 0);
    }

    #[test]
    fn randomized_element_ids_are_unique_and_stable() {
        let mut tree = RandomizedElementTree::new(0, ELEMENT_COUNT);
        let initial_ids = tree.snapshot().element_ids();
        assert_eq!(initial_ids.len(), ELEMENT_COUNT);
        assert_eq!(
            initial_ids.iter().copied().collect::<BTreeSet<_>>().len(),
            ELEMENT_COUNT
        );

        for mutation in [
            RandomizedChildMutation::Color,
            RandomizedChildMutation::Opacity,
            RandomizedChildMutation::Border,
            RandomizedChildMutation::Bounds,
            RandomizedChildMutation::Clipping,
            RandomizedChildMutation::Visibility,
        ] {
            tree.mutate_random_child(mutation);
            assert_eq!(tree.snapshot().element_ids(), initial_ids);
        }

        tree.reorder_child();
        let reordered_ids = tree.snapshot().element_ids();
        assert_eq!(
            reordered_ids.iter().copied().collect::<BTreeSet<_>>(),
            initial_ids.iter().copied().collect()
        );

        tree.insert_child();
        let inserted_ids = tree.snapshot().element_ids();
        let inserted_id_set = inserted_ids.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(inserted_ids.len(), ELEMENT_COUNT + 1);
        assert!(initial_ids.iter().all(|id| inserted_id_set.contains(id)));

        tree.remove_child();
        let remaining_ids = tree.snapshot().element_ids();
        assert_eq!(
            remaining_ids.iter().copied().collect::<BTreeSet<_>>().len(),
            remaining_ids.len()
        );
        assert!(remaining_ids.iter().all(|id| inserted_id_set.contains(id)));
    }

    #[gpui::test]
    fn randomized_entities_and_work_counters_support_incremental_benchmarks(
        cx: &mut TestAppContext,
    ) {
        let config = RandomizedElementTreeConfig::new(0, 16)
            .with_topology(RandomizedElementTreeTopology::Wide)
            .with_entity_density(1.0)
            .with_handler_density(1.0);
        let window = cx.open_window(WINDOW_SIZE, move |_, cx| {
            RandomizedElementTree::new_with_config(config, cx)
        });
        let root = window
            .root(cx)
            .expect("randomized tree window should have a root");
        let window = AnyWindowHandle::from(window);

        draw_window(window, cx);
        let initial_counters = root.read_with(cx, |tree, _| tree.work_counters());
        assert!(initial_counters.root_render_count() >= 1);
        assert!(initial_counters.entity_render_count() >= 16);
        assert!(initial_counters.element_render_count() >= 16);
        assert!(initial_counters.handler_registration_count() >= 32);

        let mutation = root.update(cx, |tree, cx| {
            tree.reset_work_counters();
            tree.apply_mutation(RandomizedElementTreeMutationKind::ChildColor, cx)
        });
        let RandomizedElementTreeMutation::ChildColor { element_id } = mutation else {
            panic!("a nonempty tree should apply the requested child-color mutation");
        };
        draw_window(window, cx);

        let incremental_counters = root.read_with(cx, |tree, _| tree.work_counters());
        assert!(incremental_counters.root_render_count() >= 1);
        assert!(incremental_counters.entity_render_count() >= 1);
        assert!(incremental_counters.element_render_count() >= 1);
        assert!(incremental_counters.handler_registration_count() >= 2);
        assert!(
            incremental_counters
                .rendered_entity_ids()
                .contains(&element_id)
        );

        cx.update_window(window, |_, window, _| window.remove_window())
            .expect("randomized tree window should remain open until the case completes");
    }

    #[gpui::property_test(config = ProptestConfig {
        cases: 4,
        ..Default::default()
    })]
    fn randomized_element_tree_matches_full_render(cx: &mut TestAppContext, seed: u64) {
        for topology in 0..4 {
            let topology_seed = (seed & !3) | topology;
            let config = RandomizedElementTreeConfig::new(topology_seed, ELEMENT_COUNT)
                .with_entity_density(0.2)
                .with_handler_density(0.2);
            run_render_equivalence_case(cx, config);
        }
        run_render_equivalence_case(
            cx,
            RandomizedElementTreeConfig::new(seed, 0)
                .with_entity_density(0.2)
                .with_handler_density(0.2),
        );
    }

    fn run_render_equivalence_case(cx: &mut TestAppContext, config: RandomizedElementTreeConfig) {
        let persistent_window = cx.open_window(WINDOW_SIZE, move |_, cx| {
            RandomizedElementTree::new_with_config(config, cx)
        });
        let persistent_root = persistent_window
            .root(cx)
            .expect("randomized tree window should have a root");
        let persistent_window = AnyWindowHandle::from(persistent_window);

        assert_matches_reference(&persistent_root, persistent_window, cx, config, None);

        for mutation_index in 0..MUTATION_COUNT {
            let mutation = persistent_root.update(cx, |tree, cx| match mutation_index {
                0 => tree.apply_mutation(RandomizedElementTreeMutationKind::Insert, cx),
                1 => tree.apply_mutation(RandomizedElementTreeMutationKind::Remove, cx),
                _ => tree.apply_random_mutation(cx),
            });

            assert_matches_reference(
                &persistent_root,
                persistent_window,
                cx,
                config,
                Some((mutation_index, mutation)),
            );
        }

        cx.update_window(persistent_window, |_, window, _| window.remove_window())
            .expect("persistent window should remain open until the case completes");
    }

    fn assert_matches_reference(
        persistent_root: &crate::Entity<RandomizedElementTree>,
        persistent_window: AnyWindowHandle,
        cx: &mut TestAppContext,
        config: RandomizedElementTreeConfig,
        mutation: Option<(usize, RandomizedElementTreeMutation)>,
    ) {
        let snapshot = persistent_root.read_with(cx, |tree, _| tree.snapshot());
        let expected_element_ids = snapshot.element_ids().into_iter().collect::<BTreeSet<_>>();
        let persistent_scene = draw_window(persistent_window, cx);
        assert_eq!(
            persistent_scene.element_ids, expected_element_ids,
            "persistent frame element IDs diverged from the model for config {config:?}, after \
             mutation {mutation:?}"
        );
        let reference_scene = draw_fresh_reference(snapshot, cx);

        assert_eq!(
            persistent_scene, reference_scene,
            "rendered scene diverged for config {config:?}, after mutation {mutation:?}"
        );
    }

    fn draw_fresh_reference(
        snapshot: RandomizedElementTreeSnapshot,
        cx: &mut TestAppContext,
    ) -> SceneSnapshot {
        let reference_window = cx.open_window(WINDOW_SIZE, move |_, cx| {
            RandomizedElementTree::from_snapshot_with_entities(snapshot, cx)
        });
        let reference_window = AnyWindowHandle::from(reference_window);
        let scene = draw_window(reference_window, cx);
        cx.update_window(reference_window, |_, window, _| window.remove_window())
            .expect("reference window should remain open until its frame is captured");
        scene
    }

    fn draw_window(window: AnyWindowHandle, cx: &mut TestAppContext) -> SceneSnapshot {
        cx.update_window(window, |_, window, cx| {
            let arena_clear_needed = window.draw(cx);
            let scene = SceneSnapshot::from_window(window);
            arena_clear_needed.clear(cx);
            scene
        })
        .expect("randomized element tree window should remain open while drawing")
    }

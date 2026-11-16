use super::*;
use gpui_kit::{
	Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, px, size,
};

#[test]
fn flatten_tree_honors_expansion() {
	let roots = vec![
		ProjectEntry::new(1, "Footage", true),
		ProjectEntry::new(2, "Notes.md", false),
	];
	let expanded = HashSet::from([1u64]);
	let children = |id: u64| -> Vec<ProjectEntry> {
		if id == 1 {
			vec![
				ProjectEntry::new(10, "a.mov", false),
				ProjectEntry::new(11, "b.mov", false),
			]
		} else {
			Vec::new()
		}
	};
	let rows = flatten_tree(&roots, &expanded, children);
	assert_eq!(
		rows.iter().map(|(e, d)| (e.id, *d)).collect::<Vec<_>>(),
		vec![(1, 0), (10, 1), (11, 1), (2, 0)]
	);

	let collapsed = flatten_tree(&roots, &HashSet::new(), children);
	assert_eq!(
		collapsed.iter().map(|(e, _)| e.id).collect::<Vec<_>>(),
		vec![1, 2]
	);
}

#[test]
fn entry_thumbnail_optional() {
	let plain = ProjectEntry::new(1, "x.mov", false);
	assert_eq!(plain.thumbnail, None);
	let with = plain.clone().with_thumbnail("thumbs/x.png");
	assert_eq!(with.thumbnail.as_deref(), Some("thumbs/x.png"));
}

// --- view interaction tests ---

struct MockData;
impl ProjectDataSource for MockData {
	fn roots(&self) -> Vec<ProjectEntry> {
		vec![
			ProjectEntry::new(1, "Footage", true),
			ProjectEntry::new(2, "Notes.md", false),
		]
	}
	fn children(&self, parent_id: u64) -> Vec<ProjectEntry> {
		if parent_id == 1 {
			vec![ProjectEntry::new(10, "a.mov", false)]
		} else {
			Vec::new()
		}
	}
}

struct Host {
	explorer: Entity<ProjectExplorer<MockData>>,
	events: Vec<ProjectExplorerEvent>,
}
impl Render for Host {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.explorer.clone())
	}
}

fn make_explorer(cx: &mut TestAppContext) -> (&'static mut VisualTestContext, Entity<Host>) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(300.0), px(240.0)), |window, cx| {
		let data = cx.new(|_| MockData);
		let explorer = cx.new(|cx| ProjectExplorer::new(1, data, window, cx));
		let host = Host {
			explorer,
			events: Vec::new(),
		};
		cx.subscribe(
			&host.explorer,
			|host: &mut Host,
			 _e: Entity<ProjectExplorer<MockData>>,
			 event: &ProjectExplorerEvent,
			 _cx: &mut Context<Host>| {
				host.events.push(event.clone());
			},
		)
		.detach();
		host
	});
	cx.run_until_parked();
	let host = window.root(cx).unwrap();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	(cx, host)
}

#[gpui_kit::test]
async fn ctrl_and_shift_click_build_a_media_selection(cx: &mut TestAppContext) {
	let (cx, host) = make_explorer(cx);
	cx.update(|_, cx| {
		host.update(cx, |host, cx| {
			host.explorer.update(cx, |explorer, _cx| {
				let ids = [2u64, 10];
				explorer.select_click(2, &ids, &Modifiers::default());
				explorer.select_click(
					10,
					&ids,
					&Modifiers {
						control: true,
						..Default::default()
					},
				);
				assert_eq!(explorer.selected, BTreeSet::from([2, 10]));
				explorer.select_click(
					2,
					&ids,
					&Modifiers {
						shift: true,
						..Default::default()
					},
				);
				assert_eq!(explorer.selected, BTreeSet::from([2, 10]));
				explorer.select_click(2, &ids, &Modifiers::default());
				assert_eq!(explorer.selected, BTreeSet::from([2]));
			});
		});
	});
}

#[gpui_kit::test]
async fn clicking_a_folder_expands_it(cx: &mut TestAppContext) {
	let (cx, _host) = make_explorer(cx);
	// The first row (Footage) is at the top of the content area.
	let first = cx
		.debug_bounds("gpui-widgets-explorer-entry-1")
		.expect("first row rendered");
	cx.simulate_click(first.center(), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// The child a.mov should now be visible.
	let child = cx.debug_bounds("gpui-widgets-explorer-entry-10");
	assert!(child.is_some(), "expanded folder should reveal its child");
}

#[gpui_kit::test]
async fn double_clicking_a_file_emits_open_request(cx: &mut TestAppContext) {
	let (cx, host) = make_explorer(cx);
	// Expand Footage first.
	let first = cx
		.debug_bounds("gpui-widgets-explorer-entry-1")
		.expect("first row rendered");
	cx.simulate_click(first.center(), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Double-click a.mov.
	let child = cx
		.debug_bounds("gpui-widgets-explorer-entry-10")
		.expect("child row rendered");
	let modifiers = Modifiers::none();
	cx.simulate_event(MouseDownEvent {
		position: child.center(),
		modifiers,
		button: MouseButton::Left,
		click_count: 2,
		first_mouse: false,
	});
	cx.simulate_event(gpui_kit::MouseUpEvent {
		position: child.center(),
		modifiers,
		button: MouseButton::Left,
		click_count: 2,
	});
	cx.run_until_parked();

	let opened = cx.read(|app| {
		host.read(app)
			.events
			.iter()
			.any(|e| matches!(e, ProjectExplorerEvent::OpenRequested { id: 10, .. }))
	});
	assert!(opened, "expected an OpenRequested for a.mov");
}

#[gpui_kit::test]
async fn switching_to_icons_emits_view_changed(cx: &mut TestAppContext) {
	let (cx, host) = make_explorer(cx);
	let toggle = cx
		.debug_bounds("gpui-widgets-explorer-icons")
		.expect("icons toggle rendered");
	cx.simulate_click(toggle.center(), Modifiers::none());
	cx.run_until_parked();

	let changed = cx.read(|app| {
		host.read(app).events.iter().any(|e| {
			matches!(
				e,
				ProjectExplorerEvent::ViewChanged {
					view: ExplorerView::Icons,
					..
				}
			)
		})
	});
	assert!(changed, "expected a ViewChanged(Icons) event");
}

/// The icon grid shows the tree's *visible* set: root-level media
/// entries (leaves) appear directly, folder roots are expanded to their
/// children, and the folder root itself is not shown. Regression test
/// for the old `children(root)` loop, which skipped root-level footage
/// entirely.
#[gpui_kit::test]
async fn icons_view_lists_root_files_and_folder_children(cx: &mut TestAppContext) {
	let (cx, _host) = make_explorer(cx);
	let toggle = cx
		.debug_bounds("gpui-widgets-explorer-icons")
		.expect("icons toggle rendered");
	cx.simulate_click(toggle.center(), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// MockData roots: folder 1 "Footage" (child 10 "a.mov") and the
	// root-level file 2 "Notes.md".
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-2").is_some(),
		"a root-level media entry must show as an icon"
	);
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-10").is_some(),
		"a folder child must show as an icon"
	);
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-icon-1").is_none(),
		"the folder root itself has no icon"
	);
}

/// A data source mixing a thumbnailed footage entry with a plain one (the
/// real engine attaches thumbnail paths via
/// [`ProjectEntry::with_thumbnail`]).
struct ThumbnailData;
impl ProjectDataSource for ThumbnailData {
	fn roots(&self) -> Vec<ProjectEntry> {
		vec![
			ProjectEntry::new(2, "clip.mov", false)
				.with_thumbnail("/tmp/oak-thumbnails/thumb2.png"),
			ProjectEntry::new(3, "notes.md", false),
		]
	}
	fn children(&self, _parent_id: u64) -> Vec<ProjectEntry> {
		Vec::new()
	}
}

/// Root view hosting the thumbnailed explorer (no event recording needed
/// for the structure assertion).
struct ThumbnailHost {
	explorer: Entity<ProjectExplorer<ThumbnailData>>,
}
impl Render for ThumbnailHost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.explorer.clone())
	}
}

/// The icon grid renders a real `img` element for entries carrying a
/// thumbnail asset path and the letter placeholder for entries without
/// one — the widget half of the footage-thumbnail chain.
#[gpui_kit::test]
async fn icon_view_renders_thumbnail_img_or_placeholder(cx: &mut TestAppContext) {
	cx.update(|cx| cx.init_colors());
	let window = cx.open_window(size(px(300.0), px(240.0)), |window, cx| {
		let data = cx.new(|_| ThumbnailData);
		let explorer = cx.new(|cx| ProjectExplorer::new(1, data, window, cx));
		ThumbnailHost { explorer }
	});
	cx.run_until_parked();
	let _host = window.root(cx).unwrap();
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();

	// Switch to the icon grid.
	let toggle = cx
		.debug_bounds("gpui-widgets-explorer-icons")
		.expect("icons toggle rendered");
	cx.simulate_click(toggle.center(), Modifiers::none());
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	assert!(
		cx.debug_bounds("gpui-widgets-explorer-thumb-2").is_some(),
		"a thumbnailed entry must render an img element"
	);
	assert!(
		cx.debug_bounds("gpui-widgets-explorer-thumb-placeholder-3")
			.is_some(),
		"an entry without a thumbnail must render the letter placeholder"
	);
}

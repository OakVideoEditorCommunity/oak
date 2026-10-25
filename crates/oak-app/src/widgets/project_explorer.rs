//! The material bin (project explorer): a tree or icon view of the project's
//! media, with file-drop support.
//!
//! Data-agnostic: the host implements [`ProjectDataSource`] over its model;
//! the widget only *requests* — opening an entry or importing dropped files
//! is emitted as a [`ProjectExplorerEvent`] for the host to apply. Thumbnails
//! are asset paths rendered with `img`; without one the entry shows a
//! placeholder swatch.

use gpui_kit::{
	App, ClickEvent, Context, ElementId, Entity, EventEmitter, ExternalPaths, FocusHandle,
	Focusable, Hsla, MouseButton, MouseDownEvent, Point, Pixels, Render, SharedString, Window,
	colors::DefaultColors, div, hsla, img, prelude::*, px,
};
use std::collections::HashSet;
use std::path::PathBuf;

/// A fully transparent color (for un-hovered rows).
fn transparent() -> gpui_kit::Rgba {
	gpui_kit::Rgba {
		r: 0.0,
		g: 0.0,
		b: 0.0,
		a: 0.0,
	}
}

/// The proxy lifecycle marker shown on a footage entry (the host maps
/// its proxy state onto this; `None` hides the badge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyBadge {
	/// The proxy file exists and preview may use it.
	Ready,
	/// A proxy transcode is running.
	Generating,
	/// The last generation attempt failed.
	Failed,
}

/// A single entry in the project tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectEntry {
	/// The entry's stable id (host-side key).
	pub id: u64,
	/// The display name.
	pub name: SharedString,
	/// Whether the entry is a folder (expandable).
	pub is_dir: bool,
	/// An asset path for a thumbnail, if any.
	pub thumbnail: Option<SharedString>,
	/// The proxy-state badge, if the entry is footage with proxy state
	/// worth surfacing (folders and proxy-less footage carry `None`).
	pub proxy: Option<ProxyBadge>,
}

impl ProjectEntry {
	/// Create an entry.
	pub fn new(id: u64, name: impl Into<SharedString>, is_dir: bool) -> Self {
		Self {
			id,
			name: name.into(),
			is_dir,
			thumbnail: None,
			proxy: None,
		}
	}

	/// Attach a thumbnail asset path.
	pub fn with_thumbnail(mut self, thumbnail: impl Into<SharedString>) -> Self {
		self.thumbnail = Some(thumbnail.into());
		self
	}

	/// Attach a proxy-state badge.
	pub fn with_proxy_badge(mut self, badge: ProxyBadge) -> Self {
		self.proxy = Some(badge);
		self
	}
}

/// The proxy badge chip: a small rounded "P" colored by lifecycle state
/// (green ready / amber generating / red failed), readable at a glance
/// over thumbnails and tree rows alike.
fn proxy_badge_chip(badge: ProxyBadge) -> gpui_kit::Div {
	let (bg, label) = match badge {
		ProxyBadge::Ready => (hsla(0.36, 0.65, 0.45, 1.0), "P"),
		ProxyBadge::Generating => (hsla(0.12, 0.8, 0.5, 1.0), "P"),
		ProxyBadge::Failed => (hsla(0.0, 0.7, 0.5, 1.0), "!"),
	};
	div()
		.w(px(14.0))
		.h(px(14.0))
		.rounded_sm()
		.bg(bg)
		.flex()
		.items_center()
		.justify_center()
		.text_size(px(9.0))
		.text_color(gpui_kit::white())
		.child(label)
}

/// The drag payload of a draggable media entry: the entry's stable id (the
/// same value the host uses as the [`ProjectEntry`] key). Drop targets (the
/// timeline) receive this id and place a clip of the footage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootageDrag(pub u64);

/// The ghost rendered under the cursor while dragging a media entry.
struct FootageDragGhost;

impl Render for FootageDragGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		// A small translucent swatch matching the icon placeholder size.
		div()
			.size(px(72.0))
			.h(px(48.0))
			.rounded_md()
			.bg(hsla(0.55, 0.6, 0.5, 0.5))
	}
}

/// Builds the drag ghost for a footage drag (see
/// [`StatefulInteractiveElement::on_drag`](gpui_kit::StatefulInteractiveElement::on_drag)).
fn footage_drag_ghost(
	_drag: &FootageDrag,
	_offset: Point<Pixels>,
	_window: &mut Window,
	cx: &mut App,
) -> Entity<FootageDragGhost> {
	cx.new(|_cx| FootageDragGhost)
}

/// The host's project model, read through this trait.
pub trait ProjectDataSource: 'static {
	/// The top-level entries.
	fn roots(&self) -> Vec<ProjectEntry>;
	/// The children of a folder entry.
	fn children(&self, parent_id: u64) -> Vec<ProjectEntry>;
}

/// The material bin's display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerView {
	/// A hierarchical tree.
	Tree,
	/// A flat grid of thumbnails.
	Icons,
}

/// A request emitted by the explorer.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectExplorerEvent {
	/// Open an entry (double-click on a file, or an explicit open request).
	OpenRequested {
		/// The explorer's stable id.
		control: usize,
		/// The entry's id.
		id: u64,
		/// The entry's name.
		name: SharedString,
	},
	/// Files were dropped onto the bin.
	FileDropRequested {
		/// The explorer's stable id.
		control: usize,
		/// The dropped paths.
		paths: Vec<PathBuf>,
	},
	/// The view mode changed.
	ViewChanged {
		/// The explorer's stable id.
		control: usize,
		/// The new view mode.
		view: ExplorerView,
	},
	/// An entry (or the empty area) was right-clicked. The widget selects
	/// the entry but opens no menu itself; the host assembles and shows the
	/// context menu at `position`.
	ContextMenuRequested {
		/// The explorer's stable id.
		control: usize,
		/// The right-clicked entry, or `None` when the empty area was
		/// clicked.
		id: Option<u64>,
		/// Mouse position in window coordinates.
		position: Point<Pixels>,
	},
}

/// Flatten a tree into visible rows, honoring the `expanded` set.
///
/// Pure and unit-tested; `children` resolves a folder's entries.
pub fn flatten_tree<F>(
	roots: &[ProjectEntry],
	expanded: &HashSet<u64>,
	children: F,
) -> Vec<(ProjectEntry, usize)>
where
	F: Fn(u64) -> Vec<ProjectEntry>,
{
	fn walk<F>(
		entries: &[ProjectEntry],
		depth: usize,
		expanded: &HashSet<u64>,
		children: &F,
		out: &mut Vec<(ProjectEntry, usize)>,
	) where
		F: Fn(u64) -> Vec<ProjectEntry>,
	{
		for entry in entries {
			out.push((entry.clone(), depth));
			if entry.is_dir && expanded.contains(&entry.id) {
				let kids = children(entry.id);
				walk(&kids, depth + 1, expanded, children, out);
			}
		}
	}

	let mut out = Vec::new();
	walk(roots, 0, expanded, &children, &mut out);
	out
}

/// A material bin with tree and icon views.
pub struct ProjectExplorer<D: ProjectDataSource> {
	control: usize,
	data: Entity<D>,
	expanded: HashSet<u64>,
	selected: Option<u64>,
	view: ExplorerView,
	focus_handle: FocusHandle,
}

impl<D: ProjectDataSource> ProjectExplorer<D> {
	/// Create an explorer over `data`.
	pub fn new(
		control: usize,
		data: Entity<D>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		// Re-render when the host's model changes (e.g. a background
		// thumbnail finished and the entries now carry one).
		cx.observe(&data, |_, _, cx| cx.notify()).detach();
		Self {
			control,
			data,
			expanded: HashSet::new(),
			selected: None,
			view: ExplorerView::Tree,
			focus_handle: cx.focus_handle(),
		}
	}

	/// The current view mode.
	pub fn view(&self) -> ExplorerView {
		self.view
	}

	/// Set the view mode from the host.
	pub fn set_view(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
		if self.view != view {
			self.view = view;
			cx.notify();
		}
	}

	fn toggle(&mut self, id: u64, cx: &mut Context<Self>) {
		if !self.expanded.remove(&id) {
			self.expanded.insert(id);
		}
		cx.notify();
	}

	fn open(&mut self, entry: &ProjectEntry, cx: &mut Context<Self>) {
		cx.emit(ProjectExplorerEvent::OpenRequested {
			control: self.control,
			id: entry.id,
			name: entry.name.clone(),
		});
		cx.notify();
	}

	fn set_view_mode(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
		if self.view != view {
			self.view = view;
			cx.emit(ProjectExplorerEvent::ViewChanged {
				control: self.control,
				view,
			});
			cx.notify();
		}
	}

	/// Reports a right-click: selects the entry (if any) and emits
	/// [`ProjectExplorerEvent::ContextMenuRequested`] for the host's menu.
	fn context_menu(&mut self, id: Option<u64>, position: Point<Pixels>, cx: &mut Context<Self>) {
		if let Some(id) = id {
			self.selected = Some(id);
		}
		cx.emit(ProjectExplorerEvent::ContextMenuRequested {
			control: self.control,
			id,
			position,
		});
		cx.notify();
	}
}

impl<D: ProjectDataSource> EventEmitter<ProjectExplorerEvent> for ProjectExplorer<D> {}

impl<D: ProjectDataSource> Focusable for ProjectExplorer<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: ProjectDataSource> Render for ProjectExplorer<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let roots = self.data.read(cx).roots();
		let selected = self.selected;
		let control = self.control;

		let toolbar = div()
			.flex()
			.items_center()
			.gap_1()
			.px_2()
			.py_1()
			.bg(colors.container)
			.child(toggle_button(
				"gpui-widgets-explorer-tree",
				crate::widgets::glyphs::Glyph::List,
				crate::widgets::i18n::tr("explorer.tree", "树"),
				self.view == ExplorerView::Tree,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.set_view_mode(ExplorerView::Tree, cx);
				}),
			))
			.child(toggle_button(
				"gpui-widgets-explorer-icons",
				crate::widgets::glyphs::Glyph::Grid,
				crate::widgets::i18n::tr("explorer.icons", "图标"),
				self.view == ExplorerView::Icons,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.set_view_mode(ExplorerView::Icons, cx);
				}),
			));

		let content = match self.view {
			ExplorerView::Tree => {
				let expanded = self.expanded.clone();
				let rows = flatten_tree(&roots, &expanded, |id| self.data.read(cx).children(id));
				let mut column = div()
					.id(ElementId::named_usize(
						"gpui-widgets-explorer-tree",
						control,
					))
					.flex()
					.flex_col()
					.py_1()
					.overflow_y_scroll();
				for (entry, depth) in rows {
					let is_selected = selected == Some(entry.id);
					let click_entry = entry.clone();
					let entry_id = entry.id;
					let mut row = div()
						.id(ElementId::named_usize(
							"gpui-widgets-explorer-entry",
							entry.id as usize,
						))
						.debug_selector(move || {
							format!("gpui-widgets-explorer-entry-{entry_id}").into()
						})
						.h(px(24.0))
						.flex()
						.items_center()
						.gap_1()
						.pl(px(8.0 + depth as f32 * 14.0))
						.pr_2()
						.cursor_pointer()
						.bg(if is_selected {
							colors.selected
						} else {
							transparent()
						})
						.text_color(if is_selected {
							colors.selected_text
						} else {
							colors.text
						})
						.on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
							this.selected = Some(click_entry.id);
							if click_entry.is_dir {
								if event.click_count() >= 2 {
									this.open(&click_entry, cx);
								} else {
									this.toggle(click_entry.id, cx);
								}
							} else if event.click_count() >= 2 {
								this.open(&click_entry, cx);
							}
							cx.notify();
						}))
						.on_mouse_down(
							MouseButton::Right,
							cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
								this.context_menu(Some(entry_id), event.position, cx);
								cx.stop_propagation();
							}),
						)
						.child(div().w(px(14.0)).children(entry.is_dir.then(|| {
							crate::widgets::glyphs::icon(
								if this_expanded(&expanded, entry.id) {
									crate::widgets::glyphs::Glyph::ChevronDown
								} else {
									crate::widgets::glyphs::Glyph::ChevronRight
								},
								if is_selected {
									colors.selected_text
								} else {
									colors.text
								},
							)
						})))
						// Small type glyph: a folder box for directories, a
						// document for media files. The widget crate ships no
						// folder/file SVGs, so the rows use text glyphs (the
						// host's icon set is timeline/viewer tools only).
						.child(div().w(px(16.0)).child(if entry.is_dir { "▣" } else { "▤" }))
						.child(div().child(entry.name.clone()))
						// The proxy badge sits right after the name (tree
						// rows have no thumbnail to overlay).
						.children(entry.proxy.map(proxy_badge_chip));
					if !entry.is_dir {
						// Media rows are drag sources: the payload carries the
						// entry id for the timeline drop target.
						row = row.on_drag(FootageDrag(entry.id), footage_drag_ghost);
					}
					if !is_selected {
						row = row.hover(|style| style.bg(Hsla::from(colors.selected).opacity(0.3)));
					}
					column = column.child(row);
				}
				column
			}
			ExplorerView::Icons => {
				// A flat grid of the visible media: root-level entries (leaves)
				// directly, folder roots expanded to their children — the same
				// visible set the tree shows. (The former `children(root)`
				// only looped the roots' children, so root-level footage —
				// imported media that lives directly in the project root —
				// never appeared.)
				let visible: Vec<ProjectEntry> = roots
					.iter()
					.flat_map(|root| {
						if root.is_dir {
							self.data.read(cx).children(root.id)
						} else {
							vec![root.clone()]
						}
					})
					.collect();
				let mut grid = div()
					.id(ElementId::named_usize(
						"gpui-widgets-explorer-icons",
						control,
					))
					.flex()
					.flex_wrap()
					.items_start()
					.gap_2()
					.p_2()
					.overflow_y_scroll();
				for entry in visible {
					let is_selected = selected == Some(entry.id);
					let click_entry = entry.clone();
					let entry_id = entry.id;
					let mut icon = div()
						.id(ElementId::named_usize(
							"gpui-widgets-explorer-icon",
							entry.id as usize,
						))
						.debug_selector(move || {
							format!("gpui-widgets-explorer-icon-{entry_id}").into()
						})
						.w(px(96.0))
						.p_1()
						.rounded_md()
						.flex()
						.flex_col()
						.items_center()
						.gap_1()
						.cursor_pointer()
						.bg(if is_selected {
							colors.selected
						} else {
							transparent()
						})
						.hover(|style| {
							if is_selected {
								style
							} else {
								style.bg(Hsla::from(colors.selected).opacity(0.3))
							}
						})
						.on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
							this.selected = Some(click_entry.id);
							if event.click_count() >= 2 {
								this.open(&click_entry, cx);
							}
							cx.notify();
						}))
						.on_mouse_down(
							MouseButton::Right,
							cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
								this.context_menu(Some(entry_id), event.position, cx);
								cx.stop_propagation();
							}),
						)
						.child(
							div()
								.relative()
								.w(px(72.0))
								.h(px(48.0))
								.child(if let Some(thumbnail) = entry.thumbnail.clone() {
									// A filesystem path: load through the path resource so
									// generated thumbnails resolve without an asset source.
									// The wrapper div carries the test selector (the `img`
									// element itself has no debug bounds).
									div()
										.debug_selector(move || {
											format!("gpui-widgets-explorer-thumb-{entry_id}").into()
										})
										.w(px(72.0))
										.h(px(48.0))
										.child(img(PathBuf::from(thumbnail.as_ref())).w(px(72.0)).h(px(48.0)))
										.into_any_element()
								} else {
									div()
										.debug_selector(move || {
											format!("gpui-widgets-explorer-thumb-placeholder-{entry_id}").into()
										})
										.w(px(72.0))
										.h(px(48.0))
										.rounded_md()
										.bg(Hsla::from(colors.selected).opacity(0.4))
										.flex()
										.items_center()
										.justify_center()
										.text_color(colors.text)
										.child(entry.name.chars().next().unwrap_or(' ').to_string())
										.into_any_element()
								})
								// The proxy badge overlays the thumbnail's
								// bottom-right corner (角标).
								.children(entry.proxy.map(|badge| {
									div()
										.absolute()
										.bottom(px(2.0))
										.right(px(2.0))
										.child(proxy_badge_chip(badge))
										.into_any_element()
								})),
						)
						.child(
							div()
								.w_full()
								.text_color(colors.text)
								.child(entry.name.clone()),
						);
					if !entry.is_dir {
						// Media icons are drag sources like the tree rows.
						icon = icon.on_drag(FootageDrag(entry.id), footage_drag_ghost);
					}
					grid = grid.child(icon);
				}
				grid
			}
		};

		div()
			.id(ElementId::named_usize("gpui-widgets-explorer", control))
			.size_full()
			.flex()
			.flex_col()
			.on_drop(
				cx.listener(|this, paths: &ExternalPaths, _window, cx| {
					cx.emit(ProjectExplorerEvent::FileDropRequested {
						control: this.control,
						paths: paths.0.iter().cloned().collect(),
					});
					cx.notify();
				}),
			)
			// A right-click that bubbles up to the container hit no entry
			// (entry handlers stop propagation): the empty-area menu.
			.on_mouse_down(
				MouseButton::Right,
				cx.listener(|this, event: &MouseDownEvent, _window, cx| {
					this.context_menu(None, event.position, cx);
				}),
			)
			.child(toolbar)
			.child(content.flex_1())
	}
}

fn this_expanded(expanded: &HashSet<u64>, id: u64) -> bool {
	expanded.contains(&id)
}

fn toggle_button(
	id: &'static str,
	glyph: crate::widgets::glyphs::Glyph,
	label: impl Into<gpui_kit::SharedString>,
	active: bool,
	colors: &gpui_kit::colors::Colors,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
	let label: gpui_kit::SharedString = label.into();
	let icon_color = if active {
		colors.selected_text
	} else {
		colors.text
	};
	div()
		.id(id)
		.debug_selector(move || id.into())
		.size(px(24.0))
		.flex()
		.items_center()
		.justify_center()
		.rounded_md()
		.bg(if active {
			colors.selected
		} else {
			transparent()
		})
		.hover(|style| {
			if active {
				style
			} else {
				style.bg(crate::widgets::theme::neutral_hover(colors))
			}
		})
		.cursor_pointer()
		.tooltip(move |window, cx| crate::widgets::tooltip::tooltip_view(label.clone(), window, cx))
		.on_click(on_click)
		.child(crate::widgets::glyphs::icon(glyph, icon_color))
}

#[cfg(test)]
mod tests {
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
			cx.debug_bounds("gpui-widgets-explorer-thumb-placeholder-3").is_some(),
			"an entry without a thumbnail must render the letter placeholder"
		);
	}
}

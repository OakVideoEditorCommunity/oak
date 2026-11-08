//! Pure menu data model: items with nesting, enable/check state, keyboard
//! navigation arithmetic. No gpui coupling, unit-tested.

use gpui_kit::SharedString;

/// A menu item.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
	/// The item's stable id (used in [`MenuEvent`](super::MenuEvent)).
	pub id: usize,
	/// The label shown in the menu.
	pub label: SharedString,
	/// A shortcut to display on the right (e.g. `"⌘S"`).
	pub shortcut: Option<SharedString>,
	/// Whether the item can be activated.
	pub enabled: bool,
	/// `None` = no checkmark; `Some(checked)` = a check/tick state.
	pub checked: Option<bool>,
	/// A nested submenu, opened on hover/click.
	pub submenu: Option<Box<Menu>>,
	/// Whether a separator line follows this item.
	pub separator_after: bool,
}

impl MenuItem {
	/// Creates a plain enabled item with no shortcut, checkmark, submenu or
	/// trailing separator; use the `with_*`/`separated` builders to add those.
	///
	/// # Arguments
	///
	/// * `id` - Stable item id reported by [`MenuEvent`](super::MenuEvent) on
	///   activation and used by [`Menu::set_item_checked`] to find the item, so
	///   ids should be unique within a menu tree.
	/// * `label` - Display text (`impl Into<SharedString>`, so `&str`/`String`
	///   work). An empty label marks the item as a separator
	///   ([`Menu::is_separator`]), which navigation then skips.
	pub fn new(id: usize, label: impl Into<SharedString>) -> Self {
		Self {
			id,
			label: label.into(),
			shortcut: None,
			enabled: true,
			checked: None,
			submenu: None,
			separator_after: false,
		}
	}

	/// Mark the item disabled.
	pub fn disabled(mut self) -> Self {
		self.enabled = false;
		self
	}

	/// Builder: attaches a display-only shortcut label (e.g. `"⌘S"`), drawn on
	/// the right of the item.
	///
	/// The text is neither parsed nor bound to a key handler; callers format it
	/// themselves. Takes and returns `self` so it chains after
	/// [`MenuItem::new`]; the item keeps its previous shortcut if the builder
	/// is not used.
	pub fn with_shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
		self.shortcut = Some(shortcut.into());
		self
	}

	/// Builder: gives the item a checkmark and sets its state.
	///
	/// `true` draws a tick and `false` draws an unchecked item that still
	/// reserves the checkmark slot; `None` (the default) draws no mark at all.
	/// For toggling a menu already handed to a view, use
	/// [`MenuItem::set_checked`] / [`Menu::set_item_checked`] instead of
	/// rebuilding the menu.
	pub fn with_checked(mut self, checked: bool) -> Self {
		self.checked = Some(checked);
		self
	}

	/// Set the checked state of an already-built item (runtime mutation).
	///
	/// Unlike the construction-only [`with_checked`](Self::with_checked), this
	/// lets a host flip a menu checkmark after the [`Menu`] has been handed
	/// to a view — e.g. through [`Menu::set_item_checked`] on the menu held
	/// by a [`MenuBar`](super::MenuBar) — without rebuilding the menu. The
	/// renderers read `checked` on every frame, so the change shows up on the
	/// next repaint.
	pub fn set_checked(&mut self, checked: bool) -> &mut Self {
		self.checked = Some(checked);
		self
	}

	/// Remove the checkmark from an already-built item (runtime mutation).
	pub fn clear_checked(&mut self) -> &mut Self {
		self.checked = None;
		self
	}

	/// Builder: attaches a nested submenu, opened on hover/click of this item.
	///
	/// The submenu is boxed inside the item; its items are ordinary
	/// [`MenuItem`]s with their own ids, so [`Menu::set_item_checked`] searches
	/// into it recursively. Takes and returns `self` so it chains after
	/// [`MenuItem::new`].
	pub fn with_submenu(mut self, submenu: Menu) -> Self {
		self.submenu = Some(Box::new(submenu));
		self
	}

	/// Draw a separator line after this item.
	pub fn separated(mut self) -> Self {
		self.separator_after = true;
		self
	}
}

/// A menu: an ordered list of items.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Menu {
	/// The items of this menu.
	pub items: Vec<MenuItem>,
}

impl Menu {
	/// Creates a menu from an ordered item list; the order is both the
	/// top-to-bottom display order and the navigation order.
	///
	/// `items` may contain separators (empty labels) and disabled items; both
	/// are skipped by [`Menu::navigate`]. Use `Menu::default` for an empty menu.
	pub fn new(items: Vec<MenuItem>) -> Self {
		Self { items }
	}

	/// Whether the item at `index` is a visual separator (an empty item).
	pub fn is_separator(item: &MenuItem) -> bool {
		item.label.is_empty()
	}

	/// Sets the checked state of the item with `id`, searching top-level
	/// items and their submenus recursively. Returns whether an item with
	/// that id was found and updated.
	///
	/// Runtime counterpart to [`MenuItem::with_checked`]: hosts that hold a
	/// live [`Menu`] (e.g. in a [`MenuBar`](super::MenuBar)) can toggle a
	/// checkmark without rebuilding the menu.
	pub fn set_item_checked(&mut self, id: usize, checked: bool) -> bool {
		for item in &mut self.items {
			if item.id == id {
				item.set_checked(checked);
				return true;
			}
			if let Some(submenu) = item.submenu.as_mut() {
				if submenu.set_item_checked(id, checked) {
					return true;
				}
			}
		}
		false
	}

	/// The next selectable index from `current`, moving `delta` steps
	/// (skipping separators and disabled items). `None` returns the first
	/// (or last) selectable item. Returns `None` if nothing is selectable.
	pub fn navigate(&self, current: Option<usize>, delta: i32) -> Option<usize> {
		let selectable: Vec<usize> = self
			.items
			.iter()
			.enumerate()
			.filter(|(_, item)| item.enabled && !Self::is_separator(item))
			.map(|(index, _)| index)
			.collect();
		if selectable.is_empty() {
			return None;
		}
		let position = current.and_then(|c| selectable.iter().position(|&i| i == c));
		let next = match position {
			Some(pos) => ((pos as i64 + delta as i64).rem_euclid(selectable.len() as i64)) as usize,
			None if delta > 0 => 0,
			None => selectable.len() - 1,
		};
		Some(selectable[next])
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn sample_menu() -> Menu {
		Menu::new(vec![
			MenuItem::new(1, "Open…").with_shortcut("⌘O"),
			MenuItem::new(2, "Save").with_shortcut("⌘S"),
			MenuItem::new(3, "Save As…")
				.with_shortcut("⇧⌘S")
				.separated(),
			MenuItem::new(4, "Export").disabled(),
			MenuItem::new(5, "Export Again").with_checked(true),
		])
	}

	#[test]
	fn navigate_skips_disabled_and_separators() {
		let menu = sample_menu();
		// From none, down lands on the first selectable (Open).
		assert_eq!(menu.navigate(None, 1), Some(0));
		// From none, up lands on the last selectable (Export Again, idx 4).
		assert_eq!(menu.navigate(None, -1), Some(4));
		// From Open (0), down skips nothing until Save (1).
		assert_eq!(menu.navigate(Some(0), 1), Some(1));
		// From Save As (2), down skips the disabled Export (3) to Export Again (4).
		assert_eq!(menu.navigate(Some(2), 1), Some(4));
		// Wrap around.
		assert_eq!(menu.navigate(Some(4), 1), Some(0));
		assert_eq!(menu.navigate(Some(0), -1), Some(4));
	}

	#[test]
	fn navigate_returns_none_when_nothing_selectable() {
		let menu = Menu::new(vec![
			MenuItem::new(1, "Only").disabled(),
			MenuItem::new(2, ""),
		]);
		assert_eq!(menu.navigate(None, 1), None);
		assert_eq!(menu.navigate(Some(1), 1), None);
	}

	#[test]
	fn checked_state_is_optional() {
		let menu = sample_menu();
		assert_eq!(menu.items[0].checked, None);
		assert_eq!(menu.items[4].checked, Some(true));
	}

	#[test]
	fn set_item_checked_mutates_in_place() {
		let mut menu = sample_menu();
		// Runtime toggle on a built item, found by id.
		assert!(menu.set_item_checked(5, false));
		assert_eq!(menu.items[4].checked, Some(false));
		assert!(menu.set_item_checked(5, true));
		assert_eq!(menu.items[4].checked, Some(true));
		// Clear the checkmark entirely.
		assert!(menu.items[4].clear_checked().checked.is_none());
		// Unknown ids report failure and change nothing.
		assert!(!menu.set_item_checked(999, true));
		assert_eq!(menu.items[0].checked, None);
	}

	#[test]
	fn set_item_checked_reaches_nested_submenus() {
		let sub = Menu::new(vec![MenuItem::new(10, "A"), MenuItem::new(11, "B")]);
		let mut menu = Menu::new(vec![
			MenuItem::new(5, "Nested").with_submenu(sub),
			MenuItem::new(6, "Top"),
		]);
		assert!(menu.set_item_checked(11, true));
		assert_eq!(
			menu.items[0].submenu.as_ref().unwrap().items[1].checked,
			Some(true)
		);
		// The top-level item is untouched.
		assert_eq!(menu.items[1].checked, None);
	}

	#[test]
	fn cascade_nesting() {
		let sub = Menu::new(vec![MenuItem::new(10, "A"), MenuItem::new(11, "B")]);
		let item = MenuItem::new(5, "Nested").with_submenu(sub.clone());
		assert_eq!(item.submenu.as_ref().unwrap().items.len(), 2);
		assert_eq!(item.submenu.as_ref().unwrap().items[1].id, 11);
		// The nested menu itself navigates independently.
		assert_eq!(item.submenu.as_ref().unwrap().navigate(None, -1), Some(1));
	}

	#[test]
	fn separator_detection() {
		assert!(Menu::is_separator(&MenuItem::new(0, "")));
		assert!(!Menu::is_separator(&MenuItem::new(0, "Save")));
	}
}

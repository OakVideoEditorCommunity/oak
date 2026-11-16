use std::fmt::Debug;

use ::sum_tree::SumTree;
use collections::FxHashMap;
use sum_tree::Bias;

use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
#[derive(Debug)]
pub(crate) struct TabStopMap {
    current_path: TabStopPath,
    pub(crate) insertion_history: Vec<TabStopOperation>,
    by_id: FxHashMap<FocusId, TabStopNode>,
    order: SumTree<TabStopNode>,
}

#[derive(Debug, Clone)]
pub enum TabStopOperation {
    Insert(FocusHandle),
    Group(TabIndex),
    GroupEnd,
}

impl TabStopOperation {
    fn focus_handle(&self) -> Option<&FocusHandle> {
        match self {
            TabStopOperation::Insert(focus_handle) => Some(focus_handle),
            _ => None,
        }
    }
}

type TabIndex = isize;

#[derive(Debug, Default, PartialEq, Eq, Clone, Ord, PartialOrd)]
struct TabStopPath(smallvec::SmallVec<[TabIndex; 6]>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TabStopNode {
    /// Path to access the node in the tree
    /// The final node in the list is a leaf node corresponding to an actual focus handle,
    /// all other nodes are group nodes
    path: TabStopPath,
    /// index into the backing array of nodes. Corresponds to insertion order
    node_insertion_index: usize,

    /// Whether this node is a tab stop
    tab_stop: bool,
}

impl Ord for TabStopNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then(self.node_insertion_index.cmp(&other.node_insertion_index))
    }
}

impl PartialOrd for TabStopNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl Default for TabStopMap {
    fn default() -> Self {
        Self {
            current_path: TabStopPath::default(),
            insertion_history: Vec::new(),
            by_id: FxHashMap::default(),
            order: SumTree::new(()),
        }
    }
}

impl TabStopMap {
    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
        let mut path = self.current_path.clone();
        path.0.push(focus_handle.tab_index);
        let order = TabStopNode {
            node_insertion_index: self.insertion_history.len() - 1,
            tab_stop: focus_handle.tab_stop,
            path,
        };
        self.by_id.insert(focus_handle.id, order.clone());
        self.order.insert_or_replace(order, ());
    }

    pub fn begin_group(&mut self, tab_index: isize) {
        self.insertion_history
            .push(TabStopOperation::Group(tab_index));
        self.current_path.0.push(tab_index);
    }

    pub fn end_group(&mut self) {
        self.insertion_history.push(TabStopOperation::GroupEnd);
        self.current_path.0.pop();
    }

    pub fn clear(&mut self) {
        *self = Self::default();
        self.current_path.0.clear();
        self.insertion_history.clear();
        self.by_id.clear();
        self.order = SumTree::new(());
    }

    pub fn next(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let first = self.order.first()?;
            if first.tab_stop {
                return self.focus_handle_for_order(first);
            } else {
                return self
                    .next_inner(first)
                    .and_then(|order| self.focus_handle_for_order(order));
            }
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.next(None);
        };
        let item = self.next_inner(node);

        if let Some(item) = item {
            self.focus_handle_for_order(&item)
        } else {
            self.next(None)
        }
    }

    fn next_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        let mut cursor = self.order.cursor::<TabStopNode>(());
        cursor.seek(&node, Bias::Left);
        cursor.next();
        while let Some(item) = cursor.item()
            && !item.tab_stop
        {
            cursor.next();
        }

        cursor.item()
    }

    pub fn prev(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let last = self.order.last()?;
            if last.tab_stop {
                return self.focus_handle_for_order(last);
            } else {
                return self
                    .prev_inner(last)
                    .and_then(|order| self.focus_handle_for_order(order));
            }
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.prev(None);
        };
        let item = self.prev_inner(node);

        if let Some(item) = item {
            self.focus_handle_for_order(&item)
        } else {
            self.prev(None)
        }
    }

    fn prev_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        let mut cursor = self.order.cursor::<TabStopNode>(());
        cursor.seek(&node, Bias::Left);
        cursor.prev();
        while let Some(item) = cursor.item()
            && !item.tab_stop
        {
            cursor.prev();
        }

        cursor.item()
    }

    pub fn replay(&mut self, nodes: &[TabStopOperation]) {
        for node in nodes {
            match node {
                TabStopOperation::Insert(focus_handle) => self.insert(focus_handle),
                TabStopOperation::Group(tab_index) => self.begin_group(*tab_index),
                TabStopOperation::GroupEnd => self.end_group(),
            }
        }
    }

    pub fn paint_index(&self) -> usize {
        self.insertion_history.len()
    }

    pub(crate) fn tab_stop_count(&self) -> usize {
        self.by_id.values().filter(|node| node.tab_stop).count()
    }

    fn focus_handle_for_order(&self, order: &TabStopNode) -> Option<FocusHandle> {
        let handle = self.insertion_history[order.node_insertion_index].focus_handle();
        debug_assert!(
            handle.is_some(),
            "The order node did not correspond to an element, this is a GPUI bug"
        );
        handle.cloned()
    }

    fn tab_node_for_focus_id(&self, focused_id: &FocusId) -> Option<&TabStopNode> {
        let Some(order) = self.by_id.get(focused_id) else {
            return None;
        };
        Some(order)
    }
}

mod sum_tree_impl {
    use sum_tree::SeekTarget;

    use crate::tab_stop::{TabStopNode, TabStopPath};

    #[derive(Clone, Debug)]
    pub struct TabStopOrderNodeSummary {
        max_index: usize,
        max_path: TabStopPath,
        pub tab_stops: usize,
    }

    pub type TabStopCount = usize;

    impl sum_tree::ContextLessSummary for TabStopOrderNodeSummary {
        fn zero() -> Self {
            TabStopOrderNodeSummary {
                max_index: 0,
                max_path: TabStopPath::default(),
                tab_stops: 0,
            }
        }

        fn add_summary(&mut self, summary: &Self) {
            self.max_index = summary.max_index;
            self.max_path = summary.max_path.clone();
            self.tab_stops += summary.tab_stops;
        }
    }

    impl sum_tree::KeyedItem for TabStopNode {
        type Key = Self;

        fn key(&self) -> Self::Key {
            self.clone()
        }
    }

    impl sum_tree::Item for TabStopNode {
        type Summary = TabStopOrderNodeSummary;

        fn summary(&self, _cx: <Self::Summary as sum_tree::Summary>::Context<'_>) -> Self::Summary {
            TabStopOrderNodeSummary {
                max_index: self.node_insertion_index,
                max_path: self.path.clone(),
                tab_stops: if self.tab_stop { 1 } else { 0 },
            }
        }
    }

    impl<'a> sum_tree::Dimension<'a, TabStopOrderNodeSummary> for TabStopCount {
        fn zero(_: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>) -> Self {
            0
        }

        fn add_summary(
            &mut self,
            summary: &'a TabStopOrderNodeSummary,
            _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
        ) {
            *self += summary.tab_stops;
        }
    }

    impl<'a> sum_tree::Dimension<'a, TabStopOrderNodeSummary> for TabStopNode {
        fn zero(_: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>) -> Self {
            TabStopNode::default()
        }

        fn add_summary(
            &mut self,
            summary: &'a TabStopOrderNodeSummary,
            _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
        ) {
            self.node_insertion_index = summary.max_index;
            self.path = summary.max_path.clone();
        }
    }

    impl<'a, 'b> SeekTarget<'a, TabStopOrderNodeSummary, TabStopNode> for &'b TabStopNode {
        fn cmp(
            &self,
            cursor_location: &TabStopNode,
            _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
        ) -> std::cmp::Ordering {
            Iterator::cmp(self.path.0.iter(), cursor_location.path.0.iter()).then(
                <usize as Ord>::cmp(
                    &self.node_insertion_index,
                    &cursor_location.node_insertion_index,
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests;

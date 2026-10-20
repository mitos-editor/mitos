//! Stable pane identities, split topology, and rectangle allocation.
//! Applications own document bindings and other lifecycle effects around structural changes.

use slotmap::{Key, SlotMap};
use ui_core::graphics::Rect;

/// A caller-owned pane value. The layout assigns its stable key and geometry.
pub trait Pane<K: Key> {
    fn id(&self) -> K;
    fn set_id(&mut self, id: K);
    fn area(&self) -> Rect;
    fn set_area(&mut self, area: Rect);
}

/// Split topology and geometry over stable keys and caller-owned pane values.
#[derive(Debug)]
pub struct Tree<K: Key, P: Pane<K>> {
    root: K,
    focus: K,
    area: Rect,

    nodes: SlotMap<K, Node<K, P>>,

    // used for traversals
    stack: Vec<(K, Rect)>,
}

#[derive(Debug)]
struct Node<K: Key, P: Pane<K>> {
    parent: K,
    content: Content<K, P>,
}

#[derive(Debug)]
enum Content<K: Key, P: Pane<K>> {
    Pane(Box<P>),
    Container(Box<Container<K>>),
}

impl<K: Key, P: Pane<K>> Node<K, P> {
    pub fn container(layout: Layout) -> Self {
        Self {
            parent: K::default(),
            content: Content::Container(Box::new(Container::new(layout))),
        }
    }

    pub fn pane(pane: P) -> Self {
        Self {
            parent: K::default(),
            content: Content::Pane(Box::new(pane)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Horizontal,
    Vertical,
    // could explore stacked/tabbed
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug)]
struct Container<K: Key> {
    layout: Layout,
    children: Vec<K>,
    area: Rect,
}

impl<K: Key> Container<K> {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            area: Rect::default(),
        }
    }
}

impl<K: Key> Default for Container<K> {
    fn default() -> Self {
        Self::new(Layout::Vertical)
    }
}

impl<K: Key, P: Pane<K>> Tree<K, P> {
    pub fn new(area: Rect) -> Self {
        let root = Node::container(Layout::Vertical);

        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(root);

        // The root is its own parent.
        nodes[root].parent = root;

        Self {
            root,
            focus: root,
            area,
            nodes,
            stack: Vec::new(),
        }
    }

    /// The focused pane key, or the root container key while empty.
    pub fn focus(&self) -> K {
        self.focus
    }

    pub fn set_focus(&mut self, id: K) {
        assert!(
            self.try_get(id).is_some() || (self.is_empty() && id == self.root),
            "focus must identify a pane"
        );
        self.focus = id;
    }

    pub fn insert(&mut self, pane: P) -> K {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;
        let mut node = Node::pane(pane);
        node.parent = parent;
        let node = self.nodes.insert(node);
        self.get_mut(node).set_id(node);

        let container = match &mut self.nodes[parent] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        };

        // insert node after the current item if there is children already
        let pos = if container.children.is_empty() {
            0
        } else {
            let pos = container
                .children
                .iter()
                .position(|&child| child == focus)
                .unwrap();
            pos + 1
        };

        container.children.insert(pos, node);
        // focus the new node
        self.focus = node;

        // recalculate all the sizes
        self.recalculate();

        node
    }

    pub fn split(&mut self, pane: P, layout: Layout) -> K {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;

        let node = Node::pane(pane);
        let node = self.nodes.insert(node);
        self.get_mut(node).set_id(node);

        let container = match &mut self.nodes[parent] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        };
        if container.layout == layout {
            // insert node after the current item if there is children already
            let pos = if container.children.is_empty() {
                0
            } else {
                let pos = container
                    .children
                    .iter()
                    .position(|&child| child == focus)
                    .unwrap();
                pos + 1
            };
            container.children.insert(pos, node);
            self.nodes[node].parent = parent;
        } else {
            let mut split = Node::container(layout);
            split.parent = parent;
            let split = self.nodes.insert(split);

            let container = match &mut self.nodes[split] {
                Node {
                    content: Content::Container(container),
                    ..
                } => container,
                _ => unreachable!(),
            };
            container.children.push(focus);
            container.children.push(node);
            self.nodes[focus].parent = split;
            self.nodes[node].parent = split;

            let container = match &mut self.nodes[parent] {
                Node {
                    content: Content::Container(container),
                    ..
                } => container,
                _ => unreachable!(),
            };

            let pos = container
                .children
                .iter()
                .position(|&child| child == focus)
                .unwrap();

            // replace focus on parent with split
            container.children[pos] = split;
        }

        // focus the new node
        self.focus = node;

        // recalculate all the sizes
        self.recalculate();

        node
    }

    /// Get a mutable reference to a [Container] by index.
    /// # Panics
    /// Panics if `index` is not in self.nodes, or if the node's content is not a [Content::Container].
    fn container_mut(&mut self, index: K) -> &mut Container<K> {
        match &mut self.nodes[index] {
            Node {
                content: Content::Container(container),
                ..
            } => container,
            _ => unreachable!(),
        }
    }

    fn remove_or_replace(&mut self, child: K, replacement: Option<K>) {
        let parent = self.nodes[child].parent;

        self.nodes.remove(child);

        let container = self.container_mut(parent);
        let pos = container
            .children
            .iter()
            .position(|&item| item == child)
            .unwrap();

        if let Some(new) = replacement {
            container.children[pos] = new;
            self.nodes[new].parent = parent;
        } else {
            container.children.remove(pos);
        }
    }

    pub fn remove(&mut self, index: K) {
        if self.focus == index {
            // focus on something else
            self.focus = self.prev();
        }

        let parent = self.nodes[index].parent;
        let parent_is_root = parent == self.root;

        self.remove_or_replace(index, None);

        let parent_container = self.container_mut(parent);
        if parent_container.children.len() == 1 && !parent_is_root {
            // Merge the only child back into its grandparent so that panes
            // are equally spaced.
            let sibling = parent_container.children.pop().unwrap();
            self.remove_or_replace(parent, Some(sibling));
        }

        self.recalculate()
    }

    pub fn panes(&self) -> impl Iterator<Item = (&P, bool)> {
        let focus = self.focus;
        self.nodes.iter().filter_map(move |(key, node)| match node {
            Node {
                content: Content::Pane(pane),
                ..
            } => Some((pane.as_ref(), focus == key)),
            _ => None,
        })
    }

    pub fn panes_mut(&mut self) -> impl Iterator<Item = (&mut P, bool)> {
        let focus = self.focus;
        self.nodes
            .iter_mut()
            .filter_map(move |(key, node)| match node {
                Node {
                    content: Content::Pane(pane),
                    ..
                } => Some((pane.as_mut(), focus == key)),
                _ => None,
            })
    }

    /// Borrow the pane identified by `index`.
    ///
    /// # Panics
    ///
    /// Panics for removed keys and container nodes. Use [`Self::try_get`] to check.
    pub fn get(&self, index: K) -> &P {
        self.try_get(index).unwrap()
    }

    /// Borrow a pane, returning `None` for removed keys and container nodes.
    pub fn try_get(&self, index: K) -> Option<&P> {
        match self.nodes.get(index) {
            Some(Node {
                content: Content::Pane(pane),
                ..
            }) => Some(pane),
            _ => None,
        }
    }

    /// Mutably borrow the pane identified by `index`.
    ///
    /// # Panics
    ///
    /// Panics for removed keys and container nodes. Use [`Self::try_get`] to check.
    pub fn get_mut(&mut self, index: K) -> &mut P {
        match &mut self.nodes[index] {
            Node {
                content: Content::Pane(pane),
                ..
            } => pane,
            _ => unreachable!(),
        }
    }

    /// Check whether a key belongs to this tree, including split containers.
    pub fn contains(&self, index: K) -> bool {
        self.nodes.contains_key(index)
    }

    pub fn is_empty(&self) -> bool {
        match &self.nodes[self.root] {
            Node {
                content: Content::Container(container),
                ..
            } => container.children.is_empty(),
            _ => unreachable!(),
        }
    }

    pub fn resize(&mut self, area: Rect) -> bool {
        if self.area != area {
            self.area = area;
            self.recalculate();
            return true;
        }
        false
    }

    pub fn recalculate(&mut self) {
        if self.is_empty() {
            // There are no more panes, so the tree should focus itself again.
            self.focus = self.root;

            return;
        }

        self.stack.push((self.root, self.area));

        // take the area
        // fetch the node
        // a) node is pane, give it whole area
        // b) node is container, calculate areas for each child and push them on the stack

        while let Some((key, area)) = self.stack.pop() {
            let node = &mut self.nodes[key];

            match &mut node.content {
                Content::Pane(pane) => {
                    pane.set_area(area);
                }
                Content::Container(container) => {
                    container.area = area;

                    match container.layout {
                        Layout::Horizontal => {
                            let len = container.children.len();

                            let height = (usize::from(area.height) / len) as u16;

                            let mut child_y = area.y;

                            for (i, child) in container.children.iter().enumerate() {
                                let mut area = Rect::new(
                                    container.area.x,
                                    child_y,
                                    container.area.width,
                                    height,
                                );
                                child_y =
                                    child_y.saturating_add(height).min(container.area.bottom());

                                // The last child takes the remaining space after rounding.
                                if i == len - 1 {
                                    area.height = container.area.bottom().saturating_sub(area.y);
                                }

                                self.stack.push((*child, area));
                            }
                        }
                        Layout::Vertical => {
                            let len = container.children.len();

                            let inner_gap = 1u16;
                            let total_gap = len.saturating_sub(2);

                            let used_area = usize::from(area.width).saturating_sub(total_gap);
                            let width = (used_area / len) as u16;

                            let mut child_x = area.x;

                            for (i, child) in container.children.iter().enumerate() {
                                let mut area = Rect::new(
                                    child_x,
                                    container.area.y,
                                    width,
                                    container.area.height,
                                );
                                child_x = child_x
                                    .saturating_add(width)
                                    .saturating_add(inner_gap)
                                    .min(container.area.right());

                                // The last child takes the remaining space after rounding.
                                if i == len - 1 {
                                    area.width = container.area.right().saturating_sub(area.x);
                                }

                                self.stack.push((*child, area));
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn traverse(&self) -> Traverse<'_, K, P> {
        Traverse::new(self)
    }

    // Finds the split in the given direction if it exists
    pub fn find_split_in_direction(&self, id: K, direction: Direction) -> Option<K> {
        let parent = self.nodes[id].parent;
        // Base case, we found the root of the tree
        if parent == id {
            return None;
        }
        // Parent must always be a container
        let parent_container = match &self.nodes[parent].content {
            Content::Container(container) => container,
            Content::Pane(_) => unreachable!(),
        };

        match (direction, parent_container.layout) {
            (Direction::Up, Layout::Vertical)
            | (Direction::Left, Layout::Horizontal)
            | (Direction::Right, Layout::Horizontal)
            | (Direction::Down, Layout::Vertical) => {
                // The desired direction of movement is not possible within
                // the parent container so the search must continue closer to
                // the root of the split tree.
                self.find_split_in_direction(parent, direction)
            }
            (Direction::Up, Layout::Horizontal)
            | (Direction::Down, Layout::Horizontal)
            | (Direction::Left, Layout::Vertical)
            | (Direction::Right, Layout::Vertical) => {
                // It's possible to move in the desired direction within
                // the parent container so an attempt is made to find the
                // correct child.
                match self.find_child(id, &parent_container.children, direction) {
                    // Child is found, search is ended
                    Some(id) => Some(id),
                    // A child is not found. This could be because of either two scenarios
                    // 1. Its not possible to move in the desired direction, and search should end
                    // 2. A layout like the following with focus at X and desired direction Right
                    // | _ | x |   |
                    // | _ _ _ |   |
                    // | _ _ _ |   |
                    // The container containing X ends at X so no rightward movement is possible
                    // however there still exists another pane/container to the right that hasn't
                    // been explored. Thus another search is done here in the parent container
                    // before concluding it's not possible to move in the desired direction.
                    None => self.find_split_in_direction(parent, direction),
                }
            }
        }
    }

    fn find_child(&self, id: K, children: &[K], direction: Direction) -> Option<K> {
        let mut child_id = match direction {
            // index wise in the child list the Up and Left represents a -1
            // thus reversed iterator.
            Direction::Up | Direction::Left => children
                .iter()
                .rev()
                .skip_while(|i| **i != id)
                .copied()
                .nth(1)?,
            // Down and Right => +1 index wise in the child list
            Direction::Down | Direction::Right => {
                children.iter().skip_while(|i| **i != id).copied().nth(1)?
            }
        };
        let (current_x, current_y) = match &self.nodes[self.focus].content {
            Content::Pane(current_pane) => (current_pane.area().left(), current_pane.area().top()),
            Content::Container(_) => unreachable!(),
        };

        // If the child is a container the search finds the closest container child
        // visually based on screen location.
        while let Content::Container(container) = &self.nodes[child_id].content {
            match (direction, container.layout) {
                (_, Layout::Vertical) => {
                    // find closest split based on x because y is irrelevant
                    // in a vertical container (and already correct based on previous search)
                    child_id = *container.children.iter().min_by_key(|id| {
                        let x = match &self.nodes[**id].content {
                            Content::Pane(pane) => pane.area().left(),
                            Content::Container(container) => container.area.left(),
                        };
                        current_x.abs_diff(x)
                    })?;
                }
                (_, Layout::Horizontal) => {
                    // find closest split based on y because x is irrelevant
                    // in a horizontal container (and already correct based on previous search)
                    child_id = *container.children.iter().min_by_key(|id| {
                        let y = match &self.nodes[**id].content {
                            Content::Pane(pane) => pane.area().top(),
                            Content::Container(container) => container.area.top(),
                        };
                        current_y.abs_diff(y)
                    })?;
                }
            }
        }
        Some(child_id)
    }

    pub fn prev(&self) -> K {
        // This function is very dumb, but that's because we don't store any parent links.
        // (we'd be able to go parent.prev_sibling() recursively until we find something)
        // For now that's okay though, since it's unlikely you'll be able to open a large enough
        // number of splits to notice.

        let mut panes = self
            .traverse()
            .rev()
            .skip_while(|&(id, _pane)| id != self.focus)
            .skip(1); // Skip focused value
        if let Some((id, _)) = panes.next() {
            id
        } else {
            // extremely crude, take the last item
            let (key, _) = self.traverse().next_back().unwrap();
            key
        }
    }

    pub fn next(&self) -> K {
        // This function is very dumb, but that's because we don't store any parent links.
        // (we'd be able to go parent.next_sibling() recursively until we find something)
        // For now that's okay though, since it's unlikely you'll be able to open a large enough
        // number of splits to notice.

        let mut panes = self
            .traverse()
            .skip_while(|&(id, _pane)| id != self.focus)
            .skip(1); // Skip focused value
        if let Some((id, _)) = panes.next() {
            id
        } else {
            // extremely crude, take the first item again
            let (key, _) = self.traverse().next().unwrap();
            key
        }
    }

    pub fn transpose(&mut self) {
        let focus = self.focus;
        let parent = self.nodes[focus].parent;
        if let Content::Container(container) = &mut self.nodes[parent].content {
            container.layout = match container.layout {
                Layout::Vertical => Layout::Horizontal,
                Layout::Horizontal => Layout::Vertical,
            };
            self.recalculate();
        }
    }

    pub fn swap_split_in_direction(&mut self, direction: Direction) -> Option<()> {
        let focus = self.focus;
        let target = self.find_split_in_direction(focus, direction)?;
        let focus_parent = self.nodes[focus].parent;
        let target_parent = self.nodes[target].parent;

        if focus_parent == target_parent {
            let parent = focus_parent;
            let [parent, focus, target] = self.nodes.get_disjoint_mut([parent, focus, target])?;
            match (&mut parent.content, &mut focus.content, &mut target.content) {
                (
                    Content::Container(parent),
                    Content::Pane(focus_pane),
                    Content::Pane(target_pane),
                ) => {
                    let focus_pos = parent
                        .children
                        .iter()
                        .position(|id| focus_pane.id() == *id)?;
                    let target_pos = parent
                        .children
                        .iter()
                        .position(|id| target_pane.id() == *id)?;
                    // swap node positions so that traversal order is kept
                    parent.children[focus_pos] = target_pane.id();
                    parent.children[target_pos] = focus_pane.id();
                    // swap area so that panes rendered at the correct location
                    let area = focus_pane.area();
                    focus_pane.set_area(target_pane.area());
                    target_pane.set_area(area);

                    Some(())
                }
                _ => unreachable!(),
            }
        } else {
            let [focus_parent, target_parent, focus, target] =
                self.nodes
                    .get_disjoint_mut([focus_parent, target_parent, focus, target])?;
            match (
                &mut focus_parent.content,
                &mut target_parent.content,
                &mut focus.content,
                &mut target.content,
            ) {
                (
                    Content::Container(focus_parent),
                    Content::Container(target_parent),
                    Content::Pane(focus_pane),
                    Content::Pane(target_pane),
                ) => {
                    let focus_pos = focus_parent
                        .children
                        .iter()
                        .position(|id| focus_pane.id() == *id)?;
                    let target_pos = target_parent
                        .children
                        .iter()
                        .position(|id| target_pane.id() == *id)?;
                    // re-parent target and focus nodes
                    std::mem::swap(
                        &mut focus_parent.children[focus_pos],
                        &mut target_parent.children[target_pos],
                    );
                    std::mem::swap(&mut focus.parent, &mut target.parent);
                    // swap area so that panes rendered at the correct location
                    let area = focus_pane.area();
                    focus_pane.set_area(target_pane.area());
                    target_pane.set_area(area);

                    Some(())
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn area(&self) -> Rect {
        self.area
    }
}

#[derive(Debug)]
pub struct Traverse<'a, K: Key, P: Pane<K>> {
    tree: &'a Tree<K, P>,
    stack: Vec<K>,
}

impl<'a, K: Key, P: Pane<K>> Traverse<'a, K, P> {
    fn new(tree: &'a Tree<K, P>) -> Self {
        Self {
            tree,
            stack: vec![tree.root],
        }
    }
}

impl<'a, K: Key, P: Pane<K>> Iterator for Traverse<'a, K, P> {
    type Item = (K, &'a P);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let key = self.stack.pop()?;

            let node = &self.tree.nodes[key];

            match &node.content {
                Content::Pane(pane) => return Some((key, pane)),
                Content::Container(container) => {
                    self.stack.extend(container.children.iter().rev());
                }
            }
        }
    }
}

impl<K: Key, P: Pane<K>> DoubleEndedIterator for Traverse<'_, K, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            let key = self.stack.pop()?;

            let node = &self.tree.nodes[key];

            match &node.content {
                Content::Pane(pane) => return Some((key, pane)),
                Content::Container(container) => {
                    self.stack.extend(container.children.iter());
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    slotmap::new_key_type! { pub struct PaneId; pub struct ContentId; }
    type Tree = super::Tree<PaneId, TestPane>;
    #[derive(Debug)]
    struct TestPane {
        id: PaneId,
        doc: ContentId,
        area: Rect,
    }
    impl TestPane {
        fn new(doc: ContentId) -> Self {
            Self {
                id: PaneId::default(),
                doc,
                area: Rect::default(),
            }
        }
    }
    impl Pane<PaneId> for TestPane {
        fn id(&self) -> PaneId {
            self.id
        }
        fn set_id(&mut self, id: PaneId) {
            self.id = id;
        }
        fn area(&self) -> Rect {
            self.area
        }
        fn set_area(&mut self, area: Rect) {
            self.area = area;
        }
    }

    #[test]
    fn find_split_in_direction() {
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 80,
        });
        let mut view = TestPane::new(ContentId::default());
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let l0 = tree.focus;
        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Vertical);
        let r0 = tree.focus;

        tree.focus = l0;
        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Horizontal);
        let l1 = tree.focus;

        tree.focus = l0;
        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Vertical);

        // Tree in test
        // | L0  | L2 |    |
        // |    L1    | R0 |
        let l2 = tree.focus;
        assert_eq!(Some(l0), tree.find_split_in_direction(l2, Direction::Left));
        assert_eq!(Some(l1), tree.find_split_in_direction(l2, Direction::Down));
        assert_eq!(Some(r0), tree.find_split_in_direction(l2, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(l2, Direction::Up));

        tree.focus = l1;
        assert_eq!(None, tree.find_split_in_direction(l1, Direction::Left));
        assert_eq!(None, tree.find_split_in_direction(l1, Direction::Down));
        assert_eq!(Some(r0), tree.find_split_in_direction(l1, Direction::Right));
        assert_eq!(Some(l0), tree.find_split_in_direction(l1, Direction::Up));

        tree.focus = l0;
        assert_eq!(None, tree.find_split_in_direction(l0, Direction::Left));
        assert_eq!(Some(l1), tree.find_split_in_direction(l0, Direction::Down));
        assert_eq!(Some(l2), tree.find_split_in_direction(l0, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(l0, Direction::Up));

        tree.focus = r0;
        assert_eq!(Some(l2), tree.find_split_in_direction(r0, Direction::Left));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Down));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Right));
        assert_eq!(None, tree.find_split_in_direction(r0, Direction::Up));
    }

    #[test]
    fn swap_split_in_direction() {
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 80,
        });

        let doc_l0 = ContentId::default();
        let mut view = TestPane::new(doc_l0);
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let l0 = tree.focus;

        let doc_r0 = ContentId::default();
        let view = TestPane::new(doc_r0);
        tree.split(view, Layout::Vertical);
        let r0 = tree.focus;

        tree.focus = l0;

        let doc_l1 = ContentId::default();
        let view = TestPane::new(doc_l1);
        tree.split(view, Layout::Horizontal);
        let l1 = tree.focus;

        tree.focus = l0;

        let doc_l2 = ContentId::default();
        let view = TestPane::new(doc_l2);
        tree.split(view, Layout::Vertical);
        let l2 = tree.focus;

        // Views in test
        // | L0  | L2 |    |
        // |    L1    | R0 |

        // Document IDs in test
        // | l0  | l2 |    |
        // |    l1    | r0 |

        fn doc_id(tree: &Tree, view_id: PaneId) -> Option<ContentId> {
            if let Content::Pane(view) = &tree.nodes[view_id].content {
                Some(view.doc)
            } else {
                None
            }
        }

        tree.focus = l0;
        // `*` marks the view in focus from view table (here L0)
        // | l0*  | l2 |    |
        // |    l1     | r0 |
        tree.swap_split_in_direction(Direction::Down);
        // | l1   | l2 |    |
        // |    l0*    | r0 |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_l0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_r0));

        tree.swap_split_in_direction(Direction::Right);

        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        // cannot swap, nothing changes
        tree.swap_split_in_direction(Direction::Up);
        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        // cannot swap, nothing changes
        tree.swap_split_in_direction(Direction::Down);
        // | l1  | l2 |     |
        // |    r0    | l0* |
        assert_eq!(tree.focus, l0);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_r0));
        assert_eq!(doc_id(&tree, l2), Some(doc_l2));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        tree.focus = l2;
        // | l1  | l2* |    |
        // |    r0     | l0 |

        tree.swap_split_in_direction(Direction::Down);
        // | l1  | r0  |    |
        // |    l2*    | l0 |
        assert_eq!(tree.focus, l2);
        assert_eq!(doc_id(&tree, l0), Some(doc_l1));
        assert_eq!(doc_id(&tree, l1), Some(doc_l2));
        assert_eq!(doc_id(&tree, l2), Some(doc_r0));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));

        tree.swap_split_in_direction(Direction::Up);
        // | l2* | r0 |    |
        // |    l1    | l0 |
        assert_eq!(tree.focus, l2);
        assert_eq!(doc_id(&tree, l0), Some(doc_l2));
        assert_eq!(doc_id(&tree, l1), Some(doc_l1));
        assert_eq!(doc_id(&tree, l2), Some(doc_r0));
        assert_eq!(doc_id(&tree, r0), Some(doc_l0));
    }

    #[test]
    fn all_vertical_views_have_same_width() {
        let tree_area_width = 180;
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: tree_area_width,
            height: 80,
        });
        let mut view = TestPane::new(ContentId::default());
        view.area = Rect::new(0, 0, 180, 80);
        tree.insert(view);

        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Vertical);

        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Horizontal);

        tree.remove(tree.focus);

        let view = TestPane::new(ContentId::default());
        tree.split(view, Layout::Vertical);

        // Make sure that we only have one level in the tree.
        assert_eq!(3, tree.panes().count());
        assert_eq!(
            vec![
                tree_area_width / 3 - 1, // gap here
                tree_area_width / 3 - 1, // gap here
                tree_area_width / 3
            ],
            tree.panes()
                .map(|(view, _)| view.area.width)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn vsplit_gap_rounding() {
        let (tree_area_width, tree_area_height) = (80, 24);
        let mut tree = Tree::new(Rect {
            x: 0,
            y: 0,
            width: tree_area_width,
            height: tree_area_height,
        });
        let mut view = TestPane::new(ContentId::default());
        view.area = Rect::new(0, 0, tree_area_width, tree_area_height);
        tree.insert(view);

        for _ in 0..9 {
            let view = TestPane::new(ContentId::default());
            tree.split(view, Layout::Vertical);
        }

        assert_eq!(10, tree.panes().count());
        assert_eq!(
            std::iter::repeat_n(7, 9)
                .chain(Some(8)) // Rounding in `recalculate`.
                .collect::<Vec<_>>(),
            tree.panes()
                .map(|(view, _)| view.area.width)
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod invariants {
    use super::*;
    slotmap::new_key_type! { struct Id; }
    #[derive(Debug)]
    struct Value {
        id: Id,
        area: Rect,
    }
    impl Value {
        fn new() -> Self {
            Self {
                id: Id::default(),
                area: Rect::default(),
            }
        }
    }
    impl Pane<Id> for Value {
        fn id(&self) -> Id {
            self.id
        }
        fn set_id(&mut self, id: Id) {
            self.id = id;
        }
        fn area(&self) -> Rect {
            self.area
        }
        fn set_area(&mut self, area: Rect) {
            self.area = area;
        }
    }

    #[test]
    fn nested_panes_remain_bounded_in_tiny_viewports() {
        for width in 0..5 {
            for height in 0..5 {
                let area = Rect::new(10, 20, width, height);
                let mut tree = Tree::new(area);
                tree.insert(Value::new());
                for index in 0..12 {
                    tree.split(
                        Value::new(),
                        if index % 2 == 0 {
                            Layout::Vertical
                        } else {
                            Layout::Horizontal
                        },
                    );
                }
                for (value, _) in tree.panes() {
                    assert!(value.area.left() >= area.left());
                    assert!(value.area.top() >= area.top());
                    assert!(value.area.right() <= area.right());
                    assert!(value.area.bottom() <= area.bottom());
                }
            }
        }
    }

    #[test]
    fn removal_and_key_reuse_preserve_other_panes() {
        let mut tree = Tree::new(Rect::new(0, 0, 100, 50));
        let first = tree.insert(Value::new());
        let second = tree.split(Value::new(), Layout::Vertical);
        tree.remove(first);
        let replacement = tree.split(Value::new(), Layout::Horizontal);
        assert_ne!(first, replacement);
        assert!(tree.try_get(first).is_none());
        assert_eq!(tree.get(second).id(), second);
        tree.transpose();
        assert_eq!(
            tree.traverse().map(|(id, _)| id).collect::<Vec<_>>(),
            [second, replacement]
        );
        tree.remove(replacement);
        tree.remove(second);
        assert!(tree.is_empty());
        let new = tree.insert(Value::new());
        assert_eq!(tree.focus(), new);
    }
}

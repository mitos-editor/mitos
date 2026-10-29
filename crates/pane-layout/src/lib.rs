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
    min_pane_width: u16,
    min_pane_height: u16,

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

/// The pane extent a resize changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Width,
    Height,
}

impl Dimension {
    /// The container layout whose children divide this dimension.
    fn layout(self) -> Layout {
        match self {
            Self::Width => Layout::Vertical,
            Self::Height => Layout::Horizontal,
        }
    }
}

#[derive(Debug)]
struct Container<K: Key> {
    layout: Layout,
    children: Vec<K>,
    /// Relative size of each child, by position. Equal weights give the plain even split.
    weights: Vec<u32>,
    area: Rect,
}

impl<K: Key> Container<K> {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            weights: Vec::new(),
            area: Rect::default(),
        }
    }

    /// Insert a child with the average weight, so it takes `1 / len` of the container and
    /// the other children keep their proportions.
    fn insert_child(&mut self, pos: usize, child: K) {
        let len = self.weights.len() as u64;
        let total: u64 = self.weights.iter().map(|&w| u64::from(w)).sum();
        let weight = (total + len / 2)
            .checked_div(len)
            .map_or(1, |average| average.max(1) as u32);
        self.children.insert(pos, child);
        self.weights.insert(pos, weight);
    }

    fn push_child(&mut self, child: K) {
        self.insert_child(self.children.len(), child);
    }

    fn remove_child(&mut self, pos: usize) -> K {
        self.weights.remove(pos);
        self.children.remove(pos)
    }

    /// Cells the children share along the layout axis, before the last child's rounding.
    fn used_extent(&self) -> usize {
        match self.layout {
            Layout::Horizontal => usize::from(self.area.height),
            Layout::Vertical => {
                usize::from(self.area.width).saturating_sub(self.children.len().saturating_sub(2))
            }
        }
    }

    /// Each child's share of `used` cells. The last child takes what rounding leaves.
    fn shares(&self, used: usize, minimums: &[usize]) -> Vec<usize> {
        let total: u64 = self.weights.iter().map(|&w| u64::from(w)).sum();
        let mut shares: Vec<usize> = self
            .weights
            .iter()
            .map(|&w| (used as u64 * u64::from(w) / total.max(1)) as usize)
            .collect();
        if let Some((last, rest)) = shares.split_last_mut() {
            *last = used.saturating_sub(rest.iter().sum());
        }
        // An undersized terminal cannot satisfy every minimum. Keep the weighted
        // layout bounded and let it recover when space becomes available again.
        if minimums.iter().sum::<usize>() > used {
            return shares;
        }

        let mut fixed = vec![false; shares.len()];
        let mut remaining = used;
        let mut remaining_weight = total;
        loop {
            let mut changed = false;
            for (i, &minimum) in minimums.iter().enumerate() {
                if !fixed[i] && shares[i] < minimum {
                    shares[i] = minimum;
                    fixed[i] = true;
                    remaining -= minimum;
                    remaining_weight -= u64::from(self.weights[i]);
                    changed = true;
                }
            }
            if !changed {
                return shares;
            }

            // Pin children at their minimum and divide the rest proportionally.
            let mut allocated = 0;
            let mut last = None;
            for (i, &weight) in self.weights.iter().enumerate() {
                if !fixed[i] {
                    shares[i] =
                        (remaining as u64 * u64::from(weight) / remaining_weight.max(1)) as usize;
                    allocated += shares[i];
                    last = Some(i);
                }
            }
            if let Some(last) = last {
                shares[last] += remaining - allocated;
            }
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
        Self::with_minimum_size(area, 0, 0)
    }

    /// Keep each pane at least this size whenever the available area permits it.
    /// Weights are retained while constrained, so a larger area restores proportions.
    pub fn with_minimum_size(area: Rect, min_pane_width: u16, min_pane_height: u16) -> Self {
        let root = Node::container(Layout::Vertical);

        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(root);

        // The root is its own parent.
        nodes[root].parent = root;

        Self {
            root,
            focus: root,
            area,
            min_pane_width,
            min_pane_height,
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

        container.insert_child(pos, node);
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
            container.insert_child(pos, node);
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
            container.push_child(focus);
            container.push_child(node);
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
            container.remove_child(pos);
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
            // Merge the only child back into its grandparent, where it takes
            // over the removed container's size.
            let sibling = parent_container.remove_child(0);
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
            let minimums = match &self.nodes[key].content {
                Content::Container(container) => self.minimum_shares(container),
                Content::Pane(_) => Vec::new(),
            };
            let node = &mut self.nodes[key];

            match &mut node.content {
                Content::Pane(pane) => {
                    pane.set_area(area);
                }
                Content::Container(container) => {
                    container.area = area;
                    let shares = container.shares(container.used_extent(), &minimums);

                    match container.layout {
                        Layout::Horizontal => {
                            let len = container.children.len();

                            let mut child_y = area.y;

                            for (i, child) in container.children.iter().enumerate() {
                                let height = shares[i] as u16;
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

                            let mut child_x = area.x;

                            for (i, child) in container.children.iter().enumerate() {
                                let width = shares[i] as u16;
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

    /// Grow (positive `delta`) or shrink the focused pane by `delta` cells along `dimension`.
    ///
    /// The nearest enclosing split that divides `dimension` changes: the pane (or the
    /// container holding it) takes cells from the following siblings first, nearest first,
    /// then from the preceding ones. Shrinking gives the cells to the next sibling, or to
    /// the previous one for the last child. Respects the tree's minimum pane size.
    /// Returns whether any size changed.
    pub fn resize_focus(&mut self, dimension: Dimension, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }
        let Some((parent, idx)) = self.enclosing_split(self.focus, dimension.layout()) else {
            return false;
        };
        let container = self.container_ref(parent);
        let len = container.children.len();
        let mins = self.minimum_shares(container);
        let mut shares = container.shares(container.used_extent(), &mins);

        let amount = delta.unsigned_abs();
        if delta > 0 {
            let donors = (idx + 1..len).chain((0..idx).rev());
            let mut wanted = amount;
            for donor in donors {
                let take = wanted.min(shares[donor].saturating_sub(mins[donor]));
                shares[donor] -= take;
                shares[idx] += take;
                wanted -= take;
                if wanted == 0 {
                    break;
                }
            }
            if wanted == amount {
                return false;
            }
        } else {
            let give = amount.min(shares[idx].saturating_sub(mins[idx]));
            if give == 0 {
                return false;
            }
            let receiver = if idx + 1 < len { idx + 1 } else { idx - 1 };
            shares[idx] -= give;
            shares[receiver] += give;
        }

        let container = self.container_mut(parent);
        container.weights = shares
            .into_iter()
            .map(|share| share.max(1) as u32)
            .collect();
        self.recalculate();
        true
    }

    /// Grow toward `direction` by taking up to `cells` from the adjacent subtree.
    /// At the outer edge, shrink from the opposite side instead. Search outward
    /// through nested splits before falling back, and stop at subtree minimums.
    /// An undersized layout is left unchanged until its minimums fit again.
    pub fn resize_focus_direction(&mut self, direction: Direction, cells: usize) -> bool {
        if cells == 0 {
            return false;
        }
        let (layout, forward) = match direction {
            Direction::Left => (Layout::Vertical, false),
            Direction::Right => (Layout::Vertical, true),
            Direction::Up => (Layout::Horizontal, false),
            Direction::Down => (Layout::Horizontal, true),
        };
        let mut child = self.focus;
        let mut fallback = None;
        let mut target = None;
        while let Some((parent, idx)) = self.enclosing_split(child, layout) {
            let len = self.container_ref(parent).children.len();
            if (forward && idx + 1 < len) || (!forward && idx > 0) {
                let neighbor = if forward { idx + 1 } else { idx - 1 };
                target = Some((parent, neighbor, idx));
                break;
            }
            // This split has no neighbor in the requested direction, but its
            // opposite boundary can move if no enclosing split has one either.
            fallback.get_or_insert((parent, idx, if forward { idx - 1 } else { idx + 1 }));
            child = parent;
        }
        let Some((parent, donor, receiver)) = target.or(fallback) else {
            return false;
        };
        let container = self.container_ref(parent);
        let minimums = self.minimum_shares(container);
        let used = container.used_extent();
        if minimums.iter().sum::<usize>() > used {
            return false;
        }
        let mut shares = container.shares(used, &minimums);
        let amount = cells.min(shares[donor].saturating_sub(minimums[donor]));
        if amount == 0 {
            return false;
        }
        shares[donor] -= amount;
        shares[receiver] += amount;
        self.container_mut(parent).weights = shares.into_iter().map(|share| share as u32).collect();
        self.recalculate();
        true
    }

    /// Give every child of every split the same size again.
    pub fn equalize(&mut self) {
        for node in self.nodes.values_mut() {
            if let Content::Container(container) = &mut node.content {
                container.weights.fill(1);
            }
        }
        self.recalculate();
    }

    /// The nearest container with `layout` above `id`, and the position of the child on the
    /// path to `id` inside it. Containers with a single child do not count.
    fn enclosing_split(&self, id: K, layout: Layout) -> Option<(K, usize)> {
        let mut child = id;
        loop {
            let parent = self.nodes[child].parent;
            if parent == child {
                return None;
            }
            let container = self.container_ref(parent);
            if container.layout == layout && container.children.len() > 1 {
                let idx = container.children.iter().position(|&c| c == child)?;
                return Some((parent, idx));
            }
            child = parent;
        }
    }

    /// Each child's minimum share, including the last column's leading gap.
    fn minimum_shares(&self, container: &Container<K>) -> Vec<usize> {
        let dimension = match container.layout {
            Layout::Vertical => Dimension::Width,
            Layout::Horizontal => Dimension::Height,
        };
        let mut minimums: Vec<_> = container
            .children
            .iter()
            .map(|&child| usize::from(self.min_extent(child, dimension)))
            .collect();
        if dimension == Dimension::Width && minimums.len() > 1 {
            *minimums.last_mut().unwrap() += 1;
        }
        minimums
    }

    /// The fewest cells `id` can occupy along `dimension` at the minimum pane size.
    fn min_extent(&self, id: K, dimension: Dimension) -> u16 {
        match &self.nodes[id].content {
            Content::Pane(_) => match dimension {
                Dimension::Width => self.min_pane_width,
                Dimension::Height => self.min_pane_height,
            },
            Content::Container(container) => {
                let mins = container
                    .children
                    .iter()
                    .map(|&child| self.min_extent(child, dimension));
                if container.layout == dimension.layout() {
                    let gaps = match dimension {
                        Dimension::Width => container.children.len().saturating_sub(1) as u16,
                        Dimension::Height => 0,
                    };
                    mins.fold(gaps, u16::saturating_add)
                } else {
                    mins.max().unwrap_or(0)
                }
            }
        }
    }

    fn container_ref(&self, index: K) -> &Container<K> {
        match &self.nodes[index].content {
            Content::Container(container) => container,
            Content::Pane(_) => unreachable!("split parents are containers"),
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

    fn widths(tree: &Tree) -> Vec<u16> {
        tree.traverse().map(|(_, pane)| pane.area.width).collect()
    }

    fn heights(tree: &Tree) -> Vec<u16> {
        tree.traverse().map(|(_, pane)| pane.area.height).collect()
    }

    fn columns(width: u16, count: usize, minimum: u16) -> (Tree, Vec<PaneId>) {
        let mut tree = Tree::with_minimum_size(Rect::new(0, 0, width, 24), minimum, 2);
        let mut ids = vec![tree.insert(TestPane::new(ContentId::default()))];
        for _ in 1..count {
            ids.push(tree.split(TestPane::new(ContentId::default()), Layout::Vertical));
        }
        (tree, ids)
    }

    #[test]
    fn directional_resize_uses_only_the_neighbor_on_the_requested_side() {
        let (mut tree, ids) = columns(80, 3, 5);
        tree.focus = ids[1];
        assert!(tree.resize_focus_direction(Direction::Left, 4));
        assert_eq!(widths(&tree), [22, 30, 26]);
        assert!(tree.resize_focus_direction(Direction::Right, 6));
        assert_eq!(widths(&tree), [22, 36, 20]);
        assert!(tree.resize_focus_direction(Direction::Right, usize::MAX));
        assert_eq!(widths(&tree), [22, 51, 5]);
        // A blocked boundary must neither draw from the other side nor reverse.
        assert!(!tree.resize_focus_direction(Direction::Right, 1));
        assert!(!tree.resize_focus_direction(Direction::Left, 0));
        assert_eq!(widths(&tree), [22, 51, 5]);
    }

    #[test]
    fn directional_resize_preserves_zero_minimum_and_unrelated_siblings() {
        let (mut tree, ids) = columns(80, 3, 0);
        tree.focus = ids[1];
        assert!(tree.resize_focus_direction(Direction::Left, usize::MAX));
        assert_eq!(widths(&tree), [0, 52, 26]);
        assert!(!tree.resize_focus_direction(Direction::Left, 1));
        tree.focus = ids[0];
        assert!(tree.resize_focus_direction(Direction::Right, 3));
        assert_eq!(widths(&tree), [3, 49, 26]);
    }

    #[test]
    fn directional_resize_reverses_at_each_outer_edge() {
        for (direction, layout, first) in [
            (Direction::Left, Layout::Vertical, true),
            (Direction::Right, Layout::Vertical, false),
            (Direction::Up, Layout::Horizontal, true),
            (Direction::Down, Layout::Horizontal, false),
        ] {
            let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 81, 24), 5, 2);
            let a = tree.insert(TestPane::new(ContentId::default()));
            let b = tree.split(TestPane::new(ContentId::default()), layout);
            tree.focus = if first { a } else { b };
            let before = tree.get(tree.focus).area;
            assert!(tree.resize_focus_direction(direction, 3));
            let after = tree.get(tree.focus).area;
            if layout == Layout::Vertical {
                assert_eq!(after.width, before.width - 3);
                assert_eq!(after.height, before.height);
            } else {
                assert_eq!(after.height, before.height - 3);
                assert_eq!(after.width, before.width);
            }
            assert!(tree.resize_focus_direction(direction, usize::MAX));
            assert!(!tree.resize_focus_direction(direction, 1));
        }
    }

    #[test]
    fn directional_resize_finds_the_outer_neighbor_before_reversing() {
        // A and B are in a column split nested inside a row in the left column.
        // | A | B | R |
        // |   C   |   |
        let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 121, 24), 5, 2);
        let a = tree.insert(TestPane::new(ContentId::default()));
        let r = tree.split(TestPane::new(ContentId::default()), Layout::Vertical);
        tree.focus = a;
        tree.split(TestPane::new(ContentId::default()), Layout::Horizontal);
        tree.focus = a;
        let b = tree.split(TestPane::new(ContentId::default()), Layout::Vertical);
        let before_a = tree.get(a).area;
        let before_b = tree.get(b).area;
        let before_r = tree.get(r).area;
        assert!(tree.resize_focus_direction(Direction::Right, 10));
        assert_eq!(tree.get(r).area.width, before_r.width - 10);
        assert!(tree.get(a).area.width > before_a.width);
        assert!(tree.get(b).area.width > before_b.width);
        // Resizing R left must leave enough space for both nested panes and their gap.
        tree.focus = r;
        assert!(tree.resize_focus_direction(Direction::Left, usize::MAX));
        assert_eq!(tree.get(a).area.width, 5);
        assert_eq!(tree.get(b).area.width, 5);
    }

    #[test]
    fn directional_resize_handles_missing_axes_and_undersized_terminals() {
        let (mut tree, ids) = columns(81, 2, 10);
        tree.focus = ids[1];
        assert!(!tree.resize_focus_direction(Direction::Up, 5));
        let before = widths(&tree);
        tree.resize(Rect::new(0, 0, 10, 1));
        let constrained = widths(&tree);
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert!(!tree.resize_focus_direction(direction, usize::MAX));
            assert_eq!(widths(&tree), constrained);
        }
        tree.resize(Rect::new(0, 0, 81, 24));
        assert_eq!(widths(&tree), before);
        assert!(tree.resize_focus_direction(Direction::Left, 7));
        assert_eq!(widths(&tree), [before[0] - 7, before[1] + 7]);
        tree.resize(Rect::new(0, 0, 161, 24));
        assert_eq!(widths(&tree), [65, 95]);
        tree.equalize();
        assert_eq!(widths(&tree), [80, 80]);
        tree.remove(ids[0]);
        assert!(!tree.resize_focus_direction(Direction::Left, 5));
    }

    #[test]
    fn grow_takes_from_the_next_column_then_the_previous_ones() {
        // | 26 | 26 | 26 | with two gaps on 80 columns.
        let (mut tree, ids) = columns(80, 3, 5);
        assert_eq!(widths(&tree), [26, 26, 26]);

        tree.focus = ids[1];
        assert!(tree.resize_focus(Dimension::Width, 4));
        assert_eq!(widths(&tree), [26, 30, 22]);

        // The next column stops at the minimum, the rest comes from the previous one.
        assert!(tree.resize_focus(Dimension::Width, 20));
        assert_eq!(widths(&tree), [23, 50, 5]);

        // Every neighbour is at the minimum, so nothing changes.
        assert!(tree.resize_focus(Dimension::Width, 30));
        assert_eq!(widths(&tree), [5, 68, 5]);
        assert!(!tree.resize_focus(Dimension::Width, 1));
        assert_eq!(widths(&tree), [5, 68, 5]);
    }

    #[test]
    fn shrink_gives_to_the_next_column_or_the_previous_one_for_the_last() {
        let (mut tree, ids) = columns(80, 3, 5);

        tree.focus = ids[0];
        assert!(tree.resize_focus(Dimension::Width, -6));
        assert_eq!(widths(&tree), [20, 32, 26]);

        tree.focus = ids[2];
        assert!(tree.resize_focus(Dimension::Width, -6));
        assert_eq!(widths(&tree), [20, 38, 20]);

        assert!(tree.resize_focus(Dimension::Width, -100));
        assert_eq!(widths(&tree), [20, 53, 5]);
        assert!(!tree.resize_focus(Dimension::Width, -1));
    }

    #[test]
    fn resizing_keeps_panes_inside_the_container() {
        let (mut tree, ids) = columns(80, 4, 3);
        for (step, &id) in ids.iter().cycle().take(40).enumerate() {
            tree.focus = id;
            let delta = if step % 3 == 0 { -7 } else { 9 };
            tree.resize_focus(Dimension::Width, delta);
            let panes: Vec<_> = tree.traverse().map(|(_, pane)| pane.area).collect();
            for pair in panes.windows(2) {
                assert_eq!(
                    pair[0].right() + 1,
                    pair[1].left(),
                    "one gap between columns"
                );
            }
            assert_eq!(panes[0].left(), 0);
            assert_eq!(panes.last().unwrap().right(), 80);
            assert!(panes.iter().all(|area| area.width >= 3), "{panes:?}");
        }
    }

    #[test]
    fn height_resizes_the_enclosing_row_split() {
        // | L0 | R0 |
        // | L1 |    |
        let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 80, 24), 5, 2);
        let l0 = tree.insert(TestPane::new(ContentId::default()));
        let r0 = tree.split(TestPane::new(ContentId::default()), Layout::Vertical);
        tree.focus = l0;
        let l1 = tree.split(TestPane::new(ContentId::default()), Layout::Horizontal);
        assert_eq!(heights(&tree), [12, 12, 24]);

        tree.focus = l1;
        assert!(tree.resize_focus(Dimension::Height, 5));
        assert_eq!(heights(&tree), [7, 17, 24]);

        // R0 has no row split around it.
        tree.focus = r0;
        assert!(!tree.resize_focus(Dimension::Height, 5));
        assert_eq!(heights(&tree), [7, 17, 24]);

        // Width from L1 grows the left column, which holds L0 and L1.
        tree.focus = l1;
        assert!(tree.resize_focus(Dimension::Width, 10));
        assert_eq!(widths(&tree), [50, 50, 29]);
    }

    #[test]
    fn a_nested_split_keeps_room_for_each_of_its_panes() {
        // | L0 | L2 | R0 |  where L0 and L2 sit in a column split under a row split.
        let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 80, 24), 5, 2);
        let l0 = tree.insert(TestPane::new(ContentId::default()));
        let r0 = tree.split(TestPane::new(ContentId::default()), Layout::Vertical);
        tree.focus = l0;
        tree.split(TestPane::new(ContentId::default()), Layout::Horizontal);
        tree.focus = l0;
        tree.split(TestPane::new(ContentId::default()), Layout::Vertical);

        tree.focus = r0;
        assert!(tree.resize_focus(Dimension::Width, 100));
        // The left column keeps two 5-cell panes and the gap between them.
        let left = tree.get(l0).area;
        assert_eq!(left.width, 5);
        assert_eq!(tree.get(r0).area.left(), 12);
    }

    #[test]
    fn new_and_removed_panes_keep_the_other_proportions() {
        let (mut tree, ids) = columns(81, 2, 5);
        tree.focus = ids[0];
        assert!(tree.resize_focus(Dimension::Width, 20));
        assert_eq!(widths(&tree), [60, 20]);

        // A new column takes a third, the old ones keep their 3:1 ratio.
        tree.focus = ids[1];
        let third = tree.split(TestPane::new(ContentId::default()), Layout::Vertical);
        assert_eq!(widths(&tree), [39, 13, 27]);

        tree.remove(third);
        assert_eq!(widths(&tree), [60, 20]);

        // A terminal resize scales the split.
        tree.resize(Rect::new(0, 0, 41, 24));
        assert_eq!(widths(&tree), [30, 10]);
    }

    #[test]
    fn equalize_restores_the_even_split() {
        let (mut tree, ids) = columns(80, 3, 5);
        tree.focus = ids[0];
        tree.split(TestPane::new(ContentId::default()), Layout::Horizontal);
        let even = (widths(&tree), heights(&tree));

        tree.resize_focus(Dimension::Width, 11);
        tree.resize_focus(Dimension::Height, 4);
        assert_ne!((widths(&tree), heights(&tree)), even);

        tree.equalize();
        assert_eq!((widths(&tree), heights(&tree)), even);
    }

    #[test]
    fn ancestor_resizes_preserve_minimum_sizes_in_weighted_descendants() {
        for layout in [Layout::Vertical, Layout::Horizontal] {
            let (dimension, perpendicular, step) = match layout {
                Layout::Vertical => (Dimension::Width, Layout::Horizontal, 10),
                Layout::Horizontal => (Dimension::Height, Layout::Vertical, 2),
            };
            let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 120, 24), 10, 2);
            let first = tree.insert(TestPane::new(ContentId::default()));
            let outer = tree.split(TestPane::new(ContentId::default()), layout);
            tree.focus = first;
            tree.split(TestPane::new(ContentId::default()), perpendicular);
            tree.focus = first;
            let inner = tree.split(TestPane::new(ContentId::default()), layout);

            tree.focus = first;
            assert!(tree.resize_focus(dimension, step));
            let extent = |area: Rect| match dimension {
                Dimension::Width => area.width,
                Dimension::Height => area.height,
            };
            assert!(extent(tree.get(first).area) > extent(tree.get(inner).area));
            tree.focus = outer;
            assert!(tree.resize_focus(dimension, isize::MAX));
            assert_eq!(extent(tree.get(first).area), step as u16);
            assert_eq!(extent(tree.get(inner).area), step as u16);
            assert!(tree
                .panes()
                .all(|(pane, _)| pane.area.width >= 10 && pane.area.height >= 2));
        }
    }

    #[test]
    fn terminal_resizes_preserve_minimum_sizes_and_restore_proportions() {
        let original = Rect::new(10, 20, 81, 25);
        for layout in [Layout::Vertical, Layout::Horizontal] {
            let dimension = match layout {
                Layout::Vertical => Dimension::Width,
                Layout::Horizontal => Dimension::Height,
            };
            let mut tree = Tree::with_minimum_size(original, 10, 2);
            let first = tree.insert(TestPane::new(ContentId::default()));
            tree.split(TestPane::new(ContentId::default()), layout);
            tree.focus = first;
            assert!(tree.resize_focus(dimension, isize::MAX));
            let before = (widths(&tree), heights(&tree));

            tree.resize(Rect::new(10, 20, 41, 9));
            assert!(tree
                .panes()
                .all(|(pane, _)| pane.area.width >= 10 && pane.area.height >= 2));
            tree.resize(original);
            assert_eq!((widths(&tree), heights(&tree)), before);

            // Below the feasible minimum, panes remain bounded and saved weights
            // still restore their proportions when the terminal grows again.
            for size in 0..3 {
                let tiny = Rect::new(10, 20, size, size);
                tree.resize(tiny);
                for (pane, _) in tree.panes() {
                    assert!(pane.area.left() >= tiny.left());
                    assert!(pane.area.top() >= tiny.top());
                    assert!(pane.area.right() <= tiny.right());
                    assert!(pane.area.bottom() <= tiny.bottom());
                }
                tree.resize(original);
                assert_eq!((widths(&tree), heights(&tree)), before);
            }
        }
    }

    #[test]
    fn new_splits_preserve_minimum_sizes_and_removal_restores_proportions() {
        for layout in [Layout::Vertical, Layout::Horizontal] {
            let dimension = match layout {
                Layout::Vertical => Dimension::Width,
                Layout::Horizontal => Dimension::Height,
            };
            let mut tree = Tree::with_minimum_size(Rect::new(0, 0, 120, 24), 10, 2);
            let first = tree.insert(TestPane::new(ContentId::default()));
            tree.split(TestPane::new(ContentId::default()), layout);
            tree.focus = first;
            assert!(tree.resize_focus(dimension, isize::MAX));
            let before = (widths(&tree), heights(&tree));

            // Both insertion paths must constrain the old, smaller sibling.
            for split in [true, false] {
                tree.focus = first;
                let pane = TestPane::new(ContentId::default());
                let added = if split {
                    tree.split(pane, layout)
                } else {
                    tree.insert(pane)
                };
                assert!(tree
                    .panes()
                    .all(|(pane, _)| pane.area.width >= 10 && pane.area.height >= 2));
                tree.remove(added);
                assert_eq!((widths(&tree), heights(&tree)), before);
            }
        }
    }

    #[test]
    fn resize_commands_use_visible_sizes_after_minimum_constraints() {
        let (mut tree, ids) = columns(81, 2, 10);
        tree.focus = ids[0];
        assert!(tree.resize_focus(Dimension::Width, isize::MAX));
        assert_eq!(widths(&tree), [70, 10]);
        tree.resize(Rect::new(0, 0, 41, 24));
        assert_eq!(widths(&tree), [30, 10]);

        // A one-cell step uses the constrained widths, not the saved weights.
        tree.focus = ids[1];
        assert!(tree.resize_focus(Dimension::Width, 1));
        assert_eq!(widths(&tree), [29, 11]);
        assert!(tree.resize_focus(Dimension::Width, isize::MIN));
        assert_eq!(widths(&tree), [30, 10]);
        assert!(!tree.resize_focus(Dimension::Width, -1));
    }

    #[test]
    fn a_single_pane_does_not_resize() {
        let (mut tree, _) = columns(80, 1, 5);
        assert!(!tree.resize_focus(Dimension::Width, 5));
        assert!(!tree.resize_focus(Dimension::Height, 5));
        assert_eq!(widths(&tree), [80]);
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

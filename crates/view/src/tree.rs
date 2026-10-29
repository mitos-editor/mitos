//! Editor split layout adapter. Structural changes remain owned by `Editor`.
use crate::{graphics::Rect, View, ViewId};
use pane_layout::SplitStrategy;
pub use pane_layout::{Direction, Layout};
pub type Traverse<'a> = pane_layout::Traverse<'a, ViewId, View>;

impl pane_layout::Pane<ViewId> for View {
    fn id(&self) -> ViewId {
        self.id
    }
    fn set_id(&mut self, id: ViewId) {
        self.id = id;
    }
    fn area(&self) -> Rect {
        self.area
    }
    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }
}

#[derive(Debug)]
pub struct Tree {
    layout: pane_layout::Tree<ViewId, View>,
}
impl Tree {
    pub(crate) fn new(area: Rect, strategy: SplitStrategy) -> Self {
        let mut layout = pane_layout::Tree::new(area);
        layout.set_split_strategy(strategy);
        Self { layout }
    }
    pub fn focus(&self) -> ViewId {
        self.layout.focus()
    }
    pub(crate) fn set_focus(&mut self, id: ViewId) {
        self.layout.set_focus(id);
    }
    pub(crate) fn insert(&mut self, view: View) -> ViewId {
        self.layout.insert(view)
    }
    pub(crate) fn split(&mut self, view: View, layout: Layout) -> ViewId {
        self.layout.split(view, layout)
    }
    pub(crate) fn remove(&mut self, id: ViewId) {
        self.layout.remove(id);
    }
    pub fn views(&self) -> impl Iterator<Item = (&View, bool)> {
        self.layout.panes()
    }
    pub fn views_mut(&mut self) -> impl Iterator<Item = (&mut View, bool)> {
        self.layout.panes_mut()
    }
    pub fn get(&self, id: ViewId) -> &View {
        self.layout.get(id)
    }
    pub fn try_get(&self, id: ViewId) -> Option<&View> {
        self.layout.try_get(id)
    }
    pub fn get_mut(&mut self, id: ViewId) -> &mut View {
        self.layout.get_mut(id)
    }
    pub fn contains(&self, id: ViewId) -> bool {
        self.layout.contains(id)
    }
    pub fn is_empty(&self) -> bool {
        self.layout.is_empty()
    }
    pub fn resize(&mut self, area: Rect) -> bool {
        self.layout.resize(area)
    }
    pub fn recalculate(&mut self) {
        self.layout.recalculate();
    }
    pub(crate) fn set_split_strategy(&mut self, strategy: SplitStrategy) {
        self.layout.set_split_strategy(strategy);
    }
    pub fn traverse(&self) -> Traverse<'_> {
        self.layout.traverse()
    }
    pub fn find_split_in_direction(&self, id: ViewId, direction: Direction) -> Option<ViewId> {
        self.layout.find_split_in_direction(id, direction)
    }
    pub fn prev(&self) -> ViewId {
        self.layout.prev()
    }
    pub fn next(&self) -> ViewId {
        self.layout.next()
    }
    pub fn transpose(&mut self) {
        self.layout.transpose();
    }
    pub fn swap_split_in_direction(&mut self, direction: Direction) -> Option<()> {
        self.layout.swap_split_in_direction(direction)
    }
    pub fn area(&self) -> Rect {
        self.layout.area()
    }
}

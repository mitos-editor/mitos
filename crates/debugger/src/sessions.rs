//! Generational session membership and explicit selection history.
use slotmap::{Key, SlotMap};

pub struct Sessions<K: Key, T> {
    entries: SlotMap<K, T>,
    selection_order: Vec<K>,
    next_number: u64,
}

impl<K: Key, T> Default for Sessions<K, T> {
    fn default() -> Self {
        Self {
            entries: SlotMap::with_key(),
            selection_order: Vec::new(),
            next_number: 0,
        }
    }
}

impl<K: Key, T> Sessions<K, T> {
    pub fn insert(&mut self, select: bool, create: impl FnOnce(K, u64) -> T) -> K {
        self.next_number = self
            .next_number
            .checked_add(1)
            .expect("debug session numbers exhausted");
        let number = self.next_number;
        let id = self.entries.insert_with_key(|id| create(id, number));
        if select || self.selection_order.is_empty() {
            self.selection_order.push(id);
        } else {
            self.selection_order.insert(0, id);
        }
        id
    }

    /// Select a live session, returning whether the selection changed.
    pub fn select(&mut self, id: K) -> anyhow::Result<bool> {
        anyhow::ensure!(self.entries.contains_key(id), "Debugging session has ended");
        if self.active_id() == Some(id) {
            return Ok(false);
        }
        self.selection_order.retain(|session| *session != id);
        self.selection_order.push(id);
        Ok(true)
    }

    pub fn remove(&mut self, id: K) -> Option<T> {
        let removed = self.entries.remove(id)?;
        self.selection_order.retain(|session| *session != id);
        Some(removed)
    }

    pub fn active_id(&self) -> Option<K> {
        self.selection_order.last().copied()
    }
    pub fn active(&self) -> Option<&T> {
        self.get(self.active_id()?)
    }
    pub fn get(&self, id: K) -> Option<&T> {
        self.entries.get(id)
    }
    pub fn get_mut(&mut self, id: K) -> Option<&mut T> {
        self.entries.get_mut(id)
    }
    pub fn contains_key(&self, id: K) -> bool {
        self.entries.contains_key(id)
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (K, &T)> {
        self.entries.iter()
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.values_mut()
    }
    /// Most recently selected first, including unselected child sessions.
    pub fn by_selection(&self) -> impl ExactSizeIterator<Item = &T> {
        self.selection_order
            .iter()
            .rev()
            .map(|id| &self.entries[*id])
    }
}

impl<K: Key, T> std::ops::Index<K> for Sessions<K, T> {
    type Output = T;
    fn index(&self, id: K) -> &T {
        &self.entries[id]
    }
}
impl<K: Key, T> std::ops::IndexMut<K> for Sessions<K, T> {
    fn index_mut(&mut self, id: K) -> &mut T {
        &mut self.entries[id]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_history_rejects_reused_ids_and_preserves_child_selection() {
        let mut sessions = Sessions::<slotmap::DefaultKey, _>::default();
        let parent = sessions.insert(true, |_, number| number);
        let child = sessions.insert(false, |_, number| number);
        assert_eq!(sessions.active_id(), Some(parent));
        assert!(sessions.select(child).unwrap());
        sessions.remove(child);
        assert_eq!(sessions.active_id(), Some(parent));
        let replacement = sessions.insert(true, |_, number| number);
        assert_ne!(replacement, child);
        assert!(sessions.select(child).is_err());
        assert_eq!(
            sessions.by_selection().copied().collect::<Vec<_>>(),
            vec![3, 1]
        );
    }
}

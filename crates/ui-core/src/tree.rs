//! Retained expansion state and lazy, depth-first rows for hierarchical views.

use std::{collections::HashSet, hash::Hash};

/// Node identities must be stable across reloads. Children are requested only for
/// expanded nodes, so a collapsed subtree need not be loaded or kept in memory.
#[derive(Debug, Clone)]
pub struct Tree<K> {
    expanded: HashSet<K>,
}

impl<K> Default for Tree<K> {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
        }
    }
}

/// Keep matching rows and their ancestors in depth-first order. Descendants of
/// matching directories still have to match independently.
pub fn filter_rows<T>(
    rows: Vec<(T, usize)>,
    mut matches: impl FnMut(&T) -> bool,
) -> Vec<(T, usize)> {
    let mut keep = vec![false; rows.len()];
    let mut ancestors = Vec::new();
    for (index, (node, depth)) in rows.iter().enumerate() {
        ancestors.truncate(*depth);
        if matches(node) {
            keep[index] = true;
            for &ancestor in &ancestors {
                keep[ancestor] = true;
            }
        }
        ancestors.push(index);
    }
    rows.into_iter()
        .zip(keep)
        .filter_map(|(row, keep)| keep.then_some(row))
        .collect()
}

impl<K: Eq + Hash> Tree<K> {
    pub fn is_expanded(&self, key: &K) -> bool {
        self.expanded.contains(key)
    }

    pub fn toggle(&mut self, key: K) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
    }

    /// Flatten a forest in sibling order, keeping each subtree next to its parent.
    /// The loader returns no children for leaves. Collapsing a parent preserves
    /// its descendants' expansion state for the next time it is opened.
    pub fn rows<T, E>(
        &self,
        roots: Vec<T>,
        key: impl Fn(&T) -> &K,
        mut children: impl FnMut(&T) -> Result<Vec<T>, E>,
    ) -> Result<Vec<(T, usize)>, E> {
        let mut pending: Vec<_> = roots.into_iter().rev().map(|node| (node, 0)).collect();
        let mut rows = Vec::new();
        while let Some((node, depth)) = pending.pop() {
            if self.is_expanded(key(&node)) {
                pending.extend(
                    children(&node)?
                        .into_iter()
                        .rev()
                        .map(|child| (child, depth + 1)),
                );
            }
            rows.push((node, depth));
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_keeps_only_matches_and_their_parent_chains() {
        let rows = vec![
            ("root", 0),
            ("left", 1),
            ("match", 2),
            ("other", 2),
            ("right", 1),
            ("unrelated", 2),
        ];
        assert_eq!(
            filter_rows(rows.clone(), |node| *node == "match"),
            [("root", 0), ("left", 1), ("match", 2)]
        );
        assert_eq!(
            filter_rows(rows.clone(), |node| *node == "left"),
            [("root", 0), ("left", 1)]
        );
        assert!(filter_rows(rows, |_| false).is_empty());
    }

    #[test]
    fn expansion_is_lazy_ordered_and_retained_below_collapsed_parents() {
        let mut tree = Tree::default();
        tree.toggle(1);
        tree.toggle(2);
        let rows = |tree: &Tree<i32>| {
            tree.rows(
                vec![1, 5],
                |n| n,
                |n| {
                    Ok::<_, ()>(match n {
                        1 => vec![2, 4],
                        2 => vec![3],
                        _ => panic!("loaded a collapsed node"),
                    })
                },
            )
            .unwrap()
        };
        assert_eq!(rows(&tree), [(1, 0), (2, 1), (3, 2), (4, 1), (5, 0)]);
        tree.toggle(1);
        assert_eq!(rows(&tree), [(1, 0), (5, 0)]);
        tree.toggle(1);
        assert_eq!(rows(&tree), [(1, 0), (2, 1), (3, 2), (4, 1), (5, 0)]);
    }
}

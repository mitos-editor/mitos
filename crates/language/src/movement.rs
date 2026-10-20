//! Movement through syntax nodes and query-defined text objects.
use crate::{syntax, tree_sitter::Node, Syntax};
use editor_core::{
    movement::{Direction, Movement},
    textobject::TextObject,
    Range, RopeSlice, Selection,
};
use std::{borrow::Cow, cmp::Reverse};

/// Finds the range of the next or previous textobject in the syntax tree.
/// Returns the range in the forwards direction.
pub fn goto_treesitter_object(
    slice: RopeSlice,
    range: Range,
    object_name: &str,
    dir: Direction,
    syntax: &Syntax,
    loader: &syntax::Loader,
    count: usize,
) -> Range {
    let get_range = move |range: Range| -> Option<Range> {
        let byte_pos = slice.char_to_byte_idx(range.cursor(slice));

        // Walk the layer at the cursor with that language's own tree and textobject query.
        // Resolved per step so the motion can cross into and out of injected regions.
        let layer = syntax.layer_for_byte_range(byte_pos as u32, byte_pos as u32);
        let slice_tree = syntax
            .tree_for_byte_range(byte_pos as u32, byte_pos as u32)
            .root_node();
        let textobject_query = loader.textobject_query(syntax.layer(layer).language);

        let cap_name = |t: TextObject| format!("{}.{}", object_name, t);
        let nodes = textobject_query?.capture_nodes_any(
            &[
                &cap_name(TextObject::Movement),
                &cap_name(TextObject::Around),
                &cap_name(TextObject::Inside),
            ],
            &slice_tree,
            slice,
        )?;

        let node = match dir {
            Direction::Forward => nodes
                .filter(|n| n.start_byte() > byte_pos)
                .min_by_key(|n| (n.start_byte(), Reverse(n.end_byte())))?,
            Direction::Backward => nodes
                .filter(|n| n.end_byte() < byte_pos)
                .max_by_key(|n| (n.end_byte(), Reverse(n.start_byte())))?,
        };

        let len = slice.len();
        let start_byte = node.start_byte();
        let end_byte = node.end_byte();
        if start_byte >= len || end_byte >= len {
            return None;
        }

        let start_char = slice.byte_to_char_idx(start_byte);
        let end_char = slice.byte_to_char_idx(end_byte);

        // head of range should be at beginning
        Some(Range::new(start_char, end_char))
    };
    let mut last_range = range;
    for _ in 0..count {
        match get_range(last_range) {
            Some(r) if r != last_range => last_range = r,
            _ => break,
        }
    }
    last_range
}

fn find_parent_start<'tree>(node: &Node<'tree>) -> Option<Node<'tree>> {
    let start = node.start_byte();
    let mut node = Cow::Borrowed(node);

    while node.start_byte() >= start || !node.is_named() {
        node = Cow::Owned(node.parent()?);
    }

    Some(node.into_owned())
}

pub fn move_parent_node_end(
    syntax: &Syntax,
    text: RopeSlice,
    selection: Selection,
    dir: Direction,
    movement: Movement,
) -> Selection {
    selection.transform(|range| {
        let start_from = text.char_to_byte_idx(range.from()) as u32;
        let start_to = text.char_to_byte_idx(range.to()) as u32;

        let mut node = match syntax.named_descendant_for_byte_range(start_from, start_to) {
            Some(node) => node,
            None => {
                log::debug!(
                    "no descendant found for byte range: {} - {}",
                    start_from,
                    start_to
                );
                return range;
            }
        };

        let mut end_head = match dir {
            // moving forward, we always want to move one past the end of the
            // current node, so use the end byte of the current node, which is an exclusive
            // end of the range
            Direction::Forward => text.byte_to_char_idx(node.end_byte() as usize),

            // moving backward, we want the cursor to land on the start char of
            // the current node, or if it is already at the start of a node, to traverse up to
            // the parent
            Direction::Backward => {
                let end_head = text.byte_to_char_idx(node.start_byte() as usize);

                // if we're already on the beginning, look up to the parent
                if end_head == range.cursor(text) {
                    node = find_parent_start(&node).unwrap_or(node);
                    text.byte_to_char_idx(node.start_byte() as usize)
                } else {
                    end_head
                }
            }
        };

        if movement == Movement::Move {
            // preserve direction of original range
            if range.direction() == Direction::Forward {
                Range::new(end_head, end_head + 1)
            } else {
                Range::new(end_head + 1, end_head)
            }
        } else {
            // if we end up with a forward range, then adjust it to be one past
            // where we want
            if end_head >= range.anchor {
                end_head += 1;
            }

            Range::new(range.anchor, end_head)
        }
    })
}

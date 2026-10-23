//! Bracket and pair matching through syntax trees.
use crate::{tree_sitter::Node, Syntax};
use editor_core::{
    match_brackets::{find_matching_bracket_plaintext, is_valid_pair, PAIRS},
    movement::Direction::{self, Backward, Forward},
    RopeSlice,
};
use std::iter;
use stdx::rope::RopeExt as _;
const MATCH_LIMIT: usize = 16;

/// Returns the position of the matching bracket under cursor.
///
/// If the cursor is on the opening bracket, the position of
/// the closing bracket is returned. If the cursor on the closing
/// bracket, the position of the opening bracket is returned.
///
/// If the cursor is not on a bracket, `None` is returned.
///
/// If no matching bracket is found, `None` is returned.
#[must_use]
pub fn find_matching_bracket(syntax: &Syntax, doc: RopeSlice, pos: usize) -> Option<usize> {
    if pos >= doc.len_chars() || !is_valid_pair(doc.char_at(pos)) {
        return None;
    }
    find_pair(syntax, doc, pos, false)
}

// Returns the position of the bracket that is closing the current scope.
//
// If the cursor is on an opening or closing bracket, the function
// behaves equivalent to [`find_matching_bracket`].
//
// If the cursor position is within a scope, the function searches
// for the surrounding scope that is surrounded by brackets and
// returns the position of the closing bracket for that scope.
//
// If no surrounding scope is found, the function returns `None`.
#[must_use]
pub fn find_matching_bracket_fuzzy(syntax: &Syntax, doc: RopeSlice, pos: usize) -> Option<usize> {
    find_pair(syntax, doc, pos, true)
}

fn find_pair(
    syntax: &Syntax,
    doc: RopeSlice,
    pos_: usize,
    traverse_parents: bool,
) -> Option<usize> {
    let pos = doc.char_to_byte_idx(pos_) as u32;

    let root = syntax.tree_for_byte_range(pos, pos).root_node();
    let mut node = root.descendant_for_byte_range(pos, pos)?;

    loop {
        if node.is_named() && node.child_count() >= 2 {
            let open = node.child(0).unwrap();
            let close = node.child(node.child_count() - 1).unwrap();

            if let (Some((start_pos, open)), Some((end_pos, close))) =
                (as_char(doc, &open), as_char(doc, &close))
                && PAIRS.contains(&(open, close))
                && start_pos <= pos_
                && pos_ <= end_pos
            {
                if end_pos == pos_ {
                    return Some(start_pos);
                }

                // We return the end char if the cursor is either on the start char
                // or at some arbitrary position between start and end char.
                if traverse_parents || start_pos == pos_ {
                    return Some(end_pos);
                }
            }
        }
        // this node itselt wasn't a pair but maybe its siblings are

        if let Some((start_char, end_char)) = as_close_pair(doc, &node)
            && let Some(pair_start) =
                find_pair_end(doc, node.prev_sibling(), start_char, end_char, Backward)
        {
            return Some(pair_start);
        }
        if let Some((start_char, end_char)) = as_open_pair(doc, &node)
            && let Some(pair_end) =
                find_pair_end(doc, node.next_sibling(), start_char, end_char, Forward)
        {
            return Some(pair_end);
        }

        if traverse_parents {
            for sibling in
                iter::successors(node.next_sibling(), |node| node.next_sibling()).take(MATCH_LIMIT)
            {
                let Some((start_char, end_char)) = as_close_pair(doc, &sibling) else {
                    continue;
                };
                if find_pair_end(doc, sibling.prev_sibling(), start_char, end_char, Backward)
                    .is_some()
                {
                    return doc.try_byte_to_char_idx(sibling.start_byte() as usize).ok();
                }
            }
        } else if node.is_named() {
            break;
        }

        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }
    let node = root.named_descendant_for_byte_range(pos, pos + 1)?;
    if node.child_count() != 0 {
        return None;
    }
    let node_start = doc.byte_to_char_idx(node.start_byte() as usize);
    let node_text = doc.slice(node.start_byte() as usize..node.end_byte() as usize);
    find_matching_bracket_plaintext(node_text, pos_.checked_sub(node_start)?)
        .map(|pos| pos + node_start)
}

/// Tests if this node is a pair close char and returns the expected open char
/// and close char contained in this node
fn as_close_pair(doc: RopeSlice, node: &Node) -> Option<(char, char)> {
    let close = as_char(doc, node)?.1;
    PAIRS
        .iter()
        .find_map(|&(open, close_)| (close_ == close).then_some((close, open)))
}

/// Checks if `node` or its siblings (at most MATCH_LIMIT nodes) is the specified closing char
///
/// # Returns
///
/// The position of the found node or `None` otherwise
fn find_pair_end(
    doc: RopeSlice,
    node: Option<Node>,
    start_char: char,
    end_char: char,
    direction: Direction,
) -> Option<usize> {
    let advance = match direction {
        Forward => Node::next_sibling,
        Backward => Node::prev_sibling,
    };
    let mut depth = 0;
    iter::successors(node, advance)
        .take(MATCH_LIMIT)
        .find_map(|node| {
            let (pos, c) = as_char(doc, &node)?;
            if c == end_char {
                if depth == 0 {
                    return Some(pos);
                }
                depth -= 1;
            } else if c == start_char {
                depth += 1;
            }
            None
        })
}

/// Tests if this node is a pair open char and returns the expected close char
/// and open char contained in this node
fn as_open_pair(doc: RopeSlice, node: &Node) -> Option<(char, char)> {
    let open = as_char(doc, node)?.1;
    PAIRS
        .iter()
        .find_map(|&(open_, close)| (open_ == open).then_some((open, close)))
}

/// If node is a single char return it (and its char position)
fn as_char(doc: RopeSlice, node: &Node) -> Option<(usize, char)> {
    // TODO: multi char/non ASCII pairs
    if node.byte_range().len() != 1 {
        return None;
    }
    let pos = doc.try_byte_to_char_idx(node.start_byte() as usize).ok()?;
    Some((pos, doc.char_at(pos)))
}

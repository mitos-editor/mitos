//! Bounded syntax refinement using the source trees already parsed for highlighting.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use editor_core::Rope;
use gix_imara_diff::{Algorithm, Diff, InternedInput};
use tree_house::tree_sitter::{Node, Tree};

const MAX_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 30_000;
const BUDGET: Duration = Duration::from_millis(300);

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;

#[derive(Debug, Default)]
pub(super) struct Changes {
    pub before: Vec<Range<usize>>,
    pub after: Vec<Range<usize>>,
}

struct Budget<'a> {
    deadline: Instant,
    canceled: &'a dyn Fn() -> bool,
}

impl Budget<'_> {
    fn check(&self) -> Option<()> {
        (!(self.canceled)() && Instant::now() < self.deadline).then_some(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key<'a> {
    Enter(u16),
    Atom(u16, &'a str),
    Gap(u16, &'a str),
    Leave(u16),
}

struct Event<'a> {
    key: Key<'a>,
    range: Range<usize>,
}

struct Parent {
    kind: u16,
    end: usize,
    literal_gaps: bool,
}

fn literal_gaps(node: &Node<'_>) -> bool {
    // Some grammars leave literal whitespace out of the tree, including HTML content and
    // all-whitespace shell heredocs. Preserve these gaps even when no content leaf exists.
    match node.kind() {
        "element" | "script_element" | "style_element" | "jsx_element" | "jsx_fragment"
        | "heredoc_redirect" | "heredoc_body" => true,
        "document" => node.children().any(|child| {
            matches!(
                child.kind(),
                "element" | "script_element" | "style_element" | "doctype"
            )
        }),
        _ => false,
    }
}

fn events<'a>(tree: &Tree, source: &'a str, budget: &Budget<'_>) -> Option<Vec<Event<'a>>> {
    // Explicit traversal avoids a Rust call-stack limit on deeply nested input.
    let mut result = Vec::new();
    let mut cursor = tree.walk();
    let mut parents: Vec<Parent> = Vec::new();
    loop {
        budget.check()?;
        if result.len() >= MAX_EVENTS {
            return None;
        }
        let node = cursor.node();
        if node.is_missing() || node.kind() == "ERROR" {
            return None;
        }
        if let Some(parent) = parents.last_mut() {
            gap(
                &mut result,
                source,
                parent.end..node.start_byte() as usize,
                parent.kind,
                parent.literal_gaps,
            );
            parent.end = node.end_byte() as usize;
        }
        // Comments may expose only delimiters as children. Whitespace inside a literal leaf
        // is significant; only layout between parsed nodes can be ignored.
        if node.child_count() == 0 || node.is_extra() {
            let range = range(&node);
            let literal = &source[range.clone()];
            if !literal.is_empty() {
                result.push(Event {
                    key: Key::Atom(node.kind_id(), literal),
                    range,
                });
            }
        } else {
            result.push(Event {
                key: Key::Enter(node.kind_id()),
                range: range(&node),
            });
            parents.push(Parent {
                kind: node.kind_id(),
                end: node.start_byte() as usize,
                literal_gaps: literal_gaps(&node),
            });
            if cursor.goto_first_child() {
                continue;
            }
        }
        loop {
            if result.len() > MAX_EVENTS {
                return None;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Some(result);
            }
            let node = cursor.node();
            let parent = parents.pop().unwrap();
            gap(
                &mut result,
                source,
                parent.end..node.end_byte() as usize,
                parent.kind,
                parent.literal_gaps,
            );
            result.push(Event {
                key: Key::Leave(node.kind_id()),
                range: range(&node),
            });
        }
    }
}

fn gap<'a>(
    events: &mut Vec<Event<'a>>,
    source: &'a str,
    range: Range<usize>,
    kind: u16,
    literal: bool,
) {
    let text = &source[range.clone()];
    if !text.is_empty() && (literal || !text.chars().all(char::is_whitespace)) {
        events.push(Event {
            key: Key::Gap(kind, text),
            range,
        });
    }
}

fn range(node: &Node<'_>) -> Range<usize> {
    node.start_byte() as usize..node.end_byte() as usize
}

fn normalize(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|range| !range.is_empty());
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut result: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = result.last_mut()
            && previous.end >= range.start
        {
            previous.end = previous.end.max(range.end);
        } else {
            result.push(range);
        }
    }
    result
}

pub(super) fn compare(
    before: &Rope,
    after: &Rope,
    before_tree: &Tree,
    after_tree: &Tree,
    canceled: &dyn Fn() -> bool,
) -> Option<Changes> {
    if before.len().max(after.len()) > MAX_BYTES {
        return None;
    }
    let budget = Budget {
        deadline: Instant::now() + BUDGET,
        canceled,
    };
    let before = before.to_string();
    let after = after.to_string();
    let before = events(before_tree, &before, &budget)?;
    let after = events(after_tree, &after, &budget)?;
    let mut input = InternedInput {
        before: Vec::new(),
        after: Vec::new(),
        interner: gix_imara_diff::Interner::new(before.len() + after.len()),
    };
    input.update_before(before.iter().map(|event| event.key));
    input.update_after(after.iter().map(|event| event.key));
    budget.check()?;
    // Histogram bounds repeated-token searches. Event/byte limits bound this non-interruptible
    // step; check the total budget before publishing the result as well.
    let diff = Diff::compute(Algorithm::Histogram, &input);
    budget.check()?;
    let mut changes = Changes::default();
    for hunk in diff.hunks() {
        changes.before.extend(
            before[hunk.before.start as usize..hunk.before.end as usize]
                .iter()
                .map(|event| event.range.clone()),
        );
        changes.after.extend(
            after[hunk.after.start as usize..hunk.after.end as usize]
                .iter()
                .map(|event| event.range.clone()),
        );
    }
    changes.before = normalize(changes.before);
    changes.after = normalize(changes.after);
    budget.check()?;
    Some(changes)
}

//! Optional syntax comparison using the host's grammar, independent of Git and editor state.
//!
//! Compare ordered tree events, including node kinds and literal leaves. Layout between nodes
//! does not participate in matching; strings and comments keep their literal content. Structural
//! changes emphasize the affected subtree. This is syntactic comparison, not move detection or
//! semantic equivalence. Exact textual hunks remain the responsibility of `text-diff`.

use std::{
    ops::{ControlFlow, Range},
    time::{Duration, Instant},
};

use editor_core::Rope;
use imara_diff::{Algorithm, Diff, InternedInput};
use text_diff::{ReviewChanges, ReviewProvider, ReviewUnavailable};
use tree_house::tree_sitter::{Grammar, Node, ParseOptions, ParseState, Parser, RopeInput, Tree};

const MAX_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 30_000;
const BUDGET: Duration = Duration::from_millis(300);

#[cfg(test)]
mod tests;

pub struct SyntaxDiff {
    grammar: Grammar,
}

impl SyntaxDiff {
    pub fn new(grammar: Grammar) -> Self {
        Self { grammar }
    }
}

struct Budget<'a> {
    deadline: Instant,
    canceled: &'a dyn Fn() -> bool,
}

impl Budget<'_> {
    fn check(&self) -> Result<(), ReviewUnavailable> {
        if (self.canceled)() {
            Err(ReviewUnavailable("comparison canceled"))
        } else if Instant::now() >= self.deadline {
            Err(ReviewUnavailable("syntax time budget exceeded"))
        } else {
            Ok(())
        }
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

fn parse(grammar: Grammar, source: &Rope, budget: &Budget<'_>) -> Result<Tree, ReviewUnavailable> {
    budget.check()?;
    let mut parser = Parser::new();
    parser
        .set_grammar(grammar)
        .map_err(|_| ReviewUnavailable("incompatible grammar"))?;
    // Parsers are cached per worker thread and may previously have parsed an injection range.
    parser
        .set_included_ranges(&[])
        .map_err(|_| ReviewUnavailable("invalid parser ranges"))?;
    let mut progress = |_: &ParseState| {
        if budget.check().is_err() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let tree = parser
        .parse_with_options(
            RopeInput::new(source.slice(..)),
            None,
            ParseOptions::new(&mut progress),
        )
        .ok_or(ReviewUnavailable("syntax parsing interrupted"))?;
    budget.check()?;
    Ok(tree)
}

fn events<'a>(
    tree: &Tree,
    source: &'a str,
    budget: &Budget<'_>,
) -> Result<Vec<Event<'a>>, ReviewUnavailable> {
    // Explicit traversal avoids a Rust call-stack limit on deeply nested input.
    let mut result = Vec::new();
    let mut cursor = tree.walk();
    let mut previous_child = Vec::new();
    loop {
        budget.check()?;
        if result.len() >= MAX_EVENTS {
            return Err(ReviewUnavailable("syntax node limit exceeded"));
        }
        let node = cursor.node();
        if node.is_missing() || node.kind() == "ERROR" {
            return Err(ReviewUnavailable("syntax contains parse errors"));
        }
        if let Some(previous) = previous_child.last_mut() {
            gap(
                &mut result,
                source,
                *previous..node.start_byte() as usize,
                node.parent().unwrap().kind_id(),
            );
            *previous = node.end_byte() as usize;
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
            previous_child.push(node.start_byte() as usize);
            if cursor.goto_first_child() {
                continue;
            }
        }
        loop {
            if result.len() > MAX_EVENTS {
                return Err(ReviewUnavailable("syntax node limit exceeded"));
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok(result);
            }
            let node = cursor.node();
            let previous = previous_child.pop().unwrap();
            gap(
                &mut result,
                source,
                previous..node.end_byte() as usize,
                node.kind_id(),
            );
            result.push(Event {
                key: Key::Leave(node.kind_id()),
                range: range(&node),
            });
        }
    }
}

fn gap<'a>(events: &mut Vec<Event<'a>>, source: &'a str, range: Range<usize>, kind: u16) {
    let text = &source[range.clone()];
    if !text.chars().all(char::is_whitespace) {
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

impl ReviewProvider for SyntaxDiff {
    fn name(&self) -> &'static str {
        "syntax"
    }

    fn compare(
        &self,
        before: &Rope,
        after: &Rope,
        canceled: &dyn Fn() -> bool,
    ) -> Result<ReviewChanges, ReviewUnavailable> {
        if before.len().max(after.len()) > MAX_BYTES {
            return Err(ReviewUnavailable("syntax size limit exceeded (512 KiB)"));
        }
        let budget = Budget {
            deadline: Instant::now() + BUDGET,
            canceled,
        };
        let before_tree = parse(self.grammar, before, &budget)?;
        let after_tree = parse(self.grammar, after, &budget)?;
        let before = before.to_string();
        let after = after.to_string();
        let before = events(&before_tree, &before, &budget)?;
        let after = events(&after_tree, &after, &budget)?;
        let mut input = InternedInput {
            before: Vec::new(),
            after: Vec::new(),
            interner: imara_diff::Interner::new(before.len() + after.len()),
        };
        input.update_before(before.iter().map(|event| event.key));
        input.update_after(after.iter().map(|event| event.key));
        budget.check()?;
        // Histogram bounds repeated-token searches. Event/byte limits bound this non-interruptible
        // step; check the total budget before publishing the result as well.
        let diff = Diff::compute(Algorithm::Histogram, &input);
        budget.check()?;
        let mut changes = ReviewChanges::default();
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
        Ok(changes)
    }
}

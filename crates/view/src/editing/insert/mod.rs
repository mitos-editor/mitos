//! Insert-mode text operations over documents and per-view selections.
//!
//! Callers own mode changes, key handling, completion navigation, and frontend
//! events. These operations apply through the shared mutation path and leave
//! undo groups open so consecutive typed edits remain a single undo step.

use super::continued_line_comment_token;
use stdx::rope::RopeExt as _;
mod delete;
use crate::{editing::EditError, Document, ViewId};
pub use delete::{
    delete_char_backward, delete_char_forward, delete_word_backward, delete_word_forward,
    kill_to_line_end, kill_to_line_start,
};
use editor_core::{
    auto_pairs::{self, AutoPairs},
    graphemes,
    indent::IndentStyle,
    line_ending::{get_line_ending, line_end_char_index},
    Change, Range, Rope, RopeSlice, Selection, SmallVec, Tendril, Transaction,
};
use std::collections::HashMap;
use stdx::rope::RopeSliceExt;

/// Which line boundary receives the cursor when entering insert mode.
#[derive(Clone, Copy)]
pub enum LineBoundary {
    LineStart,
    LineEnd,
}

/// Which side of the selected lines receives new lines.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Open {
    Below,
    Above,
}

/// Whether opening lines continues the current language's comment leader.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommentContinuation {
    Enabled,
    Disabled,
}

/// Insert a character at each cursor, respecting configured auto pairs.
pub fn insert_char(
    doc: &mut Document,
    view_id: ViewId,
    c: char,
    global_pairs: Option<&AutoPairs>,
) -> Result<(), EditError> {
    let auto_pairs = doc.auto_pairs(global_pairs, doc.selection(view_id));
    let insert = |text: &editor_core::Rope, selection: &Selection| {
        Transaction::change_by_and_with_selection(text, selection, |range| {
            auto_pairs
                .and_then(|pairs| auto_pairs::hook_insert(text, range, c, pairs))
                .map(|(change, range)| (change, Some(range)))
                .unwrap_or_else(|| {
                    let cursor = range.cursor(text.slice(..));
                    ((cursor, cursor, Some(Tendril::from_iter([c]))), None)
                })
        })
    };
    let transaction = if let Some(replacement) = doc.pending_snippet_placeholder(view_id) {
        let mut text = doc.text().clone();
        replacement.apply(&mut text);
        let selection = doc
            .selection(view_id)
            .clone()
            .map(replacement.changes())
            .ensure_invariants(text.slice(..));
        replacement.compose(insert(&text, &selection))
    } else {
        insert(doc.text(), doc.selection(view_id))
    };
    super::apply(doc, &transaction, view_id)?;
    doc.snippet_input_applied(view_id);
    Ok(())
}

/// Insert indentation up to the next indent stop at each cursor.
pub fn insert_tab(doc: &mut Document, view_id: ViewId, count: usize) -> Result<(), EditError> {
    let transaction = Transaction::change(
        doc.text(),
        doc.selection(view_id).ranges().iter().map(|range| {
            let cursor = range.cursor(doc.text().slice(..));
            let indent = if let IndentStyle::Spaces(indent_width) = doc.indent_style {
                let line = range.cursor_line(doc.text().slice(..));
                let line_start = doc.text().line_to_char_idx(line);
                let offset = (cursor - line_start) % indent_width as usize;

                Tendril::from(doc.indent_style.as_str().repeat(count)).split_off(offset)
            } else {
                Tendril::from(doc.indent_style.as_str().repeat(count))
            };

            (cursor, cursor, Some(indent))
        }),
    );
    super::apply(doc, &transaction, view_id)
}

/// Insert line endings with language-aware indentation and comment continuation.
pub fn insert_newline(
    doc: &mut Document,
    view_id: ViewId,
    global_pairs: Option<&AutoPairs>,
) -> Result<(), EditError> {
    let config = doc.config.load();
    let text = doc.text().slice(..);
    let line_ending = doc.line_ending.as_str();

    let contents = doc.text();
    let selection = doc.selection(view_id);
    let mut ranges = SmallVec::with_capacity(selection.len());

    // TODO: this is annoying, but we need to do it to properly calculate pos after edits
    let mut global_offs = 0;
    let mut new_text = String::new();

    let auto_pairs = doc.auto_pairs(global_pairs, selection);
    let mut last_pos = 0;
    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        // Tracks the number of trailing whitespace characters deleted by this selection.
        let mut chars_deleted = 0;
        let pos = range.cursor(text);

        let prev = if pos == 0 {
            ' '
        } else {
            contents.char_at(pos - 1)
        };
        let curr = contents.get_char_at(pos).unwrap_or(' ');

        let current_line = text.char_to_line_idx(pos);
        let line_start = text.line_to_char_idx(current_line);

        // Continue the comment leader using the comment tokens of the layer at the comment
        // leader (i.e. the first non-whitespace char on the line). Looking up at the cursor
        // would land inside an injected layer (e.g. `comment`, or markdown in a doc comment)
        // and miss the host language's tokens.
        let continue_comment_token = if config.continue_comments {
            text.line(current_line, stdx::rope::LINE_TYPE)
                .first_non_whitespace_char()
                .map(|c| text.char_to_byte_idx(line_start + c))
                .and_then(|byte| continued_line_comment_token(doc, text, current_line, byte))
        } else {
            None
        };

        let (from, to, local_offs) = if let Some(idx) =
            text.slice_chars(line_start..pos).last_non_whitespace_char()
        {
            let first_trailing_whitespace_char = (line_start + idx + 1).clamp(last_pos, pos);
            last_pos = pos;
            let line = text.line(current_line, stdx::rope::LINE_TYPE);

            let indent = match line.first_non_whitespace_char() {
                Some(pos) if continue_comment_token.is_some() => {
                    line.slice_chars(..pos).to_string()
                }
                _ => language::indent::indent_for_newline(
                    doc.syntax_loader(),
                    doc.syntax(),
                    &config.indent.heuristic,
                    &doc.indent_style,
                    doc.tab_width(),
                    text,
                    current_line,
                    pos,
                    current_line,
                ),
            };

            // If we are between pairs (such as brackets), we want to
            // insert an additional line which is indented one level
            // more and place the cursor there
            let on_auto_pair = auto_pairs
                .and_then(|pairs| pairs.get(prev))
                .is_some_and(|pair| pair.open == prev && pair.close == curr);

            let local_offs = if let Some(token) = continue_comment_token {
                new_text.reserve_exact(line_ending.len() + indent.len() + token.len() + 1);
                new_text.push_str(line_ending);
                new_text.push_str(&indent);
                new_text.push_str(token);
                new_text.push(' ');
                new_text.chars().count()
            } else if on_auto_pair {
                // line where the cursor will be
                let inner_indent = indent.clone() + doc.indent_style.as_str();
                new_text.reserve_exact(line_ending.len() * 2 + indent.len() + inner_indent.len());
                new_text.push_str(line_ending);
                new_text.push_str(&inner_indent);

                // line where the matching pair will be
                let local_offs = new_text.chars().count();
                new_text.push_str(line_ending);
                new_text.push_str(&indent);

                local_offs
            } else {
                new_text.reserve_exact(line_ending.len() + indent.len());
                new_text.push_str(line_ending);
                new_text.push_str(&indent);

                new_text.chars().count()
            };

            // Note that `first_trailing_whitespace_char` is at most `pos` so this unsigned
            // subtraction cannot underflow.
            chars_deleted = pos - first_trailing_whitespace_char;

            (
                first_trailing_whitespace_char,
                pos,
                local_offs as isize - chars_deleted as isize,
            )
        } else {
            // If the current line is all whitespace, insert a line ending at the beginning of
            // the current line. This makes the current line empty and the new line contain the
            // indentation of the old line.
            new_text.push_str(line_ending);

            (line_start, line_start, new_text.chars().count() as isize)
        };

        let new_range = if range.cursor(text) > range.anchor {
            // when appending, extend the range by local_offs
            Range::new(
                (range.anchor as isize + global_offs) as usize,
                (range.head as isize + local_offs + global_offs) as usize,
            )
        } else {
            // when inserting, slide the range by local_offs
            Range::new(
                (range.anchor as isize + local_offs + global_offs) as usize,
                (range.head as isize + local_offs + global_offs) as usize,
            )
        };

        // TODO: range replace or extend
        // range.replace(|range| range.is_empty(), head); -> fn extend if cond true, new head pos
        // can use the caller's mode to replace or extend on most changes
        ranges.push(new_range);
        global_offs += new_text.chars().count() as isize - chars_deleted as isize;
        let tendril = Tendril::from(&new_text);
        new_text.clear();

        (from, to, Some(tendril))
    });

    transaction = transaction.with_selection(Selection::new(ranges, selection.primary_index()));

    drop(config);
    super::apply(doc, &transaction, view_id)
}

/// Move cursors to line boundaries and indent empty lines.
pub fn insert_at_line_boundary(
    doc: &mut Document,
    view_id: ViewId,
    boundary: LineBoundary,
    extend: bool,
) -> Result<(), EditError> {
    let text = doc.text().slice(..);
    let contents = doc.text();
    let selection = doc.selection(view_id);

    let syntax = doc.syntax();
    let tab_width = doc.tab_width();

    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut offs = 0;

    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        let cursor_line = range.cursor_line(text);
        let cursor_line_start = text.line_to_char_idx(cursor_line);

        if line_end_char_index(&text, cursor_line) == cursor_line_start {
            // line is empty => auto indent
            let line_end_index = cursor_line_start;

            let indent = language::indent::indent_for_newline(
                doc.syntax_loader(),
                syntax,
                &doc.config.load().indent.heuristic,
                &doc.indent_style,
                tab_width,
                text,
                cursor_line,
                line_end_index,
                cursor_line,
            );

            // calculate new selection ranges
            let pos = offs + cursor_line_start;
            let indent_width = indent.chars().count();
            ranges.push(Range::point(pos + indent_width));
            offs += indent_width;

            (line_end_index, line_end_index, Some(indent.into()))
        } else {
            // move cursor to the fallback position
            let pos = match boundary {
                LineBoundary::LineStart => text
                    .line(cursor_line, stdx::rope::LINE_TYPE)
                    .first_non_whitespace_char()
                    .map(|ws_offset| ws_offset + cursor_line_start)
                    .unwrap_or(cursor_line_start),
                LineBoundary::LineEnd => line_end_char_index(&text, cursor_line),
            };

            ranges.push(range.put_cursor(text, pos + offs, extend));

            (cursor_line_start, cursor_line_start, None)
        }
    });

    transaction = transaction.with_selection(Selection::new(ranges, selection.primary_index()));
    super::apply(doc, &transaction, view_id)
}

/// Open indented lines above or below selections.
pub fn open_lines(
    doc: &mut Document,
    view_id: ViewId,
    open: Open,
    comment_continuation: CommentContinuation,
    count: usize,
) -> Result<(), EditError> {
    let config = doc.config.load();

    let text = doc.text().slice(..);
    let contents = doc.text();
    let selection = doc.selection(view_id);
    let mut offs = 0;

    let mut ranges = SmallVec::with_capacity(selection.len());

    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        // the line number, where the cursor is currently
        let curr_line_num = text.char_to_line_idx(match open {
            Open::Below => graphemes::prev_grapheme_boundary(text, range.to()),
            Open::Above => range.from(),
        });

        // the next line number, where the cursor will be, after finishing the transaction
        let next_new_line_num = match open {
            Open::Below => curr_line_num + 1,
            Open::Above => curr_line_num,
        };

        let above_next_new_line_num = next_new_line_num.saturating_sub(1);

        // Continue the comment leader using the comment tokens of the layer at the current line.
        let continue_comment_token =
            if comment_continuation == CommentContinuation::Enabled && config.continue_comments {
                text.line(curr_line_num, stdx::rope::LINE_TYPE)
                    .first_non_whitespace_char()
                    .map(|c| text.char_to_byte_idx(text.line_to_char_idx(curr_line_num) + c))
                    .and_then(|byte| continued_line_comment_token(doc, text, curr_line_num, byte))
            } else {
                None
            };

        // Index to insert newlines after, as well as the char width
        // to use to compensate for those inserted newlines.
        let (above_next_line_end_index, above_next_line_end_width) = if next_new_line_num == 0 {
            (0, 0)
        } else {
            (
                line_end_char_index(&text, above_next_new_line_num),
                doc.line_ending.len_chars(),
            )
        };

        let line = text.line(curr_line_num, stdx::rope::LINE_TYPE);
        let indent = match line.first_non_whitespace_char() {
            Some(pos) if continue_comment_token.is_some() => line.slice_chars(..pos).to_string(),
            _ => language::indent::indent_for_newline(
                doc.syntax_loader(),
                doc.syntax(),
                &config.indent.heuristic,
                &doc.indent_style,
                doc.tab_width(),
                text,
                above_next_new_line_num,
                above_next_line_end_index,
                curr_line_num,
            ),
        };

        let indent_len = indent.chars().count();
        let mut text = String::with_capacity(1 + indent_len);

        if open == Open::Above && next_new_line_num == 0 {
            text.push_str(&indent);
            if let Some(token) = continue_comment_token {
                text.push_str(token);
                text.push(' ');
            }
            text.push_str(doc.line_ending.as_str());
        } else {
            text.push_str(doc.line_ending.as_str());
            text.push_str(&indent);

            if let Some(token) = continue_comment_token {
                text.push_str(token);
                text.push(' ');
            }
        }

        let text = text.repeat(count);

        // calculate new selection ranges
        let pos = offs + above_next_line_end_index + above_next_line_end_width;
        let comment_len = continue_comment_token
            .map(|token| token.chars().count() + 1) // `+ 1` for the extra space added
            .unwrap_or_default();
        for i in 0..count {
            // pos                     -> beginning of reference line,
            // + (i * (line_ending_len + indent_len + comment_len)) -> beginning of i'th line from pos (possibly including comment token)
            // + indent_len + comment_len ->        -> indent for i'th line
            ranges.push(Range::point(
                pos + (i * (doc.line_ending.len_chars() + indent_len + comment_len))
                    + indent_len
                    + comment_len,
            ));
        }

        // update the offset for the next range
        offs += text.chars().count();

        (
            above_next_line_end_index,
            above_next_line_end_index,
            Some(text.into()),
        )
    });

    transaction = transaction.with_selection(Selection::new(ranges, selection.primary_index()));

    drop(config);
    super::apply(doc, &transaction, view_id)
}

/// Add blank lines above or below selections without changing editing mode.
pub fn add_newlines(
    doc: &mut Document,
    view_id: ViewId,
    open: Open,
    count: usize,
) -> Result<(), EditError> {
    let selection = doc.selection(view_id);
    let text = doc.text();
    let slice = text.slice(..);

    let changes = selection.into_iter().map(|range| {
        let (start, end) = range.line_range(slice);
        let line = match open {
            Open::Above => start,
            Open::Below => end + 1,
        };
        let pos = text.line_to_char_idx(line);
        (
            pos,
            pos,
            Some(doc.line_ending.as_str().repeat(count).into()),
        )
    });

    let transaction = Transaction::change(text, changes);
    super::apply(doc, &transaction, view_id)
}

/// Which direction the selected lines move.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MoveSelection {
    Above,
    Below,
}

/// A maximal contiguous run of lines (inclusive) that move together as a block.
struct LineBlock {
    start: usize,
    end: usize,
}

/// Where a moving line's content lands: the destination line and how far that
/// line's start offset shifts between the old and new text.
struct LineDestination {
    line: usize,
    start_shift: isize,
}

/// A line of a rotation window, snapshotted before the rotation.
struct WindowLine {
    /// Char index of the line's first content char.
    from: usize,
    /// Char index one past the line's last content char.
    to: usize,
    /// The line's content.
    content: String,
}

impl WindowLine {
    fn len(&self) -> usize {
        self.to - self.from
    }
}

/// Move lines or blocks of text in the specified direction.
///
/// Each cursor owns a contiguous run of lines (its `line_range`); consecutive
/// cursors and every line of a multi-line selection are merged into single runs.
/// Moving a run swaps it with the single line on the target side, so the run
/// travels as one block. A run at a document edge has no line to swap with and
/// is left in place.
///
/// The operation is a pure permutation of line contents, so no text is ever
/// created or destroyed. Each cursor follows the line its text landed on, keeping
/// its offset within the line and the direction of its selection. An endpoint
/// sitting exactly at the end of a moving block (the start of the line just past
/// it) follows the block, so the block keeps moving as a whole on repeated
/// invocations.
pub fn move_lines(
    doc: &mut Document,
    view_id: ViewId,
    direction: MoveSelection,
) -> Result<(), EditError> {
    let selection = doc.selection(view_id);
    let primary = selection.primary_index();
    let text = doc.text();
    let slice = text.slice(..);
    let total_lines = text.len_lines(stdx::rope::LINE_TYPE);

    let blocks = line_blocks(selection, slice);
    let (destinations, block_ends, edits) =
        plan_line_moves(&blocks, direction, text, slice, total_lines);
    let ranges: SmallVec<[Range; 1]> = selection
        .iter()
        .map(|range| displace_range(range, &destinations, &block_ends, text, slice))
        .collect();

    let transaction = Transaction::change(text, edits.into_iter())
        .with_selection(Selection::new(ranges, primary));
    super::apply(doc, &transaction, view_id)
}

/// Group the lines touched by the selection into maximal contiguous runs.
///
/// Consecutive cursors and every line of a multi-line selection are merged so
/// that each run always moves as a single block.
fn line_blocks(selection: &Selection, slice: RopeSlice) -> Vec<LineBlock> {
    let mut lines: Vec<usize> = selection
        .iter()
        .flat_map(|range| {
            let (start, end) = range.line_range(slice);
            start..=end
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();

    let mut blocks: Vec<LineBlock> = Vec::new();
    for line in lines {
        match blocks.last_mut() {
            Some(last) if last.end + 1 == line => last.end = line,
            _ => blocks.push(LineBlock {
                start: line,
                end: line,
            }),
        }
    }
    blocks
}

/// Compute the line-content replacements and destinations for the blocks.
///
/// Returns a map from each moving line to where its content lands, a map from
/// each "just past the block's end" position to where it lands, and the
/// ordered, non-overlapping replacements that perform the permutation.
fn plan_line_moves(
    blocks: &[LineBlock],
    direction: MoveSelection,
    text: &Rope,
    slice: RopeSlice,
    total_lines: usize,
) -> (
    HashMap<usize, LineDestination>,
    HashMap<usize, usize>,
    Vec<Change>,
) {
    let mut destinations: HashMap<usize, LineDestination> = HashMap::new();
    let mut block_ends: HashMap<usize, usize> = HashMap::new();
    let mut edits: Vec<Change> = Vec::new();

    for block in blocks {
        // The window of lines the block rotates over. Skipped when the block
        // sits at a document edge and has no neighbour line to swap with.
        let (window_start, window_end) = match direction {
            MoveSelection::Below if block.end + 1 < total_lines => (block.start, block.end + 1),
            MoveSelection::Above if block.start >= 1 => (block.start - 1, block.end),
            _ => continue,
        };

        let window: Vec<WindowLine> = (window_start..=window_end)
            .map(|line| {
                let from = text.line_to_char_idx(line);
                let to = line_end_char_index(&slice, line);
                WindowLine {
                    from,
                    to,
                    content: text.slice_chars(from..to).to_string(),
                }
            })
            .collect();

        // A one-step rotation: each line takes the content of its neighbour on
        // the incoming side, and the boundary line takes the block's far edge.
        // Only lines whose content actually changes produce a replacement.
        for i in 0..window.len() {
            let source = match direction {
                MoveSelection::Below if i > 0 => i - 1,
                MoveSelection::Below => window.len() - 1,
                MoveSelection::Above if i + 1 < window.len() => i + 1,
                MoveSelection::Above => 0,
            };
            if window[source].content != window[i].content {
                edits.push((
                    window[i].from,
                    window[i].to,
                    Some(window[source].content.clone().into()),
                ));
            }

            // The boundary line never carries a cursor, so record a
            // destination for the block's own lines only. Every rotation is
            // length-neutral, so the start offset a line's content travels
            // equals the content-length difference between the boundary line
            // it swaps across and the line itself.
            let is_block_line = match direction {
                MoveSelection::Below => i + 1 < window.len(),
                MoveSelection::Above => i > 0,
            };
            if is_block_line {
                let line = window_start + i;
                destinations.insert(
                    line,
                    LineDestination {
                        line: match direction {
                            MoveSelection::Below => line + 1,
                            MoveSelection::Above => line - 1,
                        },
                        start_shift: match direction {
                            MoveSelection::Below => {
                                window.last().unwrap().len() as isize - window[i].len() as isize
                            }
                            MoveSelection::Above => {
                                window[i - 1].len() as isize - window[0].len() as isize
                            }
                        },
                    },
                );
            }
        }

        // A selection endpoint sitting exactly at the start of the line after
        // the block ends the block without being on a block line: `line_range`
        // stops at the previous line, so the line the endpoint sits on is the
        // boundary line and has no destination of its own (e.g. selecting a
        // paragraph with `mip` then extending it with `x`). Without an explicit
        // mapping the endpoint would keep its stale position, which the
        // rotation moves underneath it, silently shrinking the block. The
        // position "just past the block" therefore travels with the block: the
        // rotation is a permutation, so it shifts by the boundary line's length
        // plus one line ending, in the direction of the move. That line ending
        // is the one below the block's far edge either way: moving down, the
        // boundary line's own ending; moving up, the block's last line's.
        if block.end + 1 < total_lines {
            let block_end = text.line_to_char_idx(block.end + 1);
            let boundary_len = match direction {
                MoveSelection::Below => window.last().unwrap().len(),
                MoveSelection::Above => window.first().unwrap().len(),
            } as isize;
            let line_ending_len = get_line_ending(&slice.line(window_end, stdx::rope::LINE_TYPE))
                .map(|ending| ending.len_chars() as isize)
                .unwrap_or(0);
            let shift = (boundary_len + line_ending_len)
                * match direction {
                    MoveSelection::Below => 1,
                    MoveSelection::Above => -1,
                };
            block_ends.insert(block_end, (block_end as isize + shift) as usize);
        }
    }

    (destinations, block_ends, edits)
}

/// Relocate a single cursor to where its text landed, preserving each endpoint's
/// offset within its line and the range's direction.
fn displace_range(
    range: &Range,
    destinations: &HashMap<usize, LineDestination>,
    block_ends: &HashMap<usize, usize>,
    text: &Rope,
    slice: RopeSlice,
) -> Range {
    let move_pos = |pos: usize| -> usize {
        // Endpoints exactly at the end of a moving block have no line of their
        // own to follow; they are mapped to their new position directly.
        if let Some(&pos) = block_ends.get(&pos) {
            return pos;
        }
        let line = slice.char_to_line_idx(pos);
        let Some(destination) = destinations.get(&line) else {
            return pos;
        };
        let offset = pos - text.line_to_char_idx(line);
        let line_start = text.line_to_char_idx(destination.line) as isize + destination.start_shift;
        line_start as usize + offset
    };
    Range::new(move_pos(range.anchor), move_pos(range.head)).with_direction(range.direction())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{self, tests::document_and_view};
    use editor_core::test;
    use editor_core::transform::ReorderStrategy;

    #[tokio::test]
    async fn rejected_interactive_edits_preserve_text_selection_and_history() {
        type Edit = fn(&mut Document, ViewId) -> Result<(), EditError>;
        let edits: &[Edit] = &[
            |doc, view| insert_char(doc, view, '界', None),
            |doc, view| insert_tab(doc, view, 1),
            |doc, view| insert_newline(doc, view, None),
            |doc, view| open_lines(doc, view, Open::Below, CommentContinuation::Enabled, 1),
            |doc, view| add_newlines(doc, view, Open::Above, 1),
            |doc, view| delete_char_backward(doc, view, 1, None),
            |doc, view| delete_char_forward(doc, view, 1),
            |doc, view| delete_word_backward(doc, view, 1),
            |doc, view| delete_word_forward(doc, view, 1),
            kill_to_line_start,
            kill_to_line_end,
            |doc, view| editing::join_selections(doc, view, true),
            editing::toggle_comments,
            editing::toggle_line_comments,
            editing::toggle_block_comments,
            |doc, view| {
                editing::reorder_selection_contents(doc, view, ReorderStrategy::RotateForward, 1)
            },
        ];
        for edit in edits {
            let (doc, mut view) = document_and_view("a #[bb|]# #(ccc|)#\nnext\n");
            let mut doc = doc.with_readonly_name("adapter source".into());
            let text = doc.text().clone();
            let selection = doc.selection(view.id).clone();
            let version = doc.version();
            assert_eq!(edit(&mut doc, view.id), Err(EditError::ReadOnlyDocument));
            assert_eq!(doc.text(), &text);
            assert_eq!(doc.selection(view.id), &selection);
            assert_eq!(doc.version(), version);
            assert!(doc.changes().is_empty());
            doc.commit_changes();
            view.sync_changes(&mut doc);
            assert!(!editing::undo(&mut doc, view.id));
        }
    }

    #[tokio::test]
    async fn skipping_a_paired_closer_moves_selection_without_changing_text() {
        let (mut doc, view) = document_and_view("(#[|)]#\n");
        let pairs = AutoPairs::default();
        let version = doc.version();
        insert_char(&mut doc, view.id, ')', Some(&pairs)).unwrap();
        assert_eq!(doc.text(), "()\n");
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2
        );
        assert_eq!(doc.version(), version);
        assert!(doc.changes().is_empty());
    }

    #[tokio::test]
    async fn rejected_placeholder_replacement_preserves_the_session_and_pending_input() {
        let (mut doc, view) = document_and_view("#[|]#\n");
        let snippet = snippets::Snippet::parse("${1:one} ${2:two}$0").unwrap();
        let (transaction, selection, rendered) = snippet.render(
            doc.text(),
            doc.selection(view.id),
            |_| (0, 0),
            &mut doc.snippet_ctx(
                doc.text(),
                doc.selection(view.id),
                crate::document::Mode::Insert,
            ),
        );
        editing::apply(&mut doc, &transaction.with_selection(selection), view.id).unwrap();
        doc.install_snippet(view.id, rendered);
        let mut doc = doc.with_readonly_name("adapter source".into());
        let before = doc.text().clone();
        let selection = doc.selection(view.id).clone();
        let version = doc.version();
        assert_eq!(
            insert_char(&mut doc, view.id, '界', None),
            Err(EditError::ReadOnlyDocument)
        );
        assert_eq!(doc.text(), &before);
        assert_eq!(doc.selection(view.id), &selection);
        assert_eq!(doc.version(), version);
        assert!(doc.active_snippet().is_some());
        assert!(doc.pending_snippet_placeholder(view.id).is_some());
    }

    #[tokio::test]
    async fn move_lines_down_with_single_cursor() {
        let (mut doc, view) = document_and_view("#[a|]#aa\nbbb\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "bbb\n#[a|]#aa\n"
        );
    }

    #[tokio::test]
    async fn move_lines_up_with_single_cursor() {
        let (mut doc, view) = document_and_view("aaa\n#[b|]#bb\n");
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[b|]#bb\naaa\n"
        );
    }

    #[tokio::test]
    async fn move_lines_down_with_two_cursors_on_same_line() {
        let (mut doc, view) = document_and_view("#[a|]#b#(c|)#d\nbbbb\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "bbbb\n#[a|]#b#(c|)#d\n"
        );
    }

    #[tokio::test]
    async fn move_lines_down_with_adjacent_cursors_on_same_line() {
        let (mut doc, view) = document_and_view("#[a|]##(b|)#\nbbbb\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(doc.text(), "bbbb\nab\n");
        // The markers of the two adjacent cursors share a position, so assert
        // the selection explicitly instead of through `test::plain`.
        let selection = doc.selection(view.id);
        assert_eq!(selection.primary_index(), 0);
        assert_eq!(
            selection.iter().copied().collect::<Vec<_>>(),
            vec![Range::new(5, 6), Range::new(6, 7)]
        );
    }

    #[tokio::test]
    async fn move_lines_up_with_multiple_cursors_at_top() {
        let (mut doc, view) = document_and_view("#[|a]#aa\nbbb\n#(c|)#cc\n");
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[|a]#aa\n#(c|)#cc\nbbb\n"
        );
    }

    #[tokio::test]
    async fn move_lines_down_with_multiple_cursors_at_bottom() {
        let (mut doc, view) = document_and_view("aaaa\n#[b|]#bbb\ncccc\n#(d|)#ddd\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "aaaa\ncccc\n#[b|]#bbb\n\n#(d|)#ddd"
        );
    }

    #[tokio::test]
    async fn move_lines_up_with_edge_and_middle_blocks() {
        let (mut doc, view) = document_and_view("#[|a]#aa\nbbbb\ncccc\n#(d|)#ddd\n");
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[|a]#aa\nbbbb\n#(d|)#ddd\ncccc\n"
        );
    }

    #[tokio::test]
    async fn move_lines_down_on_last_line_without_trailing_newline_is_a_noop() {
        let (mut doc, view) = document_and_view("aaa\n#[b|]#bb");
        let version = doc.version();
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "aaa\n#[b|]#bb"
        );
        assert_eq!(doc.version(), version);
        assert!(doc.changes().is_empty());
    }

    #[tokio::test]
    async fn move_lines_down_with_multiline_selection() {
        let (mut doc, view) = document_and_view("#[aaa\nbbb|]#\nccc\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "ccc\n#[aaa\nbbb|]#\n"
        );
    }

    #[tokio::test]
    async fn move_lines_up_with_block_of_unequal_lengths() {
        let (mut doc, view) = document_and_view("a\n#(b|)#bb\n#[c|]#cc\nddd\n");
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#(b|)#bb\n#[c|]#cc\na\nddd\n"
        );
    }

    #[tokio::test]
    async fn move_lines_preserves_backward_selection_direction() {
        let (mut doc, view) = document_and_view("#[|b]#b\nccc\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "ccc\n#[|b]#b\n"
        );
    }

    #[tokio::test]
    async fn move_lines_up_with_block_at_top_is_a_noop() {
        let (mut doc, view) = document_and_view("#[aaa\nbbb|]#\nccc\n");
        let version = doc.version();
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[aaa\nbbb|]#\nccc\n"
        );
        assert_eq!(doc.version(), version);
        assert!(doc.changes().is_empty());
    }

    #[tokio::test]
    async fn move_lines_down_with_block_at_bottom_is_a_noop() {
        let (mut doc, view) = document_and_view("aaa\n#[bbb\nccc|]#");
        let version = doc.version();
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "aaa\n#[bbb\nccc|]#"
        );
        assert_eq!(doc.version(), version);
        assert!(doc.changes().is_empty());
    }

    #[tokio::test]
    async fn move_lines_at_edge_is_a_noop() {
        let (mut doc, view) = document_and_view("#[|a]#aa\nbbb\n");
        let version = doc.version();
        move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[|a]#aa\nbbb\n"
        );
        assert_eq!(doc.version(), version);
        assert!(doc.changes().is_empty());
    }

    #[tokio::test]
    async fn move_lines_between_identical_lines_moves_selection_only() {
        let (mut doc, view) = document_and_view("#[a|]#a\naa\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(doc.text(), "aa\naa\n");
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "aa\n#[a|]#a\n"
        );
    }

    #[tokio::test]
    async fn move_lines_down_block_ending_in_empty_line_crlf() {
        let (mut doc, view) =
            document_and_view("#[aaaa\r\nbbbb\r\n \r\n|]#cccc\r\ndddd\r\neeee\r\n");
        for _ in 0..3 {
            move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        }
        assert_eq!(doc.text(), "cccc\r\ndddd\r\neeee\r\naaaa\r\nbbbb\r\n \r\n");
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "cccc\r\ndddd\r\neeee\r\n#[aaaa\r\nbbbb\r\n \r\n|]#"
        );
    }

    #[tokio::test]
    async fn move_lines_up_block_ending_in_empty_line() {
        let (mut doc, view) = document_and_view(
            "b2-line_1\nb2-line_2\nb2-line_3\nb2-line_4\nb2-line_5\n\n\
             #[b1-line_1\nb1-line_2\nb1-line_3\nb1-line_4\nb1-line_5\n\n|]#",
        );
        for _ in 0..6 {
            move_lines(&mut doc, view.id, MoveSelection::Above).unwrap();
        }
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[b1-line_1\nb1-line_2\nb1-line_3\nb1-line_4\nb1-line_5\n\n|]#\
             b2-line_1\nb2-line_2\nb2-line_3\nb2-line_4\nb2-line_5\n\n"
        );
    }

    #[tokio::test]
    async fn move_lines_undo_and_redo() {
        let (mut doc, view) = document_and_view("#[a|]#aa\nbbb\n");
        move_lines(&mut doc, view.id, MoveSelection::Below).unwrap();
        assert_eq!(doc.text(), "bbb\naaa\n");
        assert!(editing::undo(&mut doc, view.id));
        assert_eq!(doc.text(), "aaa\nbbb\n");
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "#[a|]#aa\nbbb\n"
        );
        assert!(editing::redo(&mut doc, view.id));
        assert_eq!(doc.text(), "bbb\naaa\n");
        assert_eq!(
            test::plain(doc.text().slice(..), doc.selection(view.id)),
            "bbb\n#[a|]#aa\n"
        );
    }
}

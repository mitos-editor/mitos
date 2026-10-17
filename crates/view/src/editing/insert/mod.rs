//! Insert-mode text operations over documents and per-view selections.
//!
//! Callers own mode changes, key handling, completion navigation, and frontend
//! events. These operations apply through the shared mutation path and leave
//! undo groups open so consecutive typed edits remain a single undo step.

use super::continued_line_comment_token;
mod delete;
use crate::{Document, ViewId};
pub use delete::{
    delete_char_backward, delete_char_forward, delete_word_backward, delete_word_forward,
    kill_to_line_end, kill_to_line_start,
};
use editor_core::{
    auto_pairs::{self, AutoPairs},
    graphemes,
    indent::IndentStyle,
    line_ending::line_end_char_index,
    Range, Selection, SmallVec, Tendril, Transaction,
};
use language::syntax;
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
    loader: &syntax::Loader,
    global_pairs: Option<&AutoPairs>,
) {
    let text = doc.text();
    let selection = doc.selection(view_id);

    let auto_pairs = doc.auto_pairs(global_pairs, loader, doc.selection(view_id));

    let insert_char = |range: Range, ch: char| {
        let cursor = range.cursor(text.slice(..));
        let t = Tendril::from_iter([ch]);
        ((cursor, cursor, Some(t)), None)
    };

    let transaction = Transaction::change_by_and_with_selection(text, selection, |range| {
        auto_pairs
            .and_then(|pairs| auto_pairs::hook_insert(text, range, c, pairs))
            .map(|(change, range)| (change, Some(range)))
            .unwrap_or_else(|| insert_char(*range, c))
    });

    super::apply(doc, &transaction, view_id);
}

/// Insert indentation up to the next indent stop at each cursor.
pub fn insert_tab(doc: &mut Document, view_id: ViewId, count: usize) {
    let transaction = Transaction::change(
        doc.text(),
        doc.selection(view_id).ranges().iter().map(|range| {
            let cursor = range.cursor(doc.text().slice(..));
            let indent = if let IndentStyle::Spaces(indent_width) = doc.indent_style {
                let line = range.cursor_line(doc.text().slice(..));
                let line_start = doc.text().line_to_char(line);
                let offset = (cursor - line_start) % indent_width as usize;

                Tendril::from(doc.indent_style.as_str().repeat(count)).split_off(offset)
            } else {
                Tendril::from(doc.indent_style.as_str().repeat(count))
            };

            (cursor, cursor, Some(indent))
        }),
    );
    super::apply(doc, &transaction, view_id);
}

/// Insert line endings with language-aware indentation and comment continuation.
pub fn insert_newline(
    doc: &mut Document,
    view_id: ViewId,
    loader: &syntax::Loader,
    global_pairs: Option<&AutoPairs>,
) {
    let config = doc.config.load();
    let text = doc.text().slice(..);
    let line_ending = doc.line_ending.as_str();

    let contents = doc.text();
    let selection = doc.selection(view_id);
    let mut ranges = SmallVec::with_capacity(selection.len());

    // TODO: this is annoying, but we need to do it to properly calculate pos after edits
    let mut global_offs = 0;
    let mut new_text = String::new();

    let auto_pairs = doc.auto_pairs(global_pairs, loader, selection);
    let mut last_pos = 0;
    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        // Tracks the number of trailing whitespace characters deleted by this selection.
        let mut chars_deleted = 0;
        let pos = range.cursor(text);

        let prev = if pos == 0 {
            ' '
        } else {
            contents.char(pos - 1)
        };
        let curr = contents.get_char(pos).unwrap_or(' ');

        let current_line = text.char_to_line(pos);
        let line_start = text.line_to_char(current_line);

        // Continue the comment leader using the comment tokens of the layer at the comment
        // leader (i.e. the first non-whitespace char on the line). Looking up at the cursor
        // would land inside an injected layer (e.g. `comment`, or markdown in a doc comment)
        // and miss the host language's tokens.
        let continue_comment_token = if config.continue_comments {
            text.line(current_line)
                .first_non_whitespace_char()
                .map(|c| text.char_to_byte(line_start + c))
                .and_then(|byte| {
                    continued_line_comment_token(doc, loader, text, current_line, byte)
                })
        } else {
            None
        };

        let (from, to, local_offs) = if let Some(idx) =
            text.slice(line_start..pos).last_non_whitespace_char()
        {
            let first_trailing_whitespace_char = (line_start + idx + 1).clamp(last_pos, pos);
            last_pos = pos;
            let line = text.line(current_line);

            let indent = match line.first_non_whitespace_char() {
                Some(pos) if continue_comment_token.is_some() => line.slice(..pos).to_string(),
                _ => language::indent::indent_for_newline(
                    loader,
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
    super::apply(doc, &transaction, view_id);
}

/// Move cursors to line boundaries and indent empty lines.
pub fn insert_at_line_boundary(
    doc: &mut Document,
    view_id: ViewId,
    loader: &syntax::Loader,
    boundary: LineBoundary,
    extend: bool,
) {
    let text = doc.text().slice(..);
    let contents = doc.text();
    let selection = doc.selection(view_id);

    let syntax = doc.syntax();
    let tab_width = doc.tab_width();

    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut offs = 0;

    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        let cursor_line = range.cursor_line(text);
        let cursor_line_start = text.line_to_char(cursor_line);

        if line_end_char_index(&text, cursor_line) == cursor_line_start {
            // line is empty => auto indent
            let line_end_index = cursor_line_start;

            let indent = language::indent::indent_for_newline(
                loader,
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
                    .line(cursor_line)
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
    super::apply(doc, &transaction, view_id);
}

/// Open indented lines above or below selections.
pub fn open_lines(
    doc: &mut Document,
    view_id: ViewId,
    loader: &syntax::Loader,
    open: Open,
    comment_continuation: CommentContinuation,
    count: usize,
) {
    let config = doc.config.load();

    let text = doc.text().slice(..);
    let contents = doc.text();
    let selection = doc.selection(view_id);
    let mut offs = 0;

    let mut ranges = SmallVec::with_capacity(selection.len());

    let mut transaction = Transaction::change_by_selection(contents, selection, |range| {
        // the line number, where the cursor is currently
        let curr_line_num = text.char_to_line(match open {
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
                text.line(curr_line_num)
                    .first_non_whitespace_char()
                    .map(|c| text.char_to_byte(text.line_to_char(curr_line_num) + c))
                    .and_then(|byte| {
                        continued_line_comment_token(doc, loader, text, curr_line_num, byte)
                    })
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

        let line = text.line(curr_line_num);
        let indent = match line.first_non_whitespace_char() {
            Some(pos) if continue_comment_token.is_some() => line.slice(..pos).to_string(),
            _ => language::indent::indent_for_newline(
                loader,
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
    super::apply(doc, &transaction, view_id);
}

/// Add blank lines above or below selections without changing editing mode.
pub fn add_newlines(doc: &mut Document, view_id: ViewId, open: Open, count: usize) {
    let selection = doc.selection(view_id);
    let text = doc.text();
    let slice = text.slice(..);

    let changes = selection.into_iter().map(|range| {
        let (start, end) = range.line_range(slice);
        let line = match open {
            Open::Above => start,
            Open::Below => end + 1,
        };
        let pos = text.line_to_char(line);
        (
            pos,
            pos,
            Some(doc.line_ending.as_str().repeat(count).into()),
        )
    });

    let transaction = Transaction::change(text, changes);
    super::apply(doc, &transaction, view_id);
}

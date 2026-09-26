//! Character insertion, smart tabs, newlines, and insert-mode deletion commands.

use std::borrow::Cow;

use editor_core::{
    auto_pairs, graphemes,
    indent::{self, IndentStyle},
    line_ending::line_end_char_index,
    movement::{self as core_movement, Direction},
    unicode::width::UnicodeWidthChar,
    Deletion, Range, Rope, RopeSlice, Selection, SmallVec, Tendril, Transaction,
};
use stdx::rope::RopeSliceExt;
use ui_core::{input::KeyEvent, keyboard::KeyCode};
use view::{document::Mode, editor::SmartTabConfig, Document};

use super::{
    append_mode, context::Context, continued_line_comment_token, goto_next_tabstop, insert_mode,
    move_parent_node_end,
};
use crate::{events::PostInsertChar, key};

pub type Hook = fn(&Rope, &Selection, char) -> Option<Transaction>;

/// Exclude the cursor in range.
fn exclude_cursor(text: RopeSlice, range: Range, cursor: Range) -> Range {
    if range.to() == cursor.to() && text.len_chars() != cursor.to() {
        Range::new(
            range.from(),
            graphemes::prev_grapheme_boundary(text, cursor.to()),
        )
    } else {
        range
    }
}

pub fn insert_char(cx: &mut Context, c: char) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text();
    let selection = doc.selection(view.id);

    let loader: &editor_core::syntax::Loader = &cx.editor.syn_loader.load();
    let auto_pairs = doc.auto_pairs(cx.editor, loader, view);

    let insert_char = |range: Range, ch: char| {
        let cursor = range.cursor(text.slice(..));
        let t = Tendril::from_iter([ch]);
        ((cursor, cursor, Some(t)), None)
    };

    let transaction = Transaction::change_by_and_with_selection(text, selection, |range| {
        auto_pairs
            .as_ref()
            .and_then(|ap| {
                auto_pairs::hook_insert(text, range, c, ap)
                    .map(|(change, range)| (change, Some(range)))
                    .or_else(|| Some(insert_char(*range, c)))
            })
            .unwrap_or_else(|| insert_char(*range, c))
    });

    let doc = doc_mut!(cx.editor, &doc.id());
    doc.apply(&transaction, view.id);

    event::dispatch(PostInsertChar { c, cx });
}

pub fn smart_tab(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let view_id = view.id;

    if matches!(
        cx.editor.config().smart_tab,
        Some(SmartTabConfig { enable: true, .. })
    ) {
        let cursors_after_whitespace = doc.selection(view_id).ranges().iter().all(|range| {
            let cursor = range.cursor(doc.text().slice(..));
            let current_line_num = doc.text().char_to_line(cursor);
            let current_line_start = doc.text().line_to_char(current_line_num);
            let left = doc.text().slice(current_line_start..cursor);
            left.chars().all(|c| c.is_whitespace())
        });

        if !cursors_after_whitespace {
            if doc.active_snippet.is_some() {
                goto_next_tabstop(cx);
            } else {
                move_parent_node_end(cx);
            }
            return;
        }
    }

    insert_tab(cx);
}

pub fn insert_tab(cx: &mut Context) {
    insert_tab_impl(cx, 1)
}

fn insert_tab_impl(cx: &mut Context, count: usize) {
    let (view, doc) = current!(cx.editor);

    let transaction = Transaction::change(
        doc.text(),
        doc.selection(view.id).ranges().iter().map(|range| {
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
    doc.apply(&transaction, view.id);
}

pub fn append_char_interactive(cx: &mut Context) {
    // Save the current mode, so we can restore it later.
    let mode = cx.editor.mode;
    append_mode(cx);
    insert_selection_interactive(cx, mode);
}

pub fn insert_char_interactive(cx: &mut Context) {
    let mode = cx.editor.mode;
    insert_mode(cx);
    insert_selection_interactive(cx, mode);
}

fn insert_selection_interactive(cx: &mut Context, old_mode: Mode) {
    let count = cx.count();

    // need to wait for next key
    cx.on_next_key(move |cx, event| {
        match event {
            KeyEvent {
                code: KeyCode::Char(ch),
                ..
            } => {
                for _ in 0..count {
                    insert_char(cx, ch)
                }
            }
            key!(Enter) => {
                if count != 1 {
                    cx.editor
                        .set_error(|| "inserting multiple newlines not yet supported");
                    return;
                }
                insert_newline(cx)
            }
            key!(Tab) => insert_tab_impl(cx, count),
            _ => (),
        };
        // Restore the old mode.
        cx.editor.mode = old_mode;
    });
}

pub fn insert_newline(cx: &mut Context) {
    let config = cx.editor.config();
    let (view, doc) = current_ref!(cx.editor);
    let loader = cx.editor.syn_loader.load();
    let text = doc.text().slice(..);
    let line_ending = doc.line_ending.as_str();

    let contents = doc.text();
    let selection = doc.selection(view.id);
    let mut ranges = SmallVec::with_capacity(selection.len());

    // TODO: this is annoying, but we need to do it to properly calculate pos after edits
    let mut global_offs = 0;
    let mut new_text = String::new();

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
                    continued_line_comment_token(doc, &loader, text, current_line, byte)
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
                _ => indent::indent_for_newline(
                    &loader,
                    doc.syntax(),
                    &config.indent_heuristic,
                    &doc.indent_style,
                    doc.tab_width(),
                    text,
                    current_line,
                    pos,
                    current_line,
                ),
            };

            let loader: &editor_core::syntax::Loader = &cx.editor.syn_loader.load();
            // If we are between pairs (such as brackets), we want to
            // insert an additional line which is indented one level
            // more and place the cursor there
            let on_auto_pair = doc
                .auto_pairs(cx.editor, loader, view)
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

            // Note that `first_trailing_whitespace_char` is at least `pos` so this unsigned
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
        // can be used with cx.mode to do replace or extend on most changes
        ranges.push(new_range);
        global_offs += new_text.chars().count() as isize - chars_deleted as isize;
        let tendril = Tendril::from(&new_text);
        new_text.clear();

        (from, to, Some(tendril))
    });

    transaction = transaction.with_selection(Selection::new(ranges, selection.primary_index()));

    let (view, doc) = current!(cx.editor);
    doc.apply(&transaction, view.id);
}

fn dedent(doc: &Document, range: &Range) -> Option<Deletion> {
    let text = doc.text().slice(..);
    let pos = range.cursor(text);
    let line_start_pos = text.line_to_char(range.cursor_line(text));

    // consider to delete by indent level if all characters before `pos` are indent units.
    let fragment = Cow::from(text.slice(line_start_pos..pos));

    if fragment.is_empty() || !fragment.chars().all(|ch| ch == ' ' || ch == '\t') {
        return None;
    }

    if text.get_char(pos.saturating_sub(1)) == Some('\t') {
        // fast path, delete one char
        return Some((graphemes::nth_prev_grapheme_boundary(text, pos, 1), pos));
    }

    let tab_width = doc.tab_width();
    let indent_width = doc.indent_width();

    let width: usize = fragment
        .chars()
        .map(|ch| {
            if ch == '\t' {
                tab_width
            } else {
                // it can be none if it still meet control characters other than '\t'
                // here just set the width to 1 (or some value better?).
                ch.width().unwrap_or(1)
            }
        })
        .sum();

    // round down to nearest unit
    let mut drop = width % indent_width;

    // if it's already at a unit, consume a whole unit
    if drop == 0 {
        drop = indent_width
    };

    let mut chars = fragment.chars().rev();
    let mut start = pos;

    for _ in 0..drop {
        // delete up to `drop` spaces
        match chars.next() {
            Some(' ') => start -= 1,
            _ => break,
        }
    }

    Some((start, pos)) // delete!
}

pub fn delete_char_backward(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);

    let loader: &editor_core::syntax::Loader = &cx.editor.syn_loader.load();
    let auto_pairs = doc.auto_pairs(cx.editor, loader, view);

    let transaction =
        Transaction::delete_by_and_with_selection(doc.text(), doc.selection(view.id), |range| {
            let pos = range.cursor(text);

            log::debug!("cursor: {}, len: {}", pos, text.len_chars());

            if pos == 0 {
                return ((pos, pos), None);
            }

            dedent(doc, range)
                .map(|dedent| (dedent, None))
                .or_else(|| {
                    // [TODO] should this be fixed to get the auto pairs for
                    // each selection after 46af40017c0704142516b5740cf1a000ba4fd7c1 ?
                    auto_pairs::hook_delete(doc.text(), range, auto_pairs?)
                        .map(|(delete, new_range)| (delete, Some(new_range)))
                })
                .unwrap_or_else(|| {
                    (
                        (graphemes::nth_prev_grapheme_boundary(text, pos, count), pos),
                        None,
                    )
                })
        });

    log::debug!("delete_char_backward transaction: {:?}", transaction);

    let doc = doc_mut!(cx.editor, &doc.id());
    doc.apply(&transaction, view.id);
}

pub fn delete_char_forward(cx: &mut Context) {
    let count = cx.count();
    delete_by_selection_insert_mode(
        cx,
        |text, range| {
            let pos = range.cursor(text);
            (pos, graphemes::nth_next_grapheme_boundary(text, pos, count))
        },
        Direction::Forward,
    )
}

pub fn delete_word_backward(cx: &mut Context) {
    let count = cx.count();
    delete_by_selection_insert_mode(
        cx,
        |text, range| {
            let anchor = core_movement::move_prev_word_start(text, *range, count).from();
            let next = Range::new(anchor, range.cursor(text));
            let range = exclude_cursor(text, next, *range);
            (range.from(), range.to())
        },
        Direction::Backward,
    );
}

pub fn delete_word_forward(cx: &mut Context) {
    let count = cx.count();
    delete_by_selection_insert_mode(
        cx,
        |text, range| {
            let head = core_movement::move_next_word_end(text, *range, count).to();
            (range.cursor(text), head)
        },
        Direction::Forward,
    );
}

#[inline]
fn delete_by_selection_insert_mode(
    cx: &mut Context,
    mut f: impl FnMut(RopeSlice, &Range) -> Deletion,
    direction: Direction,
) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let mut selection = SmallVec::new();
    let mut insert_newline = false;
    let text_len = text.len_chars();
    let mut transaction =
        Transaction::delete_by_selection(doc.text(), doc.selection(view.id), |range| {
            let (start, end) = f(text, range);
            if direction == Direction::Forward {
                let mut range = *range;
                if range.head > range.anchor {
                    insert_newline |= end == text_len;
                    // move the cursor to the right so that the selection
                    // doesn't shrink when deleting forward (so the text appears to
                    // move to  left)
                    // += 1 is enough here as the range is normalized to grapheme boundaries
                    // later anyway
                    range.head += 1;
                }
                selection.push(range);
            }
            (start, end)
        });

    // in case we delete the last character and the cursor would be moved to the EOF char
    // insert a newline, just like when entering append mode
    if insert_newline {
        transaction = transaction.insert_at_eof(doc.line_ending.as_str().into());
    }

    if direction == Direction::Forward {
        doc.set_selection(
            view.id,
            Selection::new(selection, doc.selection(view.id).primary_index()),
        );
    }
    doc.apply(&transaction, view.id);
}

pub(super) fn kill_to_line_start(cx: &mut Context) {
    delete_by_selection_insert_mode(
        cx,
        move |text, range| {
            let line = range.cursor_line(text);
            let first_char = text.line_to_char(line);
            let anchor = range.cursor(text);
            let head = if anchor == first_char && line != 0 {
                // select until previous line
                line_end_char_index(&text, line - 1)
            } else if let Some(pos) = text.line(line).first_non_whitespace_char() {
                if first_char + pos < anchor {
                    // select until first non-blank in line if cursor is after it
                    first_char + pos
                } else {
                    // select until start of line
                    first_char
                }
            } else {
                // select until start of line
                first_char
            };
            (head, anchor)
        },
        Direction::Backward,
    );
}

pub(super) fn kill_to_line_end(cx: &mut Context) {
    delete_by_selection_insert_mode(
        cx,
        |text, range| {
            let line = range.cursor_line(text);
            let line_end_pos = line_end_char_index(&text, line);
            let pos = range.cursor(text);

            // if the cursor is on the newline char delete that
            if pos == line_end_pos {
                (pos, text.line_to_char(line + 1))
            } else {
                (pos, line_end_pos)
            }
        },
        Direction::Forward,
    );
}

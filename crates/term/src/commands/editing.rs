//! Text transformations, indentation, comments, replacement, deletion, and surrounds over selections.

use std::{
    borrow::Cow,
    char::{ToLowercase, ToUppercase},
};

use editor_core::{
    comment, indent::IndentStyle, line_ending::line_end_char_index, match_brackets,
    movement as core_movement, surround, syntax::config::BlockCommentToken, Range, Rope, RopeSlice,
    Selection, SmallVec, Tendril, Transaction,
};

use stdx::rope::RopeSliceExt;
use ui_core::{input::KeyEvent, keyboard::KeyCode};
use view::{document::Mode, info::Info, Document, Editor, ViewId};

use super::{
    context::Context,
    insert::{continued_line_comment_token, open, CommentContinuation, Open},
    mode::{enter_insert_mode, exit_select_mode},
    LINE_ENDING_REGEX,
};
use crate::ui::{self, Prompt, PromptEvent};

// align text in selection
#[allow(deprecated)]
pub(super) fn align_selections(cx: &mut Context) {
    use editor_core::visual_coords_at_pos;

    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);

    let tab_width = doc.tab_width();

    let mut column_widths: Vec<usize> = Vec::new();
    let mut coordinates = Vec::with_capacity(selection.len());

    let mut previous_line = usize::MAX;
    let mut col_idx = 0;
    let mut running_offset = 0;

    for range in selection {
        let coords = visual_coords_at_pos(text, range.head, tab_width);
        let anchor_coords = visual_coords_at_pos(text, range.anchor, tab_width);

        if coords.row != anchor_coords.row {
            cx.editor
                .set_error(|| "align cannot work with multi line selections");
            return;
        }
        if coords.row != previous_line {
            col_idx = 0;
            running_offset = 0;
            previous_line = coords.row;
        }

        let width = coords.col - running_offset;

        match column_widths.get_mut(col_idx) {
            Some(n) => *n = (*n).max(width),
            None => column_widths.push(width),
        }

        coordinates.push(coords);

        running_offset += width;
        col_idx += 1;
    }

    let column_positions: Vec<_> = column_widths
        .into_iter()
        .scan(0, |sum, n| {
            *sum += n;
            Some(*sum)
        })
        .collect();

    previous_line = usize::MAX;

    let changes = coordinates
        .into_iter()
        .zip(selection)
        .map(|(coords, range)| {
            if coords.row != previous_line {
                col_idx = 0;
                running_offset = 0;
                previous_line = coords.row;
            }
            let current_inserts = column_positions[col_idx] - coords.col - running_offset;
            let insert_pos = range.from();

            col_idx += 1;
            running_offset += current_inserts;

            (
                insert_pos,
                insert_pos,
                Some(" ".repeat(current_inserts).into()),
            )
        });

    let transaction = Transaction::change(doc.text(), changes);
    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

fn switch_case_impl<F>(cx: &mut Context, change_fn: F)
where
    F: Fn(RopeSlice) -> Tendril,
{
    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id);
    let transaction = Transaction::change_by_selection(doc.text(), selection, |range| {
        let text: Tendril = change_fn(range.slice(doc.text().slice(..)));

        (range.from(), range.to(), Some(text))
    });

    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

enum CaseSwitcher {
    Upper(ToUppercase),
    Lower(ToLowercase),
    Keep(Option<char>),
}

impl Iterator for CaseSwitcher {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CaseSwitcher::Upper(upper) => upper.next(),
            CaseSwitcher::Lower(lower) => lower.next(),
            CaseSwitcher::Keep(ch) => ch.take(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            CaseSwitcher::Upper(upper) => upper.size_hint(),
            CaseSwitcher::Lower(lower) => lower.size_hint(),
            CaseSwitcher::Keep(ch) => {
                let n = if ch.is_some() { 1 } else { 0 };
                (n, Some(n))
            }
        }
    }
}

impl ExactSizeIterator for CaseSwitcher {}

pub(super) fn switch_case(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string
            .chars()
            .flat_map(|ch| {
                if ch.is_lowercase() {
                    CaseSwitcher::Upper(ch.to_uppercase())
                } else if ch.is_uppercase() {
                    CaseSwitcher::Lower(ch.to_lowercase())
                } else {
                    CaseSwitcher::Keep(Some(ch))
                }
            })
            .collect()
    });
}

pub(super) fn switch_to_uppercase(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string.chunks().map(|chunk| chunk.to_uppercase()).collect()
    });
}

pub(super) fn switch_to_lowercase(cx: &mut Context) {
    switch_case_impl(cx, |string| {
        string.chunks().map(|chunk| chunk.to_lowercase()).collect()
    });
}

fn join_selections_impl(cx: &mut Context, select_space: bool) {
    use core_movement::skip_while;
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    let text = doc.text();
    let slice = text.slice(..);

    let mut changes = Vec::new();

    for selection in doc.selection(view.id) {
        let (start, mut end) = selection.line_range(slice);
        if start == end {
            end = (end + 1).min(text.len_lines() - 1);
        }
        let lines = start..end;

        changes.reserve(lines.len());

        let first_line_idx = slice.line_to_char(start);
        let first_line_idx = skip_while(slice, first_line_idx, |ch| matches!(ch, ' ' | '\t'))
            .unwrap_or(first_line_idx);
        let mut current_comment_token = continued_line_comment_token(
            doc,
            &loader,
            slice,
            start,
            slice.char_to_byte(first_line_idx),
        );

        for line in lines {
            let start = line_end_char_index(&slice, line);
            let mut end = text.line_to_char(line + 1);
            end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
            if let Some(token) =
                continued_line_comment_token(doc, &loader, slice, line + 1, slice.char_to_byte(end))
            {
                if Some(token) == current_comment_token {
                    end += token.chars().count();
                    end = skip_while(slice, end, |ch| matches!(ch, ' ' | '\t')).unwrap_or(end);
                } else {
                    // update current token, but don't delete this one.
                    current_comment_token = Some(token);
                }
            }

            let separator = if end == line_end_char_index(&slice, line + 1) {
                // the joining line contains only space-characters => don't include a whitespace when joining
                None
            } else {
                Some(Tendril::from(" "))
            };
            changes.push((start, end, separator));
        }
    }

    // nothing to do, bail out early to avoid crashes later
    if changes.is_empty() {
        return;
    }

    changes.sort_unstable_by_key(|(from, _to, _text)| *from);
    changes.dedup();

    // select inserted spaces
    let transaction = if select_space {
        let mut offset: usize = 0;
        let ranges: SmallVec<_> = changes
            .iter()
            .filter_map(|change| {
                if change.2.is_some() {
                    let range = Range::point(change.0 - offset);
                    offset += change.1 - change.0 - 1; // -1 adjusts for the replacement of the range by a space
                    Some(range)
                } else {
                    offset += change.1 - change.0;
                    None
                }
            })
            .collect();
        let t = Transaction::change(text, changes.into_iter());
        if ranges.is_empty() {
            t
        } else {
            let selection = Selection::new(ranges, 0);
            t.with_selection(selection)
        }
    } else {
        Transaction::change(text, changes.into_iter())
    };

    doc.apply(&transaction, view.id);
}

pub(super) fn join_selections(cx: &mut Context) {
    join_selections_impl(cx, false)
}

pub(super) fn join_selections_space(cx: &mut Context) {
    join_selections_impl(cx, true)
}

#[derive(Debug)]
enum ReorderStrategy {
    RotateForward,
    RotateBackward,
    Reverse,
}

fn reorder_selection_contents(cx: &mut Context, strategy: ReorderStrategy) {
    let count = cx.count;
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id);

    let mut ranges: Vec<_> = selection
        .slices(text)
        .map(|fragment| fragment.chunks().collect())
        .collect();

    let rotate_by = count.map_or(1, |count| count.get().min(ranges.len()));

    let primary_index = match strategy {
        ReorderStrategy::RotateForward => {
            ranges.rotate_right(rotate_by);
            // Like `usize::wrapping_add`, but provide a custom range from `0` to `ranges.len()`
            (selection.primary_index() + ranges.len() + rotate_by) % ranges.len()
        }
        ReorderStrategy::RotateBackward => {
            ranges.rotate_left(rotate_by);
            // Like `usize::wrapping_sub`, but provide a custom range from `0` to `ranges.len()`
            (selection.primary_index() + ranges.len() - rotate_by) % ranges.len()
        }
        ReorderStrategy::Reverse => {
            if rotate_by.is_multiple_of(2) {
                // nothing changed, if we reverse something an even
                // amount of times, the output will be the same
                return;
            }
            ranges.reverse();
            // -1 to turn 1-based len into 0-based index
            (ranges.len() - 1) - selection.primary_index()
        }
    };

    let transaction = Transaction::change(
        doc.text(),
        selection
            .ranges()
            .iter()
            .zip(ranges)
            .map(|(range, fragment)| (range.from(), range.to(), Some(fragment))),
    );

    doc.set_selection(
        view.id,
        Selection::new(selection.ranges().into(), primary_index),
    );
    doc.apply(&transaction, view.id);
}

pub(super) fn rotate_selection_contents_forward(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::RotateForward)
}
pub(super) fn rotate_selection_contents_backward(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::RotateBackward)
}
pub(super) fn reverse_selection_contents(cx: &mut Context) {
    reorder_selection_contents(cx, ReorderStrategy::Reverse)
}

fn get_lines(doc: &Document, view_id: ViewId) -> Vec<usize> {
    let mut lines = Vec::new();

    // Get all line numbers
    for range in doc.selection(view_id) {
        let (start, end) = range.line_range(doc.text().slice(..));

        for line in start..=end {
            lines.push(line)
        }
    }
    lines.sort_unstable(); // sorting by usize so _unstable is preferred
    lines.dedup();
    lines
}

pub(super) fn indent(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let lines = get_lines(doc, view.id);

    // Indent by one level
    let indent = Tendril::from(doc.indent_style.as_str().repeat(count));

    let transaction = Transaction::change(
        doc.text(),
        lines.into_iter().filter_map(|line| {
            let is_blank = doc.text().line(line).chunks().all(|s| s.trim().is_empty());
            if is_blank {
                return None;
            }
            let pos = doc.text().line_to_char(line);

            let indent = if let IndentStyle::Spaces(indent_width) = doc.indent_style {
                let line = doc.text().line(line);
                let offset = line.first_non_whitespace_char().unwrap_or(0) % indent_width as usize;
                indent.clone().split_off(offset)
            } else {
                indent.clone()
            };

            Some((pos, pos, Some(indent)))
        }),
    );
    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

pub(super) fn unindent(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let lines = get_lines(doc, view.id);
    let mut changes = Vec::with_capacity(lines.len());
    let tab_width = doc.tab_width();
    let indent_width = count * doc.indent_width();

    for line_idx in lines {
        let line = doc.text().line(line_idx);
        let mut width = 0;
        let mut pos = 0;

        for ch in line.chars() {
            match ch {
                ' ' => width += 1,
                '\t' => width = (width / tab_width + 1) * tab_width,
                _ => break,
            }

            pos += 1;

            if width >= indent_width {
                break;
            }
        }

        // now delete from start to first non-blank
        if pos > 0 {
            let start = doc.text().line_to_char(line_idx);
            changes.push((start, start + pos, None))
        }
    }

    let transaction = Transaction::change(doc.text(), changes.into_iter());

    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

// comments
type CommentTransactionFn = fn(
    line_token: Option<&str>,
    block_tokens: Option<&[BlockCommentToken]>,
    doc: &Rope,
    selection: &Selection,
) -> Transaction;

fn toggle_comments_impl(cx: &mut Context, comment_transaction: CommentTransactionFn) {
    let loader: &editor_core::syntax::Loader = &cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    let byte_pos = doc.text().char_to_byte(cursor);
    // Resolve the comment tokens from the enclosing injection layer that owns the comment,
    // not the innermost layer at the cursor. Prefer the innermost layer that defines
    // *line* comment tokens, falling back to the innermost layer with block tokens.
    let mut line_layer = None;
    let mut block_layer = None;
    if let Some(syntax) = doc.syntax() {
        for layer in syntax.layers_for_byte_range(byte_pos as u32, byte_pos as u32) {
            let language = syntax.layer(layer).language;
            let config = loader.language(language).config();
            if config.comment_tokens.is_some() {
                line_layer = Some(language);
            }
            if config.block_comment_tokens.is_some() {
                block_layer = Some(language);
            }
        }
    }
    let lang_config = line_layer
        .or(block_layer)
        .map(|language| &**loader.language(language).config())
        .or_else(|| doc.language_config());

    // Pick the token the cursor's line is already commented with (longest match, so `///` wins over `//`).
    // If the line isn't commented yet, fall back to the primary token for adding a comment.
    let cursor_line = doc.text().char_to_line(cursor);
    let line_token: Option<&str> = lang_config
        .and_then(|lc| lc.comment_tokens.as_ref())
        .and_then(|tokens| {
            comment::get_comment_token(doc.text().slice(..), tokens, cursor_line)
                .or_else(|| tokens.first().map(|token| token.as_str()))
        });
    let block_tokens: Option<&[BlockCommentToken]> = lang_config
        .and_then(|lc| lc.block_comment_tokens.as_ref())
        .map(|tc| &tc[..]);

    let transaction =
        comment_transaction(line_token, block_tokens, doc.text(), doc.selection(view.id));

    doc.apply(&transaction, view.id);
    exit_select_mode(cx);
}

/// commenting behavior:
/// 1. only line comment tokens -> line comment
/// 2. each line block commented -> uncomment all lines
/// 3. whole selection block commented -> uncomment selection
/// 4. all lines not commented and block tokens -> comment uncommented lines
/// 5. no comment tokens and not block commented -> line comment
pub(super) fn toggle_comments(cx: &mut Context) {
    toggle_comments_impl(cx, |line_token, block_tokens, doc, selection| {
        let text = doc.slice(..);

        // only have line comment tokens
        if line_token.is_some() && block_tokens.is_none() {
            return comment::toggle_line_comments(doc, selection, line_token);
        }

        let split_lines = comment::split_lines_of_selection(text, selection);

        let default_block_tokens = &[BlockCommentToken::default()];
        let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);

        let (line_commented, line_comment_changes) =
            comment::find_block_comments(block_comment_tokens, text, &split_lines);

        // block commented by line would also be block commented so check this first
        if line_commented {
            return comment::create_block_comment_transaction(
                doc,
                &split_lines,
                line_commented,
                line_comment_changes,
            )
            .0;
        }

        let (block_commented, comment_changes) =
            comment::find_block_comments(block_comment_tokens, text, selection);

        // check if selection has block comments
        if block_commented {
            return comment::create_block_comment_transaction(
                doc,
                selection,
                block_commented,
                comment_changes,
            )
            .0;
        }

        // not commented and only have block comment tokens
        if line_token.is_none() && block_tokens.is_some() {
            return comment::create_block_comment_transaction(
                doc,
                &split_lines,
                line_commented,
                line_comment_changes,
            )
            .0;
        }

        // not block commented at all and don't have any tokens
        comment::toggle_line_comments(doc, selection, line_token)
    })
}

pub(super) fn toggle_line_comments(cx: &mut Context) {
    toggle_comments_impl(cx, |line_token, block_tokens, doc, selection| {
        if line_token.is_none() && block_tokens.is_some() {
            let default_block_tokens = &[BlockCommentToken::default()];
            let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);
            comment::toggle_block_comments(
                doc,
                &comment::split_lines_of_selection(doc.slice(..), selection),
                block_comment_tokens,
            )
        } else {
            comment::toggle_line_comments(doc, selection, line_token)
        }
    });
}

pub(super) fn toggle_block_comments(cx: &mut Context) {
    toggle_comments_impl(cx, |line_token, block_tokens, doc, selection| {
        if line_token.is_some() && block_tokens.is_none() {
            comment::toggle_line_comments(doc, selection, line_token)
        } else {
            let default_block_tokens = &[BlockCommentToken::default()];
            let block_comment_tokens = block_tokens.unwrap_or(default_block_tokens);
            comment::toggle_block_comments(doc, selection, block_comment_tokens)
        }
    });
}

pub(super) fn replace_char(cx: &mut Context) {
    let mut buf = [0u8; 4]; // To hold utf8 encoded char.

    // need to wait for next key
    cx.on_next_key(move |cx, event| {
        let (view, doc) = current!(cx.editor);
        let ch: Option<&str> = match event {
            KeyEvent {
                code: KeyCode::Char(ch),
                ..
            } => Some(ch.encode_utf8(&mut buf[..])),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => Some(doc.line_ending.as_str()),
            KeyEvent {
                code: KeyCode::Tab, ..
            } => Some("\t"),
            _ => None,
        };

        let selection = doc.selection(view.id);

        if let Some(ch) = ch {
            let transaction = Transaction::change_by_selection(doc.text(), selection, |range| {
                if !range.is_empty() {
                    let text: Tendril = doc
                        .text()
                        .slice(range.from()..range.to())
                        .graphemes()
                        .map(|_g| ch)
                        .collect();
                    (range.from(), range.to(), Some(text))
                } else {
                    // No change.
                    (range.from(), range.to(), None)
                }
            });

            doc.apply(&transaction, view.id);
            exit_select_mode(cx);
        }
    })
}

pub(super) fn replace(cx: &mut Context) {
    let prompt = Prompt::new_with_callback(
        "replace:".into(),
        None,
        ui::completers::none,
        |_cx, input, event| {
            if event != PromptEvent::Validate {
                return None;
            }
            let replacement = Tendril::from(input);
            Some(Box::new(move |compositor, cx| {
                replace_selections(cx.editor, &replacement);
                compositor.find::<ui::EditorView>().unwrap().last_edit =
                    ui::editor::LastEdit::Replace(replacement);
            }))
        },
    );
    cx.push_layer(Box::new(prompt));
}

pub(crate) fn replace_selections(editor: &mut Editor, replacement: &str) {
    let scrolloff = editor.config().scrolloff;
    let (view, doc) = current!(editor);
    let replacement = Tendril::from(
        LINE_ENDING_REGEX
            .replace_all(replacement, doc.line_ending.as_str())
            .as_ref(),
    );
    let len = replacement.chars().count();
    let transaction =
        Transaction::change_by_and_with_selection(doc.text(), doc.selection(view.id), |range| {
            let selection =
                Range::new(range.from(), range.from() + len).with_direction(range.direction());
            (
                (range.from(), range.to(), Some(replacement.clone())),
                Some(selection),
            )
        });
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);
    if editor.mode == Mode::Select {
        editor.mode = Mode::Normal;
    }
}

enum Operation {
    Delete,
    Change,
}

fn selection_is_linewise(selection: &Selection, text: &Rope) -> bool {
    selection.ranges().iter().all(|range| {
        let text = text.slice(..);
        if range.slice(text).len_lines() < 2 {
            return false;
        }
        // If the start of the selection is at the start of a line and the end at the end of a line.
        let (start_line, end_line) = range.line_range(text);
        let start = text.line_to_char(start_line);
        let end = text.line_to_char((end_line + 1).min(text.len_lines()));
        start == range.from() && end == range.to()
    })
}

enum YankAction {
    Yank,
    NoYank,
}

fn delete_selection_impl(cx: &mut Context, op: Operation, yank: YankAction) {
    let (view, doc) = current!(cx.editor);

    let selection = doc.selection(view.id);
    let only_whole_lines = selection_is_linewise(selection, doc.text());

    if cx.register != Some('_') && matches!(yank, YankAction::Yank) {
        // yank the selection
        let text = doc.text().slice(..);
        let values: Vec<String> = selection.fragments(text).map(Cow::into_owned).collect();
        let reg_name = cx
            .register
            .unwrap_or_else(|| cx.editor.config.load().default_yank_register);
        if let Err(err) = cx.editor.registers.write(reg_name, values) {
            cx.editor.set_error(|| err.to_string());
            return;
        }
    }

    // delete the selection
    let transaction =
        Transaction::delete_by_selection(doc.text(), selection, |range| (range.from(), range.to()));
    doc.apply(&transaction, view.id);

    match op {
        Operation::Delete => {
            // exit select mode, if currently in select mode
            exit_select_mode(cx);
        }
        Operation::Change => {
            if only_whole_lines {
                open(cx, Open::Above, CommentContinuation::Disabled);
            } else {
                enter_insert_mode(cx);
            }
        }
    }
}

pub(super) fn delete_selection(cx: &mut Context) {
    delete_selection_impl(cx, Operation::Delete, YankAction::Yank);
}

pub(super) fn delete_selection_noyank(cx: &mut Context) {
    delete_selection_impl(cx, Operation::Delete, YankAction::NoYank);
}

pub(super) fn change_selection(cx: &mut Context) {
    delete_selection_impl(cx, Operation::Change, YankAction::Yank);
}

pub(super) fn change_selection_noyank(cx: &mut Context) {
    delete_selection_impl(cx, Operation::Change, YankAction::NoYank);
}

static SURROUND_HELP_TEXT: [(&str, &str); 6] = [
    ("m", "Nearest matching pair"),
    ("( or )", "Parentheses"),
    ("{ or }", "Curly braces"),
    ("< or >", "Angled brackets"),
    ("[ or ]", "Square brackets"),
    (" ", "... or any character"),
];

pub(super) fn surround_add(cx: &mut Context) {
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        let (view, doc) = current!(cx.editor);
        // surround_len is the number of new characters being added.
        let (open, close, surround_len) = match event.char() {
            Some(ch) => {
                let (o, c) = match_brackets::get_pair(ch);
                let mut open = Tendril::new();
                open.push(o);
                let mut close = Tendril::new();
                close.push(c);
                (open, close, 2)
            }
            None if event.code == KeyCode::Enter => (
                doc.line_ending.as_str().into(),
                doc.line_ending.as_str().into(),
                2 * doc.line_ending.len_chars(),
            ),
            None => return,
        };

        let selection = doc.selection(view.id);
        let mut changes = Vec::with_capacity(selection.len() * 2);
        let mut ranges = SmallVec::with_capacity(selection.len());
        let mut offs = 0;

        for range in selection.iter() {
            changes.push((range.from(), range.from(), Some(open.clone())));
            changes.push((range.to(), range.to(), Some(close.clone())));

            ranges.push(
                Range::new(offs + range.from(), offs + range.to() + surround_len)
                    .with_direction(range.direction()),
            );

            offs += surround_len;
        }

        let transaction = Transaction::change(doc.text(), changes.into_iter())
            .with_selection(Selection::new(ranges, selection.primary_index()));
        doc.apply(&transaction, view.id);
        exit_select_mode(cx);
    });

    cx.editor.autoinfo = Some(Info::new(
        "Surround selections with",
        &SURROUND_HELP_TEXT[1..],
    ));
}

pub(super) fn surround_replace(cx: &mut Context) {
    let count = cx.count();
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        let surround_ch = match event.char() {
            Some('m') => None, // m selects the closest surround pair
            Some(ch) => Some(ch),
            None => return,
        };
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);

        let change_pos =
            match surround::get_surround_pos(doc.syntax(), text, selection, surround_ch, count) {
                Ok(c) => c,
                Err(err) => {
                    cx.editor.set_error(|| err.to_string());
                    return;
                }
            };

        let selection = selection.clone();
        let ranges: SmallVec<[Range; 1]> = change_pos.iter().map(|&p| Range::point(p)).collect();
        doc.set_selection(
            view.id,
            Selection::new(ranges, selection.primary_index() * 2),
        );

        cx.on_next_key(move |cx, event| {
            cx.editor.autoinfo = None;
            let (view, doc) = current!(cx.editor);
            let to = match event.char() {
                Some(to) => to,
                None => return doc.set_selection(view.id, selection),
            };
            let (open, close) = match_brackets::get_pair(to);

            // the changeset has to be sorted to allow nested surrounds
            let mut sorted_pos: Vec<(usize, char)> = Vec::new();
            for p in change_pos.chunks(2) {
                sorted_pos.push((p[0], open));
                sorted_pos.push((p[1], close));
            }
            sorted_pos.sort_unstable();

            let transaction = Transaction::change(
                doc.text(),
                sorted_pos.iter().map(|&pos| {
                    let mut t = Tendril::new();
                    t.push(pos.1);
                    (pos.0, pos.0 + 1, Some(t))
                }),
            );
            doc.set_selection(view.id, selection);
            doc.apply(&transaction, view.id);
            exit_select_mode(cx);
        });

        cx.editor.autoinfo = Some(Info::new(
            "Replace with a pair of",
            &SURROUND_HELP_TEXT[1..],
        ));
    });

    cx.editor.autoinfo = Some(Info::new(
        "Replace surrounding pair of",
        &SURROUND_HELP_TEXT,
    ));
}

pub(super) fn surround_delete(cx: &mut Context) {
    let count = cx.count();
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        let surround_ch = match event.char() {
            Some('m') => None, // m selects the closest surround pair
            Some(ch) => Some(ch),
            None => return,
        };
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);

        let mut change_pos =
            match surround::get_surround_pos(doc.syntax(), text, selection, surround_ch, count) {
                Ok(c) => c,
                Err(err) => {
                    cx.editor.set_error(|| err.to_string());
                    return;
                }
            };
        change_pos.sort_unstable(); // the changeset has to be sorted to allow nested surrounds
        let transaction =
            Transaction::change(doc.text(), change_pos.into_iter().map(|p| (p, p + 1, None)));
        doc.apply(&transaction, view.id);
        exit_select_mode(cx);
    });

    cx.editor.autoinfo = Some(Info::new("Delete surrounding pair of", &SURROUND_HELP_TEXT));
}

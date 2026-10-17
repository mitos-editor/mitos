//! Text transformations and editing commands over selections.

use super::{
    context::Context,
    insert::{open, CommentContinuation, Open},
    mode::{enter_insert_mode, exit_select_mode},
};
use crate::ui::{self, Prompt, PromptEvent};
use arc_swap::access::DynAccess;
use editor_core::{
    increment, match_brackets, Range, Rope, Selection, SmallVec, Tendril, Transaction,
};
use language::surround;
use std::borrow::Cow;
use stdx::rope::RopeSliceExt;
use ui_core::{input::KeyEvent, keyboard::KeyCode};
use view::{document::Mode, info::Info, Editor};

pub(super) fn align_selections(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let Some(transaction) = editor_core::transform::align_selections(
        doc.text(),
        doc.selection(view.id),
        doc.tab_width(),
    ) else {
        cx.editor
            .set_error(|| "align cannot work with multi line selections");
        return;
    };
    view::editing::apply(doc, &transaction, view.id);
    exit_select_mode(cx);
}

fn apply_transform(cx: &mut Context, operation: fn(&Rope, &Selection) -> Transaction) {
    let (view, doc) = current!(cx.editor);
    let transaction = operation(doc.text(), doc.selection(view.id));
    view::editing::apply(doc, &transaction, view.id);
    exit_select_mode(cx);
}

pub(super) fn switch_case(cx: &mut Context) {
    apply_transform(cx, editor_core::transform::switch_case);
}

pub(super) fn switch_to_uppercase(cx: &mut Context) {
    apply_transform(cx, editor_core::transform::switch_to_uppercase);
}

pub(super) fn switch_to_lowercase(cx: &mut Context) {
    apply_transform(cx, editor_core::transform::switch_to_lowercase);
}

pub(super) fn join_selections(cx: &mut Context) {
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    view::editing::join_selections(doc, view.id, &loader, false);
}

pub(super) fn join_selections_space(cx: &mut Context) {
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    view::editing::join_selections(doc, view.id, &loader, true);
}

fn reorder_selection_contents(cx: &mut Context, strategy: editor_core::transform::ReorderStrategy) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    if let Some((transaction, selection)) = editor_core::transform::reorder_selection_contents(
        doc.text(),
        doc.selection(view.id),
        strategy,
        count,
    ) {
        doc.set_selection(view.id, selection);
        view::editing::apply(doc, &transaction, view.id);
    }
}

pub(super) fn rotate_selection_contents_forward(cx: &mut Context) {
    reorder_selection_contents(cx, editor_core::transform::ReorderStrategy::RotateForward);
}

pub(super) fn rotate_selection_contents_backward(cx: &mut Context) {
    reorder_selection_contents(cx, editor_core::transform::ReorderStrategy::RotateBackward);
}

pub(super) fn reverse_selection_contents(cx: &mut Context) {
    reorder_selection_contents(cx, editor_core::transform::ReorderStrategy::Reverse);
}

pub(super) fn indent(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let transaction =
        editor_core::transform::indent(doc.text(), doc.selection(view.id), doc.indent_style, count);
    view::editing::apply(doc, &transaction, view.id);
    exit_select_mode(cx);
}

pub(super) fn unindent(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let transaction = editor_core::transform::unindent(
        doc.text(),
        doc.selection(view.id),
        doc.tab_width(),
        count * doc.indent_width(),
    );
    view::editing::apply(doc, &transaction, view.id);
    exit_select_mode(cx);
}

pub(super) fn toggle_comments(cx: &mut Context) {
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    view::editing::toggle_comments(doc, view.id, &loader);
    exit_select_mode(cx);
}

pub(super) fn toggle_line_comments(cx: &mut Context) {
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    view::editing::toggle_line_comments(doc, view.id, &loader);
    exit_select_mode(cx);
}

pub(super) fn toggle_block_comments(cx: &mut Context) {
    let loader = cx.editor.syn_loader.load();
    let (view, doc) = current!(cx.editor);
    view::editing::toggle_block_comments(doc, view.id, &loader);
    exit_select_mode(cx);
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

            view::editing::apply(doc, &transaction, view.id);
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
    let scrolloff = editor.config().scroll.padding;
    let (view, doc) = current!(editor);
    view::editing::replace_selections(doc, view, replacement);
    view.ensure_cursor_in_view(doc, scrolloff);
    if editor.mode() == Mode::Select {
        editor.set_mode(Mode::Normal);
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
            .unwrap_or_else(|| cx.editor.config.load().clipboard.default_yank_register);
        if let Err(err) = cx.editor.registers.write(reg_name, values) {
            cx.editor.set_error(|| err.to_string());
            return;
        }
    }

    // delete the selection
    let transaction =
        Transaction::delete_by_selection(doc.text(), selection, |range| (range.from(), range.to()));
    view::editing::apply(doc, &transaction, view.id);

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
        view::editing::apply(doc, &transaction, view.id);
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
            view::editing::apply(doc, &transaction, view.id);
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
        view::editing::apply(doc, &transaction, view.id);
        exit_select_mode(cx);
    });

    cx.editor.autoinfo = Some(Info::new("Delete surrounding pair of", &SURROUND_HELP_TEXT));
}

enum IncrementDirection {
    Increase,
    Decrease,
}

/// Increment objects within selections by count.
pub(super) fn increment(cx: &mut Context) {
    increment_impl(cx, IncrementDirection::Increase);
}

/// Decrement objects within selections by count.
pub(super) fn decrement(cx: &mut Context) {
    increment_impl(cx, IncrementDirection::Decrease);
}

/// Increment objects within selections by `amount`.
/// A negative `amount` will decrement objects within selections.
fn increment_impl(cx: &mut Context, increment_direction: IncrementDirection) {
    let sign = match increment_direction {
        IncrementDirection::Increase => 1,
        IncrementDirection::Decrease => -1,
    };
    let mut amount = sign * cx.count() as i64;
    // If the register is `#` then increase or decrease the `amount` by 1 per element
    let increase_by = if cx.register == Some('#') { sign } else { 0 };

    let (view, doc) = current!(cx.editor);
    let selection = doc.selection(view.id);
    let text = doc.text().slice(..);

    let mut new_selection_ranges = SmallVec::new();
    let mut cumulative_length_diff: i128 = 0;
    let mut changes = vec![];

    for range in selection {
        let selected_text: Cow<str> = range.fragment(text);
        let new_from = ((range.from() as i128) + cumulative_length_diff) as usize;
        let incremented = [increment::integer, increment::date_time]
            .iter()
            .find_map(|incrementor| incrementor(selected_text.as_ref(), amount));

        amount += increase_by;

        match incremented {
            None => {
                let new_range = Range::new(
                    new_from,
                    (range.to() as i128 + cumulative_length_diff) as usize,
                );
                new_selection_ranges.push(new_range);
            }
            Some(new_text) => {
                let new_range = Range::new(new_from, new_from + new_text.len());
                cumulative_length_diff += new_text.len() as i128 - selected_text.len() as i128;
                new_selection_ranges.push(new_range);
                changes.push((range.from(), range.to(), Some(new_text.into())));
            }
        }
    }

    if !changes.is_empty() {
        let new_selection = Selection::new(new_selection_ranges, selection.primary_index());
        let transaction = Transaction::change(doc.text(), changes.into_iter());
        let transaction = transaction.with_selection(new_selection);
        view::editing::apply(doc, &transaction, view.id);
        exit_select_mode(cx);
    }
}

pub(super) mod typed {
    //! Typable editing commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::bail;
    use editor_core::{Tendril, Transaction};

    pub(in crate::commands) fn sort(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let scrolloff = cx.editor.config().scroll.padding;
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);

        let selection = doc.selection(view.id);

        if selection.len() == 1 {
            bail!("Sorting requires multiple selections. Hint: split selection first");
        }

        let mut fragments: Vec<_> = selection
            .slices(text)
            .map(|fragment| fragment.chunks().collect())
            .collect();

        fragments.sort_by(
            match (args.has_flag("insensitive"), args.has_flag("reverse")) {
                (true, true) => |a: &Tendril, b: &Tendril| b.to_lowercase().cmp(&a.to_lowercase()),
                (true, false) => |a: &Tendril, b: &Tendril| a.to_lowercase().cmp(&b.to_lowercase()),
                (false, true) => |a: &Tendril, b: &Tendril| b.cmp(a),
                (false, false) => |a: &Tendril, b: &Tendril| a.cmp(b),
            },
        );

        let transaction = Transaction::change(
            doc.text(),
            selection
                .into_iter()
                .zip(fragments)
                .map(|(s, fragment)| (s.from(), s.to(), Some(fragment))),
        );

        view::editing::apply(doc, &transaction, view.id);
        view::editing::commit(doc, view);
        view.ensure_cursor_in_view(doc, scrolloff);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn reflow(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let scrolloff = cx.editor.config().scroll.padding;
        let (view, doc) = current!(cx.editor);

        // Find the text_width by checking the following sources in order:
        //   - The passed argument in `args`
        //   - The configured text-width for this language in languages.toml
        //   - The configured text-width in the config.toml
        let text_width: usize = args
            .first()
            .map(|num| num.parse::<usize>())
            .transpose()?
            .unwrap_or_else(|| doc.text_width());

        let rope = doc.text();

        let selection = doc.selection(view.id);
        let transaction = Transaction::change_by_selection(rope, selection, |range| {
            let fragment = range.fragment(rope.slice(..));
            let reflowed_text = editor_core::wrap::reflow_hard_wrap(&fragment, text_width);

            (range.from(), range.to(), Some(reflowed_text))
        });

        view::editing::apply(doc, &transaction, view.id);
        view::editing::commit(doc, view);
        view.ensure_cursor_in_view(doc, scrolloff);

        Ok(())
    }
}

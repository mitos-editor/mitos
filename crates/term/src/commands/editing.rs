//! Text transformations and editing commands over selections.

use super::{context::Context, mode::exit_select_mode};
use crate::ui::{self, Prompt, PromptEvent};
use editor_core::{Rope, Selection, Tendril, Transaction};
use ui_core::{input::KeyEvent, keyboard::KeyCode};
use view::editing::surround::{self, Delimiter, Replacement};
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
    if let Err(error) = view::editing::try_apply(doc, &transaction, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
    exit_select_mode(cx);
}

fn apply_transform(cx: &mut Context, operation: fn(&Rope, &Selection) -> Transaction) {
    let (view, doc) = current!(cx.editor);
    let transaction = operation(doc.text(), doc.selection(view.id));
    if let Err(error) = view::editing::try_apply(doc, &transaction, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
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
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::join_selections(doc, view.id, false) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(super) fn join_selections_space(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::join_selections(doc, view.id, true) {
        cx.editor.set_error(|| error.to_string());
    }
}

fn reorder_selection_contents(cx: &mut Context, strategy: editor_core::transform::ReorderStrategy) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::reorder_selection_contents(doc, view.id, strategy, count) {
        cx.editor.set_error(|| error.to_string());
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
    if let Err(error) = view::editing::try_apply(doc, &transaction, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
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
    if let Err(error) = view::editing::try_apply(doc, &transaction, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
    exit_select_mode(cx);
}

pub(super) fn toggle_comments(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::toggle_comments(doc, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
    exit_select_mode(cx);
}

pub(super) fn toggle_line_comments(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::toggle_line_comments(doc, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
    exit_select_mode(cx);
}

pub(super) fn toggle_block_comments(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    if let Err(error) = view::editing::toggle_block_comments(doc, view.id) {
        cx.editor.set_error(|| error.to_string());
        return;
    }
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
            let transaction = editor_core::transform::replace_graphemes(doc.text(), selection, ch);
            if let Err(error) = view::editing::try_apply(doc, &transaction, view.id) {
                cx.editor.set_error(|| error.to_string());
                return;
            }
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
    if let Err(error) = view::editing::replace_selections(doc, view, replacement) {
        editor.set_error(|| error.to_string());
        return;
    }
    view.ensure_cursor_in_view(doc, scrolloff);
    if editor.mode() == Mode::Select {
        editor.set_mode(Mode::Normal);
    }
}

enum Operation {
    Delete,
    Change,
}

enum YankAction {
    Yank,
    NoYank,
}

fn delete_selection_impl(cx: &mut Context, operation: Operation, yank: YankAction) {
    let register = matches!(yank, YankAction::Yank).then(|| {
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register)
    });
    let result = match operation {
        Operation::Change => cx.editor.change_selections(register, cx.count()),
        Operation::Delete => cx.editor.delete_selections(register),
    };
    if let Err(error) = result {
        cx.editor.set_error(|| error.to_string());
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
        let delimiter = match event.char() {
            Some(ch) => Delimiter::Character(ch),
            None if event.code == KeyCode::Enter => Delimiter::LineEnding,
            None => return,
        };
        let (view, doc) = current!(cx.editor);
        match surround::add(doc, view.id, delimiter) {
            Ok(()) => exit_select_mode(cx),
            Err(err) => cx.editor.set_error(|| err.to_string()),
        }
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
        let character = match event.char() {
            Some('m') => None,
            Some(ch) => Some(ch),
            None => return,
        };
        let (view, doc) = current!(cx.editor);
        let replacement = match Replacement::prepare(doc, view.id, character, count) {
            Ok(replacement) => replacement,
            Err(err) => {
                cx.editor.set_error(|| err.to_string());
                return;
            }
        };
        cx.on_next_key(move |cx, event| {
            cx.editor.autoinfo = None;
            let (view, doc) = current!(cx.editor);
            let Some(character) = event.char() else {
                replacement.cancel(doc, view.id);
                return;
            };
            match replacement.apply(doc, view.id, character) {
                Ok(()) => exit_select_mode(cx),
                Err(err) => cx.editor.set_error(|| err.to_string()),
            }
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
        let character = match event.char() {
            Some('m') => None,
            Some(ch) => Some(ch),
            None => return,
        };
        let (view, doc) = current!(cx.editor);
        match surround::delete(doc, view.id, character, count) {
            Ok(()) => exit_select_mode(cx),
            Err(err) => cx.editor.set_error(|| err.to_string()),
        }
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
    let amount = sign * i64::try_from(cx.count()).unwrap_or(i64::MAX);
    let step = if cx.register == Some('#') { sign } else { 0 };
    let (view, doc) = current!(cx.editor);
    let Some(transaction) = editor_core::transform::increment_selections(
        doc.text(),
        doc.selection(view.id),
        amount,
        step,
    ) else {
        return;
    };
    match view::editing::try_apply(doc, &transaction, view.id) {
        Ok(()) => exit_select_mode(cx),
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

pub(super) mod typed {
    //! Typable editing commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::bail;

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
        let Some(transaction) = editor_core::transform::sort_selection_contents(
            doc.text(),
            doc.selection(view.id),
            args.has_flag("insensitive"),
            args.has_flag("reverse"),
        ) else {
            bail!("Sorting requires multiple selections. Hint: split selection first");
        };
        view::editing::try_apply(doc, &transaction, view.id)?;
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

        let transaction = editor_core::transform::reflow_selections(
            doc.text(),
            doc.selection(view.id),
            text_width,
            doc.line_ending,
        );
        view::editing::try_apply(doc, &transaction, view.id)?;
        view::editing::commit(doc, view);
        view.ensure_cursor_in_view(doc, scrolloff);

        Ok(())
    }
}

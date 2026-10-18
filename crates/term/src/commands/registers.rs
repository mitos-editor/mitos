//! Register and clipboard commands: yanking, pasting, replacement, and register selection.

use super::{context::Context, mode::exit_select_mode};
use editor_core::transform::PastePosition;
use std::borrow::Cow;
use view::{document::Mode, info::Info, Editor};

pub(super) fn yank(cx: &mut Context) {
    yank_impl(
        cx.editor,
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register),
    );
    exit_select_mode(cx);
}

pub(super) fn yank_to_clipboard(cx: &mut Context) {
    yank_impl(cx.editor, '+');
    exit_select_mode(cx);
}

pub(super) fn yank_to_primary_clipboard(cx: &mut Context) {
    yank_impl(cx.editor, '*');
    exit_select_mode(cx);
}

fn yank_impl(editor: &mut Editor, register: char) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let values: Vec<String> = doc
        .selection(view.id)
        .fragments(text)
        .map(Cow::into_owned)
        .collect();
    let selections = values.len();

    match editor.registers.write(register, values) {
        Ok(_) => editor.set_status(format!(
            "yanked {selections} selection{} to register {register}",
            if selections == 1 { "" } else { "s" }
        )),
        Err(err) => editor.set_error(|| err.to_string()),
    }
}

pub(super) fn yank_joined_impl(editor: &mut Editor, separator: &str, register: char) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id);
    let selections = selection.len();
    let joined = selection
        .fragments(text)
        .fold(String::new(), |mut acc, fragment| {
            if !acc.is_empty() {
                acc.push_str(separator);
            }
            acc.push_str(&fragment);
            acc
        });

    match editor.registers.write(register, vec![joined]) {
        Ok(_) => editor.set_status(format!(
            "joined and yanked {selections} selection{} to register {register}",
            if selections == 1 { "" } else { "s" }
        )),
        Err(err) => editor.set_error(|| err.to_string()),
    }
}

pub(super) fn yank_joined(cx: &mut Context) {
    let separator = doc!(cx.editor).line_ending.as_str();
    yank_joined_impl(
        cx.editor,
        separator,
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register),
    );
    exit_select_mode(cx);
}

pub(super) fn yank_joined_to_clipboard(cx: &mut Context) {
    let line_ending = doc!(cx.editor).line_ending;
    yank_joined_impl(cx.editor, line_ending.as_str(), '+');
    exit_select_mode(cx);
}

pub(super) fn yank_joined_to_primary_clipboard(cx: &mut Context) {
    let line_ending = doc!(cx.editor).line_ending;
    yank_joined_impl(cx.editor, line_ending.as_str(), '*');
    exit_select_mode(cx);
}

pub(crate) fn yank_main_selection_to_register(editor: &mut Editor, register: char) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id).primary().fragment(text).to_string();

    match editor.registers.write(register, vec![selection]) {
        Ok(_) => editor.set_status(format!("yanked primary selection to register {register}",)),
        Err(err) => editor.set_error(|| err.to_string()),
    }
}

pub(super) fn yank_main_selection_to_clipboard(cx: &mut Context) {
    yank_main_selection_to_register(cx.editor, '+');
    exit_select_mode(cx);
}

pub(super) fn yank_main_selection_to_primary_clipboard(cx: &mut Context) {
    yank_main_selection_to_register(cx.editor, '*');
    exit_select_mode(cx);
}

pub(crate) fn paste_bracketed_value(cx: &mut Context, contents: String) {
    let count = cx.count();
    let paste = match cx.editor.mode() {
        Mode::Insert | Mode::Select => PastePosition::Cursor,
        Mode::Normal => PastePosition::Before,
    };
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);
    view::editing::paste(doc, view, &[contents], paste, count, mode);
    exit_select_mode(cx);
}

pub(super) fn paste_clipboard_after(cx: &mut Context) {
    paste(cx.editor, '+', PastePosition::After, cx.count());
    exit_select_mode(cx);
}

pub(super) fn paste_clipboard_before(cx: &mut Context) {
    paste(cx.editor, '+', PastePosition::Before, cx.count());
    exit_select_mode(cx);
}

pub(super) fn paste_primary_clipboard_after(cx: &mut Context) {
    paste(cx.editor, '*', PastePosition::After, cx.count());
    exit_select_mode(cx);
}

pub(super) fn paste_primary_clipboard_before(cx: &mut Context) {
    paste(cx.editor, '*', PastePosition::Before, cx.count());
    exit_select_mode(cx);
}

pub(super) fn replace_with_yanked(cx: &mut Context) {
    replace_selections_with_register(
        cx.editor,
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register),
        cx.count(),
    );
    exit_select_mode(cx);
}

pub(crate) fn replace_selections_with_register(editor: &mut Editor, register: char, count: usize) {
    let Some(values) = editor
        .registers
        .read(register, editor)
        .filter(|values| values.len() > 0)
    else {
        return;
    };
    let scrolloff = editor.config().scroll.padding;
    let values: Vec<_> = values.map(Cow::into_owned).collect();
    let (view, doc) = current!(editor);
    view::editing::replace_selection_contents(doc, view, &values, count);
    view.ensure_cursor_in_view(doc, scrolloff);
}

pub(super) fn replace_selections_with_clipboard(cx: &mut Context) {
    replace_selections_with_register(cx.editor, '+', cx.count());
    exit_select_mode(cx);
}

pub(super) fn replace_selections_with_primary_clipboard(cx: &mut Context) {
    replace_selections_with_register(cx.editor, '*', cx.count());
    exit_select_mode(cx);
}

pub(crate) fn paste(editor: &mut Editor, register: char, pos: PastePosition, count: usize) {
    let Some(values) = editor.registers.read(register, editor) else {
        return;
    };
    let values: Vec<_> = values.map(|value| value.to_string()).collect();

    let mode = editor.mode();
    let (view, doc) = current!(editor);
    view::editing::paste(doc, view, &values, pos, count, mode);
}

pub(super) fn paste_after(cx: &mut Context) {
    paste(
        cx.editor,
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register),
        PastePosition::After,
        cx.count(),
    );
    exit_select_mode(cx);
}

pub(super) fn paste_before(cx: &mut Context) {
    paste(
        cx.editor,
        cx.register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register),
        PastePosition::Before,
        cx.count(),
    );
    exit_select_mode(cx);
}

pub(super) fn select_register(cx: &mut Context) {
    cx.editor.autoinfo = Some(Info::from_registers(
        "Select register",
        &cx.editor.registers,
    ));
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        if let Some(ch) = event.char() {
            cx.editor.selected_register = Some(ch);
        }
    })
}

pub(super) fn insert_register(cx: &mut Context) {
    // TODO: count is reset to 1 before next key so we move it into the closure here.
    // Would be nice to carry over.
    let count = cx.count();
    cx.editor.autoinfo = Some(Info::from_registers(
        "Insert register",
        &cx.editor.registers,
    ));
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        if let Some(ch) = event.char() {
            cx.register = Some(ch);
            paste(
                cx.editor,
                cx.register
                    .unwrap_or(cx.editor.config().clipboard.default_yank_register),
                PastePosition::Cursor,
                count,
            );
        }
    })
}

pub(super) fn copy_between_registers(cx: &mut Context) {
    cx.editor.autoinfo = Some(Info::from_registers(
        "Copy from register",
        &cx.editor.registers,
    ));
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;

        let Some(source) = event.char() else {
            return;
        };

        let Some(values) = cx.editor.registers.read(source, cx.editor) else {
            cx.editor
                .set_error(|| format!("register {source} is empty"));
            return;
        };
        let values: Vec<_> = values.map(|value| value.to_string()).collect();

        cx.editor.autoinfo = Some(Info::from_registers(
            "Copy into register",
            &cx.editor.registers,
        ));
        cx.on_next_key(move |cx, event| {
            cx.editor.autoinfo = None;

            let Some(dest) = event.char() else {
                return;
            };

            let n_values = values.len();
            match cx.editor.registers.write(dest, values) {
                Ok(_) => cx.editor.set_status(format!(
                    "yanked {n_values} value{} from register {source} to {dest}",
                    if n_values == 1 { "" } else { "s" }
                )),
                Err(err) => cx.editor.set_error(|| err.to_string()),
            }
        });
    });
}

pub(super) mod typed {
    //! Typable registers commands.

    use crate::{
        commands::registers::{
            paste, replace_selections_with_register, yank_joined_impl,
            yank_main_selection_to_register,
        },
        compositor,
        ui::PromptEvent,
    };
    use ::command_line::Args;
    use anyhow::ensure;
    use editor_core::transform::PastePosition;
    use std::borrow::Cow;

    #[cold]
    pub(in crate::commands) fn yank_main_selection_to_clipboard(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        yank_main_selection_to_register(cx.editor, '+');
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn yank_joined(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let doc = doc!(cx.editor);
        let default_sep = Cow::Borrowed(doc.line_ending.as_str());
        let separator = args.first().unwrap_or(&default_sep);
        let register = cx
            .editor
            .selected_register
            .unwrap_or(cx.editor.config().clipboard.default_yank_register);
        yank_joined_impl(cx.editor, separator, register);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn yank_joined_to_clipboard(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let doc = doc!(cx.editor);
        let default_sep = Cow::Borrowed(doc.line_ending.as_str());
        let separator = args.first().unwrap_or(&default_sep);
        yank_joined_impl(cx.editor, separator, '+');
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn yank_main_selection_to_primary_clipboard(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        yank_main_selection_to_register(cx.editor, '*');
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn yank_joined_to_primary_clipboard(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let doc = doc!(cx.editor);
        let default_sep = Cow::Borrowed(doc.line_ending.as_str());
        let separator = args.first().unwrap_or(&default_sep);
        yank_joined_impl(cx.editor, separator, '*');
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn paste_clipboard_after(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        paste(cx.editor, '+', PastePosition::After, 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn paste_clipboard_before(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        paste(cx.editor, '+', PastePosition::Before, 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn paste_primary_clipboard_after(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        paste(cx.editor, '*', PastePosition::After, 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn paste_primary_clipboard_before(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        paste(cx.editor, '*', PastePosition::Before, 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn replace_selections_with_clipboard(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        replace_selections_with_register(cx.editor, '+', 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn replace_selections_with_primary_clipboard(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        replace_selections_with_register(cx.editor, '*', 1);
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn show_clipboard_provider(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        cx.editor
            .set_status(cx.editor.registers.clipboard_provider_name());
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn clear_register(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if args.is_empty() {
            cx.editor.registers.clear();
            cx.editor.set_status("All registers cleared");
            return Ok(());
        }

        ensure!(
            args[0].chars().count() == 1,
            format!("Invalid register {}", &args[0])
        );
        let register = args[0].chars().next().unwrap_or_default();
        if cx.editor.registers.remove(register) {
            cx.editor
                .set_status(format!("Register {} cleared", register));
        } else {
            cx.editor
                .set_error(|| format!("Register {} not found", register));
        }
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn set_register(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        ensure!(
            args[0].chars().count() == 1,
            format!("Invalid register {}", &args[0])
        );

        let register = args[0].chars().next().unwrap_or_default();
        cx.editor.registers.write(register, vec![args[1].into()])
    }
}

//! Edit-history commands: count adaptation, history limits, and explicit checkpoints.

use super::context::Context;
use buffer::history::UndoKind;

pub(super) fn undo(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        if !view::editing::undo(doc, view) {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn redo(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        if !view::editing::redo(doc, view) {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn earlier(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !view::editing::earlier(doc, view, UndoKind::Steps(1)) {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn later(cx: &mut Context) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !view::editing::later(doc, view, UndoKind::Steps(1)) {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn commit_undo_checkpoint(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    view::editing::commit(doc, view);
}

pub(super) mod typed {
    //! Typable history commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::anyhow;
    use buffer::history::UndoKind;

    #[cold]
    pub(in crate::commands) fn earlier(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let uk = args.join(" ").parse::<UndoKind>().map_err(|s| anyhow!(s))?;

        let (view, doc) = current!(cx.editor);
        let success = view::editing::earlier(doc, view, uk);
        if !success {
            cx.editor.set_status("Already at oldest change");
        }

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn later(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let uk = args.join(" ").parse::<UndoKind>().map_err(|s| anyhow!(s))?;
        let (view, doc) = current!(cx.editor);
        let success = view::editing::later(doc, view, uk);
        if !success {
            cx.editor.set_status("Already at newest change");
        }

        Ok(())
    }
}

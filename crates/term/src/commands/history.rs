//! Edit-history commands: count adaptation, history limits, and explicit checkpoints.

use super::context::Context;
use buffer::{history::UndoKind, HistoryAction};

pub(super) fn undo(cx: &mut Context) {
    let count = cx.count();
    for _ in 0..count {
        if !cx.editor.change_history(HistoryAction::Undo) {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn redo(cx: &mut Context) {
    let count = cx.count();
    for _ in 0..count {
        if !cx.editor.change_history(HistoryAction::Redo) {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn earlier(cx: &mut Context) {
    let count = cx.count();
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !cx
            .editor
            .change_history(HistoryAction::Earlier(UndoKind::Steps(1)))
        {
            cx.editor.set_status("Already at oldest change");
            break;
        }
    }
}

pub(super) fn later(cx: &mut Context) {
    let count = cx.count();
    for _ in 0..count {
        // rather than doing in batch we do this so get error halfway
        if !cx
            .editor
            .change_history(HistoryAction::Later(UndoKind::Steps(1)))
        {
            cx.editor.set_status("Already at newest change");
            break;
        }
    }
}

pub(super) fn commit_undo_checkpoint(cx: &mut Context) {
    cx.editor.commit_changes();
}

pub(super) mod typed {
    //! Typable history commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::anyhow;
    use buffer::{history::UndoKind, HistoryAction};

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

        let success = cx.editor.change_history(HistoryAction::Earlier(uk));
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
        let success = cx.editor.change_history(HistoryAction::Later(uk));
        if !success {
            cx.editor.set_status("Already at newest change");
        }

        Ok(())
    }
}

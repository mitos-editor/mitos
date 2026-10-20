//! Recording and replay of the terminal's last insert or replacement operation.

use std::sync::Arc;

use editor_core::{Change, Tendril};
use ui_core::input::KeyEvent;
use view::{
    document::{Mode, SavePoint},
    editing::replay,
};

use crate::commands::{self, Context, MappableCommand};

#[derive(Clone)]
pub(crate) enum RepeatPlan {
    Insert(MappableCommand, Vec<InsertEvent>),
    Replace(Tendril),
}

#[derive(Debug, Clone)]
pub(crate) enum InsertEvent {
    Key(KeyEvent),
    CompletionApply {
        trigger_offset: usize,
        changes: Vec<Change>,
    },
    InlineCompletionApply(view::features::inline_completion::AppliedInlineCompletion),
    TriggerCompletion,
    RequestCompletion,
}

#[derive(Default)]
pub(crate) struct RepeatController {
    last_edit: Option<RepeatPlan>,
    replaying: bool,
}

impl RepeatController {
    pub fn start_insert(&mut self, command: MappableCommand) {
        if !self.replaying {
            self.last_edit = Some(RepeatPlan::Insert(command, Vec::new()));
        }
    }

    pub fn replace(&mut self, text: Tendril) {
        if !self.replaying {
            self.last_edit = Some(RepeatPlan::Replace(text));
        }
    }

    pub fn record(&mut self, event: InsertEvent) {
        if !self.replaying
            && let Some(RepeatPlan::Insert(_, events)) = &mut self.last_edit
        {
            events.push(event);
        }
    }

    /// Snapshot once for all counts. Mode transitions and input during replay
    /// must not overwrite the operation that a subsequent dot will repeat.
    pub fn begin_replay(&mut self) -> Option<RepeatPlan> {
        let plan = self.last_edit.clone()?;
        self.replaying = true;
        Some(plan)
    }

    pub fn end_replay(&mut self) {
        self.replaying = false;
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RepeatError {
    #[error("Cannot repeat insertion: the entry command did not enter insert mode")]
    EntryCommand,
    #[error(transparent)]
    Replay(#[from] replay::ReplayError),
    #[error(transparent)]
    Edit(#[from] view::editing::EditError),
}

impl RepeatPlan {
    pub fn replay(
        &self,
        cx: &mut Context,
        count: usize,
        mut insert_key: impl FnMut(&mut Context, KeyEvent) -> Result<(), view::editing::EditError>,
    ) -> Result<(), RepeatError> {
        for _ in 0..count {
            match self {
                Self::Replace(text) => commands::replace_selections(cx.editor, text)?,
                Self::Insert(command, events) => {
                    replay::validate_target(current_ref!(cx.editor).1)?;
                    command.execute(cx);
                    if cx.editor.mode() != Mode::Insert {
                        return Err(RepeatError::EntryCommand);
                    }
                    let mut checkpoint: Option<Arc<SavePoint>> = None;
                    let mut request = None;
                    for event in events {
                        match event {
                            InsertEvent::Key(key) => {
                                replay::validate_target(current_ref!(cx.editor).1)?;
                                insert_key(cx, *key)?;
                            }
                            InsertEvent::CompletionApply {
                                trigger_offset,
                                changes,
                            } => {
                                let (view, doc) = current!(cx.editor);
                                replay::completion(
                                    doc,
                                    view.id,
                                    checkpoint.as_deref(),
                                    *trigger_offset,
                                    changes,
                                )?;
                            }
                            InsertEvent::InlineCompletionApply(applied) => {
                                let (view, doc) = current!(cx.editor);
                                replay::inline_completion(doc, view.id, applied)?;
                            }
                            InsertEvent::RequestCompletion => {
                                let (view, doc) = current!(cx.editor);
                                request = Some(doc.savepoint(view.id));
                            }
                            InsertEvent::TriggerCompletion => checkpoint = request.take(),
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_keeps_the_original_operation_for_later_repetitions() {
        let mut controller = RepeatController::default();
        assert!(controller.begin_replay().is_none());
        controller.start_insert(MappableCommand::insert_mode);
        controller.record(InsertEvent::RequestCompletion);
        let Some(RepeatPlan::Insert(_, events)) = controller.begin_replay() else {
            panic!("recorded insertion")
        };
        assert_eq!(events.len(), 1);
        controller.start_insert(MappableCommand::append_mode);
        controller.record(InsertEvent::TriggerCompletion);
        controller.replace("replacement".into());
        controller.end_replay();
        let Some(RepeatPlan::Insert(command, events)) = controller.begin_replay() else {
            panic!("original insertion")
        };
        assert_eq!(command.name(), MappableCommand::insert_mode.name());
        assert!(matches!(
            events.as_slice(),
            [InsertEvent::RequestCompletion]
        ));
        controller.end_replay();
        controller.replace("界".into());
        assert!(
            matches!(controller.begin_replay(), Some(RepeatPlan::Replace(text)) if text == "界")
        );
    }
}

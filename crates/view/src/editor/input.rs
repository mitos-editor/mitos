//! Successful user input and its shared completion/signature-help effects.

use crate::{
    editing::{self, EditError},
    events::InputApplied,
    features, Editor,
};

/// The semantic input accepted by the editor, independent of frontend keybindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Character(char),
    Backspace,
    Delete,
    /// Other insertion/deletion forms end the current completion filter.
    Other,
}

impl Editor {
    pub fn insert_char(&mut self, c: char) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::insert_char(doc, view.id, c, self.auto_pairs.as_ref())?;
        self.input_applied(InputKind::Character(c));
        Ok(())
    }

    pub fn insert_tab(&mut self, count: usize) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::insert_tab(doc, view.id, count)?;
        self.input_applied(InputKind::Other);
        Ok(())
    }

    pub fn insert_newline(&mut self) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::insert_newline(doc, view.id, self.auto_pairs.as_ref())?;
        self.input_applied(InputKind::Other);
        Ok(())
    }

    pub fn delete_char_backward(&mut self, count: usize) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::delete_char_backward(doc, view.id, count, self.auto_pairs.as_ref())?;
        self.input_applied(InputKind::Backspace);
        Ok(())
    }

    pub fn delete_char_forward(&mut self, count: usize) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::delete_char_forward(doc, view.id, count)?;
        self.input_applied(InputKind::Delete);
        Ok(())
    }

    pub fn delete_word_backward(&mut self, count: usize) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::delete_word_backward(doc, view.id, count)?;
        self.input_applied(InputKind::Other);
        Ok(())
    }

    pub fn delete_word_forward(&mut self, count: usize) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::delete_word_forward(doc, view.id, count)?;
        self.input_applied(InputKind::Delete);
        Ok(())
    }

    pub fn kill_to_line_start(&mut self) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::kill_to_line_start(doc, view.id)?;
        self.input_applied(InputKind::Other);
        Ok(())
    }

    pub fn kill_to_line_end(&mut self) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::insert::kill_to_line_end(doc, view.id)?;
        self.input_applied(InputKind::Other);
        Ok(())
    }

    fn input_applied(&mut self, kind: InputKind) {
        features::completion::input_applied(self, kind);
        if matches!(kind, InputKind::Character(_)) {
            features::signature_help::post_insert_char(self);
        }
        event::dispatch(InputApplied { editor: self, kind }).report_to(self.status_sender());
    }

    /// Cancel completion work and dismiss its presentation after unrelated commands.
    pub fn cancel_completions(&mut self) {
        features::completion::invalidate(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editor::{tests::editor, Action},
        Document,
    };
    use editor_core::Rope;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn rejected_input_and_programmatic_edits_do_not_emit_user_input() {
        let (mut editor, _store) = editor("#[|a]#\n");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let events = observed.clone();
        event::register_hook!(move |event: &mut InputApplied<'_>| {
            events.lock().unwrap().push(event.kind);
            Ok(())
        });
        let (view, doc) = current!(editor);
        let transaction =
            editor_core::Transaction::change(doc.text(), [(0, 1, Some("b".into()))].into_iter());
        editing::apply(doc, &transaction, view.id).unwrap();
        assert!(observed.lock().unwrap().is_empty());
        let readonly = Document::from(
            Rope::from("a\n"),
            None,
            editor.config.clone(),
            editor.syn_loader.load_full(),
        )
        .with_readonly_name("adapter source".into());
        editor.new_file_from_document(Action::Replace, readonly);
        assert!(editor.insert_char('x').is_err());
        assert!(editor.delete_char_forward(1).is_err());
        assert!(observed.lock().unwrap().is_empty());
        editor.new_file(Action::Replace);
        editor.enter_insert_mode();
        editor.insert_char('(').unwrap();
        editor.insert_char(')').unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            [InputKind::Character('('), InputKind::Character(')')]
        );
    }
}

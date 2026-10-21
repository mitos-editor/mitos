//! Coordinate text edits with the focused view's navigation state.

use buffer::HistoryAction;
use editor_core::transform::PastePosition;

use crate::{
    editing::{self, EditError},
    Editor,
};

impl Editor {
    /// Commit pending text and bring the focused view's jumps to that revision.
    pub fn commit_changes(&mut self) {
        let (view, doc) = current!(self);
        doc.commit_changes();
        view.sync_changes(doc);
    }

    /// Replay history with cursor restoration, then synchronize navigation state.
    pub fn change_history(&mut self, action: HistoryAction) -> bool {
        let (view, doc) = current!(self);
        let changed = doc.change_history(action, Some(view.id));
        if changed {
            view.sync_changes(doc);
        }
        changed
    }

    /// Paste literal text using the focused selection and current editing mode.
    pub fn paste_text(
        &mut self,
        text: &str,
        position: PastePosition,
        count: usize,
    ) -> Result<bool, EditError> {
        let mode = self.mode();
        let (view, doc) = current!(self);
        let changed = editing::paste(doc, view.id, &[text], position, count, mode)?;
        if changed {
            view.sync_changes(doc);
        }
        Ok(changed)
    }

    /// Replace selections and synchronize jumps after committing the edit.
    pub fn replace_selections(&mut self, replacement: &str) -> Result<(), EditError> {
        let (view, doc) = current!(self);
        editing::replace_selections(doc, view.id, replacement)?;
        view.sync_changes(doc);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::tests::editor;
    use editor_core::Selection;

    #[tokio::test]
    async fn replacement_and_history_keep_existing_jumps_at_the_current_revision() {
        let (mut editor, _store) = editor("#[|a]#bc\n");
        let (view, doc) = current!(editor);
        view.push_jump(doc, (doc.id(), Selection::point(3)));
        editor.replace_selections("long").unwrap();
        let (view, doc) = current_ref!(editor);
        assert_eq!(doc.text(), "longbc\n");
        assert_eq!(view.jumps.iter().next_back().unwrap().1.primary().anchor, 6);
        assert!(editor.change_history(HistoryAction::Undo));
        let (view, doc) = current_ref!(editor);
        assert_eq!(doc.text(), "abc\n");
        assert_eq!(view.jumps.iter().next_back().unwrap().1.primary().anchor, 3);
        assert!(editor.change_history(HistoryAction::Redo));
        let (view, doc) = current_ref!(editor);
        assert_eq!(doc.text(), "longbc\n");
        assert_eq!(view.jumps.iter().next_back().unwrap().1.primary().anchor, 6);
    }

    #[tokio::test]
    async fn literal_paste_retains_its_undo_boundary_in_insert_mode() {
        let (mut editor, _store) = editor("#[|]#\n");
        editor.enter_insert_mode();
        editor.insert_char('a').unwrap();
        assert!(editor.paste_text("界", PastePosition::Cursor, 1).unwrap());
        assert_eq!(current_ref!(editor).1.text(), "a界\n");
        assert!(editor.change_history(HistoryAction::Undo));
        assert_eq!(current_ref!(editor).1.text(), "a\n");
        assert!(editor.change_history(HistoryAction::Undo));
        assert_eq!(current_ref!(editor).1.text(), "\n");
    }
}

//! Saving and restoring where files were left, plus history, between sessions.

use crate::{
    editor::Action,
    persistence::{FilePosition, Persistence},
    DocumentId, Editor, ViewId,
};

impl Editor {
    /// Read the state enabled in `[editor.persistence]` from the state directory.
    /// Call before opening files so they reopen where they were left.
    pub fn load_persistent_state(&mut self) {
        let config = self.config().persistence.clone();
        self.persistence = Persistence::load(loader::state_dir(), &config, &mut self.registers);
    }

    /// Remember the open files' positions and write the enabled state.
    pub fn save_persistent_state(&mut self) -> std::io::Result<()> {
        let docs: Vec<DocumentId> = self.documents().map(|doc| doc.id()).collect();
        for doc in docs {
            if let Some(view) = self.position_view(doc) {
                self.remember_position(doc, view);
            }
        }
        let config = self.config().persistence.clone();
        self.persistence.save(&config, &self.registers)
    }

    /// The view whose position a document should be saved with: one showing it,
    /// preferring the focused view, or else any open view that last showed it.
    pub(crate) fn position_view(&self, doc: DocumentId) -> Option<ViewId> {
        let focus = self.tree.focus();
        let showing: Vec<ViewId> = self
            .tree
            .views()
            .filter(|(view, _)| view.doc == doc)
            .map(|(view, _)| view.id)
            .collect();
        if showing.contains(&focus) {
            return Some(focus);
        }
        showing.first().copied().or_else(|| {
            self.document(doc)?
                .view_ids()
                .find(|view| self.tree.try_get(*view).is_some())
        })
    }

    /// Record where `doc` was left in `view`, if it is a file.
    pub(crate) fn remember_position(&mut self, doc: DocumentId, view: ViewId) {
        let Some(doc) = self.document(doc) else {
            return;
        };
        let (Some(path), Some(selection)) = (doc.path(), doc.try_selection(view)) else {
            return;
        };
        let position = FilePosition::new(path.to_owned(), selection, doc.view_offset(view));
        self.persistence.record(position);
    }

    /// Put a newly opened file back where it was left, in the focused view.
    ///
    /// A file loaded in the background gets the position for the focused view too,
    /// where it is shown when the user switches to it. Callers that set an explicit
    /// selection afterwards, such as `ms a.txt b.txt:1`, replace it.
    pub(crate) fn restore_position(&mut self, doc: DocumentId, action: Action) {
        let view = self.tree.focus();
        let Some(document) = self.document(doc) else {
            return;
        };
        let Some(saved) = document
            .path()
            .and_then(|path| self.persistence.position(path))
        else {
            return;
        };
        let Some(selection) = saved.selection(document.text().len_chars()) else {
            return;
        };
        let offset = saved.view_position();
        let scrolloff = self.config().scroll.padding;
        let document = self.document_mut(doc).unwrap();
        document.set_view_offset(view, offset);
        document.set_selection(view, selection);
        // A background file is scrolled into view when it is shown.
        if !matches!(action, Action::Load)
            && let Some(view) = self.tree.try_get(view)
        {
            view.ensure_cursor_in_view(self.documents.get_mut(&doc).unwrap(), scrolloff);
        }
    }
}

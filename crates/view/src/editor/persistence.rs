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

    /// Put a newly opened file back where it was left. A file loaded without a
    /// view gets its selection when a view first shows it.
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
        if matches!(action, Action::Load) {
            self.document_mut(doc).unwrap().pending_navigation = Some(selection);
            return;
        }
        let offset = saved.view_position();
        let scrolloff = self.config().scroll.padding;
        let document = self.document_mut(doc).unwrap();
        document.set_view_offset(view, offset);
        document.set_selection(view, selection);
        if let Some(view) = self.tree.try_get(view) {
            view.ensure_cursor_in_view(self.documents.get_mut(&doc).unwrap(), scrolloff);
        }
    }
}

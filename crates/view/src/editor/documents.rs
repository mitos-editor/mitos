//! Document registration, loading, view binding, and closure.

use std::{io::stdin, path::Path};

use anyhow::Error;
use editor_core::Selection;
use event::dispatch;
use workspace::trust::TrustQuery;

use crate::{
    document::DocumentOpenError,
    editor::{Action, CloseError},
    events::{DocumentDidClose, DocumentDidOpen, DocumentFocusLost},
    Document, DocumentId, Editor, View, ViewId,
};

impl Editor {
    pub(super) fn replace_document_in_view(&mut self, current_view: ViewId, doc_id: DocumentId) {
        let scrolloff = self.config().scroll.padding;
        let view = self.tree.get_mut(current_view);

        view.bind_document(doc_id);
        let doc = doc_mut!(self, &doc_id);

        doc.ensure_view_init(view.id);
        view.sync_changes(doc);
        doc.mark_as_focused();

        view.ensure_cursor_in_view(doc, scrolloff)
    }

    pub fn switch(&mut self, id: DocumentId, action: Action) {
        use crate::tree::Layout;

        if !self.documents.contains_key(&id) {
            log::error!("cannot switch to document that does not exist (anymore)");
            return;
        }

        if !matches!(action, Action::Load) {
            self.enter_normal_mode();
        }

        let focus_lost = match action {
            Action::Replace => {
                let (view, doc) = current_ref!(self);
                // If the current view is an empty scratch buffer and is not displayed in any other views, delete it.
                // Boolean value is determined before the call to `view_mut` because the operation requires a borrow
                // of `self.tree`, which is mutably borrowed when `view_mut` is called.
                let remove_empty_scratch = !doc.is_modified() && !doc.is_virtual()
                    && !doc.is_content_readonly()
                    // If the buffer has no path and is not modified, it is an empty scratch buffer.
                    && doc.path().is_none()
                    // If the buffer we are changing to is not this buffer
                    && id != doc.id
                    // Ensure the buffer is not displayed in any other splits.
                    && !self
                        .tree
                        .traverse()
                        .any(|(_, v)| v.doc == doc.id && v.id != view.id);

                let (view, doc) = current!(self);
                let view_id = view.id;
                let old_doc_id = doc.id();

                // Append any outstanding changes to history in the old document.
                doc.commit_changes();
                view.sync_changes(doc);

                let closed = if remove_empty_scratch {
                    // Copy `doc.id` into a variable before calling `self.documents.remove`, which requires a mutable
                    // borrow, invalidating direct access to `doc.id`.
                    let id = doc.id;
                    let closed = self.documents.remove(&id).unwrap();
                    self.writes.close_document(id);

                    // Remove the scratch buffer from any jumplists
                    for (view, _) in self.tree.views_mut() {
                        view.remove_document(&id);
                    }
                    Some(closed)
                } else {
                    let jump = (view.doc, doc.selection(view.id).clone());
                    view.push_jump(doc, jump);
                    // Set last accessed doc if it is a different document
                    if doc.id != id {
                        view.add_to_history(view.doc);
                        // Set last modified doc if modified and last modified doc is different
                        if std::mem::take(&mut doc.modified_since_accessed)
                            && view.last_modified_docs[0] != Some(view.doc)
                        {
                            view.last_modified_docs = [Some(view.doc), view.last_modified_docs[0]];
                        }
                    }
                    None
                };

                self.replace_document_in_view(view_id, id);

                if let Some(doc) = closed {
                    dispatch(DocumentDidClose { editor: self, doc })
                        .report_to(self.status_sender());
                } else {
                    dispatch(DocumentFocusLost {
                        editor: self,
                        doc: old_doc_id,
                        view: view_id,
                    })
                    .report_to(self.status_sender());
                }
                return;
            }
            Action::Load => {
                let doc = doc_mut!(self, &id);
                doc.mark_as_focused();
                return;
            }
            Action::HorizontalSplit | Action::VerticalSplit => {
                let focus_lost = self
                    .tree
                    .try_get(self.tree.focus())
                    .map(|view| (view.doc, view.id));
                // Copy only persistent selection/viewport state. Feature sessions and
                // transient overlays continue to belong to the originating view.
                let source = self
                    .tree
                    .try_get(self.tree.focus())
                    .filter(|view| view.doc == id);
                let state = source.map(|view| {
                    let doc = &self.documents[&id];
                    (doc.selection(view.id).clone(), doc.view_offset(view.id))
                });
                // copy the current view, unless there is no view yet
                let view = source.cloned().unwrap_or_else(|| {
                    View::new(id, self.config().gutters.clone(), self.redraw().clone())
                });
                let view_id = self.tree.split(
                    view,
                    match action {
                        Action::HorizontalSplit => Layout::Horizontal,
                        Action::VerticalSplit => Layout::Vertical,
                        _ => unreachable!(),
                    },
                );
                let doc = doc_mut!(self, &id);
                if let Some((selection, offset)) = state {
                    // Install the viewport before selection observers inspect the new split.
                    doc.set_view_offset(view_id, offset);
                    doc.set_selection(view_id, selection);
                }
                doc.ensure_view_init(view_id);
                doc.mark_as_focused();
                focus_lost
            }
        };

        self._refresh();
        if let Some((focus_lost, view)) = focus_lost {
            dispatch(DocumentFocusLost {
                editor: self,
                doc: focus_lost,
                view,
            })
            .report_to(self.status_sender());
        }
    }

    /// Register a document and bind its editor-owned services before publishing it.
    fn new_document(&mut self, doc: Document) -> DocumentId {
        let id = self.documents.insert(doc);
        let doc = self.documents.get_mut(&id).unwrap();
        self.handlers.attach_document(doc);
        doc.breakpoint_source = doc.path().map(|path| self.debugger.breakpoint_source(path));
        doc.breakpoint_saved_text = doc.path().map(|_| doc.text().clone());
        if let Some(path) = doc.path()
            && let Some(old) = self.debugger.closed_breakpoint_sources.remove(path)
            && let Some(source) = &doc.breakpoint_source
        {
            let changes = text_diff::compare_ropes(&old, doc.text());
            crate::debugger::map_breakpoint_positions(source, changes.changes(), &old, doc.text());
        }
        doc.initialize_syntax(self.syn_loader.load_full());
        doc.detect_spelling_languages();
        self.refresh_vcs_watches();
        self.writes.open_document(id);
        id
    }

    pub(crate) fn new_file_from_document(&mut self, action: Action, doc: Document) -> DocumentId {
        let id = self.new_document(doc);
        self.switch(id, action);
        self.refresh_spelling(id);
        id
    }

    pub fn new_file(&mut self, action: Action) -> DocumentId {
        self.new_file_from_document(
            action,
            Document::default(self.config.clone(), self.syn_loader.load_full()),
        )
    }

    /// Create the initial scratch document shown when Mitos starts without a file.
    pub fn new_file_welcome(&mut self) -> DocumentId {
        self.new_file_from_document(
            Action::VerticalSplit,
            Document::default(self.config.clone(), self.syn_loader.load_full()).with_welcome(),
        )
    }

    pub fn new_file_from_stdin(&mut self, action: Action) -> Result<DocumentId, Error> {
        let (stdin, encoding, has_bom) = text_io::read_to_string(&mut stdin(), None)?;
        let doc = Document::from(
            editor_core::Rope::default(),
            Some((encoding, has_bom)),
            self.config.clone(),
            self.syn_loader.load_full(),
        );
        let doc_id = self.new_file_from_document(action, doc);
        let doc = doc_mut!(self, &doc_id);
        let view = view_mut!(self);
        doc.ensure_view_init(view.id);
        let transaction =
            editor_core::Transaction::insert(doc.text(), doc.selection(view.id), stdin.into())
                .with_selection(Selection::point(0));
        crate::editing::apply(doc, &transaction, view.id)?;
        doc.commit_changes();
        view.sync_changes(doc);
        Ok(doc_id)
    }

    /// Open a fresh scratch copy of a text file, retaining its language and encoding.
    /// The source is never registered with language servers or filesystem watchers.
    pub fn open_scratch(&mut self, path: &Path, action: Action) -> anyhow::Result<DocumentId> {
        let doc = Document::open_scratch(path, self.config.clone(), self.syn_loader.load_full())?;
        let id = self.new_file_from_document(action, doc);
        dispatch(DocumentDidOpen {
            editor: self,
            doc: id,
        })
        .report_to(self.status_sender());
        Ok(id)
    }

    pub fn document_id_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_by_path(path).map(|doc| doc.id)
    }

    // ??? possible use for integration tests
    pub fn open(&mut self, path: &Path, action: Action) -> Result<DocumentId, DocumentOpenError> {
        let path = stdx::path::canonicalize(path);
        let id = self.document_id_by_path(&path);

        let id = if let Some(id) = id {
            id
        } else {
            self.ensure_path_not_writing(&path)?;
            let mut doc = Document::open(
                &path,
                None,
                true,
                self.config.clone(),
                self.syn_loader.load_full(),
            )?;

            let diagnostics =
                Editor::doc_diagnostics(&self.language_servers, &self.diagnostics, &doc);
            doc.replace_language_server_diagnostics(diagnostics);

            let trust_full = self
                .workspace_trust
                .query(doc.workspace_root(), TrustQuery::Git)
                .is_trusted();
            let diff_base = if doc.is_binary() {
                None
            } else {
                self.diff_providers.get_diff_base(&path, trust_full)
            };
            doc.set_version_control_head(
                self.diff_providers.get_current_head_name(&path, trust_full),
            );

            let id = self.new_document(doc);
            if let Some(diff_base) = diff_base {
                self.document_mut(id).unwrap().set_diff_base(diff_base);
            }
            self.launch_language_servers(id);

            event::dispatch(DocumentDidOpen {
                editor: self,
                doc: id,
            })
            .report_to(self.status_sender());

            id
        };

        self.switch(id, action);

        Ok(id)
    }

    pub fn close(&mut self, id: ViewId) {
        // Remove selections for the closed view on all documents.
        for doc in self.documents_mut() {
            doc.remove_view(id);
        }
        self.tree.remove(id);
        self._refresh();
    }

    pub fn close_document(&mut self, doc_id: DocumentId, force: bool) -> Result<(), CloseError> {
        let doc = match self.documents.get(&doc_id) {
            Some(doc) => doc,
            None => return Err(CloseError::DoesNotExist),
        };
        if !force && doc.is_modified() {
            return Err(CloseError::BufferModified(doc.display_name().into_owned()));
        }

        // This will also disallow any follow-up writes
        self.writes.close_document(doc_id);

        enum Action {
            Close(ViewId),
            ReplaceDoc(ViewId, DocumentId),
        }

        let actions: Vec<Action> = self
            .tree
            .views_mut()
            .filter_map(|(view, _focus)| {
                view.remove_document(&doc_id);

                if view.doc == doc_id {
                    // something was previously open in the view, switch to previous doc
                    if let Some(prev_doc) = view.docs_access_history.pop() {
                        Some(Action::ReplaceDoc(view.id, prev_doc))
                    } else {
                        // only the document that is being closed was in the view, close it
                        Some(Action::Close(view.id))
                    }
                } else {
                    None
                }
            })
            .collect();

        for action in actions {
            match action {
                Action::Close(view_id) => {
                    self.close(view_id);
                }
                Action::ReplaceDoc(view_id, doc_id) => {
                    self.replace_document_in_view(view_id, doc_id);
                }
            }
        }

        let doc = self.documents.remove(&doc_id).unwrap();
        self.refresh_vcs_watches();

        // If the document we removed was visible in all views, we will have no more views. We don't
        // want to close the editor just for a simple buffer close, so we need to create a new view
        // containing either an existing document, or a brand new document.
        if self.tree.views().next().is_none() {
            let first_document = self.documents.keys().next().copied();
            let doc_id = first_document.unwrap_or_else(|| {
                self.new_document(Document::default(
                    self.config.clone(),
                    self.syn_loader.load_full(),
                ))
            });
            let view = View::new(doc_id, self.config().gutters.clone(), self.redraw().clone());
            let view_id = self.tree.insert(view);
            let doc = doc_mut!(self, &doc_id);
            doc.ensure_view_init(view_id);
            doc.mark_as_focused();
        }

        self._refresh();

        crate::handlers::lsp::close_document(&doc);
        event::dispatch(DocumentDidClose { editor: self, doc }).report_to(self.status_sender());

        Ok(())
    }

    #[inline]
    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    #[inline]
    pub fn document_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    /// Finish pending syntax initialization for an explicit syntax-dependent command.
    /// Normal file loading publishes syntax through the editor's completion sender.
    pub fn ensure_syntax(&mut self, id: DocumentId) {
        if self
            .document_mut(id)
            .is_some_and(Document::finish_syntax_initialization)
        {
            self.refresh_spelling(id);
        }
    }

    #[inline]
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    #[inline]
    pub fn documents_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.documents.values_mut()
    }

    pub fn document_by_path<P: AsRef<Path>>(&self, path: P) -> Option<&Document> {
        self.documents()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }

    pub fn document_by_path_mut<P: AsRef<Path>>(&mut self, path: P) -> Option<&mut Document> {
        self.documents_mut()
            .find(|doc| doc.path().is_some_and(|p| p == path.as_ref()))
    }
}

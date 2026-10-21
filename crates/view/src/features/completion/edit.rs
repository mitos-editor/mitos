//! Completion text edits and previews, independent of menu presentation.

use std::sync::Arc;

use editor_core::{Change, Rope, Selection, Transaction};
use lsp_client::{lsp, util, OffsetEncoding};
use snippets::{ActiveSnippet, RenderedSnippet, Snippet, SnippetRenderCtx};

use super::{
    additional_edits_transaction, trigger_auto_completion, AcceptedCompletion, CompletionItem,
    CompletionSession, TextCompletion,
};
use crate::{document::SavePoint, editing::EditError, Document, DocumentId, Editor, ViewId};

/// Accepted edit information a frontend can record for insert-mode repetition.
#[derive(Debug, Clone)]
pub struct CompletionApply {
    pub(crate) trigger_offset: usize,
    pub(crate) cursors: Vec<usize>,
    pub(crate) changes: Vec<Change>,
    pub(crate) selection: Selection,
    pub(crate) snippet: Option<RenderedSnippet>,
}

impl CompletionApply {
    fn new(
        trigger_offset: usize,
        text: &Rope,
        selection: &Selection,
        primary: &Transaction,
        snippet: Option<&RenderedSnippet>,
        additional: Option<&Transaction>,
    ) -> Self {
        let mut after_primary = text.clone();
        primary.apply(&mut after_primary);
        // Postfix completions can delete their receiver in an additional edit.
        // Include deletions adjoining the primary replacement in the local edit;
        // file-wide additional edits such as imports are not insertion repetition.
        let ranges: Vec<_> = primary
            .changes_iter()
            .map(|(from, to, _)| {
                (
                    primary.changes().map_pos(from, editor_core::Assoc::Before),
                    primary.changes().map_pos(to, editor_core::Assoc::After),
                )
            })
            .collect();
        let local = Transaction::change(
            &after_primary,
            additional
                .into_iter()
                .flat_map(|edit| edit.changes_iter())
                .filter_map(|(from, to, _)| {
                    (from < to
                        && ranges
                            .iter()
                            .any(|&(start, end)| to == start || from == end))
                    .then_some((from, to, None))
                }),
        );
        let snippet = snippet
            .cloned()
            .and_then(ActiveSnippet::new)
            .and_then(|mut snippet| {
                (!snippet.map(local.changes())).then(|| snippet.into_rendered())
            });
        let resulting_selection = primary
            .selection()
            .cloned()
            .unwrap_or_else(|| selection.clone().map(primary.changes()))
            .map(local.changes());
        let transaction = primary.clone().compose(local);
        Self {
            trigger_offset,
            cursors: selection
                .iter()
                .map(|range| range.cursor(text.slice(..)))
                .collect(),
            changes: transaction.changes_iter().collect(),
            selection: resulting_selection,
            snippet,
        }
    }
}

pub(super) enum CompletionEdit {
    Shown,
    Preview {
        savepoint: Arc<SavePoint>,
        document: DocumentId,
        view: ViewId,
    },
    Applied(CompletionApply),
}

struct PreparedCompletion {
    savepoint: Arc<SavePoint>,
    transaction: Transaction,
    snippet: Option<RenderedSnippet>,
    additional: Option<Transaction>,
}

impl PreparedCompletion {
    // Check the whole edit against its saved text before restoring previews or
    // committing typing. Additional edits use the primary edit's resulting text.
    fn validate(&self, doc: &Document) -> Result<(), EditError> {
        if !self.transaction.changes().is_empty()
            || self
                .additional
                .as_ref()
                .is_some_and(|edit| !edit.changes().is_empty())
        {
            doc.validate_editable()?;
        }
        let (mut text, _) = self.savepoint.saved_state(doc)?;
        for edit in std::iter::once(&self.transaction).chain(self.additional.iter()) {
            let expected = text.len_chars();
            if edit.changes().len_before() != expected {
                return Err(EditError::InvalidLength {
                    expected,
                    actual: edit.changes().len_before(),
                });
            }
            edit.apply(&mut text);
        }
        Ok(())
    }
}

impl Editor {
    /// Mark a valid session as displayed after the frontend filters its items.
    /// A stale or already displayed session cannot replace preview or repeat state.
    pub fn begin_completion(&mut self, session: &CompletionSession) -> bool {
        if !session.is_current(self) || self.handlers.completions.edit.is_some() {
            return false;
        }
        self.handlers.completions.edit = Some(CompletionEdit::Shown);
        true
    }

    fn prepare_completion(
        &self,
        session: &CompletionSession,
        item: &CompletionItem,
    ) -> Option<PreparedCompletion> {
        if matches!(
            self.handlers.completions.edit,
            Some(CompletionEdit::Applied(_))
        ) || !session.is_current(self)
            || !session.supports(self, item.provider())
        {
            return None;
        }
        let context = self
            .handlers
            .completions
            .active_completions
            .get(&item.provider())?;
        let (transaction, snippet, additional) = match item {
            CompletionItem::Lsp(item) => {
                let server = self.language_servers.get_by_id(item.provider)?;
                let encoding = server.offset_encoding();
                let doc = self.document(session.trigger.doc)?;
                let (text, selection) = context.savepoint.saved_state(doc).ok()?;
                let (transaction, snippet) = lsp_item_to_transaction(
                    &text,
                    &selection,
                    &item.item,
                    encoding,
                    session.trigger.pos,
                    self.config().completion.replace,
                    &mut doc.snippet_ctx(),
                )?;
                let additional = item
                    .item
                    .additional_text_edits
                    .as_ref()
                    .filter(|edits| !edits.is_empty())
                    .and_then(|edits| {
                        let mut current = text.clone();
                        transaction.apply(&mut current);
                        let additional = additional_edits_transaction(
                            &text,
                            &current,
                            transaction.changes(),
                            edits.clone(),
                            encoding,
                        );
                        if additional.is_none() {
                            log::error!("Completion contains conflicting additional edits");
                        }
                        additional
                    });
                (transaction, snippet, additional)
            }
            CompletionItem::Path(TextCompletion { transaction, .. })
            | CompletionItem::Word(TextCompletion { transaction, .. }) => {
                (transaction.clone(), None, None)
            }
        };
        Some(PreparedCompletion {
            savepoint: context.savepoint.clone(),
            transaction,
            snippet,
            additional,
        })
    }

    /// Show an unsynchronized edit. Restore it before applying real changes, so
    /// incremental language-server updates always start from synchronized text.
    /// Returns `Ok(false)` for an expired provider or session, or the edit's rejection.
    pub fn preview_completion(
        &mut self,
        session: &CompletionSession,
        item: &CompletionItem,
    ) -> Result<bool, EditError> {
        let Some(prepared) = self.prepare_completion(session, item) else {
            return Ok(false);
        };
        prepared.validate(
            self.document(session.trigger.doc)
                .expect("validated session"),
        )?;
        let previewing = super::has_preview(self);
        let (view, doc) = current!(self);
        if !previewing {
            self.handlers.completions.edit = Some(CompletionEdit::Preview {
                savepoint: doc.savepoint(view.id),
                document: doc.id(),
                view: view.id,
            });
        }
        doc.restore(view.id, &prepared.savepoint, false)?;
        crate::editing::apply_temporary(doc, &prepared.transaction, view.id)?;
        if let Some(additional) = prepared.additional {
            crate::editing::apply_temporary(doc, &additional, view.id)?;
        }
        Ok(true)
    }

    /// Insert the selected text and snippet, then apply or resolve additional
    /// edits. The primary edit and ready additional edits share an undo group.
    pub fn accept_completion(
        &mut self,
        session: &CompletionSession,
        item: &CompletionItem,
    ) -> Result<bool, EditError> {
        let Some(prepared) = self.prepare_completion(session, item) else {
            return Ok(false);
        };
        prepared.validate(
            self.document(session.trigger.doc)
                .expect("validated session"),
        )?;
        let PreparedCompletion {
            savepoint,
            transaction,
            snippet,
            additional,
        } = prepared;
        let trigger_offset = session.trigger.pos;
        self.restore_completion_preview()?;
        let (view, doc) = current!(self);
        doc.restore(view.id, &savepoint, true)?;
        doc.commit_changes();
        view.sync_changes(doc);
        let before = doc.snapshot();
        let applied = CompletionApply::new(
            trigger_offset,
            doc.text(),
            doc.selection(view.id),
            &transaction,
            snippet.as_ref(),
            additional.as_ref(),
        );
        crate::editing::apply(doc, &transaction, view.id)?;
        if let Some(snippet) = snippet {
            doc.install_snippet(view.id, snippet);
        }
        self.handlers.completions.edit = Some(CompletionEdit::Applied(applied));

        if let Some(additional) = additional {
            crate::editing::apply(doc, &additional, view.id)?;
        }
        let document = doc.id();
        if let CompletionItem::Lsp(item) = item
            && !item.resolved
        {
            self.resolve_accepted_completion(AcceptedCompletion {
                document,
                item: item.clone(),
                before,
                savepoint,
            });
        }
        // An accepted item can itself end in a completion trigger, such as `::`.
        trigger_auto_completion(self, true);
        Ok(true)
    }

    pub(super) fn restore_completion_preview(&mut self) -> Result<(), EditError> {
        let Some(CompletionEdit::Preview {
            savepoint,
            document,
            view,
        }) = &self.handlers.completions.edit
        else {
            return Ok(());
        };
        let (savepoint, document, view) = (savepoint.clone(), *document, *view);
        if self
            .tree
            .try_get(view)
            .is_some_and(|view| view.doc == document)
            && let Some(doc) = self.documents.get_mut(&document)
        {
            doc.restore(view, &savepoint, false)?;
        }
        self.handlers.completions.edit = None;
        Ok(())
    }

    /// Dismiss a menu, restoring an unaccepted preview and returning only accepted
    /// edit metadata. A newly scheduled completion trigger remains intact.
    pub fn finish_completion(&mut self) -> Result<Option<CompletionApply>, EditError> {
        self.restore_completion_preview()?;
        self.handlers.completions.dismiss();
        Ok(match self.handlers.completions.edit.take() {
            Some(CompletionEdit::Applied(applied)) => Some(applied),
            _ => None,
        })
    }
}

fn lsp_item_to_transaction(
    rope: &Rope,
    selection: &Selection,
    item: &lsp::CompletionItem,
    offset_encoding: OffsetEncoding,
    trigger_offset: usize,
    replace_mode: bool,
    snippet_ctx: &mut SnippetRenderCtx,
) -> Option<(Transaction, Option<RenderedSnippet>)> {
    let text = rope.slice(..);
    let primary_cursor = selection.primary().cursor(text);

    let (edit_offset, new_text) = if let Some(edit) = &item.text_edit {
        let edit = match edit {
            lsp::CompletionTextEdit::Edit(edit) => edit.clone(),
            lsp::CompletionTextEdit::InsertAndReplace(item) => {
                let range = if replace_mode {
                    item.replace
                } else {
                    item.insert
                };
                lsp::TextEdit::new(range, item.new_text.clone())
            }
        };

        let range = util::lsp_range_to_range(rope, edit.range, offset_encoding)?;

        let start_offset = range.anchor as i128 - primary_cursor as i128;
        let end_offset = range.head as i128 - primary_cursor as i128;

        (Some((start_offset, end_offset)), edit.new_text)
    } else {
        let new_text = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        // check that we are still at the correct savepoint
        // we can still generate a transaction regardless but if the
        // document changed (and not just the selection) then we will
        // likely delete the wrong text (same if we applied an edit sent by the LS)
        debug_assert!(primary_cursor == trigger_offset);
        (None, new_text)
    };

    if matches!(item.kind, Some(lsp::CompletionItemKind::SNIPPET))
        || matches!(
            item.insert_text_format,
            Some(lsp::InsertTextFormat::SNIPPET)
        )
    {
        let Ok(snippet) = Snippet::parse(&new_text) else {
            log::error!("Failed to parse snippet: {new_text:?}",);
            return None;
        };
        let (transaction, snippet) =
            crate::language_servers::edits::generate_transaction_from_snippet(
                rope,
                selection,
                edit_offset,
                replace_mode,
                snippet,
                snippet_ctx,
            );
        Some((transaction, Some(snippet)))
    } else {
        let transaction = crate::language_servers::edits::generate_transaction_from_completion_edit(
            rope,
            selection,
            edit_offset,
            replace_mode,
            new_text,
        );
        Some((transaction, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;
    use arc_swap::ArcSwap;

    #[tokio::test]
    async fn snippet_replay_preserves_multiple_cursors_after_a_unicode_prefix_shift() {
        use editor_core::Range;
        let (mut doc, view) = crate::editing::tests::document_and_view("#[|]#\n\n");
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::point(0), Range::point(1)].into(), 1),
        );
        let source = doc.selection(view.id).clone();
        let (primary, _, snippet) = Snippet::parse("${1:one}$0").unwrap().render(
            doc.text(),
            &source,
            |range| {
                let cursor = range.cursor(doc.text().slice(..));
                (cursor, cursor)
            },
            &mut doc.snippet_ctx(),
        );
        let primary = primary
            .with_selection(snippet.first_selection(editor_core::movement::Direction::Forward, 1));
        let applied = CompletionApply::new(1, doc.text(), &source, &primary, Some(&snippet), None);
        let prefix = Transaction::change(doc.text(), [(0, 0, Some("界\n".into()))].into_iter());
        crate::editing::apply(&mut doc, &prefix, view.id).unwrap();
        crate::editing::replay::completion(&mut doc, view.id, None, &applied).unwrap();
        assert_eq!(doc.text(), "界\none\none\n");
        assert_eq!(doc.selection(view.id).primary_index(), 1);
        crate::editing::insert::insert_char(&mut doc, view.id, 'é', None).unwrap();
        assert_eq!(doc.text(), "界\né\né\n");
    }

    #[tokio::test]
    async fn snippet_replay_removes_the_receiver_without_repeating_a_combined_import_edit() {
        let (mut doc, view) = crate::editing::tests::document_and_view("x.f#[|]#\n");
        let source = doc.selection(view.id).clone();
        let (primary, _, snippet) = Snippet::parse("call(${1:arg})$0").unwrap().render(
            doc.text(),
            &source,
            |_| (2, 3),
            &mut doc.snippet_ctx(),
        );
        let primary = primary
            .with_selection(snippet.first_selection(editor_core::movement::Direction::Forward, 0));
        let mut text = doc.text().clone();
        primary.apply(&mut text);
        let additional =
            Transaction::change(&text, [(0, 2, Some("use fruit;\n".into()))].into_iter());
        let applied = CompletionApply::new(
            3,
            doc.text(),
            &source,
            &primary,
            Some(&snippet),
            Some(&additional),
        );
        crate::editing::replay::completion(&mut doc, view.id, None, &applied).unwrap();
        assert_eq!(doc.text(), "call(arg)\n");
        assert!(doc.active_snippet().is_some());
        crate::editing::insert::insert_char(&mut doc, view.id, '界', None).unwrap();
        assert_eq!(doc.text(), "call(界)\n");
    }

    #[tokio::test]
    async fn insert_and_replace_ranges_honor_utf16_positions_and_replace_mode() {
        let mut doc = Document::from(
            Rope::from("😀 foobar\n"),
            None,
            Arc::new(ArcSwap::from_pointee(editor_config::Config::default())),
            Arc::new(language::syntax::Loader::default()),
        );
        let view = crate::View::new(doc.id(), Default::default(), event::Redraw::default());
        doc.ensure_view_init(view.id);
        doc.set_selection(view.id, Selection::point(4));
        let item = lsp::CompletionItem {
            label: "food".into(),
            text_edit: Some(lsp::CompletionTextEdit::InsertAndReplace(
                lsp::InsertReplaceEdit {
                    new_text: "food".into(),
                    insert: lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 5)),
                    replace: lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 9)),
                },
            )),
            ..Default::default()
        };
        for (replace_mode, expected) in [(false, "😀 foodobar\n"), (true, "😀 food\n")] {
            let (transaction, snippet) = lsp_item_to_transaction(
                doc.text(),
                doc.selection(view.id),
                &item,
                OffsetEncoding::Utf16,
                4,
                replace_mode,
                &mut doc.snippet_ctx(),
            )
            .unwrap();
            let mut text = doc.text().clone();
            transaction.apply(&mut text);
            assert_eq!(text, expected);
            assert!(snippet.is_none());
        }
    }
}

#[cfg(test)]
mod rejection_tests {
    use super::super::{
        request::{Trigger, TriggerKind},
        CompletionProvider, ResponseContext,
    };
    use super::*;
    use crate::{
        document::Mode,
        editing,
        editor::{tests::editor, Action},
    };

    #[tokio::test]
    async fn rejected_completions_preserve_typing_selection_and_acceptance_state() {
        for readonly in [false, true] {
            let (mut editor, _store) = editor("al#[|\n]#");
            if readonly {
                let doc = Document::from(
                    Rope::from("al\n"),
                    None,
                    editor.config.clone(),
                    editor.syn_loader.load_full(),
                )
                .with_readonly_name("adapter source".into());
                editor.new_file_from_document(Action::Replace, doc);
            }
            editor.set_mode(Mode::Insert);
            let (view, doc) = current!(editor);
            doc.set_selection(view.id, Selection::point(2));
            let trigger = Trigger {
                pos: 2,
                doc: doc.id(),
                view: view.id,
                kind: TriggerKind::Manual,
            };
            let savepoint = doc.savepoint(view.id);
            let transaction = if readonly {
                Transaction::change(doc.text(), [(0, 2, Some("alpha".into()))].into_iter())
            } else {
                let transaction =
                    Transaction::change(doc.text(), [(2, 2, Some("p".into()))].into_iter());
                editing::apply(doc, &transaction, view.id).unwrap();
                Transaction::new(&Rope::from("incompatible text"))
            };
            let before = (
                doc.text().clone(),
                doc.selection(view.id).clone(),
                doc.changes().clone(),
                doc.version(),
            );
            let epoch = editor
                .handlers
                .completions
                .shared
                .epoch
                .load(std::sync::atomic::Ordering::Relaxed);
            let session = CompletionSession::new(&mut editor, trigger, epoch);
            editor.handlers.completions.active_completions.insert(
                CompletionProvider::Word,
                ResponseContext {
                    is_incomplete: false,
                    priority: 0,
                    savepoint,
                },
            );
            let item = CompletionItem::Word(TextCompletion {
                transaction,
                label: "alpha".into(),
                kind: "word".into(),
                documentation: None,
            });
            assert!(editor.begin_completion(&session));
            assert!(editor.preview_completion(&session, &item).is_err());
            assert!(editor.accept_completion(&session, &item).is_err());
            assert!(editor.finish_completion().unwrap().is_none());
            let (view, doc) = current!(editor);
            assert_eq!(doc.text(), &before.0);
            assert_eq!(doc.selection(view.id), &before.1);
            assert_eq!(doc.changes(), &before.2);
            assert_eq!(doc.version(), before.3);
            assert!(doc.active_snippet().is_none());
        }
    }
}

//! Completion text edits and previews, independent of menu presentation.

use std::sync::Arc;

use editor_core::{Change, Rope, Selection, Transaction};
use lsp_client::{lsp, util, OffsetEncoding};
use snippets::{ActiveSnippet, RenderedSnippet, Snippet, SnippetRenderCtx};

use super::{
    additional_edits_transaction, trigger_auto_completion, AcceptedCompletion, CompletionItem,
    CompletionSession, TextCompletion,
};
use crate::{document::SavePoint, DocumentId, Editor, ViewId};

/// Accepted edit information a frontend can record for insert-mode repetition.
pub struct CompletionApply {
    pub trigger_offset: usize,
    pub changes: Vec<Change>,
    pub placeholder: bool,
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
    encoding: Option<OffsetEncoding>,
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
        let (transaction, snippet, encoding) = match item {
            CompletionItem::Lsp(item) => {
                let server = self.language_servers.get_by_id(item.provider)?;
                let encoding = server.offset_encoding();
                let doc = self.document(session.trigger.doc)?;
                let (text, selection) = context.savepoint.saved_state(doc.text());
                let (transaction, snippet) = lsp_item_to_transaction(
                    &text,
                    &selection,
                    &item.item,
                    encoding,
                    session.trigger.pos,
                    self.config().completion.replace,
                    &mut doc.snippet_ctx(),
                )?;
                (transaction, snippet, Some(encoding))
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
            encoding,
        })
    }

    /// Show an unsynchronized edit. Restore it before applying real changes, so
    /// incremental language-server updates always start from synchronized text.
    /// Returns false when the item's provider or completion session has expired.
    pub fn preview_completion(
        &mut self,
        session: &CompletionSession,
        item: &CompletionItem,
    ) -> bool {
        let Some(prepared) = self.prepare_completion(session, item) else {
            return false;
        };
        let previewing = super::has_preview(self);
        let (view, doc) = current!(self);
        if !previewing {
            self.handlers.completions.edit = Some(CompletionEdit::Preview {
                savepoint: doc.savepoint(view),
                document: doc.id(),
                view: view.id,
            });
        }
        doc.restore(view, &prepared.savepoint, false);
        crate::editing::apply_temporary(doc, &prepared.transaction, view.id);
        true
    }

    /// Insert the selected text and snippet, then apply or resolve additional
    /// edits. The primary edit and ready additional edits share an undo group.
    pub fn accept_completion(
        &mut self,
        session: &CompletionSession,
        item: &CompletionItem,
    ) -> bool {
        let Some(PreparedCompletion {
            savepoint,
            transaction,
            snippet,
            encoding,
        }) = self.prepare_completion(session, item)
        else {
            return false;
        };
        let trigger_offset = session.trigger.pos;
        self.restore_completion_preview();
        let (view, doc) = current!(self);
        doc.restore(view, &savepoint, true);
        crate::editing::commit(doc, view);
        let before = doc.snapshot();
        crate::editing::apply(doc, &transaction, view.id);
        let placeholder = snippet.is_some();
        if let Some(snippet) = snippet {
            doc.active_snippet = match doc.active_snippet.take() {
                Some(active) => active.insert_subsnippet(snippet),
                None => ActiveSnippet::new(snippet),
            };
        }
        self.handlers.completions.edit = Some(CompletionEdit::Applied(CompletionApply {
            trigger_offset,
            changes: transaction
                .changes_iter()
                .filter(|(start, end, _)| (*start..=*end).contains(&trigger_offset))
                .collect(),
            placeholder,
        }));

        if let CompletionItem::Lsp(item) = item
            && let Some(edits) = item
                .item
                .additional_text_edits
                .as_ref()
                .filter(|edits| !edits.is_empty())
        {
            if let Some(additional) = additional_edits_transaction(
                before.text(),
                doc.text(),
                transaction.changes(),
                edits.clone(),
                encoding.unwrap(),
            ) {
                crate::editing::apply(doc, &additional, view.id);
            } else {
                log::error!("Completion contains conflicting additional edits");
            }
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
        true
    }

    pub(super) fn restore_completion_preview(&mut self) {
        let Some(CompletionEdit::Preview {
            savepoint,
            document,
            view,
        }) = &self.handlers.completions.edit
        else {
            return;
        };
        let (savepoint, document, view) = (savepoint.clone(), *document, *view);
        self.handlers.completions.edit = None;
        if self
            .tree
            .try_get(view)
            .is_some_and(|view| view.doc == document)
            && let Some(doc) = self.documents.get_mut(&document)
        {
            doc.restore(self.tree.get_mut(view), &savepoint, false);
        }
    }

    /// Dismiss a menu, restoring an unaccepted preview and returning only accepted
    /// edit metadata. A newly scheduled completion trigger remains intact.
    pub fn finish_completion(&mut self) -> Option<CompletionApply> {
        self.restore_completion_preview();
        self.handlers.completions.dismiss();
        match self.handlers.completions.edit.take() {
            Some(CompletionEdit::Applied(applied)) => Some(applied),
            _ => None,
        }
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
    async fn insert_and_replace_ranges_honor_utf16_positions_and_replace_mode() {
        let mut doc = Document::from(
            Rope::from("😀 foobar\n"),
            None,
            Arc::new(ArcSwap::from_pointee(editor_config::Config::default())),
            Arc::new(ArcSwap::from_pointee(language::syntax::Loader::default())),
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

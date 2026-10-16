//! The fixed code-actions, formatting, and write pipeline for one accepted save.

use std::{collections::VecDeque, sync::Arc};

use editor_core::{syntax, Transaction, Uri};
use futures_util::{future::BoxFuture, FutureExt};
use lsp_client::{
    lsp::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionTriggerKind, WorkspaceEdit},
    LanguageServerId, OffsetEncoding,
};

use super::PreparedSave;
use crate::{
    action::{code_actions_for_range, LspActionContext},
    document::FormatterError,
    Editor,
};

/// The result of advancing an accepted save on the editor mutation path.
#[must_use = "pending save tasks must be driven to completion"]
pub enum SaveProgress {
    /// Poll this stage and apply its update before advancing further.
    Pending(SaveTask),
    /// The write is queued; the normal document-saved event reports its result.
    Queued,
    /// The document closed before its write could be submitted.
    DocumentClosed,
}

/// One asynchronous stage of a save. Dropping it abandons the pending save.
/// Frontends must retain these tasks until completion, including during shutdown.
#[must_use = "save tasks must be driven to completion"]
pub struct SaveTask(BoxFuture<'static, Result<SaveUpdate, SaveError>>);

/// A failed save stage. Formatting failures remain nonfatal and still write;
/// requesting code actions or submitting the write can fail the operation.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("{0}")]
    CodeActions(#[source] lsp_client::Error),
    #[error("{0}")]
    Write(#[source] anyhow::Error),
}

impl SaveTask {
    pub async fn run(self) -> Result<SaveUpdate, SaveError> {
        self.0.await
    }
}

/// A completed stage. Apply it to the originating editor to validate its snapshot
/// and continue the operation against the latest document state.
#[must_use = "save updates must be applied to the owning editor"]
pub struct SaveUpdate {
    operation: SaveOperation,
    stage: CompletedStage,
}

struct SaveOperation {
    request: PreparedSave,
    kinds: VecDeque<CodeActionKind>,
}

enum CompletedStage {
    Write,
    Actions {
        context: LspActionContext,
        server: LanguageServerId,
        kind: CodeActionKind,
        actions: Option<Vec<CodeActionOrCommand>>,
    },
    Edits {
        context: LspActionContext,
        server: LanguageServerId,
        offset_encoding: OffsetEncoding,
        edits: Vec<WorkspaceEdit>,
    },
    Formatting {
        snapshot: FormatSnapshot,
        result: Result<Transaction, FormatterError>,
    },
}

struct FormatSnapshot {
    version: i32,
    uri: Option<Uri>,
    language: Option<String>,
    loader: Arc<syntax::Loader>,
}

impl FormatSnapshot {
    fn is_current(&self, editor: &Editor, doc: &crate::Document) -> bool {
        doc.version() == self.version
            && doc.uri() == self.uri
            && doc
                .language_config()
                .map(|config| config.language_id.as_str())
                == self.language.as_deref()
            && Arc::ptr_eq(&self.loader, &editor.syn_loader.load_full())
    }
}

/// Submit a prepared document save. Plain writes are queued immediately; saves
/// with pre-save work return tasks for the frontend to drive without owning policy.
pub fn submit(editor: &mut Editor, request: PreparedSave) -> Result<SaveProgress, SaveError> {
    if !request.auto_format && !request.code_actions {
        return SaveOperation {
            request,
            kinds: VecDeque::new(),
        }
        .write(editor);
    }
    let kinds = request
        .code_actions
        .then(|| {
            editor
                .document(request.doc_id)?
                .language_config()?
                .code_actions_on_save
                .clone()
        })
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(CodeActionKind::from)
        .collect();
    SaveOperation { request, kinds }.advance(editor)
}

impl SaveOperation {
    fn advance(mut self, editor: &mut Editor) -> Result<SaveProgress, SaveError> {
        let Some(doc) = editor.document(self.request.doc_id) else {
            return Ok(SaveProgress::DocumentClosed);
        };
        while let Some(kind) = self.kinds.pop_front() {
            let full_range = editor_core::Range::new(0, doc.text().len_chars());
            let Some((request, server)) = code_actions_for_range(
                doc,
                full_range,
                Some(vec![kind.clone()]),
                CodeActionTriggerKind::INVOKED,
            )
            .into_iter()
            .next() else {
                continue;
            };
            let context = LspActionContext::new(doc);
            return Ok(SaveProgress::Pending(SaveTask(
                async move {
                    let actions = request.await.map_err(SaveError::CodeActions)?;
                    Ok(SaveUpdate {
                        operation: self,
                        stage: CompletedStage::Actions {
                            context,
                            server,
                            kind,
                            actions,
                        },
                    })
                }
                .boxed(),
            )));
        }
        // Build formatting only after all action kinds have completed: each stage
        // must see edits from its predecessor, including language overrides.
        if self.request.auto_format
            && let Some(format) = doc.auto_format(editor)
        {
            let snapshot = FormatSnapshot {
                version: doc.version(),
                uri: doc.uri(),
                language: doc
                    .language_config()
                    .map(|config| config.language_id.clone()),
                loader: editor.syn_loader.load_full(),
            };
            return Ok(SaveProgress::Pending(SaveTask(
                async move {
                    let result = format.await;
                    Ok(SaveUpdate {
                        operation: self,
                        stage: CompletedStage::Formatting { snapshot, result },
                    })
                }
                .boxed(),
            )));
        }
        // Keep pre-save work deferred, including when no configured provider is
        // available. This preserves command and batch submission error timing.
        Ok(SaveProgress::Pending(SaveTask(
            async move {
                Ok(SaveUpdate {
                    operation: self,
                    stage: CompletedStage::Write,
                })
            }
            .boxed(),
        )))
    }

    fn write(self, editor: &mut Editor) -> Result<SaveProgress, SaveError> {
        if editor.document(self.request.doc_id).is_none() {
            return Ok(SaveProgress::DocumentClosed);
        }
        editor
            .save(self.request.doc_id, self.request.path, self.request.force)
            .map_err(SaveError::Write)?;
        Ok(SaveProgress::Queued)
    }
}

impl SaveUpdate {
    pub fn apply(self, editor: &mut Editor) -> Result<SaveProgress, SaveError> {
        let Self { operation, stage } = self;
        match stage {
            CompletedStage::Write => operation.write(editor),
            CompletedStage::Actions {
                context,
                server,
                kind,
                actions,
            } => {
                if !context.is_current(editor, server) {
                    return operation.advance(editor);
                }
                let client = editor.language_server_by_id(server).unwrap();
                let offset_encoding = client.offset_encoding();
                let steps: Vec<_> = actions
                    .into_iter()
                    .flatten()
                    .filter_map(|action| match action {
                        CodeActionOrCommand::CodeAction(action)
                            if action.disabled.is_none()
                                && code_action_kind_matches(&action, &kind) =>
                        {
                            let resolve = (action.edit.is_none() || action.command.is_none())
                                .then(|| client.resolve_code_action(&action))
                                .flatten();
                            Some((action.edit, resolve))
                        }
                        _ => None,
                    })
                    .collect();
                Ok(SaveProgress::Pending(SaveTask(
                    async move {
                        let mut edits = Vec::new();
                        for (edit, resolve) in steps {
                            if let Some(resolve) = resolve {
                                match resolve.await {
                                    Ok(action) => edits.extend(action.edit),
                                    Err(error) => log::error!(
                                        "code-actions-on-save: resolve failed: {error:?}"
                                    ),
                                }
                            } else {
                                edits.extend(edit);
                            }
                        }
                        Ok(SaveUpdate {
                            operation,
                            stage: CompletedStage::Edits {
                                context,
                                server,
                                offset_encoding,
                                edits,
                            },
                        })
                    }
                    .boxed(),
                )))
            }
            CompletedStage::Edits {
                context,
                server,
                offset_encoding,
                edits,
            } => {
                if context.is_current(editor, server) {
                    // Preserve server order and existing partial-failure policy:
                    // log failed edits, continue remaining actions, then save.
                    for edit in edits {
                        if let Err(error) = editor.apply_workspace_edit(offset_encoding, &edit) {
                            log::error!("code-actions-on-save: failed to apply edit: {error:?}");
                        }
                    }
                }
                operation.advance(editor)
            }
            CompletedStage::Formatting { snapshot, result } => {
                let doc_id = operation.request.doc_id;
                let view_id = operation.request.view_id;
                let Some(doc) = editor.document(doc_id) else {
                    return Ok(SaveProgress::DocumentClosed);
                };
                match result {
                    Ok(transaction) if snapshot.is_current(editor, doc) => {
                        if editor
                            .tree
                            .try_get(view_id)
                            .is_some_and(|view| view.doc == doc_id)
                        {
                            let scrolloff = editor.config().scroll.padding;
                            let doc = doc_mut!(editor, &doc_id);
                            let view = view_mut!(editor, view_id);
                            doc.apply(&transaction, view_id);
                            doc.append_changes_to_history(view);
                            view.ensure_cursor_in_view(doc, scrolloff);
                        } else {
                            editor.apply_document_transaction(doc_id, &transaction);
                        }
                        doc_mut!(editor, &doc_id).detect_indent_and_line_ending();
                    }
                    Ok(_) => {
                        log::info!("discarded formatting changes because the document or formatter configuration changed")
                    }
                    Err(error) => log::info!("failed to format '{}': {error}", doc.display_name()),
                }
                // Save belongs to the document. Switching or closing a split must
                // neither redirect an edit nor discard an accepted write.
                operation.write(editor)
            }
        }
    }
}

/// LSP kinds form a dot-separated hierarchy. A server may ignore `only`, so
/// reject unrelated, unclassified, disabled, and command-only actions on save.
fn code_action_kind_matches(action: &CodeAction, requested: &CodeActionKind) -> bool {
    action.kind.as_ref().is_some_and(|kind| {
        let (kind, requested) = (kind.as_str(), requested.as_str());
        kind == requested
            || (kind.starts_with(requested) && kind[requested.len()..].starts_with('.'))
    })
}

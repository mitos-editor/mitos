//! Symbol renaming bound to the document and server that prepared it.

use anyhow::{bail, ensure, Context as _, Result};
use event::{cancelable_future, register_hook, TaskHandle};
use futures_util::future::BoxFuture;
use language_config::LanguageServerFeature;
use lsp_client::{lsp, util::lsp_range_to_range, LanguageServerId, OffsetEncoding};

use crate::{
    action::LspActionContext,
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, DocumentDidChange, LanguageServerExited, SelectionDidChange},
    DocumentId, Editor, ViewId,
};

struct RenameContext {
    document: DocumentId,
    snapshot: LspActionContext,
    server: LanguageServerId,
    owner: EditorCallbackSender,
    cancel: TaskHandle,
    identifier: lsp::TextDocumentIdentifier,
    position: lsp::Position,
    encoding: OffsetEncoding,
}

impl RenameContext {
    fn is_current(&self, editor: &Editor) -> bool {
        !self.cancel.is_canceled()
            && editor.config().lsp.enable
            && self.owner.same_destination(editor.callback_sender())
            && self.snapshot.is_current(editor, self.server)
    }
}

/// Owned preparation; dropping its future cancels transport tracking.
pub struct RenameTask {
    context: RenameContext,
    view: ViewId,
    selection: editor_core::Selection,
    fallback: String,
    future: BoxFuture<'static, lsp_client::Result<Option<lsp::PrepareRenameResponse>>>,
}

impl RenameTask {
    pub async fn run(self) -> Option<RenameUpdate> {
        let result = cancelable_future(self.future, &self.context.cancel).await?;
        Some(RenameUpdate {
            context: self.context,
            view: self.view,
            selection: self.selection,
            fallback: self.fallback,
            result,
        })
    }
}

/// Preparation is validated again after delivery through the frontend queue.
pub struct RenameUpdate {
    context: RenameContext,
    view: ViewId,
    selection: editor_core::Selection,
    fallback: String,
    result: lsp_client::Result<Option<lsp::PrepareRenameResponse>>,
}

fn rename_range(
    text: &editor_core::Rope,
    range: lsp::Range,
    encoding: OffsetEncoding,
) -> Option<editor_core::Range> {
    (range.start <= range.end && (range.end.line as usize) < text.len_lines())
        .then(|| lsp_range_to_range(text, range, encoding))
        .flatten()
}

impl RenameUpdate {
    pub fn into_result(self, editor: &Editor) -> Option<Result<RenameSession>> {
        if !self.context.is_current(editor)
            || editor.tree.focus() != self.view
            || !editor
                .tree
                .try_get(self.view)
                .is_some_and(|view| view.document_id() == self.context.document)
        {
            return None;
        }
        let doc = editor.document(self.context.document)?;
        if doc.try_selection(self.view) != Some(&self.selection) {
            return None;
        }
        Some((|| {
            let prefill = match self.result? {
                Some(lsp::PrepareRenameResponse::Range(range)) => {
                    rename_range(doc.text(), range, self.context.encoding)
                        .context("Language server sent an invalid rename range")?
                        .fragment(doc.text().slice(..))
                        .into()
                }
                Some(lsp::PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }) => {
                    ensure!(
                        rename_range(doc.text(), range, self.context.encoding).is_some(),
                        "Language server sent an invalid rename range"
                    );
                    placeholder
                }
                Some(lsp::PrepareRenameResponse::DefaultBehavior {
                    default_behavior: true,
                }) => self.fallback,
                _ => bail!("Symbol cannot be renamed"),
            };
            Ok(RenameSession {
                context: self.context,
                prefill,
            })
        })())
    }
}

/// A prepared target. Focus can move without redirecting its submitted rename.
pub struct RenameSession {
    context: RenameContext,
    prefill: String,
}

impl RenameSession {
    pub fn prefill(&self) -> &str {
        &self.prefill
    }
}

impl Editor {
    /// Prepare a symbol at an explicit document and view without borrowing the editor while waiting.
    pub fn prepare_rename(&mut self, document: DocumentId, view: ViewId) -> Result<RenameTask> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        ensure!(
            self.tree
                .try_get(view)
                .is_some_and(|v| v.document_id() == document),
            "Rename view no longer displays the document"
        );
        let owner = self.callback_sender().clone();
        let doc = self
            .document_mut(document)
            .context("Rename document is closed")?;
        let mut servers = doc.language_servers_with_feature(LanguageServerFeature::RenameSymbol);
        let first = servers
            .next()
            .context("No configured language server supports symbol renaming")?;
        // Preserve preference for a provider that can validate the target first.
        let server = std::iter::once(first)
            .chain(servers)
            .find(|server| {
                matches!(
                    server.capabilities().rename_provider,
                    Some(lsp::OneOf::Right(lsp::RenameOptions {
                        prepare_provider: Some(true),
                        ..
                    }))
                )
            })
            .unwrap_or(first);
        let selection = doc
            .try_selection(view)
            .context("Rename view has no selection")?
            .clone();
        let primary = selection.primary();
        let word = if primary.len() > 1 {
            primary
        } else {
            editor_core::textobject::textobject_word(
                doc.text().slice(..),
                primary,
                editor_core::textobject::TextObject::Inside,
                1,
                false,
            )
        };
        let fallback = word.fragment(doc.text().slice(..)).into();
        let encoding = server.offset_encoding();
        let position = doc.position(view, encoding);
        let identifier = doc.identifier();
        let server_id = server.id();
        let future = match server.prepare_rename(identifier.clone(), position) {
            Some(future) => Box::pin(future) as BoxFuture<'static, _>,
            None => Box::pin(async {
                Ok(Some(lsp::PrepareRenameResponse::DefaultBehavior {
                    default_behavior: true,
                }))
            }),
        };
        let context = RenameContext {
            document,
            snapshot: LspActionContext::new(doc),
            server: server_id,
            owner,
            cancel: doc.rename.restart(),
            identifier,
            position,
            encoding,
        };
        Ok(RenameTask {
            context,
            view,
            selection,
            fallback,
            future,
        })
    }

    /// Submit against the prepared position. Replies validate the original snapshot before editing.
    pub fn rename_symbol(&mut self, session: RenameSession, name: String) -> Result<()> {
        let context = session.context;
        ensure!(
            context.is_current(self),
            "Rename target is no longer current"
        );
        let server = self
            .language_server_by_id(context.server)
            .context("Rename server has stopped")?;
        let future = server
            .rename_symbol(context.identifier.clone(), context.position, name)
            .context("Language server no longer supports renaming")?;
        let callbacks = context.owner.clone();
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, &context.cancel).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !context.is_current(editor) {
                        return;
                    }
                    match result {
                        Ok(Some(edit)) => {
                            if let Err(error) = editor.apply_workspace_edit(context.encoding, &edit)
                            {
                                editor.set_error(|| format!("Failed to apply rename: {error}"));
                            }
                        }
                        Ok(None) => {}
                        Err(error) => editor.set_error(|| format!("Rename failed: {error}")),
                    }
                })
                .await;
        });
        Ok(())
    }
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.rename.cancel();
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.rename.cancel();
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.rename.cancel();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.rename.cancel();
            }
        }
        Ok(())
    });
}

//! Read-only content owned by a language-server instance and URI.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{ensure, Context, Result};
use editor_core::Rope;
use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use lsp_client::{lsp, LanguageServerId};
use tokio::sync::OnceCell;

use crate::{
    callbacks::EditorCallbackSender,
    editor::Action,
    events::{ConfigDidChange, LanguageServerExited},
    features::{request::SelectionRequest, source_documents::SourceDocumentKey},
    Document, DocumentId, Editor,
};

/// The editor destination for content loads and refreshes.
pub struct VirtualDocumentsHandler {
    pub(crate) callbacks: EditorCallbackSender,
}

impl VirtualDocumentsHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self { callbacks }
    }
}

pub(crate) struct DocumentSource {
    pub uri: lsp::Uri,
    pub server: LanguageServerId,
    pub workspace: PathBuf,
}

/// Expected cancellation when a newer interaction or content generation takes over.
#[derive(Debug, thiserror::Error)]
#[error("Content request was canceled")]
pub struct ContentRequestCanceled;

type Key = (LanguageServerId, String);

struct Entry {
    content: Arc<OnceCell<String>>,
    _controller: TaskController,
    current: TaskHandle,
}

impl Entry {
    fn new() -> Self {
        let mut controller = TaskController::new();
        let current = controller.restart();
        Self {
            content: Arc::new(OnceCell::new()),
            _controller: controller,
            current,
        }
    }
}

struct Navigation {
    selection: Option<lsp::Range>,
    action: Action,
    cancel: Option<TaskHandle>,
}

#[derive(Default)]
pub(crate) struct VirtualDocuments {
    entries: HashMap<Key, Entry>,
}

impl VirtualDocuments {
    pub(super) fn remove(&mut self, server: LanguageServerId, uri: &str) {
        self.entries.remove(&(server, uri.to_owned()));
    }
}

impl Editor {
    /// Find an already loaded document without confusing equal URIs from two servers.
    pub fn language_server_document(
        &self,
        server: LanguageServerId,
        uri: &lsp::Uri,
    ) -> Option<DocumentId> {
        self.source_document(&SourceDocumentKey::LanguageServer(
            server,
            uri.as_str().into(),
        ))
    }

    fn content_request(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
    ) -> Result<(
        TaskHandle,
        impl std::future::Future<Output = lsp_client::Result<String>> + Send + 'static + use<>,
    )> {
        ensure!(self.config().lsp.enable, "Language servers are disabled");
        let client = self
            .language_servers
            .get_by_id(server)
            .cloned()
            .context("Language server has stopped")?;
        ensure!(
            self.language_servers.is_ready(server) && client.supports_text_document_content(&uri),
            "No content provider for {}",
            uri.as_str()
        );
        let entry = self
            .virtual_documents
            .entries
            .entry((server, uri.as_str().into()))
            .or_insert_with(Entry::new);
        let current = entry.current.clone();
        let content = entry.content.clone();
        Ok((current, async move {
            content
                .get_or_try_init(|| async {
                    client
                        .text_document_content(uri)
                        .await
                        .map(|result| result.text)
                })
                .await
                .cloned()
        }))
    }

    /// Fetch a server URI, then navigate only while the initiating selection is current.
    /// Concurrent requests share the content fetch. The callback always receives a result.
    pub fn request_language_server_document(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
        selection: Option<lsp::Range>,
        action: Action,
        callback: impl FnOnce(&mut Editor, Result<DocumentId>) + Send + 'static,
    ) {
        self.request_content_navigation(
            server,
            uri,
            Navigation {
                selection,
                action,
                cancel: None,
            },
            callback,
        );
    }

    /// Load content for an explicit background operation without changing focus.
    pub fn load_language_server_document(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
        cancel: TaskHandle,
        callback: impl FnOnce(&mut Editor, Result<DocumentId>) + Send + 'static,
    ) {
        self.request_content_navigation(
            server,
            uri,
            Navigation {
                selection: None,
                action: Action::Load,
                cancel: Some(cancel),
            },
            callback,
        );
    }

    /// Load a picker preview without changing focus; hiding the picker cancels publication.
    pub fn preload_language_server_document(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
        cancel: TaskHandle,
    ) {
        self.request_content_navigation(
            server,
            uri,
            Navigation {
                selection: None,
                action: Action::Load,
                cancel: Some(cancel),
            },
            |_, result| {
                if let Err(error) = result {
                    log::debug!("Server document preview: {error}");
                }
            },
        );
    }

    fn request_content_navigation(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
        navigation: Navigation,
        callback: impl FnOnce(&mut Editor, Result<DocumentId>) + Send + 'static,
    ) {
        let Navigation {
            selection,
            action,
            cancel,
        } = navigation;
        let (current, future) = match self.content_request(server, uri.clone()) {
            Ok(request) => request,
            Err(error) => {
                callback(self, Err(error));
                return;
            }
        };
        let (view, doc) = crate::current!(self);
        let binding = view.binding_revision();
        let navigation = if !matches!(action, Action::Load) {
            doc.view_state_mut(view.id)
                .navigation
                .get_or_insert_default()
                .restart()
        } else {
            current.clone()
        };
        let origin = SelectionRequest::new(doc, view.id, navigation, Vec::new());
        let mode = self.mode();
        let callbacks = self.handlers.virtual_documents.callbacks.clone();
        let owner = callbacks.clone();
        tokio::spawn(async move {
            let future = cancelable_future(future, &current);
            let future = cancelable_future(future, origin.cancel_handle());
            let result = if let Some(cancel) = &cancel {
                cancelable_future(future, cancel).await.flatten().flatten()
            } else {
                future.await.flatten()
            };
            callbacks
                .send(move |editor| {
                    let result = (|| {
                        if !owner.same_destination(&editor.handlers.virtual_documents.callbacks)
                            || current.is_canceled()
                            || cancel.as_ref().is_some_and(|cancel| cancel.is_canceled())
                            || !editor.config().lsp.enable
                        {
                            return Err(ContentRequestCanceled.into());
                        }
                        if !matches!(action, Action::Load)
                            && (editor.mode() != mode
                                || origin.get(editor).is_none_or(|(view, _)| {
                                    view.id != editor.tree.focus()
                                        || view.binding_revision() != binding
                                }))
                        {
                            return Err(ContentRequestCanceled.into());
                        }
                        let text = result.ok_or(ContentRequestCanceled)??;
                        let document =
                            editor.install_language_server_document(server, uri, text)?;
                        let encoding = editor
                            .language_server_by_id(server)
                            .context("Language server has stopped")?
                            .offset_encoding();
                        editor.switch(document, action);
                        editor.position_document_location(document, selection, encoding);
                        Ok(document)
                    })();
                    callback(editor, result);
                    editor.redraw().request();
                })
                .await;
        });
    }

    fn install_language_server_document(
        &mut self,
        server: LanguageServerId,
        uri: lsp::Uri,
        text: String,
    ) -> Result<DocumentId> {
        let key = (server, uri.as_str().into());
        ensure!(
            self.virtual_documents.entries.contains_key(&key),
            "Content request was invalidated"
        );
        if let Some(id) = self.language_server_document(server, &uri) {
            self.replace_source_document_content(id, &Rope::from(text), false)?;
            return Ok(id);
        }
        let client = self
            .language_servers
            .get_by_id(server)
            .cloned()
            .context("Language server has stopped")?;
        let (id, _) = self.open_source_document(
            Some(SourceDocumentKey::LanguageServer(
                server,
                uri.as_str().into(),
            )),
            Action::Load,
            |editor| {
                let mut doc = Document::from(
                    Rope::from(text),
                    None,
                    editor.config.clone(),
                    editor.syn_loader.load_full(),
                )
                .with_language_server_source(DocumentSource {
                    uri: uri.clone(),
                    server,
                    workspace: client.root_path().to_path_buf(),
                });
                let loader = editor.syn_loader.load_full();
                let filename = uri.as_str().split(['?', '#']).next().unwrap_or_default();
                if let Some(language) = loader.language_for_filename(std::path::Path::new(filename))
                {
                    doc.set_language(Some(loader.language(language).config().clone()), &loader);
                }
                doc.attach_content_server(client);
                doc
            },
        );
        Ok(id)
    }

    /// Invalidate this server's content generation and update an existing buffer in place.
    pub fn refresh_language_server_document(&mut self, server: LanguageServerId, uri: lsp::Uri) {
        let key = (server, uri.as_str().into());
        let Some(old) = self.virtual_documents.entries.remove(&key) else {
            return;
        };
        drop(old);
        let Some(_) = self.language_server_document(server, &uri) else {
            return;
        };
        self.virtual_documents.entries.insert(key, Entry::new());
        let (current, future) = match self.content_request(server, uri.clone()) {
            Ok(request) => request,
            Err(error) => {
                log::debug!("Unable to refresh server content: {error}");
                return;
            }
        };
        let callbacks = self.handlers.virtual_documents.callbacks.clone();
        let owner = callbacks.clone();
        tokio::spawn(async move {
            let Some(result) = cancelable_future(future, &current).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if current.is_canceled()
                        || !editor.config().lsp.enable
                        || !owner.same_destination(&editor.handlers.virtual_documents.callbacks)
                    {
                        return;
                    }
                    let result = result.map_err(anyhow::Error::from).and_then(|text| {
                        editor.install_language_server_document(server, uri, text)
                    });
                    if let Err(error) = result {
                        log::warn!("Unable to refresh server content: {error}");
                    }
                    editor.redraw().request();
                })
                .await;
        });
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        event.editor.virtual_documents.entries.retain(|(server, _), _| *server != event.server_id);
        event.editor.source_documents.remove_server(event.server_id);
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            event.editor.virtual_documents.entries.clear();
            event.editor.source_documents.remove_language_servers();
        }
        Ok(())
    });
}

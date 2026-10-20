//! Shared code lens requests, lazy resolution, and lifecycle hooks.
// Adapted from helix-editor/helix#5063 by Matouš Dzivjak.

use std::{collections::HashSet, future::Future, sync::Arc};

use event::{cancelable_future, register_hook};
use futures_util::{stream::FuturesUnordered, StreamExt};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, Client, LanguageServerId};
use tokio::sync::Semaphore;

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        LanguageServerExited, LanguageServerInitialized,
    },
    features::{debounce::debounce_documents, request::DocumentRequest},
    DocumentId, Editor,
};

use super::CodeLens;

const MAX_CONCURRENT_RESOLUTIONS: usize = 4;

#[derive(Clone)]
pub struct DocumentCodeLensesHandler {
    callbacks: EditorCallbackSender,
    events: tokio::sync::mpsc::Sender<DocumentId>,
    resolution_slots: Arc<Semaphore>,
}

impl DocumentCodeLensesHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let events = debounce_documents(callbacks.clone(), |editor, doc| {
            request_document_code_lenses(editor, doc, None);
        });
        Self {
            callbacks,
            events,
            resolution_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_RESOLUTIONS)),
        }
    }
}

fn request_document_code_lenses(
    editor: &mut Editor,
    doc_id: DocumentId,
    exited_server: Option<LanguageServerId>,
) {
    if !editor.config().lsp.enable || !editor.config().lsp.code_lenses.enable {
        return;
    }
    let callbacks = editor.handlers.code_lenses.callbacks.clone();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    // A debounce callback or server refresh may arrive during a completion preview.
    // Its ranges would describe the server's text rather than the displayed buffer.
    if doc.code_lenses.preview_base.is_some() {
        return;
    }
    doc.code_lenses.cancel_requests();
    doc.code_lenses.version = None;
    let cancel = doc.code_lenses.request.restart();
    let mut servers = HashSet::new();
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::CodeLens)
        // Exit hooks run before removal from the registry.
        .filter(|server| Some(server.id()) != exited_server)
        .filter(|server| servers.insert(server.id()))
        .filter_map(|server| {
            let text = doc.text().clone();
            let encoding = server.offset_encoding();
            let language_server_id = server.id();
            let future = server.code_lens(doc.identifier())?;
            Some(async move {
                let lenses = future.await?.unwrap_or_default();
                let lenses: Vec<_> = lenses
                    .into_iter()
                    .filter_map(|lens| {
                        if lens.range.start > lens.range.end
                            || lens.range.start.line != lens.range.end.line
                            || lens.range.end.line as usize >= text.len_lines()
                        {
                            return None;
                        }
                        let char_idx =
                            lsp_client::util::lsp_pos_to_pos(&text, lens.range.start, encoding)?;
                        lsp_client::util::lsp_pos_to_pos(&text, lens.range.end, encoding)?;
                        Some(CodeLens {
                            char_idx,
                            lens,
                            language_server_id,
                        })
                    })
                    .collect();
                anyhow::Ok(lenses)
            })
        })
        .collect();
    if futures.is_empty() {
        doc.code_lenses.cache.clear();
        return;
    }
    let request = DocumentRequest::new(doc, cancel, servers.into_iter().collect());
    tokio::spawn(async move {
        let mut lenses = Vec::new();
        loop {
            match cancelable_future(futures.next(), &request.cancel).await {
                Some(Some(Ok(items))) => lenses.extend(items),
                Some(Some(Err(err))) => log::error!("code lens request failed: {err}"),
                Some(None) => break,
                None => return,
            }
        }
        lenses.sort_by_key(|lens| (lens.char_idx, lens.language_server_id));
        callbacks
            .send(move |editor| {
                if request.is_current(editor)
                    && editor.config().lsp.enable
                    && editor.config().lsp.code_lenses.enable
                {
                    let doc = editor.document_mut(doc_id).unwrap();
                    doc.code_lenses.version = Some(doc.version());
                    doc.code_lenses.uri = doc.uri();
                    doc.code_lenses.cache = lenses;
                    editor.redraw().request();
                }
            })
            .await;
    });
}

/// Handle workspace/codeLens/refresh for documents attached to its sender.
pub fn refresh_for_language_server(editor: &mut Editor, server: LanguageServerId) {
    let docs: Vec<_> = editor
        .documents()
        .filter(|doc| doc.supports_language_server(server))
        .map(|doc| doc.id())
        .collect();
    for doc in docs {
        request_document_code_lenses(editor, doc, None);
    }
}

/// Rebind lenses when a document changes language, path, or attached servers.
pub(crate) fn refresh_document(editor: &mut Editor, doc: DocumentId) {
    if let Some(doc) = editor.document_mut(doc) {
        doc.code_lenses.clear();
    }
    request_document_code_lenses(editor, doc, None);
}

#[derive(Clone)]
pub struct CodeLensCommand {
    pub command: lsp::Command,
    pub language_server_id: LanguageServerId,
}

/// Resolve titles for the visible portions of every split before drawing a frame.
/// Frontends call this preparation step when the viewport or cached lenses change;
/// the renderer itself only reads the cache. Repeated frames do not repeat attempts.
pub fn prepare_visible_code_lenses(editor: &mut Editor) {
    if !editor.config().lsp.enable || !editor.config().lsp.code_lenses.enable {
        return;
    }
    let mut pending = Vec::new();
    let mut seen = HashSet::new();
    for (view, _) in editor.tree.views() {
        let Some(doc) = editor.document(view.doc) else {
            continue;
        };
        if doc.code_lenses.version != Some(doc.version())
            || doc.code_lenses.uri != doc.uri()
            || doc.code_lenses.preview_base.is_some()
            || !doc.code_lenses().iter().enumerate().any(|(index, lens)| {
                lens.lens.command.is_none() && !doc.code_lenses.titles.contains_key(&index)
            })
        {
            continue;
        }
        let area = view.inner_area(doc);
        if area.width == 0 || area.height == 0 {
            continue;
        }
        let layout = view.layout(doc, Some(&editor.theme));
        let offset = layout.offset;
        let text = layout.text;
        let format = &layout.format;
        let annotations = &layout.annotations;
        let row_offset = text_layout::visual_offset_from_block(
            text,
            offset.anchor,
            offset.anchor,
            format,
            annotations,
        )
        .0
        .row + offset.vertical_offset;
        let mut visible_lines = HashSet::new();
        for grapheme in text_layout::doc_formatter::DocumentFormatter::new_at_prev_checkpoint(
            text,
            format,
            annotations,
            offset.anchor,
        ) {
            if grapheme.visual_pos.row >= row_offset + area.height as usize {
                break;
            }
            if grapheme.visual_pos.row >= row_offset && !grapheme.is_virtual() {
                visible_lines.insert(grapheme.line_idx);
            }
        }
        for (index, lens) in doc.code_lenses().iter().enumerate() {
            if lens.lens.command.is_some()
                || doc.code_lenses.titles.contains_key(&index)
                || !visible_lines.contains(&doc.text().char_to_line(lens.char_idx))
                || !seen.insert((doc.id(), index))
            {
                continue;
            }
            let Some(server) = doc
                .language_servers_with_feature(LanguageServerFeature::CodeLens)
                .find(|server| server.id() == lens.language_server_id)
            else {
                continue;
            };
            if server
                .capabilities()
                .code_lens_provider
                .as_ref()
                .and_then(|provider| provider.resolve_provider)
                != Some(true)
            {
                continue;
            }
            let Some(server) = editor.language_servers.get_by_id(server.id()) else {
                continue;
            };
            pending.push((doc.id(), index, lens.lens.clone(), server.clone()));
        }
    }
    let handler = editor.handlers.code_lenses.clone();
    for (doc_id, index, lens, server) in pending {
        let doc = editor.document_mut(doc_id).unwrap();
        let mut controller = event::TaskController::new();
        let cancel = controller.restart();
        doc.code_lenses.titles.insert(index, controller);
        let request = DocumentRequest::new(doc, cancel, vec![server.id()]);
        let slots = handler.resolution_slots.clone();
        let callbacks = handler.callbacks.clone();
        tokio::spawn(async move {
            let Some(result) =
                cancelable_future(resolve_lens(server, lens, slots), &request.cancel).await
            else {
                return;
            };
            let lens = match result {
                Ok(Some(lens)) => lens,
                Ok(None) => return,
                Err(err) => {
                    log::error!("code lens title resolution failed: {err}");
                    return;
                }
            };
            callbacks
                .send(move |editor| {
                    if request.is_current(editor)
                        && editor.config().lsp.enable
                        && editor.config().lsp.code_lenses.enable
                    {
                        editor.document_mut(doc_id).unwrap().code_lenses.cache[index].lens = lens;
                        editor.redraw().request();
                    }
                })
                .await;
        });
    }
}

async fn resolve_lens(
    server: Arc<Client>,
    lens: lsp::CodeLens,
    slots: Arc<Semaphore>,
) -> lsp_client::Result<Option<lsp::CodeLens>> {
    if lens.command.is_some() {
        return Ok(Some(lens));
    }
    // Client calls enqueue requests eagerly: acquire the slot before constructing
    // the future, rather than merely limiting the number of futures being polled.
    let _slot = slots
        .acquire()
        .await
        .expect("code lens semaphore remains open");
    let range = lens.range;
    let Some(future) = server.resolve_code_lens(lens) else {
        return Ok(None);
    };
    let resolved = future.await?;
    Ok((resolved.command.is_some() && resolved.range == range).then_some(resolved))
}

/// Resolution snapshot. Frontends must validate before presenting or executing it.
pub struct ResolvedCodeLenses {
    request: DocumentRequest,
    doc: DocumentId,
    updates: Vec<(usize, lsp::CodeLens)>,
    pub commands: Vec<CodeLensCommand>,
}

impl ResolvedCodeLenses {
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.request.is_current(editor)
            && editor.config().lsp.enable
            && editor.config().lsp.code_lenses.enable
    }

    /// Cache by original index, preserving distinct lenses with equal ranges/data.
    pub fn cache(&self, editor: &mut Editor) {
        if !self.is_current(editor) {
            return;
        }
        let doc = editor.document_mut(self.doc).unwrap();
        for (index, lens) in &self.updates {
            doc.code_lenses.cache[*index].lens = lens.clone();
        }
    }
}

/// Resolve only lenses on the requested line, through their originating servers.
/// Request and response handling stay independent of the frontend's job system.
pub fn resolve_code_lenses(
    editor: &mut Editor,
    doc_id: DocumentId,
    line: usize,
) -> Option<impl Future<Output = Option<ResolvedCodeLenses>> + Send + use<>> {
    if !editor.config().lsp.enable || !editor.config().lsp.code_lenses.enable {
        return None;
    }
    let slots = editor.handlers.code_lenses.resolution_slots.clone();
    let doc = editor.document_mut(doc_id)?;
    // Mapped gutter positions remain useful during debounce, but protocol ranges
    // and commands still belong to the snapshot that produced the cache.
    if doc.code_lenses.version != Some(doc.version()) || doc.code_lenses.uri != doc.uri() {
        return None;
    }
    let cancel = doc.code_lenses.resolve.restart();
    let mut servers = HashSet::new();
    let mut futures: FuturesUnordered<_> = doc
        .code_lenses()
        .iter()
        .enumerate()
        .filter(|(_, lens)| doc.text().char_to_line(lens.char_idx) == line)
        .filter_map(|(index, lens)| {
            let server = doc
                .language_servers_with_feature(LanguageServerFeature::CodeLens)
                .find(|server| server.id() == lens.language_server_id)?;
            servers.insert(server.id());
            let server = doc
                .language_servers
                .values()
                .find(|client| client.id() == server.id())?
                .clone();
            let language_server_id = lens.language_server_id;
            let lens = lens.lens.clone();
            let slots = slots.clone();
            Some(async move {
                let Some(resolved) = resolve_lens(server, lens, slots).await? else {
                    return anyhow::Ok(None);
                };
                anyhow::Ok(resolved.command.clone().map(|command| {
                    (
                        index,
                        resolved,
                        CodeLensCommand {
                            command,
                            language_server_id,
                        },
                    )
                }))
            })
        })
        .collect();
    let request = DocumentRequest::new(doc, cancel, servers.into_iter().collect());
    Some(async move {
        let mut items = Vec::new();
        loop {
            match cancelable_future(futures.next(), &request.cancel).await {
                Some(Some(Ok(Some(item)))) => items.push(item),
                Some(Some(Ok(None))) => (),
                Some(Some(Err(err))) => log::error!("code lens resolution failed: {err}"),
                Some(None) => break,
                None => return None,
            }
        }
        items.sort_by_key(|(index, _, _)| *index);
        let (updates, commands) = items
            .into_iter()
            .map(|(index, lens, command)| ((index, lens), command))
            .unzip();
        Some(ResolvedCodeLenses {
            request,
            doc: doc_id,
            updates,
            commands,
        })
    })
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request_document_code_lenses(event.editor, event.doc, None);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event.doc.code_lenses.preview_base.is_none()
            && let Some(handler) = &event.doc.code_lenses.handler
        {
            event::send_blocking(&handler.events, event.doc.id());
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        event.doc.code_lenses.cancel_requests();
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        refresh_for_language_server(event.editor, event.server_id);
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        let docs: Vec<_> = event
            .editor
            .documents_mut()
            .filter(|doc| doc.supports_language_server(event.server_id))
            .map(|doc| {
                doc.code_lenses
                    .cache
                    .retain(|lens| lens.language_server_id != event.server_id);
                doc.id()
            })
            .collect();
        for doc in docs {
            request_document_code_lenses(event.editor, doc, Some(event.server_id));
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        for doc in event.editor.documents_mut() {
            doc.code_lenses.clear();
        }
        let docs: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();
        for doc in docs {
            request_document_code_lenses(event.editor, doc, None);
        }
        Ok(())
    });
}

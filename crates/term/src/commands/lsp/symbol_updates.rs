//! Refresh symbol providers when their source changes, without resetting picker input.

use std::{path::PathBuf, sync::Arc};

use editor_core::Uri;
use lsp_client::LanguageServerId;
use tokio::sync::oneshot;
use view::{features::document_symbols::DocumentSymbolsUpdate, DocumentId, ViewId};

use super::*;
use crate::ui::picker::UpdateContext;

#[derive(PartialEq)]
struct SourceState {
    documents: Vec<(DocumentId, i32, Option<Uri>)>,
    providers: Vec<(LanguageServerId, usize)>,
}

fn source_state(editor: &Editor, document: DocumentId, workspace: bool) -> Option<SourceState> {
    let doc = editor.document(document)?;
    if !editor.config().lsp.enable {
        return None;
    }
    let feature = if workspace {
        LanguageServerFeature::WorkspaceSymbols
    } else {
        LanguageServerFeature::DocumentSymbols
    };
    let providers: Vec<_> = doc
        .language_servers_with_feature(feature)
        .map(|server| (server.id(), std::ptr::from_ref(server) as usize))
        .collect();
    if providers.is_empty() {
        return None;
    }
    let mut documents: Vec<_> = editor
        .documents()
        .filter(|doc| workspace || doc.id() == document)
        .map(|doc| (doc.id(), doc.version(), doc.uri()))
        .collect();
    documents.sort_by_key(|(id, _, _)| *id);
    Some(SourceState {
        documents,
        providers,
    })
}

pub(super) fn symbol_key(
    item: &SymbolInformationItem,
) -> (Option<PathBuf>, String, lsp::SymbolKind, Option<String>) {
    (
        item.location.uri.as_path().map(Path::to_owned),
        item.symbol.name.clone(),
        item.symbol.kind,
        item.symbol.container_name.clone(),
    )
}

fn flatten(
    symbols: Vec<view::features::document_symbols::DocumentSymbol>,
) -> Vec<SymbolInformationItem> {
    let mut items = Vec::new();
    for symbol in symbols {
        nested_symbol_to_flat(&mut items, symbol, "", None);
    }
    items
}

pub(super) fn document_updates(
    editor: &Editor,
    document: DocumentId,
    view: ViewId,
) -> impl FnMut(UpdateContext<'_, SymbolInformationItem>) -> Option<Vec<SymbolInformationItem>> + use<>
{
    let mut requested = source_state(editor, document, false);
    let mut pending: Option<oneshot::Receiver<Option<DocumentSymbolsUpdate>>> = None;
    move |cx| {
        if let Some(receiver) = &mut pending {
            match receiver.try_recv() {
                Ok(Some(update)) => {
                    pending = None;
                    if let Some(symbols) = update.into_symbols(cx.editor) {
                        return Some(flatten(symbols));
                    }
                    requested = None;
                }
                Ok(None) | Err(oneshot::error::TryRecvError::Closed) => {
                    pending = None;
                    requested = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        let state = source_state(cx.editor, document, false).filter(|_| {
            cx.editor.tree.focus() == view
                && cx
                    .editor
                    .tree
                    .try_get(view)
                    .is_some_and(|view| view.document_id() == document)
        });
        if requested == state {
            return None;
        }
        cx.task.cancel();
        pending = None;
        requested = state;
        if requested.is_none() {
            return Some(Vec::new());
        }
        let task = match cx.editor.document_symbols(document, view) {
            Ok(task) => task,
            Err(error) => {
                log::info!("Cannot refresh document symbols: {error}");
                return Some(Vec::new());
            }
        };
        let cancel = cx.task.restart();
        let redraw = cx.editor.redraw().on_drop();
        let (sender, receiver) = oneshot::channel();
        pending = Some(receiver);
        tokio::spawn(async move {
            let _redraw = redraw;
            let update = event::cancelable_future(task.run(), cancel).await.flatten();
            let _ = sender.send(update);
        });
        None
    }
}

pub(super) fn workspace_updates(
    editor: &Editor,
    document: DocumentId,
) -> impl FnMut(&mut Editor, &WorkspaceSymbolPickerData) -> bool + use<> {
    let mut requested = source_state(editor, document, true);
    let mut events = editor.file_watcher.subscribe();
    move |editor, data| {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut files_changed = false;
        loop {
            match events.try_recv() {
                Ok(events) => {
                    files_changed |= events.iter().any(|event| {
                        event.ty != view::file_watcher::EventType::Tempfile
                            && !editor.file_watcher.is_vcs_path(event.path.as_std_path())
                    })
                }
                Err(TryRecvError::Lagged(_)) => files_changed = true,
                Err(TryRecvError::Closed) => {
                    events = editor.file_watcher.subscribe();
                    files_changed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let state = source_state(editor, document, true);
        if requested == state && (state.is_none() || data.symbols.load().is_current(editor)) {
            return files_changed && state.is_some();
        }
        requested = state;
        if requested.is_some() {
            match editor.workspace_symbols(document) {
                Ok(session) => data.symbols.store(Arc::new(session)),
                Err(error) => log::info!("Cannot refresh workspace symbols: {error}"),
            }
        }
        true
    }
}

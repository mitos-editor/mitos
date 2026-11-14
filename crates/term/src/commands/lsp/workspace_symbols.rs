//! Lazy workspace locations for previews, navigation, and explicit quicklist export.

use super::*;
use crate::ui::picker::{Injector, UpdateContext};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use view::features::workspace_symbols::{WorkspaceSymbol, WorkspaceSymbolResolution};

type Item = Arc<WorkspaceSymbol>;

#[derive(Default)]
pub(super) struct PublishedSymbols {
    generation: u64,
    revision: u64,
    query: String,
    items: Vec<Item>,
    pending: bool,
    failed: bool,
}

pub(super) fn picker(cx: &mut Context) {
    let document = doc!(cx.editor).id();
    let session = match cx.editor.workspace_symbols(document) {
        Ok(session) => session,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    let published: Arc<Mutex<PublishedSymbols>> = Default::default();
    let rows = published.clone();
    let get_symbols = |pattern: &str,
                       editor: &mut Editor,
                       data: Arc<WorkspaceSymbolPickerData>,
                       _injector: &Injector<_, _>| {
        let request = data.symbols.load().query(editor, pattern.to_owned());
        let published = data.published.clone();
        let redraw = editor.redraw().clone();
        let generation = {
            let mut rows = published.lock().unwrap();
            rows.generation += 1;
            rows.revision += 1;
            rows.query = pattern.into();
            rows.items.clear();
            rows.pending = request.is_ok();
            rows.failed = request.is_err();
            rows.generation
        };
        async move {
            let mut task = request?;
            while let Some(response) = task.next_batch().await {
                let mut rows = published.lock().unwrap();
                if rows.generation != generation {
                    return Ok(());
                }
                match response {
                    Ok(items) => rows.items.extend(items),
                    Err(error) => {
                        rows.failed = true;
                        log::error!("Error requesting workspace symbols: {error}");
                    }
                }
                rows.revision += 1;
                drop(rows);
                redraw.request();
            }
            let mut rows = published.lock().unwrap();
            if rows.generation == generation {
                rows.pending = false;
                rows.revision += 1;
                redraw.request();
            }
            Ok(())
        }
        .boxed()
    };
    let session = Arc::new(arc_swap::ArcSwap::from_pointee(session));
    let columns = [
        ui::PickerColumn::new("kind", |item: &Item, data: &WorkspaceSymbolPickerData| {
            Cell::from(Span::styled(
                display_symbol_kind(item.kind),
                data.style.theme.get("ui.text.inactive"),
            ))
        })
        .with_prefix(|item, data| {
            data.style
                .show_icons
                .then(|| ICONS.kind().get(symbol_kind_icon_name(item.kind)))
                .flatten()
                .map(|icon| ui::icon_span(icon.with_no_left_padding()))
        }),
        ui::PickerColumn::new("name", |item: &Item, data: &WorkspaceSymbolPickerData| {
            Cell::from(Span::styled(
                item.name.as_str(),
                data.style.theme.get(symbol_kind_scope(item.kind)),
            ))
        })
        .without_filtering(),
        ui::PickerColumn::new(
            "container",
            |item: &Item, data: &WorkspaceSymbolPickerData| {
                Cell::from(Span::styled(
                    item.container_name.as_deref().unwrap_or_default(),
                    data.style.theme.get("ui.text.inactive"),
                ))
            },
        ),
        ui::PickerColumn::new("path", |item: &Item, data: &WorkspaceSymbolPickerData| {
            symbol_path_cell(&item.uri, &data.style)
        }),
    ];
    let picker_data = WorkspaceSymbolPickerData {
        style: SymbolPickerData {
            show_icons: cx.editor.config().icons,
            theme: cx.editor.theme.clone(),
        },
        symbols: session.clone(),
        published,
    };
    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        1,
        [],
        picker_data,
        move |cx, item: &Item, action| {
            if !session.load().is_current(cx.editor) {
                return;
            }
            if let Some(location) = item.location(cx.editor) {
                jump_to_location(cx.editor, location, action);
                return;
            }
            let request = match item.resolve(cx.editor) {
                Ok(request) => request,
                Err(error) => {
                    cx.editor.set_error(|| error.to_string());
                    return;
                }
            };
            let (view, doc) = current_ref!(cx.editor);
            let origin = (
                view.id,
                doc.id(),
                doc.version(),
                doc.selection(view.id).clone(),
            );
            cx.jobs.callback(async move {
                let result = request.run().await;
                Ok(Callback::editor(move |editor| {
                    let (view, doc) = current_ref!(editor);
                    if (
                        view.id,
                        doc.id(),
                        doc.version(),
                        doc.selection(view.id).clone(),
                    ) != origin
                    {
                        return;
                    }
                    match result {
                        Ok(Some(update)) => {
                            if let Some(item) = update.publish(editor)
                                && let Some(location) = item.location(editor)
                            {
                                jump_to_location(editor, location, action);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            editor.set_error(|| format!("Cannot resolve workspace symbol: {error}"))
                        }
                    }
                }))
            });
        },
    )
    .with_preview(|editor, item| {
        item.location(editor)
            .and_then(|location| location_to_file_location(editor, location))
    })
    .with_quicklist_batch(|cx, items, cancel| {
        let requests = items
            .into_iter()
            .map(|item| item.resolve(cx.editor))
            .collect::<anyhow::Result<Vec<_>>>();
        let requests = match requests {
            Ok(requests) => requests,
            Err(error) => {
                cx.editor.set_error(|| error.to_string());
                return;
            }
        };
        cx.jobs.callback(async move {
            let mut updates = Vec::with_capacity(requests.len());
            // Export is explicit; every matched item now needs an exact location.
            for request in requests {
                let Some(update) = event::cancelable_future(request.run(), &cancel)
                    .await
                    .transpose()?
                    .flatten()
                else {
                    return Ok(Callback::editor(|_| {}));
                };
                updates.push(update);
            }
            Ok(Callback::editor(move |editor| {
                if cancel.is_canceled() {
                    return;
                }
                export_quicklist(editor, updates, cancel);
            }))
        });
    })
    .with_editor_updates(resolve_selected(rows))
    .with_title("Workspace Symbols")
    .with_dynamic_query(get_symbols, None, cx.jobs)
    .with_item_key(|item| {
        (
            item.provider(),
            item.uri
                .to_url()
                .expect("workspace symbols have absolute URIs")
                .to_string(),
            item.name.clone(),
            serde_json::to_value(item.kind).expect("symbol kinds serialize as integers"),
            item.container_name.clone(),
        )
    })
    .with_query_updates(symbol_updates::workspace_updates(cx.editor, document))
    .truncate_start(false);
    cx.push_layer(Box::new(overlaid(picker)));
}

fn resolve_selected(
    rows: Arc<Mutex<PublishedSymbols>>,
) -> impl FnMut(UpdateContext<'_, Item>) -> Option<Vec<Item>> {
    let mut selected: Option<Item> = None;
    let mut query = String::new();
    let mut preview_visible = false;
    let mut content: Option<lsp::Uri> = None;
    let mut pending: Option<oneshot::Receiver<anyhow::Result<Option<WorkspaceSymbolResolution>>>> =
        None;
    let mut revision = 0;
    move |cx| {
        let replacement = {
            let mut rows = rows.lock().unwrap();
            if rows.query == cx.source_query && rows.revision != revision {
                revision = rows.revision;
                rows.items.retain(|item| item.is_current(cx.editor));
                *cx.incomplete = rows.pending;
                *cx.status = if rows.pending {
                    Some("Searching symbols…".into())
                } else if rows.failed {
                    Some("Some symbol providers failed".into())
                } else {
                    None
                };
                Some(rows.items.clone())
            } else {
                None
            }
        };
        let changed = query != cx.query
            || preview_visible != cx.preview_visible
            || selected
                .as_ref()
                .zip(cx.selected)
                .is_none_or(|(old, new)| !Arc::ptr_eq(old, new));
        if changed || cx.resumed {
            cx.task.cancel();
            pending = None;
            content = None;
            selected = cx.selected.cloned();
            query = cx.query.into();
            preview_visible = cx.preview_visible;
            if let Some(item) = cx.selected.filter(|_| cx.preview_visible)
                && item.query() == cx.source_query
                && item.location(cx.editor).is_none()
                && let Ok(request) = item.resolve(cx.editor)
            {
                let cancel = cx.task.restart();
                let redraw = cx.editor.redraw().on_drop();
                let (tx, rx) = oneshot::channel();
                pending = Some(rx);
                tokio::spawn(async move {
                    let _redraw = redraw;
                    if let Some(result) = event::cancelable_future(request.run(), cancel).await {
                        let _ = tx.send(result);
                    }
                });
            }
        }
        if let Some(rx) = &mut pending {
            match rx.try_recv() {
                Ok(Ok(Some(update))) => {
                    update.publish(cx.editor);
                    pending = None;
                }
                Ok(Err(error)) => {
                    log::info!("Workspace symbol preview unavailable: {error}");
                    pending = None;
                }
                Ok(Ok(None)) | Err(oneshot::error::TryRecvError::Closed) => {
                    pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if cx.preview_visible
            && let Some(item) = cx.selected
            && let Some(location) = item.location(cx.editor)
            && let Some(server) = location.server_id
            && let Some(uri) = location
                .uri
                .to_url()
                .ok()
                .and_then(|uri| uri.as_str().parse::<lsp::Uri>().ok())
            && cx.editor.language_server_document(server, &uri).is_none()
            && content.as_ref() != Some(&uri)
        {
            let cancel = cx.task.restart();
            content = Some(uri.clone());
            cx.editor
                .preload_language_server_document(server, uri, cancel);
        }
        replacement
    }
}

/// Resolve every matched target before replacing the quicklist. Content loads are
/// background operations, so exporting a cold virtual result never changes focus.
fn export_quicklist(
    editor: &mut Editor,
    updates: Vec<WorkspaceSymbolResolution>,
    cancel: event::TaskHandle,
) {
    let Some(items) = updates
        .into_iter()
        .map(|update| update.publish(editor))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let mut entries: Vec<futures_util::future::BoxFuture<'static, anyhow::Result<QuicklistEntry>>> =
        Vec::with_capacity(items.len());
    for item in &items {
        let Some(location) = item.location(editor).cloned() else {
            return;
        };
        if let Some(entry) = location_to_quicklist_entry(editor, &location) {
            entries.push(futures_util::future::ready(Ok(entry)).boxed());
            continue;
        }
        let Some((server, uri)) = location.server_id.zip(
            location
                .uri
                .to_url()
                .ok()
                .and_then(|uri| uri.as_str().parse::<lsp::Uri>().ok()),
        ) else {
            editor.set_error(|| "Workspace symbol has no usable document location");
            return;
        };
        let (sender, receiver) = oneshot::channel();
        editor.load_language_server_document(server, uri, cancel.clone(), move |_, result| {
            let _ = sender.send(result);
        });
        entries.push(
            async move {
                let document = receiver
                    .await
                    .map_err(|_| anyhow::anyhow!("Symbol document loader stopped"))??;
                Ok(QuicklistEntry {
                    target: QuicklistTarget::Document(document),
                    position: QuicklistPosition::LspRange {
                        range: location.range,
                        offset_encoding: location.offset_encoding,
                    },
                })
            }
            .boxed(),
        );
    }
    let callbacks = editor.callback_sender().clone();
    tokio::spawn(async move {
        let Some(result) =
            event::cancelable_future(futures_util::future::try_join_all(entries), &cancel).await
        else {
            return;
        };
        callbacks
            .send(move |editor| {
                if cancel.is_canceled() || items.iter().any(|item| !item.is_current(editor)) {
                    return;
                }
                match result {
                    Ok(entries) => {
                        if entries.iter().any(|entry| {
                            matches!(entry.target,
                            QuicklistTarget::Document(id) if editor.document(id).is_none())
                        }) {
                            editor.set_error(|| "A workspace symbol document was closed");
                            return;
                        }
                        let count = entries.len();
                        editor.replace_quicklist(entries);
                        editor.set_status(format!("Quicklist populated with {count} entries"));
                    }
                    Err(error)
                        if error
                            .is::<view::features::virtual_documents::ContentRequestCanceled>() => {}
                    Err(error) => {
                        editor.set_error(|| format!("Cannot export workspace symbols: {error}"))
                    }
                }
            })
            .await;
    });
}

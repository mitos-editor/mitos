//! Search picker bound to an open document, independently of its filesystem path.

use std::{
    ops::{ControlFlow, Range},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::Context as _;
use editor_core::Selection;
use futures_util::FutureExt;
use stdx::rope::RopeExt as _;
use view::{
    align_view,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, DocumentId, Editor,
};

use crate::{
    commands::context::Context,
    ui::{self, overlay::overlaid, Picker, PickerColumn},
};

#[derive(Clone, PartialEq)]
struct BufferMatch {
    range: Range<usize>,
    lines: (usize, usize),
    start_col: usize,
    contents: Arc<str>,
    version: i32,
}

struct BufferSearch {
    document: DocumentId,
    case_sensitive: AtomicBool,
}

fn query_buffer_search(
    query: &str,
    editor: &mut Editor,
    config: Arc<BufferSearch>,
    injector: &ui::picker::Injector<BufferMatch, BufferSearch>,
) -> futures_util::future::BoxFuture<'static, anyhow::Result<()>> {
    let request = (|| {
        if query.is_empty() {
            return Ok(None);
        }
        let smart_case = !config.case_sensitive.load(Ordering::Relaxed);
        let search = search::Search::new(query, smart_case)?;
        let doc = editor
            .document(config.document)
            .context("Search buffer was closed")?;
        Ok::<_, anyhow::Error>(Some((search, doc.text().clone(), doc.version())))
    })();
    editor.clear_status();
    if let Err(error) = &request {
        editor.set_error(|| error.to_string());
    }
    let injector = injector.clone();
    async move {
        let Some((search, text, version)) = request? else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || {
            let mut last_lines = None;
            let mut contents = Arc::<str>::from("");
            search.run_document_matches(
                &text,
                || injector.is_shutdown(),
                |range| {
                    let lines = (
                        text.char_to_line_idx(range.start),
                        text.char_to_line_idx(range.end),
                    );
                    let start_col = range.start - text.line_to_char_idx(lines.0);
                    // Consecutive matches with the same preview lines share their text.
                    if last_lines != Some(lines) {
                        let start = text.line_to_byte_idx(lines.0, stdx::rope::LINE_TYPE);
                        let end = text.line_to_byte_idx(lines.1 + 1, stdx::rope::LINE_TYPE);
                        contents = text
                            .slice(start..end)
                            .to_string()
                            .trim_end_matches(['\r', '\n'])
                            .replace(['\r', '\n'], " ")
                            .into();
                        last_lines = Some(lines);
                    }
                    let item = BufferMatch {
                        range,
                        lines,
                        start_col,
                        contents: contents.clone(),
                        version,
                    };
                    match injector.push(item) {
                        Ok(()) => ControlFlow::Continue(()),
                        Err(_) => ControlFlow::Break(()),
                    }
                },
            );
        })
        .await?;
        Ok(())
    }
    .boxed()
}

pub(in crate::commands) fn search_picker_in_current_buffer(cx: &mut Context) {
    let doc = doc!(cx.editor);
    let document = doc.id();
    let mut version = Some(doc.version());
    let reg = cx.register.unwrap_or('/');
    cx.editor.registers.last_search_register = reg;
    let case_sensitive = !cx.editor.config().search.smart_case;
    let config = BufferSearch {
        document,
        case_sensitive: AtomicBool::new(case_sensitive),
    };
    let columns = [
        PickerColumn::new("line", |item: &BufferMatch, _| {
            (item.lines.0 + 1).to_string().into()
        }),
        PickerColumn::new("column", |item: &BufferMatch, _| {
            (item.start_col + 1).to_string().into()
        }),
        PickerColumn::new("contents", |item: &BufferMatch, _| {
            item.contents.as_ref().into()
        })
        .without_filtering(),
    ];
    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        2,
        [],
        config,
        move |cx, item, action| {
            let Some(doc) = cx.editor.document(document) else {
                cx.editor.set_error(|| "Search buffer was closed");
                return;
            };
            if doc.version() != item.version {
                cx.editor
                    .set_error(|| "Search buffer changed; refresh the results");
                return;
            }
            let selection = Selection::single(item.range.start, item.range.end)
                .ensure_invariants(doc.text().slice(..));
            cx.editor.switch(document, action);
            let view = view_mut!(cx.editor);
            let doc = doc_mut!(cx.editor, &document);
            doc.set_selection(view.id, selection);
            if action.align_view(view, document) {
                align_view(doc, view, Align::Center);
            }
        },
    )
    .with_title("Buffer search")
    .with_preview(move |editor, item| {
        let doc = editor.document(document)?;
        (doc.version() == item.version).then_some((document.into(), Some(item.lines)))
    })
    .with_quicklist(move |editor, item| {
        let doc = editor.document(document)?;
        if doc.version() != item.version {
            return None;
        }
        Some(QuicklistEntry {
            target: doc
                .path()
                .map_or(QuicklistTarget::Document(document), |path| {
                    QuicklistTarget::Path(path.to_owned())
                }),
            position: QuicklistPosition::LineColRange {
                start_line: item.lines.0,
                start_col: item.start_col,
                end_line: item.lines.1,
                end_col: item.range.end - doc.text().line_to_char_idx(item.lines.1),
            },
        })
    })
    .with_case_sensitive_toggle(case_sensitive, |config, enabled| {
        config.case_sensitive.store(enabled, Ordering::Relaxed);
    })
    .with_history_register(Some(reg))
    .with_dynamic_query(query_buffer_search, Some(100), cx.jobs)
    .with_query_updates(move |editor, _| {
        let current = editor.document(document).map(|doc| doc.version());
        let changed = current != version;
        version = current;
        changed
    })
    .with_item_key(|item| item.range.start);
    cx.push_layer(Box::new(overlaid(picker)));
}

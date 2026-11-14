//! Search picker bound to an open document, independently of its filesystem path.

use std::{
    ops::ControlFlow,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{ensure, Context as _};
use editor_core::{line_ending::normalize_line_endings, Rope, Transaction};
use futures_util::FutureExt;
use stdx::rope::RopeExt as _;
use view::{
    align_view,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, DocumentId, Editor,
};

use super::selection_for_global_search_match;
use crate::{
    commands::context::Context,
    ui::{self, overlay::overlaid, picker::ReplacementStep, Picker, PickerColumn},
};

#[derive(Clone, PartialEq)]
struct BufferMatch {
    matched: search::Match,
    contents: String,
    version: i32,
}

struct BufferSearch {
    document: DocumentId,
    case_sensitive: AtomicBool,
}

impl BufferSearch {
    fn search(&self, query: &str) -> anyhow::Result<search::Search> {
        Ok(search::Search::new(
            query,
            !self.case_sensitive.load(Ordering::Relaxed),
        )?)
    }
}

fn buffer_match(matched: search::Match, text: &Rope, version: i32) -> BufferMatch {
    let start = text.line_to_char_idx(matched.line_start);
    let end = text.line_to_char_idx(matched.line_end + 1);
    let contents = text
        .slice(start..end)
        .to_string()
        .trim_end_matches(['\r', '\n'])
        .replace(['\r', '\n'], " ");
    BufferMatch {
        matched,
        contents,
        version,
    }
}

fn matches(search: &search::Search, text: &Rope, version: i32) -> anyhow::Result<Vec<BufferMatch>> {
    Ok(search
        .search_document(Path::new(""), text)?
        .into_iter()
        .map(|matched| buffer_match(matched, text, version))
        .collect())
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
        let search = config.search(query)?;
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
            if injector.is_shutdown() {
                return Ok(());
            }
            search.run_document(
                Path::new(""),
                &text,
                || injector.is_shutdown(),
                |matched| match injector.push(buffer_match(matched, &text, version)) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                },
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
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
            (item.matched.line_start + 1).to_string().into()
        }),
        PickerColumn::new("contents", |item: &BufferMatch, _| {
            item.contents.as_str().into()
        })
        .without_filtering(),
    ];
    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        1,
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
            let matched = &item.matched;
            let Some(selection) = selection_for_global_search_match(
                doc.text().slice(..),
                matched.line_start,
                matched.match_start_col,
                matched.line_end,
                matched.match_end_col,
            ) else {
                return;
            };
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
        editor.document(document)?;
        Some((
            document.into(),
            Some((item.matched.line_start, item.matched.line_end)),
        ))
    })
    .with_quicklist(move |editor, item| {
        let doc = editor.document(document)?;
        if doc.version() != item.version {
            return None;
        }
        let matched = &item.matched;
        Some(QuicklistEntry {
            target: doc
                .path()
                .map_or(QuicklistTarget::Document(document), |path| {
                    QuicklistTarget::Path(path.to_owned())
                }),
            position: QuicklistPosition::LineColRange {
                start_line: matched.line_start,
                start_col: matched.match_start_col,
                end_line: matched.line_end,
                end_col: matched.match_end_col,
            },
        })
    })
    .with_replacement(move |editor, request, config| {
        let doc = editor
            .document(document)
            .context("Search buffer was closed")?;
        let version = doc.version();
        ensure!(
            request.matches.iter().all(|item| item.version == version),
            "Search buffer changed; refresh the results"
        );
        let range = if request.replace_all {
            None
        } else {
            let item = request.matches.first().context("No selected match")?;
            let matched = &item.matched;
            let selection = selection_for_global_search_match(
                doc.text().slice(..),
                matched.line_start,
                matched.match_start_col,
                matched.line_end,
                matched.match_end_col,
            )
            .context("The selected match no longer exists")?;
            let range = selection.primary();
            Some(range.from()..range.to())
        };
        let replace_all = request.replace_all;
        let mut text = doc.text().clone();
        let line_ending = doc.line_ending;
        let search = config.search(request.query)?;
        let replacement = request.replacement.to_owned();
        let cancel = request.cancel;
        let owner = editor.callback_sender().clone();
        Ok(async move {
            let (transaction, count, mut options, cancel) =
                tokio::task::spawn_blocking(move || {
                    ensure!(!cancel.is_canceled(), "Replacement canceled");
                    let mut changes =
                        search.replacement_changes(text.slice(..), &replacement, range)?;
                    ensure!(
                        replace_all || !changes.is_empty(),
                        "The selected text no longer matches"
                    );
                    for (_, _, inserted) in &mut changes {
                        if let Some(text) = inserted {
                            *text = normalize_line_endings(text, line_ending).as_ref().into();
                        }
                    }
                    let count = changes.len();
                    let transaction = Transaction::change(&text, changes.into_iter());
                    transaction.apply(&mut text);
                    let options = matches(&search, &text, version)?;
                    Ok::<_, anyhow::Error>((transaction, count, options, cancel))
                })
                .await??;
            Ok(ReplacementStep::Apply(Box::new(move |editor| {
                ensure!(
                    !cancel.is_canceled() && owner.same_destination(editor.callback_sender()),
                    "Replacement canceled or editor changed"
                );
                editor.apply_document_changes(document, version, transaction.changes())?;
                let version = editor.document(document).unwrap().version();
                for item in &mut options {
                    item.version = version;
                }
                Ok(async move {
                    Ok(ReplacementStep::Complete {
                        count,
                        options,
                        error: None,
                    })
                }
                .boxed())
            })))
        }
        .boxed())
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
    .with_item_key(|item| (item.matched.line_start, item.matched.match_start_col));
    cx.push_layer(Box::new(overlaid(picker)));
}

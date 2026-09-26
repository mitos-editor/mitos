//! Range-formatting requests and formatting-result callbacks.

use super::context::Context;
use crate::job::{self, Callback};
use editor_core::{indent::IndentStyle, syntax::config::LanguageServerFeature, Transaction};
use std::{future::Future, path::PathBuf};
use view::{document::FormatterError, DocumentId, ViewId};

pub(super) fn format_selections(cx: &mut Context) {
    use lsp_client::{lsp, util::range_to_lsp_range};

    let (view, doc) = current!(cx.editor);
    let view_id = view.id;

    // via lsp if available
    // TODO: else via tree-sitter indentation calculations

    if doc.selection(view_id).len() != 1 {
        cx.editor
            .set_error(|| "format_selections only supports a single selection for now");
        return;
    }

    // TODO extra LanguageServerFeature::FormatSelections?
    // maybe such that LanguageServerFeature::Format contains it as well
    let Some(language_server) = doc
        .language_servers_with_feature(LanguageServerFeature::Format)
        .find(|ls| {
            matches!(
                ls.capabilities().document_range_formatting_provider,
                Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
            )
        })
    else {
        cx.editor
            .set_error(|| "No configured language server supports range formatting");
        return;
    };

    let offset_encoding = language_server.offset_encoding();
    let ranges: Vec<lsp::Range> = doc
        .selection(view_id)
        .iter()
        .map(|range| range_to_lsp_range(doc.text(), *range, offset_encoding))
        .collect();

    // TODO: handle fails
    // TODO: concurrent map over all ranges

    let range = ranges[0];

    let future = language_server
        .text_document_range_formatting(
            doc.identifier(),
            range,
            lsp::FormattingOptions {
                tab_size: doc.tab_width() as u32,
                insert_spaces: matches!(doc.indent_style, IndentStyle::Spaces(_)),
                ..Default::default()
            },
            None,
        )
        .unwrap();

    let text = doc.text().clone();
    let doc_id = doc.id();
    let doc_version = doc.version();

    tokio::spawn(async move {
        match future.await {
            Ok(Some(res)) => {
                let transaction =
                    lsp_client::util::generate_transaction_from_edits(&text, res, offset_encoding);
                job::dispatch(move |editor, _compositor| {
                    let Some(doc) = editor.document_mut(doc_id) else {
                        return;
                    };
                    // Updating a desynced document causes problems with applying the transaction
                    if doc.version() != doc_version {
                        return;
                    }
                    doc.apply(&transaction, view_id);
                })
                .await
            }
            Err(err) => log::error!("format sections failed: {err}"),
            Ok(None) => (),
        }
    });
}

// Creates an LspCallback that waits for formatting changes to be computed. When they're done,
// it applies them, but only if the doc hasn't changed.
//
// TODO: provide some way to cancel this, probably as part of a more general job cancellation
// scheme
pub(super) async fn make_format_callback(
    doc_id: DocumentId,
    doc_version: i32,
    view_id: ViewId,
    format: impl Future<Output = Result<Transaction, FormatterError>> + Send + 'static,
    write: Option<(Option<PathBuf>, bool)>,
) -> anyhow::Result<job::Callback> {
    let format = format.await;

    let call: job::Callback = Callback::Editor(Box::new(move |editor| {
        if !editor.documents.contains_key(&doc_id) || !editor.tree.contains(view_id) {
            return;
        }

        let scrolloff = editor.config().scrolloff;
        let doc = doc_mut!(editor, &doc_id);
        let view = view_mut!(editor, view_id);

        match format {
            Ok(format) => {
                if doc.version() == doc_version {
                    doc.apply(&format, view.id);
                    doc.append_changes_to_history(view);
                    doc.detect_indent_and_line_ending();
                    view.ensure_cursor_in_view(doc, scrolloff);
                } else {
                    log::info!("discarded formatting changes because the document changed");
                }
            }
            Err(err) => {
                if write.is_none() {
                    editor.set_error(|| err.to_string());
                    return;
                }
                log::info!("failed to format '{}': {err}", doc.display_name());
            }
        }

        if let Some((path, force)) = write {
            let id = doc.id();
            if let Err(err) = editor.save(id, path, force) {
                editor.set_error(|| format!("Error saving: {}", err));
            }
        }
    }));

    Ok(call)
}

pub(super) mod typed {
    //! Typable formatting commands.

    use crate::{commands::formatting::make_format_callback, compositor, ui::PromptEvent};
    use ::command_line::Args;
    use anyhow::Context as _;

    #[cold]
    pub(in crate::commands) fn format(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let (view, doc) = current_ref!(cx.editor);
        let format = doc.format(cx.editor).context(
            "A formatter isn't available, and no language server provides formatting capabilities",
        )?;
        let callback = make_format_callback(doc.id(), doc.version(), view.id, format, None);
        cx.jobs.callback(callback);

        Ok(())
    }
}

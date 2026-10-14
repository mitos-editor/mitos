//! Range-formatting requests and formatting-result callbacks.

use super::context::Context;
use crate::job::{self, Callback};
use editor_core::{indent::IndentStyle, syntax::config::LanguageServerFeature, Transaction};
use std::future::Future;
use view::{document::FormatterError, DocumentId, ViewId};

pub(super) fn format_selections(cx: &mut Context) {
    use lsp_client::{lsp, util::range_to_lsp_range};

    let (view, doc) = current!(cx.editor);
    let view_id = view.id;
    let binding_revision = view.binding_revision();

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

    cx.jobs.callback(async move {
        let response = future.await?;
        Ok(Callback::Editor(Box::new(move |editor| {
            let Some(edits) = response else {
                return;
            };
            if editor.tree.try_get(view_id).is_none_or(|view| {
                view.doc != doc_id || view.binding_revision() != binding_revision
            }) {
                return;
            }
            let Some(doc) = editor.document_mut(doc_id) else {
                return;
            };
            if doc.version() != doc_version {
                return;
            }
            let transaction =
                lsp_client::util::generate_transaction_from_edits(&text, edits, offset_encoding);
            doc.apply(&transaction, view_id);
        })))
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
    binding_revision: u64,
    format: impl Future<Output = Result<Transaction, FormatterError>> + Send + 'static,
) -> anyhow::Result<job::Callback> {
    let format = format.await;

    let call: job::Callback = Callback::Editor(Box::new(move |editor| {
        if !editor.documents.contains_key(&doc_id) {
            return;
        }
        let current_binding = editor
            .tree
            .try_get(view_id)
            .is_some_and(|view| view.doc == doc_id && view.binding_revision() == binding_revision);
        if current_binding {
            let scrolloff = editor.config().scroll.padding;
            let doc = doc_mut!(editor, &doc_id);
            let view = view_mut!(editor, view_id);

            match format {
                Ok(format) if doc.version() == doc_version => {
                    doc.apply(&format, view.id);
                    doc.append_changes_to_history(view);
                    doc.detect_indent_and_line_ending();
                    view.ensure_cursor_in_view(doc, scrolloff);
                }
                Ok(_) => {
                    log::info!("discarded formatting changes because the document changed");
                }
                Err(err) => {
                    editor.set_error(|| err.to_string());
                }
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
        let callback = make_format_callback(
            doc.id(),
            doc.version(),
            view.id,
            view.binding_revision(),
            format,
        );
        cx.jobs.callback(callback);

        Ok(())
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;
    use crate::{application::Application, args::Args, config::Config};
    use view::editor::Action;

    fn application() -> anyhow::Result<Application> {
        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.files.watch.enable = false;
        config.editor.files.reload.enable = false;
        config.editor.files.reload.poll.enable = false;
        let mut app = Application::new(
            Args::default(),
            config,
            editor_core::syntax::Loader::new(
                toml::from_str("language = []")?,
                loader::syntax::Resources::default(),
            )?,
            loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        )?;
        let (view, doc) = current!(app.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some("original\n".into()))].into_iter(),
        );
        assert!(doc.apply(&transaction, view.id));
        doc.append_changes_to_history(view);
        Ok(app)
    }

    async fn formatting_callback(
        app: &Application,
    ) -> anyhow::Result<(DocumentId, ViewId, Callback)> {
        let (view, doc) = current_ref!(app.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some("formatted\n".into()))].into_iter(),
        );
        let callback = make_format_callback(
            doc.id(),
            doc.version(),
            view.id,
            view.binding_revision(),
            async { Ok(transaction) },
        )
        .await?;
        Ok((doc.id(), view.id, callback))
    }

    fn apply(app: &mut Application, callback: Callback) {
        let Callback::Editor(callback) = callback else {
            panic!("expected editor callback")
        };
        callback(&mut app.editor);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn formatting_applies_when_the_original_view_binding_is_current() -> anyhow::Result<()> {
        let mut app = application()?;
        let (doc, _, callback) = formatting_callback(&app).await?;
        apply(&mut app, callback);
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "formatted\n"
        );
        assert!(app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn formatting_is_discarded_after_switching_documents() -> anyhow::Result<()> {
        let mut app = application()?;
        let (doc, _, callback) = formatting_callback(&app).await?;
        let other = app.editor.new_file(Action::Replace);
        let other_text = app.editor.document(other).unwrap().text().clone();
        apply(&mut app, callback);
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "original\n"
        );
        assert_eq!(app.editor.document(other).unwrap().text(), &other_text);
        assert!(app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn switching_away_and_back_does_not_revive_a_formatting_callback() -> anyhow::Result<()> {
        let mut app = application()?;
        let (doc, _, callback) = formatting_callback(&app).await?;
        app.editor.new_file(Action::Replace);
        app.editor.switch(doc, Action::Replace);
        apply(&mut app, callback);
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "original\n"
        );
        assert!(app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_the_origin_view_does_not_apply_formatting_to_another_split(
    ) -> anyhow::Result<()> {
        let mut app = application()?;
        let (doc, view, callback) = formatting_callback(&app).await?;
        app.editor.switch(doc, Action::VerticalSplit);
        app.editor.close(view);
        apply(&mut app, callback);
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "original\n"
        );
        assert!(app.close().await.is_empty());
        Ok(())
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn save_writes_current_text_after_the_origin_view_is_rebound() -> anyhow::Result<()> {
        let mut app = application()?;
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("saved.txt");
        let (view, document) = current_ref!(app.editor);
        let (doc, view) = (document.id(), view.id);
        let request = view::save::PreparedSave {
            doc_id: doc,
            view_id: view,
            path: Some(target.clone()),
            force: false,
            auto_format: true,
            code_actions: false,
        };
        let view::save::SaveProgress::Pending(task) = view::save::submit(&mut app.editor, request)?
        else {
            anyhow::bail!("expected deferred save stage")
        };
        let update = task.run().await?;
        app.editor.new_file(Action::Replace);
        assert!(matches!(
            update.apply(&mut app.editor)?,
            view::save::SaveProgress::Queued
        ));
        app.editor.flush_writes().await?;
        assert_eq!(std::fs::read_to_string(target)?, "original\n");
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "original\n"
        );
        assert!(app.close().await.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn formatting_is_discarded_after_the_document_changes() -> anyhow::Result<()> {
        let mut app = application()?;
        let (doc, _, callback) = formatting_callback(&app).await?;
        let (view, document) = current!(app.editor);
        let edit = Transaction::change(document.text(), [(0, 0, Some("new ".into()))].into_iter());
        assert!(document.apply(&edit, view.id));
        apply(&mut app, callback);
        assert_eq!(
            app.editor.document(doc).unwrap().text().to_string(),
            "new original\n"
        );
        assert!(app.close().await.is_empty());
        Ok(())
    }
}

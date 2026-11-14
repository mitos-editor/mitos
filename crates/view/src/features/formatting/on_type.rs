//! Server-triggered formatting stays in the typing undo group and never rebases stale edits.

use std::time::Duration;

use editor_core::indent::IndentStyle;
use event::cancelable_future;
use futures_util::FutureExt as _;
use language_config::LanguageServerFeature;
use lsp_client::lsp;

use super::{prepare_target, FormatKind, FormattingTask};
use crate::{document::Mode, Editor};

/// Called only after successful user input, never after programmatic edits or paste.
pub(crate) fn request(editor: &mut Editor, character: Option<char>) {
    let (view, doc) = current!(editor);
    let view_id = view.id;
    doc.cancel_view_request(view_id, |state| &mut state.on_type_formatting);
    let Some(character) = character else { return };
    if !editor.config().lsp.enable
        || !editor.config().format.on_type
        || editor.mode() != Mode::Insert
    {
        return;
    }
    let Ok((mut target, doc)) = prepare_target(editor, view_id, FormatKind::OnType) else {
        return;
    };
    if doc.is_content_readonly() || doc.selection(view_id).len() != 1 {
        return;
    }
    let ch = character.to_string();
    let Some(server) = doc
        .language_config()
        .into_iter()
        .flat_map(|config| &config.language_servers)
        .filter(|config| config.has_feature(LanguageServerFeature::Format))
        .filter_map(|config| doc.language_servers.get(&config.name))
        .filter(|attachment| attachment.opened)
        .map(|attachment| &attachment.client)
        .find(|server| {
            server
                .capabilities()
                .document_on_type_formatting_provider
                .as_ref()
                .is_some_and(|provider| {
                    provider.first_trigger_character == ch
                        || provider
                            .more_trigger_character
                            .as_ref()
                            .is_some_and(|chars| chars.contains(&ch))
                })
        })
    else {
        return;
    };
    let encoding = server.offset_encoding();
    let Some(request) = server.text_document_on_type_formatting(
        doc.identifier(),
        doc.position(view_id, encoding),
        ch,
        lsp::FormattingOptions {
            tab_size: doc.indent_width() as u32,
            insert_spaces: matches!(doc.indent_style, IndentStyle::Spaces(_)),
            ..Default::default()
        },
    ) else {
        return;
    };
    target.snapshot.server = Some(server.id());
    target.selection = Some(doc.selection(view_id).clone());
    let text = doc.text().clone();
    let doc_id = doc.id();
    let task = FormattingTask {
        target,
        future: async move {
            let edits = tokio::time::timeout(Duration::from_millis(1000), request).await??;
            edits
                .map(|edits| {
                    crate::language_servers::edits::generate_transaction_from_edits(
                        &text, edits, encoding,
                    )
                })
                .transpose()
                .map_err(Into::into)
        }
        .boxed(),
    };
    let cancel = editor
        .documents
        .get_mut(&doc_id)
        .unwrap()
        .view_state_mut(view_id)
        .on_type_formatting
        .get_or_insert_default()
        .restart();
    let callbacks = editor.callback_sender().clone();
    tokio::spawn(async move {
        let Some(update) = cancelable_future(task.run(), &cancel).await else {
            return;
        };
        callbacks
            .send(move |editor| {
                if cancel.is_canceled() {
                    return;
                }
                match update.apply(editor) {
                    Ok(true) => editor.redraw().request(),
                    Ok(false) => (),
                    Err(error) => log::debug!("on-type formatting failed: {error}"),
                }
            })
            .await;
    });
}

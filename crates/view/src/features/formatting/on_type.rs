//! Server-triggered formatting stays in the typing undo group and never rebases stale edits.

use std::time::Duration;

use event::cancelable_future;
use futures_util::FutureExt as _;
use language_config::LanguageServerFeature;

use super::{formatting_options, prepare_target, FormatKind, FormattingTask};
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
        .language_servers_matching_config(Some(LanguageServerFeature::Format))
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
        formatting_options(doc),
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

fn cancel(editor: &mut Editor) {
    for doc in editor.documents_mut() {
        for state in doc.view_states.values_mut() {
            if let Some(request) = &mut state.on_type_formatting {
                request.cancel();
            }
        }
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    use crate::events::{ConfigDidChange, DocumentFocusLost, ModeDidChange, SelectionDidChange};
    use event::register_hook;

    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event.doc.cancel_view_request(event.view, |state| &mut state.on_type_formatting);
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.cancel_view_request(event.view, |state| &mut state.on_type_formatting);
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut ModeDidChange<'_>| {
        cancel(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable || !event.new.format.on_type {
            cancel(event.editor);
        }
        Ok(())
    });
}

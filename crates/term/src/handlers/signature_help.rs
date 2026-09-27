use std::sync::Arc;

use event::register_hook;
use lsp_client::lsp::{self, SignatureInformation};
use view::{
    document::Mode,
    handlers::signature_help::{self, SignatureHelpChange, SignatureHelpUpdate},
    Editor,
};

use crate::{
    commands::Open,
    compositor::Compositor,
    events::{OnModeSwitch, PostInsertChar},
    ui::{
        self,
        lsp::signature_help::{Signature, SignatureHelp},
        Popup,
    },
};

fn active_param_range(
    signature: &SignatureInformation,
    response_active_parameter: Option<u32>,
) -> Option<(usize, usize)> {
    let param_idx = signature
        .active_parameter
        .or(response_active_parameter)
        .unwrap_or(0) as usize;
    let param = signature.parameters.as_ref()?.get(param_idx)?;
    match &param.label {
        lsp::ParameterLabel::Simple(string) => {
            let start = signature.label.find(string.as_str())?;
            Some((start, start + string.len()))
        }
        lsp::ParameterLabel::LabelOffsets([start, end]) => {
            // LS sends offsets based on utf-16 based string representation
            // but highlighting in mitos is done using byte offset.
            use editor_core::str_utils::char_to_byte_idx;
            let from = char_to_byte_idx(&signature.label, *start as usize);
            let to = char_to_byte_idx(&signature.label, *end as usize);
            Some((from, to))
        }
    }
}

pub fn show_signature_help(
    editor: &mut Editor,
    compositor: &mut Compositor,
    update: SignatureHelpUpdate,
) {
    let response = match update.resolve(editor) {
        Some(SignatureHelpChange::Show(response)) => response,
        Some(SignatureHelpChange::Hide) => {
            compositor.remove(SignatureHelp::ID);
            return;
        }
        None => return,
    };
    let config = &editor.config();
    let doc = doc!(editor);
    let language = doc.language_name().unwrap_or("");

    let signatures: Vec<Signature> = response
        .signatures
        .into_iter()
        .map(|s| {
            let active_param_range = active_param_range(&s, response.active_parameter);

            let signature_doc = if config.lsp.display_signature_help_docs {
                s.documentation.map(|doc| match doc {
                    lsp::Documentation::String(s) => s,
                    lsp::Documentation::MarkupContent(markup) => markup.value,
                })
            } else {
                None
            };

            Signature {
                signature: s.label,
                signature_doc,
                active_param_range,
            }
        })
        .collect();

    let old_popup = compositor.find_id::<Popup<SignatureHelp>>(SignatureHelp::ID);
    let lsp_signature = response.active_signature.map(|s| s as usize);

    // take the new suggested lsp signature if changed
    // otherwise take the old signature if possible
    // otherwise the last one (in case there is less signatures than before)
    let active_signature = old_popup
        .as_ref()
        .map(|popup| {
            let old_lsp_sig = popup.contents().lsp_signature();
            let old_sig = popup
                .contents()
                .active_signature()
                .min(signatures.len() - 1);

            if old_lsp_sig != lsp_signature {
                lsp_signature.unwrap_or(old_sig)
            } else {
                old_sig
            }
        })
        .unwrap_or(lsp_signature.unwrap_or_default());

    let contents = SignatureHelp::new(
        language.to_string(),
        Arc::clone(&editor.syn_loader),
        active_signature,
        lsp_signature,
        signatures,
    );

    let mut popup = Popup::new(SignatureHelp::ID, contents)
        .position(old_popup.and_then(|p| p.get_position()))
        .position_bias(Open::Above)
        .ignore_escape_key(true);

    // Don't create a popup if it intersects the auto-complete menu.
    let size = compositor.size();
    if compositor
        .find::<ui::EditorView>()
        .unwrap()
        .completion
        .as_mut()
        .map(|completion| completion.area(size, editor))
        .filter(|area| area.intersects(popup.area(size, editor)))
        .is_some()
    {
        return;
    }

    compositor.replace_or_push(SignatureHelp::ID, popup);
}

pub(super) fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
            signature_help::mode_changed(event.cx.editor, event.old_mode);
            if event.old_mode == Mode::Insert {
                event.cx.callback.push(Box::new(|compositor, _| {
                    compositor.remove(SignatureHelp::ID);
                }));
            }
            Ok(())
        });
        register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
            signature_help::post_insert_char(event.cx.editor);
            Ok(())
        });
    });
}

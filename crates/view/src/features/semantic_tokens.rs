//! Snapshot-bound semantic highlighting. Servers refine syntax; they do not
//! replace it. A document uses the first configured provider, deterministically.

use std::ops::Range;

use editor_core::Rope;
use event::{cancelable_future, register_hook, TaskController};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};

use crate::{
    callbacks::EditorCallbackSender,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidOpen, DocumentLanguageDidChange,
        LanguageServerExited, LanguageServerInitialized,
    },
    features::{debounce::debounce_documents, request::DocumentRequest},
    Document, DocumentId, Editor,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    /// Character offsets in the exact document snapshot used by the server.
    pub range: Range<usize>,
    pub scope: String,
}

#[derive(Default)]
pub(crate) struct SemanticTokens {
    cache: Vec<SemanticToken>,
    revision: u64,
    request: TaskController,
    pub(crate) handler: Option<SemanticTokensHandler>,
}

impl SemanticTokens {
    pub(crate) fn invalidate(&mut self) {
        self.request.cancel();
        self.cache.clear();
        self.revision = self.revision.wrapping_add(1);
    }

    fn publish(&mut self, tokens: Vec<SemanticToken>) {
        self.cache = tokens;
        self.revision = self.revision.wrapping_add(1);
    }
}

impl Document {
    pub fn semantic_tokens_revision(&self) -> u64 {
        self.semantic_tokens.revision
    }

    pub fn semantic_tokens(&self) -> &[SemanticToken] {
        &self.semantic_tokens.cache
    }
}

#[derive(Clone)]
pub struct SemanticTokensHandler {
    callbacks: EditorCallbackSender,
    events: tokio::sync::mpsc::Sender<DocumentId>,
}

impl SemanticTokensHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let events =
            debounce_documents(callbacks.clone(), |editor, doc| request(editor, doc, None));
        Self { callbacks, events }
    }
}

fn request(editor: &mut Editor, doc_id: DocumentId, exited: Option<LanguageServerId>) {
    if !editor.config().lsp.enable || !editor.config().lsp.semantic_tokens.enable {
        return;
    }
    let callbacks = editor.handlers.semantic_tokens.callbacks.clone();
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    let cancel = doc.semantic_tokens.request.restart();
    let Some(server) = doc
        .language_servers_with_feature(LanguageServerFeature::SemanticTokens)
        .find(|server| Some(server.id()) != exited)
    else {
        doc.semantic_tokens.invalidate();
        return;
    };
    let server_id = server.id();
    let encoding = server.offset_encoding();
    let legend = server.semantic_tokens_options().unwrap().legend.clone();
    let text = doc.text().clone();
    let end = lsp_client::util::pos_to_lsp_pos(&text, text.len_chars(), encoding);
    let Some(future) = server.semantic_tokens(doc.identifier(), end) else {
        return;
    };
    let snapshot = DocumentRequest::new(doc, cancel, vec![server_id]);
    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, &snapshot.cancel).await else {
            return;
        };
        let tokens = match result {
            Ok(tokens) => decode(&text, &tokens, &legend, encoding),
            Err(error) => {
                log::error!("semantic tokens request failed: {error}");
                return;
            }
        };
        callbacks
            .send(move |editor| {
                if snapshot.is_current(editor)
                    && editor.config().lsp.enable
                    && editor.config().lsp.semantic_tokens.enable
                {
                    editor
                        .document_mut(doc_id)
                        .unwrap()
                        .semantic_tokens
                        .publish(tokens);
                    editor.redraw().request();
                }
            })
            .await;
    });
}

/// Map supported standard kinds and modifiers into the same vocabulary used by
/// syntax queries. Unknown server extensions are skipped, never guessed as calls.
fn classification(
    token: &lsp::SemanticToken,
    legend: &lsp::SemanticTokensLegend,
) -> Option<String> {
    let kind = match legend.token_types.get(token.token_type as usize)?.as_str() {
        "namespace" => "namespace",
        "type" => "type",
        "class" => "type.class",
        "enum" => "type.enum",
        "interface" => "type.interface",
        "struct" => "type.struct",
        "typeParameter" => "type.parameter",
        "parameter" => "variable.parameter",
        "variable" => "variable",
        "property" | "event" => "variable.other.member",
        "enumMember" => "type.enum.variant",
        "function" => "function",
        "method" => "function.method",
        "macro" => "function.macro",
        "keyword" => "keyword",
        "modifier" => "keyword.storage.modifier",
        "comment" => "comment",
        "string" => "string",
        "number" => "constant.numeric",
        "regexp" => "string.regexp",
        "operator" => "operator",
        "decorator" => "attribute",
        "label" => "label",
        _ => return None,
    };
    let mut scope = kind.to_owned();
    for (index, modifier) in legend
        .token_modifiers
        .iter()
        .take(u32::BITS as usize)
        .enumerate()
    {
        if token.token_modifiers_bitset & (1u32 << index) == 0 {
            continue;
        }
        let modifier = match modifier.as_str() {
            "declaration" => "declaration",
            "definition" => "definition",
            "readonly" => "readonly",
            "static" => "static",
            "deprecated" => "deprecated",
            "abstract" => "abstract",
            "async" => "async",
            "modification" => "modification",
            "documentation" => "documentation",
            "defaultLibrary" => "builtin",
            _ => continue,
        };
        scope.push('.');
        scope.push_str(modifier);
    }
    Some(scope)
}

/// Decode relative, single-line, non-overlapping tokens. Invalid ranges are
/// ignored without shifting later tokens; arithmetic overflow ends the stream.
pub fn decode(
    text: &Rope,
    tokens: &[lsp::SemanticToken],
    legend: &lsp::SemanticTokensLegend,
    encoding: OffsetEncoding,
) -> Vec<SemanticToken> {
    let mut line = 0u32;
    let mut column = 0u32;
    let mut end = 0usize;
    let mut decoded = Vec::new();
    for token in tokens {
        let Some(next_line) = line.checked_add(token.delta_line) else {
            break;
        };
        let Some(next_column) = (if token.delta_line == 0 {
            column.checked_add(token.delta_start)
        } else {
            Some(token.delta_start)
        }) else {
            break;
        };
        line = next_line;
        column = next_column;
        let Some(last_column) = column.checked_add(token.length) else {
            continue;
        };
        if token.length == 0 || line as usize >= text.len_lines(stdx::rope::LINE_TYPE) {
            continue;
        }
        let start_position = lsp::Position::new(line, column);
        let end_position = lsp::Position::new(line, last_column);
        let Some(start) = lsp_client::util::lsp_pos_to_pos(text, start_position, encoding) else {
            continue;
        };
        let Some(stop) = lsp_client::util::lsp_pos_to_pos(text, end_position, encoding) else {
            continue;
        };
        // The general LSP converter clamps out-of-line columns. A token must
        // instead name exact character boundaries, including UTF-16 surrogates.
        if start < end
            || stop <= start
            || lsp_client::util::pos_to_lsp_pos(text, start, encoding) != start_position
            || lsp_client::util::pos_to_lsp_pos(text, stop, encoding) != end_position
        {
            continue;
        }
        let Some(scope) = classification(token, legend) else {
            continue;
        };
        decoded.push(SemanticToken {
            range: start..stop,
            scope,
        });
        end = stop;
    }
    decoded
}

pub(crate) fn refresh_document(editor: &mut Editor, doc: DocumentId) {
    if let Some(doc) = editor.document_mut(doc) {
        doc.semantic_tokens.invalidate();
    }
    request(editor, doc, None);
}

pub fn refresh_for_language_server(editor: &mut Editor, server: LanguageServerId) {
    let docs: Vec<_> = editor
        .documents()
        .filter(|doc| doc.supports_language_server(server))
        .map(|doc| doc.id())
        .collect();
    for doc in docs {
        request(editor, doc, None);
    }
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut DocumentDidOpen<'_>| { request(event.editor, event.doc, None); Ok(()) });
    register_hook!(events => move |event: &mut DocumentLanguageDidChange<'_>| { request(event.editor, event.doc, None); Ok(()) });
    register_hook!(events => move |event: &mut DocumentDidChange<'_>| {
        if let Some(handler) = &event.doc.semantic_tokens.handler { event::send_blocking(&handler.events, event.doc.id()); }
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerInitialized<'_>| { refresh_for_language_server(event.editor, event.server_id); Ok(()) });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        let docs: Vec<_> = event.editor.documents_mut().filter(|doc| doc.supports_language_server(event.server_id)).map(|doc| {
            doc.semantic_tokens.invalidate(); doc.id()
        }).collect();
        for doc in docs { request(event.editor, doc, Some(event.server_id)); }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        if event.old.lsp.enable != event.new.lsp.enable || event.old.lsp.semantic_tokens != event.new.lsp.semantic_tokens {
            let docs: Vec<_> = event.editor.documents_mut().map(|doc| { doc.semantic_tokens.invalidate(); doc.id() }).collect();
            for doc in docs { request(event.editor, doc, None); }
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend() -> lsp::SemanticTokensLegend {
        lsp::SemanticTokensLegend {
            token_types: vec![lsp::SemanticTokenType::METHOD],
            token_modifiers: vec![
                lsp::SemanticTokenModifier::DEFINITION,
                lsp::SemanticTokenModifier::DEPRECATED,
            ],
        }
    }
    fn token(start: u32, length: u32) -> lsp::SemanticToken {
        lsp::SemanticToken {
            delta_line: 0,
            delta_start: start,
            length,
            token_type: 0,
            token_modifiers_bitset: 0,
        }
    }

    #[test]
    fn decoding_handles_utf8_utf16_and_utf32_without_inventing_invocations() {
        let text = Rope::from_str("😀 méthode()");
        for (encoding, start, length) in [
            (OffsetEncoding::Utf8, 5, 8),
            (OffsetEncoding::Utf16, 3, 7),
            (OffsetEncoding::Utf32, 2, 7),
        ] {
            let mut definition = token(start, length);
            definition.token_modifiers_bitset = 3;
            let tokens = decode(&text, &[definition], &legend(), encoding);
            assert_eq!(
                tokens,
                vec![SemanticToken {
                    range: 2..9,
                    scope: "function.method.definition.deprecated".into()
                }]
            );
            assert_eq!(
                classification(&token(0, 1), &legend()).unwrap(),
                "function.method"
            );
        }
    }

    #[test]
    fn invalid_indices_overlaps_and_columns_do_not_corrupt_following_tokens() {
        let text = Rope::from_str("foo bar baz");
        let mut unknown = token(0, 3);
        unknown.token_type = 99;
        let tokens = decode(
            &text,
            &[unknown, token(4, 3), token(0, 2), token(4, 99)],
            &legend(),
            OffsetEncoding::Utf16,
        );
        assert_eq!(
            tokens,
            vec![SemanticToken {
                range: 4..7,
                scope: "function.method".into()
            }]
        );
        assert!(decode(
            &Rope::from_str("😀"),
            &[token(1, 1)],
            &legend(),
            OffsetEncoding::Utf16
        )
        .is_empty());
    }

    #[test]
    fn publications_and_invalidations_change_the_render_cache_revision() {
        let mut state = SemanticTokens::default();
        let initial = state.revision;
        state.publish(vec![SemanticToken {
            range: 0..3,
            scope: "function".into(),
        }]);
        assert_ne!(state.revision, initial);
        let published = state.revision;
        state.invalidate();
        assert_ne!(state.revision, published);
        assert!(state.cache.is_empty());
    }

    #[tokio::test]
    async fn edits_clear_semantics_and_cancel_queued_publications() {
        let mut doc = crate::document::test_document("foo");
        let cancel = doc.semantic_tokens.request.restart();
        doc.semantic_tokens.cache.push(SemanticToken {
            range: 0..3,
            scope: "function".into(),
        });
        doc.semantic_tokens.invalidate();
        assert!(cancel.is_canceled());
        assert!(doc.semantic_tokens().is_empty());
    }
}

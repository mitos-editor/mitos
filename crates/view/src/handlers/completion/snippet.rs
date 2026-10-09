//! Owned static snippets share the native completion session and savepoints.

use std::{borrow::Cow, sync::Arc};

use editor_core::{chars::char_is_word, completion::CompletionProvider, movement, Range};
use event::TaskHandle;

use crate::{document::SavePoint, Editor};

use super::{
    CompletionItem, CompletionItems, CompletionResponse, ResponseContext, StaticSnippetItem,
    Trigger,
};

pub(super) fn completion(
    editor: &Editor,
    trigger: Trigger,
    handle: TaskHandle,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse + use<>> {
    let snippets = editor.plugin_snippets();
    if snippets.is_empty() {
        return None;
    }
    let view = editor.tree.try_get(trigger.view)?;
    let doc = editor.document(trigger.doc)?;
    let language = doc.language_name()?;
    let selection = doc.selection(view.id).clone();
    if selection.len() > 128 {
        return None;
    }
    let rope = doc.text().clone();
    let text = rope.slice(..);
    let cursor = selection.primary().cursor(text);
    let start = movement::move_prev_word_start(text, Range::point(cursor), 1).head;
    if start == cursor || !text.slice(start..cursor).chars().all(char_is_word) {
        return None;
    }
    let typed: Cow<str> = text.slice(start..cursor).into();
    let candidates: Vec<_> = snippets
        .iter()
        .filter(|item| {
            item.snippet.language == language && item.snippet.prefix.starts_with(typed.as_ref())
        })
        .take(64)
        .cloned()
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let offset = start as i128 - cursor as i128;
    let tab_width = doc.tab_width();
    let indent_style = doc.indent_style;
    let line_ending = doc.line_ending.as_str();
    // Indentation multiplication is preflighted, including all selections, so
    // a short multiline snippet cannot expand an enormous indented document.
    let indentation = selection
        .iter()
        .map(|range| {
            let cursor = range.cursor(text);
            let line = rope.char_to_line(cursor);
            let prefix = text.slice(rope.line_to_char(line)..cursor);
            prefix
                .chars()
                .take_while(|character| character.is_whitespace())
                .take(4097)
                .map(char::len_utf8)
                .sum::<usize>()
        })
        .max()
        .unwrap_or_default();
    if indentation > 4096 {
        return None;
    }
    Some(move || {
        let mut items = Vec::new();
        let mut remaining = 1024 * 1024usize;
        for candidate in candidates {
            if handle.is_canceled() {
                break;
            }
            if candidate.rendered_nodes.saturating_mul(selection.len()) > 4096 {
                continue;
            }
            let work = candidate
                .rendered_bytes
                .checked_add(candidate.newlines.saturating_mul(indentation))
                .and_then(|size| size.checked_mul(selection.len()))
                .unwrap_or(usize::MAX);
            if work > remaining {
                continue;
            }
            remaining -= work;
            let mut ctx = snippets::SnippetRenderCtx {
                resolve_var: Box::new(|_| None),
                tab_width,
                indent_style,
                line_ending,
            };
            let (transaction, _) = lsp_client::util::generate_transaction_from_snippet(
                &rope,
                &selection,
                Some((offset, 0)),
                false,
                candidate.parsed.clone(),
                &mut ctx,
            );
            items.push(CompletionItem::Snippet(StaticSnippetItem {
                owner: candidate.owner,
                label: candidate.snippet.prefix,
                description: candidate.snippet.description,
                body: candidate.snippet.body,
                parsed: candidate.parsed,
                transaction,
                rendered_bytes: candidate.rendered_bytes,
                newlines: candidate.newlines,
                rendered_nodes: candidate.rendered_nodes,
                edit_offset: offset,
            }));
        }
        CompletionResponse {
            items: CompletionItems::Other(items),
            provider: CompletionProvider::Snippet,
            context: ResponseContext {
                is_incomplete: false,
                priority: 1,
                savepoint,
            },
        }
    })
}

//! Native snippets share completion savepoints, previews, and edit admission.
use super::{
    CompletionItems, CompletionResponse, ResponseContext, SnippetCompletion, TextCompletion,
};
use crate::{document::SavePoint, Editor};
use snippets::Catalog;
use std::sync::Arc;

pub(super) fn completion(
    editor: &Editor,
    catalog: &Catalog,
    savepoint: Arc<SavePoint>,
) -> Option<CompletionResponse> {
    if !editor.config().completion.snippets {
        return None;
    }
    let (view, doc) = current_ref!(editor);
    let text = doc.text();
    let selection = doc.selection(view.id);
    let language = crate::features::snippets::language_scope(doc, view.id);
    let mut items = Vec::new();
    for entry in catalog.matching(language) {
        if entry.prefixes.is_empty() {
            continue;
        }
        let mut context = editor.snippet_context(doc, text, selection, &entry.snippet);
        let (transaction, rendered) = crate::editing::completion::generate_transaction_from_snippet(
            text,
            selection,
            None,
            editor.config().completion.replace,
            &entry.snippet,
            &mut context,
        );
        let documentation = format!(
            "{}\n\n{}\n\n```{language}\n{}\n```",
            entry.name, entry.description, entry.body
        );
        for prefix in &entry.prefixes {
            items.push(SnippetCompletion {
                text: TextCompletion {
                    transaction: transaction.clone(),
                    label: prefix.clone().into(),
                    kind: "snippet".into(),
                    documentation: Some(documentation.clone()),
                },
                rendered: rendered.clone(),
            });
        }
    }
    Some(CompletionResponse {
        items: CompletionItems::Snippet(items),
        context: ResponseContext {
            is_incomplete: false,
            priority: 0,
            savepoint,
        },
    })
}

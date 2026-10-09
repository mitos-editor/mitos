use std::mem;

use super::ResponseContext;
use editor_core::completion::CompletionProvider;
use lsp_client::{lsp, LanguageServerId};

pub struct CompletionResponse {
    pub items: CompletionItems,
    pub provider: CompletionProvider,
    pub context: ResponseContext,
}

pub enum CompletionItems {
    Lsp(Vec<lsp::CompletionItem>),
    Other(Vec<CompletionItem>),
}

impl CompletionItems {
    pub fn is_empty(&self) -> bool {
        match self {
            CompletionItems::Lsp(items) => items.is_empty(),
            CompletionItems::Other(items) => items.is_empty(),
        }
    }
}

impl CompletionResponse {
    pub fn take_items(&mut self, dst: &mut Vec<CompletionItem>) {
        match &mut self.items {
            CompletionItems::Lsp(items) => dst.extend(items.drain(..).map(|item| {
                CompletionItem::Lsp(LspCompletionItem {
                    item,
                    provider: match self.provider {
                        CompletionProvider::Lsp(provider) => provider,
                        _ => unreachable!(),
                    },
                    resolved: false,
                    provider_priority: self.context.priority,
                })
            })),
            CompletionItems::Other(items) if dst.is_empty() => mem::swap(dst, items),
            CompletionItems::Other(items) => dst.append(items),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct LspCompletionItem {
    pub item: lsp::CompletionItem,
    pub provider: LanguageServerId,
    pub resolved: bool,
    // TODO: we should not be filtering and sorting incomplete completion list
    // according to the spec but vscode does that anyway and most servers (
    // including rust-analyzer) rely on that.. so we can't do that without
    // breaking completions.
    pub provider_priority: i8,
}

impl LspCompletionItem {
    #[inline]
    pub fn filter_text(&self) -> &str {
        self.item
            .filter_text
            .as_ref()
            .unwrap_or(&self.item.label)
            .as_str()
    }
}

#[allow(clippy::large_enum_variant)] // TODO: In a separate PR attempt the `Box<LspCompletionItem>` pattern.
#[derive(Debug, PartialEq, Clone)]
pub enum CompletionItem {
    Lsp(LspCompletionItem),
    Other(editor_core::CompletionItem),
    Snippet(StaticSnippetItem),
}

#[derive(Debug, Clone)]
pub struct StaticSnippetItem {
    pub owner: plugin_api::assets::AssetOwner,
    pub label: String,
    pub description: String,
    pub body: String,
    pub parsed: std::sync::Arc<snippets::Snippet>,
    pub transaction: editor_core::Transaction,
    pub rendered_bytes: usize,
    pub newlines: usize,
    pub rendered_nodes: usize,
    pub edit_offset: i128,
}

impl StaticSnippetItem {
    pub fn render(
        &self,
        doc: &crate::Document,
        view: crate::ViewId,
        replace_mode: bool,
    ) -> Option<(editor_core::Transaction, snippets::RenderedSnippet)> {
        let text = doc.text().slice(..);
        let selection = doc.selection(view);
        let indentation = selection
            .iter()
            .map(|range| {
                let cursor = range.cursor(text);
                let prefix =
                    text.slice(doc.text().line_to_char(doc.text().char_to_line(cursor))..cursor);
                prefix
                    .chars()
                    .take_while(|character| character.is_whitespace())
                    .take(4097)
                    .map(char::len_utf8)
                    .sum::<usize>()
            })
            .max()
            .unwrap_or_default();
        let work = self
            .rendered_bytes
            .checked_add(self.newlines.checked_mul(indentation)?)?
            .checked_mul(selection.len())?;
        if indentation > 4096
            || work > 1024 * 1024
            || self.rendered_nodes.checked_mul(selection.len())? > 4096
        {
            return None;
        }
        Some(lsp_client::util::generate_transaction_from_snippet(
            doc.text(),
            selection,
            Some((self.edit_offset, 0)),
            replace_mode,
            self.parsed.clone(),
            &mut doc.snippet_ctx(),
        ))
    }
}

impl PartialEq for StaticSnippetItem {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.label == other.label
            && self.body == other.body
            && self.transaction == other.transaction
    }
}

impl CompletionItem {
    #[inline]
    pub fn filter_text(&self) -> &str {
        match self {
            CompletionItem::Lsp(item) => item.filter_text(),
            CompletionItem::Other(item) => &item.label,
            CompletionItem::Snippet(item) => &item.label,
        }
    }
}

impl PartialEq<CompletionItem> for LspCompletionItem {
    fn eq(&self, other: &CompletionItem) -> bool {
        match other {
            CompletionItem::Lsp(other) => self == other,
            _ => false,
        }
    }
}

impl PartialEq<CompletionItem> for editor_core::CompletionItem {
    fn eq(&self, other: &CompletionItem) -> bool {
        match other {
            CompletionItem::Other(other) => self == other,
            _ => false,
        }
    }
}

impl CompletionItem {
    pub fn provider_priority(&self) -> i8 {
        match self {
            CompletionItem::Lsp(item) => item.provider_priority,
            // sorting path completions after LSP for now
            CompletionItem::Other(_) | CompletionItem::Snippet(_) => 1,
        }
    }

    pub fn provider(&self) -> CompletionProvider {
        match self {
            CompletionItem::Lsp(item) => CompletionProvider::Lsp(item.provider),
            CompletionItem::Other(item) => item.provider,
            CompletionItem::Snippet(_) => CompletionProvider::Snippet,
        }
    }

    pub fn preselect(&self) -> bool {
        match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.preselect.unwrap_or(false),
            CompletionItem::Other(_) | CompletionItem::Snippet(_) => false,
        }
    }
}

use std::borrow::Cow;

use super::ResponseContext;
use editor_core::Transaction;
use lsp_client::{lsp, LanguageServerId};

/// Identifies the provider whose requests, settings, and results are tracked together.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum CompletionProvider {
    Lsp(LanguageServerId),
    Path,
    Word,
}

impl From<LanguageServerId> for CompletionProvider {
    fn from(id: LanguageServerId) -> Self {
        Self::Lsp(id)
    }
}

/// An editor-generated completion with a prepared text transaction.
#[derive(Debug, PartialEq, Clone)]
pub struct TextCompletion {
    pub transaction: Transaction,
    pub label: Cow<'static, str>,
    pub kind: Cow<'static, str>,
    /// Markdown documentation.
    pub documentation: Option<String>,
}

pub struct CompletionResponse {
    pub(super) items: CompletionItems,
    pub context: ResponseContext,
}

/// Raw batches retain provider identity even when empty or drained.
pub(super) enum CompletionItems {
    Lsp {
        provider: LanguageServerId,
        items: Vec<lsp::CompletionItem>,
    },
    Path(Vec<TextCompletion>),
    Word(Vec<TextCompletion>),
}

impl CompletionItems {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Lsp { items, .. } => items.is_empty(),
            Self::Path(items) | Self::Word(items) => items.is_empty(),
        }
    }
}

impl CompletionResponse {
    pub fn provider(&self) -> CompletionProvider {
        match &self.items {
            CompletionItems::Lsp { provider, .. } => CompletionProvider::Lsp(*provider),
            CompletionItems::Path(_) => CompletionProvider::Path,
            CompletionItems::Word(_) => CompletionProvider::Word,
        }
    }

    pub fn take_items(&mut self, dst: &mut Vec<CompletionItem>) {
        match &mut self.items {
            CompletionItems::Lsp { provider, items } => dst.extend(items.drain(..).map(|item| {
                CompletionItem::Lsp(LspCompletionItem {
                    item,
                    provider: *provider,
                    resolved: false,
                    provider_priority: self.context.priority,
                })
            })),
            CompletionItems::Path(items) => dst.extend(items.drain(..).map(CompletionItem::Path)),
            CompletionItems::Word(items) => dst.extend(items.drain(..).map(CompletionItem::Word)),
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
    Path(TextCompletion),
    Word(TextCompletion),
}

impl CompletionItem {
    #[inline]
    pub fn filter_text(&self) -> &str {
        match self {
            CompletionItem::Lsp(item) => item.filter_text(),
            CompletionItem::Path(item) | CompletionItem::Word(item) => &item.label,
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

impl CompletionItem {
    pub fn provider_priority(&self) -> i8 {
        match self {
            CompletionItem::Lsp(item) => item.provider_priority,
            // sorting path completions after LSP for now
            CompletionItem::Path(_) | CompletionItem::Word(_) => 1,
        }
    }

    pub fn provider(&self) -> CompletionProvider {
        match self {
            CompletionItem::Lsp(item) => CompletionProvider::Lsp(item.provider),
            CompletionItem::Path(_) => CompletionProvider::Path,
            CompletionItem::Word(_) => CompletionProvider::Word,
        }
    }

    pub fn preselect(&self) -> bool {
        match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.preselect.unwrap_or(false),
            CompletionItem::Path(_) | CompletionItem::Word(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use slotmap::SlotMap;

    #[tokio::test]
    async fn completion_batches_preserve_provider_priority_and_append_order() {
        let mut doc = crate::document::test_document("");
        let view = crate::View::new(doc.id(), Default::default(), event::Redraw::default());
        doc.ensure_view_init(view.id);
        let context = ResponseContext {
            is_incomplete: false,
            priority: -2,
            savepoint: doc.savepoint(view.id),
        };
        let mut servers = SlotMap::<LanguageServerId, ()>::with_key();
        let server = servers.insert(());
        let mut response = CompletionResponse {
            items: CompletionItems::Lsp {
                provider: server,
                items: vec![lsp::CompletionItem::new_simple(
                    "server".into(),
                    "detail".into(),
                )],
            },
            context: context.clone(),
        };
        let text_item = || TextCompletion {
            transaction: Transaction::new(doc.text()),
            label: "text".into(),
            kind: "word".into(),
            documentation: None,
        };
        let mut items = vec![CompletionItem::Word(text_item())];
        response.take_items(&mut items);
        assert!(response.items.is_empty());
        assert_eq!(response.provider(), CompletionProvider::Lsp(server));
        assert_eq!(items[1].provider_priority(), -2);
        response.take_items(&mut items);
        assert_eq!(items.len(), 2);

        let mut response = CompletionResponse {
            items: CompletionItems::Path(vec![text_item()]),
            context,
        };
        response.take_items(&mut items);
        assert_eq!(response.provider(), CompletionProvider::Path);
        assert_eq!(
            items
                .iter()
                .map(CompletionItem::provider)
                .collect::<Vec<_>>(),
            [
                CompletionProvider::Word,
                CompletionProvider::Lsp(server),
                CompletionProvider::Path
            ]
        );
    }
}

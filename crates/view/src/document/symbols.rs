//! Symbol trees, per-view breadcrumbs, and their shared request lifetime.

use event::TaskController;
use lsp_client::{
    lsp::{self, DocumentSymbol},
    OffsetEncoding,
};
use std::{collections::HashMap, sync::Arc};

use super::Document;
use crate::{handlers::document_symbols::DocumentSymbolsHandler, ViewId};

#[derive(Default)]
pub(crate) struct DocumentSymbols {
    cache: Option<DocumentSymbolCache>,
    breadcrumbs: HashMap<ViewId, Breadcrumbs>,
    pub(crate) request: TaskController,
    pub(crate) handler: Option<DocumentSymbolsHandler>,
}

impl DocumentSymbols {
    /// Invalidate both the request and every derived breadcrumb.
    pub(crate) fn clear(&mut self) {
        self.request.cancel();
        self.clear_cache();
    }

    fn clear_cache(&mut self) {
        self.cache = None;
        self.breadcrumbs.clear();
    }

    pub(super) fn remove_view(&mut self, view: ViewId) {
        self.breadcrumbs.remove(&view);
    }
}

struct DocumentSymbolCache {
    tree: Vec<ThinDocumentSymbol>,
    offset_encoding: OffsetEncoding,
}

#[derive(Debug, Clone)]
struct ThinDocumentSymbol {
    /// Shared with active crumbs so cursor movement never reallocates symbol names.
    name: Arc<str>,
    kind: lsp::SymbolKind,
    range: lsp::Range,
    children: Option<Box<[Self]>>,
}

impl From<DocumentSymbol> for ThinDocumentSymbol {
    #[inline]
    fn from(symbol: DocumentSymbol) -> Self {
        Self {
            name: symbol.name.into(),
            kind: symbol.kind,
            range: symbol.range,
            children: symbol.children.map(|children| {
                let mut vec = Vec::with_capacity(children.len());
                vec.extend(children.into_iter().map(Self::from));
                vec.into_boxed_slice()
            }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Breadcrumbs(Vec<Crumb>);

impl Breadcrumbs {
    #[inline]
    pub fn push(&mut self, crumb: Crumb) {
        self.0.push(crumb);
    }

    #[inline]
    pub fn clear(&mut self) {
        self.0.clear();
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Crumb> {
        self.0.iter()
    }
}

#[derive(Debug, Clone)]
pub struct Crumb {
    pub name: Arc<str>,
    pub kind: lsp::SymbolKind,
}

impl From<&ThinDocumentSymbol> for Crumb {
    #[inline]
    fn from(symbol: &ThinDocumentSymbol) -> Self {
        Self {
            name: symbol.name.clone(),
            kind: symbol.kind,
        }
    }
}

impl Document {
    #[cold]
    pub fn set_document_symbols(
        &mut self,
        symbols: Vec<DocumentSymbol>,
        offset_encoding: OffsetEncoding,
    ) {
        if !self.breadcrumb_enabled() {
            self.clear_document_symbols();
            return;
        }

        self.document_symbols.cache = Some(DocumentSymbolCache {
            tree: {
                let mut tree = Vec::with_capacity(symbols.len());
                tree.extend(symbols.into_iter().map(ThinDocumentSymbol::from));
                tree
            },
            offset_encoding,
        });

        // PERF: Symbol responses are cold. Collecting the usually tiny view-id set here avoids
        // allocations on cursor movement while ensuring every split is refreshed immediately.
        let view_ids: Vec<_> = self.selections.keys().copied().collect();
        for view_id in view_ids {
            self.update_breadcrumbs_for_view(view_id);
        }
    }

    /// Clear the symbol tree and breadcrumbs without canceling a replacement request.
    #[inline]
    pub fn clear_document_symbols(&mut self) {
        self.document_symbols.clear_cache();
    }

    #[inline]
    pub fn breadcrumbs(&self, view_id: ViewId) -> Option<&Breadcrumbs> {
        self.document_symbols.breadcrumbs.get(&view_id)
    }

    // For all non-hotpaths, we use this function to prevent code bloat.
    #[inline(never)]
    pub fn update_breadcrumbs_for_view(&mut self, view_id: ViewId) {
        self.update_breadcrumbs_for_view_inlined(view_id);
    }

    // We want to make sure this is inlined in the hotpath (cursor position change).
    #[inline(always)]
    pub fn update_breadcrumbs_for_view_inlined(&mut self, view_id: ViewId) {
        if !self.breadcrumb_enabled() {
            self.document_symbols.breadcrumbs.remove(&view_id);
            return;
        }

        #[inline(always)]
        const fn in_range(pos: lsp::Position, range: lsp::Range) -> bool {
            // PERF:
            // Line-based filtering is the most effective early exit indicator,
            // so do first, before other evaluations; this should be friendly to
            // the CPU branch predictor.
            if pos.line < range.start.line || pos.line > range.end.line {
                return false;
            }

            // Check if the cursor position is "in" the symbols "depth".
            //
            // In the context of breadcrumbs, this would be the difference between
            // if the cursor is in an impl block or in an impl block and in a
            // function of the impl block (`|` is the cursor):
            //
            // ```rust
            // impl Foo {
            //     f|n bar() {} // In `bar`: impl Foo > bar
            //
            //   | fn baz() {} // Not in `baz`: impl Foo
            //
            //     fn quux() {} | // Not in `quux`: impl Foo
            // }
            // ```
            if pos.line == range.start.line && pos.character < range.start.character {
                return false;
            }
            if pos.line == range.end.line && pos.character >= range.end.character {
                return false;
            }

            true
        }

        let Some(symbols) = self.document_symbols.cache.as_ref() else {
            return;
        };

        let position = self.position(view_id, symbols.offset_encoding);

        let breadcrumb = {
            let breadcrumb = self
                .document_symbols
                .breadcrumbs
                .entry(view_id)
                .or_default();
            breadcrumb.clear();
            breadcrumb
        };

        let mut current = symbols.tree.as_slice();

        while let Some(symbol) = current
            .iter()
            .find(|&symbol| in_range(position, symbol.range))
        {
            breadcrumb.push(Crumb::from(symbol));
            match symbol.children.as_deref() {
                Some(children) => current = children,
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use arc_swap::ArcSwap;
    use editor_core::{syntax, Rope, Selection};
    use std::path::Path;

    #[allow(deprecated)]
    fn document_symbol(
        name: &str,
        kind: lsp::SymbolKind,
        range: lsp::Range,
        children: Option<Vec<DocumentSymbol>>,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_owned(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children,
        }
    }

    #[tokio::test]
    async fn document_symbols_refresh_breadcrumbs_without_reallocating_names() {
        let text = Rope::from("impl A {\n fn b() {}\n}\n");
        let mut config = Config::default();
        config.breadcrumb.enable = true;
        let mut doc = Document::from(
            text,
            None,
            Arc::new(ArcSwap::new(Arc::new(config))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        doc.set_path(Some(Path::new("test.rs")));
        let view = ViewId::default();
        let cursor = doc.text().line_to_char(1) + 5;
        doc.set_selection(view, Selection::single(cursor, cursor));

        let child = document_symbol(
            "b",
            lsp::SymbolKind::FUNCTION,
            lsp::Range::new(lsp::Position::new(1, 1), lsp::Position::new(1, 10)),
            None,
        );
        let parent = document_symbol(
            "A",
            lsp::SymbolKind::OBJECT,
            lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(2, 1)),
            Some(vec![child]),
        );
        doc.set_document_symbols(vec![parent], OffsetEncoding::Utf8);

        let breadcrumb = &doc.document_symbols.breadcrumbs[&view];
        assert_eq!(
            breadcrumb
                .iter()
                .map(|crumb| crumb.name.as_ref())
                .collect::<Vec<_>>(),
            ["A", "b"]
        );
        let cached_parent_name = &doc.document_symbols.cache.as_ref().unwrap().tree[0].name;
        assert!(Arc::ptr_eq(cached_parent_name, &breadcrumb.0[0].name));

        let child_end = doc.text().line_to_char(1) + 10;
        doc.set_selection(view, Selection::single(child_end, child_end));
        doc.update_breadcrumbs_for_view_inlined(view);
        assert_eq!(
            doc.document_symbols.breadcrumbs[&view]
                .iter()
                .map(|crumb| crumb.name.as_ref())
                .collect::<Vec<_>>(),
            ["A"]
        );

        doc.set_selection(view, Selection::single(1, 1));
        doc.update_breadcrumbs_for_view_inlined(view);
        assert_eq!(
            doc.document_symbols.breadcrumbs[&view]
                .iter()
                .map(|crumb| crumb.name.as_ref())
                .collect::<Vec<_>>(),
            ["A"]
        );

        let request = doc.document_symbols.request.restart();
        doc.remove_view(view);
        assert!(doc.breadcrumbs(view).is_none());
        assert!(doc.document_symbols.cache.is_some());
        assert!(!request.is_canceled());

        doc.document_symbols.clear();
        assert!(doc.document_symbols.cache.is_none());
        assert!(request.is_canceled());
    }
}

//! Editor-owned user snippet catalog and replacement lifecycle.
use crate::Editor;
use snippets::Catalog;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct SnippetStore {
    catalog: Arc<Catalog>,
    loaded: bool,
}

impl Editor {
    /// Install validated definitions and cancel completions from the old catalog.
    pub fn set_snippet_catalog(&mut self, catalog: Catalog) {
        super::completion::invalidate(self);
        self.snippets.catalog = Arc::new(catalog);
        self.snippets.loaded = true;
    }

    /// Reload user definitions atomically. Invalid files leave the active catalog intact.
    pub fn reload_snippets(&mut self) -> anyhow::Result<()> {
        let catalog = loader::snippets::load(&loader::snippets::user_file())?;
        self.set_snippet_catalog(catalog);
        Ok(())
    }

    pub(crate) fn user_snippets(&mut self) -> Arc<Catalog> {
        if !self.snippets.loaded {
            self.snippets.loaded = true;
            match loader::snippets::load(&loader::snippets::user_file()) {
                Ok(catalog) => self.snippets.catalog = Arc::new(catalog),
                Err(error) => self.set_error(|| format!("{error:#}")),
            }
        }
        self.snippets.catalog.clone()
    }
}

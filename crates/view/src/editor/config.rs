//! Applying configuration to shared editor state. Frontends load their composed
//! settings, choose a theme, and install the settings snapshot before refresh.

use std::sync::Arc;

use editor_core::{config::LanguageLoaderError, syntax};

use crate::{config::Config, events::ConfigDidChange, theme::Theme, Editor};

impl Editor {
    /// Update trust before loading global and workspace language configuration.
    /// A load failure preserves the active language loader and documents, while
    /// retaining the trust update and cache invalidation.
    pub fn load_language_config(
        &mut self,
        config: &Config,
    ) -> Result<syntax::Loader, LanguageLoaderError> {
        self.workspace_trust
            .set_config((&config.workspace_trust).into());
        editor_core::config::user_lang_loader(&self.workspace_trust)
    }

    /// Install a language loader and the frontend-selected theme, then refresh
    /// document settings and diagnostics. Theme scopes must be set before parsing.
    /// As in the configuration reload path, documents refresh even if the theme is
    /// invalid; the theme error is returned separately for the frontend to handle.
    pub fn apply_language_config(
        &mut self,
        loader: syntax::Loader,
        theme: Theme,
    ) -> anyhow::Result<()> {
        self.syn_loader.store(Arc::new(loader));
        let theme_result = self.set_theme(theme);
        let loader = self.syn_loader.load();
        for document in self.documents.values_mut() {
            document.detect_editor_config();
            document.detect_language(&loader);
            let diagnostics =
                Self::doc_diagnostics(&self.language_servers, &self.diagnostics, document);
            document.replace_diagnostics(diagnostics, &[], None);
        }
        theme_result
    }

    /// Call if the config has changed to let the editor update all
    /// relevant members.
    pub fn refresh_config(&mut self, old_config: &Config) {
        let config = self.config();
        self.auto_pairs = (&config.auto_pairs).into();
        self.reset_idle_timer();
        self._refresh();
        event::dispatch(ConfigDidChange {
            editor: self,
            old: old_config,
            new: &config,
        });

        // Hooks may change document layout; keep every view's cursor visible after
        // they have observed the newly installed settings (including soft wrapping).
        let scrolloff = self.config().scrolloff;
        for (view, _) in self.tree.views() {
            let doc = doc_mut!(self, &view.doc);
            view.ensure_cursor_in_view(doc, scrolloff);
        }
    }
}

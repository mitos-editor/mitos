//! Applying configuration to shared editor state. Frontends load their composed
//! settings, choose a theme, and install the settings snapshot before refresh.

use std::sync::Arc;
use tokio::sync::mpsc;

use super::ConfigEvent;

use crate::{events::ConfigDidChange, Editor};
use editor_config::Config;
use language::{loader::LanguageLoaderError, syntax};
use theme::Theme;

/// Immutable destination for shared configuration requests, installed before editor construction.
#[derive(Clone)]
pub struct ConfigRequests(Arc<dyn Fn(ConfigEvent) -> anyhow::Result<()> + Send + Sync>);

impl ConfigRequests {
    pub fn new(send: impl Fn(ConfigEvent) -> anyhow::Result<()> + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    pub fn send(&self, request: ConfigEvent) -> anyhow::Result<()> {
        (self.0)(request)
    }

    pub(super) fn channel() -> (Self, mpsc::UnboundedReceiver<ConfigEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self::new(move |request| Ok(sender.send(request)?)),
            receiver,
        )
    }
}

impl Editor {
    pub fn config_requests(&self) -> &ConfigRequests {
        &self.config_requests
    }

    /// Update trust before loading global and workspace language configuration.
    /// A load failure preserves the active language loader and documents, while
    /// retaining the trust update and cache invalidation.
    pub fn load_language_config(
        &mut self,
        config: &Config,
        workspace: workspace::ResolvedWorkspace,
    ) -> Result<syntax::Loader, LanguageLoaderError> {
        self.workspace_trust
            .set_config((&config.workspace_trust).into());
        self.workspace = workspace;
        language::loader::user_lang_loader(
            &loader::config::ConfigSources::new(&self.workspace, &self.workspace_trust),
            self.syn_loader.load().resources().clone(),
        )
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
            document.detect_language(&loader);
            document.refresh_settings();
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
        // Apply path-based settings before layout and feature observers read them.
        // Other live settings are read through the installed configuration snapshot.
        if old_config.files.editor_config != config.files.editor_config {
            for document in self.documents.values_mut() {
                document.refresh_settings();
            }
        }
        self.tree.set_split_strategy(config.splits.strategy);
        self.reset_idle_timer();
        self._refresh();
        self.events()
            .clone()
            .dispatch(ConfigDidChange {
                editor: self,
                old: old_config,
                new: &config,
            })
            .report_to(self.status_sender());

        // Hooks may change document layout; keep every view's cursor visible after
        // they have observed the newly installed settings (including soft wrapping).
        let scrolloff = self.config().scroll.padding;
        for (view, _) in self.tree.views() {
            let doc = doc_mut!(self, &view.doc);
            view.ensure_cursor_in_view(doc, scrolloff);
        }
    }
}

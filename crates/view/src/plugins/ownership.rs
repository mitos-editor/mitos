//! Generation ownership and executable instance liveness.
use crate::Editor;
use plugin_api::PluginOwner;

impl Editor {
    /// Owner identity is host assigned and expires on unload, trap or reload.
    pub fn plugin_owner_is_current(&self, owner: &PluginOwner) -> bool {
        self.plugin_identity_is_current(&owner.plugin, owner.generation)
    }

    pub(super) fn plugin_identity_is_current(&self, plugin: &str, generation: u64) -> bool {
        !self.plugins.stopped
            && generation == self.plugins.shared.generation
            && self.plugins.manager.is_active(plugin)
    }

    /// Prune diagnostics and settings only when an instance becomes live or
    /// retires. Ordinary completions do not scan documents or setting stacks.
    pub(super) fn refresh_plugin_ownership(&mut self) {
        let current = self
            .plugins
            .manager
            .active_plugins()
            .map(|plugin| PluginOwner {
                plugin: plugin.to_owned(),
                generation: self.plugins.shared.generation,
            })
            .collect();
        if self.plugins.active_owners != current {
            self.plugins.active_owners = current;
            self.prune_plugin_settings();
            self.prune_plugin_document_diagnostics();
        }
    }
}

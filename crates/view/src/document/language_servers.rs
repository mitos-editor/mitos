//! Document attachment and protocol-open state. Feature providers are exposed only
//! after editor-side initialization has opened this document on that server.

use super::*;

impl Document {
    /// maintains the order as configured in the language_servers TOML array
    pub fn language_servers(&self) -> impl Iterator<Item = &lsp_client::Client> {
        self.language_config().into_iter().flat_map(move |config| {
            config.language_servers.iter().filter_map(move |features| {
                let ls = &**self.language_servers.get(&features.name)?;
                if self.opened_language_servers.contains(&ls.id()) {
                    Some(ls)
                } else {
                    None
                }
            })
        })
    }

    pub fn remove_language_server_by_name(&mut self, name: &str) -> Option<Arc<Client>> {
        let server = self.language_servers.remove(name);
        if let Some(server) = &server {
            self.opened_language_servers.remove(&server.id());
            self.cancel_hover_requests(None);
            self.navigation.clear();
            self.document_links.cancel_navigation();
            self.reset_all_inlay_hints();
        }
        server
    }

    pub fn language_servers_with_feature(
        &self,
        feature: LanguageServerFeature,
    ) -> impl Iterator<Item = &lsp_client::Client> {
        self.language_config().into_iter().flat_map(move |config| {
            config.language_servers.iter().filter_map(move |features| {
                let ls = &**self.language_servers.get(&features.name)?;
                if self.opened_language_servers.contains(&ls.id())
                    && crate::language_servers::supports_feature(ls.capabilities(), feature)
                    && features.has_feature(feature)
                {
                    Some(ls)
                } else {
                    None
                }
            })
        })
    }

    pub fn supports_language_server(&self, id: LanguageServerId) -> bool {
        self.language_servers().any(|l| l.id() == id)
    }

    pub(crate) fn attach_language_servers(
        &mut self,
        servers: HashMap<LanguageServerName, Arc<Client>>,
        ready: &HashSet<LanguageServerId>,
    ) {
        for (name, server) in &self.language_servers {
            if servers
                .get(name)
                .is_none_or(|next| next.id() != server.id())
                && self.opened_language_servers.remove(&server.id())
            {
                server.text_document_did_close(self.identifier());
            }
        }
        self.language_servers = servers;
        let ids: Vec<_> = self
            .language_servers
            .values()
            .map(|server| server.id())
            .filter(|id| ready.contains(id))
            .collect();
        for id in ids {
            self.open_language_server_document(id);
        }
    }

    pub(crate) fn open_language_server_document(&mut self, id: LanguageServerId) {
        let Some(server) = self
            .language_servers
            .values()
            .find(|server| server.id() == id)
        else {
            return;
        };
        let Some(uri) = self.url() else {
            return;
        };
        if self.opened_language_servers.insert(id) {
            server.text_document_did_open(
                uri,
                self.version(),
                self.text(),
                self.language_id().unwrap_or_default().to_owned(),
            );
        }
    }

    pub(crate) fn close_language_server_documents(&mut self) {
        for server in self.language_servers.values() {
            if self.opened_language_servers.remove(&server.id()) {
                server.text_document_did_close(self.identifier());
            }
        }
    }
}

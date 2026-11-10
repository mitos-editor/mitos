//! Document attachment and protocol-open state. Feature providers are exposed only
//! after editor-side initialization has opened this document on that server.

use super::*;

pub(crate) struct Attachment {
    pub(crate) client: Arc<Client>,
    pub(crate) opened: bool,
}

impl Document {
    /// Maintains the order and feature routing configured in the language-server array.
    pub fn language_servers(&self) -> impl Iterator<Item = &lsp_client::Client> {
        self.language_config().into_iter().flat_map(move |config| {
            config.language_servers.iter().filter_map(move |features| {
                let attachment = self.language_servers.get(&features.name)?;
                attachment.opened.then_some(&*attachment.client)
            })
        })
    }

    pub fn remove_language_server_by_name(&mut self, name: &str) -> Option<Arc<Client>> {
        let attachment = self.language_servers.remove(name)?;
        self.cancel_view_requests(|state| &mut state.hover);
        self.cancel_view_requests(|state| &mut state.navigation);
        self.document_links.cancel_navigation();
        self.reset_all_inlay_hints();
        Some(attachment.client)
    }

    /// Each eligible client appears once, in first matching configured order.
    pub fn language_servers_with_feature(
        &self,
        feature: LanguageServerFeature,
    ) -> impl Iterator<Item = &lsp_client::Client> {
        let mut seen = std::collections::HashSet::new();
        self.language_config()
            .into_iter()
            .flat_map(move |config| {
                config.language_servers.iter().filter_map(move |features| {
                    let attachment = self.language_servers.get(&features.name)?;
                    let client = &*attachment.client;
                    (attachment.opened
                        && crate::language_servers::supports_feature(
                            client.capabilities(),
                            feature,
                        )
                        && features.has_feature(feature))
                    .then_some(client)
                })
            })
            .filter(move |client| seen.insert(client.id()))
    }

    /// Wire synchronization visits each opened binding once, independently of
    /// duplicate configured entries used for feature routing.
    pub(crate) fn opened_language_servers(&self) -> impl Iterator<Item = &Arc<Client>> {
        self.language_servers
            .values()
            .filter(|attachment| attachment.opened)
            .map(|attachment| &attachment.client)
    }

    pub fn supports_language_server(&self, id: LanguageServerId) -> bool {
        self.language_servers().any(|l| l.id() == id)
    }

    pub(crate) fn attach_language_servers(
        &mut self,
        servers: HashMap<LanguageServerName, Arc<Client>>,
        registry: &crate::language_servers::Registry,
    ) {
        let identifier = self.identifier();
        self.language_servers.retain(|name, attachment| {
            let keep = servers
                .get(name)
                .is_some_and(|next| next.id() == attachment.client.id());
            if !keep && attachment.opened {
                attachment
                    .client
                    .text_document_did_close(identifier.clone());
            }
            keep
        });
        for (name, client) in servers {
            self.language_servers.entry(name).or_insert(Attachment {
                client,
                opened: false,
            });
        }
        let ids: Vec<_> = self
            .language_servers
            .values()
            .filter(|attachment| registry.is_ready(attachment.client.id()))
            .map(|attachment| attachment.client.id())
            .collect();
        for id in ids {
            self.open_language_server_document(id);
        }
    }

    pub(crate) fn open_language_server_document(&mut self, id: LanguageServerId) {
        let Some(client) = self
            .language_servers
            .values()
            .find(|attachment| attachment.client.id() == id && !attachment.opened)
            .map(|attachment| attachment.client.clone())
        else {
            return;
        };
        let Some(uri) = self.url() else {
            return;
        };
        // Registry clients have one fixed name, also used by the attachment caller.
        self.language_servers.get_mut(client.name()).unwrap().opened = true;
        client.text_document_did_open(
            uri,
            self.version(),
            self.text(),
            self.language_id().unwrap_or_default().to_owned(),
        );
    }

    pub(crate) fn close_language_server_documents(&mut self) {
        let Some(uri) = self.url() else {
            return;
        };
        for attachment in self.language_servers.values_mut() {
            if std::mem::replace(&mut attachment.opened, false) {
                attachment
                    .client
                    .text_document_did_close(lsp::TextDocumentIdentifier::new(uri.clone()));
            }
        }
    }
}

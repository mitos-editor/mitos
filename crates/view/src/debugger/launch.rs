//! Launch preparation and completion belong to the shared editor.

use super::startup::StartupOrigin;
use crate::{DocumentId, Editor};
use anyhow::{bail, Context as _, Result};
use dap::{ConnectionType, DebugAdapterId};
use serde_json::Value;
use std::net::SocketAddr;

/// Resolved launch or attach arguments. Their contents are adapter-specific.
#[derive(Clone)]
pub struct DebuggerLaunch {
    pub request: ConnectionType,
    pub arguments: Value,
}

impl Editor {
    /// Validate the document's launch configuration before creating an adapter.
    /// The document may be hidden; focus changes do not retarget the launch.
    pub fn start_debugger(
        &mut self,
        document: DocumentId,
        name: Option<&str>,
        socket: Option<SocketAddr>,
        params: Option<&[String]>,
    ) -> Result<DebugAdapterId> {
        let doc = self.document(document).context("Document is closed")?;
        if !self
            .workspace_trust
            .query(doc.workspace_root(), workspace::trust::TrustQuery::Dap)
            .is_trusted()
        {
            bail!("Workspace is not trusted. Run `:workspace-trust` to enable the debug adapter.");
        }
        let config = doc
            .language_config()
            .and_then(|config| config.debugger.as_ref())
            .context("No debug adapter available for language")?
            .clone();
        let template = match name {
            Some(name) => config
                .templates
                .iter()
                .find(|template| template.name == name),
            None => config.templates.first(),
        }
        .context("No debug config with given name")?;
        let connection = template.request;
        let args = template.resolve(
            params.unwrap_or_default(),
            &stdx::env::current_working_dir(),
        )?;
        let label = format!("{}: {}", config.name, template.name);
        let id = self.begin_debugger_startup(
            socket,
            config,
            Some(DebuggerLaunch {
                request: connection,
                arguments: args,
            }),
            StartupOrigin::Document(document),
        );
        self.debugger.sessions[id].name = label;
        Ok(id)
    }
}

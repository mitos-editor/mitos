//! Launch preparation and completion belong to the shared editor.

use super::startup::StartupOrigin;
use crate::{DocumentId, Editor};
use anyhow::{bail, Context as _, Result};
use dap::{registry::DebugAdapterId, ConnectionType};
use language_config::{DebugConfigCompletion, DebugTemplate};
use serde_json::{to_value, Value};
use std::{collections::HashMap, net::SocketAddr};

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
        let connection = match template.request.as_str() {
            "launch" => ConnectionType::Launch,
            "attach" => ConnectionType::Attach,
            request => bail!("Unsupported request '{request}'"),
        };
        let mut args: HashMap<_, _> = match params {
            Some(params) => {
                let params = prepare_dap_params(template, params);
                template
                    .args
                    .iter()
                    .map(|(key, value)| (key.as_str(), map_value(value, &params)))
                    .collect()
            }
            None => template
                .args
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect(),
        };
        args.insert("cwd", to_value(stdx::env::current_working_dir())?);
        let args = to_value(args)?;
        Ok(self.begin_debugger_startup(
            socket,
            config,
            Some((connection, args)),
            StartupOrigin::Document(document),
        ))
    }
}

fn prepare_dap_params(template: &DebugTemplate, params: &[String]) -> Vec<String> {
    params
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut param = x.to_string();
            if let Some(DebugConfigCompletion::Advanced(cfg)) = template.completion.get(i)
                && matches!(cfg.completion.as_deref(), Some("filename" | "directory"))
            {
                param = std::fs::canonicalize(x)
                    .ok()
                    .and_then(|pb| pb.into_os_string().into_string().ok())
                    .unwrap_or_else(|| x.to_string());
            }
            param
        })
        .collect()
}

fn map_value(value: &Value, params: &[String]) -> Value {
    match value {
        Value::String(string) => {
            let mut string = string.clone();
            for (i, x) in params.iter().enumerate() {
                let pattern = format!("{{{}}}", i);
                string = string.replace(&pattern, x);
            }
            if let Ok(integer) = string.parse::<usize>() {
                to_value(integer).unwrap()
            } else {
                to_value(string).unwrap()
            }
        }
        Value::Array(array) => Value::Array(array.iter().map(|x| map_value(x, params)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), map_value(v, params)))
                .collect(),
        ),

        _ => value.clone(),
    }
}

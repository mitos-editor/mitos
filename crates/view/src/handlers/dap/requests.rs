//! Reverse requests have one result path; errors reach the adapter as well as status UI.

use crate::Editor;
use anyhow::Context as _;
use dap::{registry::DebugAdapterId, ConnectionType, Request};
use serde_json::{json, Value};

impl Editor {
    pub(super) async fn handle_debugger_request(
        &mut self,
        id: DebugAdapterId,
        sequence: u64,
        command: String,
        arguments: Option<Value>,
    ) {
        let result = match Request::parse(&command, arguments) {
            Ok(Request::RunInTerminal(arguments)) => {
                let launch = self.debugger.run_in_terminal(arguments);
                let callbacks = self.callback_sender().clone();
                tokio::spawn(async move {
                    let result = launch
                        .await
                        .and_then(|response| serde_json::to_value(response).map_err(Into::into));
                    callbacks
                        .send(move |editor| {
                            editor.complete_debugger_request(id, sequence, &command, result);
                        })
                        .await;
                });
                return;
            }
            Ok(Request::StartDebugging(arguments)) => {
                self.start_child_debugger(id, arguments).await
            }
            Err(error) => Err(error),
        };
        self.complete_debugger_request(id, sequence, &command, result);
    }

    fn complete_debugger_request(
        &mut self,
        id: DebugAdapterId,
        sequence: u64,
        command: &str,
        result: dap::Result<Value>,
    ) {
        // A delayed terminal result belongs only to the originating adapter.
        if self.debugger.adapters.get_client(id).is_none() {
            return;
        }
        if let Err(error) = &result {
            self.set_error(|| format!("Debug adapter request failed: {error:#}"));
        }
        if let Some(debugger) = self.debugger.adapters.get_client(id)
            && let Err(error) = debugger.reply(sequence, command, result)
        {
            log::warn!("Could not reply to debug adapter: {error}");
        }
    }

    async fn start_child_debugger(
        &mut self,
        id: DebugAdapterId,
        arguments: dap::requests::StartDebuggingArguments,
    ) -> dap::Result<Value> {
        let debugger = self
            .debugger
            .adapters
            .get_client(id)
            .context("No active debugger found")
            .map_err(dap::Error::Other)?;
        let socket = debugger
            .socket
            .context("Child debugging requires a TCP adapter connection")
            .map_err(dap::Error::Other)?;
        let config = debugger
            .config
            .clone()
            .context("No debugger configuration found")
            .map_err(dap::Error::Other)?;
        let child = self.debugger.adapters.start_client(Some(socket), &config)?;
        let client = self
            .debugger
            .adapters
            .get_client_mut(child)
            .context("Child debugger is no longer available")
            .map_err(dap::Error::Other)?;
        let result = match arguments.request {
            ConnectionType::Launch => client.launch(arguments.configuration).await,
            ConnectionType::Attach => client.attach(arguments.configuration).await,
        };
        if let Err(error) = result {
            self.debugger.remove_client(child);
            return Err(error);
        }
        Ok(json!({ "success": true }))
    }
}

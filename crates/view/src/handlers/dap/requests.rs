//! Reverse requests have one result path; errors reach the adapter as well as status UI.

use crate::Editor;
use dap::{registry::DebugAdapterId, Request};
use serde_json::Value;

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
                match self.start_child_debugger(id, sequence, arguments) {
                    Ok(()) => return,
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        self.complete_debugger_request(id, sequence, &command, result);
    }

    pub(crate) fn complete_debugger_request(
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
}

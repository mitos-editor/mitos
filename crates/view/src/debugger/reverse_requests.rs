//! Session-owned reverse requests; cancellation and completion each reply once.

use crate::Editor;
use dap::{DebugAdapterId, Request};
use event::{TaskController, TaskHandle};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct ReverseRequests {
    pending: HashMap<u64, PendingRequest>,
}

struct PendingRequest {
    command: String,
    cancel: TaskController,
}

impl ReverseRequests {
    fn begin(&mut self, sequence: u64, command: String) -> TaskHandle {
        let mut cancel = TaskController::new();
        let current = cancel.restart();
        self.pending
            .insert(sequence, PendingRequest { command, cancel });
        current
    }
}

impl Editor {
    pub(crate) fn handle_debugger_request(
        &mut self,
        id: DebugAdapterId,
        sequence: u64,
        command: String,
        arguments: Option<Value>,
    ) {
        let result = match Request::parse(&command, arguments) {
            Ok(Request::RunInTerminal(arguments)) => {
                let launch = self.debugger.run_in_terminal(arguments);
                let current = self.debugger.sessions[id]
                    .reverse_requests
                    .begin(sequence, command.clone());
                let callbacks = self.callback_sender().clone();
                let owner = callbacks.clone();
                tokio::spawn(async move {
                    let Some(result) = event::cancelable_future(launch, &current).await else {
                        return;
                    };
                    let result = result
                        .and_then(|response| serde_json::to_value(response).map_err(Into::into));
                    callbacks
                        .send(move |editor| {
                            if current.is_canceled()
                                || !owner.same_destination(editor.callback_sender())
                            {
                                return;
                            }
                            let Some(session) = editor.debugger.get_session_mut(id) else {
                                return;
                            };
                            session.reverse_requests.pending.remove(&sequence);
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

    pub(crate) fn cancel_debugger_reverse_requests(&mut self, id: DebugAdapterId) {
        let Some(session) = self.debugger.get_session_mut(id) else {
            return;
        };
        for (sequence, pending) in session.reverse_requests.pending.drain() {
            drop(pending.cancel);
            if let Some(client) = &session.client
                && let Err(error) = client.reply(
                    sequence,
                    &pending.command,
                    Err(
                        anyhow::anyhow!("Debugging session ended before the request completed")
                            .into(),
                    ),
                )
            {
                log::warn!("Could not cancel debug adapter request: {error}");
            }
        }
    }

    pub(crate) fn complete_debugger_request(
        &mut self,
        id: DebugAdapterId,
        sequence: u64,
        command: &str,
        result: dap::Result<Value>,
    ) {
        // A delayed terminal result belongs only to the originating adapter.
        if self.debugger.get_client(id).is_none() {
            return;
        }
        if let Err(error) = &result {
            if self
                .debugger
                .get_active_session()
                .is_some_and(|session| session.id() == id)
            {
                self.set_error(|| format!("Debug adapter request failed: {error:#}"));
            } else {
                log::warn!("Debug adapter request failed for {id}: {error:#}");
            }
        }
        if let Some(debugger) = self.debugger.get_client(id)
            && let Err(error) = debugger.reply(sequence, command, result)
        {
            log::warn!("Could not reply to debug adapter: {error}");
        }
    }
}

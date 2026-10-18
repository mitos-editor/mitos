mod requests;

use crate::editor::Action;
use crate::{align_view, Align, Editor};
use dap::requests::DisconnectArguments;
use dap::requests::ThreadsArguments;
use dap::{self as dap, registry::DebugAdapterId, Client, ConnectionType, Payload, ThreadId};
use editor_core::Selection;
use serde_json::Value;
use std::fmt::Write;

#[macro_export]
macro_rules! debugger {
    ($editor:expr) => {{
        let Some(debugger) = $editor.debugger_mut().get_active_client_mut() else {
            return;
        };
        debugger
    }};
}

// general utils:
pub fn dap_pos_to_pos(doc: &editor_core::Rope, line: usize, column: usize) -> Option<usize> {
    // 1-indexing to 0 indexing
    let line = doc.try_line_to_char(line - 1).ok()?;
    let pos = line + column.saturating_sub(1);
    // TODO: this is probably utf-16 offsets
    Some(pos)
}

async fn select_thread_id(
    editor: &mut Editor,
    id: DebugAdapterId,
    thread_id: ThreadId,
    force: bool,
) {
    let Some(debugger) = editor.debugger.get_client_mut(id) else {
        return;
    };

    if !force && debugger.thread_id.is_some() {
        return;
    }

    debugger.thread_id = Some(thread_id);
    fetch_stack_trace(debugger, thread_id).await;

    let frame = debugger.stack_frames[&thread_id].first().cloned();
    if editor
        .debugger
        .get_active_client()
        .is_some_and(|client| client.id() == id)
        && let Some(frame) = &frame
    {
        jump_to_stack_frame(editor, frame);
    }
}

pub async fn fetch_stack_trace(debugger: &mut Client, thread_id: ThreadId) {
    let (frames, _) = match debugger.stack_trace(thread_id).await {
        Ok(frames) => frames,
        Err(_) => return,
    };
    debugger.stack_frames.insert(thread_id, frames);
    debugger.active_frame = Some(0);
}

pub fn jump_to_stack_frame(editor: &mut Editor, frame: &dap::StackFrame) {
    let path = if let Some(dap::Source {
        path: Some(ref path),
        ..
    }) = frame.source
    {
        path.clone()
    } else {
        return;
    };

    if let Err(e) = editor.open(&path, Action::Replace) {
        editor.set_error(|| format!("Unable to jump to stack frame: {}", e));
        return;
    }

    let (view, doc) = current!(editor);

    let text_end = doc.text().len_chars().saturating_sub(1);
    let start = dap_pos_to_pos(doc.text(), frame.line, frame.column).unwrap_or(0);
    let end = frame
        .end_line
        .and_then(|end_line| dap_pos_to_pos(doc.text(), end_line, frame.end_column.unwrap_or(0)))
        .unwrap_or(start);

    let selection = Selection::single(start.min(text_end), end.min(text_end));
    doc.set_selection(view.id, selection);
    align_view(doc, view, Align::Center);
}

impl Editor {
    pub async fn handle_debugger_message(
        &mut self,
        id: DebugAdapterId,
        payload: dap::Payload,
    ) -> bool {
        use dap::{events, Event};

        match payload {
            Payload::Event(event) => {
                let event = match Event::parse(&event.event, event.body) {
                    Ok(event) => event,
                    Err(dap::Error::Unhandled) => {
                        log::info!("Discarding unknown DAP event '{}'", event.event);
                        return false;
                    }
                    Err(err) => {
                        log::warn!("Discarding invalid DAP event '{}': {err}", event.event);
                        return false;
                    }
                };
                if matches!(
                    &event,
                    Event::Stopped(_)
                        | Event::Continued(_)
                        | Event::Thread(_)
                        | Event::Terminated(_)
                        | Event::Initialized(_)
                ) {
                    self.debugger.invalidate_requests(id);
                }
                match event {
                    Event::Stopped(events::StoppedBody {
                        thread_id,
                        description,
                        text,
                        reason,
                        all_threads_stopped,
                        ..
                    }) => {
                        let debugger = match self.debugger.adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let all_threads_stopped = all_threads_stopped.unwrap_or_default();

                        if all_threads_stopped {
                            if let Ok(response) = debugger
                                .request::<dap::requests::Threads>(Some(ThreadsArguments {}))
                                .await
                            {
                                for thread in response.threads {
                                    fetch_stack_trace(debugger, thread.id).await;
                                }
                                select_thread_id(self, id, thread_id.unwrap_or_default(), false)
                                    .await;
                            }
                        } else if let Some(thread_id) = thread_id {
                            debugger.thread_states.insert(thread_id, reason.clone()); // TODO: dap uses "type" || "reason" here

                            fetch_stack_trace(debugger, thread_id).await;
                            // whichever thread stops is made "current" (if no previously selected thread).
                            select_thread_id(self, id, thread_id, false).await;
                        }

                        let scope = match thread_id {
                            Some(id) => format!("Thread {}", id),
                            None => "Target".to_owned(),
                        };

                        let mut status = format!("{} stopped because of {}", scope, reason);
                        if let Some(desc) = description {
                            write!(status, " {}", desc).unwrap();
                        }
                        if let Some(text) = text {
                            write!(status, " {}", text).unwrap();
                        }
                        if all_threads_stopped {
                            status.push_str(" (all threads stopped)");
                        }

                        self.set_status(status);
                    }
                    Event::Continued(events::ContinuedBody { thread_id, .. }) => {
                        let debugger = match self.debugger.adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger
                            .thread_states
                            .insert(thread_id, "running".to_owned());
                        if debugger.thread_id == Some(thread_id) {
                            debugger.resume_application();
                        }
                    }
                    Event::Thread(thread) => {
                        self.set_status(format!("Thread {}: {}", thread.thread_id, thread.reason));
                        let debugger = match self.debugger.adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger.thread_id = Some(thread.thread_id);
                        // set the stack frame for the thread
                    }
                    Event::Breakpoint(events::BreakpointBody { reason, breakpoint }) => {
                        self.handle_breakpoint_event(id, &reason, breakpoint);
                    }
                    Event::Output(events::OutputBody {
                        category, output, ..
                    }) => {
                        let prefix = match category {
                            Some(category) => {
                                if &category == "telemetry" {
                                    return false;
                                }
                                format!("Debug ({}):", category)
                            }
                            None => "Debug:".to_owned(),
                        };

                        log::info!("{}", output);
                        self.set_status(format!("{} {}", prefix, output));
                    }
                    Event::ProgressStart(body) => {
                        let status = {
                            let debugger = match self.debugger.adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_start(body)
                        };

                        self.set_status(status);
                    }
                    Event::ProgressUpdate(body) => {
                        let status = {
                            let debugger = match self.debugger.adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_update(body)
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::ProgressEnd(body) => {
                        let status = {
                            let debugger = match self.debugger.adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_end(body)
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::Initialized(_) => {
                        self.set_status("Debugger initialized...");
                        if let Err(error) = self.configure_debugger_breakpoints(id) {
                            self.set_error(|| format!("Debugger configuration failed: {error}"));
                        }
                    }
                    Event::Terminated(terminated) => {
                        let debugger = match self.debugger.adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let restart_arg = if let Some(terminated) = terminated {
                            terminated.restart
                        } else {
                            None
                        };

                        let restart_bool = restart_arg
                            .as_ref()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let disconnect_args = Some(DisconnectArguments {
                            restart: Some(restart_bool),
                            terminate_debuggee: None,
                            suspend_debuggee: None,
                        });

                        if let Err(err) = debugger.disconnect(disconnect_args).await {
                            self.set_error(|| {
                                format!(
                                "Cannot disconnect debugger upon terminated event receival {:?}",
                                err
                            )
                            });
                            return false;
                        }

                        match restart_arg {
                            Some(Value::Bool(false)) | None => {
                                self.debugger.remove_client(id);
                                self.set_status(
                                    "Terminated debugging session and disconnected debugger.",
                                );
                            }
                            Some(val) => {
                                log::info!("Attempting to restart debug session.");
                                let connection_type = match debugger.connection_type() {
                                    Some(connection_type) => connection_type,
                                    None => {
                                        self.set_error(|| "No starting request found, to be used in restarting the debugging session.");
                                        return false;
                                    }
                                };

                                let relaunch_resp = if let ConnectionType::Launch = connection_type
                                {
                                    debugger.launch(val).await
                                } else {
                                    debugger.attach(val).await
                                };

                                if let Err(err) = relaunch_resp {
                                    self.set_error(|| {
                                        format!("Failed to restart debugging session: {:?}", err)
                                    });
                                }
                            }
                        }
                    }
                    Event::Exited(resp) => {
                        let exit_code = resp.exit_code;
                        if exit_code != 0 {
                            self.set_error(|| {
                                format!(
                                "Debuggee failed to exit successfully (exit code: {exit_code})."
                            )
                            });
                        }
                    }
                    ev => {
                        log::warn!("Unhandled event {:?}", ev);
                        return false; // return early to skip render
                    }
                }
            }
            Payload::Response(_) => unreachable!(),
            Payload::Request(request) => {
                if self.debugger.adapters.get_client(id).is_none() {
                    log::debug!("Ignoring request from a removed debug adapter");
                    return false;
                }
                self.handle_debugger_request(id, request.seq, request.command, request.arguments)
                    .await;
            }
        }
        true
    }
}

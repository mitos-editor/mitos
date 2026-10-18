use crate::editor::Action;
use crate::{align_view, Align, Editor};
use dap::{self as dap, DebugAdapterId, Payload};
use editor_core::Selection;

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
    /// Consume protocol messages and transport closure through the same session owner.
    pub async fn handle_debugger_event(
        &mut self,
        id: DebugAdapterId,
        event: dap::ClientEvent,
    ) -> bool {
        match event {
            dap::ClientEvent::Message(payload) => self.handle_debugger_message(id, payload).await,
            dap::ClientEvent::Closed { error } => {
                let Some(session) = self.debugger.get_session(id) else {
                    return false;
                };
                let name = session.name().to_owned();
                let active = self
                    .debugger
                    .get_active_session()
                    .is_some_and(|session| session.id() == id);
                self.debugger.remove_session(id);
                if active {
                    match error {
                        Some(error) => {
                            self.set_error(|| format!("Debugger {name} disconnected: {error}"))
                        }
                        None => self.set_status(format!("Debugging session ended: {name}")),
                    }
                } else if let Some(error) = error {
                    log::warn!("Debugger {name} disconnected: {error}");
                }
                true
            }
        }
    }

    pub async fn handle_debugger_message(
        &mut self,
        id: DebugAdapterId,
        payload: dap::Payload,
    ) -> bool {
        use dap::{events, Event};

        if self.debugger.get_client(id).is_none() {
            log::debug!("Ignoring message from a removed debug adapter");
            return false;
        }

        let active = self
            .debugger
            .get_active_session()
            .is_some_and(|session| session.id() == id);
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
                match event {
                    Event::Stopped(event) => self.debugger_stopped(id, event),
                    Event::Continued(event) => self.debugger_continued(id, event),
                    Event::Thread(event) => self.debugger_thread_changed(id, event),
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

                        log::info!("Debug session {id}: {output}");
                        if active {
                            self.set_status(format!("{} {}", prefix, output));
                        }
                    }
                    Event::ProgressStart(body) => {
                        let status = {
                            let debugger = match self.debugger.get_session_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_start(body)
                        };

                        if active {
                            self.set_status(status);
                        }
                    }
                    Event::ProgressUpdate(body) => {
                        let status = {
                            let debugger = match self.debugger.get_session_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_update(body)
                        };

                        if active && let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::ProgressEnd(body) => {
                        let status = {
                            let debugger = match self.debugger.get_session_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_end(body)
                        };

                        if active && let Some(status) = status {
                            self.set_status(status);
                        }
                    }
                    Event::Initialized(_) => self.debugger_initialized(id),
                    Event::Terminated(event) => {
                        self.debugger_terminated(id, event.and_then(|body| body.restart));
                    }
                    Event::Exited(resp) => {
                        let exit_code = resp.exit_code;
                        if exit_code != 0 && active {
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
                self.handle_debugger_request(id, request.seq, request.command, request.arguments);
            }
        }
        true
    }
}

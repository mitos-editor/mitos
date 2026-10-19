//! Execution transitions and their asynchronous adapter follow-up requests.

use std::future::Future;

use futures_util::future::BoxFuture;

use dap::{events, requests, ConnectionType, DebugAdapterId, ThreadId};
use event::{TaskController, TaskHandle};
use serde_json::Value;

use crate::{callbacks::EditorCallbackSender, features::request::SelectionRequest, Editor};

pub(super) struct SessionEvents {
    transition: TaskController,
    current: TaskHandle,
}

impl Default for SessionEvents {
    fn default() -> Self {
        let mut transition = TaskController::new();
        let current = transition.restart();
        Self {
            transition,
            current,
        }
    }
}

#[derive(Clone)]
struct EventContext {
    id: DebugAdapterId,
    owner: EditorCallbackSender,
    lifetime: TaskHandle,
    execution: TaskHandle,
}

impl EventContext {
    fn is_current(&self, editor: &Editor) -> bool {
        !self.lifetime.is_canceled()
            && !self.execution.is_canceled()
            && self.owner.same_destination(editor.callback_sender())
            && editor.debugger.get_client(self.id).is_some()
    }

    fn is_active(&self, editor: &Editor) -> bool {
        editor
            .debugger
            .get_active_client()
            .is_some_and(|client| client.id() == self.id)
    }

    fn report_error(&self, editor: &mut Editor, message: &str, error: dap::Error) {
        if self.is_active(editor) {
            editor.set_error(|| format!("{message}: {error}"));
        } else {
            log::warn!("{message} for {}: {error}", self.id);
        }
    }

    fn spawn<T: Send + 'static>(
        self,
        future: impl Future<Output = dap::Result<T>> + Send + 'static,
        apply: impl FnOnce(&mut Editor, Self, dap::Result<T>) + Send + 'static,
    ) {
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = self.lifetime.canceled() => return,
                _ = self.execution.canceled() => return,
                result = future => result,
            };
            let owner = self.owner.clone();
            owner
                .send(move |editor| {
                    if self.is_current(editor) {
                        apply(editor, self, result);
                    }
                })
                .await;
        });
    }
}

struct StoppedState {
    threads: Vec<ThreadId>,
    selected: Option<ThreadId>,
    frames: dap::Result<Vec<dap::StackFrame>>,
}

impl super::DebuggerState {
    pub(super) fn is_terminating(&self, id: DebugAdapterId) -> bool {
        self.get_session(id)
            .is_some_and(|session| session.lifecycle.is_disconnecting())
    }
}

impl Editor {
    fn debugger_lifecycle_context(&mut self, id: DebugAdapterId) -> EventContext {
        let lifetime = self.debugger.sessions[id].events.current.clone();
        EventContext {
            id,
            owner: self.callback_sender().clone(),
            lifetime: lifetime.clone(),
            execution: lifetime,
        }
    }

    fn debugger_execution_context(&mut self, id: DebugAdapterId) -> EventContext {
        EventContext {
            execution: self.debugger.execution_handle(id),
            ..self.debugger_lifecycle_context(id)
        }
    }

    pub(crate) fn reset_debugger_events(&mut self, id: DebugAdapterId) {
        self.debugger.invalidate_execution(id);
        let state = &mut self.debugger.sessions[id].events;
        state.current = state.transition.restart();
    }

    pub(crate) fn debugger_stopped(&mut self, id: DebugAdapterId, event: events::StoppedBody) {
        if self.debugger.is_terminating(id) {
            return;
        }
        self.debugger.invalidate_execution(id);
        let Some(session) = self.debugger.get_session_mut(id) else {
            return;
        };
        let all_stopped = event.all_threads_stopped.unwrap_or(false);
        let transition = session
            .execution
            .begin_stop(event.thread_id, all_stopped, &event.reason);
        let previous = transition.previous_thread;
        let hydrate = transition.needs_frames;
        let sender = session.client.as_ref().unwrap().request_sender();
        let context = self.debugger_execution_context(id);
        if context.is_active(self) {
            let scope = event
                .thread_id
                .map_or_else(|| "Target".to_owned(), |id| format!("Thread {id}"));
            let mut status = format!("{scope} stopped because of {}", event.reason);
            for detail in [&event.description, &event.text].into_iter().flatten() {
                status.push(' ');
                status.push_str(detail);
            }
            if all_stopped {
                status.push_str(" (all threads stopped)");
            }
            self.set_status(status);
        }
        if !hydrate {
            return;
        }
        let selection = self.debugger.current_selection.clone();
        let navigation = if !context.is_active(self) || event.preserve_focus_hint.unwrap_or(false) {
            None
        } else {
            self.tree.try_get(self.tree.focus).and_then(|view| {
                self.document(view.doc).map(|doc| {
                    SelectionRequest::new(doc, view.id, context.execution.clone(), Vec::new())
                })
            })
        };
        let mode = self.mode();
        let reason = event.reason;
        let future = async move {
            let threads = if let Some(thread) = event.thread_id.filter(|_| !all_stopped) {
                vec![thread]
            } else {
                sender
                    .request::<requests::Threads>(Some(requests::ThreadsArguments {}))
                    .await?
                    .threads
                    .into_iter()
                    .map(|thread| thread.id)
                    .collect::<Vec<_>>()
            };
            let selected = if all_stopped || event.thread_id.is_none() {
                previous
                    .filter(|id| threads.contains(id))
                    .or_else(|| event.thread_id.filter(|id| threads.contains(id)))
                    .or_else(|| threads.first().copied())
            } else {
                previous.or(event.thread_id)
            };
            let frames = if let Some(thread_id) = selected {
                sender
                    .request::<requests::StackTrace>(requests::StackTraceArguments {
                        thread_id,
                        start_frame: None,
                        levels: None,
                        format: None,
                    })
                    .await
                    .map(|response| response.stack_frames)
            } else {
                Ok(Vec::new())
            };
            let stopped_threads = if all_stopped {
                threads
            } else {
                event.thread_id.or(selected).into_iter().collect()
            };
            Ok(StoppedState {
                threads: stopped_threads,
                selected,
                frames,
            })
        };
        let inspection = self.debugger.inspection_handle(id);
        let waiting = inspection.clone();
        let future = async move {
            match event::cancelable_future(future, &waiting).await {
                Some(result) => result.map(Some),
                None => Ok(None),
            }
        };
        context.spawn(future, move |editor, context, result| {
            if inspection.is_canceled() {
                return;
            }
            let state = match result {
                Ok(Some(state)) => state,
                Ok(None) => return,
                Err(error) => {
                    context.report_error(editor, "Cannot list stopped threads", error);
                    return;
                }
            };
            let session = editor.debugger.get_session_mut(id).unwrap();
            session.execution.apply_stopped_threads(
                &state.threads,
                state.selected,
                &reason,
                all_stopped,
            );
            match state.frames {
                Ok(frames) => {
                    if let Some(thread) = state.selected {
                        session.execution.select_thread(thread, frames);
                    }
                    let frame = session.current_stack_frame().cloned();
                    // Cache frames independently, but do not interrupt input or
                    // navigation that happened while the adapter was responding.
                    if context.is_active(editor)
                        && selection
                            .as_ref()
                            .is_some_and(|selection| !selection.is_canceled())
                        && editor.mode() == mode
                        && navigation.is_some_and(|target| {
                            editor.tree.focus == target.view_id() && target.get(editor).is_some()
                        })
                        && let Some(frame) = frame
                    {
                        editor.navigate_debugger_frame(id, &frame);
                    }
                }
                Err(error) => {
                    context.report_error(editor, "Cannot fetch stopped stack frames", error)
                }
            }
        });
    }

    pub(crate) fn debugger_continued(&mut self, id: DebugAdapterId, event: events::ContinuedBody) {
        self.debugger.invalidate_execution(id);
        let Some(session) = self.debugger.get_session_mut(id) else {
            return;
        };
        if event.all_threads_continued.unwrap_or(true) {
            session.resume_all_threads();
        }
        session.resume_thread(event.thread_id);
    }

    pub(crate) fn debugger_thread_changed(
        &mut self,
        id: DebugAdapterId,
        event: events::ThreadBody,
    ) {
        if event.reason == "exited" {
            self.debugger.invalidate_execution(id);
        }
        let Some(session) = self.debugger.get_session_mut(id) else {
            return;
        };
        match event.reason.as_str() {
            "started" => {
                session.execution.thread_started(event.thread_id);
            }
            "exited" => {
                session.execution.remove_thread(event.thread_id);
            }
            _ => {}
        }
    }

    pub(super) fn terminate_debugger_session(&mut self, id: DebugAdapterId) {
        let session = &self.debugger.sessions[id];
        let client = session.client.as_ref().unwrap();
        let future: BoxFuture<'static, dap::Result<()>> = if session
            .capabilities()
            .unwrap()
            .supports_terminate_request
            .unwrap_or(false)
        {
            Box::pin(client.terminate(Some(requests::TerminateArguments {
                restart: Some(false),
            })))
        } else {
            Box::pin(client.disconnect(None))
        };
        self.debugger.invalidate_execution(id);
        self.debugger.sessions[id].lifecycle.begin_disconnect();
        self.suspend_debugger_breakpoints(id);
        self.suspend_debugger_configuration(id);
        self.debugger.cancel_child_startups(id);
        self.cancel_debugger_reverse_requests(id);
        let context = self.debugger_lifecycle_context(id);
        context.spawn(future, |editor, context, result| match result {
            Ok(_) => editor.debugger.remove_session(context.id),
            Err(error) => {
                editor.debugger.sessions[context.id]
                    .lifecycle
                    .connection_retained();
                editor.resume_debugger_configuration(context.id);
                editor.resume_debugger_breakpoints(context.id);
                context.report_error(editor, "Debugger request failed", error);
            }
        });
    }

    pub(crate) fn debugger_terminated(&mut self, id: DebugAdapterId, restart: Option<Value>) {
        self.suspend_debugger_configuration(id);
        self.debugger.cancel_child_startups(id);
        self.cancel_debugger_reverse_requests(id);
        self.debugger.cancel_startup_requests(id);
        self.debugger.invalidate_execution(id);
        let Some(session) = self.debugger.get_session_mut(id) else {
            return;
        };
        session.execution.clear();
        let launch = session.launch.clone();
        let client = session.client.as_ref().unwrap();
        let restart_allowed = session.lifecycle.adapter_terminated();
        let restart = restart.filter(|value| restart_allowed && *value != Value::Bool(false));
        let disconnect = client.disconnect(Some(requests::DisconnectArguments {
            restart: Some(restart.is_some()),
            terminate_debuggee: None,
            suspend_debuggee: None,
        }));
        // Configuration and breakpoint replies from the terminated execution are stale.
        session.breakpoints = None;
        let state = &mut session.events;
        state.current = state.transition.restart();
        // Disconnect belongs to the adapter lifecycle, independent of thread
        // and frame changes, including thread-exit events during shutdown.
        let context = self.debugger_lifecycle_context(id);
        context.spawn(disconnect, move |editor, context, result| {
            if let Err(error) = result {
                context.report_error(editor, "Cannot disconnect terminated debugger", error);
                editor.debugger.remove_session(id);
                return;
            }
            let Some(restart_data) = restart else {
                let active = context.is_active(editor);
                editor.debugger.remove_session(id);
                if active {
                    editor.set_status("Terminated debugging session and disconnected debugger.");
                }
                return;
            };
            let Some(launch) = launch else {
                if context.is_active(editor) {
                    editor.set_error(|| {
                        "No starting request found to restart the debugging session."
                    });
                }
                editor.debugger.remove_session(id);
                return;
            };
            let Value::Object(mut arguments) = launch.arguments else {
                if context.is_active(editor) {
                    editor.set_error(|| {
                        "No launch or attach arguments found to restart the debugging session."
                    });
                }
                editor.debugger.remove_session(id);
                return;
            };
            // DAP forwards the adapter's opaque restart data in the original
            // launch/attach configuration, rather than replacing that configuration.
            arguments.insert("__restart".to_owned(), restart_data);
            let arguments = Value::Object(arguments);
            editor.debugger.sessions[id].launch = Some(super::DebuggerLaunch {
                request: launch.request,
                arguments: arguments.clone(),
            });
            editor.debugger.sessions[id].lifecycle.connection_retained();
            let client = editor.debugger.get_client(id).unwrap();
            let future: BoxFuture<'static, dap::Result<()>> = match launch.request {
                ConnectionType::Launch => Box::pin(client.launch(arguments)),
                ConnectionType::Attach => Box::pin(client.attach(arguments)),
            };
            let context = editor.debugger_execution_context(id);
            context.spawn(future, |editor, context, result| {
                if let Err(error) = result {
                    context.report_error(editor, "Failed to restart debugging session", error);
                    editor.debugger.remove_session(context.id);
                }
            });
        });
    }
}

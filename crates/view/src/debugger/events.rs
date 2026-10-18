//! Execution transitions and their asynchronous adapter follow-up requests.

use std::future::Future;

use futures_util::future::BoxFuture;

use dap::{events, registry::DebugAdapterId, requests, ConnectionType, ThreadId};
use event::{TaskController, TaskHandle};
use serde_json::Value;

use crate::{
    callbacks::EditorCallbackSender, features::request::SelectionRequest,
    handlers::dap::jump_to_stack_frame, Editor,
};

pub(super) struct SessionEvents {
    transition: TaskController,
    current: TaskHandle,
    terminating: bool,
}

impl Default for SessionEvents {
    fn default() -> Self {
        let mut transition = TaskController::new();
        let current = transition.restart();
        Self {
            transition,
            current,
            terminating: false,
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
        self.events.get(&id).is_some_and(|state| state.terminating)
    }
}

impl Editor {
    fn debugger_lifecycle_context(&mut self, id: DebugAdapterId) -> EventContext {
        let lifetime = self.debugger.events.entry(id).or_default().current.clone();
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
        self.debugger.invalidate_requests(id);
        let state = self.debugger.events.entry(id).or_default();
        state.current = state.transition.restart();
        state.terminating = false;
    }

    pub(crate) fn debugger_stopped(&mut self, id: DebugAdapterId, event: events::StoppedBody) {
        if self.debugger.is_terminating(id) {
            return;
        }
        self.debugger.invalidate_requests(id);
        let Some(client) = self.debugger.get_client_mut(id) else {
            return;
        };
        let all_stopped = event.all_threads_stopped.unwrap_or(false);
        let previous = client.thread_id;
        let hydrate = all_stopped
            || event.thread_id.is_none()
            || previous == event.thread_id
            || client.current_stack_frame().is_none();
        client.thread_id = previous.or(event.thread_id);
        // Invalidate displayed frames immediately, before any adapter can reply.
        if all_stopped {
            client.stack_frames.clear();
            for state in client.thread_states.values_mut() {
                *state = event.reason.clone();
            }
        } else if let Some(thread) = event.thread_id {
            client.stack_frames.remove(&thread);
        }
        if hydrate {
            client.active_frame = None;
        }
        if let Some(thread) = event.thread_id {
            client.thread_states.insert(thread, event.reason.clone());
        }
        let sender = client.request_sender();
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
        let navigation = if event.preserve_focus_hint.unwrap_or(false) {
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
        context.spawn(future, move |editor, context, result| {
            let state = match result {
                Ok(state) => state,
                Err(error) => {
                    context.report_error(editor, "Cannot list stopped threads", error);
                    return;
                }
            };
            let client = editor.debugger.get_client_mut(id).unwrap();
            if all_stopped {
                client
                    .thread_states
                    .retain(|thread, _| state.threads.contains(thread));
            }
            for thread in state.threads {
                client.thread_states.insert(thread, reason.clone());
            }
            client.thread_id = state.selected;
            match state.frames {
                Ok(frames) => {
                    client.active_frame = (!frames.is_empty()).then_some(0);
                    if let Some(thread) = state.selected {
                        client.stack_frames.insert(thread, frames);
                    }
                    let frame = client.current_stack_frame().cloned();
                    // Cache frames independently, but do not interrupt input or
                    // navigation that happened while the adapter was responding.
                    if context.is_active(editor)
                        && editor.mode() == mode
                        && navigation.is_some_and(|target| {
                            editor.tree.focus == target.view_id() && target.get(editor).is_some()
                        })
                        && let Some(frame) = frame
                    {
                        jump_to_stack_frame(editor, &frame);
                    }
                }
                Err(error) => {
                    context.report_error(editor, "Cannot fetch stopped stack frames", error)
                }
            }
        });
    }

    pub(crate) fn debugger_continued(&mut self, id: DebugAdapterId, event: events::ContinuedBody) {
        self.debugger.invalidate_requests(id);
        let Some(client) = self.debugger.get_client_mut(id) else {
            return;
        };
        if event.all_threads_continued.unwrap_or(true) {
            client.resume_application();
            client.stack_frames.clear();
            for state in client.thread_states.values_mut() {
                *state = "running".to_owned();
            }
        } else {
            client.stack_frames.remove(&event.thread_id);
            if client.thread_id == Some(event.thread_id) {
                client.resume_application();
            }
        }
        client
            .thread_states
            .insert(event.thread_id, "running".to_owned());
    }

    pub(crate) fn debugger_thread_changed(
        &mut self,
        id: DebugAdapterId,
        event: events::ThreadBody,
    ) {
        if event.reason == "exited" {
            self.debugger.invalidate_requests(id);
        }
        let Some(client) = self.debugger.get_client_mut(id) else {
            return;
        };
        match event.reason.as_str() {
            "started" => {
                client
                    .thread_states
                    .insert(event.thread_id, "running".to_owned());
            }
            "exited" => {
                client.thread_states.remove(&event.thread_id);
                client.stack_frames.remove(&event.thread_id);
                if client.thread_id == Some(event.thread_id) {
                    client.thread_id = None;
                    client.active_frame = None;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn debugger_terminated(&mut self, id: DebugAdapterId, restart: Option<Value>) {
        self.debugger.cancel_startup_requests(id);
        self.debugger.invalidate_requests(id);
        let Some(client) = self.debugger.get_client_mut(id) else {
            return;
        };
        let connection = client.connection_type();
        let starting_arguments = client.starting_request_args().cloned();
        client.resume_application();
        client.stack_frames.clear();
        client.thread_states.clear();
        let restart = restart.filter(|value| *value != Value::Bool(false));
        let disconnect = client.disconnect(Some(requests::DisconnectArguments {
            restart: Some(restart.is_some()),
            terminate_debuggee: None,
            suspend_debuggee: None,
        }));
        // Configuration and breakpoint replies from the terminated execution are stale.
        self.debugger.breakpoint_sessions.remove(&id);
        let state = self.debugger.events.entry(id).or_default();
        state.current = state.transition.restart();
        state.terminating = true;
        // Disconnect belongs to the adapter lifecycle, independent of thread
        // and frame changes, including thread-exit events during shutdown.
        let context = self.debugger_lifecycle_context(id);
        context.spawn(disconnect, move |editor, context, result| {
            if let Err(error) = result {
                context.report_error(editor, "Cannot disconnect terminated debugger", error);
                editor.debugger.remove_client(id);
                return;
            }
            let Some(restart_data) = restart else {
                let active = context.is_active(editor);
                editor.debugger.remove_client(id);
                if active {
                    editor.set_status("Terminated debugging session and disconnected debugger.");
                }
                return;
            };
            let Some(connection) = connection else {
                if context.is_active(editor) {
                    editor.set_error(|| {
                        "No starting request found to restart the debugging session."
                    });
                }
                editor.debugger.remove_client(id);
                return;
            };
            let Some(Value::Object(mut arguments)) = starting_arguments else {
                if context.is_active(editor) {
                    editor.set_error(|| {
                        "No launch or attach arguments found to restart the debugging session."
                    });
                }
                editor.debugger.remove_client(id);
                return;
            };
            // DAP forwards the adapter's opaque restart data in the original
            // launch/attach configuration, rather than replacing that configuration.
            arguments.insert("__restart".to_owned(), restart_data);
            let arguments = Value::Object(arguments);
            editor.debugger.events.get_mut(&id).unwrap().terminating = false;
            let client = editor.debugger.get_client_mut(id).unwrap();
            let future: BoxFuture<'static, dap::Result<Value>> = match connection {
                ConnectionType::Launch => Box::pin(client.launch(arguments)),
                ConnectionType::Attach => Box::pin(client.attach(arguments)),
            };
            let context = editor.debugger_execution_context(id);
            context.spawn(future, |editor, context, result| {
                if let Err(error) = result {
                    context.report_error(editor, "Failed to restart debugging session", error);
                    editor.debugger.remove_client(context.id);
                }
            });
        });
    }
}

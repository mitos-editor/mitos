//! Asynchronous debugger commands and inspection bound to a session and execution state.

use super::DebuggerState;
use crate::{callbacks::EditorCallbackSender, Editor};
use anyhow::{bail, Context as _, Result};
use dap::{requests, DebugAdapterId, RequestSender, ThreadId};
use event::{TaskController, TaskHandle};
use futures_util::future::BoxFuture;
use std::{collections::HashMap, future::Future};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RequestKind {
    Control,
    Threads,
    Variables,
    Evaluate,
    SelectThread,
}

pub(super) struct SessionRequests {
    lifetime: TaskController,
    current: TaskHandle,
    requests: HashMap<RequestKind, TaskController>,
}

impl Default for SessionRequests {
    fn default() -> Self {
        let mut lifetime = TaskController::new();
        let current = lifetime.restart();
        Self {
            lifetime,
            current,
            requests: HashMap::new(),
        }
    }
}

impl DebuggerState {
    pub(super) fn execution_handle(&mut self, id: DebugAdapterId) -> TaskHandle {
        self.sessions[id].requests.current.clone()
    }

    pub(crate) fn invalidate_requests(&mut self, id: DebugAdapterId) {
        if let Some(session) = self.sessions.get_mut(id) {
            let requests = &mut session.requests;
            requests.current = requests.lifetime.restart();
            requests.requests.clear();
        }
    }
}

/// A session snapshot retained by queued results and frontend pickers.
#[derive(Clone)]
pub struct DebuggerContext {
    id: DebugAdapterId,
    owner: EditorCallbackSender,
    lifetime: TaskHandle,
    request: TaskHandle,
    // Inspection belongs to a particular user selection; controls belong to the session.
    selection: Option<TaskHandle>,
    thread: Option<ThreadId>,
    frame: Option<usize>,
}

impl DebuggerContext {
    pub fn is_current(&self, editor: &Editor) -> bool {
        !self.lifetime.is_canceled()
            && !self.request.is_canceled()
            && self.owner.same_destination(editor.callback_sender())
            && !editor.debugger.is_terminating(self.id)
            && self.selection.as_ref().is_none_or(|selection| {
                !selection.is_canceled()
                    && editor
                        .debugger
                        .get_active_session()
                        .is_some_and(|session| session.id() == self.id)
            })
            && editor.debugger.get_session(self.id).is_some_and(|session| {
                session.selected_thread() == self.thread
                    && session.current_stack_frame().map(|frame| frame.id) == self.frame
            })
    }
}

/// An owned request. Waiting never borrows the editor, so incoming reverse requests can run.
pub struct DebuggerTask<T> {
    context: DebuggerContext,
    future: BoxFuture<'static, dap::Result<T>>,
}

impl<T: Send + 'static> DebuggerTask<T> {
    fn new(
        context: DebuggerContext,
        future: impl Future<Output = dap::Result<T>> + Send + 'static,
    ) -> Self {
        Self {
            context,
            future: Box::pin(future),
        }
    }

    pub async fn run(self) -> Option<DebuggerUpdate<T>> {
        let result = tokio::select! {
            biased;
            _ = self.context.lifetime.canceled() => return None,
            _ = self.context.request.canceled() => return None,
            _ = async {
                match &self.context.selection {
                    Some(selection) => selection.canceled().await,
                    None => std::future::pending().await,
                }
            } => return None,
            result = self.future => result,
        };
        Some(DebuggerUpdate {
            context: self.context,
            result,
        })
    }
}

/// A response must validate again when it reaches the editor, including errors.
pub struct DebuggerUpdate<T> {
    context: DebuggerContext,
    result: dap::Result<T>,
}

impl<T> DebuggerUpdate<T> {
    pub fn context(&self) -> DebuggerContext {
        self.context.clone()
    }

    pub fn into_result(self, editor: &Editor) -> Option<dap::Result<T>> {
        self.context.is_current(editor).then_some(self.result)
    }
}

pub struct DebuggerFrames {
    pub context: DebuggerContext,
    pub frames: Vec<(usize, dap::StackFrame)>,
    pub thread_state: String,
}

pub struct DebuggerScope {
    pub name: String,
    pub variables: Vec<dap::Variable>,
}

enum ControlEffect {
    ResumeThread,
    ResumeAll,
    Terminate,
    Unchanged,
}

fn control_effect(
    request: impl Future<Output = dap::Result<serde_json::Value>> + Send + 'static,
    effect: ControlEffect,
) -> BoxFuture<'static, dap::Result<ControlEffect>> {
    Box::pin(async move {
        request.await?;
        Ok(effect)
    })
}

#[derive(Clone, Copy)]
pub enum DebuggerCommand {
    Continue,
    StepIn,
    StepOut,
    Next,
    Pause(ThreadId),
    Restart,
    Terminate,
    Exceptions(bool),
}

impl Editor {
    fn debugger_context(&mut self, kind: RequestKind) -> Result<(DebuggerContext, RequestSender)> {
        let session = self
            .debugger
            .get_active_session()
            .context("Debugger is not running")?;
        anyhow::ensure!(
            session
                .client
                .as_ref()
                .is_some_and(|client| client.caps.is_some()),
            "Debugger is initializing"
        );
        let (id, thread, frame, sender) = (
            session.id(),
            session.selected_thread(),
            session.current_stack_frame().map(|frame| frame.id),
            session
                .client
                .as_ref()
                .context("Debugger is connecting")?
                .request_sender(),
        );
        anyhow::ensure!(
            !self.debugger.is_terminating(id),
            "Debugger session is disconnecting"
        );
        let owner = self.callback_sender().clone();
        let state = &mut self.debugger.sessions[id].requests;
        let context = DebuggerContext {
            id,
            owner,
            lifetime: state.current.clone(),
            request: state.requests.entry(kind).or_default().restart(),
            selection: if kind == RequestKind::Control {
                None
            } else {
                self.debugger.current_selection.clone()
            },
            thread,
            frame,
        };
        Ok((context, sender))
    }

    fn spawn_debugger_task<T: Send + 'static>(
        &self,
        task: DebuggerTask<T>,
        apply: impl FnOnce(&mut Editor, T) + Send + 'static,
    ) {
        let callbacks = self.callback_sender().clone();
        tokio::spawn(async move {
            let Some(update) = task.run().await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    let id = update.context.id;
                    match update.into_result(editor) {
                        Some(Ok(result)) => apply(editor, result),
                        Some(Err(error)) => {
                            if editor
                                .debugger
                                .get_active_session()
                                .is_some_and(|session| session.id() == id)
                            {
                                editor.set_error(|| format!("Debugger request failed: {error}"));
                            } else {
                                log::warn!("Debugger request failed for {}: {error}", id);
                            }
                        }
                        None => {}
                    }
                })
                .await;
        });
    }

    /// List threads for a frontend picker without blocking its event loop.
    pub fn debugger_threads(&mut self) -> Result<DebuggerTask<Vec<dap::Thread>>> {
        let (context, sender) = self.debugger_context(RequestKind::Threads)?;
        Ok(DebuggerTask::new(context, async move {
            Ok(sender
                .request::<requests::Threads>(Some(requests::ThreadsArguments {}))
                .await?
                .threads)
        }))
    }

    pub fn debugger_variables(&mut self) -> Result<DebuggerTask<Vec<DebuggerScope>>> {
        let (context, sender) = self.debugger_context(RequestKind::Variables)?;
        let frame_id = context
            .frame
            .context("Cannot find current stack frame to access variables")?;
        Ok(DebuggerTask::new(context, async move {
            let scopes = sender
                .request::<requests::Scopes>(requests::ScopesArguments { frame_id })
                .await?
                .scopes;
            let mut result = Vec::with_capacity(scopes.len());
            for scope in scopes {
                let variables = sender
                    .request::<requests::Variables>(requests::VariablesArguments {
                        variables_reference: scope.variables_reference,
                        filter: None,
                        start: None,
                        count: None,
                        format: None,
                    })
                    .await?
                    .variables;
                result.push(DebuggerScope {
                    name: scope.name,
                    variables,
                });
            }
            Ok(result)
        }))
    }

    pub fn debugger_evaluate(
        &mut self,
        expression: String,
    ) -> Result<DebuggerTask<requests::EvaluateResponse>> {
        let (context, sender) = self.debugger_context(RequestKind::Evaluate)?;
        let frame_id = context
            .frame
            .context("Cannot find current stack frame to access variables")?;
        Ok(DebuggerTask::new(context, async move {
            sender
                .request::<requests::Evaluate>(requests::EvaluateArguments {
                    expression,
                    frame_id: Some(frame_id),
                    context: None,
                    format: None,
                })
                .await
        }))
    }

    pub fn select_debugger_thread(
        &mut self,
        context: &DebuggerContext,
        thread: ThreadId,
    ) -> Result<()> {
        if !context.is_current(self) {
            return Ok(());
        }
        self.debugger.invalidate_requests(context.id);
        let (context, sender) = self.debugger_context(RequestKind::SelectThread)?;
        let id = context.id;
        let task = DebuggerTask::new(context, async move {
            sender
                .request::<requests::StackTrace>(requests::StackTraceArguments {
                    thread_id: thread,
                    start_frame: None,
                    levels: None,
                    format: None,
                })
                .await
        });
        self.spawn_debugger_task(task, move |editor, response| {
            let session = editor.debugger.get_session_mut(id).unwrap();
            session
                .execution
                .select_thread(thread, response.stack_frames);
            let frame = session.current_stack_frame().cloned();
            editor.debugger.invalidate_requests(id);
            if let Some(frame) = frame {
                crate::handlers::dap::jump_to_stack_frame(editor, &frame);
            }
        });
        Ok(())
    }

    pub fn debugger_stack_frames(&mut self) -> Result<DebuggerFrames> {
        let (context, _) = self.debugger_context(RequestKind::SelectThread)?;
        let session = self.debugger.get_session(context.id).unwrap();
        let thread = session
            .selected_thread()
            .context("No thread is currently active")?;
        let frames = session
            .stack_frames()
            .get(&thread)
            .context("No stack frames for the active thread")?
            .iter()
            .cloned()
            .enumerate()
            .collect();
        let state = session
            .thread_states()
            .get(&thread)
            .map(|state| state.as_str().to_owned())
            .unwrap_or_else(|| "unknown".into());
        Ok(DebuggerFrames {
            context,
            frames,
            thread_state: state,
        })
    }

    pub fn select_debugger_frame(&mut self, context: &DebuggerContext, index: usize) -> Result<()> {
        if !context.is_current(self) {
            return Ok(());
        }
        let session = self.debugger.get_session_mut(context.id).unwrap();
        let frame = session
            .execution
            .select_frame(index)
            .context("Stack frame is no longer available")?
            .clone();
        self.debugger.invalidate_requests(context.id);
        crate::handlers::dap::jump_to_stack_frame(self, &frame);
        Ok(())
    }

    /// Queue a control request against the active session. Replies cannot mutate a replacement.
    pub fn debugger_command(&mut self, command: DebuggerCommand) -> Result<()> {
        if matches!(command, DebuggerCommand::Terminate)
            && let Some(session) = self.debugger.get_active_session()
            && self.debugger.cancel_startup(session.id())
        {
            return Ok(());
        }
        anyhow::ensure!(
            !self
                .debugger
                .get_active_session()
                .is_some_and(|session| self.debugger.is_starting(session.id())),
            "Selected debugging session is still starting"
        );
        let client = self
            .debugger
            .get_active_client()
            .context("Debugger is not running")?;
        if matches!(command, DebuggerCommand::Restart) {
            anyhow::ensure!(
                client
                    .capabilities()
                    .supports_restart_request
                    .unwrap_or(false),
                "Debugger does not support session restarts"
            );
            anyhow::ensure!(
                client.starting_request_args().is_some(),
                "No arguments found with which to restart the session"
            );
        }
        let thread = self
            .debugger
            .get_active_session()
            .unwrap()
            .selected_thread();
        if matches!(
            command,
            DebuggerCommand::Continue
                | DebuggerCommand::StepIn
                | DebuggerCommand::StepOut
                | DebuggerCommand::Next
        ) && thread.is_none()
        {
            bail!("Currently active thread is not stopped. Switch the thread.");
        }
        let id = client.id();
        self.debugger.invalidate_requests(id);
        let (context, _) = self.debugger_context(RequestKind::Control)?;
        let client = self.debugger.get_client_mut(id).unwrap();
        let future: BoxFuture<'static, dap::Result<ControlEffect>> = match command {
            DebuggerCommand::Continue => {
                let response = client.continue_thread(thread.unwrap());
                Box::pin(async move {
                    Ok(if response.await?.all_threads_continued.unwrap_or(true) {
                        ControlEffect::ResumeAll
                    } else {
                        ControlEffect::ResumeThread
                    })
                })
            }
            DebuggerCommand::StepIn => {
                control_effect(client.step_in(thread.unwrap()), ControlEffect::ResumeThread)
            }
            DebuggerCommand::StepOut => control_effect(
                client.step_out(thread.unwrap()),
                ControlEffect::ResumeThread,
            ),
            DebuggerCommand::Next => {
                control_effect(client.next(thread.unwrap()), ControlEffect::ResumeThread)
            }
            DebuggerCommand::Pause(thread) => {
                control_effect(client.pause(thread), ControlEffect::Unchanged)
            }
            DebuggerCommand::Restart => control_effect(client.restart(), ControlEffect::ResumeAll),
            DebuggerCommand::Terminate
                if client
                    .capabilities()
                    .supports_terminate_request
                    .unwrap_or(false) =>
            {
                control_effect(
                    client.terminate(Some(requests::TerminateArguments {
                        restart: Some(false),
                    })),
                    ControlEffect::Terminate,
                )
            }
            DebuggerCommand::Terminate => {
                control_effect(client.disconnect(None), ControlEffect::Terminate)
            }
            DebuggerCommand::Exceptions(enabled) => {
                let filters = if enabled {
                    client
                        .capabilities()
                        .exception_breakpoint_filters
                        .as_ref()
                        .map(|filters| filters.iter().map(|filter| filter.filter.clone()).collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                control_effect(
                    client.set_exception_breakpoints(filters),
                    ControlEffect::Unchanged,
                )
            }
        };
        self.spawn_debugger_task(DebuggerTask::new(context, future), move |editor, effect| {
            match effect {
                ControlEffect::ResumeThread => editor
                    .debugger
                    .get_session_mut(id)
                    .unwrap()
                    .resume_thread(thread.unwrap()),
                ControlEffect::ResumeAll => editor
                    .debugger
                    .get_session_mut(id)
                    .unwrap()
                    .resume_all_threads(),
                ControlEffect::Terminate => {
                    editor.debugger.remove_session(id);
                    return;
                }
                ControlEffect::Unchanged => return,
            }
            editor.debugger.invalidate_requests(id);
        });
        Ok(())
    }

    pub fn pause_debugger_thread(
        &mut self,
        context: &DebuggerContext,
        thread: ThreadId,
    ) -> Result<()> {
        if context.is_current(self) {
            self.debugger_command(DebuggerCommand::Pause(thread))
        } else {
            Ok(())
        }
    }
}

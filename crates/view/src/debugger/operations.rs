//! Asynchronous debugger commands and inspection bound to a session and execution state.

use super::DebuggerState;
use crate::{callbacks::EditorCallbackSender, Editor};
use anyhow::{bail, Context as _, Result};
use dap::{requests, DebugAdapterId, RequestSender, ThreadId};
use event::{TaskController, TaskHandle};
use futures_util::future::BoxFuture;
use std::{collections::HashMap, future::Future};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum RequestKind {
    Control,
    Threads,
    Variables,
    Evaluate,
    SelectThread,
    Configuration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestScope {
    Session,
    Execution,
    Inspection,
}

impl RequestKind {
    fn scope(self) -> RequestScope {
        match self {
            Self::Configuration => RequestScope::Session,
            Self::Control => RequestScope::Execution,
            _ => RequestScope::Inspection,
        }
    }
}

struct RequestLifetime {
    owner: TaskController,
    current: TaskHandle,
}
impl Default for RequestLifetime {
    fn default() -> Self {
        let mut owner = TaskController::new();
        let current = owner.restart();
        Self { owner, current }
    }
}
impl RequestLifetime {
    fn restart(&mut self) {
        self.current = self.owner.restart();
    }
}

#[derive(Default)]
pub(super) struct SessionRequests {
    session: RequestLifetime,
    execution: RequestLifetime,
    selection: RequestLifetime,
    requests: HashMap<RequestKind, TaskController>,
    pub(super) inspection: Option<std::sync::Arc<super::inspection::InspectionCache>>,
}

impl DebuggerState {
    pub(super) fn execution_handle(&mut self, id: DebugAdapterId) -> TaskHandle {
        self.sessions[id].requests.execution.current.clone()
    }

    pub(super) fn inspection_handle(&self, id: DebugAdapterId) -> TaskHandle {
        self.sessions[id].requests.selection.current.clone()
    }

    pub(crate) fn invalidate_inspection(&mut self, id: DebugAdapterId) {
        if let Some(session) = self.sessions.get_mut(id) {
            let requests = &mut session.requests;
            requests.selection.restart();
            requests
                .requests
                .retain(|kind, _| kind.scope() != RequestScope::Inspection);
            requests.inspection = None;
        }
    }

    pub(crate) fn invalidate_execution(&mut self, id: DebugAdapterId) {
        self.invalidate_inspection(id);
        if let Some(session) = self.sessions.get_mut(id) {
            session.requests.execution.restart();
            session
                .requests
                .requests
                .retain(|kind, _| kind.scope() == RequestScope::Session);
        }
    }
}

/// A guarded session snapshot. Only inspection follows the selected thread and frame.
#[derive(Clone)]
pub struct DebuggerContext {
    pub(super) id: DebugAdapterId,
    owner: EditorCallbackSender,
    pub(super) lifetimes: Vec<TaskHandle>,
    request: TaskHandle,
    scope: RequestScope,
    thread: Option<ThreadId>,
    pub(super) frame: Option<usize>,
}

impl DebuggerContext {
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.lifetimes.iter().all(|handle| !handle.is_canceled())
            && !self.request.is_canceled()
            && self.owner.same_destination(editor.callback_sender())
            && (self.scope == RequestScope::Session || !editor.debugger.is_terminating(self.id))
            && editor.debugger.get_session(self.id).is_some_and(|session| {
                self.scope != RequestScope::Inspection
                    || (editor
                        .debugger
                        .get_active_session()
                        .is_some_and(|active| active.id() == self.id)
                        && session.selected_thread() == self.thread
                        && session.current_stack_frame().map(|frame| frame.id) == self.frame)
            })
    }

    async fn canceled(&self) {
        use futures_util::{stream::FuturesUnordered, StreamExt};
        let mut canceled: FuturesUnordered<_> = self
            .lifetimes
            .iter()
            .map(|handle| handle.canceled())
            .collect();
        canceled.push(self.request.canceled());
        canceled.next().await;
    }
}

/// An owned request. Waiting never borrows the editor, so incoming reverse requests can run.
pub struct DebuggerTask<T> {
    context: DebuggerContext,
    future: BoxFuture<'static, dap::Result<T>>,
}

impl<T: Send + 'static> DebuggerTask<T> {
    pub(super) fn new(
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
            _ = self.context.canceled() => return None,
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

enum ControlEffect {
    ResumeThread,
    ResumeAll,
    Unchanged,
}

fn control_effect<T: Send + 'static>(
    request: impl Future<Output = dap::Result<T>> + Send + 'static,
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
    pub(super) fn debugger_context(
        &mut self,
        kind: RequestKind,
    ) -> Result<(DebuggerContext, RequestSender)> {
        let id = self
            .debugger
            .get_active_session()
            .context("Debugger is not running")?
            .id();
        self.debugger_context_for(id, kind)
    }

    pub(super) fn debugger_context_for(
        &mut self,
        id: DebugAdapterId,
        kind: RequestKind,
    ) -> Result<(DebuggerContext, RequestSender)> {
        let session = self
            .debugger
            .get_session(id)
            .context("Debugger is not running")?;
        anyhow::ensure!(session.capabilities().is_some(), "Debugger is initializing");
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
        let scope = kind.scope();
        let state = &mut self.debugger.sessions[id].requests;
        let mut lifetimes = vec![state.session.current.clone()];
        if scope != RequestScope::Session {
            lifetimes.push(state.execution.current.clone());
        }
        if scope == RequestScope::Inspection {
            lifetimes.push(state.selection.current.clone());
            if let Some(selection) = &self.debugger.current_selection {
                lifetimes.push(selection.clone());
            }
        }
        let context = DebuggerContext {
            id,
            owner,
            lifetimes,
            request: state.requests.entry(kind).or_default().restart(),
            scope,
            thread,
            frame,
        };
        Ok((context, sender))
    }

    pub(super) fn spawn_debugger_task<T: Send + 'static>(
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
        self.debugger.invalidate_inspection(context.id);
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
            editor.debugger.invalidate_inspection(id);
            if let Some(frame) = frame {
                editor.navigate_debugger_frame(id, &frame);
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
        self.debugger.invalidate_inspection(context.id);
        self.navigate_debugger_frame(context.id, &frame);
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
        let session = self
            .debugger
            .get_active_session()
            .context("Debugger is not running")?;
        session.lifecycle.ensure_command_available()?;
        let capabilities = session.capabilities().context("Debugger is initializing")?;
        if matches!(command, DebuggerCommand::Restart) {
            anyhow::ensure!(
                capabilities.supports_restart_request.unwrap_or(false),
                "Debugger does not support session restarts"
            );
            anyhow::ensure!(
                session.launch.is_some(),
                "No arguments found with which to restart the session"
            );
        }
        if matches!(command, DebuggerCommand::Terminate) {
            let id = session.id();
            self.terminate_debugger_session(id);
            return Ok(());
        }
        if let DebuggerCommand::Exceptions(enabled) = command {
            return self.configure_debugger_exceptions(session.id(), enabled);
        }
        let thread = session.selected_thread();
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
        let id = session.id();
        let client = session.client.as_ref().context("Debugger is connecting")?;
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
            DebuggerCommand::Restart => control_effect(
                client.restart(session.launch.as_ref().unwrap().arguments.clone()),
                ControlEffect::ResumeAll,
            ),
            DebuggerCommand::Terminate => unreachable!("shutdown handled by the lifecycle owner"),
            DebuggerCommand::Exceptions(_) => {
                unreachable!("configuration has its own request lifetime")
            }
        };
        self.debugger.invalidate_execution(id);
        let (context, _) = self.debugger_context(RequestKind::Control)?;
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
                ControlEffect::Unchanged => return,
            }
            editor.debugger.invalidate_execution(id);
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

impl SessionRequests {
    pub(super) fn cancel_configuration(&mut self) {
        if let Some(request) = self.requests.get_mut(&RequestKind::Configuration) {
            request.cancel();
        }
    }
}

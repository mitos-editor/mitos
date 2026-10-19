//! Request lifetimes, cancellation, and checked delivery to the owning editor.
use super::DebuggerState;
use crate::{callbacks::EditorCallbackSender, Editor};
use anyhow::{Context as _, Result};
use dap::{DebugAdapterId, RequestSender, ThreadId};
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
    RefreshStack,
    Source,
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
            Self::Control | Self::RefreshStack => RequestScope::Execution,
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
    pub(super) request: TaskHandle,
    scope: RequestScope,
    pub(super) thread: Option<ThreadId>,
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
        self.spawn_debugger_task_if(task, |_| true, apply);
    }

    /// Check frontend navigation before delivering either a value or an error.
    pub(super) fn spawn_debugger_task_if<T: Send + 'static>(
        &self,
        task: DebuggerTask<T>,
        current: impl FnOnce(&Editor) -> bool + Send + 'static,
        apply: impl FnOnce(&mut Editor, T) + Send + 'static,
    ) {
        let callbacks = self.callback_sender().clone();

        tokio::spawn(async move {
            let Some(update) = task.run().await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !current(editor) {
                        return;
                    }
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
}

impl SessionRequests {
    pub(super) fn cancel(&mut self, kind: RequestKind) {
        if let Some(request) = self.requests.get_mut(&kind) {
            request.cancel();
        }
    }

    pub(super) fn cancel_configuration(&mut self) {
        if let Some(request) = self.requests.get_mut(&RequestKind::Configuration) {
            request.cancel();
        }
    }
}

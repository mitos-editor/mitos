//! Lazy, session-owned variable data for one paused frame.

use super::operations::RequestKind;
use super::{DebuggerContext, DebuggerTask};
use crate::Editor;
use anyhow::{Context as _, Result};
use dap::{requests, DebugAdapterId, RequestSender};
use event::{TaskController, TaskHandle};
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::OnceCell;

/// A scope or value in the variable browser. References remain private to its page.
#[derive(Clone)]
pub struct DebuggerVariable {
    pub name: String,
    pub value: Option<String>,
    pub ty: Option<String>,
    pub expensive: bool,
    reference: usize,
}

impl DebuggerVariable {
    pub fn is_expandable(&self) -> bool {
        self.reference != 0
    }
}

#[derive(Clone)]
struct VariableKey {
    name: String,
    occurrence: usize,
}

/// A page of values bound to its original editor, session, stop, and frame selection.
#[derive(Clone)]
pub struct DebuggerVariables {
    context: DebuggerContext,
    data: TaskHandle,
    entries: Arc<[DebuggerVariable]>,
    path: Vec<VariableKey>,
}

impl DebuggerVariables {
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.can_refresh(editor) && !self.data.is_canceled()
    }

    /// Data invalidation can be refreshed while the original frame is still selected.
    pub fn can_refresh(&self, editor: &Editor) -> bool {
        self.context.is_current(editor)
    }

    pub fn entries(&self) -> &[DebuggerVariable] {
        &self.entries
    }

    fn task_context(&self) -> DebuggerContext {
        let mut context = self.context.clone();
        context.lifetimes.push(self.data.clone());
        context
    }

    fn key(&self, index: usize) -> VariableKey {
        let name = self.entries[index].name.clone();
        let occurrence = self.entries[..index]
            .iter()
            .filter(|entry| entry.name == name)
            .count();
        VariableKey { name, occurrence }
    }
}

type VariableCell = Arc<OnceCell<Arc<[DebuggerVariable]>>>;

struct CachedValues {
    lifetime: TaskController,
    current: TaskHandle,
    variables: HashMap<usize, VariableCell>,
}

pub(super) struct InspectionCache {
    sender: RequestSender,
    values: Mutex<CachedValues>,
}

impl InspectionCache {
    fn new(sender: RequestSender) -> Self {
        let mut lifetime = TaskController::new();
        let current = lifetime.restart();
        Self {
            sender,
            values: Mutex::new(CachedValues {
                lifetime,
                current,
                variables: HashMap::new(),
            }),
        }
    }

    pub(super) fn invalidate(&self) {
        let mut values = self.values.lock();
        values.current = values.lifetime.restart();
        values.variables.clear();
    }

    fn current(&self) -> TaskHandle {
        self.values.lock().current.clone()
    }

    async fn scopes(
        &self,
        context: DebuggerContext,
        data: TaskHandle,
    ) -> dap::Result<DebuggerVariables> {
        let frame_id = context.frame.expect("inspection requires a frame");
        let scopes = self
            .sender
            .request::<requests::Scopes>(requests::ScopesArguments { frame_id })
            .await?
            .scopes;
        let entries = scopes
            .into_iter()
            .map(|scope| DebuggerVariable {
                name: scope.name,
                value: None,
                ty: None,
                expensive: scope.expensive,
                reference: scope.variables_reference,
            })
            .collect();
        Ok(DebuggerVariables {
            context,
            data,
            entries,
            path: Vec::new(),
        })
    }

    async fn variables(
        &self,
        reference: usize,
        data: &TaskHandle,
    ) -> dap::Result<Arc<[DebuggerVariable]>> {
        // Requests share initialization, but never hold the map lock across an await.
        // Invalidation detaches old cells and cancels their consumers.
        let cell = {
            let mut values = self.values.lock();
            if data.is_canceled() {
                return Err(anyhow::anyhow!("Debugger values were invalidated").into());
            }
            values.variables.entry(reference).or_default().clone()
        };
        let variables = cell
            .get_or_try_init(|| async {
                let variables = self
                    .sender
                    .request::<requests::Variables>(requests::VariablesArguments {
                        variables_reference: reference,
                        filter: None,
                        start: None,
                        count: None,
                        format: None,
                    })
                    .await?
                    .variables;
                Ok::<Arc<[DebuggerVariable]>, dap::Error>(
                    variables
                        .into_iter()
                        .map(|variable| DebuggerVariable {
                            name: variable.name,
                            value: Some(variable.value),
                            ty: variable.ty,
                            expensive: false,
                            reference: variable.variables_reference,
                        })
                        .collect(),
                )
            })
            .await?;
        Ok(variables.clone())
    }

    async fn children(
        &self,
        page: &DebuggerVariables,
        index: usize,
    ) -> dap::Result<DebuggerVariables> {
        let entries = self
            .variables(page.entries[index].reference, &page.data)
            .await?;
        let mut path = page.path.clone();
        path.push(page.key(index));
        Ok(DebuggerVariables {
            context: page.context.clone(),
            data: page.data.clone(),
            entries,
            path,
        })
    }
}

impl Editor {
    pub(super) fn debugger_inspection_cache(
        &mut self,
        id: DebugAdapterId,
        sender: RequestSender,
    ) -> Arc<InspectionCache> {
        self.debugger.sessions[id]
            .requests
            .inspection
            .get_or_insert_with(|| Arc::new(InspectionCache::new(sender)))
            .clone()
    }

    /// Load scope names without fetching any scope's values, including expensive scopes.
    pub fn debugger_variables(&mut self) -> Result<DebuggerTask<DebuggerVariables>> {
        let (context, sender) = self.debugger_context(RequestKind::Variables)?;
        context
            .frame
            .context("Cannot find current stack frame to access variables")?;
        let cache = self.debugger_inspection_cache(context.id, sender);
        cache.invalidate();
        let data = cache.current();
        let mut task_context = context.clone();
        task_context.lifetimes.push(data.clone());
        Ok(DebuggerTask::new(task_context, async move {
            cache.scopes(context, data).await
        }))
    }

    /// Expand a value from a current page. Raw adapter references cannot cross sessions.
    pub fn debugger_variable_children(
        &self,
        page: &DebuggerVariables,
        index: usize,
    ) -> Result<DebuggerTask<DebuggerVariables>> {
        anyhow::ensure!(
            page.is_current(self),
            "Debugger variables are no longer current"
        );
        let entry = page
            .entries
            .get(index)
            .context("Unknown debugger variable")?;
        anyhow::ensure!(entry.is_expandable(), "Debugger variable has no children");
        let cache = self.debugger.sessions[page.context.id]
            .requests
            .inspection
            .as_ref()
            .context("Debugger variable inspection is unavailable")?
            .clone();
        let page = page.clone();
        Ok(DebuggerTask::new(page.task_context(), async move {
            cache.children(&page, index).await
        }))
    }

    /// Rebuild only the open ancestry. Missing children return the nearest surviving parent.
    pub fn debugger_refresh_variables(
        &self,
        page: &DebuggerVariables,
    ) -> Result<DebuggerTask<Vec<DebuggerVariables>>> {
        anyhow::ensure!(
            page.can_refresh(self),
            "Debugger frame is no longer current"
        );
        let cache = self.debugger.sessions[page.context.id]
            .requests
            .inspection
            .as_ref()
            .context("Debugger variable inspection is unavailable")?
            .clone();
        let context = page.context.clone();
        let data = cache.current();
        let path = page.path.clone();
        let mut task_context = context.clone();
        task_context.lifetimes.push(data.clone());
        Ok(DebuggerTask::new(task_context, async move {
            let mut pages = vec![cache.scopes(context, data).await?];
            for key in path {
                let parent = pages.last().unwrap();
                let Some((index, entry)) = parent
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.name == key.name)
                    .nth(key.occurrence)
                else {
                    break;
                };
                if !entry.is_expandable() {
                    break;
                }
                let child = cache.children(parent, index).await?;
                pages.push(child);
            }
            Ok(pages)
        }))
    }

    pub(crate) fn debugger_invalidated(
        &mut self,
        id: DebugAdapterId,
        event: dap::events::InvalidatedBody,
    ) {
        let Some(session) = self.debugger.get_session(id) else {
            return;
        };
        // A frame scope takes precedence over threadId in the protocol.
        let matches = match event.stack_frame_id {
            Some(frame) => Some(frame) == session.current_stack_frame().map(|frame| frame.id),
            None => event
                .thread_id
                .is_none_or(|thread| Some(thread) == session.selected_thread()),
        };
        if self.debugger.is_terminating(id) || !matches {
            return;
        }
        let areas = event.areas.unwrap_or_default();
        let all = areas.is_empty()
            || areas
                .iter()
                .any(|area| !matches!(area.as_str(), "variables" | "stacks" | "threads"));
        if (all || areas.iter().any(|area| area == "variables"))
            && let Some(cache) = &session.requests.inspection
        {
            cache.invalidate();
        }
        if all
            || areas
                .iter()
                .any(|area| area == "stacks" || area == "threads")
        {
            self.refresh_debugger_stack(id);
        }
    }
}

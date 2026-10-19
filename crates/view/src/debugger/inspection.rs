//! Lazy, session-owned variable data for one paused frame.

use super::operations::RequestKind;
use super::{DebuggerContext, DebuggerTask};
use crate::Editor;
use anyhow::{Context as _, Result};
use dap::{requests, RequestSender};
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

/// A page of values bound to its original editor, session, stop, and frame selection.
#[derive(Clone)]
pub struct DebuggerVariables {
    context: DebuggerContext,
    entries: Arc<[DebuggerVariable]>,
}

impl DebuggerVariables {
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.context.is_current(editor)
    }

    pub fn entries(&self) -> &[DebuggerVariable] {
        &self.entries
    }
}

type VariableCell = Arc<OnceCell<Arc<[DebuggerVariable]>>>;

pub(super) struct InspectionCache {
    sender: RequestSender,
    variables: Mutex<HashMap<usize, VariableCell>>,
}

impl InspectionCache {
    async fn variables(&self, reference: usize) -> dap::Result<Arc<[DebuggerVariable]>> {
        // The map lock only protects cell lookup. Concurrent expansions share the
        // same initialization; failed or canceled requests can be retried.
        let cell = self.variables.lock().entry(reference).or_default().clone();
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
}

impl Editor {
    /// Load scope names without fetching any scope's values, including expensive scopes.
    pub fn debugger_variables(&mut self) -> Result<DebuggerTask<DebuggerVariables>> {
        let (context, sender) = self.debugger_context(RequestKind::Variables)?;
        let frame = context
            .frame
            .context("Cannot find current stack frame to access variables")?;
        let cache = Arc::new(InspectionCache {
            sender,
            variables: Mutex::new(HashMap::new()),
        });
        // A new browser refreshes values even if an evaluation changed the debuggee.
        self.debugger.sessions[context.id].requests.inspection = Some(cache.clone());
        let page_context = context.clone();
        Ok(DebuggerTask::new(context, async move {
            let scopes = cache
                .sender
                .request::<requests::Scopes>(requests::ScopesArguments { frame_id: frame })
                .await?
                .scopes;
            let entries = scopes
                .iter()
                .map(|scope| DebuggerVariable {
                    name: scope.name.clone(),
                    value: None,
                    ty: None,
                    expensive: scope.expensive,
                    reference: scope.variables_reference,
                })
                .collect();
            Ok(DebuggerVariables {
                context: page_context,
                entries,
            })
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
        let reference = entry.reference;
        let context = page.context.clone();
        let cache = self.debugger.sessions[page.context.id]
            .requests
            .inspection
            .as_ref()
            .context("Debugger variable inspection is unavailable")?
            .clone();
        Ok(DebuggerTask::new(context.clone(), async move {
            let entries = cache.variables(reference).await?;
            Ok(DebuggerVariables { context, entries })
        }))
    }
}

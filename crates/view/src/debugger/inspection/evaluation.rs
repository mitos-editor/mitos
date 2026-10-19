//! Evaluation results are snapshots: refreshing must never execute an expression again.
use super::{
    child_count, DebuggerContext, DebuggerVariable, DebuggerVariables, InspectionCache, Refresh,
};
use crate::Editor;
use dap::requests;
use std::sync::Arc;

/// A displayed evaluation and its privately owned, frame-bound adapter reference.
pub struct DebuggerEvaluation {
    pub result: String,
    values: DebuggerVariables,
}

impl DebuggerEvaluation {
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.values.is_current(editor)
    }
    pub fn is_expandable(&self) -> bool {
        self.values.entries[0].is_expandable()
    }

    /// Use the existing variable browser without exposing a raw adapter reference.
    pub fn into_variables(self, editor: &Editor) -> Option<DebuggerVariables> {
        (self.is_current(editor) && self.is_expandable()).then_some(self.values)
    }
}

impl InspectionCache {
    pub(in crate::debugger) fn evaluation(
        &self,
        context: DebuggerContext,
        expression: String,
        response: requests::EvaluateResponse,
    ) -> DebuggerEvaluation {
        let value = DebuggerVariable {
            name: expression,
            value: Some(response.result.clone()),
            ty: response.ty,
            expensive: false,
            reference: response.variables_reference,
            children: child_count(response.named_variables, response.indexed_variables),
        };
        DebuggerEvaluation {
            result: response.result,
            values: DebuggerVariables {
                context,
                data: self.current(),
                entries: Arc::from([value]),
                path: Vec::new(),
                container: None,
                total: None,
                more: false,
                refresh: Refresh::Snapshot,
            },
        }
    }
}

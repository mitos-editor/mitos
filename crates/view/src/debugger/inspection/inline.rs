//! Bounded background inspection for inline labels, sharing browser scope and variable caches.

use super::{DebuggerVariable, InspectionCache, PAGE_SIZE};
use event::TaskHandle;
use std::cmp::Reverse;

impl InspectionCache {
    /// Read bounded pages from cheap scopes, preferring the selected frame's lexical scope.
    /// Inline labels never expand objects or walk expensive scopes.
    pub(in crate::debugger) async fn inline_locals(
        &self,
        frame: &dap::StackFrame,
        data: TaskHandle,
    ) -> dap::Result<Vec<DebuggerVariable>> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let scopes = match tokio::time::timeout_at(deadline, self.scope_data(frame.id)).await {
            Ok(result) => result?,
            Err(_) => return Ok(Vec::new()),
        };
        let mut locals = Vec::new();
        for scope in scopes_for_frame(&scopes, frame).into_iter().take(8) {
            let values = match tokio::time::timeout_at(
                deadline,
                self.variables(scope.variables_reference, 0, &data),
            )
            .await
            {
                Ok(Ok(values)) => values,
                Ok(Err(error)) => {
                    log::debug!("Inline scope lookup failed: {error}");
                    continue;
                }
                Err(_) => break,
            };
            for value in values.iter() {
                if !locals
                    .iter()
                    .any(|local: &DebuggerVariable| local.name == value.name)
                {
                    locals.push(value.clone());
                }
                if locals.len() == PAGE_SIZE {
                    return Ok(locals);
                }
            }
        }
        Ok(locals)
    }
}

type Position = (usize, usize);

fn scopes_for_frame<'a>(scopes: &'a [dap::Scope], frame: &dap::StackFrame) -> Vec<&'a dap::Scope> {
    let position = (frame.line, frame.column);
    let mut scopes: Vec<_> = scopes
        .iter()
        .filter(|scope| !scope.expensive && scope.variables_reference != 0)
        .filter_map(|scope| {
            let range = scope_range(scope, frame);
            if range.is_some_and(|(start, end)| {
                if frame.column == 0 {
                    frame.line < start.0 || frame.line > end.0
                } else {
                    position < start || position >= end
                }
            }) {
                return None;
            }
            Some((scope, range))
        })
        .collect();
    // Prefer the narrowest containing scope. Preserve adapter order when no range is known.
    scopes.sort_by_key(|(_, range)| {
        range.map_or(
            (usize::MAX, Reverse((0, 0)), (usize::MAX, usize::MAX)),
            |(start, end)| (end.0 - start.0, Reverse(start), end),
        )
    });
    scopes.into_iter().map(|(scope, _)| scope).collect()
}

fn scope_range(scope: &dap::Scope, frame: &dap::StackFrame) -> Option<(Position, Position)> {
    if let (Some(source), Some(frame_source)) = (&scope.source, &frame.source) {
        // Locations in another source do not describe the selected frame's lexical extent.
        if source
            .path
            .as_ref()
            .zip(frame_source.path.as_ref())
            .is_some_and(|(a, b)| a != b)
            || source
                .source_reference
                .filter(|reference| *reference > 0)
                .zip(
                    frame_source
                        .source_reference
                        .filter(|reference| *reference > 0),
                )
                .is_some_and(|(a, b)| a != b)
        {
            return None;
        }
    }
    let start = (scope.line?, scope.column.unwrap_or(1).max(1));
    let end = (
        scope.end_line?,
        scope
            .end_column
            .filter(|column| *column > 0)
            .unwrap_or(usize::MAX),
    );
    (start.0 > 0 && start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_columns_and_locations_in_other_sources_do_not_hide_available_scopes() {
        let mut frame: dap::StackFrame = serde_json::from_value(json!({
            "id":1, "name":"main", "line":6, "column":0, "source":{"path":"current.rs"}
        }))
        .unwrap();
        let scopes: Vec<dap::Scope> = serde_json::from_value(json!([
            {"name":"outside","variablesReference":1,"expensive":false,"line":8,"endLine":10},
            {"name":"inner","variablesReference":2,"expensive":false,"line":6,"column":4,"endLine":6,"endColumn":20},
            {"name":"foreign","variablesReference":3,"expensive":false,"source":{"path":"other.rs"},"line":8,"endLine":10},
            {"name":"expensive","variablesReference":4,"expensive":true},
            {"name":"empty","variablesReference":0,"expensive":false}
        ])).unwrap();
        let names = |frame: &dap::StackFrame| {
            scopes_for_frame(&scopes, frame)
                .into_iter()
                .map(|scope| scope.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&frame), ["inner", "foreign"]);
        frame.column = 1;
        assert_eq!(names(&frame), ["foreign"]);
    }

    #[test]
    fn nested_scopes_with_the_same_line_span_prefer_the_inner_columns() {
        let frame: dap::StackFrame = serde_json::from_value(json!({
            "id":1, "name":"main", "line":6, "column":1
        }))
        .unwrap();
        let scopes: Vec<dap::Scope> = serde_json::from_value(json!([
            {"name":"outer","variablesReference":1,"expensive":false,"line":1,"column":1,"endLine":10,"endColumn":80},
            {"name":"inner","variablesReference":2,"expensive":false,"line":1,"column":5,"endLine":10,"endColumn":20}
        ])).unwrap();
        let names: Vec<_> = scopes_for_frame(&scopes, &frame)
            .into_iter()
            .map(|scope| scope.name.as_str())
            .collect();
        assert_eq!(names, ["inner", "outer"]);
    }
}

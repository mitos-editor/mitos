//! Ordered breakpoint synchronization, independent of thread and frame inspection.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use dap::{registry::DebugAdapterId, requests, SourceBreakpoint};
use event::{TaskController, TaskHandle};

use super::{validate_capabilities, Breakpoint, BreakpointId, BreakpointProperty};
use crate::Editor;

#[derive(Default)]
struct SourceSync {
    request: TaskController,
    dirty: bool,
}

pub(super) struct BreakpointSession {
    _lifetime: TaskController,
    current: TaskHandle,
    sources: HashMap<PathBuf, SourceSync>,
    configuring: bool,
}

impl Default for BreakpointSession {
    fn default() -> Self {
        let mut lifetime = TaskController::new();
        let current = lifetime.restart();
        Self {
            _lifetime: lifetime,
            current,
            sources: HashMap::new(),
            configuring: false,
        }
    }
}

fn source_breakpoints(
    breakpoints: &[Breakpoint],
    caps: Option<&dap::DebuggerCapabilities>,
) -> Result<Vec<SourceBreakpoint>> {
    breakpoints
        .iter()
        .map(|breakpoint| {
            if let Some(caps) = caps {
                validate_capabilities(
                    caps,
                    breakpoint.condition.is_some(),
                    breakpoint.hit_condition.is_some(),
                    breakpoint.log_message.is_some(),
                )?;
            }
            Ok(SourceBreakpoint {
                line: breakpoint.line + 1,
                column: breakpoint.column,
                condition: breakpoint.condition.clone(),
                hit_condition: breakpoint.hit_condition.clone(),
                log_message: breakpoint.log_message.clone(),
            })
        })
        .collect()
}

fn apply_response(
    breakpoints: &mut [Breakpoint],
    identities: &[BreakpointId],
    response: Vec<dap::Breakpoint>,
) {
    let mut by_identity: HashMap<_, _> = breakpoints
        .iter_mut()
        .map(|breakpoint| (breakpoint.identity, breakpoint))
        .collect();
    for (identity, response) in identities.iter().zip(response) {
        let Some(breakpoint) = by_identity.get_mut(identity) else {
            continue;
        };
        breakpoint.id = response.id;
        breakpoint.verified = response.verified;
        breakpoint.message = response.message;
        // Adapters may omit relocation; retain the requested source line in that case.
        if let Some(line) = response.line {
            breakpoint.line = line.saturating_sub(1);
        }
        breakpoint.column = response.column;
    }
}

impl Editor {
    /// Update the stored model immediately and synchronize the active adapter asynchronously.
    pub fn toggle_breakpoints(&mut self, path: PathBuf, lines: &[usize]) -> Result<()> {
        self.debugger.toggle_breakpoints(path.clone(), lines);
        if let Some(client) = self.debugger.get_active_client() {
            self.sync_debugger_breakpoints(client.id(), path)?;
        }
        Ok(())
    }

    pub fn set_breakpoint_property(
        &mut self,
        path: &Path,
        identity: BreakpointId,
        property: BreakpointProperty,
        value: Option<String>,
    ) -> Result<()> {
        self.debugger
            .set_breakpoint_property(path, identity, property, value)?;
        if let Some(client) = self.debugger.get_active_client() {
            self.sync_debugger_breakpoints(client.id(), path.to_owned())?;
        }
        Ok(())
    }

    /// The initialized event starts configuration without awaiting transport on the editor path.
    pub(crate) fn configure_debugger_breakpoints(&mut self, id: DebugAdapterId) -> Result<()> {
        self.debugger
            .get_client(id)
            .context("Debugger has stopped")?;
        // Adapter IDs and verification belong to the newly selected session.
        for breakpoint in self.debugger.breakpoints.values_mut().flatten() {
            breakpoint.id = None;
            breakpoint.verified = false;
            breakpoint.message = None;
        }
        self.debugger.breakpoint_sessions.insert(
            id,
            BreakpointSession {
                configuring: true,
                ..Default::default()
            },
        );
        self.debugger.adapters.set_active_client(id);
        let paths: Vec<_> = self.debugger.breakpoints.keys().cloned().collect();
        for path in paths {
            if let Err(error) = self.sync_debugger_breakpoints(id, path) {
                // Unsupported stored properties must not prevent the adapter from starting.
                log::error!("Failed to configure breakpoints: {error}");
            }
        }
        self.finish_breakpoint_configuration(id);
        Ok(())
    }

    fn sync_debugger_breakpoints(&mut self, id: DebugAdapterId, path: PathBuf) -> Result<()> {
        let client = self
            .debugger
            .adapters
            .get_client(id)
            .context("Debugger has stopped")?;
        let session = self.debugger.breakpoint_sessions.entry(id).or_default();
        let source = session.sources.entry(path.clone()).or_default();
        if source.request.is_running() {
            source.dirty = true;
            return Ok(());
        }
        let breakpoints = self
            .debugger
            .breakpoints
            .get(&path)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let arguments = requests::SetBreakpointsArguments {
            source: dap::Source {
                path: Some(path.clone()),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(source_breakpoints(breakpoints, client.caps.as_ref())?),
            source_modified: Some(false),
        };
        let identities: Vec<_> = breakpoints.iter().map(Breakpoint::identity).collect();
        let request = source.request.restart();
        let lifetime = session.current.clone();
        let sender = client.request_sender();
        let callbacks = self.callback_sender().clone();
        let owner = callbacks.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = lifetime.canceled() => return,
                _ = request.canceled() => return,
                result = sender.request::<requests::SetBreakpoints>(arguments) => result,
            };
            callbacks
                .send(move |editor| {
                    if !owner.same_destination(editor.callback_sender())
                        || lifetime.is_canceled()
                        || request.is_canceled()
                    {
                        return;
                    }
                    let Some(session) = editor.debugger.breakpoint_sessions.get_mut(&id) else {
                        return;
                    };
                    let dirty = session
                        .sources
                        .remove(&path)
                        .is_some_and(|source| source.dirty);
                    if dirty {
                        // A source has at most one request in flight. Send the latest model
                        // only after its predecessor completes, preserving adapter-side order.
                        if let Err(error) = editor.sync_debugger_breakpoints(id, path) {
                            editor.set_error(|| error.to_string());
                        }
                    } else if editor
                        .debugger
                        .get_active_client()
                        .is_some_and(|client| client.id() == id)
                    {
                        match result {
                            Ok(response) => {
                                if let (Some(breakpoints), Some(response)) = (
                                    editor.debugger.breakpoints.get_mut(&path),
                                    response.breakpoints,
                                ) {
                                    apply_response(breakpoints, &identities, response);
                                }
                            }
                            Err(error) => {
                                editor.set_error(|| format!("Failed to set breakpoints: {error}"))
                            }
                        }
                    }
                    editor.finish_breakpoint_configuration(id);
                })
                .await;
        });
        Ok(())
    }

    fn finish_breakpoint_configuration(&mut self, id: DebugAdapterId) {
        let Some(session) = self.debugger.breakpoint_sessions.get_mut(&id) else {
            return;
        };
        if !session.configuring
            || session
                .sources
                .values()
                .any(|source| source.request.is_running())
        {
            return;
        }
        session.configuring = false;
        let lifetime = session.current.clone();
        let Some(client) = self.debugger.get_client(id) else {
            return;
        };
        let sender = client.request_sender();
        let supported = client
            .capabilities()
            .supports_configuration_done_request
            .unwrap_or(false);
        let callbacks = self.callback_sender().clone();
        let owner = callbacks.clone();
        tokio::spawn(async move {
            let result = event::cancelable_future(
                async move {
                    if supported {
                        sender
                            .request::<requests::ConfigurationDone>(Some(
                                requests::ConfigurationDoneArguments {},
                            ))
                            .await?;
                    }
                    dap::Result::Ok(())
                },
                &lifetime,
            )
            .await;
            callbacks
                .send(move |editor| {
                    if !owner.same_destination(editor.callback_sender())
                        || lifetime.is_canceled()
                        || !editor
                            .debugger
                            .get_active_client()
                            .is_some_and(|client| client.id() == id)
                    {
                        return;
                    }
                    match result {
                        Some(Ok(()))
                            if editor.debugger.get_active_client().is_some_and(|client| {
                                client.id() == id && client.thread_id.is_none()
                            }) =>
                        {
                            editor.set_status("Debugged application started")
                        }
                        Some(Ok(())) => {}
                        Some(Err(error)) => {
                            editor.set_error(|| format!("Debugger configuration failed: {error}"))
                        }
                        None => {}
                    }
                })
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_replies_follow_breakpoint_identity_and_preserve_unspecified_lines() {
        let mut breakpoints = [Breakpoint {
            identity: BreakpointId(2),
            line: 7,
            ..Default::default()
        }];
        let response = serde_json::from_value(serde_json::json!([
            {"id":100,"verified":true,"line":42}, {"id":200,"verified":true}
        ]))
        .unwrap();
        apply_response(
            &mut breakpoints,
            &[BreakpointId(1), BreakpointId(2)],
            response,
        );
        assert_eq!(breakpoints[0].id, Some(200));
        assert_eq!(breakpoints[0].line, 7);
        assert!(breakpoints[0].verified);
    }
}

//! Ordered breakpoint synchronization, independent of thread and frame inspection.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use dap::{requests, DebugAdapterId, SourceBreakpoint};
use event::{TaskController, TaskHandle};

use super::{
    validate_capabilities, Breakpoint, BreakpointBinding, BreakpointId, BreakpointLocation,
    BreakpointProperty, DebuggerState,
};
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
    bindings: ::debugger::BreakpointBindings,
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
            bindings: ::debugger::BreakpointBindings::default(),
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
                line: breakpoint
                    .line
                    .checked_add(1)
                    .context("Breakpoint line is out of range")?,
                column: breakpoint.column.filter(|column| *column > 0),
                condition: breakpoint.condition.clone(),
                hit_condition: breakpoint.hit_condition.clone(),
                log_message: breakpoint.log_message.clone(),
            })
        })
        .collect()
}

impl BreakpointSession {
    pub(super) fn is_configuring(&self) -> bool {
        self.configuring
    }
    pub(super) fn remove_binding(&mut self, identity: BreakpointId) {
        self.bindings.remove_binding(identity);
    }
    fn apply_response(
        &mut self,
        path: &Path,
        definitions: &[Breakpoint],
        response: Vec<dap::Breakpoint>,
    ) {
        self.bindings.apply_response(path, definitions, response);
    }
    fn handle_event(&mut self, reason: &str, response: dap::Breakpoint) {
        self.bindings.handle_event(reason, response);
    }
}

impl DebuggerState {
    /// Verification and resolved location in the currently selected adapter.
    pub fn breakpoint_binding(&self, identity: BreakpointId) -> Option<&BreakpointBinding> {
        let session = self.get_active_session()?.breakpoints.as_ref()?;
        session.bindings.get(identity)
    }

    pub(super) fn breakpoint_line(&self, definition: &Breakpoint) -> usize {
        self.breakpoint_binding(definition.identity())
            .map_or(definition.line, |binding| binding.line)
    }

    /// Markers projected through the active adapter's bindings.
    pub fn breakpoint_locations<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = BreakpointLocation<'a>> {
        self.breakpoints.locations(
            path,
            self.get_active_session()
                .and_then(|session| session.breakpoints.as_ref())
                .map(|session| &session.bindings),
        )
    }
}

impl Editor {
    /// Update source definitions and synchronize each configured adapter asynchronously.
    pub fn toggle_breakpoints(&mut self, path: PathBuf, lines: &[usize]) -> Result<()> {
        self.debugger.toggle_breakpoints(path.clone(), lines);
        self.sync_all_debugger_breakpoints(&path)?;
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
        self.sync_all_debugger_breakpoints(path)?;
        Ok(())
    }

    /// The initialized event starts configuration without awaiting transport on the editor path.
    pub(crate) fn configure_debugger_breakpoints(&mut self, id: DebugAdapterId) -> Result<()> {
        self.debugger
            .get_client(id)
            .context("Debugger has stopped")?;
        self.debugger.sessions[id].breakpoints = Some(BreakpointSession {
            configuring: true,
            ..Default::default()
        });
        let paths: Vec<_> = self.debugger.breakpoints.paths().cloned().collect();
        for path in paths {
            if let Err(error) = self.sync_debugger_breakpoints(id, path) {
                // Unsupported stored properties must not prevent the adapter from starting.
                log::error!("Failed to configure breakpoints: {error}");
            }
        }
        self.finish_breakpoint_configuration(id);
        Ok(())
    }

    fn sync_all_debugger_breakpoints(&mut self, path: &Path) -> Result<()> {
        let sessions: Vec<_> = self
            .debugger
            .sessions
            .iter()
            .filter_map(|(id, session)| session.breakpoints.is_some().then_some(id))
            .collect();
        let mut first_error = None;
        for id in sessions {
            if let Err(error) = self.sync_debugger_breakpoints(id, path.to_owned()) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn handle_breakpoint_event(
        &mut self,
        id: DebugAdapterId,
        reason: &str,
        response: dap::Breakpoint,
    ) {
        let Some(session) = self
            .debugger
            .sessions
            .get_mut(id)
            .and_then(|session| session.breakpoints.as_mut())
        else {
            return;
        };
        let message = response.message.clone();
        session.handle_event(reason, response);
        if self
            .debugger
            .get_active_client()
            .is_some_and(|client| client.id() == id)
            && let Some(message) = message
        {
            self.set_status(format!("Breakpoint: {message}"));
        }
    }

    fn sync_debugger_breakpoints(&mut self, id: DebugAdapterId, path: PathBuf) -> Result<()> {
        let debug_session = self
            .debugger
            .sessions
            .get_mut(id)
            .context("Debugger has stopped")?;
        let client = debug_session
            .client
            .as_ref()
            .context("Debugger is connecting")?;
        let session = debug_session
            .breakpoints
            .get_or_insert_with(BreakpointSession::default);
        let source = session.sources.entry(path.clone()).or_default();
        if source.request.is_running() {
            source.dirty = true;
            return Ok(());
        }
        let breakpoints = self.debugger.breakpoints.breakpoints(&path);
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
            breakpoints: Some(source_breakpoints(
                breakpoints,
                debug_session.capabilities.as_ref(),
            )?),
            source_modified: Some(false),
        };
        let definitions = breakpoints.to_vec();
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
                    let Some(session) = editor
                        .debugger
                        .sessions
                        .get_mut(id)
                        .and_then(|session| session.breakpoints.as_mut())
                    else {
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
                    } else {
                        match result {
                            Ok(response) => {
                                if let Some(response) = response.breakpoints {
                                    session.apply_response(&path, &definitions, response);
                                }
                            }
                            Err(error) => {
                                if editor
                                    .debugger
                                    .get_active_client()
                                    .is_some_and(|client| client.id() == id)
                                {
                                    editor.set_error(|| {
                                        format!("Failed to set breakpoints: {error}")
                                    });
                                }
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
        let Some(session) = self
            .debugger
            .sessions
            .get_mut(id)
            .and_then(|session| session.breakpoints.as_mut())
        else {
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
        let supported = self.debugger.sessions[id]
            .capabilities()
            .and_then(|caps| caps.supports_configuration_done_request)
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
                            if editor.debugger.get_active_session().is_some_and(|session| {
                                session.id() == id && session.selected_thread().is_none()
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

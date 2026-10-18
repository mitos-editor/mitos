//! Ordered breakpoint synchronization, independent of thread and frame inspection.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use dap::{requests, DebugAdapterId, SourceBreakpoint};
use event::{TaskController, TaskHandle};

use super::{
    source::line_index, validate_capabilities, Breakpoint, BreakpointId, BreakpointProperty,
    DebuggerState,
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
    bindings: HashMap<BreakpointId, BreakpointBinding>,
    discovered: HashMap<usize, BreakpointBinding>,
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
            bindings: HashMap::new(),
            discovered: HashMap::new(),
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

/// Adapter-specific verification and location. Requested source definitions are unchanged.
#[derive(Debug, Clone)]
pub struct BreakpointBinding {
    pub id: Option<usize>,
    pub verified: bool,
    pub message: Option<String>,
    pub path: PathBuf,
    pub line: usize,
    /// Optional one-based UTF-16 column supplied by the adapter.
    pub column: Option<usize>,
}

impl BreakpointBinding {
    fn new(path: PathBuf, line: usize, column: Option<usize>, response: dap::Breakpoint) -> Self {
        Self {
            id: response.id,
            verified: response.verified,
            message: response.message,
            path,
            line: response.line.and_then(line_index).unwrap_or(line),
            column: response.column.filter(|column| *column > 0).or(column),
        }
    }

    fn update(&mut self, response: dap::Breakpoint) {
        self.verified = response.verified;
        self.message = response.message;
        if let Some(line) = response.line.and_then(line_index) {
            self.line = line;
        }
        if let Some(column) = response.column.filter(|column| *column > 0) {
            self.column = Some(column);
        }
    }
}

/// A source marker projected through the active adapter's bindings, or its requested location.
pub struct BreakpointLocation<'a> {
    pub definition: Option<&'a Breakpoint>,
    pub line: usize,
    pub verified: bool,
}

impl BreakpointSession {
    pub(super) fn is_configuring(&self) -> bool {
        self.configuring
    }

    pub(super) fn remove_binding(&mut self, identity: BreakpointId) {
        self.bindings.remove(&identity);
    }

    fn apply_response(
        &mut self,
        path: &Path,
        definitions: &[Breakpoint],
        response: Vec<dap::Breakpoint>,
    ) {
        self.bindings.retain(|_, binding| binding.path != path);
        for (definition, response) in definitions.iter().zip(response) {
            if let Some(id) = response.id {
                self.discovered.remove(&id);
            }
            self.bindings.insert(
                definition.identity,
                BreakpointBinding::new(
                    path.to_owned(),
                    definition.line,
                    definition.column,
                    response,
                ),
            );
        }
    }

    fn handle_event(&mut self, reason: &str, response: dap::Breakpoint) {
        let Some(id) = response.id else {
            log::warn!("DAP breakpoint event missing id");
            return;
        };
        if reason == "removed" {
            self.bindings.retain(|_, binding| binding.id != Some(id));
            self.discovered.remove(&id);
            return;
        }
        if reason != "new" && reason != "changed" {
            log::warn!("Unknown breakpoint event: {reason}");
            return;
        }
        if let Some(binding) = self
            .bindings
            .values_mut()
            .find(|binding| binding.id == Some(id))
            .or_else(|| self.discovered.get_mut(&id))
        {
            binding.update(response);
            return;
        }
        // Breakpoints created by the adapter belong only to this session.
        if reason == "new"
            && let Some(path) = response
                .source
                .as_ref()
                .and_then(|source| source.path.clone())
            && let Some(line) = response.line.and_then(line_index)
        {
            self.discovered
                .insert(id, BreakpointBinding::new(path, line, None, response));
        }
    }
}

impl DebuggerState {
    /// Verification and resolved location in the currently selected adapter.
    pub fn breakpoint_binding(&self, identity: BreakpointId) -> Option<&BreakpointBinding> {
        let session = self.get_active_session()?.breakpoints.as_ref()?;
        session.bindings.get(&identity)
    }

    pub(super) fn breakpoint_line(&self, definition: &Breakpoint) -> usize {
        self.breakpoint_binding(definition.identity)
            .map_or(definition.line, |binding| binding.line)
    }

    /// Markers for a source file, including breakpoints discovered by the active adapter.
    pub fn breakpoint_locations<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = BreakpointLocation<'a>> {
        let session = self
            .get_active_session()
            .and_then(|session| session.breakpoints.as_ref());
        self.breakpoints(path)
            .iter()
            .map(move |definition| {
                let binding =
                    session.and_then(|session| session.bindings.get(&definition.identity));
                BreakpointLocation {
                    definition: Some(definition),
                    line: binding.map_or(definition.line, |binding| binding.line),
                    verified: binding.is_some_and(|binding| binding.verified),
                }
            })
            .chain(
                session
                    .into_iter()
                    .flat_map(|session| session.discovered.values())
                    .filter(move |binding| binding.path == path)
                    .map(|binding| BreakpointLocation {
                        definition: None,
                        line: binding.line,
                        verified: binding.verified,
                    }),
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
                            if editor.debugger.get_active_session().is_some_and(|session| {
                                session.id() == id && session.thread_id.is_none()
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

    fn response(id: usize, line: Option<usize>) -> dap::Breakpoint {
        serde_json::from_value(serde_json::json!({"id":id,"verified":true,"line":line})).unwrap()
    }

    #[test]
    fn adapter_locations_do_not_change_requested_source_definitions() {
        let definitions = [Breakpoint {
            identity: BreakpointId(1),
            line: 7,
            ..Default::default()
        }];
        let mut session = BreakpointSession::default();
        session.apply_response(
            Path::new("main.rs"),
            &definitions,
            vec![response(100, Some(42))],
        );
        assert_eq!(definitions[0].line, 7);
        assert_eq!(session.bindings[&BreakpointId(1)].line, 41);
        session.apply_response(
            Path::new("main.rs"),
            &definitions,
            vec![response(200, None)],
        );
        assert_eq!(session.bindings[&BreakpointId(1)].line, 7);
    }

    #[test]
    fn unknown_adapter_locations_preserve_requested_breakpoints() {
        let path = PathBuf::from("main.rs");
        let mut binding = BreakpointBinding::new(path.clone(), 7, Some(3), response(1, Some(0)));
        assert_eq!(binding.line, 7);
        binding.update(response(1, Some(0)));
        assert_eq!(binding.line, 7);
        let mut session = BreakpointSession::default();
        let mut unknown = response(2, Some(0));
        unknown.source = Some(serde_json::from_value(serde_json::json!({"path":path})).unwrap());
        session.handle_event("new", unknown);
        assert!(session.discovered.is_empty());
    }
}

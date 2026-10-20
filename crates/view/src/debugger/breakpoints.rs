//! Ordered breakpoint synchronization, independent of thread and frame inspection.

use stdx::rope::RopeExt as _;
mod source;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use ::debugger::BreakpointSource;
use anyhow::{Context as _, Result};
use dap::{requests, DebugAdapterId, SourceBreakpoint};
use event::{TaskController, TaskHandle};

use super::{
    validate_capabilities, Breakpoint, BreakpointBinding, BreakpointId, BreakpointLocation,
    BreakpointProperty, DebuggerState,
};
use crate::{events::DocumentDidClose, Editor};

#[derive(Default)]
struct SourceSync {
    request: TaskController,
    dirty: bool,
}

pub(super) struct BreakpointSession {
    _lifetime: TaskController,
    current: TaskHandle,
    sources: HashMap<BreakpointSource, SourceSync>,
    bindings: ::debugger::BreakpointBindings,
    modified_sources: HashSet<BreakpointSource>,
}

impl Default for BreakpointSession {
    fn default() -> Self {
        let mut lifetime = TaskController::new();
        let current = lifetime.restart();
        Self {
            _lifetime: lifetime,
            current,
            sources: HashMap::new(),
            bindings: ::debugger::BreakpointBindings::default(),
            modified_sources: HashSet::new(),
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

pub(crate) fn map_breakpoint_positions(
    source: &::debugger::SourceBreakpoints,
    changes: &editor_core::ChangeSet,
    old: &editor_core::Rope,
    new: &editor_core::Rope,
) {
    source.map_positions(|breakpoint| {
        let position = super::source::position_to_char(
            old,
            breakpoint.line.saturating_add(1),
            breakpoint.column.unwrap_or(1),
        )
        .unwrap_or_else(|| old.len_chars());
        let position = changes
            .map_pos(position, editor_core::Assoc::After)
            .min(new.len_chars());
        let line = new.char_to_line_idx(position);
        let column = breakpoint.column.map(|_| {
            new.char_to_utf16_idx(position) - new.char_to_utf16_idx(new.line_to_char_idx(line)) + 1
        });
        (line, column)
    });
}

impl BreakpointSession {
    pub(super) fn remove_binding(&mut self, identity: BreakpointId) {
        self.bindings.remove_binding(identity);
    }
    pub(super) fn remove_source(&mut self, source: &BreakpointSource) {
        self.sources.remove(source);
        self.modified_sources.remove(source);
        self.bindings.remove_source_key(source);
    }
    fn apply_response(
        &mut self,
        source: &BreakpointSource,
        definitions: &[Breakpoint],
        response: Vec<dap::Breakpoint>,
    ) {
        self.bindings
            .apply_source_response(source, definitions, response);
    }
    fn handle_event(
        &mut self,
        reason: &str,
        response: dap::Breakpoint,
        source: Option<BreakpointSource>,
    ) {
        self.bindings.handle_source_event(reason, response, source);
    }
}

impl DebuggerState {
    /// Verification and resolved location in the currently selected adapter.
    pub fn breakpoint_binding(&self, identity: BreakpointId) -> Option<&BreakpointBinding> {
        let session = self.get_active_session()?.breakpoints.as_ref()?;
        session
            .bindings
            .get_for(&self.breakpoints.definition(identity)?)
    }

    pub(super) fn breakpoint_line(&self, definition: &Breakpoint) -> usize {
        self.breakpoint_binding(definition.identity())
            .map_or(definition.line, |binding| binding.line)
    }

    /// Markers projected through the active adapter's bindings.
    pub fn breakpoint_locations<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = BreakpointLocation> {
        self.breakpoints.locations(
            path,
            self.get_active_session()
                .and_then(|session| session.breakpoints.as_ref())
                .map(|session| &session.bindings),
        )
    }
}

impl Editor {
    pub(crate) fn debugger_closed_source_written(
        &mut self,
        event: &crate::document::DocumentSavedEvent,
    ) {
        if let Some(old) = self.debugger.closed_breakpoint_sources.get(&event.path) {
            let changes = text_diff::compare_ropes(old, &event.text);
            let source = self.debugger.breakpoints.source(event.path.clone());
            map_breakpoint_positions(&source, changes.changes(), old, &event.text);
            self.debugger
                .closed_breakpoint_sources
                .insert(event.path.clone(), event.text.clone());
            self.debugger_source_saved(&event.path);
        }
    }

    pub(crate) fn debugger_source_saved(&mut self, path: &Path) {
        if let Err(error) = self.sync_tracked_debugger_source(path, true) {
            log::warn!("Could not synchronize saved debugger source: {error}");
        }
    }

    fn debugger_source_is_tracked(&self, path: &Path) -> bool {
        !self.debugger.breakpoints(path).is_empty()
            || self.debugger.sessions.iter().any(|(_, session)| {
                session.breakpoints.as_ref().is_some_and(|state| {
                    state
                        .sources
                        .contains_key(&BreakpointSource::File(path.to_owned()))
                })
            })
    }

    fn sync_tracked_debugger_source(&mut self, path: &Path, modified: bool) -> Result<()> {
        if self.debugger_source_is_tracked(path) {
            self.sync_all_debugger_breakpoints(path, modified)?;
        }
        Ok(())
    }

    pub(crate) fn rename_debugger_sources(
        &mut self,
        old: &Path,
        new: &Path,
    ) -> Vec<(PathBuf, PathBuf)> {
        let mut paths = self.debugger.breakpoints.rename_sources(old, new);
        for (old, new) in &paths {
            if let Some(text) = self.debugger.closed_breakpoint_sources.remove(old) {
                self.debugger
                    .closed_breakpoint_sources
                    .insert(new.clone(), text);
            }
        }
        for session in self
            .debugger
            .sessions
            .values_mut()
            .filter_map(|session| session.breakpoints.as_mut())
        {
            for (old, _) in &paths {
                session.bindings.remove_source(old);
            }
        }
        paths.retain(|(old, new)| {
            self.debugger_source_is_tracked(old) || self.debugger_source_is_tracked(new)
        });
        paths
    }

    pub(crate) fn sync_renamed_debugger_sources(&mut self, paths: Vec<(PathBuf, PathBuf)>) {
        for (old, new) in paths {
            if let Err(error) = self
                .sync_all_debugger_breakpoints(&old, false)
                .and_then(|_| self.sync_all_debugger_breakpoints(&new, true))
            {
                log::warn!("Could not synchronize renamed debugger source: {error}");
            }
        }
    }

    /// Update source definitions and synchronize each configured adapter asynchronously.
    pub fn toggle_breakpoints(&mut self, path: PathBuf, lines: &[usize]) -> Result<()> {
        self.debugger.toggle_breakpoints(path.clone(), lines);
        self.sync_all_debugger_breakpoints(&path, false)?;
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
        self.sync_all_debugger_breakpoints(path, false)?;
        Ok(())
    }

    pub(super) fn suspend_debugger_breakpoints(&mut self, id: DebugAdapterId) {
        let paths: Vec<_> = self
            .debugger
            .breakpoints
            .paths()
            .cloned()
            .map(BreakpointSource::File)
            .collect();
        let Some(state) = self.debugger.sessions[id].breakpoints.as_mut() else {
            return;
        };
        for path in paths {
            state.sources.entry(path).or_default();
        }
        for source in state.sources.values_mut() {
            source.request.cancel();
            source.dirty = true;
        }
    }

    pub(super) fn resume_debugger_breakpoints(&mut self, id: DebugAdapterId) {
        let Some(state) = self.debugger.sessions[id].breakpoints.as_ref() else {
            return;
        };
        let paths: HashSet<_> = state
            .sources
            .keys()
            .cloned()
            .chain(
                self.debugger
                    .breakpoints
                    .paths()
                    .cloned()
                    .map(BreakpointSource::File),
            )
            .collect();
        for path in paths {
            if let Err(error) = self.sync_debugger_source_breakpoints(id, path, false) {
                log::warn!("Could not resume breakpoint synchronization: {error}");
            }
        }
        self.finish_breakpoint_configuration(id);
    }

    /// The initialized event starts configuration without awaiting transport on the editor path.
    pub(crate) fn configure_debugger_breakpoints(&mut self, id: DebugAdapterId) -> Result<()> {
        self.debugger
            .get_client(id)
            .context("Debugger has stopped")?;
        let session = &mut self.debugger.sessions[id];
        session.lifecycle.begin_configuration();
        session.breakpoints = Some(BreakpointSession::default());
        let paths: Vec<_> = self.debugger.breakpoints.paths().cloned().collect();
        for path in paths {
            if let Err(error) = self.sync_debugger_breakpoints(id, path, false) {
                // Unsupported stored properties must not prevent the adapter from starting.
                log::error!("Failed to configure breakpoints: {error}");
            }
        }
        self.finish_breakpoint_configuration(id);
        Ok(())
    }

    fn sync_all_debugger_breakpoints(&mut self, path: &Path, modified: bool) -> Result<()> {
        let sessions: Vec<_> = self
            .debugger
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                (session.breakpoints.is_some() && !session.lifecycle.is_disconnecting())
                    .then_some(id)
            })
            .collect();
        let mut first_error = None;
        for id in sessions {
            if let Err(error) = self.sync_debugger_breakpoints(id, path.to_owned(), modified) {
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
        let source = response
            .source
            .as_ref()
            .and_then(|source| self.debugger.source_key(id, source));
        let Some(session) = self
            .debugger
            .sessions
            .get_mut(id)
            .and_then(|session| session.breakpoints.as_mut())
        else {
            return;
        };
        let message = response.message.clone();
        session.handle_event(reason, response, source);
        if self
            .debugger
            .get_active_client()
            .is_some_and(|client| client.id() == id)
            && let Some(message) = message
        {
            self.set_status(format!("Breakpoint: {message}"));
        }
    }

    fn sync_debugger_breakpoints(
        &mut self,
        id: DebugAdapterId,
        path: PathBuf,
        modified: bool,
    ) -> Result<()> {
        self.sync_debugger_source_breakpoints(id, BreakpointSource::File(path), modified)
    }

    fn sync_debugger_source_breakpoints(
        &mut self,
        id: DebugAdapterId,
        key: BreakpointSource,
        modified: bool,
    ) -> Result<()> {
        if self.debugger.is_terminating(id) {
            return Ok(());
        }
        let (descriptor, unsaved) = match &key {
            BreakpointSource::File(path) => (
                dap::Source {
                    path: Some(path.clone()),
                    ..Default::default()
                },
                self.document_by_path(path).is_some_and(|doc| {
                    doc.is_modified()
                        && doc
                            .breakpoint_saved_text
                            .as_ref()
                            .is_none_or(|saved| doc.text() != saved)
                }),
            ),
            BreakpointSource::Adapter(source) => (
                self.debugger
                    .adapter_source_descriptor(id, *source)
                    .context("Debugger source is no longer available")?,
                false,
            ),
        };

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
        if modified {
            session.modified_sources.insert(key.clone());
        }
        if unsaved {
            // Retain deferred removals too: the source may no longer have definitions.
            session.sources.entry(key).or_default().dirty = true;
            return Ok(());
        }
        let source = session.sources.entry(key.clone()).or_default();
        if source.request.is_running() {
            source.dirty = true;
            return Ok(());
        }
        let breakpoints = self.debugger.breakpoints.breakpoints_for(&key);
        let arguments = requests::SetBreakpointsArguments {
            source: descriptor,
            breakpoints: Some(source_breakpoints(
                &breakpoints,
                debug_session.capabilities.as_ref(),
            )?),
            source_modified: Some(session.modified_sources.contains(&key)),
        };
        let definitions = breakpoints;
        source.dirty = false;
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
                        .remove(&key)
                        .is_some_and(|source| source.dirty);
                    if dirty {
                        // A source has at most one request in flight. Send the latest model
                        // only after its predecessor completes, preserving adapter-side order.
                        if let Err(error) = editor.sync_debugger_source_breakpoints(id, key, false)
                        {
                            editor.set_error(|| error.to_string());
                        }
                    } else {
                        match result {
                            Ok(response) => {
                                session.modified_sources.remove(&key);
                                if let Some(response) = response.breakpoints {
                                    session.apply_response(&key, &definitions, response);
                                }
                            }
                            Err(error) => {
                                // Keep failed clears eligible for a later save or explicit retry.
                                session.sources.entry(key).or_default().dirty = true;
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

    pub(super) fn finish_breakpoint_configuration(&mut self, id: DebugAdapterId) {
        let Some(debug_session) = self.debugger.sessions.get_mut(id) else {
            return;
        };
        let Some(session) = debug_session.breakpoints.as_mut() else {
            return;
        };
        if debug_session.configuration.in_flight
            || debug_session.lifecycle.is_disconnecting()
            || !debug_session.lifecycle.needs_configuration_done()
            || session
                .sources
                .values()
                .any(|source| source.request.is_running())
        {
            return;
        }
        debug_session.lifecycle.configuration_sent();
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
                        || editor.debugger.get_session(id).is_none()
                    {
                        return;
                    }
                    editor.debugger.sessions[id]
                        .lifecycle
                        .configuration_finished();
                    if editor.debugger.is_terminating(id) {
                        return;
                    }
                    if editor
                        .debugger
                        .get_active_session()
                        .is_none_or(|session| session.id() != id)
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

pub(super) fn register_hooks() {
    event::register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let (Some(saved), Some(path)) = (&event.doc.breakpoint_saved_text, event.doc.path())
            && !event.editor.debugger.breakpoints(path).is_empty()
        {
            event
                .editor
                .debugger
                .closed_breakpoint_sources
                .insert(path.to_owned(), saved.clone());
        }
        if event.doc.is_modified()
            && let (Some(source), Some(saved), Some(path)) = (
                &event.doc.breakpoint_source,
                &event.doc.breakpoint_saved_text,
                event.doc.path(),
            )
        {
            let changes = text_diff::compare_ropes(event.doc.text(), saved);
            map_breakpoint_positions(source, changes.changes(), event.doc.text(), saved);
            event.editor.sync_tracked_debugger_source(path, false)?;
        }
        Ok(())
    });
}

//! Bounded native requests. Guest IDs are correlation values, never identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use plugin_api::ui::{
    BuiltinRequest, BuiltinResponse, KeymapResponse, KeymapUpdate, UiCancellation, UiIdentity,
    UiKind, UiLocation, UiOpenAction, UiOrigin, UiOutcome, UiOwner, UiRequest, UiResponse, UiValue,
    MAX_PLUGIN_KEYBINDINGS, MAX_UI_INPUT_BYTES,
};

use super::*;

pub(super) const MAX_FRONTEND_REQUESTS: usize = 8;
const MAX_FRONTEND_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn reply_event(event: Event) -> bool {
    matches!(
        event,
        Event::UiResult | Event::BuiltinResult | Event::KeymapResult
    )
}

#[derive(Default)]
pub(super) struct FrontendState {
    next_token: u64,
    pending: BTreeMap<UiIdentity, Pending>,
    ui: VecDeque<UiRequest>,
    builtins: VecDeque<BuiltinRequest>,
    keymaps: VecDeque<KeymapUpdate>,
    bytes: usize,
}

struct Pending {
    origin: Option<UiOrigin>,
    source: PluginEventSource,
    model: Model,
    bytes: usize,
    resolved: bool,
    task: Option<crate::callbacks::InvocationTask>,
}

enum Model {
    Prompt,
    NextKey,
    Picker(BTreeMap<String, Option<UiLocation>>),
    Builtin(usize),
    Keymap,
}

pub(super) enum PreparedFrontend {
    Ui(UiRequest),
    Builtin(BuiltinRequest),
    Keymap(KeymapUpdate),
}

impl FrontendState {
    pub(super) fn release_reply(&mut self, data: &Value) {
        let identity = data
            .get("response")
            .and_then(|response| response.get("identity"));
        let Some(identity) = identity
            .and_then(|identity| serde_json::from_value::<UiIdentity>(identity.clone()).ok())
        else {
            return;
        };
        if self
            .pending
            .get(&identity)
            .is_some_and(|pending| pending.resolved)
        {
            let pending = self.pending.remove(&identity).unwrap();
            self.bytes -= pending.bytes;
        }
    }
}

impl Editor {
    pub(super) fn prune_plugin_frontend(&mut self) {
        let expired = self
            .plugins
            .frontend
            .pending
            .keys()
            .filter(|identity| !self.plugin_owner_is_current(&identity.owner))
            .cloned()
            .collect::<Vec<_>>();
        for identity in expired {
            if let Some(pending) = self.plugins.frontend.pending.remove(&identity) {
                self.plugins.frontend.bytes -= pending.bytes;
            }
        }
        self.plugins.frontend.ui.retain(|request| {
            self.plugins
                .frontend
                .pending
                .contains_key(&request.identity)
        });
        self.plugins.frontend.builtins.retain(|request| {
            self.plugins
                .frontend
                .pending
                .contains_key(&request.identity)
        });
        self.plugins.frontend.keymaps.retain(|request| {
            self.plugins
                .frontend
                .pending
                .contains_key(&request.identity)
        });
    }

    /// Owner identity is host assigned and expires on unload, trap or reload.
    pub fn plugin_owner_is_current(&self, owner: &UiOwner) -> bool {
        !self.plugins.stopped
            && owner.generation == self.plugins.shared.generation
            && self
                .plugins
                .manager
                .receives_event(&owner.plugin, Event::UiResult)
    }

    fn plugin_origin_is_bound(&self, origin: UiOrigin) -> bool {
        self.tree
            .try_get(ViewId::from_u64(origin.view))
            .is_some_and(|view| {
                view.doc.as_u64() == origin.document
                    && view.binding_revision() == origin.binding_revision
            })
    }

    pub fn plugin_ui_request_is_current(&self, identity: &UiIdentity) -> bool {
        self.plugin_owner_is_current(&identity.owner)
            && self
                .plugins
                .frontend
                .pending
                .get(identity)
                .is_some_and(|pending| {
                    !pending.resolved
                        && pending
                            .origin
                            .is_none_or(|origin| self.plugin_origin_is_bound(origin))
                        && matches!(
                            pending.model,
                            Model::Prompt | Model::NextKey | Model::Picker(_)
                        )
                })
    }

    pub fn take_plugin_ui_requests(&mut self) -> Vec<UiRequest> {
        self.plugins.frontend.ui.drain(..).collect()
    }

    pub fn take_plugin_builtin_requests(&mut self) -> Vec<BuiltinRequest> {
        self.plugins.frontend.builtins.drain(..).collect()
    }

    pub fn take_plugin_keymap_requests(&mut self) -> Vec<KeymapUpdate> {
        self.plugins.frontend.keymaps.drain(..).collect()
    }

    pub(super) fn prepare_plugin_frontend(
        &self,
        action: Action,
        plugin: &str,
        batch: &mut Vec<PreparedFrontend>,
    ) -> Result<(), ServiceError> {
        let (request, origin) = match &action {
            Action::ShowUi {
                request, origin, ..
            } => (*request, *origin),
            Action::InvokeBuiltin {
                request, origin, ..
            } => (*request, Some(*origin)),
            Action::UpdateKeymap { request, .. } => (*request, None),
            _ => return Err(failure(ErrorCode::InvalidRequest, "not a frontend action")),
        };
        let owner = UiOwner {
            plugin: plugin.into(),
            generation: self.plugins.shared.generation,
        };
        if self
            .plugins
            .frontend
            .pending
            .iter()
            .any(|(identity, pending)| {
                !pending.resolved && identity.owner == owner && identity.request == request
            })
            || batch.iter().any(|prepared| {
                prepared.identity().owner == owner && prepared.identity().request == request
            })
        {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "plugin request ID is already outstanding",
            ));
        }
        if self.plugins.frontend.pending.len() + batch.len() >= MAX_FRONTEND_REQUESTS {
            return Err(failure(
                ErrorCode::ResourceExhausted,
                "too many outstanding plugin frontend requests",
            ));
        }
        if origin.is_some_and(|origin| !self.plugin_origin_is_bound(origin)) {
            return Err(failure(
                ErrorCode::StaleState,
                "plugin frontend origin is no longer bound",
            ));
        }
        let token = self
            .plugins
            .frontend
            .next_token
            .checked_add(batch.len() as u64 + 1)
            .ok_or_else(|| {
                failure(
                    ErrorCode::HostFailure,
                    "plugin frontend identity space exhausted",
                )
            })?;
        let identity = UiIdentity {
            owner,
            request,
            token,
        };
        let prepared = match action {
            Action::ShowUi { origin, kind, .. } => {
                let kind = kind.normalize()?;
                if let UiKind::Picker { rows, .. } = &kind {
                    if rows.iter().any(|row| row.location.is_some()) {
                        if origin.is_none() {
                            return Err(failure(
                                ErrorCode::InvalidRequest,
                                "picker navigation requires an originating view",
                            ));
                        }
                        self.plugins
                            .manager
                            .require_capability(plugin, Capability::EditorNavigate)?;
                        self.plugins
                            .manager
                            .require_capability(plugin, Capability::EditorSelection)?;
                    }
                    for location in rows.iter().filter_map(|row| row.location) {
                        self.validate_plugin_location(location)?;
                    }
                }
                PreparedFrontend::Ui(UiRequest {
                    identity,
                    origin,
                    kind,
                })
            }
            Action::InvokeBuiltin {
                origin, commands, ..
            } => {
                let request = BuiltinRequest {
                    identity,
                    origin,
                    commands,
                };
                request.validate()?;
                self.validate_plugin_builtin_origin(origin)?;
                let document = self
                    .plugin_document(origin.document, origin.version)
                    .map_err(|error| failure(ErrorCode::StaleState, error.to_string()))?;
                self.validate_plugin_builtin_editable(document, &request.commands)?;
                request.validate_work(
                    self.documents[&document]
                        .selection(ViewId::from_u64(origin.view))
                        .len(),
                )?;
                PreparedFrontend::Builtin(request)
            }
            Action::UpdateKeymap { bindings, .. } => {
                if bindings.len() > MAX_PLUGIN_KEYBINDINGS {
                    return Err(failure(
                        ErrorCode::ResourceExhausted,
                        "too many plugin keybindings",
                    ));
                }
                let mut paths = BTreeSet::new();
                for binding in &bindings {
                    if binding.keys.is_empty()
                        || binding.keys.len() > 8
                        || binding.keys.iter().any(|key| key.len() > 64)
                        || !paths.insert((binding.mode, binding.keys.clone()))
                        || self
                            .plugin_command_doc(&format!("{plugin}.{}", binding.command))
                            .is_none()
                    {
                        return Err(failure(ErrorCode::InvalidRequest,"plugin keybinding must name a declared local command and unique bounded key path"));
                    }
                    for key in &binding.keys {
                        key.parse::<crate::input::KeyEvent>().map_err(|error| {
                            failure(
                                ErrorCode::InvalidRequest,
                                format!("invalid plugin key: {error}"),
                            )
                        })?;
                    }
                }
                PreparedFrontend::Keymap(KeymapUpdate { identity, bindings })
            }
            _ => unreachable!(),
        };
        let bytes = self
            .plugins
            .frontend
            .bytes
            .saturating_add(batch.iter().map(PreparedFrontend::bytes).sum::<usize>())
            .saturating_add(prepared.bytes());
        if bytes > MAX_FRONTEND_BYTES {
            return Err(failure(
                ErrorCode::ResourceExhausted,
                "plugin frontend models exceed their byte limit",
            ));
        }
        batch.push(prepared);
        Ok(())
    }

    pub(super) fn apply_plugin_frontend(&mut self, prepared: PreparedFrontend) {
        let identity = prepared.identity().clone();
        let bytes = prepared.bytes();
        let source = self.plugin_event_source();
        let task = crate::callbacks::InvocationTask::new_named(
            self.invocation_tasks(),
            "plugin frontend operation cancelled before completion",
        );
        let (origin, model) = match &prepared {
            PreparedFrontend::Ui(request) => (
                request.origin,
                match &request.kind {
                    UiKind::Prompt { .. } => Model::Prompt,
                    UiKind::NextKey { .. } => Model::NextKey,
                    UiKind::Picker { rows, .. } => Model::Picker(
                        rows.iter()
                            .map(|row| (row.id.clone(), row.location))
                            .collect(),
                    ),
                },
            ),
            PreparedFrontend::Builtin(request) => {
                (Some(request.origin), Model::Builtin(request.commands.len()))
            }
            PreparedFrontend::Keymap(_) => (None, Model::Keymap),
        };
        self.plugins.frontend.next_token = identity.token;
        self.plugins.frontend.bytes += bytes;
        self.plugins.frontend.pending.insert(
            identity,
            Pending {
                origin,
                source,
                model,
                bytes,
                resolved: false,
                task,
            },
        );
        match prepared {
            PreparedFrontend::Ui(request) => self.plugins.frontend.ui.push_back(request),
            PreparedFrontend::Builtin(request) => self.plugins.frontend.builtins.push_back(request),
            PreparedFrontend::Keymap(request) => self.plugins.frontend.keymaps.push_back(request),
        }
        event::request_redraw();
    }

    fn pending_plugin_frontend(&self, identity: &UiIdentity) -> Result<&Pending, ServiceError> {
        if !self.plugin_owner_is_current(&identity.owner) {
            return Err(failure(
                ErrorCode::Cancelled,
                "plugin frontend owner has expired",
            ));
        }
        self.plugins
            .frontend
            .pending
            .get(identity)
            .filter(|pending| !pending.resolved)
            .ok_or_else(|| {
                failure(
                    ErrorCode::StaleState,
                    "plugin frontend request has already completed or expired",
                )
            })
    }

    pub fn resolve_plugin_ui(&mut self, mut response: UiResponse) -> Result<(), ServiceError> {
        let pending = self.pending_plugin_frontend(&response.identity)?;
        let mut location = None;
        if let UiOutcome::Accepted { value } = &response.outcome {
            match (&pending.model, value) {
                (Model::Prompt, UiValue::Prompt { text }) if text.len() <= MAX_UI_INPUT_BYTES => (),
                (Model::NextKey, UiValue::NextKey { key })
                    if key.len() <= 64 && key.parse::<crate::input::KeyEvent>().is_ok() => {}
                (Model::Picker(rows), UiValue::Picker { row, action }) => {
                    location = Some((
                        *rows.get(row).ok_or_else(|| {
                            failure(ErrorCode::InvalidRequest, "unknown plugin picker row")
                        })?,
                        *action,
                    ));
                }
                _ => {
                    return Err(failure(
                        ErrorCode::InvalidRequest,
                        "plugin UI result does not match its request",
                    ))
                }
            }
            if pending
                .origin
                .is_some_and(|origin| !self.plugin_origin_is_bound(origin))
            {
                response.outcome = UiOutcome::Cancelled {
                    reason: UiCancellation::OriginLost,
                };
                location = None;
            }
        }
        if let Some((Some(location), action)) = location {
            let origin = pending.origin.unwrap();
            let source = pending.source.clone();
            let result = (|| {
                self.plugins.manager.require_capability(
                    &response.identity.owner.plugin,
                    Capability::EditorNavigate,
                )?;
                self.plugins.manager.require_capability(
                    &response.identity.owner.plugin,
                    Capability::EditorSelection,
                )?;
                let document = self.validate_plugin_location(location)?;
                let owner = self.plugins.shared.clone();
                let previous = std::mem::replace(&mut owner.queue.lock().origin, source.origin);
                let _guard = ApplyingGuard { owner, previous };
                self.focus(ViewId::from_u64(origin.view));
                self.switch(
                    document,
                    match action {
                        UiOpenAction::Replace => crate::editor::Action::Replace,
                        UiOpenAction::HorizontalSplit => crate::editor::Action::HorizontalSplit,
                        UiOpenAction::VerticalSplit => crate::editor::Action::VerticalSplit,
                    },
                );
                let view = self.tree.focus;
                let doc = self.documents.get_mut(&document).unwrap();
                doc.set_selection(view, Selection::point(location.offset));
                self.tree
                    .get_mut(view)
                    .ensure_cursor_in_view(doc, self.config.load().scrolloff);
                Ok::<_, ServiceError>(())
            })();
            if let Err(error) = result {
                response.outcome = UiOutcome::Failed { error };
            }
        }
        let outcome = match &response.outcome {
            UiOutcome::Accepted { .. } => crate::callbacks::TaskOutcome::Success,
            UiOutcome::Cancelled { reason } => {
                crate::callbacks::TaskOutcome::Cancelled(format!("plugin UI cancelled: {reason:?}"))
            }
            UiOutcome::Failed { error } => crate::callbacks::TaskOutcome::Error(error.to_string()),
        };
        self.finish_plugin_frontend_task(&response.identity, outcome);
        self.finish_plugin_frontend(
            &response.identity,
            Event::UiResult,
            serde_json::to_value(&response).unwrap(),
        )
    }

    /// The command's opening milestone is presentation, separate from its later
    /// user-choice completion. Failed presentation is resolved through UiResult.
    pub fn ack_plugin_ui_presented(&mut self, identity: &UiIdentity) -> Result<(), ServiceError> {
        if !self.plugin_ui_request_is_current(identity) {
            return Err(failure(
                ErrorCode::StaleState,
                "plugin UI request is no longer current",
            ));
        }
        self.finish_plugin_frontend_task(identity, crate::callbacks::TaskOutcome::Success);
        Ok(())
    }

    fn finish_plugin_frontend_task(
        &mut self,
        identity: &UiIdentity,
        outcome: crate::callbacks::TaskOutcome,
    ) {
        if let Some(task) = self
            .plugins
            .frontend
            .pending
            .get_mut(identity)
            .and_then(|pending| pending.task.take())
        {
            task.finish(outcome);
        }
    }

    pub fn resolve_plugin_builtin(
        &mut self,
        response: BuiltinResponse,
    ) -> Result<(), ServiceError> {
        let pending = self.pending_plugin_frontend(&response.identity)?;
        if !matches!(pending.model,Model::Builtin(count) if response.completed<=count) {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "invalid builtin completion count",
            ));
        }
        let outcome = response
            .error
            .as_ref()
            .map_or(crate::callbacks::TaskOutcome::Success, |error| {
                crate::callbacks::TaskOutcome::Error(error.to_string())
            });
        self.finish_plugin_frontend_task(&response.identity, outcome);
        self.finish_plugin_frontend(
            &response.identity,
            Event::BuiltinResult,
            serde_json::to_value(&response).unwrap(),
        )
    }

    pub fn resolve_plugin_keymap(&mut self, response: KeymapResponse) -> Result<(), ServiceError> {
        if !matches!(
            self.pending_plugin_frontend(&response.identity)?.model,
            Model::Keymap
        ) {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "request is not a keymap registration",
            ));
        }
        let outcome = response
            .error
            .as_ref()
            .map_or(crate::callbacks::TaskOutcome::Success, |error| {
                crate::callbacks::TaskOutcome::Error(error.to_string())
            });
        self.finish_plugin_frontend_task(&response.identity, outcome);
        self.finish_plugin_frontend(
            &response.identity,
            Event::KeymapResult,
            serde_json::to_value(&response).unwrap(),
        )
    }

    fn finish_plugin_frontend(
        &mut self,
        identity: &UiIdentity,
        event: Event,
        response: Value,
    ) -> Result<(), ServiceError> {
        self.pending_plugin_frontend(identity)?;
        let pending = self.plugins.frontend.pending.get_mut(identity).unwrap();
        pending.resolved = true;
        let source = pending.source.clone();
        let owner = self.plugins.shared.clone();
        let previous = std::mem::replace(&mut owner.queue.lock().origin, source.origin);
        let _guard = ApplyingGuard { owner, previous };
        if let Some(sender) = self.plugins.sender(&self.handlers.callbacks) {
            sender.enqueue_target(
                event,
                self.plugin_global_context(),
                serde_json::json!({"response":response}),
                Some(identity.owner.plugin.clone()),
            );
        }
        Ok(())
    }

    pub fn run_plugin_frontend_request(
        &mut self,
        request: &BuiltinRequest,
        run: impl FnOnce(&mut Editor) -> Result<(), ServiceError>,
    ) -> Result<(), ServiceError> {
        let pending = self.pending_plugin_frontend(&request.identity)?;
        if !matches!(pending.model,Model::Builtin(count) if request.commands.len()==count)
            || pending.origin != Some(request.origin)
        {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "builtin request does not match the host record",
            ));
        }
        request.validate()?;
        self.validate_plugin_builtin_origin(request.origin)?;
        let document = self
            .plugin_document(request.origin.document, request.origin.version)
            .map_err(|error| failure(ErrorCode::StaleState, error.to_string()))?;
        self.validate_plugin_builtin_editable(document, &request.commands)?;
        request.validate_work(
            self.documents[&document]
                .selection(ViewId::from_u64(request.origin.view))
                .len(),
        )?;
        for command in &request.commands {
            for &capability in command.command.capabilities() {
                self.plugins
                    .manager
                    .require_capability(&request.identity.owner.plugin, capability)?;
            }
        }
        let source = pending.source.clone();
        let owner = self.plugins.shared.clone();
        let previous = std::mem::replace(&mut owner.queue.lock().origin, source.origin);
        let _guard = ApplyingGuard { owner, previous };
        run(self)
    }

    fn validate_plugin_builtin_editable(
        &self,
        document: DocumentId,
        commands: &[plugin_api::ui::BuiltinInvocation],
    ) -> Result<(), ServiceError> {
        if commands.iter().any(|command| {
            command
                .command
                .capabilities()
                .contains(&Capability::EditorEdit)
        }) && (self.documents[&document].readonly || self.documents[&document].is_binary())
        {
            return Err(failure(
                ErrorCode::PermissionDenied,
                "plugin builtin cannot edit a readonly or binary document",
            ));
        }
        Ok(())
    }

    fn validate_plugin_builtin_origin(&self, origin: UiOrigin) -> Result<(), ServiceError> {
        if !self.plugin_origin_is_bound(origin) || self.tree.focus != ViewId::from_u64(origin.view)
        {
            return Err(failure(
                ErrorCode::StaleState,
                "builtin origin is no longer focused and bound",
            ));
        }
        let id = self
            .plugin_document(origin.document, origin.version)
            .map_err(|error| failure(ErrorCode::StaleState, error.to_string()))?;
        if self.documents[&id].selection_revision(ViewId::from_u64(origin.view))
            != Some(origin.selection_revision)
        {
            return Err(failure(
                ErrorCode::StaleState,
                "builtin selection has changed",
            ));
        }
        Ok(())
    }

    fn validate_plugin_location(&self, location: UiLocation) -> Result<DocumentId, ServiceError> {
        let id = self
            .plugin_document(location.document, location.version)
            .map_err(|error| failure(ErrorCode::StaleState, error.to_string()))?;
        if location.offset > self.documents[&id].text().len_chars() {
            return Err(failure(
                ErrorCode::InvalidRequest,
                "picker location is outside its document",
            ));
        }
        Ok(id)
    }

    pub(super) fn cancel_plugin_frontend(&mut self) {
        let pending: Vec<_> = self
            .plugins
            .frontend
            .pending
            .iter()
            .filter(|(_, pending)| !pending.resolved)
            .map(|(id, pending)| {
                (
                    id.clone(),
                    match pending.model {
                        Model::Builtin(_) => Event::BuiltinResult,
                        Model::Keymap => Event::KeymapResult,
                        _ => Event::UiResult,
                    },
                )
            })
            .collect();
        for (identity, event) in pending {
            let error = failure(
                ErrorCode::Cancelled,
                "plugin frontend request cancelled during shutdown",
            );
            let response = match event {
                Event::BuiltinResult => serde_json::to_value(BuiltinResponse {
                    identity: identity.clone(),
                    completed: 0,
                    error: Some(error),
                })
                .unwrap(),
                Event::KeymapResult => serde_json::to_value(KeymapResponse {
                    identity: identity.clone(),
                    error: Some(error),
                })
                .unwrap(),
                _ => serde_json::to_value(UiResponse {
                    identity: identity.clone(),
                    outcome: UiOutcome::Cancelled {
                        reason: UiCancellation::Shutdown,
                    },
                })
                .unwrap(),
            };
            let _ = self.finish_plugin_frontend(&identity, event, response);
            self.finish_plugin_frontend_task(
                &identity,
                crate::callbacks::TaskOutcome::Cancelled(
                    "plugin frontend request cancelled during shutdown".into(),
                ),
            );
        }
        self.plugins.frontend.ui.clear();
        self.plugins.frontend.builtins.clear();
        self.plugins.frontend.keymaps.clear();
    }
}

impl PreparedFrontend {
    fn identity(&self) -> &UiIdentity {
        match self {
            Self::Ui(request) => &request.identity,
            Self::Builtin(request) => &request.identity,
            Self::Keymap(request) => &request.identity,
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Ui(request) => json_size(request).saturating_mul(2),
            Self::Builtin(request) => json_size(request),
            Self::Keymap(request) => json_size(request),
        }
    }
}

fn failure(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}

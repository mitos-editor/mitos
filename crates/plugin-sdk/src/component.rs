//! Generated component bindings and bounded host-service helpers.

use crate::{JobPoll, JobRequest, ServiceError};

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod bindings {
    plugin_api::generate_bindings!(wit_bindgen::generate, world: "plugin", pub_export_macro: true);
}

#[cfg(target_arch = "wasm32")]
use bindings::mitos::plugin::{host, types};

#[cfg(target_arch = "wasm32")]
fn native_error(error: types::Failure) -> ServiceError {
    use crate::ErrorCode;
    ServiceError::new(
        match error.code {
            types::ErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
            types::ErrorCode::StaleState => ErrorCode::StaleState,
            types::ErrorCode::PermissionDenied => ErrorCode::PermissionDenied,
            types::ErrorCode::ResourceExhausted => ErrorCode::ResourceExhausted,
            types::ErrorCode::Cancelled => ErrorCode::Cancelled,
            types::ErrorCode::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            types::ErrorCode::UnsupportedInterface => ErrorCode::UnsupportedInterface,
            types::ErrorCode::GuestTrap => ErrorCode::GuestTrap,
            types::ErrorCode::HostFailure => ErrorCode::HostFailure,
        },
        error.message,
    )
}
#[cfg(not(target_arch = "wasm32"))]
fn unavailable() -> ServiceError {
    ServiceError::new(
        crate::ErrorCode::UnsupportedInterface,
        "plugin host services require a component guest",
    )
}

/// Read one snapshot region; offsets count Unicode scalar values.
pub fn read_document(
    document: u64,
    version: i32,
    start: u64,
    end: u64,
) -> Result<String, ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        host::read_document(document, version, start, end).map_err(native_error)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (document, version, start, end);
        Err(unavailable())
    }
}

/// Call one closed editor service. Arbitrary methods and expansion strings are
/// not accepted; the host validates the typed request and its capabilities.
pub fn editor_request(
    request: crate::editor::EditorRequest,
) -> Result<crate::editor::EditorReply, ServiceError> {
    request.validate()?;
    #[cfg(target_arch = "wasm32")]
    {
        let payload = serde_json::to_string(&request).map_err(|_| {
            ServiceError::new(
                crate::ErrorCode::InvalidRequest,
                "editor request cannot be encoded",
            )
        })?;
        if payload.len() > 4096 {
            return Err(ServiceError::new(
                crate::ErrorCode::ResourceExhausted,
                "editor request exceeds its byte limit",
            ));
        }
        let reply = host::editor_request(&payload).map_err(native_error)?;
        serde_json::from_str(&reply).map_err(|_| {
            ServiceError::new(
                crate::ErrorCode::InvalidRequest,
                "host returned an invalid editor result",
            )
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err(unavailable())
    }
}

/// Read a UTF-8 file relative to a granted root, without ambient filesystem access.
pub fn read_file(root: u32, path: &str) -> Result<String, ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        host::read_file(root, path).map_err(native_error)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (root, path);
        Err(unavailable())
    }
}
pub fn write_file(root: u32, path: &str, value: &str) -> Result<(), ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        host::write_file(root, path)
            .map_err(native_error)?
            .write(value)
            .map_err(native_error)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (root, path, value);
        Err(unavailable())
    }
}
pub fn storage_read(key: &str) -> Result<Option<String>, ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        host::storage_read(key).map_err(native_error)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Err(unavailable())
    }
}
pub fn storage_write(key: &str, value: &str) -> Result<(), ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        host::storage_write(key)
            .map_err(native_error)?
            .write(value)
            .map_err(native_error)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
        Err(unavailable())
    }
}

/// An owned host job. Cancel/drop revokes its work and waits for host cleanup.
pub struct Job {
    #[cfg(target_arch = "wasm32")]
    handle: host::Job,
}
impl Job {
    /// Host-owned identity carried in targeted `Event::JobReady` data.
    pub fn id(&self) -> Result<u64, ServiceError> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(self.handle.id())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(unavailable())
        }
    }
    pub fn poll(&self) -> Result<JobPoll, ServiceError> {
        #[cfg(target_arch = "wasm32")]
        {
            let value = self.handle.poll().map_err(native_error)?;
            serde_json::from_str(&value).map_err(|_| {
                ServiceError::new(
                    crate::ErrorCode::InvalidRequest,
                    "host returned an invalid typed job result",
                )
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(unavailable())
        }
    }
    pub fn cancel(&self) -> Result<(), ServiceError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.handle.cancel().map_err(native_error)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(unavailable())
        }
    }
}
pub fn start_job(request: JobRequest) -> Result<Job, ServiceError> {
    #[cfg(target_arch = "wasm32")]
    {
        let handle = match request {
            JobRequest::Timer { milliseconds } => host::start_timer(milliseconds),
            JobRequest::Search { root, query } => host::start_search(root, &query),
            JobRequest::Process {
                command,
                args,
                input,
                root,
                timeout_milliseconds,
            } => {
                let process = host::process(&command, root).map_err(native_error)?;
                process
                    .timeout(timeout_milliseconds)
                    .map_err(native_error)?;
                process.input(&input).map_err(native_error)?;
                for argument in args {
                    process.argument(&argument).map_err(native_error)?;
                }
                process.start()
            }
        }
        .map_err(native_error)?;
        Ok(Job { handle })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = request;
        Err(unavailable())
    }
}

#[cfg(target_arch = "wasm32")]
fn invalid(message: impl Into<String>) -> types::Failure {
    types::Failure {
        code: types::ErrorCode::InvalidRequest,
        message: message.into(),
    }
}

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn dispatch(
    request: types::Request,
    handler: fn(crate::Request) -> crate::Response,
) -> Result<(), types::Failure> {
    use crate::{
        Action, DocumentSnapshot, EditorContext, Event, Request, SelectionRange, StateQuery,
        ViewSnapshot, ABI_VERSION,
    };
    if request.abi_version != ABI_VERSION {
        return Err(invalid("plugin interface version is unsupported"));
    }
    let event = match request.event {
        types::Event::Init => Event::Init,
        types::Event::Shutdown => Event::Shutdown,
        types::Event::Command => Event::Command,
        types::Event::DocumentOpened => Event::DocumentOpened,
        types::Event::DocumentChanged => Event::DocumentChanged,
        types::Event::DocumentSaved => Event::DocumentSaved,
        types::Event::DocumentClosed => Event::DocumentClosed,
        types::Event::SelectionChanged => Event::SelectionChanged,
        types::Event::ModeChanged => Event::ModeChanged,
        types::Event::PostCommand => Event::PostCommand,
        types::Event::PostInsertChar => Event::PostInsertChar,
        types::Event::DocumentFocusLost => Event::DocumentFocusLost,
        types::Event::TerminalFocusGained => Event::TerminalFocusGained,
        types::Event::TerminalFocusLost => Event::TerminalFocusLost,
        types::Event::ResyncRequired => Event::ResyncRequired,
        types::Event::State => Event::State,
        types::Event::UiResult => Event::UiResult,
        types::Event::BuiltinResult => Event::BuiltinResult,
        types::Event::KeymapResult => Event::KeymapResult,
        types::Event::JobReady => Event::JobReady,
    };
    let response = handler(Request {
        abi_version: request.abi_version,
        event,
        command: request.command,
        args: request.args,
        config: serde_json::from_str(&request.config_json)
            .map_err(|_| invalid("host configuration is invalid JSON"))?,
        data: serde_json::from_str(&request.data_json)
            .map_err(|_| invalid("host event data is invalid JSON"))?,
        editor: EditorContext {
            generation: request.editor.generation,
            mode: request.editor.mode,
            document: request.editor.document.map(|doc| DocumentSnapshot {
                id: doc.id,
                version: doc.version,
                path: doc.path,
                language: doc.language,
                char_count: doc.char_count,
                byte_count: doc.byte_count,
            }),
            view: request
                .editor
                .view
                .map(|view| -> Result<ViewSnapshot, types::Failure> {
                    let selections = view
                        .selections
                        .into_iter()
                        .map(|range| -> Result<SelectionRange, types::Failure> {
                            Ok(SelectionRange {
                                anchor: range.anchor.try_into().map_err(|_| {
                                    invalid("selection offset exceeds this Rust guest's range")
                                })?,
                                head: range.head.try_into().map_err(|_| {
                                    invalid("selection offset exceeds this Rust guest's range")
                                })?,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ViewSnapshot {
                        id: view.id,
                        document: view.document,
                        binding_revision: view.binding_revision,
                        selection_revision: view.selection_revision,
                        selections,
                        primary: view.primary as usize,
                    })
                })
                .transpose()?,
        },
    });
    if let Some(message) = response.error {
        return Err(invalid(message));
    }
    if response.actions.is_empty() {
        return Ok(());
    }
    let effects = host::begin_effects()?;
    for action in response.actions {
        match action {
            Action::Edit {
                document,
                version,
                edits,
            } => {
                let group = effects.edit(document, version)?;
                for edit in edits {
                    group.add(edit.start as u64, edit.end as u64, &edit.text)?;
                }
                group.finish()?;
            }
            Action::SetSelection {
                document,
                version,
                view,
                binding_revision,
                selection_revision,
                ranges,
                primary,
            } => {
                let group = effects.selection(
                    document,
                    version,
                    view,
                    binding_revision,
                    selection_revision,
                    primary
                        .try_into()
                        .map_err(|_| invalid("primary selection exceeds interface range"))?,
                )?;
                for range in ranges {
                    group.add(range.anchor as u64, range.head as u64)?;
                }
                group.finish()?;
            }
            Action::Status { message } => effects.status(&message)?,
            Action::Error { message } => effects.error(&message)?,
            Action::Open { path } => effects.open(&path)?,
            Action::ShowUi {
                request,
                origin,
                kind,
            } => {
                let origin = origin.map(wit_origin);
                match kind {
                    crate::ui::UiKind::Prompt { title, initial } => {
                        let prompt = effects.prompt(request, origin, &title)?;
                        prompt.initial(&initial)?;
                        prompt.finish()?;
                    }
                    crate::ui::UiKind::NextKey { title, timeout_ms } => {
                        effects.next_key(request, origin, &title, timeout_ms)?
                    }
                    crate::ui::UiKind::Picker { title, rows } => {
                        let picker = effects.picker(request, origin, &title)?;
                        for row in rows {
                            let entry = picker.row(&row.id)?;
                            entry.label(&row.label)?;
                            entry.description(&row.description)?;
                            if let Some(preview) = row.preview {
                                entry.preview(&preview)?;
                            }
                            if let Some(location) = row.location {
                                entry.location(types::UiLocation {
                                    document: location.document,
                                    version: location.version,
                                    offset: location.offset as u64,
                                })?;
                            }
                            entry.finish()?;
                        }
                        picker.finish()?;
                    }
                }
            }
            Action::InvokeBuiltin {
                request,
                origin,
                commands,
            } => {
                let group = effects.builtins(request, wit_origin(origin))?;
                for invocation in commands {
                    group.add(
                        wit_builtin(invocation.command),
                        invocation
                            .count
                            .map(|count| {
                                u32::try_from(count)
                                    .map_err(|_| invalid("builtin count exceeds interface range"))
                            })
                            .transpose()?,
                    )?;
                }
                group.finish()?;
            }
            Action::UpdateKeymap { request, bindings } => {
                let group = effects.keymap(request)?;
                for binding in bindings {
                    let mode = match binding.mode {
                        crate::ui::KeymapMode::Normal => types::KeymapMode::Normal,
                        crate::ui::KeymapMode::Select => types::KeymapMode::Select,
                        crate::ui::KeymapMode::Insert => types::KeymapMode::Insert,
                    };
                    let keys = group.binding(mode, &binding.command)?;
                    for key in binding.keys {
                        keys.key(&key)?;
                    }
                    keys.finish()?;
                }
                group.finish()?;
            }
            Action::RequestState {
                query:
                    StateQuery {
                        document,
                        view,
                        after_document,
                        after_view,
                        limit,
                    },
            } => effects.request_state(types::StateQuery {
                document,
                view,
                after_document,
                after_view,
                limit: limit
                    .try_into()
                    .map_err(|_| invalid("catalog page exceeds interface range"))?,
            })?,
        }
    }
    effects.finish()
}

#[cfg(target_arch = "wasm32")]
fn wit_origin(origin: crate::ui::UiOrigin) -> types::UiOrigin {
    types::UiOrigin {
        view: origin.view,
        document: origin.document,
        binding_revision: origin.binding_revision,
        version: origin.version,
        selection_revision: origin.selection_revision,
    }
}
#[cfg(target_arch = "wasm32")]
fn wit_builtin(command: crate::ui::BuiltinCommand) -> types::BuiltinCommand {
    use crate::ui::BuiltinCommand as B;
    match command {
        B::MoveCharLeft => types::BuiltinCommand::MoveCharLeft,
        B::MoveCharRight => types::BuiltinCommand::MoveCharRight,
        B::MoveLineUp => types::BuiltinCommand::MoveLineUp,
        B::MoveLineDown => types::BuiltinCommand::MoveLineDown,
        B::SelectAll => types::BuiltinCommand::SelectAll,
        B::CollapseSelection => types::BuiltinCommand::CollapseSelection,
        B::KeepPrimarySelection => types::BuiltinCommand::KeepPrimarySelection,
        B::DeleteSelectionNoYank => types::BuiltinCommand::DeleteSelectionNoYank,
        B::ChangeSelectionNoYank => types::BuiltinCommand::ChangeSelectionNoYank,
        B::Undo => types::BuiltinCommand::Undo,
        B::Redo => types::BuiltinCommand::Redo,
        B::InsertMode => types::BuiltinCommand::InsertMode,
        B::NormalMode => types::BuiltinCommand::NormalMode,
    }
}

/// Export a `fn(Request) -> Response` handler through the typed component world.
/// Package the compiled core module into a component before installing it.
#[macro_export]
macro_rules! export_component {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            struct MitosComponent;
            impl $crate::component::bindings::Guest for MitosComponent {
                fn handle(request: $crate::component::bindings::Request) -> Result<(), $crate::component::bindings::Failure> {
                    $crate::component::dispatch(request, $handler)
                }
            }
            $crate::component::bindings::export!(MitosComponent with_types_in $crate::component::bindings);
        };
    };
}

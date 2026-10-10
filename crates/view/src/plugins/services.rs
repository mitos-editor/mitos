//! Immediate, capability checked native milestones. These are not staged edits.
use super::*;
use plugin_api::{
    editor::{
        DocumentTarget, EditorReply, EditorRequest, OpenDisposition, SettingValue, UnsavedDocument,
        ViewTarget, MAX_UNSAVED_DOCUMENTS,
    },
    ui::UiOwner,
    HostFuture,
};

fn failure(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}
fn cancelled() -> ServiceError {
    failure(ErrorCode::Cancelled, "plugin editor request revoked")
}
fn stale(error: impl std::fmt::Display) -> ServiceError {
    failure(ErrorCode::StaleState, error.to_string())
}
fn host(error: impl std::fmt::Display) -> ServiceError {
    failure(ErrorCode::HostFailure, error.to_string())
}
fn disposition(action: OpenDisposition) -> crate::editor::Action {
    match action {
        OpenDisposition::Replace => crate::editor::Action::Replace,
        OpenDisposition::HorizontalSplit => crate::editor::Action::HorizontalSplit,
        OpenDisposition::VerticalSplit => crate::editor::Action::VerticalSplit,
    }
}

struct Prepared {
    document: Option<crate::document::PreparedPluginDocument>,
    theme: Option<crate::Theme>,
    _open_quota: Option<tokio::sync::OwnedSemaphorePermit>,
}

pub(super) struct Scope {
    pub owner: Weak<Shared>,
    pub callbacks: EditorCallbackSender,
    pub policy: Arc<::plugins::policy::AccessPolicy>,
    pub source: PluginEventSource,
    pub event: Event,
    pub work: Arc<tokio::sync::Semaphore>,
    pub open_bytes: Arc<tokio::sync::Semaphore>,
}

pub(super) fn request(request: EditorRequest, scope: Scope) -> HostFuture<EditorReply> {
    let Scope {
        owner,
        callbacks,
        policy,
        source,
        event,
        work,
        open_bytes,
    } = scope;
    Box::pin(async move {
        request.validate()?;
        if json_size(&request) > plugin_api::editor::MAX_EDITOR_REQUEST_BYTES {
            return Err(failure(
                ErrorCode::ResourceExhausted,
                "editor request exceeds 4 KiB",
            ));
        }
        for cap in request.capabilities() {
            policy.require(cap)?;
        }
        let shared = owner.upgrade().ok_or_else(cancelled)?;
        if !shared.accepting.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        if matches!(
            &request,
            EditorRequest::ReadRegister {
                name: '*' | '+',
                ..
            } | EditorRequest::WriteRegister {
                name: '*' | '+',
                ..
            }
        ) {
            return clipboard(request, owner, callbacks, policy, source, work).await;
        }
        let mut prepared = Prepared {
            document: None,
            theme: None,
            _open_quota: None,
        };
        match &request {
            EditorRequest::OpenAt { path, .. } => {
                let policy = policy.clone();
                let path = path.clone();
                let quota = open_bytes
                    .acquire_many_owned(::plugins::MAX_MESSAGE_BYTES as u32)
                    .await
                    .map_err(|_| cancelled())?;
                let permit = work
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| cancelled())?;
                let (document, quota) = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let (path, bytes) = policy.read_path(Path::new(&path))?;
                    let doc = Document::prepare_plugin_bytes(
                        path,
                        bytes,
                        editor_core::editor_config::EditorConfig::default(),
                    )
                    .map_err(host)?;
                    if doc.binary {
                        return Err(failure(
                            ErrorCode::InvalidRequest,
                            "plugin navigation only supports text files",
                        ));
                    }
                    if doc.byte_count() > ::plugins::MAX_MESSAGE_BYTES {
                        return Err(failure(
                            ErrorCode::ResourceExhausted,
                            "decoded open exceeds 4 MiB",
                        ));
                    }
                    Ok((doc, quota))
                })
                .await
                .map_err(host)??;
                prepared.document = Some(document);
                prepared._open_quota = Some(quota);
            }
            EditorRequest::OverrideSetting {
                value: SettingValue::Theme(name),
                ..
            } => {
                let name = name.clone();
                let (send, receive) = tokio::sync::oneshot::channel();
                let weak = owner.clone();
                callbacks
                    .send(move |editor| {
                        let result = weak
                            .upgrade()
                            .filter(|shared| Arc::ptr_eq(shared, &editor.plugins.shared))
                            .map(|_| editor.theme_loader.clone())
                            .ok_or_else(cancelled);
                        let _ = send.send(result);
                    })
                    .await;
                let loader = receive.await.map_err(|_| cancelled())??;
                let permit = work
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| cancelled())?;
                prepared.theme = Some(
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let theme = loader.load(&name).map_err(host)?;
                        if theme.find_highlight_exact("ui.selection").is_none() {
                            return Err(failure(
                                ErrorCode::InvalidRequest,
                                "theme requires ui.selection",
                            ));
                        }
                        Ok(theme)
                    })
                    .await
                    .map_err(host)??,
                );
            }
            _ => (),
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        callbacks
            .send(move |editor| {
                // A cancelled waiter must not leave an unobserved queued mutation.
                if send.is_closed() {
                    return;
                }
                let result = (|| {
                    let shared = owner.upgrade().ok_or_else(cancelled)?;
                    if !Arc::ptr_eq(&shared, &editor.plugins.shared) || editor.plugins.stopped {
                        return Err(cancelled());
                    }
                    for cap in request.capabilities() {
                        policy.require(cap)?;
                    }
                    if mutates(&request)
                        && (editor.plugins.shutting_down
                            || (editor.tree.views().next().is_none() && event != Event::Init))
                    {
                        return Err(cancelled());
                    }
                    let previous =
                        std::mem::replace(&mut shared.queue.lock().origin, source.origin.clone());
                    let _guard = ApplyingGuard {
                        owner: shared,
                        previous,
                    };
                    let caller = UiOwner {
                        plugin: source.origin.as_ref().ok_or_else(cancelled)?.plugin.clone(),
                        generation: source.generation,
                    };
                    let navigates = matches!(
                        &request,
                        EditorRequest::Focus { .. }
                            | EditorRequest::Split { .. }
                            | EditorRequest::OpenAt { .. }
                            | EditorRequest::Scratch { .. }
                            | EditorRequest::CloseView { .. }
                            | EditorRequest::CloseDocument { .. }
                    );
                    let result = apply(editor, request, prepared, caller.clone())?;
                    if navigates {
                        editor.record_plugin_navigation(&source, &result);
                    }
                    if json_size(&result) > plugin_api::editor::MAX_EDITOR_REPLY_BYTES {
                        return Err(failure(
                            ErrorCode::ResourceExhausted,
                            "editor reply exceeds 1 MiB",
                        ));
                    }
                    Ok(result)
                })();
                let _ = send.send(result);
            })
            .await;
        receive.await.map_err(|_| cancelled())?
    })
}

fn mutates(request: &EditorRequest) -> bool {
    !matches!(
        request,
        EditorRequest::DocumentStatus { .. }
            | EditorRequest::UnsavedDocuments { .. }
            | EditorRequest::ReadRegister { .. }
            | EditorRequest::ReadSettings { .. }
            | EditorRequest::SyntaxQuery { .. }
            | EditorRequest::LanguageHover { .. }
            | EditorRequest::LanguageSymbols { .. }
            | EditorRequest::LanguageFormat { .. }
            | EditorRequest::LanguageCodeActions { .. }
    )
}

pub(super) fn view(editor: &Editor, target: ViewTarget) -> Result<ViewId, ServiceError> {
    let id = editor
        .plugin_document(target.document, target.version)
        .map_err(stale)?;
    let view = ViewId::from_u64(target.view);
    let actual = editor
        .tree
        .try_get(view)
        .ok_or_else(|| stale("view is closed"))?;
    if actual.doc != id || actual.binding_revision() != target.binding_revision {
        return Err(stale("view has been rebound"));
    }
    if editor
        .document(id)
        .and_then(|doc| doc.selection_revision(view))
        != Some(target.selection_revision)
    {
        return Err(stale("view selection changed"));
    }
    Ok(view)
}
fn current_target(editor: &Editor, view: ViewId) -> Result<ViewTarget, ServiceError> {
    let actual = editor
        .tree
        .try_get(view)
        .ok_or_else(|| stale("view is closed"))?;
    let doc = editor
        .document(actual.doc)
        .ok_or_else(|| stale("document is closed"))?;
    Ok(ViewTarget {
        view: view.as_u64(),
        document: doc.id().as_u64(),
        version: doc.version(),
        binding_revision: actual.binding_revision(),
        selection_revision: doc
            .selection_revision(view)
            .ok_or_else(|| stale("view has no selection"))?,
    })
}
fn origin(editor: &mut Editor, target: Option<ViewTarget>) -> Result<(), ServiceError> {
    if let Some(target) = target {
        let target = view(editor, target)?;
        editor.focus(target);
    } else if editor.tree.views().next().is_some() {
        return Err(stale("navigation requires an explicit originating view"));
    }
    Ok(())
}
fn apply(
    editor: &mut Editor,
    request: EditorRequest,
    prepared: Prepared,
    caller: UiOwner,
) -> Result<EditorReply, ServiceError> {
    match request {
        EditorRequest::DocumentStatus { target } => {
            let id = editor
                .plugin_document(target.document, target.version)
                .map_err(stale)?;
            let doc = editor
                .document(id)
                .ok_or_else(|| stale("document is closed"))?;
            Ok(EditorReply::DocumentStatus {
                target,
                modified: doc.is_modified(),
            })
        }
        EditorRequest::UnsavedDocuments { max_documents } => {
            let limit = if max_documents == 0 {
                MAX_UNSAVED_DOCUMENTS
            } else {
                max_documents
            } as usize;
            let mut modified = editor.documents().filter(|doc| doc.is_modified());
            let documents = modified
                .by_ref()
                .take(limit)
                .map(|doc| UnsavedDocument {
                    target: DocumentTarget {
                        document: doc.id().as_u64(),
                        version: doc.version(),
                    },
                    path: doc.path().map(|path| path.to_string_lossy().into_owned()),
                })
                .collect();
            Ok(EditorReply::UnsavedDocuments {
                documents,
                truncated: modified.next().is_some(),
            })
        }
        EditorRequest::Focus { target } => {
            let id = view(editor, target)?;
            editor.focus(id);
            Ok(EditorReply::View {
                target: current_target(editor, id)?,
            })
        }
        EditorRequest::Split { target, action } => {
            let id = view(editor, target)?;
            editor.focus(id);
            editor.switch(editor.tree.get(id).doc, disposition(action));
            Ok(EditorReply::View {
                target: current_target(editor, editor.tree.focus)?,
            })
        }
        EditorRequest::OpenAt {
            origin: target,
            line,
            column,
            action,
            ..
        } => {
            if let Some(target) = target {
                view(editor, target)?;
            } else if editor.tree.views().next().is_some() {
                return Err(stale("open requires an explicit originating view"));
            }
            let prepared = prepared
                .document
                .ok_or_else(|| host("open was not prepared"))?;
            let doc = Document::from_prepared_plugin(
                prepared,
                editor.config.clone(),
                editor.syn_loader.clone(),
            );
            let text = doc
                .path()
                .and_then(|path| editor.document_id_by_path(path))
                .and_then(|id| editor.document(id))
                .map_or_else(|| doc.text(), Document::text);
            let line = usize::try_from(line)
                .map_err(|_| failure(ErrorCode::InvalidRequest, "line is out of bounds"))?;
            let column = usize::try_from(column)
                .map_err(|_| failure(ErrorCode::InvalidRequest, "column is out of bounds"))?;
            if line >= text.len_lines() {
                return Err(failure(ErrorCode::InvalidRequest, "line is out of bounds"));
            }
            let slice = text.line(line);
            let ending = slice
                .chars()
                .reversed()
                .take_while(|c| matches!(c, '\r' | '\n'))
                .count();
            if column > slice.len_chars() - ending {
                return Err(failure(
                    ErrorCode::InvalidRequest,
                    "column is out of bounds",
                ));
            }
            let offset = text.line_to_char(line) + column;
            origin(editor, target)?;
            let id = editor.adopt_plugin_document(doc, disposition(action));
            let view = editor.tree.focus;
            editor
                .document_mut(id)
                .unwrap()
                .set_selection(view, Selection::point(offset));
            let scrolloff = editor.config().scrolloff;
            editor
                .tree
                .get_mut(view)
                .ensure_cursor_in_view(editor.documents.get_mut(&id).unwrap(), scrolloff);
            Ok(EditorReply::View {
                target: current_target(editor, view)?,
            })
        }
        EditorRequest::Scratch {
            name,
            text,
            language,
            origin: target,
            action,
        } => {
            if text.len() > plugin_api::editor::MAX_EDITOR_REQUEST_BYTES {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "scratch text exceeds 4 KiB",
                ));
            }
            let language = language
                .map(|name| {
                    editor
                        .syn_loader
                        .load()
                        .language_for_name(name.as_str())
                        .map(|id| editor.syn_loader.load().language(id).config().clone())
                        .ok_or_else(|| {
                            failure(ErrorCode::InvalidRequest, "unknown scratch language")
                        })
                })
                .transpose()?;
            // Validate every fallible input before changing focus or opening a view.
            if let Some(target) = target {
                view(editor, target)?;
            } else if editor.tree.views().next().is_some() {
                return Err(stale("scratch requires an explicit originating view"));
            }
            let doc = Document::plugin_scratch(
                name,
                text,
                language,
                editor.config.clone(),
                editor.syn_loader.clone(),
            );
            origin(editor, target)?;
            editor.adopt_plugin_scratch(doc, disposition(action));
            Ok(EditorReply::View {
                target: current_target(editor, editor.tree.focus)?,
            })
        }
        EditorRequest::CloseView { target } => {
            let id = view(editor, target)?;
            editor.close(id);
            Ok(EditorReply::Closed {
                document: None,
                view: Some(target.view),
            })
        }
        EditorRequest::CloseDocument { target } => {
            let id = editor
                .plugin_document(target.document, target.version)
                .map_err(stale)?;
            editor
                .close_document(id, false)
                .map_err(|error| match error {
                    crate::editor::CloseError::DoesNotExist => stale("document is closed"),
                    crate::editor::CloseError::BufferModified(name) => {
                        stale(format!("cannot close modified document {name}"))
                    }
                    crate::editor::CloseError::SaveError(error) => host(error),
                })?;
            Ok(EditorReply::Closed {
                document: Some(target.document),
                view: None,
            })
        }
        EditorRequest::ReadRegister {
            name: '*' | '+', ..
        }
        | EditorRequest::WriteRegister {
            name: '*' | '+', ..
        } => Err(host("clipboard request was not prepared")),
        EditorRequest::ReadRegister { name, origin } => {
            let values = if matches!(name, '#' | '.' | '%') {
                let origin = origin.ok_or_else(|| {
                    failure(
                        ErrorCode::InvalidRequest,
                        "derived register requires origin",
                    )
                })?;
                let view = view(editor, origin)?;
                let doc = editor.document(editor.tree.get(view).doc).unwrap();
                let selection = doc.selection(view);
                if selection.len() > plugin_api::editor::MAX_REGISTER_VALUES {
                    return Err(failure(
                        ErrorCode::ResourceExhausted,
                        "derived register exceeds 64 values",
                    ));
                }
                match name {
                    '#' => (1..=selection.len())
                        .map(|index| index.to_string())
                        .collect(),
                    '%' => {
                        let name = doc.display_name();
                        if name.len() > plugin_api::editor::MAX_REGISTER_BYTES {
                            return Err(failure(
                                ErrorCode::ResourceExhausted,
                                "register exceeds 4 KiB",
                            ));
                        }
                        vec![name.into_owned()]
                    }
                    '.' => {
                        let bytes = selection
                            .iter()
                            .map(|range| {
                                doc.text().char_to_byte(range.to())
                                    - doc.text().char_to_byte(range.from())
                            })
                            .sum::<usize>();
                        if bytes > plugin_api::editor::MAX_REGISTER_BYTES {
                            return Err(failure(
                                ErrorCode::ResourceExhausted,
                                "register exceeds 4 KiB",
                            ));
                        }
                        selection
                            .fragments(doc.text().slice(..))
                            .map(|value| value.into_owned())
                            .collect()
                    }
                    _ => unreachable!(),
                }
            } else {
                let mut values = Vec::new();
                let mut bytes = 0usize;
                if let Some(register) = editor.registers.read(name, editor) {
                    for value in register {
                        bytes = bytes.saturating_add(value.len());
                        if values.len() >= plugin_api::editor::MAX_REGISTER_VALUES
                            || bytes > plugin_api::editor::MAX_REGISTER_BYTES
                        {
                            return Err(failure(
                                ErrorCode::ResourceExhausted,
                                "register exceeds 64 values / 4 KiB",
                            ));
                        }
                        values.push(value.into_owned());
                    }
                }
                values
            };
            Ok(EditorReply::Register { values })
        }
        EditorRequest::WriteRegister { name, values } => {
            editor.registers.write(name, values).map_err(host)?;
            Ok(EditorReply::Updated)
        }
        EditorRequest::ReadSettings { scope } => Ok(EditorReply::Settings {
            values: editor.plugin_read_settings(scope)?,
        }),
        EditorRequest::OverrideSetting { scope, value } => {
            editor.plugin_override_setting(caller, scope, value, prepared.theme)?;
            Ok(EditorReply::Updated)
        }
        EditorRequest::ClearSettings { scope } => {
            editor.plugin_clear_settings(&caller, scope)?;
            Ok(EditorReply::Updated)
        }
        _ => Err(failure(
            ErrorCode::UnsupportedInterface,
            "language request requires the language adapter",
        )),
    }
}

async fn clipboard(
    request: EditorRequest,
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
    policy: Arc<::plugins::policy::AccessPolicy>,
    source: PluginEventSource,
    work: Arc<tokio::sync::Semaphore>,
) -> Result<EditorReply, ServiceError> {
    let (name, write) = match &request {
        EditorRequest::ReadRegister { name, .. } => (*name, false),
        EditorRequest::WriteRegister { name, .. } => (*name, true),
        _ => return Err(host("not a clipboard request")),
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    let weak = owner.clone();
    callbacks
        .send(move |editor| {
            if send.is_closed() {
                return;
            }
            let result = weak
                .upgrade()
                .filter(|owner| {
                    Arc::ptr_eq(owner, &editor.plugins.shared)
                        && !editor.plugins.stopped
                        && !editor.plugins.shutting_down
                })
                .map(|_| editor.registers.plugin_clipboard_snapshot())
                .ok_or_else(cancelled);
            let _ = send.send(result);
        })
        .await;
    let (provider, backend) = receive.await.map_err(|_| cancelled())??;
    let kind = if name == '+' {
        crate::clipboard::ClipboardType::Clipboard
    } else {
        crate::clipboard::ClipboardType::Selection
    };
    if let Some((command, args)) = provider.plugin_custom_command(kind, write) {
        policy
            .permissions
            .require_process(&policy.declared, &command, &args)?;
    }
    let _permit = work.acquire_owned().await.map_err(|_| cancelled())?;
    policy.require(Capability::Clipboard)?;
    match request {
        EditorRequest::ReadRegister { .. } => {
            let value = backend.get_plugin_contents(provider, kind).await?;
            policy.require(Capability::Clipboard)?;
            if owner
                .upgrade()
                .is_none_or(|shared| !shared.accepting.load(Ordering::Acquire))
            {
                return Err(cancelled());
            }
            if value.len() > plugin_api::editor::MAX_REGISTER_BYTES {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "clipboard exceeds 4 KiB",
                ));
            }
            Ok(EditorReply::Register {
                values: vec![value],
            })
        }
        EditorRequest::WriteRegister { values, .. } => {
            let text = values.join(editor_core::NATIVE_LINE_ENDING.as_str());
            if text.len() > plugin_api::editor::MAX_REGISTER_BYTES {
                return Err(failure(
                    ErrorCode::ResourceExhausted,
                    "clipboard exceeds 4 KiB",
                ));
            }
            backend.set_plugin_contents(provider, text, kind).await?;
            // Native clipboard milestone is explicit; a later guest trap does not undo it.
            let (send, receive) = tokio::sync::oneshot::channel();
            callbacks
                .send(move |editor| {
                    if send.is_closed() {
                        return;
                    }
                    let result = (|| {
                        let shared = owner.upgrade().ok_or_else(cancelled)?;
                        if !Arc::ptr_eq(&shared, &editor.plugins.shared)
                            || editor.plugins.stopped
                            || editor.plugins.shutting_down
                        {
                            return Err(cancelled());
                        }
                        policy.require(Capability::Clipboard)?;
                        policy.require(Capability::EditorSelection)?;
                        let previous =
                            std::mem::replace(&mut shared.queue.lock().origin, source.origin);
                        let _guard = ApplyingGuard {
                            owner: shared,
                            previous,
                        };
                        editor.registers.plugin_store_clipboard(name, values);
                        Ok(EditorReply::Updated)
                    })();
                    let _ = send.send(result);
                })
                .await;
            receive.await.map_err(|_| cancelled())?
        }
        _ => unreachable!(),
    }
}

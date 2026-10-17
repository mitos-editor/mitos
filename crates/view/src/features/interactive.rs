//! Frontend-independent interactive LSP commands. No RPC stays open during input.

use std::{
    collections::{HashSet, VecDeque},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use lsp_client::{
    lsp::{self, FormFieldType},
    LanguageServerId,
};
use parking_lot::Mutex;
use serde_json::Value;

use crate::{
    action::LspActionContext,
    events::{ConfigDidChange, DocumentDidChange, DocumentDidClose, LanguageServerExited},
    Editor,
};

#[derive(Default)]
pub(crate) struct InteractiveHandler {
    active: Option<Arc<Session>>,
    pending: VecDeque<FormRequest>,
}

pub(crate) struct Session {
    controller: Mutex<TaskController>,
    handle: TaskHandle,
    context: LspActionContext,
    server: LanguageServerId,
    base_dir: PathBuf,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InteractiveSession")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl Session {
    fn cancel(&self) {
        self.controller.lock().cancel();
    }
    fn is_current(&self, editor: &Editor) -> bool {
        !self.handle.is_canceled() && self.context.is_current(editor, self.server)
    }
}

/// A round of questions. Dropping an unanswered request cancels the operation.
#[derive(Debug)]
pub struct FormRequest {
    session: Arc<Session>,
    params: Option<lsp::ExecuteCommandParams>,
}

impl Drop for FormRequest {
    fn drop(&mut self) {
        if self.params.is_some() {
            self.session.cancel();
        }
    }
}

impl FormRequest {
    pub fn fields(&self) -> &[lsp::FormField] {
        &self.params.as_ref().unwrap().interactive.form_fields
    }
    pub fn answers(&self) -> &[lsp::FormAnswer] {
        &self.params.as_ref().unwrap().interactive.form_answers
    }
    pub fn base_dir(&self) -> &Path {
        &self.session.base_dir
    }
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.session.is_current(editor)
    }

    pub fn list_enum(
        &self,
        editor: &Editor,
        params: lsp::InteractiveListEnumParams,
    ) -> Option<
        impl std::future::Future<Output = Option<lsp_client::Result<Vec<lsp::FormEnumEntry>>>> + use<>,
    > {
        if !self.is_current(editor) {
            return None;
        }
        let server = Arc::downgrade(editor.language_servers.get_by_id(self.session.server)?);
        let handle = self.session.handle.clone();
        Some(async move {
            if handle.is_canceled() {
                return None;
            }
            let server = server.upgrade()?;
            let future = server.interactive_list_enum(params);
            drop(server);
            cancelable_future(future, handle).await
        })
    }

    /// Submit exactly the server's current form, including null for skipped optional fields.
    pub fn submit(mut self, editor: &mut Editor, answers: Vec<lsp::FormAnswer>) {
        if !self.is_current(editor) {
            return;
        }
        let mut params = self.params.take().unwrap();
        let result = validate_answers(&params.interactive.form_fields, &answers);
        if let Err(err) = result {
            self.session.cancel();
            editor.set_error(|| format!("Invalid interactive answers: {err}"));
            return;
        }
        params.interactive.form_answers = answers;
        // FormFields is a server-to-client field, not part of the submission.
        params.interactive.form_fields.clear();
        resolve(editor, self.session.clone(), params);
    }
}

pub(crate) fn execute(
    editor: &mut Editor,
    command: lsp::Command,
    server_id: LanguageServerId,
    context: LspActionContext,
) {
    let Some(server) = editor.language_server_by_id(server_id) else {
        return;
    };
    if server.capabilities().execute_command_provider.is_none() {
        editor.set_error(|| "Language server does not support executing commands");
        return;
    }
    let params = lsp::ExecuteCommandParams {
        command: command.command,
        arguments: command.arguments.unwrap_or_default(),
        ..Default::default()
    };
    if !server.supports_interactive_commands() {
        execute_resolved(editor, server_id, params);
        return;
    }
    if !context.is_current(editor, server_id) {
        editor.set_error(|| "Command is no longer current");
        return;
    }
    let doc = editor.document(context.document_id()).unwrap();
    let base_dir = doc
        .path()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."))
        .to_owned();
    let mut controller = TaskController::new();
    let handle = controller.restart();
    let session = Arc::new(Session {
        controller: Mutex::new(controller),
        handle,
        context,
        server: server_id,
        base_dir,
    });
    let handler = &mut editor.handlers.interactive;
    if let Some(previous) = handler.active.take() {
        previous.cancel();
    }
    handler.pending.clear();
    handler.active = Some(session.clone());
    editor
        .documents
        .get_mut(&session.context.document_id())
        .unwrap()
        .interactive_command = Some(Arc::downgrade(&session));
    resolve(editor, session, params);
}

fn resolve(editor: &mut Editor, session: Arc<Session>, params: lsp::ExecuteCommandParams) {
    if !session.is_current(editor) {
        return;
    }
    let future = editor
        .language_server_by_id(session.server)
        .unwrap()
        .resolve_command(params);
    let callbacks = editor.callback_sender().clone();
    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, session.handle.clone()).await else {
            return;
        };
        callbacks
            .send(move |editor| {
                if !session.is_current(editor) {
                    return;
                }
                match result {
                    Err(err) => {
                        session.cancel();
                        editor.set_error(|| format!("Failed to resolve LSP command: {err}"));
                    }
                    Ok(params) if params.interactive.form_fields.is_empty() => {
                        // Execute the returned command and arguments, which the server may have changed.
                        editor.handlers.interactive.active = None;
                        execute_resolved(editor, session.server, params);
                    }
                    Ok(params) => {
                        if let Err(err) = validate_form(&params.interactive) {
                            session.cancel();
                            editor.set_error(|| format!("Invalid interactive form: {err}"));
                            return;
                        }
                        editor.handlers.interactive.pending.push_back(FormRequest {
                            session,
                            params: Some(params),
                        });
                        editor.redraw().request();
                    }
                }
            })
            .await;
    });
}

fn execute_resolved(
    editor: &mut Editor,
    server_id: LanguageServerId,
    params: lsp::ExecuteCommandParams,
) {
    let Some(future) = editor
        .language_server_by_id(server_id)
        .and_then(|server| server.execute_command(params))
    else {
        return;
    };
    let callbacks = editor.callback_sender().clone();
    tokio::spawn(async move {
        if let Err(err) = future.await {
            callbacks
                .send(move |editor| {
                    editor.set_error(|| format!("Failed to execute LSP command: {err}"))
                })
                .await;
        }
    });
}

pub fn next_request(editor: &mut Editor) -> Option<FormRequest> {
    while let Some(request) = editor.handlers.interactive.pending.pop_front() {
        if request.is_current(editor) {
            return Some(request);
        }
    }
    None
}

/// Cancel the dialog or resolution RPC that belongs to the current operation.
pub fn cancel(editor: &mut Editor) -> bool {
    let Some(session) = editor.handlers.interactive.active.take() else {
        return false;
    };
    let running = !session.handle.is_canceled();
    session.cancel();
    editor.handlers.interactive.pending.clear();
    editor.redraw().request();
    running
}

fn supported(kind: &FormFieldType) -> bool {
    match kind {
        FormFieldType::Unknown => false,
        FormFieldType::List { element_type } => supported(element_type),
        _ => true,
    }
}

fn validate_form(params: &lsp::InteractiveParams) -> Result<(), &'static str> {
    let mut ids = HashSet::new();
    for field in &params.form_fields {
        if !ids.insert(&field.id) {
            return Err("duplicate field ID");
        }
        if !supported(&field.field_type) {
            return Err("unsupported input type");
        }
    }
    ids.clear();
    for answer in &params.form_answers {
        if !ids.insert(&answer.id) {
            return Err("duplicate answer ID");
        }
    }
    Ok(())
}

fn validate_answers(fields: &[lsp::FormField], answers: &[lsp::FormAnswer]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for answer in answers {
        if !ids.insert(&answer.id) {
            return Err("duplicate answer ID".into());
        }
        if !fields.iter().any(|field| field.id == answer.id) {
            return Err("unknown answer ID".into());
        }
    }
    for field in fields {
        let answer = answers
            .iter()
            .find(|answer| answer.id == field.id)
            .ok_or_else(|| format!("Missing answer for {}", field.description))?;
        if answer.value.is_null() && !field.required {
            continue;
        }
        validate_value(&field.field_type, &answer.value)?;
    }
    Ok(())
}

/// Validate typed answers before submission, also used by terminal prompts.
pub fn validate_value(kind: &FormFieldType, value: &Value) -> Result<(), String> {
    match kind {
        FormFieldType::String if value.is_string() => Ok(()),
        FormFieldType::Bool if value.is_boolean() => Ok(()),
        FormFieldType::Number if value.is_number() => Ok(()),
        FormFieldType::Enum { entries, .. }
            if entries
                .iter()
                .any(|entry| value.as_str() == Some(&entry.value)) =>
        {
            Ok(())
        }
        FormFieldType::LazyEnum { .. } if value.is_string() => Ok(()),
        FormFieldType::List { element_type } => {
            let values = value.as_array().ok_or("Expected a list")?;
            for value in values {
                validate_value(element_type, value)?;
            }
            Ok(())
        }
        FormFieldType::File {
            existence,
            file_type,
            filters,
        } => {
            let url = value
                .as_str()
                .and_then(|s| lsp::Url::parse(s).ok())
                .ok_or("Expected a file URI")?;
            let path = url
                .to_file_path()
                .map_err(|_| "Expected a local file URI")?;
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => Some(metadata),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(err.to_string()),
            };
            let exists = metadata.is_some();
            if existence.unwrap_or(3) & if exists { 2 } else { 1 } == 0 {
                return Err(if exists {
                    "Path must be new"
                } else {
                    "Path must already exist"
                }
                .into());
            }
            if let Some(metadata) = metadata {
                let actual_type = if metadata.is_file() {
                    1
                } else if metadata.is_dir() {
                    2
                } else {
                    0
                };
                if file_type.unwrap_or(1) & actual_type == 0 {
                    return Err("Wrong file type".into());
                }
            }
            if !filters.is_empty()
                && !path.is_dir()
                && !path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|ext| filters.iter().any(|filter| filter == ext))
            {
                return Err(format!("Expected extension: {}", filters.join(", ")));
            }
            Ok(())
        }
        _ => Err("Answer has the wrong type".into()),
    }
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if let Some(session) = event
            .doc
            .interactive_command
            .take()
            .and_then(|session| session.upgrade())
        {
            session.cancel();
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(session) = event
            .doc
            .interactive_command
            .take()
            .and_then(|session| session.upgrade())
        {
            session.cancel();
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        if let Some(session) = &event.editor.handlers.interactive.active
            && session.server == event.server_id
        {
            session.cancel();
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable
            && let Some(session) = &event.editor.handlers.interactive.active
        {
            session.cancel();
        }
        Ok(())
    });
}

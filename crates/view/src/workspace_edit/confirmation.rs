//! Editor-owned consent for annotated edits, bound to the server and affected state.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Weak,
    time::SystemTime,
};

use anyhow::{ensure, Context as _};
use lsp_client::{lsp, LanguageServerId, OffsetEncoding, UriExt as _};
use parking_lot::Mutex;

use super::{ApplyEditError, ApplyEditErrorKind};
use crate::{
    action::LspActionContext, callbacks::EditorCallbackSender, Document, DocumentId, Editor,
};

type Completion = Box<dyn FnOnce(&mut Editor, anyhow::Result<bool>) + Send>;

/// One queued prompt. Resolving it more than once has no effect.
pub struct ConfirmationRequest {
    annotations: Vec<lsp::ChangeAnnotation>,
    operation: Mutex<Option<Operation>>,
}

impl std::fmt::Debug for ConfirmationRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceEditConfirmation")
            .field("annotations", &self.annotations)
            .finish_non_exhaustive()
    }
}

struct Operation {
    edits: Edits,
    encoding: OffsetEncoding,
    owner: EditorCallbackSender,
    server: Weak<lsp_client::Client>,
    origin: Option<LspActionContext>,
    targets: Vec<Targets>,
    completed: Completion,
}

enum Edits {
    Workspace(lsp::WorkspaceEdit),
    Batch(Vec<lsp::WorkspaceEdit>),
}

impl Edits {
    fn iter(&self) -> std::slice::Iter<'_, lsp::WorkspaceEdit> {
        match self {
            Self::Workspace(edit) => std::slice::from_ref(edit).iter(),
            Self::Batch(edits) => edits.iter(),
        }
    }

    fn allow_confirmation(&mut self) {
        let edits = match self {
            Self::Workspace(edit) => std::slice::from_mut(edit),
            Self::Batch(edits) => edits.as_mut_slice(),
        };
        for edit in edits {
            if let Some(annotations) = &mut edit.change_annotations {
                for annotation in annotations.values_mut() {
                    annotation.needs_confirmation = Some(false);
                }
            }
        }
    }

    fn apply(self, editor: &mut Editor, encoding: OffsetEncoding) -> anyhow::Result<bool> {
        match self {
            Self::Workspace(edit) => editor
                .apply_workspace_edit(encoding, &edit)
                .map_err(anyhow::Error::from)?,
            Self::Batch(edits) => editor.apply_code_action_edits(encoding, edits)?,
        }
        Ok(true)
    }
}

impl Operation {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner.same_destination(editor.callback_sender())
            && editor.config().lsp.enable
            && self.server.upgrade().is_some_and(|server| {
                editor
                    .language_server_by_id(server.id())
                    .is_some_and(|current| std::ptr::eq(current, server.as_ref()))
                    && self
                        .origin
                        .as_ref()
                        .is_none_or(|origin| origin.is_current(editor, server.id()))
            })
            && self
                .targets
                .iter()
                .all(|targets| targets.is_current(editor))
    }
}

impl ConfirmationRequest {
    pub fn annotations(&self) -> &[lsp::ChangeAnnotation] {
        &self.annotations
    }

    pub fn is_current(&self, editor: &Editor) -> bool {
        self.operation
            .lock()
            .as_ref()
            .is_some_and(|operation| operation.is_current(editor))
    }

    /// Consent applies only to the captured edit and current affected state.
    pub fn resolve(&self, editor: &mut Editor, accepted: bool) {
        let mut pending = self.operation.lock();
        if pending
            .as_ref()
            .is_none_or(|operation| !operation.owner.same_destination(editor.callback_sender()))
        {
            return;
        }
        let mut operation = pending.take().unwrap();
        drop(pending);
        editor.present(|pending| pending.workspace_edit_active = false);
        let result = if !accepted {
            Ok(false)
        } else if !operation.is_current(editor) {
            Err(anyhow::anyhow!(
                "Workspace edit changed while awaiting confirmation; request it again"
            ))
        } else {
            // The public entrypoint still validates IDs and all normal edit constraints.
            operation.edits.allow_confirmation();
            operation.edits.apply(editor, operation.encoding)
        };
        (operation.completed)(editor, result);
    }
}

impl Editor {
    /// Apply directly when no confirmation is required, otherwise queue one editor-owned prompt.
    /// The completion runs once, after application, rejection, or stale-state detection.
    pub fn request_workspace_edit(
        &mut self,
        server: LanguageServerId,
        edit: lsp::WorkspaceEdit,
        origin: Option<LspActionContext>,
        completed: impl FnOnce(&mut Editor, anyhow::Result<bool>) + Send + 'static,
    ) {
        self.request_edits(server, Edits::Workspace(edit), origin, Box::new(completed));
    }

    /// Preserve per-action annotation namespaces until the shared batch preflight applies them.
    pub fn request_code_action_edits(
        &mut self,
        server: LanguageServerId,
        edits: Vec<lsp::WorkspaceEdit>,
        origin: Option<LspActionContext>,
        completed: impl FnOnce(&mut Editor, anyhow::Result<bool>) + Send + 'static,
    ) {
        self.request_edits(server, Edits::Batch(edits), origin, Box::new(completed));
    }

    fn request_edits(
        &mut self,
        server: LanguageServerId,
        edits: Edits,
        origin: Option<LspActionContext>,
        completed: Completion,
    ) {
        let result = (|| -> anyhow::Result<_> {
            let annotations = edits
                .iter()
                .map(required_annotations)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let client = self
                .language_servers
                .get_by_id(server)
                .context("Workspace edit server has stopped")?;
            ensure!(
                self.config().lsp.enable && client.is_initialized(),
                "Workspace edit server is not active"
            );
            ensure!(
                origin
                    .as_ref()
                    .is_none_or(|origin| origin.is_current(self, server)),
                "Workspace edit is no longer current"
            );
            let targets = if annotations.is_empty() {
                Vec::new()
            } else {
                edits
                    .iter()
                    .map(|edit| Targets::capture(self, edit))
                    .collect::<anyhow::Result<_>>()?
            };
            Ok((
                annotations,
                client.offset_encoding(),
                std::sync::Arc::downgrade(client),
                targets,
            ))
        })();
        let (annotations, encoding, server, targets) = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                completed(self, Err(error));
                return;
            }
        };
        if annotations.is_empty() {
            let result = edits.apply(self, encoding);
            completed(self, result);
            return;
        }
        let operation = Operation {
            edits,
            encoding,
            owner: self.callback_sender().clone(),
            server,
            origin,
            targets,
            completed,
        };
        self.present(|pending| {
            pending.workspace_edits.push_back(ConfirmationRequest {
                annotations,
                operation: Mutex::new(Some(operation)),
            })
        });
    }
}

/// Validate every reference before reporting confirmation, so malformed later edits cannot mutate earlier targets.
pub(super) fn validate(edit: &lsp::WorkspaceEdit) -> Result<(), ApplyEditError> {
    let annotations = references(edit)?;
    if let Some((index, _)) = annotations
        .iter()
        .find(|(_, annotation)| annotation.needs_confirmation == Some(true))
    {
        return Err(ApplyEditError {
            kind: ApplyEditErrorKind::ConfirmationRequired,
            failed_change_idx: *index,
        });
    }
    Ok(())
}

fn required_annotations(
    edit: &lsp::WorkspaceEdit,
) -> Result<Vec<lsp::ChangeAnnotation>, ApplyEditError> {
    Ok(references(edit)?
        .into_iter()
        .filter(|(_, annotation)| annotation.needs_confirmation == Some(true))
        .map(|(_, annotation)| annotation.clone())
        .collect())
}

fn references(
    edit: &lsp::WorkspaceEdit,
) -> Result<Vec<(usize, &lsp::ChangeAnnotation)>, ApplyEditError> {
    fn document<'a>(
        index: usize,
        document: &'a lsp::TextDocumentEdit,
        ids: &mut Vec<(usize, &'a str)>,
    ) {
        for edit in &document.edits {
            match edit {
                lsp::DocumentTextEdit::Annotated(edit) => ids.push((index, &edit.annotation_id)),
                lsp::DocumentTextEdit::Snippet(edit) => {
                    if let Some(id) = &edit.annotation_id {
                        ids.push((index, id));
                    }
                }
                lsp::DocumentTextEdit::Plain(_) => (),
            }
        }
    }
    let mut ids = Vec::new();
    match &edit.document_changes {
        Some(lsp::DocumentChanges::Edits(edits)) => {
            for (index, edit) in edits.iter().enumerate() {
                document(index, edit, &mut ids);
            }
        }
        Some(lsp::DocumentChanges::Operations(operations)) => {
            for (index, operation) in operations.iter().enumerate() {
                match operation {
                    lsp::DocumentChangeOperation::Edit(edit) => document(index, edit, &mut ids),
                    lsp::DocumentChangeOperation::Op(op) => {
                        let annotation = match op {
                            lsp::ResourceOp::Create(op) => &op.annotation_id,
                            lsp::ResourceOp::Rename(op) => &op.annotation_id,
                            lsp::ResourceOp::Delete(op) => &op.annotation_id,
                        };
                        if let Some(id) = annotation {
                            ids.push((index, id));
                        }
                    }
                }
            }
        }
        None => (),
    }
    let mut references = Vec::new();
    let mut seen = HashSet::new();
    for (index, id) in ids {
        let annotation = edit
            .change_annotations
            .as_ref()
            .and_then(|annotations| annotations.get(id))
            .ok_or_else(|| ApplyEditError {
                kind: ApplyEditErrorKind::UnknownAnnotation(id.to_owned()),
                failed_change_idx: index,
            })?;
        if seen.insert(id) {
            references.push((index, annotation));
        }
    }
    Ok(references)
}

struct Targets {
    paths: Vec<PathBuf>,
    disk: Vec<PathState>,
    documents: HashMap<DocumentId, (buffer::Snapshot, Option<editor_core::Uri>, Option<String>)>,
}

impl Targets {
    fn capture(editor: &Editor, edit: &lsp::WorkspaceEdit) -> anyhow::Result<Self> {
        let mut paths = Vec::new();
        let mut add = |url: &lsp::Uri| -> anyhow::Result<()> {
            let uri = url.to_editor_uri()?;
            paths.push(
                uri.as_path()
                    .context("Workspace edit requires local file paths")?
                    .to_owned(),
            );
            Ok(())
        };
        match &edit.document_changes {
            Some(lsp::DocumentChanges::Edits(edits)) => {
                for edit in edits {
                    add(&edit.text_document.uri)?;
                }
            }
            Some(lsp::DocumentChanges::Operations(operations)) => {
                for operation in operations {
                    match operation {
                        lsp::DocumentChangeOperation::Edit(edit) => add(&edit.text_document.uri)?,
                        lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(op)) => {
                            add(&op.uri)?
                        }
                        lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(op)) => {
                            add(&op.uri)?
                        }
                        lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(op)) => {
                            add(&op.old_uri)?;
                            add(&op.new_uri)?;
                        }
                    }
                }
            }
            None => {
                if let Some(changes) = &edit.changes {
                    for url in changes.keys() {
                        add(url)?;
                    }
                }
            }
        }
        paths.sort();
        paths.dedup();
        let disk = paths
            .iter()
            .map(|path| PathState::capture(path))
            .collect::<io::Result<Vec<_>>>()?;
        let documents = editor
            .documents()
            .map(|doc| {
                (
                    doc.id(),
                    (
                        doc.snapshot(),
                        doc.uri(),
                        doc.language_config()
                            .map(|language| language.language_id.clone()),
                    ),
                )
            })
            .collect();
        Ok(Self {
            paths,
            disk,
            documents,
        })
    }

    fn affected(&self, uri: Option<&editor_core::Uri>) -> bool {
        uri.and_then(editor_core::Uri::as_path).is_some_and(|path| {
            self.paths.iter().any(|root| path.starts_with(root)) || {
                let resolved = stdx::path::canonicalize_existing(path);
                self.disk
                    .iter()
                    .any(|state| resolved.starts_with(&state.resolved))
            }
        })
    }

    fn is_current(&self, editor: &Editor) -> bool {
        let matches = |doc: &Document,
                       (snapshot, uri, language): &(
            buffer::Snapshot,
            Option<editor_core::Uri>,
            Option<String>,
        )| {
            snapshot.is_current(&doc.buffer)
                && *uri == doc.uri()
                && language.as_deref()
                    == doc
                        .language_config()
                        .map(|language| language.language_id.as_str())
        };
        self.documents
            .iter()
            .filter(|(_, (_, uri, _))| self.affected(uri.as_ref()))
            .all(|(id, state)| editor.document(*id).is_some_and(|doc| matches(doc, state)))
            && editor
                .documents()
                .filter(|doc| self.affected(doc.uri().as_ref()))
                .all(|doc| {
                    self.documents
                        .get(&doc.id())
                        .is_some_and(|state| matches(doc, state))
                })
            && self.paths.iter().zip(&self.disk).all(|(path, state)| {
                PathState::capture(path).is_ok_and(|current| current == *state)
            })
    }
}

#[derive(PartialEq, Eq)]
struct PathState {
    resolved: PathBuf,
    entries: BTreeMap<PathBuf, Option<FileState>>,
}

#[derive(PartialEq, Eq)]
struct FileState {
    kind: fs::FileType,
    len: u64,
    modified: Option<SystemTime>,
    readonly: bool,
    link: Option<PathBuf>,
}

impl PathState {
    fn capture(path: &Path) -> io::Result<Self> {
        let mut state = Self {
            resolved: stdx::path::canonicalize_existing(path),
            entries: BTreeMap::new(),
        };
        state.visit(path)?;
        // Text-edit URIs may point through a symlink. Capture the actual target
        // as well as the link, without following nested directory symlinks.
        if state.resolved != path {
            state.visit(&state.resolved.clone())?;
        }
        Ok(state)
    }

    fn visit(&mut self, path: &Path) -> io::Result<()> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.entries.insert(path.to_owned(), None);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let directory = metadata.is_dir();
        let link = metadata
            .is_symlink()
            .then(|| fs::read_link(path))
            .transpose()?;
        self.entries.insert(
            path.to_owned(),
            Some(FileState {
                kind: metadata.file_type(),
                len: metadata.len(),
                modified: metadata.modified().ok(),
                readonly: metadata.permissions().readonly(),
                link,
            }),
        );
        if directory {
            for entry in fs::read_dir(path)? {
                self.visit(&entry?.path())?;
            }
        }
        Ok(())
    }
}

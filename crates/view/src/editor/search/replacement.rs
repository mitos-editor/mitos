//! Background replacement planning and direct writes for files without editor buffers.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{ensure, Context as _};
use editor_core::{
    line_ending::{line_end_char_index, normalize_line_endings},
    LineEnding, Rope, Transaction,
};
use event::TaskHandle;
use futures_util::FutureExt;
use stdx::rope::RopeExt as _;

use crate::{
    document::DocumentSavedEventFuture, save::WriteReservation, Document, DocumentId, Editor,
};

struct OpenBuffer {
    id: DocumentId,
    version: i32,
    text: Rope,
    line_ending: LineEnding,
}

struct Target {
    path: PathBuf,
    source_path: PathBuf,
    aliases: Vec<PathBuf>,
    matched: search::Match,
    buffer: Option<OpenBuffer>,
}

/// Capture each target once without opening documents or starting editor services.
pub struct WorkspaceReplacement {
    targets: Vec<Target>,
    search: search::Search,
    replacement: String,
    replace_all: bool,
    config: editor_config::Config,
    loader: Arc<language::syntax::Loader>,
    cancel: TaskHandle,
    owner: crate::callbacks::EditorCallbackSender,
}

struct BufferEdit {
    path: PathBuf,
    id: DocumentId,
    version: i32,
    transaction: Transaction,
    count: usize,
    rows: HashMap<PathBuf, Vec<search::Match>>,
}

struct FileEdit {
    path: PathBuf,
    reservations: Vec<PathBuf>,
    write: Option<DocumentSavedEventFuture>,
    count: usize,
    rows: HashMap<PathBuf, Vec<search::Match>>,
}

pub struct ReplacementPlan {
    buffers: Vec<BufferEdit>,
    files: Vec<FileEdit>,
    cancel: TaskHandle,
    owner: crate::callbacks::EditorCallbackSender,
}

pub struct PendingReplacement {
    files: Vec<FileEdit>,
    outcome: ReplacementOutcome,
    _reservation: Arc<WriteReservation>,
}

pub struct ReplacementOutcome {
    pub count: usize,
    pub rows: HashMap<PathBuf, Vec<search::Match>>,
    /// Writes already completed remain applied when a later write fails.
    pub error: Option<anyhow::Error>,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn new(metadata: std::fs::Metadata) -> anyhow::Result<Self> {
        ensure!(
            metadata.is_file(),
            "Replacement target is not a regular file"
        );
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

impl WorkspaceReplacement {
    pub fn new(
        editor: &Editor,
        search: search::Search,
        matches: &[&search::Match],
        replacement: &str,
        replace_all: bool,
        cancel: TaskHandle,
    ) -> anyhow::Result<Self> {
        let documents = documents_by_file(editor);
        let mut targets: Vec<Target> = Vec::new();
        let mut indices = HashMap::new();
        let mut paths = HashSet::new();
        for matched in matches {
            if !paths.insert(matched.path.clone()) {
                continue;
            }
            let path = stdx::path::canonicalize_existing(&matched.path);
            if let Some(&index) = indices.get(&path) {
                let target: &mut Target = &mut targets[index];
                if !target.aliases.contains(&matched.path) {
                    target.aliases.push(matched.path.clone());
                }
                continue;
            }
            let buffer = documents
                .get(&path)
                .map(|documents| {
                    ensure!(
                        documents.len() == 1,
                        "Multiple buffers are open for {}",
                        path.display()
                    );
                    let doc = documents[0];
                    doc.validate_editable()?;
                    Ok::<_, anyhow::Error>(OpenBuffer {
                        id: doc.id(),
                        version: doc.version(),
                        text: doc.text().clone(),
                        line_ending: doc.line_ending,
                    })
                })
                .transpose()?;
            indices.insert(path.clone(), targets.len());
            targets.push(Target {
                path,
                source_path: stdx::path::canonicalize(&matched.path),
                aliases: vec![matched.path.clone()],
                matched: (*matched).clone(),
                buffer,
            });
            if !replace_all {
                break;
            }
        }
        Ok(Self {
            targets,
            search,
            replacement: replacement.into(),
            replace_all,
            config: (*editor.config()).clone(),
            loader: editor.syn_loader.load_full(),
            cancel,
            owner: editor.callback_sender().clone(),
        })
    }

    /// All file loading, matching, and result rebuilding run off the editor thread.
    pub async fn prepare(self) -> anyhow::Result<ReplacementPlan> {
        tokio::task::spawn_blocking(move || self.plan()).await?
    }

    fn plan(self) -> anyhow::Result<ReplacementPlan> {
        let mut buffers = Vec::new();
        let mut files = Vec::new();
        let config = Arc::new(arc_swap::access::Constant(self.config));
        for target in self.targets {
            ensure!(!self.cancel.is_canceled(), "Replacement canceled");
            let (text, line_ending, file) = if let Some(buffer) = &target.buffer {
                (buffer.text.clone(), buffer.line_ending, None)
            } else {
                let stamp = FileStamp::new(
                    std::fs::metadata(&target.path)
                        .with_context(|| format!("failed to read '{}'", target.path.display()))?,
                )?;
                let doc = Document::open(
                    &target.source_path,
                    None,
                    false,
                    config.clone(),
                    self.loader.clone(),
                )
                .with_context(|| format!("failed to read '{}'", target.path.display()))?;
                ensure!(
                    !doc.is_readonly(),
                    "File is read-only: {}",
                    target.path.display()
                );
                ensure!(
                    stdx::path::canonicalize_existing(&target.source_path) == target.path
                        && stamp == FileStamp::new(std::fs::metadata(&target.path)?)?,
                    "File changed while reading: {}",
                    target.path.display()
                );
                (doc.text().clone(), doc.line_ending, Some((doc, stamp)))
            };
            let range = if self.replace_all {
                None
            } else {
                let matched = &target.matched;
                let slice = text.slice(..);
                ensure!(
                    matched.line_start <= matched.line_end
                        && matched.line_end < slice.len_lines(stdx::rope::LINE_TYPE),
                    "the selected match no longer exists"
                );
                let start = slice
                    .line_to_char_idx(matched.line_start)
                    .checked_add(matched.match_start_col);
                let end = slice
                    .line_to_char_idx(matched.line_end)
                    .checked_add(matched.match_end_col);
                let (Some(start), Some(end)) = (start, end) else {
                    anyhow::bail!("the selected match no longer exists");
                };
                ensure!(
                    start <= end
                        && start <= line_end_char_index(&slice, matched.line_start)
                        && end <= line_end_char_index(&slice, matched.line_end),
                    "the selected match no longer exists"
                );
                Some(start..end)
            };
            let mut changes =
                self.search
                    .replacement_changes(text.slice(..), &self.replacement, range)?;
            for (_, _, inserted) in &mut changes {
                if let Some(text) = inserted {
                    *text = normalize_line_endings(text, line_ending).as_ref().into();
                }
            }
            ensure!(
                self.replace_all || !changes.is_empty(),
                "the selected text no longer matches the search pattern"
            );
            let count = changes.len();
            let transaction = Transaction::change(&text, changes.into_iter());
            let mut updated = text;
            transaction.apply(&mut updated);
            let matches = self.search.search_document(&target.path, &updated)?;
            let rows = target
                .aliases
                .into_iter()
                .map(|alias| {
                    let matches = matches
                        .iter()
                        .cloned()
                        .map(|mut matched| {
                            matched.path = alias.clone();
                            matched
                        })
                        .collect();
                    (alias, matches)
                })
                .collect();
            if let Some(buffer) = target.buffer {
                buffers.push(BufferEdit {
                    path: target.path,
                    id: buffer.id,
                    version: buffer.version,
                    transaction,
                    count,
                    rows,
                });
            } else {
                let (mut doc, stamp) = file.unwrap();
                let write = if count == 0 {
                    None
                } else {
                    // This document is never registered with the editor. Only its
                    // buffer and normal encoding/save machinery are needed here.
                    doc.buffer.apply(transaction.changes())?;
                    // Keep settings from the searched alias, but write the resolved
                    // file reserved by the plan even if an alias changes later.
                    doc.set_path(Some(&target.path));
                    let save = doc.save(None::<PathBuf>, false)?;
                    let path = target.path.clone();
                    Some(
                        async move {
                            ensure!(
                                stamp == FileStamp::new(tokio::fs::metadata(&path).await?)?,
                                "File changed before replacement: {}",
                                path.display()
                            );
                            save.await
                        }
                        .boxed(),
                    )
                };
                let reservations = if write.is_some() {
                    rows.keys()
                        .map(stdx::path::canonicalize)
                        .chain(std::iter::once(target.path.clone()))
                        .collect()
                } else {
                    Vec::new()
                };
                files.push(FileEdit {
                    reservations,
                    path: target.path,
                    write,
                    count,
                    rows,
                });
            }
        }
        Ok(ReplacementPlan {
            buffers,
            files,
            cancel: self.cancel,
            owner: self.owner,
        })
    }
}

fn documents_by_file(editor: &Editor) -> HashMap<PathBuf, Vec<&Document>> {
    let mut documents: HashMap<_, Vec<_>> = HashMap::new();
    for doc in editor.documents() {
        if let Some(path) = doc.path() {
            documents
                .entry(stdx::path::canonicalize_existing(path))
                .or_default()
                .push(doc);
        }
    }
    documents
}

impl ReplacementPlan {
    /// Validate every buffer before editing, then reserve disk paths until writes finish.
    pub fn apply(self, editor: &mut Editor) -> anyhow::Result<PendingReplacement> {
        ensure!(
            !self.cancel.is_canceled() && self.owner.same_destination(editor.callback_sender()),
            "Replacement canceled or editor changed"
        );
        let documents = documents_by_file(editor);
        for edit in &self.buffers {
            ensure!(
                documents
                    .get(&edit.path)
                    .is_some_and(|docs| docs.len() == 1 && docs[0].id() == edit.id),
                "Replacement buffer was closed or moved: {}",
                edit.path.display()
            );
            let doc = editor.document(edit.id).unwrap();
            ensure!(
                doc.version() == edit.version,
                "Replacement buffer changed: {}",
                edit.path.display()
            );
            doc.validate_changes(edit.transaction.changes())?;
        }
        for (path, rows) in self
            .buffers
            .iter()
            .map(|edit| (&edit.path, &edit.rows))
            .chain(self.files.iter().map(|edit| (&edit.path, &edit.rows)))
        {
            ensure!(
                rows.keys()
                    .all(|alias| stdx::path::canonicalize_existing(alias) == *path),
                "Replacement path changed: {}",
                path.display()
            );
        }
        for edit in &self.files {
            ensure!(
                !editor.writes.is_blocked(&edit.path)
                    && !edit
                        .reservations
                        .iter()
                        .any(|path| editor.writes.is_blocked(path)),
                "A filesystem operation is pending for {}",
                edit.path.display()
            );
            ensure!(
                !documents.contains_key(&edit.path)
                    && !edit
                        .reservations
                        .iter()
                        .any(|path| documents.contains_key(path)),
                "Replacement file was opened while planning: {}",
                edit.path.display()
            );
            ensure!(
                !editor.writes.conflicts_with(&edit.path)
                    && !edit
                        .reservations
                        .iter()
                        .any(|path| editor.writes.conflicts_with(path)),
                "A save is pending for {}",
                edit.path.display()
            );
        }
        let reservation = editor.writes.reserve_paths(
            self.files
                .iter()
                .filter(|edit| edit.write.is_some())
                .flat_map(|edit| edit.reservations.iter().cloned())
                .collect(),
        );
        let mut outcome = ReplacementOutcome {
            count: 0,
            rows: HashMap::new(),
            error: None,
        };
        for edit in self.buffers {
            editor.apply_document_changes(edit.id, edit.version, edit.transaction.changes())?;
            outcome.count += edit.count;
            outcome.rows.extend(edit.rows);
        }
        Ok(PendingReplacement {
            files: self.files,
            outcome,
            _reservation: reservation,
        })
    }
}

impl PendingReplacement {
    /// Accepted writes finish even if their picker closes. The frontend retains this task on exit.
    pub async fn finish(mut self) -> ReplacementOutcome {
        for file in self.files {
            if let Some(write) = file.write
                && let Err(error) = write.await
            {
                self.outcome.error =
                    Some(error.context(format!("failed to write '{}'", file.path.display())));
                break;
            }
            self.outcome.count += file.count;
            self.outcome.rows.extend(file.rows);
        }
        self.outcome
    }
}

#[cfg(test)]
#[path = "replacement_tests.rs"]
mod tests;

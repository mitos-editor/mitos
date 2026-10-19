//! Shared save preparation, execution, and policy for commands and automatic saves.
//!
//! Owns normalization, code actions, formatting, and write submission. Frontends
//! drive [`SaveTask`]s and apply their typed updates on the editor mutation path,
//! retaining every stage until completion before exiting.

mod operation;
mod queue;

pub use operation::{submit, SaveError, SaveProgress, SaveTask, SaveUpdate};
pub use queue::FileWriteBlock;
pub(crate) use queue::WriteQueue;

use std::path::PathBuf;

use crate::{Document, DocumentId, Editor, ViewId};
use editor_config::Config;
use editor_core::{line_ending, Selection, Transaction};
use stdx::rope::RopeSliceExt;

/// Options for an explicit single-document save.
#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    pub force: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

/// Options for saving modified documents.
#[derive(Debug, Clone, Copy)]
pub struct WriteAllOptions {
    pub force: bool,
    /// Report modified scratch buffers as errors; they are always skipped.
    pub write_scratch: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

/// A document prepared for saving, with effective pre-save policy.
///
/// Formatting is requested here; formatter availability and language overrides
/// are resolved against the latest document after code actions have completed.
#[derive(Debug)]
pub struct PreparedSave {
    pub doc_id: DocumentId,
    /// Originating split, when the document is displayed.
    pub origin: Option<ViewId>,
    pub path: Option<PathBuf>,
    pub force: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

/// Normalize a document and checkpoint its edits before submitting a save.
/// The document must exist. An optional origin supplies interactive selection
/// context; hidden documents can be prepared without any view.
pub fn prepare(
    editor: &mut Editor,
    doc_id: DocumentId,
    origin: Option<ViewId>,
    path: Option<PathBuf>,
    options: WriteOptions,
) -> PreparedSave {
    let config = editor.config();
    prepare_with_config(editor, doc_id, origin, path, options, &config)
}

fn prepare_with_config(
    editor: &mut Editor,
    doc_id: DocumentId,
    origin: Option<ViewId>,
    path: Option<PathBuf>,
    options: WriteOptions,
    config: &Config,
) -> PreparedSave {
    editor.sync_document_views(doc_id);
    let doc = doc_mut!(editor, &doc_id);

    if doc.trim_trailing_whitespace() {
        trim_trailing_whitespace(doc, origin);
    }
    if config.files.save.trim_final_newlines {
        trim_final_newlines(doc, origin);
    }
    if doc.insert_final_newline() {
        insert_final_newline(doc, origin);
    }
    doc.commit_changes();

    let code_actions = options.code_actions
        && doc
            .language_config()
            .and_then(|config| config.code_actions_on_save.as_deref())
            .is_some_and(|kinds| !kinds.is_empty());

    let request = PreparedSave {
        doc_id,
        origin,
        path,
        force: options.force,
        auto_format: config.format.on_save && options.auto_format,
        code_actions,
    };
    editor.sync_document_views(doc_id);
    request
}

/// Prepare and submit each modified file-backed document in editor order.
///
/// Preparation happens immediately before submission. An immediate submission
/// error stops the batch, leaving later documents unprepared. Scratch-buffer
/// errors are reported after the eligible documents have been submitted, unless
/// forced. Asynchronous completion and failures belong to the submitter.
pub fn save_all(
    editor: &mut Editor,
    options: WriteAllOptions,
    mut submit: impl FnMut(&mut Editor, PreparedSave) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut errors: Vec<&'static str> = Vec::new();
    let config = editor.config();
    let saves: Vec<_> = editor
        .documents
        .iter()
        .filter_map(|(&id, doc)| {
            if !doc.is_modified() {
                return None;
            }
            if doc.path().is_none() {
                if options.write_scratch {
                    errors.push("cannot write a buffer without a filename");
                }
                return None;
            }
            let target_view = editor
                .tree
                .views()
                .find_map(|(view, _)| (view.doc == id).then_some(view.id));
            Some((id, target_view))
        })
        .collect();

    for (doc_id, origin) in saves {
        let request = prepare_with_config(
            editor,
            doc_id,
            origin,
            None,
            WriteOptions {
                force: options.force,
                auto_format: options.auto_format,
                code_actions: options.code_actions,
            },
            &config,
        );
        submit(editor, request)?;
    }

    if !errors.is_empty() && !options.force {
        anyhow::bail!("{:?}", errors);
    }
    Ok(())
}

/// Save modified files without running formatters or code actions, ignoring scratch buffers.
/// Debouncing, focus, and mode restrictions belong to `features::auto_save`.
pub fn auto_save(editor: &mut Editor) -> anyhow::Result<()> {
    save_all(
        editor,
        WriteAllOptions {
            force: false,
            write_scratch: false,
            auto_format: false,
            code_actions: false,
        },
        |editor, request| editor.save(request.doc_id, request.path, request.force),
    )
}

fn apply_normalization(doc: &mut Document, transaction: &Transaction, origin: Option<ViewId>) {
    if let Some(origin) = origin {
        crate::editing::apply(doc, transaction, origin);
    } else {
        // Normalization is built synchronously from this document's current text.
        doc.apply(transaction.changes())
            .expect("normalization uses current text");
    }
}

/// Trim all whitespace preceding line-endings in a document.
fn trim_trailing_whitespace(doc: &mut Document, origin: Option<ViewId>) {
    let text = doc.text();
    let mut pos = 0;
    let transaction = Transaction::delete(
        text,
        text.lines().filter_map(|line| {
            let line_end_len_chars = line_ending::get_line_ending(&line)
                .map(|le| le.len_chars())
                .unwrap_or_default();
            // Char after the last non-whitespace character or the beginning of the line if the
            // line is all whitespace:
            let first_trailing_whitespace =
                pos + line.last_non_whitespace_char().map_or(0, |idx| idx + 1);
            pos += line.len_chars();
            // Char before the line ending character(s), or the final char in the text if there
            // is no line-ending on this line:
            let line_end = pos - line_end_len_chars;
            if first_trailing_whitespace != line_end {
                Some((first_trailing_whitespace, line_end))
            } else {
                None
            }
        }),
    );
    apply_normalization(doc, &transaction, origin);
}

/// Trim any extra line-endings after the final line-ending.
fn trim_final_newlines(doc: &mut Document, origin: Option<ViewId>) {
    let rope = doc.text();
    let mut text = rope.slice(..);
    let mut total_char_len = 0;
    let mut final_char_len = 0;
    while let Some(line_ending) = line_ending::get_line_ending(&text) {
        total_char_len += line_ending.len_chars();
        final_char_len = line_ending.len_chars();
        text = text.slice(..text.len_chars() - line_ending.len_chars());
    }
    let chars_to_delete = total_char_len - final_char_len;
    if chars_to_delete != 0 {
        let transaction = Transaction::delete(
            rope,
            [(rope.len_chars() - chars_to_delete, rope.len_chars())].into_iter(),
        );
        apply_normalization(doc, &transaction, origin);
    }
}

/// Ensure that the document is terminated with a line ending.
fn insert_final_newline(doc: &mut Document, origin: Option<ViewId>) {
    let text = doc.text();
    if text.len_chars() > 0 && line_ending::get_line_ending(&text.slice(..)).is_none() {
        let eof = Selection::point(text.len_chars());
        let insert = Transaction::insert(text, &eof, doc.line_ending.as_str().into());
        apply_normalization(doc, &insert, origin);
    }
}

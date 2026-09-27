//! Shared save preparation and policy for commands and automatic saves.
//!
//! Frontends submit prepared requests, scheduling code actions and formatting when
//! requested, then call [`Editor::save`] to enqueue the write. This module owns
//! document normalization, history checkpoints, target selection, and batch policy.

use std::path::PathBuf;

use editor_core::{line_ending, Selection, Transaction};
use stdx::rope::RopeSliceExt;

use crate::{config::Config, Document, DocumentId, Editor, ViewId};

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
    pub view_id: ViewId,
    pub path: Option<PathBuf>,
    pub force: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

/// Normalize a document and checkpoint its edits before submitting a save.
/// The document and view must exist, with a selection initialized for that view.
pub fn prepare(
    editor: &mut Editor,
    doc_id: DocumentId,
    view_id: ViewId,
    path: Option<PathBuf>,
    options: WriteOptions,
) -> PreparedSave {
    let config = editor.config();
    prepare_with_config(editor, doc_id, view_id, path, options, &config)
}

fn prepare_with_config(
    editor: &mut Editor,
    doc_id: DocumentId,
    view_id: ViewId,
    path: Option<PathBuf>,
    options: WriteOptions,
    config: &Config,
) -> PreparedSave {
    let doc = doc_mut!(editor, &doc_id);
    let view = view_mut!(editor, view_id);

    if doc.trim_trailing_whitespace() {
        trim_trailing_whitespace(doc, view_id);
    }
    if config.trim_final_newlines {
        trim_final_newlines(doc, view_id);
    }
    if doc.insert_final_newline() {
        insert_final_newline(doc, view_id);
    }
    doc.append_changes_to_history(view);

    let code_actions = options.code_actions
        && doc
            .language_config()
            .and_then(|config| config.code_actions_on_save.as_deref())
            .is_some_and(|kinds| !kinds.is_empty());

    PreparedSave {
        doc_id,
        view_id,
        path,
        force: options.force,
        auto_format: config.auto_format && options.auto_format,
        code_actions,
    }
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
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|id| {
            let doc = doc!(editor, &id);
            if !doc.is_modified() {
                return None;
            }
            if doc.path().is_none() {
                if options.write_scratch {
                    errors.push("cannot write a buffer without a filename");
                }
                return None;
            }
            let target_view = editor.get_synced_view_id(doc.id());
            Some((id, target_view))
        })
        .collect();

    for (doc_id, view_id) in saves {
        let request = prepare_with_config(
            editor,
            doc_id,
            view_id,
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
/// Debouncing, focus, and mode restrictions belong to `handlers::auto_save`.
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

/// Trim all whitespace preceding line-endings in a document.
fn trim_trailing_whitespace(doc: &mut Document, view_id: ViewId) {
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
    doc.apply(&transaction, view_id);
}

/// Trim any extra line-endings after the final line-ending.
fn trim_final_newlines(doc: &mut Document, view_id: ViewId) {
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
        doc.apply(&transaction, view_id);
    }
}

/// Ensure that the document is terminated with a line ending.
fn insert_final_newline(doc: &mut Document, view_id: ViewId) {
    let text = doc.text();
    if text.len_chars() > 0 && line_ending::get_line_ending(&text.slice(..)).is_none() {
        let eof = Selection::point(text.len_chars());
        let insert = Transaction::insert(text, &eof, doc.line_ending.as_str().into());
        doc.apply(&insert, view_id);
    }
}

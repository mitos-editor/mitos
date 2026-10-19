//! Shared save preparation, execution, and policy for commands and automatic saves.
//!
//! Owns normalization, code actions, formatting, and write submission. Frontends
//! drive [`SaveTask`]s and apply their typed updates on the editor mutation path,
//! retaining every stage until completion before exiting.

use stdx::rope::RopeExt as _;
mod operation;
mod queue;

pub use operation::{submit, SaveError, SaveProgress, SaveTask, SaveUpdate};
pub use queue::FileWriteBlock;
pub(crate) use queue::WriteQueue;
pub(crate) use queue::WriteReservation;

use std::path::PathBuf;

use crate::editing::EditError;
use crate::{Document, DocumentId, Editor, ViewId};
use editor_config::Config;
use editor_core::{line_ending, LineEnding, Rope, Transaction};
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
) -> Result<PreparedSave, EditError> {
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
) -> Result<PreparedSave, EditError> {
    editor.sync_document_views(doc_id);
    let doc = doc_mut!(editor, &doc_id);

    // Build normalization against a working rope and publish one transaction. Rejection
    // must not leave a partly normalized document or commit pending typing.
    let mut text = doc.text().clone();
    let mut normalization = Transaction::new(&text);
    if doc.trim_trailing_whitespace() {
        let transaction = trim_trailing_whitespace(&text);
        transaction.apply(&mut text);
        normalization = normalization.compose(transaction);
    }
    if config.files.save.trim_final_newlines {
        let transaction = trim_final_newlines(&text);
        transaction.apply(&mut text);
        normalization = normalization.compose(transaction);
    }
    if doc.insert_final_newline() {
        normalization = normalization.compose(insert_final_newline(&text, doc.line_ending));
    }
    apply_normalization(doc, &normalization, origin)?;
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
    Ok(request)
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
        )?;
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

fn apply_normalization(
    doc: &mut Document,
    transaction: &Transaction,
    origin: Option<ViewId>,
) -> Result<(), EditError> {
    match origin {
        Some(origin) => crate::editing::apply(doc, transaction, origin),
        None => doc.apply(transaction.changes()),
    }
}

/// Trim all whitespace preceding line-endings in a document.
fn trim_trailing_whitespace(text: &Rope) -> Transaction {
    let mut pos = 0;
    Transaction::delete(
        text,
        text.lines(stdx::rope::LINE_TYPE).filter_map(|line| {
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
    )
}

/// Trim any extra line-endings after the final line-ending.
fn trim_final_newlines(rope: &Rope) -> Transaction {
    let mut text = rope.slice(..);
    let mut total_char_len = 0;
    let mut final_char_len = 0;
    while let Some(line_ending) = line_ending::get_line_ending(&text) {
        total_char_len += line_ending.len_chars();
        final_char_len = line_ending.len_chars();
        text = text.slice_chars(..text.len_chars() - line_ending.len_chars());
    }
    let chars_to_delete = total_char_len - final_char_len;
    Transaction::delete(
        rope,
        [(rope.len_chars() - chars_to_delete, rope.len_chars())].into_iter(),
    )
}

/// Ensure that the document is terminated with a line ending.
fn insert_final_newline(text: &Rope, ending: LineEnding) -> Transaction {
    if text.len_chars() > 0 && line_ending::get_line_ending(&text.slice(..)).is_none() {
        let end = text.len_chars();
        Transaction::change(text, [(end, end, Some(ending.as_str().into()))].into_iter())
    } else {
        Transaction::new(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editing,
        editor::{tests::editor, Action},
    };

    #[tokio::test]
    async fn rejected_normalization_preserves_document_with_or_without_an_origin() {
        for visible in [false, true] {
            let (mut editor, _store) = editor("#[|x]#\n");
            let mut config = editor_config::Config::default();
            config.files.save.trim_trailing_whitespace = true;
            let doc = Document::from(
                Rope::from("x  \n"),
                None,
                std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(config)),
                editor.syn_loader.load_full(),
            )
            .with_readonly_name("adapter source".into());
            let id = editor.new_file_from_document(Action::Replace, doc);
            let view = editor.tree.focus();
            let before = editor.document(id).unwrap().text().clone();
            let result = prepare(
                &mut editor,
                id,
                visible.then_some(view),
                None,
                WriteOptions {
                    force: false,
                    auto_format: false,
                    code_actions: false,
                },
            );
            assert!(matches!(result, Err(EditError::ReadOnlyDocument)));
            assert_eq!(editor.document(id).unwrap().text(), &before);
            let (view, doc) = current!(editor);
            assert!(!editing::undo(doc, view.id));
        }
    }
}

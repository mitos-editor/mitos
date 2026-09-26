use std::fmt::Write;
use std::io::BufReader;

use crate::job::{Job, Jobs};

pub use super::catalog::{
    CommandCompleter, TypableCommand, SHELL_COMPLETER, SHELL_SIGNATURE, TYPABLE_COMMAND_LIST,
    TYPABLE_COMMAND_MAP,
};

pub use super::command_line::complete_command_args;

use super::catalog::{WRITE_NO_CODE_ACTIONS_FLAG, WRITE_NO_FORMAT_FLAG};
use super::registers::{
    paste, replace_selections_with_register, yank_joined_impl, yank_main_selection_to_register,
    Paste,
};
use super::shell::{shell, shell_impl_async, ShellBehavior};
use super::*;

use crate::config::{ConfigEvent, EditorSettings};
use ::command_line::{self, Args};
use anyhow::ensure;
use editor_core::history::UndoKind;
use editor_core::indent::{self, MAX_INDENT};
use editor_core::line_ending;
use serde_json::Value;
use stdx::path::home_dir;
use view::document::{read_to_string, DEFAULT_LANGUAGE_NAME};
use view::editor::CloseError;

#[cold]
pub(super) fn exit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if doc!(cx.editor).is_modified() {
        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: false,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
    }
    cx.block_try_flush_writes()?;
    quit(cx, Args::default(), event)
}

#[cold]
pub(super) fn force_exit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if doc!(cx.editor).is_modified() {
        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
    }
    cx.block_try_flush_writes()?;
    quit(cx, Args::default(), event)
}

#[cold]
pub(super) fn quit(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    log::debug!("quitting...");

    if event != PromptEvent::Validate {
        return Ok(());
    }

    // last view and we have unsaved changes
    if cx.editor.tree.views().count() == 1 {
        buffers_remaining_impl(cx.editor)?
    }

    cx.block_try_flush_writes()?;
    cx.editor.close(view!(cx.editor).id);

    Ok(())
}

#[cold]
pub(super) fn force_quit(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.block_try_flush_writes()?;
    cx.editor.close(view!(cx.editor).id);

    Ok(())
}

#[cold]
pub(super) fn open(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    open_impl(cx, args, Action::Replace)
}

fn open_impl(cx: &mut compositor::Context, args: Args, action: Action) -> anyhow::Result<()> {
    for arg in args {
        let (path, pos) = crate::args::parse_file(&arg);
        let path = stdx::path::expand_tilde(path);
        // If the path is a directory, open a file picker on that directory and update the status
        // message
        if let Ok(true) = std::fs::canonicalize(&path).map(|p| p.is_dir()) {
            let callback = async move {
                let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        let picker =
                            ui::file_picker(editor, path.into_owned()).with_default_action(action);
                        compositor.push(Box::new(overlaid(picker)));
                    },
                ));
                Ok(call)
            };
            cx.jobs.callback(callback);
        } else {
            // Otherwise, just open the file
            let _ = cx.editor.open(&path, action)?;
            let (view, doc) = current!(cx.editor);
            let pos = Selection::point(pos_at_coords(doc.text().slice(..), pos, true));
            doc.set_selection(view.id, pos);
            // does not affect opening a buffer without pos
            align_view(doc, view, Align::Center);
        }
    }
    Ok(())
}

fn buffer_close_by_ids_impl(
    cx: &mut compositor::Context,
    doc_ids: &[DocumentId],
    force: bool,
) -> anyhow::Result<()> {
    cx.block_try_flush_writes()?;

    let (modified_ids, modified_names): (Vec<_>, Vec<_>) = doc_ids
        .iter()
        .filter_map(|&doc_id| match cx.editor.close_document(doc_id, force) {
            Err(CloseError::BufferModified(name)) => Some((doc_id, name)),
            _ => None,
        })
        .unzip();

    if let Some(first) = modified_ids.first() {
        let current = doc!(cx.editor);
        // If the current document is unmodified, and there are modified
        // documents, switch focus to the first modified doc.
        if !modified_ids.contains(&current.id()) {
            cx.editor.switch(*first, Action::Replace);
        }
        bail!(
            "{} unsaved buffer{} remaining: {:?}",
            modified_names.len(),
            if modified_names.len() == 1 { "" } else { "s" },
            modified_names,
        );
    }

    Ok(())
}

fn buffer_gather_paths_impl(editor: &mut Editor, args: Args) -> Vec<DocumentId> {
    // No arguments implies current document
    if args.is_empty() {
        let doc_id = view!(editor).doc;
        return vec![doc_id];
    }

    let mut nonexistent_buffers = vec![];
    let mut document_ids = vec![];
    for arg in args {
        let doc_id = editor.documents().find_map(|doc| {
            let arg_path = Some(Path::new(arg.as_ref()));
            if doc.path() == arg_path
                || doc.relative_path() == arg_path
                || doc.display_path() == arg_path
            {
                Some(doc.id())
            } else {
                None
            }
        });

        match doc_id {
            Some(doc_id) => document_ids.push(doc_id),
            None => nonexistent_buffers.push(format!("'{}'", arg)),
        }
    }

    if !nonexistent_buffers.is_empty() {
        editor.set_error(|| {
            format!(
                "cannot close non-existent buffers: {}",
                nonexistent_buffers.join(", ")
            )
        });
    }

    document_ids
}

#[cold]
pub(super) fn buffer_close(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_paths_impl(cx.editor, args);
    buffer_close_by_ids_impl(cx, &document_ids, false)
}

#[cold]
pub(super) fn force_buffer_close(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_paths_impl(cx.editor, args);
    buffer_close_by_ids_impl(cx, &document_ids, true)
}

fn buffer_gather_others_impl(editor: &mut Editor, skip_visible: bool) -> Vec<DocumentId> {
    if skip_visible {
        let visible_document_ids = editor
            .tree
            .views()
            .map(|view| &view.0.doc)
            .collect::<HashSet<_>>();
        editor
            .documents()
            .map(|doc| doc.id())
            .filter(|doc_id| !visible_document_ids.contains(doc_id))
            .collect()
    } else {
        let current_document = &doc!(editor).id();
        editor
            .documents()
            .map(|doc| doc.id())
            .filter(|doc_id| doc_id != current_document)
            .collect()
    }
}

#[cold]
pub(super) fn buffer_close_others(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_others_impl(cx.editor, args.has_flag("skip-visible"));
    buffer_close_by_ids_impl(cx, &document_ids, false)
}

#[cold]
pub(super) fn force_buffer_close_others(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_others_impl(cx.editor, args.has_flag("skip-visible"));
    buffer_close_by_ids_impl(cx, &document_ids, true)
}

fn buffer_gather_all_impl(editor: &mut Editor) -> Vec<DocumentId> {
    editor.documents().map(|doc| doc.id()).collect()
}

#[cold]
pub(super) fn buffer_close_all(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_all_impl(cx.editor);
    buffer_close_by_ids_impl(cx, &document_ids, false)
}

#[cold]
pub(super) fn force_buffer_close_all(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let document_ids = buffer_gather_all_impl(cx.editor);
    buffer_close_by_ids_impl(cx, &document_ids, true)
}

#[cold]
pub(super) fn buffer_next(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    goto_buffer(cx.editor, Direction::Forward, 1);
    Ok(())
}

#[cold]
pub(super) fn buffer_previous(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    goto_buffer(cx.editor, Direction::Backward, 1);
    Ok(())
}

fn write_impl(
    cx: &mut compositor::Context,
    path: Option<&str>,
    options: WriteOptions,
) -> anyhow::Result<()> {
    let config = cx.editor.config();
    let (view, doc) = current!(cx.editor);
    let doc_id = doc.id();
    let view_id = view.id;

    if doc.trim_trailing_whitespace() {
        trim_trailing_whitespace(doc, view_id);
    }
    if config.trim_final_newlines {
        trim_final_newlines(doc, view_id);
    }
    if doc.insert_final_newline() {
        insert_final_newline(doc, view_id);
    }

    // Save an undo checkpoint for any outstanding changes.
    doc.append_changes_to_history(view);

    let auto_format = config.auto_format && options.auto_format;
    let force = options.force;
    let path: Option<PathBuf> = path.map(Into::into);

    // Does the document configure any code actions to run on save?
    let run_code_actions = options.code_actions
        && doc!(cx.editor, &doc_id)
            .language_config()
            .and_then(|c| c.code_actions_on_save.as_deref())
            .is_some_and(|kinds| !kinds.is_empty());

    // The tail of the on-save chain: re-build the auto-format job against the
    // latest document (so it formats after any code-action edits), or save
    // directly when there's no formatter. Deferred via `Followup`, and always
    // saves, so code-actions-on-save works even with auto-format off. Only
    // built when there is pre-save work. A plain `:w` saves synchronously below.
    let tail = (auto_format || run_code_actions).then(|| {
        let path = path.clone();
        let callback = Callback::Followup(Box::new(move |editor| {
            // The document could have been closed mid-chain
            if !editor.documents.contains_key(&doc_id) {
                return None;
            }
            let doc = doc!(editor, &doc_id);
            let fmt_job = auto_format
                .then(|| doc.auto_format(editor))
                .flatten()
                .map(|fmt| {
                    let call = make_format_callback(
                        doc_id,
                        doc.version(),
                        view_id,
                        fmt,
                        Some((path.clone(), force)),
                    );
                    Job::with_callback(call).wait_before_exiting()
                });
            if fmt_job.is_none()
                && let Err(err) = editor.save(doc_id, path, force)
            {
                editor.set_error(|| format!("Error saving: {}", err));
            }
            fmt_job
        }));
        Job::with_callback(async { Ok(callback) }).wait_before_exiting()
    });

    let job = if run_code_actions {
        code_actions_on_save(cx.editor, doc_id, tail)
    } else {
        tail
    };

    if let Some(job) = job {
        cx.jobs.add(job);
    } else {
        cx.editor.save(doc_id, path, force)?;
    }

    Ok(())
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

#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    pub force: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

#[cold]
pub(super) fn write(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: false,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )
}

#[cold]
pub(super) fn force_write(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )
}

#[cold]
pub(super) fn write_buffer_close(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: false,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )?;

    let document_ids = buffer_gather_paths_impl(cx.editor, args);
    buffer_close_by_ids_impl(cx, &document_ids, false)
}

#[cold]
pub(super) fn force_write_buffer_close(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )?;

    let document_ids = buffer_gather_paths_impl(cx.editor, args);
    buffer_close_by_ids_impl(cx, &document_ids, false)
}

#[cold]
pub(super) fn new_file(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor.new_file(Action::Replace);

    Ok(())
}

#[cold]
pub(super) fn format(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current_ref!(cx.editor);
    let format = doc.format(cx.editor).context(
        "A formatter isn't available, and no language server provides formatting capabilities",
    )?;
    let callback = make_format_callback(doc.id(), doc.version(), view.id, format, None);
    cx.jobs.callback(callback);

    Ok(())
}

#[cold]
pub(super) fn set_indent_style(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    use IndentStyle::*;

    // If no argument, report current indent style.
    if args.is_empty() {
        let style = doc!(cx.editor).indent_style;
        cx.editor.set_status(match style {
            Tabs => "tabs".to_owned(),
            Spaces(1) => "1 space".to_owned(),
            Spaces(n) => format!("{} spaces", n),
        });
        return Ok(());
    }

    // Attempt to parse argument as an indent style.
    let style = match args.first() {
        Some(arg) if "tabs".starts_with(&arg.to_lowercase()) => Some(Tabs),
        Some("0") => Some(Tabs),
        Some(arg) => arg
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=MAX_INDENT).contains(n))
            .map(Spaces),
        _ => None,
    };

    let style = style.context("invalid indent style")?;
    let doc = doc_mut!(cx.editor);
    doc.indent_style = style;

    Ok(())
}

/// Sets or reports the current document's line ending setting.
#[cold]
pub(super) fn set_line_ending(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    use LineEnding::*;

    // If no argument, report current line ending setting.
    if args.is_empty() {
        let line_ending = doc!(cx.editor).line_ending;
        cx.editor.set_status(match line_ending {
            Crlf => "crlf",
            LF => "line feed",
            #[cfg(feature = "unicode-lines")]
            FF => "form feed",
            #[cfg(feature = "unicode-lines")]
            CR => "carriage return",
            #[cfg(feature = "unicode-lines")]
            Nel => "next line",

            // These should never be a document's default line ending.
            #[cfg(feature = "unicode-lines")]
            VT | LS | PS => "error",
        });

        return Ok(());
    }

    let arg = args
        .first()
        .context("argument missing")?
        .to_ascii_lowercase();

    // Attempt to parse argument as a line ending.
    let line_ending = match arg {
        arg if arg.starts_with("crlf") => Crlf,
        arg if arg.starts_with("lf") => LF,
        #[cfg(feature = "unicode-lines")]
        arg if arg.starts_with("cr") => CR,
        #[cfg(feature = "unicode-lines")]
        arg if arg.starts_with("ff") => FF,
        #[cfg(feature = "unicode-lines")]
        arg if arg.starts_with("nel") => Nel,
        _ => bail!("invalid line ending"),
    };
    let (view, doc) = current!(cx.editor);
    doc.line_ending = line_ending;

    let mut pos = 0;
    let transaction = Transaction::change(
        doc.text(),
        doc.text().lines().filter_map(|line| {
            pos += line.len_chars();
            match editor_core::line_ending::get_line_ending(&line) {
                Some(ending) if ending != line_ending => {
                    let start = pos - ending.len_chars();
                    let end = pos;
                    Some((start, end, Some(line_ending.as_str().into())))
                }
                _ => None,
            }
        }),
    );
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);

    Ok(())
}

#[cold]
pub(super) fn earlier(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let uk = args.join(" ").parse::<UndoKind>().map_err(|s| anyhow!(s))?;

    let (view, doc) = current!(cx.editor);
    let success = doc.earlier(view, uk);
    if !success {
        cx.editor.set_status("Already at oldest change");
    }

    Ok(())
}

#[cold]
pub(super) fn later(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let uk = args.join(" ").parse::<UndoKind>().map_err(|s| anyhow!(s))?;
    let (view, doc) = current!(cx.editor);
    let success = doc.later(view, uk);
    if !success {
        cx.editor.set_status("Already at newest change");
    }

    Ok(())
}

#[cold]
pub(super) fn write_quit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: false,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )?;
    cx.block_try_flush_writes()?;
    quit(cx, Args::default(), event)
}

#[cold]
pub(super) fn force_write_quit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_impl(
        cx,
        args.first(),
        WriteOptions {
            force: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )?;
    cx.block_try_flush_writes()?;
    force_quit(cx, Args::default(), event)
}

/// Results in an error if there are modified buffers remaining and sets editor
/// error, otherwise returns `Ok(())`. If the current document is unmodified,
/// and there are modified documents, switches focus to one of them.
pub(super) fn buffers_remaining_impl(editor: &mut Editor) -> anyhow::Result<()> {
    let modified_ids: Vec<_> = editor
        .documents()
        .filter(|doc| doc.is_modified())
        .map(|doc| doc.id())
        .collect();

    if let Some(first) = modified_ids.first() {
        let current = doc!(editor);
        // If the current document is unmodified, and there are modified
        // documents, switch focus to the first modified doc.
        if !modified_ids.contains(&current.id()) {
            editor.switch(*first, Action::Replace);
        }

        let modified_names: Vec<_> = modified_ids
            .iter()
            .map(|doc_id| doc!(editor, doc_id).display_name())
            .collect();

        bail!(
            "{} unsaved buffer{} remaining: {:?}",
            modified_names.len(),
            if modified_names.len() == 1 { "" } else { "s" },
            modified_names,
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct WriteAllOptions {
    pub force: bool,
    pub write_scratch: bool,
    pub auto_format: bool,
    pub code_actions: bool,
}

pub fn write_all_impl(
    editor: &mut Editor,
    jobs: &mut Jobs,
    options: WriteAllOptions,
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

            // Look for a view to apply the formatting change to.
            let target_view = editor.get_synced_view_id(doc.id());
            Some((id, target_view))
        })
        .collect();

    for (doc_id, target_view) in saves {
        let doc = doc_mut!(editor, &doc_id);
        let view = view_mut!(editor, target_view);

        if doc.trim_trailing_whitespace() {
            trim_trailing_whitespace(doc, target_view);
        }
        if config.trim_final_newlines {
            trim_final_newlines(doc, target_view);
        }
        if doc.insert_final_newline() {
            insert_final_newline(doc, target_view);
        }

        // Save an undo checkpoint for any outstanding changes.
        doc.append_changes_to_history(view);

        let auto_format = config.auto_format && options.auto_format;
        let force = options.force;

        let run_code_actions = options.code_actions
            && doc!(editor, &doc_id)
                .language_config()
                .and_then(|c| c.code_actions_on_save.as_deref())
                .is_some_and(|kinds| !kinds.is_empty());

        // See `write_impl`: deferred format-or-save tail that always saves, only
        // built when there is pre-save work; otherwise a synchronous save below.
        let tail = (auto_format || run_code_actions).then(|| {
            let callback: job::Callback = Callback::Followup(Box::new(move |editor| {
                // The document could have been closed mid-chain
                if !editor.documents.contains_key(&doc_id) {
                    return None;
                }
                let doc = doc!(editor, &doc_id);
                let fmt_job = auto_format
                    .then(|| doc.auto_format(editor))
                    .flatten()
                    .map(|fmt| {
                        let call = make_format_callback(
                            doc_id,
                            doc.version(),
                            target_view,
                            fmt,
                            Some((None, force)),
                        );
                        Job::with_callback(call).wait_before_exiting()
                    });
                if fmt_job.is_none()
                    && let Err(err) = editor.save::<PathBuf>(doc_id, None, force)
                {
                    editor.set_error(|| format!("Error saving: {}", err));
                }
                fmt_job
            }));
            Job::with_callback(async { Ok(callback) }).wait_before_exiting()
        });

        let job = if run_code_actions {
            code_actions_on_save(editor, doc_id, tail)
        } else {
            tail
        };

        if let Some(job) = job {
            jobs.add(job);
        } else {
            editor.save::<PathBuf>(doc_id, None, force)?;
        }
    }

    if !errors.is_empty() && !options.force {
        bail!("{:?}", errors);
    }

    Ok(())
}

#[cold]
pub(super) fn write_all(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_all_impl(
        cx.editor,
        cx.jobs,
        WriteAllOptions {
            force: false,
            write_scratch: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )
}

#[cold]
pub(super) fn force_write_all(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    write_all_impl(
        cx.editor,
        cx.jobs,
        WriteAllOptions {
            force: true,
            write_scratch: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )
}

#[cold]
pub(super) fn write_all_quit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    write_all_impl(
        cx.editor,
        cx.jobs,
        WriteAllOptions {
            force: false,
            write_scratch: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    )?;
    quit_all_impl(cx, false)
}

#[cold]
pub(super) fn force_write_all_quit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let _ = write_all_impl(
        cx.editor,
        cx.jobs,
        WriteAllOptions {
            force: true,
            write_scratch: true,
            auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
            code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
        },
    );
    quit_all_impl(cx, true)
}

fn quit_all_impl(cx: &mut compositor::Context, force: bool) -> anyhow::Result<()> {
    cx.block_try_flush_writes()?;
    if !force {
        buffers_remaining_impl(cx.editor)?;
    }

    // close all views
    let views: Vec<_> = cx.editor.tree.views().map(|(view, _)| view.id).collect();
    for view_id in views {
        cx.editor.close(view_id);
    }

    Ok(())
}

#[cold]
pub(super) fn quit_all(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    quit_all_impl(cx, false)
}

#[cold]
pub(super) fn force_quit_all(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    quit_all_impl(cx, true)
}

#[cold]
pub(super) fn cquit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let exit_code = args
        .first()
        .and_then(|code| code.parse::<i32>().ok())
        .unwrap_or(1);

    cx.editor.exit_code = exit_code;
    quit_all_impl(cx, false)
}

#[cold]
pub(super) fn force_cquit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let exit_code = args
        .first()
        .and_then(|code| code.parse::<i32>().ok())
        .unwrap_or(1);
    cx.editor.exit_code = exit_code;

    quit_all_impl(cx, true)
}

#[cold]
pub(super) fn theme(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    let true_color = cx.config.current.terminal.true_color || crate::true_color();
    match event {
        PromptEvent::Abort => {
            cx.editor.unset_theme_preview()?;
        }
        PromptEvent::Update => {
            if args.is_empty() {
                // Ensures that a preview theme gets cleaned up if the user backspaces until the prompt is empty.
                cx.editor.unset_theme_preview()?;
            } else if let Some(theme_name) = args.first()
                && let Ok(theme) = cx.editor.theme_loader.load(theme_name)
            {
                if !(true_color || theme.is_16_color()) {
                    bail!("Unsupported theme: theme requires true color support");
                }
                cx.editor.set_theme_preview(theme)?;
            }
        }
        PromptEvent::Validate => {
            if let Some(theme_name) = args.first() {
                let theme = cx
                    .editor
                    .theme_loader
                    .load(theme_name)
                    .map_err(|err| anyhow::anyhow!("Could not load theme: {}", err))?;
                if !(true_color || theme.is_16_color()) {
                    bail!("Unsupported theme: theme requires true color support");
                }
                cx.editor.set_theme(theme)?;
            } else {
                let name = cx.editor.theme.name().to_string();

                cx.editor.set_status(name);
            }
        }
    };

    Ok(())
}

#[cold]
pub(super) fn yank_main_selection_to_clipboard(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    yank_main_selection_to_register(cx.editor, '+');
    Ok(())
}

#[cold]
pub(super) fn yank_joined(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let doc = doc!(cx.editor);
    let default_sep = Cow::Borrowed(doc.line_ending.as_str());
    let separator = args.first().unwrap_or(&default_sep);
    let register = cx
        .editor
        .selected_register
        .unwrap_or(cx.editor.config().default_yank_register);
    yank_joined_impl(cx.editor, separator, register);
    Ok(())
}

#[cold]
pub(super) fn yank_joined_to_clipboard(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let doc = doc!(cx.editor);
    let default_sep = Cow::Borrowed(doc.line_ending.as_str());
    let separator = args.first().unwrap_or(&default_sep);
    yank_joined_impl(cx.editor, separator, '+');
    Ok(())
}

#[cold]
pub(super) fn yank_main_selection_to_primary_clipboard(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    yank_main_selection_to_register(cx.editor, '*');
    Ok(())
}

#[cold]
pub(super) fn yank_joined_to_primary_clipboard(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let doc = doc!(cx.editor);
    let default_sep = Cow::Borrowed(doc.line_ending.as_str());
    let separator = args.first().unwrap_or(&default_sep);
    yank_joined_impl(cx.editor, separator, '*');
    Ok(())
}

#[cold]
pub(super) fn paste_clipboard_after(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    paste(cx.editor, '+', Paste::After, 1);
    Ok(())
}

#[cold]
pub(super) fn paste_clipboard_before(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    paste(cx.editor, '+', Paste::Before, 1);
    Ok(())
}

#[cold]
pub(super) fn paste_primary_clipboard_after(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    paste(cx.editor, '*', Paste::After, 1);
    Ok(())
}

#[cold]
pub(super) fn paste_primary_clipboard_before(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    paste(cx.editor, '*', Paste::Before, 1);
    Ok(())
}

#[cold]
pub(super) fn replace_selections_with_clipboard(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    replace_selections_with_register(cx.editor, '+', 1);
    Ok(())
}

#[cold]
pub(super) fn replace_selections_with_primary_clipboard(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    replace_selections_with_register(cx.editor, '*', 1);
    Ok(())
}

#[cold]
pub(super) fn show_clipboard_provider(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor
        .set_status(cx.editor.registers.clipboard_provider_name());
    Ok(())
}

/// Helper function to parse the first argument as a directory
#[inline]
fn parse_first_arg_as_dir(args: &Args, last_cwd: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match args.first().map(AsRef::as_ref) {
        Some("-") => last_cwd.ok_or_else(|| anyhow!("No previous working directory")),
        Some(path) => Ok(stdx::path::expand_tilde(Path::new(path)).into_owned()),
        None => Ok(home_dir()?),
    }
}

/// Helper function to apply a directory change for an already-parsed Path ref
#[inline]
fn apply_directory_change(cx: &mut compositor::Context, dir: &Path) -> anyhow::Result<()> {
    cx.editor.set_cwd(dir).map_err(|err| {
        anyhow!(
            "Could not change working directory to '{}': {err}",
            dir.display()
        )
    })?;

    cx.editor.set_status(format!(
        "Current working directory is now {}",
        stdx::env::current_working_dir().display()
    ));

    Ok(())
}

#[cold]
pub(super) fn change_current_directory(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let dir = parse_first_arg_as_dir(&args, cx.editor.get_last_cwd().map(|p| p.to_path_buf()))?;

    apply_directory_change(cx, &dir)
}

#[cold]
pub(super) fn show_directory_stack(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let serialized_stack = cx
        .editor
        .dir_stack
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" ");

    if !serialized_stack.is_empty() {
        cx.editor.set_status(serialized_stack);
    } else {
        cx.editor.set_error(|| "Stack is empty");
    }

    Ok(())
}

#[cold]
pub(super) fn push_directory(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    // avoid an unbounded directory stack and reallocs for perf
    if cx.editor.dir_stack.len() == cx.editor.dir_stack.capacity() {
        cx.editor.dir_stack.pop_back();
    }

    cx.editor
        .dir_stack
        .push_front(stdx::env::current_working_dir());

    change_current_directory(cx, args, event)
}

#[cold]
pub(super) fn pop_directory(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if let Some(dir) = cx.editor.dir_stack.pop_front() {
        apply_directory_change(cx, &dir)?;
    } else {
        cx.editor.set_error(|| "Stack is empty");
    }

    Ok(())
}

#[cold]
pub(super) fn show_current_directory(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let cwd = stdx::env::current_working_dir();
    let message = format!("Current working directory is {}", cwd.display());

    if cwd.exists() {
        cx.editor.set_status(message);
    } else {
        cx.editor.set_error(|| format!("{} (deleted)", message));
    }
    Ok(())
}

/// Sets the [`Document`]'s encoding..
#[cold]
pub(super) fn set_encoding(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let doc = doc_mut!(cx.editor);
    if let Some(label) = args.first() {
        doc.set_encoding(label)
    } else {
        let encoding = doc.encoding().name().to_owned();
        cx.editor.set_status(encoding);
        Ok(())
    }
}

/// Shows info about the character under the primary cursor.
#[cold]
pub(super) fn get_character_info(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);

    let grapheme_start = doc.selection(view.id).primary().cursor(text);
    let grapheme_end = graphemes::next_grapheme_boundary(text, grapheme_start);

    if grapheme_start == grapheme_end {
        return Ok(());
    }

    let grapheme = text.slice(grapheme_start..grapheme_end).to_string();
    let encoding = doc.encoding();

    let printable = grapheme.chars().fold(String::new(), |mut s, c| {
        match c {
            '\0' => s.push_str("\\0"),
            '\t' => s.push_str("\\t"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            _ => s.push(c),
        }

        s
    });

    // Convert to Unicode codepoints if in UTF-8
    let unicode = if encoding == encoding::UTF_8 {
        let mut unicode = " (".to_owned();

        for (i, char) in grapheme.chars().enumerate() {
            if i != 0 {
                unicode.push(' ');
            }

            unicode.push_str("U+");

            let codepoint: u32 = if char.is_ascii() {
                char.into()
            } else {
                // Not ascii means it will be multi-byte, so strip out the extra
                // bits that encode the length & mark continuation bytes

                let s = String::from(char);
                let bytes = s.as_bytes();

                // First byte starts with 2-4 ones then a zero, so strip those off
                let first = bytes[0];
                let codepoint = first & (0xFF >> (first.leading_ones() + 1));
                let mut codepoint = u32::from(codepoint);

                // Following bytes start with 10
                for byte in bytes.iter().skip(1) {
                    codepoint <<= 6;
                    codepoint += u32::from(*byte) & 0x3F;
                }

                codepoint
            };

            write!(unicode, "{codepoint:0>4x}").unwrap();
        }

        unicode.push(')');
        unicode
    } else {
        String::new()
    };

    // Give the decimal value for ascii characters
    let dec = if encoding.is_ascii_compatible() && grapheme.len() == 1 {
        format!(" Dec {}", grapheme.as_bytes()[0])
    } else {
        String::new()
    };

    let hex = {
        let mut encoder = encoding.new_encoder();
        let max_encoded_len = encoder
            .max_buffer_length_from_utf8_without_replacement(grapheme.len())
            .unwrap();
        let mut bytes = Vec::with_capacity(max_encoded_len);
        let mut current_byte = 0;
        let mut hex = String::new();

        for (i, char) in grapheme.chars().enumerate() {
            if i != 0 {
                hex.push_str(" +");
            }

            let (result, _input_bytes_read) = encoder.encode_from_utf8_to_vec_without_replacement(
                &char.to_string(),
                &mut bytes,
                true,
            );

            if let encoding::EncoderResult::Unmappable(char) = result {
                bail!("{char:?} cannot be mapped to {}", encoding.name());
            }

            for byte in &bytes[current_byte..] {
                write!(hex, " {byte:0>2x}").unwrap();
            }

            current_byte = bytes.len();
        }

        hex
    };

    cx.editor
        .set_status(format!("\"{printable}\"{unicode}{dec} Hex{hex}"));

    Ok(())
}

/// Reload the [`Document`] from its source file.
#[cold]
pub(super) fn reload(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let trust_full = doc_trust_full(cx.editor);
    let (view, doc) = current!(cx.editor);
    doc.reload(view, &cx.editor.diff_providers, trust_full)
        .map(|_| {
            view.ensure_cursor_in_view(doc, scrolloff);
        })?;
    if let Some(path) = doc.path().map(ToOwned::to_owned)
        && !cx.editor.file_watcher.is_watching(&path)
    {
        cx.editor
            .language_servers
            .file_event_handler
            .file_changed(path, editor_core::file_watcher::EventType::Modified);
    }
    Ok(())
}

#[cold]
pub(super) fn reload_all(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let view_id = view!(cx.editor).id;

    let docs_view_ids: Vec<(DocumentId, Vec<ViewId>)> = cx
        .editor
        .documents_mut()
        .map(|doc| {
            let mut view_ids: Vec<_> = doc.selections().keys().cloned().collect();

            if view_ids.is_empty() {
                doc.ensure_view_init(view_id);
                view_ids.push(view_id);
            };

            (doc.id(), view_ids)
        })
        .collect();

    for (doc_id, view_ids) in docs_view_ids {
        let doc = doc_mut!(cx.editor, &doc_id);

        // Every doc is guaranteed to have at least 1 view at this point.
        let view = view_mut!(cx.editor, view_ids[0]);

        // Ensure that the view is synced with the document's history.
        view.sync_changes(doc);

        // Per-document trust: each doc's workspace may differ.
        let trust_full = cx
            .editor
            .workspace_trust
            .query(
                doc.workspace_root(),
                loader::workspace_trust::TrustQuery::Git,
            )
            .is_trusted();
        if let Err(error) = doc.reload(view, &cx.editor.diff_providers, trust_full) {
            cx.editor.set_error(|| format!("{}", error));
            continue;
        }

        if let Some(path) = doc.path().map(ToOwned::to_owned)
            && !cx.editor.file_watcher.is_watching(&path)
        {
            cx.editor
                .language_servers
                .file_event_handler
                .file_changed(path, editor_core::file_watcher::EventType::Modified);
        }

        for view_id in view_ids {
            let view = view_mut!(cx.editor, view_id);
            if view.doc.eq(&doc_id) {
                // Reloading commits the diff against disk through the first view
                // only (above). Any other view onto this document is left
                // pointing at the pre-reload revision, so sync it now; otherwise
                // its jumplist entries keep referencing the old (e.g. larger)
                // text and a later commit panics when mapping them through a
                // changeset whose pre-image no longer contains them.
                view.sync_changes(doc);
                view.ensure_cursor_in_view(doc, scrolloff);
            }
        }
    }

    Ok(())
}

/// Update the [`Document`] if it has been modified.
#[cold]
pub(super) fn update(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (_view, doc) = current!(cx.editor);
    if doc.is_modified() {
        write_impl(
            cx,
            None,
            WriteOptions {
                force: false,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )
    } else {
        Ok(())
    }
}

#[cold]
pub(super) fn lsp_workspace_command(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let doc = doc!(cx.editor);
    let ls_id_commands = doc
        .language_servers_with_feature(LanguageServerFeature::WorkspaceCommand)
        .flat_map(|ls| {
            ls.capabilities()
                .execute_command_provider
                .iter()
                .flat_map(|options| options.commands.iter())
                .map(|command| (ls.id(), command))
        });

    if args.is_empty() {
        let commands = ls_id_commands
            .map(|(ls_id, command)| {
                (
                    ls_id,
                    lsp_client::lsp::Command {
                        title: command.clone(),
                        command: command.clone(),
                        arguments: None,
                    },
                )
            })
            .collect::<Vec<_>>();
        let callback = async move {
            let call: job::Callback = Callback::EditorCompositor(Box::new(
                move |_editor: &mut Editor, compositor: &mut Compositor| {
                    let columns = [ui::PickerColumn::new(
                        "title",
                        |(_ls_id, command): &(_, lsp_client::lsp::Command), _| {
                            command.title.as_str().into()
                        },
                    )];
                    let picker = ui::Picker::new(
                        columns,
                        0,
                        commands,
                        (),
                        move |cx, (ls_id, command), _action| {
                            cx.editor.execute_lsp_command(command.clone(), *ls_id);
                        },
                    );
                    compositor.push(Box::new(overlaid(picker)))
                },
            ));
            Ok(call)
        };
        cx.jobs.callback(callback);
    } else {
        let command = args[0].to_string();
        let matches: Vec<_> = ls_id_commands
            .filter(|(_ls_id, c)| *c == &command)
            .collect();

        match matches.as_slice() {
            [(ls_id, _command)] => {
                let arguments = args
                    .get(1)
                    .map(|rest| {
                        serde_json::Deserializer::from_str(rest)
                            .into_iter()
                            .collect::<Result<Vec<Value>, _>>()
                            .map_err(|err| anyhow!("failed to parse arguments: {err}"))
                    })
                    .transpose()?
                    .filter(|args| !args.is_empty());

                cx.editor.execute_lsp_command(
                    lsp_client::lsp::Command {
                        title: command.clone(),
                        arguments,
                        command,
                    },
                    *ls_id,
                );
            }
            [] => {
                cx.editor.set_status(format!(
                    "`{command}` is not supported for any language server"
                ));
            }
            _ => {
                cx.editor.set_status(format!(
                    "`{command}` supported by multiple language servers"
                ));
            }
        }
    }
    Ok(())
}

#[cold]
pub(super) fn lsp_restart(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let editor_config = cx.editor.config.load();
    let doc = doc!(cx.editor);
    let config = doc
        .language_config()
        .context("LSP not defined for the current document")?;

    let language_servers: Vec<_> = config
        .language_servers
        .iter()
        .map(|ls| ls.name.as_str())
        .collect();
    let language_servers = if args.is_empty() {
        language_servers
    } else {
        let (valid, invalid): (Vec<_>, Vec<_>) = args
            .iter()
            .map(|arg| arg.as_ref())
            .partition(|name| language_servers.contains(name));
        if !invalid.is_empty() {
            let s = if invalid.len() == 1 { "" } else { "s" };
            bail!("Unknown language server{s}: {}", invalid.join(", "));
        }
        valid
    };

    let mut errors = Vec::new();
    for server in language_servers.iter() {
        match cx
            .editor
            .language_servers
            .restart_server(
                server,
                config,
                doc.path(),
                &editor_config.workspace_lsp_roots,
                editor_config.lsp.snippets,
            )
            .transpose()
        {
            // Ignore the executable-not-found error unless the server was explicitly requested
            // in the arguments.
            Err(lsp_client::Error::ExecutableNotFound(_))
                if !args.iter().any(|arg| arg == server) => {}
            Err(err) => errors.push(err.to_string()),
            _ => (),
        }
    }

    // This collect is needed because refresh_language_server would need to re-borrow editor.
    let document_ids_to_refresh: Vec<DocumentId> = cx
        .editor
        .documents()
        .filter_map(|doc| match doc.language_config() {
            Some(config)
                if config.language_servers.iter().any(|ls| {
                    language_servers
                        .iter()
                        .any(|restarted_ls| restarted_ls == &ls.name)
                }) =>
            {
                Some(doc.id())
            }
            _ => None,
        })
        .collect();

    for document_id in document_ids_to_refresh {
        cx.editor.refresh_language_servers(document_id);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Error restarting language servers: {}",
            errors.join(", ")
        ))
    }
}

#[cold]
pub(super) fn lsp_stop(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let doc = doc!(cx.editor);

    let language_servers: Vec<_> = doc
        .language_servers()
        .map(|ls| ls.name().to_string())
        .collect();
    let language_servers = if args.is_empty() {
        language_servers
    } else {
        let (valid, invalid): (Vec<_>, Vec<_>) = args
            .iter()
            .map(|arg| arg.to_string())
            .partition(|name| language_servers.contains(name));
        if !invalid.is_empty() {
            let s = if invalid.len() == 1 { "" } else { "s" };
            bail!("Unknown language server{s}: {}", invalid.join(", "));
        }
        valid
    };

    for ls_name in &language_servers {
        cx.editor.language_servers.stop(ls_name);

        for doc in cx.editor.documents_mut() {
            if let Some(client) = doc.remove_language_server_by_name(ls_name) {
                doc.clear_diagnostics_for_language_server(client.id());
                doc.reset_all_inlay_hints();
                doc.inlay_hints_oudated = true;
                doc.clear_document_symbols();
            }
        }
    }

    Ok(())
}

#[cold]
pub(super) fn tree_sitter_scopes(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let pos = doc.selection(view.id).primary().cursor(text);
    let scopes = indent::get_scopes(doc.syntax(), text, pos);

    let contents = format!("```json\n{:?}\n````", scopes);

    let callback = async move {
        let call: job::Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                let contents = ui::Markdown::new(contents, editor.syn_loader.clone());
                let popup = Popup::new("hover", contents).auto_close(true);
                compositor.replace_or_push("hover", popup);
            },
        ));
        Ok(call)
    };

    cx.jobs.callback(callback);

    Ok(())
}

#[cold]
pub(super) fn tree_sitter_highlight_name(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current_ref!(cx.editor);
    let Some(syntax) = doc.syntax() else {
        return Ok(());
    };
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let byte = text.char_to_byte(cursor) as u32;
    // Query the same range as the one used in syntax highlighting.
    let range = {
        // Calculate viewport byte ranges:
        let row = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
        // Saturating subs to make it inclusive zero indexing.
        let last_line = text.len_lines().saturating_sub(1);
        let height = view.inner_area(doc).height;
        let last_visible_line = (row + height as usize).saturating_sub(1).min(last_line);
        let start = text.line_to_byte(row.min(last_line)) as u32;
        let end = text.line_to_byte(last_visible_line + 1) as u32;

        start..end
    };

    let loader = cx.editor.syn_loader.load();
    let mut highlighter = syntax.highlighter(text, &loader, range);
    let mut highlights = Vec::new();

    while highlighter.next_event_offset() <= byte {
        let (event, new_highlights) = highlighter.advance();
        if event == editor_core::syntax::HighlightEvent::Refresh {
            highlights.clear();
        }
        highlights.extend(new_highlights);
    }

    let content = highlights
        .into_iter()
        .fold(String::new(), |mut acc, highlight| {
            if !acc.is_empty() {
                acc.push_str(", ");
            }
            acc.push_str(cx.editor.theme.scope(highlight));
            acc
        });

    let callback = async move {
        let call: job::Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                let content = ui::Markdown::new(content, editor.syn_loader.clone());
                let popup = Popup::new("hover", content).auto_close(true);
                compositor.replace_or_push("hover", popup);
            },
        ));
        Ok(call)
    };

    cx.jobs.callback(callback);

    Ok(())
}

#[cold]
pub(super) fn tree_sitter_layers(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current_ref!(cx.editor);
    let Some(syntax) = doc.syntax() else {
        bail!("Syntax information is not available");
    };

    let loader: &editor_core::syntax::Loader = &cx.editor.syn_loader.load();
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let byte = text.char_to_byte(cursor) as u32;
    let languages =
        syntax
            .layers_for_byte_range(byte, byte)
            .fold(String::new(), |mut acc, layer| {
                if !acc.is_empty() {
                    acc.push_str(", ");
                }
                acc.push_str(
                    &loader
                        .language(syntax.layer(layer).language)
                        .config()
                        .language_id,
                );
                acc
            });

    let callback = async move {
        let call: job::Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                let content = ui::Markdown::new(languages, editor.syn_loader.clone());
                let popup = Popup::new("hover", content).auto_close(true);
                compositor.replace_or_push("hover", popup);
            },
        ));
        Ok(call)
    };

    cx.jobs.callback(callback);

    Ok(())
}

#[cold]
pub(super) fn vsplit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if args.is_empty() {
        split(cx.editor, Action::VerticalSplit);
    } else {
        open_impl(cx, args, Action::VerticalSplit)?;
    }

    Ok(())
}

#[cold]
pub(super) fn hsplit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if args.is_empty() {
        split(cx.editor, Action::HorizontalSplit);
    } else {
        open_impl(cx, args, Action::HorizontalSplit)?;
    }

    Ok(())
}

#[cold]
pub(super) fn vsplit_new(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor.new_file(Action::VerticalSplit);

    Ok(())
}

#[cold]
pub(super) fn hsplit_new(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor.new_file(Action::HorizontalSplit);

    Ok(())
}

#[cold]
pub(super) fn debug_eval(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if let Some(debugger) = cx.editor.debug_adapters.get_active_client() {
        let (frame, thread_id) = match (debugger.active_frame, debugger.thread_id) {
            (Some(frame), Some(thread_id)) => (frame, thread_id),
            _ => {
                bail!("Cannot find current stack frame to access variables")
            }
        };

        // TODO: support no frame_id

        let frame_id = debugger.stack_frames[&thread_id][frame].id;
        let response = lsp_client::block_on(debugger.eval(args.join(" "), Some(frame_id)))?;
        cx.editor.set_status(response.result);
    }
    Ok(())
}

#[cold]
pub(super) fn debug_start(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let mut args: Vec<_> = args.into_iter().collect();
    let name = match args.len() {
        0 => None,
        _ => Some(args.remove(0)),
    };
    dap_start_impl(cx, name.as_deref(), None, Some(args))
}

#[cold]
pub(super) fn debug_remote(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let mut args: Vec<_> = args.into_iter().collect();
    let address = match args.len() {
        0 => None,
        _ => Some(args.remove(0).parse()?),
    };
    let name = match args.len() {
        0 => None,
        _ => Some(args.remove(0)),
    };
    dap_start_impl(cx, name.as_deref(), address, Some(args))
}

#[cold]
pub(super) fn tutor(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let path = loader::runtime_file(Path::new("tutor"));
    cx.editor.open(&path, Action::Replace)?;
    // Unset path to prevent accidentally saving to the original tutor file.
    doc_mut!(cx.editor).set_path(None);
    Ok(())
}

fn abort_goto_line_number_preview(cx: &mut compositor::Context) {
    if let Some(last_selection) = cx.editor.last_selection.take() {
        let scrolloff = cx.editor.config().scrolloff;

        let (view, doc) = current!(cx.editor);
        doc.set_selection(view.id, last_selection);
        view.ensure_cursor_in_view(doc, scrolloff);
    }
}

fn update_goto_line_number_preview(cx: &mut compositor::Context, args: Args) -> anyhow::Result<()> {
    cx.editor.last_selection.get_or_insert_with(|| {
        let (view, doc) = current!(cx.editor);
        doc.selection(view.id).clone()
    });

    let scrolloff = cx.editor.config().scrolloff;
    let line = args[0].parse::<usize>()?;
    goto_line_without_jumplist(
        cx.editor,
        NonZeroUsize::new(line),
        if cx.editor.mode == Mode::Select {
            Movement::Extend
        } else {
            Movement::Move
        },
    );

    let (view, doc) = current!(cx.editor);
    view.ensure_cursor_in_view(doc, scrolloff);

    Ok(())
}

#[cold]
pub(super) fn goto_line_number(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    match event {
        PromptEvent::Abort => abort_goto_line_number_preview(cx),
        PromptEvent::Validate => {
            // If we are invoked directly via a keybinding, Validate is
            // sent without any prior Update events. Ensure the cursor
            // is moved to the appropriate location.
            update_goto_line_number_preview(cx, args)?;

            let last_selection = cx
                .editor
                .last_selection
                .take()
                .expect("update_goto_line_number_preview should always set last_selection");

            let (view, doc) = current!(cx.editor);
            view.push_jump(doc, (doc.id(), last_selection));
        }

        // When a user hits backspace and there are no numbers left,
        // we can bring them back to their original selection. If they
        // begin typing numbers again, we'll start a new preview session.
        PromptEvent::Update if args.is_empty() => abort_goto_line_number_preview(cx),
        PromptEvent::Update => update_goto_line_number_preview(cx, args)?,
    }

    Ok(())
}

// Fetch the current value of a config option and output as status.
#[cold]
pub(super) fn get_option(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let key = &args[0].to_lowercase();
    let key_error = || anyhow::anyhow!("Unknown key `{}`", key);

    let config = serde_json::json!(cx.config.current.editor_settings());
    let pointer = format!("/{}", key.replace('.', "/"));
    let value = config.pointer(&pointer).ok_or_else(key_error)?;

    cx.editor.set_status(value.to_string());
    Ok(())
}

/// Change config at runtime. Access nested values by dot syntax, for
/// example to disable smart case search, use `:set search.smart-case false`.
#[cold]
pub(super) fn set_option(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (key, arg) = (&args[0].to_lowercase(), args[1].trim());

    let key_error = || anyhow::anyhow!("Unknown key `{}`", key);
    let field_error = |_| anyhow::anyhow!("Could not parse field `{}`", arg);

    let mut config = serde_json::json!(cx.config.current.editor_settings());
    let pointer = format!("/{}", key.replace('.', "/"));
    let value = config.pointer_mut(&pointer).ok_or_else(key_error)?;

    *value = if value.is_string() {
        // JSON strings require quotes, so we can't .parse() directly
        Value::String(arg.to_string())
    } else {
        arg.parse().map_err(field_error)?
    };
    let mut config: Box<EditorSettings> = serde_json::from_value(config).map_err(field_error)?;
    config.editor.commands = cx.editor.config().commands.clone();

    cx.config.updates.send(ConfigEvent::Update(config))?;
    Ok(())
}

/// Toggle boolean config option at runtime. Access nested values by dot
/// syntax, for example to toggle smart case search, use `:toggle search.smart-
/// case`.
#[cold]
pub(super) fn toggle_option(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let key = &args[0].to_lowercase();

    let key_error = || anyhow::anyhow!("Unknown key `{}`", key);

    let mut config = serde_json::json!(cx.config.current.editor_settings());
    let pointer = format!("/{}", key.replace('.', "/"));
    let value = config.pointer_mut(&pointer).ok_or_else(key_error)?;

    *value = match value {
        &mut Value::Bool(ref value) => {
            ensure!(
                args.len() == 1,
                "Bad arguments. For boolean configurations use: `:toggle {key}`"
            );
            Value::Bool(!value)
        }
        &mut Value::String(ref value) => {
            ensure!(
                args.len() == 2,
                "Bad arguments. For string configurations use: `:toggle {key} val1 val2 ...`",
            );
            // For string values, parse the input according to normal command line rules.
            let values: Vec<_> = command_line::Tokenizer::new(&args[1], true)
                .map(|res| res.map(|token| token.content))
                .collect::<Result<_, _>>()
                .map_err(|err| anyhow!("failed to parse values: {err}"))?;

            Value::String(
                values
                    .iter()
                    .skip_while(|e| *e != value)
                    .nth(1)
                    .map(AsRef::as_ref)
                    .unwrap_or_else(|| &values[0])
                    .to_string(),
            )
        }
        Value::Null => bail!("Configuration {key} cannot be toggled"),
        Value::Number(_) | Value::Array(_) | Value::Object(_) => {
            ensure!(
                args.len() == 2,
                "Bad arguments. For {kind} configurations use: `:toggle {key} val1 val2 ...`",
                kind = match value {
                    Value::Number(_) => "number",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                    _ => unreachable!(),
                }
            );
            // For numbers, arrays and objects, parse each argument with
            // `serde_json::StreamDeserializer`.
            let values: Vec<Value> = serde_json::Deserializer::from_str(&args[1])
                .into_iter()
                .collect::<Result<_, _>>()
                .map_err(|err| anyhow!("failed to parse value: {err}"))?;

            if let Some(wrongly_typed_value) = values
                .iter()
                .find(|v| std::mem::discriminant(*v) != std::mem::discriminant(&*value))
            {
                bail!("value '{wrongly_typed_value}' has a different type than '{value}'");
            }

            values
                .iter()
                .skip_while(|e| *e != value)
                .nth(1)
                .unwrap_or(&values[0])
                .clone()
        }
    };

    let status = format!("'{key}' is now set to {value}");
    let mut config: Box<EditorSettings> = serde_json::from_value(config)
        .map_err(|err| anyhow::anyhow!("Failed to parse config: {err}"))?;
    config.editor.commands = cx.editor.config().commands.clone();

    cx.config.updates.send(ConfigEvent::Update(config))?;
    cx.editor.set_status(status);
    Ok(())
}

/// Change the language of the current buffer at runtime.
#[cold]
pub(super) fn language(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if args.is_empty() {
        let doc = doc!(cx.editor);
        let language = &doc.language_name().unwrap_or(DEFAULT_LANGUAGE_NAME);
        cx.editor.set_status(language.to_string());
        return Ok(());
    }

    let doc = doc_mut!(cx.editor);

    let loader = cx.editor.syn_loader.load();
    if &args[0] == DEFAULT_LANGUAGE_NAME {
        doc.set_language(None, &loader)
    } else {
        doc.set_language_by_language_id(&args[0], &loader)?;
    }
    doc.detect_indent_and_line_ending();

    let id = doc.id();
    cx.editor.refresh_language_servers(id);
    let doc = doc_mut!(cx.editor);
    let diagnostics =
        Editor::doc_diagnostics(&cx.editor.language_servers, &cx.editor.diagnostics, doc);
    doc.replace_diagnostics(diagnostics, &[], None);
    cx.editor.refresh_spelling(id);
    Ok(())
}

#[cold]
pub(super) fn spelling_language(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if args.is_empty() {
        let doc = doc!(cx.editor);
        let status = if doc.spelling_languages.is_empty() {
            "off".to_string()
        } else {
            doc.spelling_languages
                .iter()
                .map(|language| language.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        cx.editor.set_status(status);
        return Ok(());
    }

    let languages = if args.len() == 1 && &args[0] == "off" {
        Vec::new()
    } else {
        args.iter()
            .map(|arg| arg.parse())
            .collect::<Result<Vec<editor_core::SpellingLanguage>, _>>()?
    };
    let doc = doc_mut!(cx.editor);
    let doc_id = doc.id();
    doc.spelling_language_override = Some(languages);
    cx.editor.refresh_spelling(doc_id);

    Ok(())
}

pub(super) fn sort(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let selection = doc.selection(view.id);

    if selection.len() == 1 {
        bail!("Sorting requires multiple selections. Hint: split selection first");
    }

    let mut fragments: Vec<_> = selection
        .slices(text)
        .map(|fragment| fragment.chunks().collect())
        .collect();

    fragments.sort_by(
        match (args.has_flag("insensitive"), args.has_flag("reverse")) {
            (true, true) => |a: &Tendril, b: &Tendril| b.to_lowercase().cmp(&a.to_lowercase()),
            (true, false) => |a: &Tendril, b: &Tendril| a.to_lowercase().cmp(&b.to_lowercase()),
            (false, true) => |a: &Tendril, b: &Tendril| b.cmp(a),
            (false, false) => |a: &Tendril, b: &Tendril| a.cmp(b),
        },
    );

    let transaction = Transaction::change(
        doc.text(),
        selection
            .into_iter()
            .zip(fragments)
            .map(|(s, fragment)| (s.from(), s.to(), Some(fragment))),
    );

    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);

    Ok(())
}

#[cold]
pub(super) fn reflow(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let (view, doc) = current!(cx.editor);

    // Find the text_width by checking the following sources in order:
    //   - The passed argument in `args`
    //   - The configured text-width for this language in languages.toml
    //   - The configured text-width in the config.toml
    let text_width: usize = args
        .first()
        .map(|num| num.parse::<usize>())
        .transpose()?
        .unwrap_or_else(|| doc.text_width());

    let rope = doc.text();

    let selection = doc.selection(view.id);
    let transaction = Transaction::change_by_selection(rope, selection, |range| {
        let fragment = range.fragment(rope.slice(..));
        let reflowed_text = editor_core::wrap::reflow_hard_wrap(&fragment, text_width);

        (range.from(), range.to(), Some(reflowed_text))
    });

    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);

    Ok(())
}

#[cold]
pub(super) fn tree_sitter_subtree(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (view, doc) = current!(cx.editor);

    if let Some(syntax) = doc.syntax() {
        let primary_selection = doc.selection(view.id).primary();
        let text = doc.text();
        let from = text.char_to_byte(primary_selection.from()) as u32;
        let to = text.char_to_byte(primary_selection.to()) as u32;
        if let Some(selected_node) = syntax.descendant_for_byte_range(from, to) {
            let mut contents = String::from("```tsq\n");
            editor_core::syntax::pretty_print_tree(&mut contents, selected_node)?;
            contents.push_str("\n```");

            let callback = async move {
                let call: job::Callback = Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        let contents = ui::Markdown::new(contents, editor.syn_loader.clone());
                        let popup = Popup::new("hover", contents).auto_close(true);
                        compositor.replace_or_push("hover", popup);
                    },
                ));
                Ok(call)
            };

            cx.jobs.callback(callback);
        }
    }

    Ok(())
}

#[cold]
pub(super) fn open_config(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor.open(&loader::config_file(), Action::Replace)?;
    Ok(())
}

#[cold]
pub(super) fn open_workspace_config(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor
        .open(&loader::workspace_config_file(), Action::Replace)?;
    Ok(())
}

#[cold]
pub(super) fn open_log(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.editor.open(&loader::log_file(), Action::Replace)?;
    Ok(())
}

#[cold]
pub(super) fn refresh_config(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    cx.config.updates.send(ConfigEvent::Refresh)?;
    Ok(())
}

#[cold]
pub(super) fn append_output(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    shell(cx, &args.join(" "), &ShellBehavior::Append);
    Ok(())
}

#[cold]
pub(super) fn insert_output(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    shell(cx, &args.join(" "), &ShellBehavior::Insert);
    Ok(())
}

pub(super) fn pipe_to(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    pipe_impl(cx, args, event, &ShellBehavior::Ignore)
}

pub(super) fn pipe(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    pipe_impl(cx, args, event, &ShellBehavior::Replace)
}

#[cold]
fn pipe_impl(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
    behavior: &ShellBehavior,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    shell(cx, &args.join(" "), behavior);
    Ok(())
}

#[cold]
pub(super) fn run_shell_command(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let shell = cx.editor.config().shell.clone();
    let args = args.join(" ");

    let callback = async move {
        let output = shell_impl_async(&shell, &args, None).await?;
        let call: job::Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                if !output.trim().is_empty() {
                    let contents = ui::Markdown::new(
                        format!("```sh\n{}\n```", output.trim_end()),
                        editor.syn_loader.clone(),
                    );
                    let popup = Popup::new("shell", contents).position(Some(
                        editor_core::Position::new(editor.cursor().0.unwrap_or_default().row, 2),
                    ));
                    compositor.replace_or_push("shell", popup);
                }
                editor.set_status("Command run");
            },
        ));
        Ok(call)
    };
    cx.jobs.callback(callback);

    Ok(())
}

#[cold]
pub(super) fn reset_diff_change(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let editor = &mut cx.editor;
    let scrolloff = editor.config().scrolloff;

    let (view, doc) = current!(editor);
    let Some(handle) = doc.diff_handle() else {
        bail!("Diff is not available in the current buffer")
    };

    let diff = handle.load();
    let doc_text = doc.text().slice(..);
    let diff_base = diff.diff_base();
    let mut changes = 0;

    let transaction = Transaction::change(
        doc.text(),
        diff.hunks_intersecting_line_ranges(doc.selection(view.id).line_ranges(doc_text))
            .map(|hunk| {
                changes += 1;
                let start = diff_base.line_to_char(hunk.before.start as usize);
                let end = diff_base.line_to_char(hunk.before.end as usize);
                let text: Tendril = diff_base.slice(start..end).chunks().collect();
                (
                    doc_text.line_to_char(hunk.after.start as usize),
                    doc_text.line_to_char(hunk.after.end as usize),
                    (!text.is_empty()).then_some(text),
                )
            }),
    );
    if changes == 0 {
        bail!("There are no changes under any selection");
    }

    drop(diff); // make borrow check happy
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);
    cx.editor.set_status(format!(
        "Reset {changes} change{}",
        if changes == 1 { "" } else { "s" }
    ));
    Ok(())
}

#[cold]
pub(super) fn clear_register(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if args.is_empty() {
        cx.editor.registers.clear();
        cx.editor.set_status("All registers cleared");
        return Ok(());
    }

    ensure!(
        args[0].chars().count() == 1,
        format!("Invalid register {}", &args[0])
    );
    let register = args[0].chars().next().unwrap_or_default();
    if cx.editor.registers.remove(register) {
        cx.editor
            .set_status(format!("Register {} cleared", register));
    } else {
        cx.editor
            .set_error(|| format!("Register {} not found", register));
    }
    Ok(())
}

#[cold]
pub(super) fn set_register(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    ensure!(
        args[0].chars().count() == 1,
        format!("Invalid register {}", &args[0])
    );

    let register = args[0].chars().next().unwrap_or_default();
    cx.editor.registers.write(register, vec![args[1].into()])
}

#[cold]
pub(super) fn redraw(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let callback = Box::pin(async move {
        let call: job::Callback =
            job::Callback::EditorCompositor(Box::new(|_editor, compositor| {
                compositor.need_full_redraw();
            }));

        Ok(call)
    });

    cx.jobs.callback(callback);

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct MoveBufferOptions {
    pub force: bool,
}

#[cold]
pub(super) fn move_buffer(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let new_path: PathBuf = args.first().unwrap().into();
    move_buffer_impl(cx, new_path, MoveBufferOptions { force: false })
}

#[cold]
pub(super) fn force_move_buffer(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let new_path: PathBuf = args.first().unwrap().into();
    move_buffer_impl(cx, new_path, MoveBufferOptions { force: true })
}

fn move_buffer_impl(
    cx: &mut compositor::Context,
    new_path: PathBuf,
    options: MoveBufferOptions,
) -> anyhow::Result<()> {
    let doc = doc!(cx.editor);
    let old_path = doc
        .path()
        .map(ToOwned::to_owned)
        .context("Scratch buffer cannot be moved. Use :write instead")?;

    // if new_path is a directory, append the original file name
    // to move the file into that directory.
    let new_path = old_path
        .file_name()
        .filter(|_| new_path.is_dir())
        .map(|old_file_name| new_path.join(old_file_name))
        .unwrap_or(new_path);

    if old_path.exists()
        && let Some(parent) = new_path.parent()
        && !parent.exists()
    {
        if options.force {
            std::fs::DirBuilder::new().recursive(true).create(parent)?;
        } else {
            bail!("can't move file, parent directory does not exist (use :mv! to create it)")
        }
    }

    if let Err(err) = cx.editor.move_path(&old_path, new_path.as_ref()) {
        bail!("Could not move file: {err}");
    }
    Ok(())
}

#[cold]
pub(super) fn yank_diagnostic(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let reg = match args.first() {
        Some(s) => {
            ensure!(s.chars().count() == 1, format!("Invalid register {s}"));
            s.chars().next().unwrap()
        }
        None => '+',
    };

    let (view, doc) = current_ref!(cx.editor);
    let primary = doc.selection(view.id).primary();

    // Look only for diagnostics that intersect with the primary selection
    let diag: Vec<_> = doc
        .diagnostics()
        .iter()
        .filter(|d| primary.overlaps(&editor_core::Range::new(d.range.start, d.range.end)))
        .map(|d| d.message.clone().into_string())
        .collect();
    let n = diag.len();
    if n == 0 {
        bail!("No diagnostics under primary selection");
    }

    cx.editor.registers.write(reg, diag)?;
    cx.editor.set_status(format!(
        "Yanked {n} diagnostic{} to register {reg}",
        if n == 1 { "" } else { "s" }
    ));
    Ok(())
}

#[cold]
pub(super) fn read(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let (view, doc) = current!(cx.editor);

    let filename = args.first().unwrap();
    let path = stdx::path::expand_tilde(PathBuf::from(filename.to_string()));

    ensure!(
        path.exists() && path.is_file(),
        "path is not a file: {:?}",
        path
    );

    let file = std::fs::File::open(path).map_err(|err| anyhow!("error opening file: {}", err))?;
    let mut reader = BufReader::new(file);
    let (contents, _, _) = read_to_string(&mut reader, Some(doc.encoding()))
        .map_err(|err| anyhow!("error reading file: {}", err))?;
    let contents = Tendril::from(contents);
    let selection = doc.selection(view.id);
    let transaction = Transaction::insert(doc.text(), selection, contents);
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    view.ensure_cursor_in_view(doc, scrolloff);

    Ok(())
}

#[cold]
pub(super) fn echo(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let output = args.into_iter().fold(String::new(), |mut acc, arg| {
        if !acc.is_empty() {
            acc.push(' ');
        }
        acc.push_str(&arg);
        acc
    });
    cx.editor.set_status(output);

    Ok(())
}

#[cold]
pub(super) fn noop(
    _cx: &mut compositor::Context,
    _args: Args,
    _event: PromptEvent,
) -> anyhow::Result<()> {
    Ok(())
}

fn current_workspace(cx: &compositor::Context) -> std::path::PathBuf {
    let (_, doc) = current_ref!(cx.editor);
    doc.workspace_root().to_path_buf()
}

/// Whether the currently focused document's workspace is trusted for git operations (gix
/// `Trust::Full`).
fn doc_trust_full(editor: &view::Editor) -> bool {
    let (_, doc) = current_ref!(editor);
    editor
        .workspace_trust
        .query(
            doc.workspace_root(),
            loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted()
}

#[cold]
pub(super) fn trust_workspace(
    cx: &mut compositor::Context,
    args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.trust(&workspace);

    cx.config.updates.send(ConfigEvent::Refresh)?;
    // Restart any LSPs that didn't start because trust was missing.
    lsp_restart(cx, args, event)
}

#[cold]
pub(super) fn untrust_workspace(
    cx: &mut compositor::Context,
    _args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.untrust(&workspace);
    // Drop any workspace overrides that were merged into the live editor config while trust was
    // granted. Running LSPs are not stopped here (use `:lsp-stop` for that); this only handles
    // in-memory config.
    cx.config.updates.send(ConfigEvent::Refresh)?;
    Ok(())
}

#[cold]
pub(super) fn exclude_workspace(
    cx: &mut compositor::Context,
    _args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.exclude(&workspace);
    cx.config.updates.send(ConfigEvent::Refresh)?;
    Ok(())
}

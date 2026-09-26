//! File links, file pickers, and document open/save/reload commands.

use crate::{
    commands::context::Context,
    compositor::Compositor,
    job::{self, Callback},
    ui::{self, overlay::overlaid},
};
use editor_core::{find_workspace, Range};
use event::status;
use std::{borrow::Cow, collections::HashSet, future::Future, io::Read, path::Path};
use stdx::{
    path::{self, find_paths},
    rope::RopeSliceExt,
    Url,
};
use view::{editor::Action, Editor};

pub(super) fn goto_file(cx: &mut Context) {
    goto_file_impl(cx, Action::Replace);
}

pub(super) fn goto_file_hsplit(cx: &mut Context) {
    goto_file_impl(cx, Action::HorizontalSplit);
}

pub(super) fn goto_file_vsplit(cx: &mut Context) {
    goto_file_impl(cx, Action::VerticalSplit);
}

/// Returns true when a selection overlaps an LSP document link range.
fn selection_overlaps_document_link(
    selection: &Range,
    link: &view::document::DocumentLink,
) -> bool {
    if selection.is_empty() {
        let pos = selection.from();
        link.start <= pos && pos < link.end
    } else {
        selection.from() < link.end && selection.to() > link.start
    }
}

/// Create a document link resolve request when the target isn't already present.
///
/// This only builds the LSP request. The request is awaited from a background
/// job so `goto_file_impl` does not block the UI thread while the language
/// server resolves the target.
fn resolve_document_link_request(
    editor: &Editor,
    link: &view::document::DocumentLink,
) -> Option<
    impl Future<Output = lsp_client::Result<lsp_client::lsp::DocumentLink>> + Send + 'static + use<>,
> {
    let language_server = editor.language_server_by_id(link.language_server_id)?;
    let supports_resolve = language_server
        .capabilities()
        .document_link_provider
        .as_ref()?
        .resolve_provider
        .unwrap_or(false);

    if !supports_resolve {
        return None;
    }

    language_server.resolve_document_link(link.link.clone())
}

/// Goto files/URLs in selection.
///
/// Prefers LSP document links when the cursor/selection overlaps a link range,
/// falling back to the built-in path/URL detection otherwise.
fn goto_file_impl(cx: &mut Context, action: Action) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().clone();
    let selections = doc.selection(view.id).ranges().to_vec();
    let document_dir = doc
        .path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let text = text.slice(..);

    let mut lsp_targets = Vec::new();
    let mut lsp_targets_seen = HashSet::new();
    let mut unresolved_links = HashSet::new();
    let mut resolve_requests = Vec::new();
    let mut fallback_ranges = Vec::new();

    if doc.document_links.is_empty() {
        fallback_ranges.extend_from_slice(&selections);
    } else {
        for selection in &selections {
            let mut matched = false;
            for link in &doc.document_links {
                if !selection_overlaps_document_link(selection, link) {
                    continue;
                }
                matched = true;
                if let Some(target) = link.link.target.clone() {
                    if lsp_targets_seen.insert(target.clone()) {
                        lsp_targets.push(target);
                    }
                } else if unresolved_links.insert((link.start, link.end, link.language_server_id))
                    && let Some(request) = resolve_document_link_request(cx.editor, link)
                {
                    resolve_requests.push(request);
                }
            }
            if !matched {
                fallback_ranges.push(*selection);
            }
        }
    }

    for target in lsp_targets {
        open_url(cx, target, action);
    }

    if !resolve_requests.is_empty() {
        let document_dir = document_dir.clone();
        cx.jobs.callback(async move {
            let mut targets = Vec::new();
            let mut seen = HashSet::new();

            // Resolve links off the main thread, then hand the resulting URLs
            // back to the editor/compositor callback once all requests finish.
            for request in resolve_requests {
                match request.await {
                    Ok(link) => {
                        if let Some(target) = link.target
                            && seen.insert(target.clone())
                        {
                            targets.push(target);
                        }
                    }
                    Err(err) => log::warn!("Failed to resolve document link: {err}"),
                }
            }

            Ok(Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    for target in targets {
                        open_url_in_callback(editor, compositor, target, action, &document_dir);
                    }
                },
            )))
        });
    }

    if fallback_ranges.is_empty() {
        return;
    }

    let paths: Vec<_> = if fallback_ranges.len() == 1 && fallback_ranges[0].len() == 1 {
        let selection = fallback_ranges[0];
        // Cap the search at roughly 1k bytes around the cursor.
        let lookaround = 1000;
        let pos = text.char_to_byte(selection.cursor(text));
        let search_start = text
            .line_to_byte(text.byte_to_line(pos))
            .max(text.floor_char_boundary(pos.saturating_sub(lookaround)));
        let search_end = text
            .line_to_byte(text.byte_to_line(pos) + 1)
            .min(text.ceil_char_boundary(pos + lookaround));
        let search_range = text.byte_slice(search_start..search_end);
        // we also allow paths that are next to the cursor (can be ambiguous but
        // rarely so in practice) so that gf on quoted/braced path works (not sure about this
        // but apparently that is how gf has worked historically in mitos)
        let path = find_paths(search_range, true)
            .take_while(|range| search_start + range.start <= pos + 1)
            .find(|range| pos <= search_start + range.end)
            .map(|range| Cow::from(search_range.byte_slice(range)));
        log::debug!("goto_file auto-detected path: {path:?}");
        let path = path.unwrap_or_else(|| selection.fragment(text));
        vec![path.into_owned()]
    } else {
        // Otherwise use each selection, trimmed.
        fallback_ranges
            .iter()
            .map(|range| range.fragment(text).trim().to_owned())
            .filter(|sel| !sel.is_empty())
            .collect()
    };

    for sel in paths {
        if let Ok(url) = Url::parse(&sel) {
            open_url(cx, url, action);
            continue;
        }

        let path = path::expand(&sel);
        let path = &document_dir.join(path);
        if path.is_dir() {
            let picker = ui::file_picker(cx.editor, path.into());
            cx.push_layer(Box::new(overlaid(picker)));
        } else if let Err(e) = cx.editor.open(path, action) {
            cx.editor.set_error(|| format!("Open file failed: {:?}", e));
        }
    }
}

/// Opens the given url. If the URL points to a valid textual file it is open in mitos.
/// Otherwise, the file is open using external program.
fn open_url(cx: &mut Context, url: Url, action: Action) {
    let doc = doc!(cx.editor);
    let document_dir = doc
        .path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();

    if should_open_url_externally(&url) {
        return cx.jobs.callback(crate::open_external_url_callback(url));
    }

    let path = &document_dir.join(url.path());
    if path.is_dir() {
        let picker = ui::file_picker(cx.editor, path.into());
        cx.push_layer(Box::new(overlaid(picker)));
    } else if let Err(e) = cx.editor.open(path, action) {
        cx.editor.set_error(|| format!("Open file failed: {:?}", e));
    }
}

/// Open a URL from an editor/compositor callback.
///
/// This mirrors `open_url` but does not require a full `Context`, which makes
/// it usable from async job completions such as deferred document link
/// resolves.
fn open_url_in_callback(
    editor: &mut Editor,
    compositor: &mut Compositor,
    url: Url,
    action: Action,
    rel_path: &Path,
) {
    if should_open_url_externally(&url) {
        tokio::spawn(async move {
            match crate::open_external_url_callback(url).await {
                Ok(callback) => job::dispatch_callback(callback).await,
                Err(err) => status::report(err).await,
            }
        });
        return;
    }

    let path = &rel_path.join(url.path());
    if path.is_dir() {
        let picker = ui::file_picker(editor, path.into());
        compositor.push(Box::new(overlaid(picker)));
    } else if let Err(e) = editor.open(path, action) {
        editor.set_error(|| format!("Open file failed: {:?}", e));
    }
}

/// Returns whether a URL should opened externally.
///
/// Non-`file` URLs always open externally. `file` URLs are opened externally
/// only when the target looks like a binary file (a non-textual file that can't
/// be viewed in mitos).
fn should_open_url_externally(url: &Url) -> bool {
    if url.scheme() != "file" {
        return true;
    }

    let is_binary = std::fs::File::open(url.path()).and_then(|file| {
        // Read up to 1kb to detect the content type
        let mut read_buffer = Vec::new();
        let n = file.take(1024).read_to_end(&mut read_buffer)?;
        Ok(crate::is_binary(&read_buffer[..n]))
    });

    matches!(is_binary, Ok(true))
}

pub(super) fn file_picker(cx: &mut Context) {
    let root = find_workspace().0;
    if !root.exists() {
        cx.editor.set_error(|| "Workspace directory does not exist");
        return;
    }
    let picker = ui::file_picker(cx.editor, root);
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn file_picker_in_current_buffer_directory(cx: &mut Context) {
    let doc_dir = doc!(cx.editor)
        .path()
        .and_then(|path| path.parent().map(|path| path.to_path_buf()));

    let path = match doc_dir {
        Some(path) => path,
        None => {
            let cwd = stdx::env::current_working_dir();
            if !cwd.exists() {
                cx.editor.set_error(|| {
                    "Current buffer has no parent and current working directory does not exist"
                });
                return;
            }
            cx.editor.set_error(|| {
                "Current buffer has no parent, opening file picker in current working directory"
            });
            cwd
        }
    };

    let picker = ui::file_picker(cx.editor, path);
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn file_picker_in_current_directory(cx: &mut Context) {
    let cwd = stdx::env::current_working_dir();
    if !cwd.exists() {
        cx.editor
            .set_error(|| "Current working directory does not exist");
        return;
    }
    let picker = ui::file_picker(cx.editor, cwd);
    cx.push_layer(Box::new(overlaid(picker)));
}

pub(super) fn file_explorer(cx: &mut Context) {
    let root = find_workspace().0;
    if !root.exists() {
        cx.editor.set_error(|| "Workspace directory does not exist");
        return;
    }

    if let Ok(picker) = ui::file_explorer(root, cx.editor) {
        cx.push_layer(Box::new(overlaid(picker)));
    }
}

pub(super) fn file_explorer_in_current_buffer_directory(cx: &mut Context) {
    let doc_dir = doc!(cx.editor)
        .path()
        .and_then(|path| path.parent().map(|path| path.to_path_buf()));

    let path = match doc_dir {
        Some(path) => path,
        None => {
            let cwd = stdx::env::current_working_dir();
            if !cwd.exists() {
                cx.editor.set_error(|| {
                    "Current buffer has no parent and current working directory does not exist"
                });
                return;
            }
            cx.editor.set_error(|| {
                "Current buffer has no parent, opening file explorer in current working directory"
            });
            cwd
        }
    };

    if let Ok(picker) = ui::file_explorer(path, cx.editor) {
        cx.push_layer(Box::new(overlaid(picker)));
    }
}

pub(super) fn file_explorer_in_current_directory(cx: &mut Context) {
    let cwd = stdx::env::current_working_dir();
    if !cwd.exists() {
        cx.editor
            .set_error(|| "Current working directory does not exist");
        return;
    }

    if let Ok(picker) = ui::file_explorer(cwd, cx.editor) {
        cx.push_layer(Box::new(overlaid(picker)));
    }
}

pub(super) mod typed {
    //! Typable files commands.

    use view::save::{self, PreparedSave};
    pub use view::save::{WriteAllOptions, WriteOptions};

    use crate::{
        commands::{
            buffers::typed::{buffer_close_by_ids_impl, buffer_gather_paths_impl},
            catalog::{WRITE_NO_CODE_ACTIONS_FLAG, WRITE_NO_FORMAT_FLAG},
            formatting::make_format_callback,
            lsp::code_actions_on_save,
            workspace::doc_trust_full,
        },
        compositor::{self, Compositor},
        job::{self, Callback, Job, Jobs},
        ui::{self, overlay::overlaid, PromptEvent},
    };
    use ::command_line::Args;
    use anyhow::{anyhow, bail, ensure, Context as _};
    use arc_swap::access::DynAccess;
    use editor_core::{
        encoding, graphemes,
        indent::{IndentStyle, MAX_INDENT},
        pos_at_coords, LineEnding, Selection, Tendril, Transaction,
    };
    use std::{
        fmt::Write,
        io::BufReader,
        path::{Path, PathBuf},
    };
    use view::{
        align_view,
        document::{read_to_string, DEFAULT_LANGUAGE_NAME},
        editor::Action,
        Align, DocumentId, Editor, ViewId,
    };

    #[cold]
    pub(in crate::commands) fn open(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        open_impl(cx, args, Action::Replace)
    }

    pub(in crate::commands) fn open_impl(
        cx: &mut compositor::Context,
        args: Args,
        action: Action,
    ) -> anyhow::Result<()> {
        for arg in args {
            let (path, pos) = crate::args::parse_file(&arg);
            let path = stdx::path::expand_tilde(path);
            // If the path is a directory, open a file picker on that directory and update the status
            // message
            if let Ok(true) = std::fs::canonicalize(&path).map(|p| p.is_dir()) {
                let callback = async move {
                    let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                        move |editor: &mut Editor, compositor: &mut Compositor| {
                            let picker = ui::file_picker(editor, path.into_owned())
                                .with_default_action(action);
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

    pub(in crate::commands) fn write_impl(
        cx: &mut compositor::Context,
        path: Option<&str>,
        options: WriteOptions,
    ) -> anyhow::Result<()> {
        let (view, doc) = current!(cx.editor);
        let (doc_id, view_id) = (doc.id(), view.id);
        let request = save::prepare(cx.editor, doc_id, view_id, path.map(Into::into), options);
        submit_save(cx.editor, cx.jobs, request)
    }

    fn submit_save(
        editor: &mut Editor,
        jobs: &mut Jobs,
        request: PreparedSave,
    ) -> anyhow::Result<()> {
        let PreparedSave {
            doc_id,
            view_id,
            path,
            force,
            auto_format,
            code_actions: run_code_actions,
        } = request;

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
            code_actions_on_save(editor, doc_id, tail)
        } else {
            tail
        };

        if let Some(job) = job {
            jobs.add(job);
        } else {
            editor.save(doc_id, path, force)?;
        }

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn write(
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
    pub(in crate::commands) fn force_write(
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
    pub(in crate::commands) fn write_buffer_close(
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
    pub(in crate::commands) fn force_write_buffer_close(
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
    pub(in crate::commands) fn new_file(
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
    pub(in crate::commands) fn set_indent_style(
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
    pub(in crate::commands) fn set_line_ending(
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

    pub fn write_all_impl(
        editor: &mut Editor,
        jobs: &mut Jobs,
        options: WriteAllOptions,
    ) -> anyhow::Result<()> {
        save::save_all(editor, options, |editor, request| {
            submit_save(editor, jobs, request)
        })
    }

    #[cold]
    pub(in crate::commands) fn write_all(
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
    pub(in crate::commands) fn force_write_all(
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

    /// Sets the [`view::Document`]'s encoding..
    #[cold]
    pub(in crate::commands) fn set_encoding(
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
    pub(in crate::commands) fn get_character_info(
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

                let (result, _input_bytes_read) = encoder
                    .encode_from_utf8_to_vec_without_replacement(
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

    /// Reload the [`view::Document`] from its source file.
    #[cold]
    pub(in crate::commands) fn reload(
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
    pub(in crate::commands) fn reload_all(
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

    /// Update the [`view::Document`] if it has been modified.
    #[cold]
    pub(in crate::commands) fn update(
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
    pub(in crate::commands) fn tutor(
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

    /// Change the language of the current buffer at runtime.
    #[cold]
    pub(in crate::commands) fn language(
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

    #[derive(Debug, Clone, Copy)]
    pub struct MoveBufferOptions {
        pub force: bool,
    }

    #[cold]
    pub(in crate::commands) fn move_buffer(
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
    pub(in crate::commands) fn force_move_buffer(
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
    pub(in crate::commands) fn read(
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

        let file =
            std::fs::File::open(path).map_err(|err| anyhow!("error opening file: {}", err))?;
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
}

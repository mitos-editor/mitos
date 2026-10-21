//! File links, file pickers, and document open/save/reload commands.

use crate::{
    commands::context::Context,
    job::{self, Callback},
    ui::{self, overlay::overlaid},
};
use std::io::Read;
use stdx::Url;
use view::{editor::Action, Editor};
use workspace::find_workspace;

pub(super) fn goto_file(cx: &mut Context) {
    goto_file_impl(cx, Action::Replace);
}

pub(super) fn goto_file_hsplit(cx: &mut Context) {
    goto_file_impl(cx, Action::HorizontalSplit);
}

pub(super) fn goto_file_vsplit(cx: &mut Context) {
    goto_file_impl(cx, Action::VerticalSplit);
}

/// Follow cached document links or literal paths at the current selections.
fn goto_file_impl(cx: &mut Context, action: Action) {
    use view::features::document_links::{prepare_navigation, FileNavigation};

    match prepare_navigation(cx.editor) {
        FileNavigation::Ready(update) => {
            if let Some(targets) = update.into_targets(cx.editor) {
                for target in targets {
                    if let Some(layer) =
                        open_navigation_target(cx.editor, &cx.jobs.sender(), target, action)
                    {
                        cx.push_layer(layer);
                    }
                }
            }
        }
        FileNavigation::Pending(task) => {
            let jobs = cx.jobs.sender();
            cx.jobs.callback(async move {
                let update = task.run().await;
                Ok(Callback::EditorCompositor(Box::new(
                    move |editor, compositor| {
                        let Some(targets) = update.into_targets(editor) else {
                            return;
                        };
                        for target in targets {
                            if let Some(layer) =
                                open_navigation_target(editor, &jobs, target, action)
                            {
                                compositor.push(layer);
                            }
                        }
                    },
                )))
            });
        }
    }
}

/// One opening policy for immediate targets and queued link resolutions.
fn open_navigation_target(
    editor: &mut Editor,
    jobs: &job::JobSender,
    target: view::features::document_links::NavigationTarget,
    action: Action,
) -> Option<Box<dyn crate::compositor::Component>> {
    use view::features::document_links::NavigationTarget;

    let (path, open_binary_externally) = match target {
        NavigationTarget::Path(path) => (path, false),
        NavigationTarget::Url(url) if url.scheme() == "file" => match url.to_file_path() {
            Ok(path) => (path, true),
            Err(()) => {
                editor.set_error(|| format!("Invalid file URL: {url}"));
                return None;
            }
        },
        NavigationTarget::Url(url) => {
            launch_url(jobs, url);
            return None;
        }
    };
    if path.is_dir() {
        return Some(Box::new(overlaid(ui::file_picker(editor, path))));
    }
    let binary = open_binary_externally
        && std::fs::File::open(&path)
            .and_then(|file| {
                let mut prefix = Vec::new();
                file.take(1024).read_to_end(&mut prefix)?;
                Ok(crate::is_binary(&prefix))
            })
            .unwrap_or(false);
    if binary {
        match Url::from_file_path(stdx::path::canonicalize(&path)) {
            Ok(url) => launch_url(jobs, url),
            Err(()) => editor.set_error(|| format!("Invalid file path: {}", path.display())),
        }
    } else if let Err(error) = editor.open(&path, action) {
        editor.set_error(|| format!("Open file failed: {error}"));
    }
    None
}

fn launch_url(jobs: &job::JobSender, url: Url) {
    let jobs = jobs.clone();
    tokio::spawn(async move {
        match crate::open_external_url_callback(url).await {
            Ok(callback) => jobs.send_callback(callback).await,
            Err(error) => {
                jobs.send(move |editor, _| editor.set_error(|| error.to_string()))
                    .await
            }
        }
    });
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

    use stdx::rope::RopeExt;
    use view::save::{self, PreparedSave};
    use view::save::{WriteAllOptions, WriteOptions};

    use crate::{
        commands::{
            buffers::typed::{buffer_close_by_ids_impl, buffer_gather_paths_impl},
            catalog::{WRITE_NO_CODE_ACTIONS_FLAG, WRITE_NO_FORMAT_FLAG},
        },
        compositor::{self, Compositor},
        job::{self, Callback, Job, Jobs},
        ui::{self, overlay::overlaid, PromptEvent},
    };
    use ::command_line::Args;
    use anyhow::{anyhow, bail, ensure, Context as _};
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
    use text_io::read_to_string;
    use view::{align_view, document::DEFAULT_LANGUAGE_NAME, editor::Action, Align, Editor};

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
        let request = save::prepare(
            cx.editor,
            doc_id,
            Some(view_id),
            path.map(Into::into),
            options,
        )?;
        submit_save(cx.editor, cx.jobs, request)
    }

    fn submit_save(
        editor: &mut Editor,
        jobs: &mut Jobs,
        request: PreparedSave,
    ) -> anyhow::Result<()> {
        if let save::SaveProgress::Pending(task) = save::submit(editor, request)? {
            jobs.add(save_job(task));
        }
        Ok(())
    }

    fn save_job(task: save::SaveTask) -> Job {
        Job::with_callback(async move {
            let update = task.run().await?;
            Ok(Callback::Followup(Box::new(move |editor| {
                match update.apply(editor) {
                    Ok(save::SaveProgress::Pending(task)) => Some(save_job(task)),
                    Ok(_) => None,
                    Err(error) => {
                        editor.set_error(|| format!("Error saving: {error}"));
                        None
                    }
                }
            })))
        })
        .wait_before_exiting()
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

        let mut pos = 0;
        let transaction = Transaction::change(
            doc.text(),
            doc.text().lines(stdx::rope::LINE_TYPE).filter_map(|line| {
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
        view::editing::apply(doc, &transaction, view.id)?;
        doc.line_ending = line_ending;
        doc.commit_changes();
        view.sync_changes(doc);

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

        let grapheme = text.slice_chars(grapheme_start..grapheme_end).to_string();
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

        let id = current_ref!(cx.editor).1.id();
        cx.editor.reload_document(id)?;
        let doc = doc!(cx.editor, &id);
        if let Some(path) = doc.path().map(ToOwned::to_owned)
            && !cx.editor.file_watcher.is_watching(&path)
        {
            cx.editor
                .language_servers
                .file_event_handler
                .file_changed(path, lsp_client::lsp::FileChangeType::CHANGED);
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

        let documents: Vec<_> = cx.editor.documents.keys().copied().collect();
        for id in documents {
            if let Err(error) = cx.editor.reload_document(id) {
                cx.editor.set_error(|| error.to_string());
                continue;
            }
            if let Some(path) = doc!(cx.editor, &id).path().map(ToOwned::to_owned)
                && !cx.editor.file_watcher.is_watching(&path)
            {
                cx.editor
                    .language_servers
                    .file_event_handler
                    .file_changed(path, lsp_client::lsp::FileChangeType::CHANGED);
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
        cx.editor.open_scratch(&path, Action::Replace)?;
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

        let id = doc!(cx.editor).id();
        let language = (&args[0] != DEFAULT_LANGUAGE_NAME).then_some(args[0].as_ref());
        cx.editor.set_document_language(id, language)?;
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

        if let Err(err) = cx
            .editor
            .request_file_operation(view::editor::FileOperation::Move {
                from: old_path,
                to: new_path,
            })
        {
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

        let scrolloff = cx.editor.config().scroll.padding;
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
        view::editing::apply(doc, &transaction, view.id)?;
        doc.commit_changes();
        view.sync_changes(doc);
        view.ensure_cursor_in_view(doc, scrolloff);

        Ok(())
    }
}

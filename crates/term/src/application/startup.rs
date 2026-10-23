//! Construct the frontend, bind editor services, and open initial documents.
#[cfg(not(feature = "integration"))]
use super::configuration::terminal_config;
#[cfg(all(windows, not(feature = "integration")))]
use super::CrosstermBackend;
#[cfg(not(windows))]
use super::Signals;
#[cfg(all(not(windows), not(feature = "integration")))]
use super::TerminaBackend;
#[cfg(feature = "integration")]
use super::TestBackend;
use super::{render, Application, Terminal};
use crate::{
    args::Args,
    compositor::Compositor,
    config::Config,
    handlers,
    job::Jobs,
    keymap::Keymaps,
    ui::{self, overlay::overlaid},
};
#[cfg(not(windows))]
use anyhow::Context as _;
use anyhow::Error;
use arc_swap::{access::Map, ArcSwap};
use editor_core::{pos_at_coords, Range};
use language::syntax;
#[cfg(not(windows))]
use signal_hook::consts::signal;
use std::{
    io::{stdin, IsTerminal},
    path::Path,
    sync::Arc,
};
use tui::backend::BackendExt;
use view::{align_view, document::DocumentOpenError, graphics::Rect, tree::Layout, Align, Editor};

impl Application {
    pub fn new(
        args: Args,
        config: Config,
        lang_loader: syntax::Loader,
        workspace_trust: workspace::trust::WorkspaceTrust,
    ) -> Result<Self, Error> {
        Self::new_with_handler_setup(args, config, lang_loader, workspace_trust, |_, _| {})
    }

    /// Configure shared services before the editor owns them or opens any documents.
    pub fn new_with_handler_setup(
        args: Args,
        config: Config,
        lang_loader: syntax::Loader,
        workspace_trust: workspace::trust::WorkspaceTrust,
        setup: impl FnOnce(&mut view::handlers::Handlers, &editor_config::Config),
    ) -> Result<Self, Error> {
        #[cfg(feature = "integration")]
        setup_integration_logging();

        use view::editor::Action;

        let mut theme_parent_dirs = vec![loader::config_dir()];
        theme_parent_dirs.extend(loader::runtime_dirs().iter().cloned());
        let theme_loader = theme::Loader::new(loader::theme::Resources::new(theme_parent_dirs));

        #[cfg(all(not(windows), not(feature = "integration")))]
        let backend = TerminaBackend::new(terminal_config(&config))
            .context("failed to create terminal backend")?;
        #[cfg(all(windows, not(feature = "integration")))]
        let backend = CrosstermBackend::new(std::io::stdout(), terminal_config(&config));

        #[cfg(feature = "integration")]
        let backend = TestBackend::new(120, 150);

        let theme_mode = backend.get_theme_mode();
        let terminal = Terminal::new(backend)?;
        let area = Rect::from(terminal.size()?);
        let mut compositor = Compositor::new(area);
        let config = Arc::new(ArcSwap::from_pointee(config));
        let jobs = Jobs::new();
        let mut handlers =
            view::handlers::Handlers::new(&config.load().editor, jobs.editor_callback_sender());
        setup(&mut handlers, &config.load().editor);
        handlers::register_hooks();
        let mut editor = Editor::new(
            area,
            Arc::new(theme_loader),
            Arc::new(ArcSwap::from_pointee(lang_loader)),
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| {
                &config.editor
            })),
            handlers,
            workspace_trust,
        );
        let presentation_hooks =
            handlers::bind_editor(&editor, jobs.sender(), compositor.prompt_sessions());
        editor
            .registers
            .set_clipboard_backend(Box::new(crate::clipboard::TerminalClipboard));
        let theme = Self::configured_theme(
            &editor,
            &config.load(),
            &terminal,
            theme_mode,
            args.theme_file.as_deref(),
        );
        let _ = editor.set_theme(theme);
        editor.debugger_mut().set_terminal_launcher(
            crate::terminal_launcher::ExternalTerminal::new(config.clone())?,
        )?;

        let keys = Box::new(Map::new(Arc::clone(&config), |config: &Config| {
            &config.keys
        }));
        let editor_view = Box::new(ui::EditorView::new(Keymaps::new(keys)));
        compositor.push(editor_view);

        if args.load_tutor {
            let path = loader::runtime_file(Path::new("tutor"));
            editor.open_scratch(&path, Action::VerticalSplit)?;
        } else if !args.files.is_empty() {
            let mut files_it = args.files.into_iter().peekable();

            // If the first file is a directory, skip it and open a picker
            if let Some((first, _)) = files_it.next_if(|(p, _)| p.is_dir()) {
                let picker = ui::file_picker(&editor, first);
                compositor.push(Box::new(overlaid(picker)));
            }

            // If there are any more files specified, open them
            if files_it.peek().is_some() {
                let mut nr_of_files = 0;
                for (file, pos) in files_it {
                    nr_of_files += 1;
                    if file.is_dir() {
                        return Err(anyhow::anyhow!(
                            "expected a path to file, but found a directory: {file:?}. (to open a directory pass it as first argument)"
                        ));
                    } else {
                        // If the user passes in either `--vsplit` or
                        // `--hsplit` as a command line argument, all the given
                        // files will be opened according to the selected
                        // option. If neither of those two arguments are passed
                        // in, just load the files normally.
                        let action = match args.split {
                            _ if nr_of_files == 1 => Action::VerticalSplit,
                            Some(Layout::Vertical) => Action::VerticalSplit,
                            Some(Layout::Horizontal) => Action::HorizontalSplit,
                            None => Action::Load,
                        };
                        let old_id = editor.document_id_by_path(&file);
                        let doc_id = match editor.open(&file, action) {
                            // Ignore irregular files during application init.
                            Err(DocumentOpenError::IrregularFile) => {
                                nr_of_files -= 1;
                                continue;
                            }
                            Err(err) => return Err(anyhow::anyhow!(err)),
                            // We can't open more than 1 buffer for 1 file, in this case we already have opened this file previously
                            Ok(doc_id) if old_id == Some(doc_id) => {
                                nr_of_files -= 1;
                                doc_id
                            }
                            Ok(doc_id) => doc_id,
                        };
                        // with Action::Load all documents have the same view
                        // NOTE: this isn't necessarily true anymore. If
                        // `--vsplit` or `--hsplit` are used, the file which is
                        // opened last is focused on.
                        let view_id = current_ref!(editor).0.id;
                        let doc = doc_mut!(editor, &doc_id);
                        let selection = pos
                            .into_iter()
                            .map(|coords| {
                                Range::point(pos_at_coords(doc.text().slice(..), coords, true))
                            })
                            .collect();
                        doc.set_selection(view_id, selection);
                    }
                }

                // if all files were invalid, replace with empty buffer
                if nr_of_files == 0 {
                    editor.new_file(Action::VerticalSplit);
                } else {
                    editor.set_status(format!(
                        "Loaded {} file{}.",
                        nr_of_files,
                        if nr_of_files == 1 { "" } else { "s" } // avoid "Loaded 1 files." grammo
                    ));
                    // align the view to center after all files are loaded,
                    // does not affect views without pos since it is at the top
                    let (view, doc) = current!(editor);
                    align_view(doc, view, Align::Center);
                }
            } else {
                editor.new_file(Action::VerticalSplit);
            }
        } else if stdin().is_terminal() || cfg!(feature = "integration") {
            editor.new_file_welcome();
        } else {
            editor
                .new_file_from_stdin(Action::VerticalSplit)
                .unwrap_or_else(|_| editor.new_file_welcome());
        }

        #[cfg(windows)]
        let signals = futures_util::stream::empty();
        #[cfg(not(windows))]
        let signals = Signals::new([
            signal::SIGTSTP,
            signal::SIGCONT,
            signal::SIGUSR1,
            signal::SIGTERM,
            signal::SIGINT,
        ])
        .context("build signal handler")?;

        let app = Self {
            compositor,
            terminal,
            render_state: render::RenderState::default(),
            editor,
            image_picker: None,
            config,
            config_updates: tokio::sync::mpsc::unbounded_channel(),
            signals,
            jobs,
            _presentation_hooks: presentation_hooks,
            theme_mode,
            theme_file: args.theme_file,
        };

        Ok(app)
    }
}

#[cfg(feature = "integration")]
fn setup_integration_logging() {
    let level = std::env::var("MITOS_LOG_LEVEL")
        .map(|lvl| lvl.parse().unwrap())
        .unwrap_or(log::LevelFilter::Info);

    crate::logging::init_stdout(level);
}

pub mod blame;
mod bufferline;
pub mod change_review;
mod completion;
pub(crate) mod debugger_stack;
pub(crate) mod debugger_variables;
mod document;
pub(crate) mod editor;
mod file_explorer;
mod gutter;
pub(crate) mod highlights;
mod icons;
pub(crate) mod image;
mod info;
mod layout;
mod list;
pub mod lsp;
mod markdown;
pub mod menu;
pub mod overlay;
mod panel;
pub mod picker;
pub mod popup;
pub mod prompt;
mod select;
mod sidebar;
mod spinner;
mod statusline;
mod text;
mod text_decorations;

use crate::job::Callback;
use crate::ui::completers::CompletionResult;
pub use completion::Completion;
pub use editor::EditorView;
pub(crate) use icons::icon_span;
pub use markdown::Markdown;
pub use menu::Menu;
pub use panel::PopupBorder;
pub use picker::{Column as PickerColumn, FileLocation, Picker};
pub use popup::Popup;
pub use prompt::{Prompt, PromptEvent};
pub use select::Select;
pub use spinner::{ProgressSpinners, Spinner};
use stdx::rope;
pub use text::Text;
use theme::icons::ICONS;
use theme::Theme;
use view::{graphics::RectExt as _, Editor};

use tui::text::{Line, Span};

use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
};

struct Utf8PathBuf {
    path: String,
    is_dir: bool,
    is_symlink: bool,
}

impl AsRef<str> for Utf8PathBuf {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

pub fn prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    callback_fn: impl FnMut(&mut crate::compositor::Context, &str, PromptEvent) + 'static,
) {
    let mut prompt = Prompt::new(prompt, history_register, completion_fn, callback_fn);
    // Calculate the initial completion
    prompt.recalculate_completion(cx.editor);
    cx.push_layer(Box::new(prompt));
}

pub fn regex_prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, rope::Regex, PromptEvent) + 'static,
) {
    raw_regex_prompt(
        cx,
        prompt,
        history_register,
        completion_fn,
        move |cx, regex, _, event| fun(cx, regex, event),
    );
}
pub fn raw_regex_prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, rope::Regex, &str, PromptEvent) + 'static,
) {
    search_prompt_impl(
        cx,
        false,
        prompt,
        history_register,
        completion_fn,
        move |cx, query, input, event| {
            fun(cx, query.into_regex(), input, event);
        },
    );
}

/// The shared regex prompt retains query identity for buffer-search statistics.
pub fn search_prompt(
    cx: &mut crate::commands::Context,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, view::features::buffer_search::Query, &str, PromptEvent)
        + 'static,
) {
    search_prompt_impl(cx, true, prompt, history_register, completion_fn, fun);
}

fn search_prompt_impl(
    cx: &mut crate::commands::Context,
    highlight_search: bool,
    prompt: std::borrow::Cow<'static, str>,
    history_register: Option<char>,
    completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
    fun: impl Fn(&mut crate::compositor::Context, view::features::buffer_search::Query, &str, PromptEvent)
        + 'static,
) {
    let (view, doc) = current!(cx.editor);
    let doc_id = view.document_id();
    let view_id = view.id;
    let snapshot = doc.selection(view.id).clone();
    let highlight_snapshot = doc.search_highlight(view.id).cloned();
    let highlight_version = doc.version();
    doc.begin_selection_preview(view.id);
    let offset_snapshot = doc.view_offset(view.id);
    let config = cx.editor.config();

    let mut prompt = Prompt::new(
        prompt,
        history_register,
        completion_fn,
        move |cx: &mut crate::compositor::Context, input: &str, event: PromptEvent| {
            match event {
                PromptEvent::Abort => {
                    let doc = doc_mut!(cx.editor, &doc_id);
                    let view = view_mut!(cx.editor, view_id);
                    doc.set_selection(view.id, snapshot.clone());
                    doc.set_view_offset(view.id, offset_snapshot);
                    doc.finish_selection_preview(view.id);
                    if highlight_search {
                        let query = (doc.version() == highlight_version)
                            .then(|| highlight_snapshot.clone())
                            .flatten();
                        doc.set_search_highlight(view.id, query);
                    }
                }
                PromptEvent::Update | PromptEvent::Validate => {
                    if input.is_empty() {
                        let doc = doc_mut!(cx.editor, &doc_id);
                        if highlight_search {
                            doc.set_search_highlight(view_id, None);
                        }
                        doc.set_selection(view_id, snapshot.clone());
                        doc.set_view_offset(view_id, offset_snapshot);
                        if event == PromptEvent::Validate {
                            doc.finish_selection_preview(view_id);
                        }
                        return;
                    }

                    let is_crlf = doc!(cx.editor).line_ending == editor_core::LineEnding::Crlf;
                    match view::features::buffer_search::Query::new(
                        input,
                        config.search.smart_case,
                        is_crlf,
                    ) {
                        Ok(query) => {
                            let doc = doc_mut!(cx.editor, &doc_id);
                            let view = view_mut!(cx.editor, view_id);

                            // revert state to what it was before the last update
                            doc.set_selection(view.id, snapshot.clone());

                            if event == PromptEvent::Validate {
                                // Equivalent to push_jump to store selection just before jump
                                view.push_jump(doc, (doc_id, snapshot.clone()));
                            }

                            fun(cx, query, input, event);

                            let (view, doc) = current!(cx.editor);
                            view.ensure_cursor_in_view(doc, config.scroll.padding);
                        }
                        Err(err) => {
                            let doc = doc_mut!(cx.editor, &doc_id);
                            let view = view_mut!(cx.editor, view_id);
                            if highlight_search {
                                doc.set_search_highlight(view.id, None);
                            }
                            doc.set_selection(view.id, snapshot.clone());
                            doc.set_view_offset(view.id, offset_snapshot);

                            if event == PromptEvent::Validate {
                                cx.jobs
                                    .sender()
                                    .enqueue(Callback::ui(move |compositor, cx| {
                                        let editor = &mut *cx.editor;
                                        let contents = Text::new(err.root_cause().to_string());
                                        let size = compositor.size();
                                        let editor_area = size.clip_bottom(
                                            1 + editor.config().command_line.reserved_height(),
                                        );
                                        let popup = Popup::new("invalid-regex", contents)
                                            .with_border(panel::PopupBorder::Search)
                                            .position(Some(text_layout::DisplayPosition::new(
                                                editor_area.bottom().saturating_sub(1) as usize,
                                                0,
                                            )))
                                            .auto_close(true);
                                        compositor.replace_or_push("invalid-regex", popup);
                                    }));
                            }
                        }
                    }
                    if event == PromptEvent::Validate {
                        doc_mut!(cx.editor, &doc_id).finish_selection_preview(view_id);
                    }
                }
            }
        },
    )
    .with_language("regex", std::sync::Arc::clone(&cx.editor.syn_loader));
    // Calculate initial completion
    prompt.recalculate_completion(cx.editor);
    // prompt
    cx.push_layer(Box::new(prompt));
}

#[derive(Debug)]
pub struct FilePickerData {
    root: PathBuf,
    theme: Arc<Theme>,
    icons: bool,
    files: Arc<std::sync::Mutex<crate::files::FilePickerCache>>,
}
type FilePicker = Picker<PathBuf, FilePickerData>;

pub fn file_picker(editor: &Editor, root: PathBuf) -> FilePicker {
    let config = editor.config();
    let files = Arc::new(std::sync::Mutex::new(crate::files::FilePickerCache::new(
        editor,
    )));
    let data = FilePickerData {
        root: root.clone(),
        theme: Arc::new(editor.theme.clone()),
        icons: config.icons,
        files: files.clone(),
    };
    let initial_config = config.file_picker.clone();

    let columns = [
        PickerColumn::new("path", |item: &PathBuf, data: &FilePickerData| {
            let theme = data.theme.as_ref();
            let path = item.strip_prefix(&data.root).unwrap_or(item);
            let mut spans = Vec::with_capacity(4);

            if let Some(dirs) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let directory_style = theme.get("ui.text.directory");
                spans.extend([
                    Span::styled(dirs.to_string_lossy(), directory_style),
                    Span::styled(std::path::MAIN_SEPARATOR_STR, directory_style),
                ]);
            }
            let filename = path
                .file_name()
                .expect("normalized paths can't end in `..`")
                .to_string_lossy();
            spans.push(Span::raw(filename));
            Line::from(spans).into()
        })
        .with_prefix(|item, data| {
            data.icons.then(|| ICONS.fs().file()).map(|file| {
                icon_span(
                    file.get_with_style_or_default(item, &data.theme)
                        .with_no_left_padding(),
                )
            })
        }),
    ];
    let picker = Picker::new(
        editor.redraw().clone(),
        columns,
        0,
        [],
        data,
        move |cx, path: &PathBuf, action| {
            if let Err(err) = cx.editor.open(path, action) {
                cx.editor.set_error(|| {
                    if let Some(err) = err.source() {
                        format!("{}", err)
                    } else {
                        format!("unable to open \"{}\"", path.display())
                    }
                });
            }
        },
    )
    .with_title("Files")
    .with_preview(|_editor, path| Some((path.as_path().into(), None)))
    .with_file_updates(
        editor,
        |data| &data.root,
        |_, editor, data, cancel| {
            let config = editor.config();
            let poll = &config.files.reload.poll;
            let interval = poll
                .enable
                .then(|| std::time::Duration::from_millis(poll.interval.max(100)));
            let config = config.file_picker.clone();
            Box::pin(async move {
                Ok(tokio::task::spawn_blocking(move || {
                    data.files
                        .lock()
                        .unwrap()
                        .refresh(&data.root, &config, interval, || cancel.is_canceled())
                })
                .await?)
            })
        },
    )
    .with_item_key(|path| path.clone());
    let injector = picker.injector();
    tokio::task::spawn_blocking(move || {
        let complete = files.lock().unwrap().initial(
            &root,
            &initial_config,
            || injector.is_shutdown(),
            |path| injector.push(path.to_owned()).is_ok(),
        );
        if complete {
            injector.complete();
        } else {
            injector.pause();
        }
    });
    picker
}

type FileExplorer = Picker<file_explorer::ExplorerItem, file_explorer::ExplorerData>;

pub fn file_explorer(root: PathBuf, editor: &Editor) -> Result<FileExplorer, std::io::Error> {
    file_explorer::picker(root, editor, false)
}

fn directory_content(
    root: &Path,
    config: &editor_config::FileExplorerConfig,
) -> std::io::Result<Vec<(PathBuf, bool)>> {
    file_explorer::directory_content(root, config)
}

pub mod completers {
    use super::Utf8PathBuf;
    use crate::config::EditorSettings;
    use crate::ui::prompt::Completion;
    use command_line::{self, Tokenizer};
    use event::TaskHandle;
    use fuzzy::{fuzzy_match, MatchKind};
    use language_config::LanguageServerFeature;
    use std::borrow::Cow;
    use std::collections::BTreeSet;
    use std::sync::LazyLock;
    use tui::text::Span;
    use view::graphics::Style;
    use view::Editor;

    /// A completion computation to be run on a background thread.
    ///
    /// The closure is handed a [TaskHandle] and should check [TaskHandle::is_canceled] at
    /// convenient intervals, bailing out early when the results are no longer needed (i.e.
    /// when the prompt line has changed or the prompt was closed).
    pub type DeferredCompletion = Box<dyn FnOnce(&TaskHandle) -> Vec<Completion> + Send>;

    /// The result of invoking a [Completer].
    ///
    /// Completers which read only in-memory editor state complete immediately. Completers which need
    /// blocking I/O (i.e. :cd, :open, etc...) return a `Deferred` closure instead.
    pub enum CompletionResult {
        Immediate(Vec<Completion>),
        Deferred(DeferredCompletion),
    }

    fn deferred(
        f: impl FnOnce(&TaskHandle) -> Vec<Completion> + Send + 'static,
    ) -> CompletionResult {
        CompletionResult::Deferred(Box::new(f))
    }

    impl CompletionResult {
        /// Applies a transformation to the completions, either immediately or, for deferred
        /// completions, on the background thread once they have been computed.
        pub fn map(
            self,
            f: impl FnOnce(Vec<Completion>) -> Vec<Completion> + Send + 'static,
        ) -> CompletionResult {
            match self {
                CompletionResult::Immediate(completions) => {
                    CompletionResult::Immediate(f(completions))
                }
                CompletionResult::Deferred(g) => deferred(move |handle| f(g(handle))),
            }
        }
    }

    impl FromIterator<Completion> for CompletionResult {
        fn from_iter<T: IntoIterator<Item = Completion>>(items: T) -> Self {
            Self::Immediate(items.into_iter().collect())
        }
    }

    pub type Completer = fn(&Editor, &str) -> CompletionResult;

    pub fn none(_editor: &Editor, _input: &str) -> CompletionResult {
        CompletionResult::Immediate(Vec::new())
    }

    pub fn diff_comparison(_editor: &Editor, input: &str) -> CompletionResult {
        fuzzy_match(
            input,
            ["head", "staged", "unstaged", "buffer"],
            MatchKind::Text,
        )
        .into_iter()
        .map(|(name, _)| ((0..), Span::raw(name)))
        .collect()
    }

    pub fn buffer(editor: &Editor, input: &str) -> CompletionResult {
        let names = editor
            .documents
            .values()
            .map(|doc| doc.display_name().into_owned());

        fuzzy_match(input, names, MatchKind::Path)
            .into_iter()
            .map(|(name, _)| ((0..), name.into()))
            .collect()
    }

    pub fn theme(editor: &Editor, input: &str) -> CompletionResult {
        // Discovering theme names reads every theme directory, which performs blocking I/O.
        let resources = editor.theme_loader.resources().clone();
        let input = String::from(input);
        deferred(move |_| {
            fuzzy_match(&input, resources.names(), MatchKind::Text)
                .into_iter()
                .map(|(name, _)| ((0..), name.into()))
                .collect()
        })
    }

    /// Recursive function to get all keys from this value and add them to vec
    fn get_keys(value: &serde_json::Value, vec: &mut Vec<String>, scope: Option<&str>) {
        if let Some(map) = value.as_object() {
            for (key, value) in map.iter() {
                let key = match scope {
                    Some(scope) => format!("{}.{}", scope, key),
                    None => key.clone(),
                };
                get_keys(value, vec, Some(&key));
                if !value.is_object() {
                    vec.push(key);
                }
            }
        }
    }

    /// Completes names of language servers which are running for the current document.
    pub fn active_language_servers(editor: &Editor, input: &str) -> CompletionResult {
        let language_servers = doc!(editor).language_servers().map(|ls| ls.name());

        fuzzy_match(input, language_servers, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name.to_string())))
            .collect()
    }

    /// Completes names of language servers which are configured for the language of the current
    /// document.
    pub fn configured_language_servers(editor: &Editor, input: &str) -> CompletionResult {
        let language_servers = doc!(editor)
            .language_config()
            .into_iter()
            .flat_map(|config| &config.language_servers)
            .map(|ls| ls.name.as_str());

        fuzzy_match(input, language_servers, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name.to_string())))
            .collect()
    }

    pub fn setting(_editor: &Editor, input: &str) -> CompletionResult {
        static KEYS: LazyLock<Vec<String>> = LazyLock::new(|| {
            let mut keys = Vec::new();
            let json = serde_json::json!(EditorSettings::default());
            get_keys(&json, &mut keys, None);
            keys
        });

        fuzzy_match(input, &*KEYS, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), Span::raw(name)))
            .collect()
    }

    pub fn filename(editor: &Editor, input: &str) -> CompletionResult {
        filename_with_git_ignore(editor, input, true)
    }

    pub fn filename_with_git_ignore(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
    ) -> CompletionResult {
        filename_impl(editor, input, git_ignore, |entry| {
            if entry.path().is_dir() {
                FileMatch::AcceptIncomplete
            } else {
                FileMatch::Accept
            }
        })
    }

    pub fn language(editor: &Editor, input: &str) -> CompletionResult {
        let text: String = "text".into();

        let loader = editor.syn_loader.load();
        let language_ids = loader
            .language_configs()
            .map(|config| &config.language_id)
            .chain(std::iter::once(&text));

        fuzzy_match(input, language_ids, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), name.to_owned().into()))
            .collect()
    }

    pub fn spelling_language(_editor: &Editor, input: &str) -> CompletionResult {
        // The available languages are the dictionaries vendored or dropped into the runtime
        // directories, plus `off` to disable spell-checking for the buffer.
        // Deferred because listing those directories performs blocking I/O.
        let input = String::from(input);
        deferred(move |_| {
            let mut languages = Vec::new();
            for runtime_dir in loader::runtime_dirs() {
                let Ok(entries) = std::fs::read_dir(runtime_dir.join("dictionaries")) else {
                    continue;
                };
                for entry in entries.flatten() {
                    if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                        continue;
                    }
                    if let Ok(language) = entry.file_name().into_string()
                        && !languages.contains(&language)
                    {
                        languages.push(language);
                    }
                }
            }
            languages.sort();

            let candidates = languages
                .iter()
                .map(String::as_str)
                .chain(std::iter::once("off"));
            fuzzy_match(&input, candidates, MatchKind::Text)
                .into_iter()
                .map(|(name, _)| ((0..), name.to_owned().into()))
                .collect()
        })
    }

    pub fn lsp_workspace_command(editor: &Editor, input: &str) -> CompletionResult {
        let commands = doc!(editor)
            .language_servers_with_feature(LanguageServerFeature::WorkspaceCommand)
            .flat_map(|ls| {
                ls.capabilities()
                    .execute_command_provider
                    .iter()
                    .flat_map(|options| options.commands.iter())
            });

        fuzzy_match(input, commands, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), name.to_owned().into()))
            .collect()
    }

    pub fn directory(editor: &Editor, input: &str) -> CompletionResult {
        directory_with_git_ignore(editor, input, true)
    }

    pub fn directory_with_git_ignore(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
    ) -> CompletionResult {
        filename_impl(editor, input, git_ignore, |entry| {
            if entry.path().is_dir() {
                FileMatch::Accept
            } else {
                FileMatch::Reject
            }
        })
    }

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum FileMatch {
        /// Entry should be ignored
        Reject,
        /// Entry is usable but can't be the end (for instance if the entry is a directory and we
        /// try to match a file)
        AcceptIncomplete,
        /// Entry is usable and can be the end of the match
        Accept,
    }

    fn filename_impl<F>(
        editor: &Editor,
        input: &str,
        git_ignore: bool,
        filter_fn: F,
    ) -> CompletionResult
    where
        F: Fn(&ignore::DirEntry) -> FileMatch + Send + 'static,
    {
        // Walking the directory performs blocking I/O, which can be slow on network
        // filesystems. Defer it so that the prompt stays responsive. Only the values below are
        // needed from the editor: everything else can move to the background thread.
        let input = String::from(input);
        let directory_style = editor.theme.get("ui.text.directory");
        let symlink_style = editor.theme.get("ui.text.symlink");
        let require_git = editor.config().file_picker.require_git;
        deferred(move |handle| {
            filename_completions(
                handle,
                &input,
                git_ignore,
                require_git,
                directory_style,
                symlink_style,
                filter_fn,
            )
        })
    }

    // TODO: we could return an iter/lazy thing so it can fetch as many as it needs.
    fn filename_completions<F>(
        handle: &TaskHandle,
        input: &str,
        git_ignore: bool,
        require_git: bool,
        directory_style: Style,
        symlink_style: Style,
        filter_fn: F,
    ) -> Vec<Completion>
    where
        F: Fn(&ignore::DirEntry) -> FileMatch,
    {
        // Rust's filename handling is really annoying.

        use ignore::WalkBuilder;
        use std::path::Path;

        let is_tilde = input == "~";
        let path = stdx::path::expand_tilde(Path::new(input));

        let (dir, file_name) = if input.ends_with(std::path::MAIN_SEPARATOR) {
            (path, None)
        } else {
            let is_period = (input.ends_with((format!("{}.", std::path::MAIN_SEPARATOR)).as_str())
                && input.len() > 2)
                || input == ".";
            let file_name = if is_period {
                Some(String::from("."))
            } else {
                path.file_name()
                    .and_then(|file| file.to_str().map(|path| path.to_owned()))
            };

            let path = if is_period {
                path
            } else {
                match path.parent() {
                    Some(path) if !path.as_os_str().is_empty() => Cow::Borrowed(path),
                    // Path::new("h")'s parent is Some("")...
                    _ => Cow::Owned(stdx::env::current_working_dir()),
                }
            };

            (path, file_name)
        };

        let end = input.len()..;

        let files = WalkBuilder::new(&dir)
            .hidden(false)
            .follow_links(false) // We're scanning over depth 1
            .require_git(require_git)
            .git_ignore(git_ignore)
            .max_depth(Some(1))
            .build()
            // Stop walking early if these completions are no longer needed. Partial results
            // are discarded by the prompt when this request becomes obsolete.
            .take_while(|_| !handle.is_canceled())
            .filter_map(|file| {
                file.ok().and_then(|entry| {
                    let fmatch = filter_fn(&entry);

                    if fmatch == FileMatch::Reject {
                        return None;
                    }

                    let path = entry.path();
                    let is_dir = search::entry_file_type(&entry).is_some_and(|kind| kind.is_dir());
                    let file_type = entry.file_type();
                    let is_symlink = file_type.is_some_and(|ft| ft.is_symlink());
                    let mut path = if is_tilde {
                        // if it's a single tilde an absolute path is displayed so that when `TAB` is pressed on
                        // one of the directories the tilde will be replaced with a valid path not with a relative
                        // home directory name.
                        // ~ -> <TAB> -> /home/user
                        // ~/ -> <TAB> -> ~/first_entry
                        path.to_path_buf()
                    } else {
                        path.strip_prefix(&dir).unwrap_or(path).to_path_buf()
                    };

                    if fmatch == FileMatch::AcceptIncomplete {
                        path.push("");
                    }

                    let path = path.into_os_string().into_string().ok()?;
                    Some(Utf8PathBuf {
                        path,
                        is_dir,
                        is_symlink,
                    })
                })
            }) // TODO: unwrap or skip
            .filter(|path| !path.path.is_empty());

        let style_from_file = |file: Utf8PathBuf| {
            if file.is_symlink {
                Span::styled(file.path, symlink_style)
            } else if file.is_dir {
                Span::styled(file.path, directory_style)
            } else {
                Span::raw(file.path)
            }
        };

        // if empty, return a list of dirs and files in current dir
        if let Some(file_name) = file_name {
            let range = (input.len().saturating_sub(file_name.len()))..;
            fuzzy_match(&file_name, files, MatchKind::Path)
                .into_iter()
                .map(|(name, _)| (range.clone(), style_from_file(name)))
                .collect()

            // TODO: complete to longest common match
        } else {
            let mut files: Vec<_> = files
                .map(|file| (end.clone(), style_from_file(file)))
                .collect();
            files.sort_unstable_by(|(_, path1), (_, path2)| path1.content.cmp(&path2.content));
            files
        }
    }

    pub fn register(editor: &Editor, input: &str) -> CompletionResult {
        let iter = editor
            .registers
            .iter_preview()
            // Exclude special registers that shouldn't be written to
            .filter(|(ch, _)| !matches!(ch, '%' | '#' | '.'))
            .map(|(ch, _)| ch.to_string());

        fuzzy_match(input, iter, MatchKind::Text)
            .into_iter()
            .map(|(name, _)| ((0..), name.into()))
            .collect()
    }

    pub fn program(_editor: &Editor, input: &str) -> CompletionResult {
        static PROGRAMS_IN_PATH: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
            // Go through the entire PATH and read all files into a set.
            let Some(path) = std::env::var_os("PATH") else {
                return Default::default();
            };

            std::env::split_paths(&path)
                .filter_map(|path| std::fs::read_dir(path).ok())
                .flatten()
                .filter_map(|res| {
                    let entry = res.ok()?;
                    let file_type = entry.file_type().ok()?;
                    if file_type.is_file() || file_type.is_symlink() {
                        entry.file_name().into_string().ok()
                    } else {
                        None
                    }
                })
                .collect()
        });

        // Deferred because the first access to `PROGRAMS_IN_PATH` scans every directory in `$PATH`.
        let input = String::from(input);
        deferred(move |_| {
            fuzzy_match(&input, PROGRAMS_IN_PATH.iter(), MatchKind::Text)
                .into_iter()
                .map(|(name, _)| ((0..), name.clone().into()))
                .collect()
        })
    }

    /// Complete the last word within an otherwise raw argument.
    pub fn last_word(editor: &Editor, input: &str, complete: Completer) -> CompletionResult {
        let token = match Tokenizer::new(input, false).last() {
            Some(token) => token.unwrap(),
            None => return complete(editor, input),
        };

        let offset = token.content_start;

        complete(editor, &input[offset..]).map(move |mut completions| {
            for completion in completions.iter_mut() {
                completion.0.start += offset;
            }
            completions
        })
    }

    pub fn shell(editor: &Editor, input: &str) -> CompletionResult {
        let (command, args, complete_command) = command_line::split(input);

        if complete_command {
            return program(editor, command);
        }

        let offset = command.len() + 1; // + 1 for separator between `command` and `args`
        last_word(editor, args, filename).map(move |mut completions| {
            for completion in completions.iter_mut() {
                completion.0.start += offset;
            }
            completions
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_content_preserves_each_directory_level() {
        let root = tempfile::tempdir().unwrap();
        let config = editor_config::FileExplorerConfig::default();
        for service in ["authland", "portier"] {
            std::fs::create_dir_all(root.path().join(service).join("fleet").join("eu-west-1"))
                .unwrap();
        }
        std::fs::write(root.path().join("config.yaml"), "").unwrap();

        assert_eq!(
            directory_content(root.path(), &config).unwrap(),
            vec![
                (root.path().join(".."), true),
                (root.path().join("authland"), true),
                (root.path().join("portier"), true),
                (root.path().join("config.yaml"), false),
            ]
        );
        let service = root.path().join("authland");
        assert_eq!(
            directory_content(&service, &config).unwrap(),
            vec![(service.join(".."), true), (service.join("fleet"), true)]
        );
    }
}

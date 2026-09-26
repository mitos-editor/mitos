mod catalog;
mod command_line;
mod context;
pub(crate) mod dap;
mod editing;
mod history;
pub mod insert;
pub(crate) mod lsp;
mod mappable;
mod mode;
mod movement;
mod registers;
mod selection;
pub(crate) mod shell;
pub(crate) mod syntax;
pub(crate) mod typed;

pub use context::{Context, OnKeyCallback, OnKeyCallbackKind};
pub use dap::*;
pub(crate) use editing::replace_selections;
use event::status;
use futures_util::FutureExt;
pub use insert::{CommentContinuation, Open};
pub use lsp::*;
pub use mappable::MappableCommand;
pub use movement::scroll;
pub(crate) use registers::{
    paste, paste_bracketed_value, replace_selections_with_register,
    yank_main_selection_to_register, Paste,
};
use stdx::{
    path::{self, find_paths},
    rope::{self, RopeSliceExt},
};
pub use syntax::*;
use tui::{
    text::{Line, Span},
    widgets::Cell,
};
pub use typed::*;
use vcs::{FileChange, Hunk};

use editor_core::{
    chars::char_is_word,
    diagnostic::DiagnosticProvider,
    encoding, find_workspace,
    graphemes::{self, next_grapheme_boundary},
    indent::IndentStyle,
    line_ending::line_end_char_index,
    match_brackets,
    movement::{self as core_movement, Direction, Movement},
    object, pos_at_coords,
    regex::{self, Regex},
    syntax::config::LanguageServerFeature,
    text_annotations::Overlay,
    textobject, LineEnding, Range, RopeReader, RopeSlice, Selection, SmallVec, Syntax, Tendril,
    Transaction,
};
use ui_core::input::{self, KeyEvent};
use view::{
    document::{FormatterError, Mode, SCRATCH_BUFFER_NAME},
    editor::Action,
    expansion,
    icons::ICONS,
    info::Info,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    theme::Style,
    tree,
    view::View,
    Document, DocumentId, Editor, ViewId,
};

use anyhow::{anyhow, bail, Context as _};
use arc_swap::access::DynAccess;
use insert::insert_char;

use crate::{
    compositor::{self, Compositor},
    filter_picker_entry,
    job::Callback,
    ui::{self, overlay::overlaid, Picker, PickerColumn, Popup, PromptEvent},
};

use crate::job;
use std::{
    collections::HashSet, error::Error, future::Future, io::Read, num::NonZeroUsize, sync::LazyLock,
};

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use stdx::Url;

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};

use view::{align_view, Align};

fn no_op(_cx: &mut Context) {}

fn goto_next_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Forward, cx.count());
}

fn goto_previous_buffer(cx: &mut Context) {
    goto_buffer(cx.editor, Direction::Backward, cx.count());
}

fn goto_buffer(editor: &mut Editor, direction: Direction, count: usize) {
    let current = view!(editor).doc;

    let id = match direction {
        Direction::Forward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.cycle().skip_while(|id| *id != &current).nth(count)
        }
        Direction::Backward => {
            let iter = editor.documents.keys();
            // skip 'count' times past current buffer
            iter.rev()
                .cycle()
                .skip_while(|id| *id != &current)
                .nth(count)
        }
    }
    .unwrap();

    let id = *id;

    editor.switch(id, Action::Replace);
}

fn goto_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Move);
}

fn extend_to_file_start(cx: &mut Context) {
    goto_file_start_impl(cx, Movement::Extend);
}

fn goto_file_start_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        goto_line_impl(cx, movement);
    } else {
        let (view, doc) = current!(cx.editor);
        let text = doc.text().slice(..);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, 0, movement == Movement::Extend));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

fn goto_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Move);
}

fn extend_to_file_end(cx: &mut Context) {
    goto_file_end_impl(cx, Movement::Extend)
}

fn goto_file_end_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let pos = doc.text().len_chars();
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

fn goto_file(cx: &mut Context) {
    goto_file_impl(cx, Action::Replace);
}

fn goto_file_hsplit(cx: &mut Context) {
    goto_file_impl(cx, Action::HorizontalSplit);
}

fn goto_file_vsplit(cx: &mut Context) {
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

#[allow(clippy::too_many_arguments)]
fn search_impl(
    editor: &mut Editor,
    regex: &rope::Regex,
    movement: Movement,
    direction: Direction,
    scrolloff: usize,
    wrap_around: bool,
    show_warnings: bool,
) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);

    // Get the right side of the primary block cursor for forward search, or the
    // grapheme before the start of the selection for reverse search.
    let start = match direction {
        Direction::Forward => text.char_to_byte(graphemes::ensure_grapheme_boundary_next(
            text,
            selection.primary().to(),
        )),
        Direction::Backward => text.char_to_byte(graphemes::ensure_grapheme_boundary_prev(
            text,
            selection.primary().from(),
        )),
    };

    // A regex::Match returns byte-positions in the str. In the case where we
    // do a reverse search and wraparound to the end, we don't need to search
    // the text before the current cursor position for matches, but by slicing
    // it out, we need to add it back to the position of the selection.
    let doc = doc!(editor).text().slice(..);

    // use find_at to find the next match after the cursor, loop around the end
    // Careful, `Regex` uses `bytes` as offsets, not character indices!
    let mut mat = match direction {
        Direction::Forward => regex.find(doc.regex_input_at_bytes(start..)),
        Direction::Backward => regex.find_iter(doc.regex_input_at_bytes(..start)).last(),
    };

    if mat.is_none() {
        if wrap_around {
            mat = match direction {
                Direction::Forward => regex.find(doc.regex_input()),
                Direction::Backward => regex.find_iter(doc.regex_input_at_bytes(start..)).last(),
            };
        }
        if show_warnings {
            if wrap_around && mat.is_some() {
                editor.set_status("Wrapped around document");
            } else {
                editor.set_error(|| "No more matches");
            }
        }
    }

    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);

    if let Some(mat) = mat {
        let start = text.byte_to_char(mat.start());
        let end = text.byte_to_char(mat.end());

        if end == 0 {
            // skip empty matches that don't make sense
            return;
        }

        // Determine range direction based on the primary range
        let primary = selection.primary();
        let range = Range::new(start, end).with_direction(primary.direction());

        let selection = match movement {
            Movement::Extend => selection.clone().push(range),
            Movement::Move => selection.clone().replace(selection.primary_index(), range),
        };

        doc.set_selection(view.id, selection);
        view.ensure_cursor_in_view_center(doc, scrolloff);
    };
}

fn search_completions(cx: &mut Context, reg: Option<char>) -> Vec<String> {
    let mut items = reg
        .and_then(|reg| cx.editor.registers.read(reg, cx.editor))
        .map_or(Vec::new(), |reg| reg.take(200).collect());
    items.sort_unstable();
    items.dedup();
    items.into_iter().map(|value| value.to_string()).collect()
}

fn search(cx: &mut Context) {
    searcher(cx, Direction::Forward)
}

fn rsearch(cx: &mut Context) {
    searcher(cx, Direction::Backward)
}

fn searcher(cx: &mut Context, direction: Direction) {
    let reg = cx.register.unwrap_or('/');
    let config = cx.editor.config();
    let scrolloff = config.scrolloff;
    let wrap_around = config.search.wrap_around;
    let movement = if cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };

    // TODO: could probably share with select_on_matches?
    let completions = search_completions(cx, Some(reg));

    ui::regex_prompt(
        cx,
        "search:".into(),
        Some(reg),
        move |_editor: &Editor, input: &str| {
            completions
                .iter()
                .filter(|comp| comp.starts_with(input))
                .map(|comp| (0.., comp.clone().into()))
                .collect()
        },
        move |cx, regex, event| {
            if event == PromptEvent::Validate {
                cx.editor.registers.last_search_register = reg;
            } else if event != PromptEvent::Update {
                return;
            }
            search_impl(
                cx.editor,
                &regex,
                movement,
                direction,
                scrolloff,
                wrap_around,
                false,
            );
        },
    );
}

fn search_next_or_prev_impl(cx: &mut Context, movement: Movement, direction: Direction) {
    let count = cx.count();
    let register = cx
        .register
        .unwrap_or(cx.editor.registers.last_search_register);
    let config = cx.editor.config();
    let scrolloff = config.scrolloff;
    if let Some(query) = cx.editor.registers.first(register, cx.editor) {
        let search_config = &config.search;
        let case_insensitive = if search_config.smart_case {
            !query.chars().any(char::is_uppercase)
        } else {
            false
        };
        let wrap_around = search_config.wrap_around;
        let is_crlf = doc!(cx.editor).line_ending == LineEnding::Crlf;
        match rope::RegexBuilder::new()
            .syntax(
                rope::Config::new()
                    .case_insensitive(case_insensitive)
                    .multi_line(true)
                    .crlf(is_crlf),
            )
            .build(&query)
        {
            Ok(regex) => {
                for _ in 0..count {
                    search_impl(
                        cx.editor,
                        &regex,
                        movement,
                        direction,
                        scrolloff,
                        wrap_around,
                        true,
                    );
                }
            }
            _ => {
                // Only take ownership on the error path so valid repeated searches keep
                // using the register's borrowed value without an extra allocation.
                let query = query.into_owned();
                cx.editor.set_error(|| format!("Invalid regex: {}", query));
            }
        }
    }
}

fn search_next(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Move, Direction::Forward);
}

fn search_prev(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Move, Direction::Backward);
}
fn extend_search_next(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Extend, Direction::Forward);
}

fn extend_search_prev(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Extend, Direction::Backward);
}

fn search_selection(cx: &mut Context) {
    search_selection_impl(cx, false)
}

fn search_selection_detect_word_boundaries(cx: &mut Context) {
    search_selection_impl(cx, true)
}

fn search_selection_impl(cx: &mut Context, detect_word_boundaries: bool) {
    fn is_at_word_start(text: RopeSlice, index: usize) -> bool {
        // This can happen when the cursor is at the last character in
        // the document +1 (ge + j), in this case text.char(index) will panic as
        // it will index out of bounds. See https://github.com/helix-editor/helix/issues/12609
        if index == text.len_chars() {
            return false;
        }
        let ch = text.char(index);
        if index == 0 {
            return char_is_word(ch);
        }
        let prev_ch = text.char(index - 1);

        !char_is_word(prev_ch) && char_is_word(ch)
    }

    fn is_at_word_end(text: RopeSlice, index: usize) -> bool {
        if index == 0 || index == text.len_chars() {
            return false;
        }
        let ch = text.char(index);
        let prev_ch = text.char(index - 1);

        char_is_word(prev_ch) && !char_is_word(ch)
    }

    let register = cx.register.unwrap_or('/');
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let regex = doc
        .selection(view.id)
        .iter()
        .map(|selection| {
            let add_boundary_prefix =
                detect_word_boundaries && is_at_word_start(text, selection.from());
            let add_boundary_suffix =
                detect_word_boundaries && is_at_word_end(text, selection.to());

            let prefix = if add_boundary_prefix { "\\b" } else { "" };
            let suffix = if add_boundary_suffix { "\\b" } else { "" };

            let word = regex::escape(&selection.fragment(text));
            format!("{}{}{}", prefix, word, suffix)
        })
        .collect::<HashSet<_>>() // Collect into hashset to deduplicate identical regexes
        .into_iter()
        .collect::<Vec<_>>()
        .join("|");

    let msg = format!("register '{}' set to '{}'", register, regex);
    match cx.editor.registers.push(register, regex) {
        Ok(_) => {
            cx.editor.registers.last_search_register = register;
            cx.editor.set_status(msg)
        }
        Err(err) => cx.editor.set_error(|| err.to_string()),
    }
}

fn make_search_word_bounded(cx: &mut Context) {
    // Defaults to the active search register instead `/` to be more ergonomic assuming most people
    // would use this command following `search_selection`. This avoids selecting the register
    // twice.
    let register = cx
        .register
        .unwrap_or(cx.editor.registers.last_search_register);
    let regex = match cx.editor.registers.first(register, cx.editor) {
        Some(regex) => regex,
        None => return,
    };
    let start_anchored = regex.starts_with("\\b");
    let end_anchored = regex.ends_with("\\b");

    if start_anchored && end_anchored {
        return;
    }

    let mut new_regex = String::with_capacity(
        regex.len() + if start_anchored { 0 } else { 2 } + if end_anchored { 0 } else { 2 },
    );

    if !start_anchored {
        new_regex.push_str("\\b");
    }
    new_regex.push_str(&regex);
    if !end_anchored {
        new_regex.push_str("\\b");
    }

    let msg = format!("register '{}' set to '{}'", register, new_regex);
    match cx.editor.registers.push(register, new_regex) {
        Ok(_) => {
            cx.editor.registers.last_search_register = register;
            cx.editor.set_status(msg)
        }
        Err(err) => cx.editor.set_error(|| err.to_string()),
    }
}

fn global_search(cx: &mut Context) {
    #[derive(Debug)]
    struct FileResult<'a> {
        path: Cow<'a, Path>,
        /// 0 indexed line start
        line_start: usize,
        /// 0 indexed line end
        line_end: usize,
        /// Zero-based character column where the match starts.
        match_start_col: usize,
        /// Zero-based character column where the match ends.
        match_end_col: usize,
    }

    impl FileResult<'_> {
        fn new(
            path: &Path,
            line_start: usize,
            line_end: usize,
            match_start_col: usize,
            match_end_col: usize,
        ) -> Self {
            Self {
                path: stdx::path::get_relative_path(path.to_path_buf()),
                line_start,
                line_end,
                match_start_col,
                match_end_col,
            }
        }
    }

    /// Converts the regex engine's byte match within `line_content` into
    /// zero-based line and character-column bounds relative to `line_start`.
    fn match_line_cols(
        line_start: usize,
        line_content: &str,
        matcher: &grep_regex::RegexMatcher,
    ) -> Option<(usize, usize, usize, usize)> {
        let matched = matcher.find(line_content.as_bytes()).ok().flatten()?;
        let prefix = &line_content[..matched.start()];
        let matched_text = &line_content[..matched.end()];

        let start_line = line_start + prefix.matches('\n').count();
        let start_col = prefix.rsplit('\n').next().unwrap_or(prefix).chars().count();

        let end_line = line_start + matched_text.matches('\n').count();
        let end_col = matched_text
            .rsplit('\n')
            .next()
            .unwrap_or(matched_text)
            .chars()
            .count();

        Some((start_line, start_col, end_line, end_col))
    }

    struct GlobalSearchConfig {
        smart_case: bool,
        file_picker_config: view::editor::FilePickerConfig,
        style: PathStyleConfig,
    }

    let config = cx.editor.config();
    let config = GlobalSearchConfig {
        smart_case: config.search.smart_case,
        file_picker_config: config.file_picker.clone(),
        style: PathStyleConfig::new(cx.editor),
    };

    let columns = [
        PickerColumn::new("path", |item: &FileResult, config: &GlobalSearchConfig| {
            config
                .style
                .stylize(Some(&item.path), Some(item.line_start))
        }),
        PickerColumn::hidden("contents"),
    ];

    let get_files = |query: &str,
                     editor: &mut Editor,
                     config: std::sync::Arc<GlobalSearchConfig>,
                     injector: &ui::picker::Injector<_, _>| {
        if query.is_empty() {
            return async { Ok(()) }.boxed();
        }

        let search_root = stdx::env::current_working_dir();
        if !search_root.exists() {
            return async { Err(anyhow::anyhow!("Current working directory does not exist")) }
                .boxed();
        }

        let documents: Vec<_> = editor
            .documents()
            .map(|doc| (doc.path().map(ToOwned::to_owned), doc.text().to_owned()))
            .collect();

        let matcher = match RegexMatcherBuilder::new()
            .case_smart(config.smart_case)
            .multi_line(true)
            .build(query)
        {
            Ok(matcher) => {
                // Clear any "Failed to compile regex" errors out of the statusline.
                editor.clear_status();
                matcher
            }
            Err(err) => {
                log::info!("Failed to compile search pattern in global search: {}", err);
                return async { Err(anyhow::anyhow!("Failed to compile regex")) }.boxed();
            }
        };

        let dedup_symlinks = config.file_picker_config.deduplicate_links;
        let absolute_root = search_root
            .canonicalize()
            .unwrap_or_else(|_| search_root.clone());

        let injector = injector.clone();
        async move {
            let searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .multi_line(true)
                .build();
            WalkBuilder::new(search_root)
                .hidden(config.file_picker_config.hidden)
                .parents(config.file_picker_config.parents)
                .ignore(config.file_picker_config.ignore)
                .follow_links(config.file_picker_config.follow_symlinks)
                .git_ignore(config.file_picker_config.git_ignore)
                .git_global(config.file_picker_config.git_global)
                .git_exclude(config.file_picker_config.git_exclude)
                .max_depth(config.file_picker_config.max_depth)
                .filter_entry(move |entry| {
                    filter_picker_entry(entry, &absolute_root, dedup_symlinks)
                })
                .add_custom_ignore_filename(loader::config_dir().join("ignore"))
                .add_custom_ignore_filename(".mitos/ignore")
                .build_parallel()
                .run(|| {
                    let mut searcher = searcher.clone();
                    let matcher = matcher.clone();
                    let injector = injector.clone();
                    let documents = &documents;
                    Box::new(move |entry: Result<DirEntry, ignore::Error>| -> WalkState {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(_) => return WalkState::Continue,
                        };

                        if !entry.path().is_file() {
                            return WalkState::Continue;
                        }

                        let mut stop = false;
                        let sink = sinks::UTF8(|line_start, line_content| {
                            let line_start = line_start as usize - 1;
                            let Some((
                                match_start_line,
                                match_start_col,
                                match_end_line,
                                match_end_col,
                            )) = match_line_cols(line_start, line_content, &matcher)
                            else {
                                return Ok(true);
                            };
                            stop = injector
                                .push(FileResult::new(
                                    entry.path(),
                                    match_start_line,
                                    match_end_line,
                                    match_start_col,
                                    match_end_col,
                                ))
                                .is_err();

                            Ok(!stop)
                        });
                        let doc = documents.iter().find(|&(doc_path, _)| {
                            doc_path
                                .as_ref()
                                .is_some_and(|doc_path| doc_path == entry.path())
                        });

                        let result = if let Some((_, doc)) = doc {
                            // there is already a buffer for this file
                            // search the buffer instead of the file because it's faster
                            // and captures new edits without requiring a save
                            if searcher.multi_line_with_matcher(&matcher) {
                                // in this case a continuous buffer is required
                                // convert the rope to a string
                                let text = doc.to_string();
                                searcher.search_slice(&matcher, text.as_bytes(), sink)
                            } else {
                                searcher.search_reader(
                                    &matcher,
                                    RopeReader::new(doc.slice(..)),
                                    sink,
                                )
                            }
                        } else {
                            searcher.search_path(&matcher, entry.path(), sink)
                        };

                        if let Err(err) = result {
                            log::error!("Global search error: {}, {}", entry.path().display(), err);
                        }
                        if stop {
                            WalkState::Quit
                        } else {
                            WalkState::Continue
                        }
                    })
                });
            Ok(())
        }
        .boxed()
    };

    let reg = cx.register.unwrap_or('/');
    cx.editor.registers.last_search_register = reg;

    let picker = Picker::new(
        columns,
        1, // contents
        [],
        config,
        move |cx,
              FileResult {
                  path,
                  line_start,
                  line_end,
                  match_start_col,
                  match_end_col,
                  ..
              },
              action| {
            let doc = match cx.editor.open(path, action) {
                Ok(id) => doc_mut!(cx.editor, &id),
                Err(e) => {
                    cx.editor
                        .set_error(|| format!("Failed to open file '{}': {}", path.display(), e));
                    return;
                }
            };

            let line_start = *line_start;
            let line_end = *line_end;
            let view = view_mut!(cx.editor);
            let text = doc.text();
            let Some(selection) = selection_for_global_search_match(
                text.slice(..),
                line_start,
                *match_start_col,
                line_end,
                *match_end_col,
            ) else {
                cx.editor
                    .set_error(|| "The match you jumped to does not exist anymore.");
                return;
            };
            doc.set_selection(view.id, selection);
            if action.align_view(view, doc.id()) {
                align_view(doc, view, Align::Center);
            }
        },
    )
    .with_preview(
        |_editor,
         FileResult {
             path,
             line_start,
             line_end,
             ..
         }| { Some((path.as_ref().into(), Some((*line_start, *line_end)))) },
    )
    .with_quicklist(|_editor, item| {
        Some(QuicklistEntry {
            target: QuicklistTarget::Path(item.path.clone().into_owned()),
            position: QuicklistPosition::LineColRange {
                start_line: item.line_start,
                start_col: item.match_start_col,
                end_line: item.line_end,
                end_col: item.match_end_col,
            },
        })
    })
    .with_history_register(Some(reg))
    .with_dynamic_query(get_files, Some(275));

    cx.push_layer(Box::new(overlaid(picker)));
}

fn selection_for_global_search_match(
    text: RopeSlice,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> Option<Selection> {
    if start_line > end_line || end_line >= text.len_lines() {
        return None;
    }

    let start = text.line_to_char(start_line).checked_add(start_col)?;
    let end = text.line_to_char(end_line).checked_add(end_col)?;
    if start > line_end_char_index(&text, start_line) || end > line_end_char_index(&text, end_line)
    {
        return None;
    }

    Some(Selection::single(start, end).ensure_invariants(text))
}

fn file_picker(cx: &mut Context) {
    let root = find_workspace().0;
    if !root.exists() {
        cx.editor.set_error(|| "Workspace directory does not exist");
        return;
    }
    let picker = ui::file_picker(cx.editor, root);
    cx.push_layer(Box::new(overlaid(picker)));
}

fn file_picker_in_current_buffer_directory(cx: &mut Context) {
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

fn file_picker_in_current_directory(cx: &mut Context) {
    let cwd = stdx::env::current_working_dir();
    if !cwd.exists() {
        cx.editor
            .set_error(|| "Current working directory does not exist");
        return;
    }
    let picker = ui::file_picker(cx.editor, cwd);
    cx.push_layer(Box::new(overlaid(picker)));
}

fn file_explorer(cx: &mut Context) {
    let root = find_workspace().0;
    if !root.exists() {
        cx.editor.set_error(|| "Workspace directory does not exist");
        return;
    }

    if let Ok(picker) = ui::file_explorer(root, cx.editor) {
        cx.push_layer(Box::new(overlaid(picker)));
    }
}

fn file_explorer_in_current_buffer_directory(cx: &mut Context) {
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

fn file_explorer_in_current_directory(cx: &mut Context) {
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

struct PathStyleConfig {
    theme: std::sync::Arc<view::Theme>,
    icons: bool,
    directory_style: Style,
    number_style: Style,
    colon_style: Style,
}

impl PathStyleConfig {
    fn new(editor: &Editor) -> Self {
        let theme = &editor.theme;
        Self {
            theme: std::sync::Arc::new(theme.clone()),
            icons: editor.config().icons,
            directory_style: theme.get("ui.text.directory"),
            number_style: theme.get("constant.numeric.integer"),
            colon_style: theme.get("punctuation"),
        }
    }

    fn stylize<'a>(&self, path: Option<&'a Path>, line: Option<usize>) -> Cell<'a> {
        let mut spans = Vec::new();
        if let Some(path) = path {
            if self.icons {
                let icons = ICONS.load();
                if let Some(file) = icons.fs().file() {
                    spans.push(Span::from(
                        file.get_with_style_or_default(path, self.theme.as_ref()),
                    ));
                }
            }
            let directories = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| format!("{}{}", p.display(), std::path::MAIN_SEPARATOR))
                .unwrap_or_default();
            spans.push(Span::styled(directories, self.directory_style));
        }
        let filename = path.as_ref().map_or(SCRATCH_BUFFER_NAME.into(), |path| {
            path.file_name()
                .expect("all document names are normalized (can't end in `..`)")
                .to_string_lossy()
        });
        spans.push(Span::raw(filename));
        if let Some(line) = line {
            spans.extend([
                Span::styled(":", self.colon_style),
                Span::styled((line + 1).to_string(), self.number_style),
            ]);
        }

        Cell::from(Line::from(spans))
    }
}

fn buffer_picker(cx: &mut Context) {
    let current = view!(cx.editor).doc;

    struct BufferMeta<'a> {
        id: DocumentId,
        path: Option<Cow<'a, Path>>,
        is_modified: bool,
        is_current: bool,
        focused_at: std::time::Instant,
    }

    let new_meta = |doc: &Document| BufferMeta {
        id: doc.id(),
        path: doc
            .path()
            .map(ToOwned::to_owned)
            .map(stdx::path::get_relative_path),
        is_modified: doc.is_modified(),
        is_current: doc.id() == current,
        focused_at: doc.focused_at,
    };

    let mut items = cx
        .editor
        .documents
        .values()
        .map(new_meta)
        .collect::<Vec<BufferMeta>>();

    // mru
    items.sort_unstable_by_key(|item| std::cmp::Reverse(item.focused_at));

    let columns = [
        PickerColumn::new("id", |meta: &BufferMeta, _| meta.id.to_string().into()),
        PickerColumn::new("flags", |meta: &BufferMeta, _| {
            let mut flags = String::new();
            if meta.is_modified {
                flags.push('+');
            }
            if meta.is_current {
                flags.push('*');
            }
            flags.into()
        }),
        PickerColumn::new("path", |meta: &BufferMeta, config: &PathStyleConfig| {
            config.stylize(meta.path.as_deref(), None)
        }),
    ];

    let initial_cursor = if cx
        .editor
        .config()
        .buffer_picker
        .start_position
        .is_previous()
        && !items.is_empty()
    {
        1
    } else {
        0
    };

    let picker = Picker::new(
        columns,
        2,
        items,
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            cx.editor.switch(meta.id, action);
        },
    )
    .with_initial_cursor(initial_cursor)
    .with_preview(|editor, meta| {
        let doc = &editor.documents.get(&meta.id)?;
        let lines = doc.selections().values().next().map(|selection| {
            let cursor_line = selection.primary().cursor_line(doc.text().slice(..));
            (cursor_line, cursor_line)
        });
        Some((meta.id.into(), lines))
    });
    cx.push_layer(Box::new(overlaid(picker)));
}

fn jumplist_picker(cx: &mut Context) {
    struct JumpMeta<'a> {
        id: DocumentId,
        path: Option<Cow<'a, Path>>,
        selection: Selection,
        line_start: usize,
        text: String,
        is_current: bool,
    }

    for (view, _) in cx.editor.tree.views_mut() {
        for doc_id in view.jumps.iter().map(|e| e.0).collect::<Vec<_>>().iter() {
            let doc = doc_mut!(cx.editor, doc_id);
            view.sync_changes(doc);
        }
    }

    let new_meta = |view: &View, doc_id: DocumentId, selection: Selection| {
        let doc = doc!(cx.editor, &doc_id);
        let text = doc.text().slice(..);
        let contents = selection
            .fragments(text)
            .map(Cow::into_owned)
            .collect::<Vec<_>>()
            .join(" ");
        let line_start = selection.primary().cursor_line(text);

        JumpMeta {
            id: doc_id,
            path: doc
                .path()
                .map(ToOwned::to_owned)
                .map(stdx::path::get_relative_path),
            selection,
            line_start,
            text: contents,
            is_current: view.doc == doc_id,
        }
    };

    let columns = [
        ui::PickerColumn::new("id", |item: &JumpMeta, _| item.id.to_string().into()),
        ui::PickerColumn::new("path", |item: &JumpMeta, config: &PathStyleConfig| {
            config.stylize(item.path.as_deref(), Some(item.line_start))
        }),
        ui::PickerColumn::new("flags", |item: &JumpMeta, _| {
            let mut flags = Vec::new();
            if item.is_current {
                flags.push("*");
            }

            if flags.is_empty() {
                "".into()
            } else {
                format!(" ({})", flags.join("")).into()
            }
        }),
        ui::PickerColumn::new("contents", |item: &JumpMeta, _| item.text.as_str().into()),
    ];

    let picker = Picker::new(
        columns,
        1, // path
        cx.editor.tree.views().flat_map(|(view, _)| {
            view.jumps
                .iter()
                .rev()
                .map(|(doc_id, selection)| new_meta(view, *doc_id, selection.clone()))
        }),
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            cx.editor.switch(meta.id, action);
            let config = cx.editor.config();
            let (view, doc) = (view_mut!(cx.editor), doc_mut!(cx.editor, &meta.id));
            doc.set_selection(view.id, meta.selection.clone());
            if action.align_view(view, doc.id()) {
                view.ensure_cursor_in_view_center(doc, config.scrolloff);
            }
        },
    )
    .with_preview(|editor, meta| {
        let doc = &editor.documents.get(&meta.id)?;
        let line = meta.selection.primary().cursor_line(doc.text().slice(..));
        Some((meta.id.into(), Some((line, line))))
    })
    .with_quicklist(|_editor, meta| {
        Some(QuicklistEntry {
            target: QuicklistTarget::Document(meta.id),
            position: QuicklistPosition::Selection(meta.selection.clone()),
        })
    });
    cx.push_layer(Box::new(overlaid(picker)));
}

fn quicklist_entry_line_range(editor: &Editor, entry: &QuicklistEntry) -> Option<(usize, usize)> {
    let text = match &entry.target {
        QuicklistTarget::Path(path) => editor
            .document_by_path(path)
            .map(|doc| doc.text().slice(..)),
        QuicklistTarget::Document(id) => editor.documents.get(id).map(|doc| doc.text().slice(..)),
    };

    entry.position.line_range(text)
}

fn quicklist_picker(cx: &mut Context) {
    #[derive(Clone)]
    struct QuicklistMeta {
        index: usize,
        entry: QuicklistEntry,
        path: Option<PathBuf>,
        label: String,
        line: Option<usize>,
        is_current: bool,
    }

    let entries = cx.editor.quicklist.entries();
    if entries.is_empty() {
        cx.editor.set_error(|| "No quicklist entries available");
        return;
    }

    let items = entries
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, entry)| {
            let path = match &entry.target {
                QuicklistTarget::Path(path) => {
                    Some(stdx::path::get_relative_path(path).into_owned())
                }
                QuicklistTarget::Document(id) => cx
                    .editor
                    .documents
                    .get(id)
                    .and_then(|doc| doc.path())
                    .map(stdx::path::get_relative_path)
                    .map(Cow::into_owned),
            };
            let label = path
                .as_deref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| match &entry.target {
                    QuicklistTarget::Document(id) => format!("{SCRATCH_BUFFER_NAME} ({id})"),
                    QuicklistTarget::Path(_) => unreachable!(),
                });

            QuicklistMeta {
                index,
                path,
                label,
                line: quicklist_entry_line_range(cx.editor, &entry).map(|(start, _)| start + 1),
                is_current: cx.editor.quicklist.current() == Some(index),
                entry,
            }
        })
        .collect::<Vec<_>>();

    let columns = [
        ui::PickerColumn::new("path", |item: &QuicklistMeta, config: &PathStyleConfig| {
            item.path.as_deref().map_or_else(
                || item.label.as_str().into(),
                |path| config.stylize(Some(path), None),
            )
        }),
        ui::PickerColumn::new("line", |item: &QuicklistMeta, _| {
            item.line
                .map_or_else(String::new, |line| line.to_string())
                .into()
        }),
        ui::PickerColumn::new("flags", |item: &QuicklistMeta, _| {
            if item.is_current {
                " (*)".into()
            } else {
                "".into()
            }
        }),
    ];

    let initial_cursor = cx.editor.quicklist.current().unwrap_or(0) as u32;

    let picker = Picker::new(
        columns,
        0,
        items,
        PathStyleConfig::new(cx.editor),
        |cx, meta, action| {
            let view_id = cx.editor.tree.focus;
            if cx
                .editor
                .activate_quicklist_entry(view_id, &meta.entry, action)
            {
                cx.editor.quicklist.set_current(Some(meta.index));
            }
        },
    )
    .with_initial_cursor(initial_cursor)
    .with_preview(|editor, meta| {
        let path_or_id = match &meta.entry.target {
            QuicklistTarget::Path(path) => path.as_path().into(),
            QuicklistTarget::Document(id) => (*id).into(),
        };
        Some((path_or_id, quicklist_entry_line_range(editor, &meta.entry)))
    });

    cx.push_layer(Box::new(overlaid(picker)));
}

fn changed_file_picker(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Directory(loader::find_workspace().0),
    );
}

fn changed_file_picker_in_repository(cx: &mut Context) {
    changed_file_picker_for_scope(
        cx,
        vcs::ChangedFileScope::Repository(stdx::env::current_working_dir()),
    );
}

fn changed_file_picker_for_scope(cx: &mut Context, scope: vcs::ChangedFileScope) {
    struct ChangedFileEntry {
        change: FileChange,
        display_path: String,
    }

    pub struct FileChangeData {
        icons: bool,
        style_untracked: Style,
        style_modified: Style,
        style_conflict: Style,
        style_deleted: Style,
        style_renamed: Style,
    }

    fn display_path(change: &FileChange, worktree_root: &Path) -> String {
        let display_path = |path: &Path| {
            stdx::path::get_relative_path_from(path, worktree_root)
                .display()
                .to_string()
        };

        match change {
            FileChange::Untracked { path }
            | FileChange::Modified { path }
            | FileChange::Conflict { path }
            | FileChange::Deleted { path } => display_path(path),
            FileChange::Renamed { from_path, to_path } => {
                format!("{} -> {}", display_path(from_path), display_path(to_path))
            }
        }
    }

    fn change_column<'a>(entry: &'a ChangedFileEntry, data: &FileChangeData) -> Cell<'a> {
        let icons = ICONS.load();
        let (plain, icon, label, style) = match &entry.change {
            FileChange::Untracked { .. } => (
                "+ untracked",
                icons.vcs().added(),
                "untracked",
                data.style_untracked,
            ),
            FileChange::Modified { .. } => (
                "~ modified",
                icons.vcs().modified(),
                "modified",
                data.style_modified,
            ),
            FileChange::Conflict { .. } => (
                "x conflict",
                icons.vcs().conflict(),
                "conflict",
                data.style_conflict,
            ),
            FileChange::Deleted { .. } => (
                "- deleted",
                icons.vcs().removed(),
                "deleted",
                data.style_deleted,
            ),
            FileChange::Renamed { .. } => (
                "> renamed",
                icons.vcs().renamed(),
                "renamed",
                data.style_renamed,
            ),
        };
        let content = if data.icons {
            icon.map_or_else(|| label.to_string(), |icon| format!("{icon}{label}"))
        } else {
            plain.to_string()
        };
        Span::styled(content, style).into()
    }

    fn path_column<'a>(entry: &'a ChangedFileEntry, _data: &FileChangeData) -> Cell<'a> {
        entry.display_path.as_str().into()
    }

    if !scope.path().exists() {
        cx.editor.set_error(|| "Changed file scope does not exist");
        return;
    }

    let workspace_root = loader::find_workspace_in(scope.path()).0;
    let display_root = match &scope {
        vcs::ChangedFileScope::Directory(path) => Some(path.clone()),
        vcs::ChangedFileScope::Repository(_) => None,
    };

    let added = cx.editor.theme.get("diff.plus");
    let modified = cx.editor.theme.get("diff.delta");
    let conflict = cx.editor.theme.get("diff.delta.conflict");
    let deleted = cx.editor.theme.get("diff.minus");
    let renamed = cx.editor.theme.get("diff.delta.moved");

    let columns = [
        PickerColumn::new("change", change_column),
        PickerColumn::new("path", path_column),
    ];

    let picker = Picker::new(
        columns,
        1, // path
        [],
        FileChangeData {
            icons: cx.editor.config().icons,
            style_untracked: added,
            style_modified: modified,
            style_conflict: conflict,
            style_deleted: deleted,
            style_renamed: renamed,
        },
        |cx, entry: &ChangedFileEntry, action| {
            let path_to_open = entry.change.path();
            if let Err(err) = cx.editor.open(path_to_open, action) {
                cx.editor.set_error(|| {
                    if let Some(err) = err.source() {
                        format!("{}", err)
                    } else {
                        format!("unable to open \"{}\"", path_to_open.display())
                    }
                });
            }
        },
    )
    .with_preview(|_editor, entry| Some((entry.change.path().into(), None)));
    let injector = picker.injector();

    let trust_full = cx
        .editor
        .workspace_trust
        .query(&workspace_root, loader::workspace_trust::TrustQuery::Git)
        .is_trusted();
    cx.editor.diff_providers.clone().for_each_changed_file(
        scope,
        trust_full,
        move |worktree_root, change| match change {
            Ok(change) => injector
                .push(ChangedFileEntry {
                    display_path: display_path(
                        &change,
                        display_root.as_deref().unwrap_or(worktree_root),
                    ),
                    change,
                })
                .is_ok(),
            Err(err) => {
                status::report_blocking(err);
                true
            }
        },
    );
    cx.push_layer(Box::new(overlaid(picker)));
}

struct CommandPaletteData {
    keymap: crate::keymap::ReverseKeymap,
    command_style: Style,
    binding_style: Style,
}

fn command_palette_name<'a>(item: &'a MappableCommand, data: &CommandPaletteData) -> Cell<'a> {
    let name: Cow<'a, str> = match item {
        MappableCommand::Typable { name, .. } => format!(":{name}").into(),
        MappableCommand::Static { name, .. } => (*name).into(),
        MappableCommand::Macro { .. } => {
            unreachable!("macros aren't included in the command palette")
        }
    };

    Span::styled(name, data.command_style).into()
}

fn command_palette_bindings<'a>(item: &MappableCommand, data: &CommandPaletteData) -> Cell<'a> {
    let bindings = data
        .keymap
        .get(item.name())
        .map(|bindings| {
            bindings.iter().fold(String::new(), |mut acc, bind| {
                if !acc.is_empty() {
                    acc.push(' ');
                }
                for key in bind {
                    acc.push_str(&key.key_sequence_format());
                }
                acc
            })
        })
        .unwrap_or_default();

    Span::styled(bindings, data.binding_style).into()
}

pub fn command_palette(cx: &mut Context) {
    let register = cx.register;
    let count = cx.count;

    cx.callback.push(Box::new(
        move |compositor: &mut Compositor, cx: &mut compositor::Context| {
            let keymap = compositor.find::<ui::EditorView>().unwrap().keymaps.map()
                [&cx.editor.mode]
                .reverse_map();
            let data = CommandPaletteData {
                keymap,
                command_style: cx.editor.theme.get("constant"),
                binding_style: cx.editor.theme.get("markup.raw.inline"),
            };

            let commands = MappableCommand::STATIC_COMMAND_LIST.iter().cloned().chain(
                catalog::TYPABLE_COMMAND_LIST
                    .iter()
                    .map(|cmd| MappableCommand::Typable {
                        name: cmd.name.to_owned(),
                        args: String::new(),
                        doc: cmd.doc.to_owned(),
                    }),
            );

            let columns = [
                ui::PickerColumn::new("name", command_palette_name),
                ui::PickerColumn::new("bindings", command_palette_bindings),
                ui::PickerColumn::new("doc", |item: &MappableCommand, _| item.doc().into()),
            ];

            let picker = Picker::new(columns, 0, commands, data, move |cx, command, _action| {
                let mut ctx = Context {
                    config: cx.config,
                    register,
                    count,
                    editor: cx.editor,
                    callback: Vec::new(),
                    on_next_key_callback: None,
                    jobs: cx.jobs,
                };
                let focus = view!(ctx.editor).id;

                command.execute(&mut ctx);

                if ctx.editor.tree.contains(focus) {
                    let config = ctx.editor.config();
                    let mode = ctx.editor.mode();
                    let view = view_mut!(ctx.editor, focus);
                    let doc = doc_mut!(ctx.editor, &view.doc);

                    view.ensure_cursor_in_view(doc, config.scrolloff);

                    if mode != Mode::Insert {
                        doc.append_changes_to_history(view);
                    }
                }
            });
            compositor.push(Box::new(overlaid(picker)));
        },
    ));
}

fn last_picker(cx: &mut Context) {
    // TODO: last picker does not seem to work well with buffer_picker
    cx.callback.push(Box::new(|compositor, cx| {
        match compositor.last_picker.take() {
            Some(picker) => {
                compositor.push(picker);
            }
            _ => cx.editor.set_error(|| "no last picker"),
        }
    }));
}

// Creates an LspCallback that waits for formatting changes to be computed. When they're done,
// it applies them, but only if the doc hasn't changed.
//
// TODO: provide some way to cancel this, probably as part of a more general job cancellation
// scheme
async fn make_format_callback(
    doc_id: DocumentId,
    doc_version: i32,
    view_id: ViewId,
    format: impl Future<Output = Result<Transaction, FormatterError>> + Send + 'static,
    write: Option<(Option<PathBuf>, bool)>,
) -> anyhow::Result<job::Callback> {
    let format = format.await;

    let call: job::Callback = Callback::Editor(Box::new(move |editor| {
        if !editor.documents.contains_key(&doc_id) || !editor.tree.contains(view_id) {
            return;
        }

        let scrolloff = editor.config().scrolloff;
        let doc = doc_mut!(editor, &doc_id);
        let view = view_mut!(editor, view_id);

        match format {
            Ok(format) => {
                if doc.version() == doc_version {
                    doc.apply(&format, view.id);
                    doc.append_changes_to_history(view);
                    doc.detect_indent_and_line_ending();
                    view.ensure_cursor_in_view(doc, scrolloff);
                } else {
                    log::info!("discarded formatting changes because the document changed");
                }
            }
            Err(err) => {
                if write.is_none() {
                    editor.set_error(|| err.to_string());
                    return;
                }
                log::info!("failed to format '{}': {err}", doc.display_name());
            }
        }

        if let Some((path, force)) = write {
            let id = doc.id();
            if let Err(err) = editor.save(id, path, force) {
                editor.set_error(|| format!("Error saving: {}", err));
            }
        }
    }));

    Ok(call)
}

// Store a jump on the jumplist.
fn push_jump(view: &mut View, doc: &mut Document) {
    doc.append_changes_to_history(view);
    let jump = (doc.id(), doc.selection(view.id).clone());
    view.push_jump(doc, jump);
}

fn goto_line(cx: &mut Context) {
    goto_line_impl(cx, Movement::Move);
}

fn goto_line_impl(cx: &mut Context, movement: Movement) {
    if cx.count.is_some() {
        let (view, doc) = current!(cx.editor);
        push_jump(view, doc);

        goto_line_without_jumplist(cx.editor, cx.count, movement);
    }
}

fn goto_line_without_jumplist(
    editor: &mut Editor,
    count: Option<NonZeroUsize>,
    movement: Movement,
) {
    if let Some(count) = count {
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let max_line = if text.line(text.len_lines() - 1).len_chars() == 0 {
            // If the last line is blank, don't jump to it.
            text.len_lines().saturating_sub(2)
        } else {
            text.len_lines() - 1
        };
        let line_idx = std::cmp::min(count.get() - 1, max_line);
        let pos = text.line_to_char(line_idx);
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

        doc.set_selection(view.id, selection);
    }
}

fn goto_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Move)
}

fn extend_to_last_line(cx: &mut Context) {
    goto_last_line_impl(cx, Movement::Extend)
}

fn goto_last_line_impl(cx: &mut Context, movement: Movement) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let line_idx = if text.line(text.len_lines() - 1).len_chars() == 0 {
        // If the last line is blank, don't jump to it.
        text.len_lines().saturating_sub(2)
    } else {
        text.len_lines() - 1
    };
    let pos = text.line_to_char(line_idx);
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(text, pos, movement == Movement::Extend));

    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

fn goto_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Move);
}

fn extend_to_column(cx: &mut Context) {
    goto_column_impl(cx, Movement::Extend);
}

fn goto_column_impl(cx: &mut Context, movement: Movement) {
    let count = cx.count();
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone().transform(|range| {
        let line = range.cursor_line(text);
        let line_start = text.line_to_char(line);
        let line_end = line_end_char_index(&text, line);
        let pos = graphemes::nth_next_grapheme_boundary(text, line_start, count - 1).min(line_end);
        range.put_cursor(text, pos, movement == Movement::Extend)
    });
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
}

fn goto_last_accessed_file(cx: &mut Context) {
    let view = view_mut!(cx.editor);
    if let Some(alt) = view.docs_access_history.pop() {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last accessed buffer")
    }
}

fn goto_last_modification(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let pos = doc.history.get_mut().last_edit_pos();
    let text = doc.text().slice(..);
    if let Some(pos) = pos {
        let selection = doc
            .selection(view.id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, cx.editor.mode == Mode::Select));
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
    }
}

fn goto_last_modified_file(cx: &mut Context) {
    let view = view!(cx.editor);
    let alternate_file = view
        .last_modified_docs
        .into_iter()
        .flatten()
        .find(|&id| id != view.doc);
    if let Some(alt) = alternate_file {
        cx.editor.switch(alt, Action::Replace);
    } else {
        cx.editor.set_error(|| "no last modified buffer")
    }
}

fn goto_first_diag(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = match doc.diagnostics().first() {
        Some(diag) => Selection::single(diag.range.start, diag.range.end),
        None => return,
    };
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
    view.diagnostics_handler
        .immediately_show_diagnostic(doc, view.id);
}

fn goto_last_diag(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let selection = match doc.diagnostics().last() {
        Some(diag) => Selection::single(diag.range.start, diag.range.end),
        None => return,
    };
    push_jump(view, doc);
    doc.set_selection(view.id, selection);
    view.diagnostics_handler
        .immediately_show_diagnostic(doc, view.id);
}

fn goto_next_diag(cx: &mut Context) {
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        let cursor_pos = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diag = doc
            .diagnostics()
            .iter()
            .find(|diag| diag.range.start > cursor_pos);

        let selection = match diag {
            Some(diag) => Selection::single(diag.range.start, diag.range.end),
            None => return,
        };
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    };

    cx.editor.apply_motion(motion);
}

fn goto_prev_diag(cx: &mut Context) {
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        let cursor_pos = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diag = doc
            .diagnostics()
            .iter()
            .rev()
            .find(|diag| diag.range.start < cursor_pos);

        let selection = match diag {
            // NOTE: the selection is reversed because we're jumping to the
            // previous diagnostic.
            Some(diag) => Selection::single(diag.range.end, diag.range.start),
            None => return,
        };
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    };
    cx.editor.apply_motion(motion)
}

fn spelling_ranges(doc: &Document) -> impl DoubleEndedIterator<Item = Range> + '_ {
    doc.diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.provider == DiagnosticProvider::Spelling)
        .map(|diagnostic| Range::new(diagnostic.range.start, diagnostic.range.end))
}

fn goto_next_spelling(cx: &mut Context) {
    goto_spelling(cx, Direction::Forward);
}

fn goto_prev_spelling(cx: &mut Context) {
    goto_spelling(cx, Direction::Backward);
}

fn goto_spelling(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    cx.editor.apply_motion(move |editor| {
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id).clone().transform(|range| {
            let cursor = range.cursor(text);
            // Exclude the finding under the cursor, including when it is already selected.
            // Taking the last available target also clamps counts at the document's boundaries.
            let target = match direction {
                Direction::Forward => spelling_ranges(doc)
                    .filter(|target| target.from() > cursor)
                    .take(count)
                    .last(),
                Direction::Backward => spelling_ranges(doc)
                    .rev()
                    .filter(|target| target.to() <= cursor)
                    .take(count)
                    .last(),
            };
            let Some(target) = target else {
                return range;
            };
            if editor.mode == Mode::Select {
                let head = if target.to() <= range.anchor {
                    target.from()
                } else {
                    target.to()
                };
                Range::new(range.anchor, head)
            } else {
                target.with_direction(direction)
            }
        });
        if selection == *doc.selection(view.id) {
            return;
        }
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    });
}

fn goto_next_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Forward, false);
}

fn goto_prev_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Backward, false);
}

fn goto_next_file_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Forward, true);
}

fn goto_prev_file_quicklist(cx: &mut Context) {
    goto_quicklist_impl(cx, Direction::Backward, true);
}

fn goto_quicklist_impl(cx: &mut Context, direction: Direction, same_file: bool) {
    let view_id = cx.editor.tree.focus;
    let jumped = match direction {
        Direction::Forward => cx
            .editor
            .jump_next_quicklist(view_id, cx.count(), same_file),
        Direction::Backward => cx
            .editor
            .jump_prev_quicklist(view_id, cx.count(), same_file),
    };

    if !jumped {
        let message = if same_file {
            "No quicklist entries available in the current file"
        } else {
            "No quicklist entries available"
        };
        cx.editor.set_error(|| message);
    }
}

fn goto_first_change(cx: &mut Context) {
    goto_first_change_impl(cx, false);
}

fn goto_last_change(cx: &mut Context) {
    goto_first_change_impl(cx, true);
}

fn goto_first_change_impl(cx: &mut Context, reverse: bool) {
    let editor = &mut cx.editor;
    let (view, doc) = current!(editor);
    if let Some(handle) = doc.diff_handle() {
        let hunk = {
            let diff = handle.load();
            let idx = if reverse {
                diff.len().saturating_sub(1)
            } else {
                0
            };
            diff.nth_hunk(idx)
        };
        if hunk != Hunk::NONE {
            let range = hunk_range(hunk, doc.text().slice(..));
            push_jump(view, doc);
            doc.set_selection(view.id, Selection::single(range.anchor, range.head));
        }
    }
}

fn goto_next_change(cx: &mut Context) {
    goto_next_change_impl(cx, Direction::Forward)
}

fn goto_prev_change(cx: &mut Context) {
    goto_next_change_impl(cx, Direction::Backward)
}

fn goto_next_change_impl(cx: &mut Context, direction: Direction) {
    let count = cx.count() as u32 - 1;
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        let doc_text = doc.text().slice(..);
        let diff_handle = if let Some(diff_handle) = doc.diff_handle() {
            diff_handle
        } else {
            editor.set_status("Diff is not available in current buffer");
            return;
        };

        let selection = doc.selection(view.id).clone().transform(|range| {
            let cursor_line = range.cursor_line(doc_text) as u32;

            let diff = diff_handle.load();
            let hunk_idx = match direction {
                Direction::Forward => diff
                    .next_hunk(cursor_line)
                    .map(|idx| (idx + count).min(diff.len() - 1)),
                Direction::Backward => diff
                    .prev_hunk(cursor_line)
                    .map(|idx| idx.saturating_sub(count)),
            };
            let Some(hunk_idx) = hunk_idx else {
                return range;
            };
            let hunk = diff.nth_hunk(hunk_idx);
            let new_range = hunk_range(hunk, doc_text);
            if editor.mode == Mode::Select {
                let head = if new_range.head < range.anchor {
                    new_range.anchor
                } else {
                    new_range.head
                };

                Range::new(range.anchor, head)
            } else {
                new_range.with_direction(direction)
            }
        });

        push_jump(view, doc);
        doc.set_selection(view.id, selection)
    };
    cx.editor.apply_motion(motion);
}

/// Returns the [Range] for a [Hunk] in the given text.
/// Additions and modifications cover the added and modified ranges.
/// Deletions are represented as the point at the start of the deletion hunk.
fn hunk_range(hunk: Hunk, text: RopeSlice) -> Range {
    let anchor = text.line_to_char(hunk.after.start as usize);
    let head = if hunk.after.is_empty() {
        anchor + 1
    } else {
        text.line_to_char(hunk.after.end as usize)
    };

    Range::new(anchor, head)
}

static LINE_ENDING_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n|\r|\n").unwrap());

fn format_selections(cx: &mut Context) {
    use lsp_client::{lsp, util::range_to_lsp_range};

    let (view, doc) = current!(cx.editor);
    let view_id = view.id;

    // via lsp if available
    // TODO: else via tree-sitter indentation calculations

    if doc.selection(view_id).len() != 1 {
        cx.editor
            .set_error(|| "format_selections only supports a single selection for now");
        return;
    }

    // TODO extra LanguageServerFeature::FormatSelections?
    // maybe such that LanguageServerFeature::Format contains it as well
    let Some(language_server) = doc
        .language_servers_with_feature(LanguageServerFeature::Format)
        .find(|ls| {
            matches!(
                ls.capabilities().document_range_formatting_provider,
                Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
            )
        })
    else {
        cx.editor
            .set_error(|| "No configured language server supports range formatting");
        return;
    };

    let offset_encoding = language_server.offset_encoding();
    let ranges: Vec<lsp::Range> = doc
        .selection(view_id)
        .iter()
        .map(|range| range_to_lsp_range(doc.text(), *range, offset_encoding))
        .collect();

    // TODO: handle fails
    // TODO: concurrent map over all ranges

    let range = ranges[0];

    let future = language_server
        .text_document_range_formatting(
            doc.identifier(),
            range,
            lsp::FormattingOptions {
                tab_size: doc.tab_width() as u32,
                insert_spaces: matches!(doc.indent_style, IndentStyle::Spaces(_)),
                ..Default::default()
            },
            None,
        )
        .unwrap();

    let text = doc.text().clone();
    let doc_id = doc.id();
    let doc_version = doc.version();

    tokio::spawn(async move {
        match future.await {
            Ok(Some(res)) => {
                let transaction =
                    lsp_client::util::generate_transaction_from_edits(&text, res, offset_encoding);
                job::dispatch(move |editor, _compositor| {
                    let Some(doc) = editor.document_mut(doc_id) else {
                        return;
                    };
                    // Updating a desynced document causes problems with applying the transaction
                    if doc.version() != doc_version {
                        return;
                    }
                    doc.apply(&transaction, view_id);
                })
                .await
            }
            Err(err) => log::error!("format sections failed: {err}"),
            Ok(None) => (),
        }
    });
}

pub fn completion(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let range = doc.selection(view.id).primary();
    let text = doc.text().slice(..);
    let cursor = range.cursor(text);

    cx.editor
        .handlers
        .trigger_completions(cursor, doc.id(), view.id);
}

// tree sitter node selection

fn expand_selection(cx: &mut Context) {
    let motion = |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        if let Some(syntax) = doc.syntax() {
            let text = doc.text().slice(..);

            let current_selection = doc.selection(view.id);
            let selection = object::expand_selection(syntax, text, current_selection.clone());

            // check if selection is different from the last one
            if *current_selection != selection {
                // save current selection so it can be restored using shrink_selection
                view.object_selections.push(current_selection.clone());

                doc.set_selection(view.id, selection);
            }
        }
    };
    cx.editor.apply_motion(motion);
}

fn shrink_selection(cx: &mut Context) {
    let motion = |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        let current_selection = doc.selection(view.id);
        // try to restore previous selection
        if let Some(prev_selection) = view.object_selections.pop() {
            if current_selection.contains(&prev_selection) {
                doc.set_selection(view.id, prev_selection);
                return;
            } else {
                // clear existing selection as they can't be shrunk to anyway
                view.object_selections.clear();
            }
        }
        // if not previous selection, shrink to first child
        if let Some(syntax) = doc.syntax() {
            let text = doc.text().slice(..);
            let selection = object::shrink_selection(syntax, text, current_selection.clone());
            doc.set_selection(view.id, selection);
        }
    };
    cx.editor.apply_motion(motion);
}

fn select_sibling_impl<F>(cx: &mut Context, sibling_fn: F)
where
    F: Fn(&editor_core::Syntax, RopeSlice, Selection) -> Selection + 'static,
{
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        if let Some(syntax) = doc.syntax() {
            let text = doc.text().slice(..);
            let current_selection = doc.selection(view.id);
            let selection = sibling_fn(syntax, text, current_selection.clone());
            doc.set_selection(view.id, selection);
        }
    };
    cx.editor.apply_motion(motion);
}

fn select_next_sibling(cx: &mut Context) {
    select_sibling_impl(cx, object::select_next_sibling)
}

fn select_prev_sibling(cx: &mut Context) {
    select_sibling_impl(cx, object::select_prev_sibling)
}

fn move_node_bound_impl(cx: &mut Context, dir: Direction, movement: Movement) {
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);

        if let Some(syntax) = doc.syntax() {
            let text = doc.text().slice(..);
            let current_selection = doc.selection(view.id);

            let selection = core_movement::move_parent_node_end(
                syntax,
                text,
                current_selection.clone(),
                dir,
                movement,
            );

            doc.set_selection(view.id, selection);
        }
    };

    cx.editor.apply_motion(motion);
}

pub fn move_parent_node_end(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Forward, Movement::Move)
}

pub fn move_parent_node_start(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Backward, Movement::Move)
}

pub fn extend_parent_node_end(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Forward, Movement::Extend)
}

pub fn extend_parent_node_start(cx: &mut Context) {
    move_node_bound_impl(cx, Direction::Backward, Movement::Extend)
}

fn select_all_impl<F>(editor: &mut Editor, select_fn: F)
where
    F: Fn(&Syntax, RopeSlice, Selection) -> Selection,
{
    let (view, doc) = current!(editor);

    if let Some(syntax) = doc.syntax() {
        let text = doc.text().slice(..);
        let current_selection = doc.selection(view.id);
        let selection = select_fn(syntax, text, current_selection.clone());
        doc.set_selection(view.id, selection);
    }
}

fn select_all_siblings(cx: &mut Context) {
    let motion = |editor: &mut Editor| {
        select_all_impl(editor, object::select_all_siblings);
    };

    cx.editor.apply_motion(motion);
}

fn select_all_children(cx: &mut Context) {
    let motion = |editor: &mut Editor| {
        select_all_impl(editor, object::select_all_children);
    };

    cx.editor.apply_motion(motion);
}

fn match_brackets(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let is_select = cx.editor.mode == Mode::Select;
    let text = doc.text();
    let text_slice = text.slice(..);

    let selection = doc.selection(view.id).clone().transform(|range| {
        let pos = range.cursor(text_slice);
        if let Some(matched_pos) = doc.syntax().map_or_else(
            || match_brackets::find_matching_bracket_plaintext(text.slice(..), pos),
            |syntax| match_brackets::find_matching_bracket_fuzzy(syntax, text.slice(..), pos),
        ) {
            range.put_cursor(text_slice, matched_pos, is_select)
        } else {
            range
        }
    });

    doc.set_selection(view.id, selection);
}

//

fn jump_forward(cx: &mut Context) {
    cx.editor.jump_forward(cx.editor.tree.focus, cx.count());
}

fn jump_backward(cx: &mut Context) {
    cx.editor.jump_backward(cx.editor.tree.focus, cx.count());
}

fn save_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    push_jump(view, doc);
    cx.editor.set_status("Selection saved to jumplist");
}

fn rotate_view(cx: &mut Context) {
    cx.editor.focus_next()
}

fn rotate_view_reverse(cx: &mut Context) {
    cx.editor.focus_prev()
}

fn jump_view_right(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Right)
}

fn jump_view_left(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Left)
}

fn jump_view_up(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Up)
}

fn jump_view_down(cx: &mut Context) {
    cx.editor.focus_direction(tree::Direction::Down)
}

fn swap_view_right(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Right)
}

fn swap_view_left(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Left)
}

fn swap_view_up(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Up)
}

fn swap_view_down(cx: &mut Context) {
    cx.editor.swap_split_in_direction(tree::Direction::Down)
}

fn transpose_view(cx: &mut Context) {
    cx.editor.transpose_view()
}

/// Open a new split in the given direction specified by the action.
///
/// Maintain the current view (both the cursor's position and view in document).
fn split(editor: &mut Editor, action: Action) {
    let (view, doc) = current!(editor);
    let id = doc.id();
    let selection = doc.selection(view.id).clone();
    let offset = doc.view_offset(view.id);

    editor.switch(id, action);

    // match the selection in the previous view
    let (view, doc) = current!(editor);
    doc.set_selection(view.id, selection);
    // match the view scroll offset (switch doesn't handle this fully
    // since the selection is only matched after the split)
    doc.set_view_offset(view.id, offset);
}

fn hsplit(cx: &mut Context) {
    split(cx.editor, Action::HorizontalSplit);
}

fn hsplit_new(cx: &mut Context) {
    cx.editor.new_file(Action::HorizontalSplit);
}

fn vsplit(cx: &mut Context) {
    split(cx.editor, Action::VerticalSplit);
}

fn vsplit_new(cx: &mut Context) {
    cx.editor.new_file(Action::VerticalSplit);
}

fn wclose(cx: &mut Context) {
    if cx.editor.tree.views().count() == 1
        && let Err(err) = typed::buffers_remaining_impl(cx.editor)
    {
        cx.editor.set_error(|| err.to_string());
        return;
    }
    let view_id = view!(cx.editor).id;
    // close current split
    cx.editor.close(view_id);
}

fn wonly(cx: &mut Context) {
    let views = cx
        .editor
        .tree
        .views()
        .map(|(v, focus)| (v.id, focus))
        .collect::<Vec<_>>();
    for (view_id, focus) in views {
        if !focus {
            cx.editor.close(view_id);
        }
    }
}

fn goto_ts_object_impl(cx: &mut Context, object: &'static str, direction: Direction) {
    let count = cx.count();
    let motion = move |editor: &mut Editor| {
        let (view, doc) = current!(editor);
        let loader = editor.syn_loader.load();
        if let Some(syntax) = doc.syntax() {
            let text = doc.text().slice(..);

            let selection = doc.selection(view.id).clone().transform(|range| {
                let new_range = core_movement::goto_treesitter_object(
                    text, range, object, direction, syntax, &loader, count,
                );

                if editor.mode == Mode::Select {
                    let head = if new_range.head < range.anchor {
                        new_range.anchor
                    } else {
                        new_range.head
                    };

                    Range::new(range.anchor, head)
                } else {
                    new_range.with_direction(direction)
                }
            });

            push_jump(view, doc);
            doc.set_selection(view.id, selection);
        } else {
            editor.set_status("Syntax-tree is not available in current buffer");
        }
    };
    cx.editor.apply_motion(motion);
}

fn goto_next_function(cx: &mut Context) {
    goto_ts_object_impl(cx, "function", Direction::Forward)
}

fn goto_prev_function(cx: &mut Context) {
    goto_ts_object_impl(cx, "function", Direction::Backward)
}

fn goto_next_class(cx: &mut Context) {
    goto_ts_object_impl(cx, "class", Direction::Forward)
}

fn goto_prev_class(cx: &mut Context) {
    goto_ts_object_impl(cx, "class", Direction::Backward)
}

fn goto_next_parameter(cx: &mut Context) {
    goto_ts_object_impl(cx, "parameter", Direction::Forward)
}

fn goto_prev_parameter(cx: &mut Context) {
    goto_ts_object_impl(cx, "parameter", Direction::Backward)
}

fn goto_next_comment(cx: &mut Context) {
    goto_ts_object_impl(cx, "comment", Direction::Forward)
}

fn goto_prev_comment(cx: &mut Context) {
    goto_ts_object_impl(cx, "comment", Direction::Backward)
}

fn goto_next_test(cx: &mut Context) {
    goto_ts_object_impl(cx, "test", Direction::Forward)
}

fn goto_prev_test(cx: &mut Context) {
    goto_ts_object_impl(cx, "test", Direction::Backward)
}

fn goto_next_xml_element(cx: &mut Context) {
    goto_ts_object_impl(cx, "xml-element", Direction::Forward)
}

fn goto_prev_xml_element(cx: &mut Context) {
    goto_ts_object_impl(cx, "xml-element", Direction::Backward)
}

fn goto_next_entry(cx: &mut Context) {
    goto_ts_object_impl(cx, "entry", Direction::Forward)
}

fn goto_prev_entry(cx: &mut Context) {
    goto_ts_object_impl(cx, "entry", Direction::Backward)
}

fn select_textobject_around(cx: &mut Context) {
    select_textobject(cx, textobject::TextObject::Around);
}

fn select_textobject_inner(cx: &mut Context) {
    select_textobject(cx, textobject::TextObject::Inside);
}

fn select_all_treesitter_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
    obj_name: &str,
    syntax: Option<&Syntax>,
    loader: &editor_core::syntax::Loader,
) -> SmallVec<[Range; 1]> {
    let Some(syntax) = syntax else {
        return SmallVec::new();
    };

    let from = text.char_to_byte(range.from()) as u32;
    let to = text.char_to_byte(range.to()) as u32;
    // Syntax layer ranges use an inclusive end while editor ranges use an exclusive end.
    let end = if to > from { to - 1 } else { to };
    let layer = syntax.layer_for_byte_range(from, end);
    let root = syntax.tree_for_byte_range(from, end).root_node();
    let Some(textobject_query) = loader.textobject_query(syntax.layer(layer).language) else {
        return SmallVec::new();
    };

    let capture_name = format!("{obj_name}.{objtype}");
    // TODO(perf): Limit the query cursor to `from..to` once textobject queries support ranges.
    let Some(nodes) = textobject_query.capture_nodes(&capture_name, &root, text) else {
        return SmallVec::new();
    };

    nodes
        .filter_map(|node| {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            if start_byte > text.len_bytes() || end_byte > text.len_bytes() {
                return None;
            }

            let object_range =
                Range::new(text.byte_to_char(start_byte), text.byte_to_char(end_byte))
                    .with_direction(range.direction());

            range.contains_range(&object_range).then_some(object_range)
        })
        .collect()
}

fn select_all_word_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
    long: bool,
) -> SmallVec<[Range; 1]> {
    let mut ranges = SmallVec::new();
    let mut pos = range.from();

    while pos < range.to() {
        let object_range = textobject::textobject_word(text, Range::point(pos), objtype, 1, long)
            .with_direction(range.direction());

        if !object_range.is_empty() && range.contains_range(&object_range) {
            ranges.push(object_range);
            pos = object_range.to();
        } else {
            pos = next_grapheme_boundary(text, pos);
        }
    }

    ranges
}

fn select_all_paragraph_textobject_ranges(
    text: RopeSlice,
    range: Range,
    objtype: textobject::TextObject,
) -> SmallVec<[Range; 1]> {
    let mut ranges = SmallVec::new();
    let mut pos = range.from();

    while pos < range.to() {
        let object_range = textobject::textobject_paragraph(text, Range::point(pos), objtype, 1)
            .with_direction(range.direction());

        if !object_range.is_empty() && range.contains_range(&object_range) {
            ranges.push(object_range);
            pos = object_range.to();
        } else {
            pos = next_grapheme_boundary(text, pos);
        }
    }

    ranges
}

fn select_all_textobjects_ranges(
    editor: &mut Editor,
    objtype: textobject::TextObject,
    ch: char,
) -> Result<Option<Selection>, ()> {
    let (view, doc) = current!(editor);
    let loader = editor.syn_loader.load();
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone();
    let mut ranges = SmallVec::new();

    for range in selection {
        match ch {
            'w' => ranges.extend(select_all_word_textobject_ranges(
                text, range, objtype, false,
            )),
            'W' => ranges.extend(select_all_word_textobject_ranges(
                text, range, objtype, true,
            )),
            'p' => ranges.extend(select_all_paragraph_textobject_ranges(text, range, objtype)),
            't' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "class",
                doc.syntax(),
                &loader,
            )),
            'f' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "function",
                doc.syntax(),
                &loader,
            )),
            'a' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "parameter",
                doc.syntax(),
                &loader,
            )),
            'c' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "comment",
                doc.syntax(),
                &loader,
            )),
            'T' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "test",
                doc.syntax(),
                &loader,
            )),
            'e' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "entry",
                doc.syntax(),
                &loader,
            )),
            'x' => ranges.extend(select_all_treesitter_textobject_ranges(
                text,
                range,
                objtype,
                "xml-element",
                doc.syntax(),
                &loader,
            )),
            'g' => {
                let Some(diff_handle) = doc.diff_handle() else {
                    editor.set_status("Diff is not available in current buffer");
                    return Err(());
                };
                let diff = diff_handle.load();
                ranges.extend((0..diff.len()).filter_map(|idx| {
                    let hunk_range =
                        hunk_range(diff.nth_hunk(idx), text).with_direction(range.direction());
                    range.contains_range(&hunk_range).then_some(hunk_range)
                }));
            }
            'd' => {
                ranges.extend(doc.diagnostics().iter().filter_map(|diagnostic| {
                    let diagnostic_range = Range::new(diagnostic.range.start, diagnostic.range.end)
                        .with_direction(range.direction());
                    range
                        .contains_range(&diagnostic_range)
                        .then_some(diagnostic_range)
                }));
            }
            's' => ranges.extend(
                spelling_ranges(doc)
                    .filter(|finding| range.contains_range(finding))
                    .map(|finding| finding.with_direction(range.direction())),
            ),
            _ => {}
        }
    }

    Ok((!ranges.is_empty()).then(|| Selection::new(ranges, 0)))
}

fn select_all_textobjects(cx: &mut Context, objtype: textobject::TextObject) {
    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        let Some(ch) = event.char() else {
            return;
        };

        match select_all_textobjects_ranges(cx.editor, objtype, ch) {
            Ok(Some(selection)) => {
                let (view, doc) = current!(cx.editor);
                doc.set_selection(view.id, selection);
            }
            Ok(None) => cx.editor.set_error(|| "nothing selected"),
            Err(()) => {}
        }
    });

    let title = match objtype {
        textobject::TextObject::Inside => "Select all inside",
        textobject::TextObject::Around => "Select all around",
        _ => return,
    };
    let help_text = [
        ("w", "Word"),
        ("W", "WORD"),
        ("p", "Paragraph"),
        ("t", "Type definition (tree-sitter)"),
        ("f", "Function (tree-sitter)"),
        ("a", "Argument/parameter (tree-sitter)"),
        ("c", "Comment (tree-sitter)"),
        ("T", "Test (tree-sitter)"),
        ("e", "Data structure entry (tree-sitter)"),
        ("g", "Change"),
        ("d", "Diagnostic"),
        ("s", "Spelling finding"),
        ("x", "(X)HTML element (tree-sitter)"),
    ];
    cx.editor.autoinfo = Some(Info::new(title, &help_text));
}

fn select_all_textobjects_around(cx: &mut Context) {
    select_all_textobjects(cx, textobject::TextObject::Around);
}

fn select_all_textobjects_inner(cx: &mut Context) {
    select_all_textobjects(cx, textobject::TextObject::Inside);
}

fn select_textobject(cx: &mut Context, objtype: textobject::TextObject) {
    let count = cx.count();

    cx.on_next_key(move |cx, event| {
        cx.editor.autoinfo = None;
        if let Some(ch) = event.char() {
            let textobject = move |editor: &mut Editor| {
                let (view, doc) = current!(editor);
                let loader = editor.syn_loader.load();
                let text = doc.text().slice(..);

                let textobject_treesitter = |obj_name: &str, range: Range| -> Range {
                    let Some(syntax) = doc.syntax() else {
                        return range;
                    };
                    textobject::textobject_treesitter(
                        text, range, objtype, obj_name, syntax, &loader, count,
                    )
                };

                if ch == 'g' && doc.diff_handle().is_none() {
                    editor.set_status("Diff is not available in current buffer");
                    return;
                }

                let textobject_change = |range: Range| -> Range {
                    let diff_handle = doc.diff_handle().unwrap();
                    let diff = diff_handle.load();
                    let line = range.cursor_line(text);
                    let hunk_idx = if let Some(hunk_idx) = diff.hunk_at(line as u32, false) {
                        hunk_idx
                    } else {
                        return range;
                    };
                    let hunk = diff.nth_hunk(hunk_idx).after;

                    let start = text.line_to_char(hunk.start as usize);
                    let end = text.line_to_char(hunk.end as usize);
                    Range::new(start, end).with_direction(range.direction())
                };

                let selection = doc.selection(view.id).clone().transform(|range| {
                    match ch {
                        'w' => textobject::textobject_word(text, range, objtype, count, false),
                        'W' => textobject::textobject_word(text, range, objtype, count, true),
                        't' => textobject_treesitter("class", range),
                        'f' => textobject_treesitter("function", range),
                        'a' => textobject_treesitter("parameter", range),
                        'c' => textobject_treesitter("comment", range),
                        'T' => textobject_treesitter("test", range),
                        'e' => textobject_treesitter("entry", range),
                        'x' => textobject_treesitter("xml-element", range),
                        'p' => textobject::textobject_paragraph(text, range, objtype, count),
                        'm' => textobject::textobject_pair_surround_closest(
                            doc.syntax(),
                            text,
                            range,
                            objtype,
                            count,
                        ),
                        'g' => textobject_change(range),
                        's' => spelling_ranges(doc)
                            .find(|finding| finding.contains(range.cursor(text)))
                            .map_or(range, |finding| finding.with_direction(range.direction())),
                        // TODO: cancel new ranges if inconsistent surround matches across lines
                        ch if !ch.is_ascii_alphanumeric() => textobject::textobject_pair_surround(
                            doc.syntax(),
                            text,
                            range,
                            objtype,
                            ch,
                            count,
                        ),
                        _ => range,
                    }
                });
                doc.set_selection(view.id, selection);
            };
            cx.editor.apply_motion(textobject);
        }
    });

    let title = match objtype {
        textobject::TextObject::Inside => "Match inside",
        textobject::TextObject::Around => "Match around",
        _ => return,
    };
    let help_text = [
        ("w", "Word"),
        ("W", "WORD"),
        ("p", "Paragraph"),
        ("t", "Type definition (tree-sitter)"),
        ("f", "Function (tree-sitter)"),
        ("a", "Argument/parameter (tree-sitter)"),
        ("c", "Comment (tree-sitter)"),
        ("T", "Test (tree-sitter)"),
        ("e", "Data structure entry (tree-sitter)"),
        ("m", "Closest surrounding pair (tree-sitter)"),
        ("g", "Change"),
        ("s", "Spelling finding"),
        ("x", "(X)HTML element (tree-sitter)"),
        (" ", "... or any character acting as a pair"),
    ];

    cx.editor.autoinfo = Some(Info::new(title, &help_text));
}

fn suspend(_cx: &mut Context) {
    #[cfg(not(windows))]
    {
        // SAFETY: These are calls to standard POSIX functions.
        // Unsafe is necessary since we are calling outside of Rust.
        let is_session_leader = unsafe { libc::getpid() == libc::getsid(0) };

        // If mitos is the session leader, there is nothing to suspend to, so skip
        if is_session_leader {
            return;
        }
        _cx.block_try_flush_writes().ok();
        signal_hook::low_level::raise(signal_hook::consts::signal::SIGTSTP).unwrap();
    }
}

fn goto_next_tabstop(cx: &mut Context) {
    goto_next_tabstop_impl(cx, Direction::Forward)
}

fn goto_prev_tabstop(cx: &mut Context) {
    goto_next_tabstop_impl(cx, Direction::Backward)
}

fn goto_next_tabstop_impl(cx: &mut Context, direction: Direction) {
    let (view, doc) = current!(cx.editor);
    let view_id = view.id;
    let Some(mut snippet) = doc.active_snippet.take() else {
        cx.editor.set_error(|| "no snippet is currently active");
        return;
    };
    let tabstop = match direction {
        Direction::Forward => Some(snippet.next_tabstop(doc.selection(view_id))),
        Direction::Backward => snippet
            .prev_tabstop(doc.selection(view_id))
            .map(|selection| (selection, false)),
    };
    let Some((selection, last_tabstop)) = tabstop else {
        return;
    };
    doc.set_selection(view_id, selection);
    if !last_tabstop {
        doc.active_snippet = Some(snippet)
    }
    if cx.editor.mode() == Mode::Insert {
        cx.on_next_key_fallback(|cx, key| {
            if let Some(c) = key.char() {
                let (view, doc) = current!(cx.editor);
                if let Some(snippet) = &doc.active_snippet {
                    doc.apply(&snippet.delete_placeholder(doc.text()), view.id);
                }
                insert_char(cx, c);
            }
        })
    }
}

fn record_macro(cx: &mut Context) {
    if let Some((reg, mut keys)) = cx.editor.macro_recording.take() {
        // Remove the keypress which ends the recording
        keys.pop();
        let s = keys
            .into_iter()
            .map(|key| {
                let s = key.to_string();
                if s.chars().count() == 1 {
                    s
                } else {
                    format!("<{}>", s)
                }
            })
            .collect::<String>();
        match cx.editor.registers.write(reg, vec![s]) {
            Ok(_) => cx
                .editor
                .set_status(format!("Recorded to register [{}]", reg)),
            Err(err) => cx.editor.set_error(|| err.to_string()),
        }
    } else {
        let reg = cx.register.take().unwrap_or('@');
        cx.editor.macro_recording = Some((reg, Vec::new()));
        cx.editor
            .set_status(format!("Recording to register [{}]", reg));
    }
}

fn replay_macro(cx: &mut Context) {
    let reg = cx.register.unwrap_or('@');

    if cx.editor.macro_replaying.contains(&reg) {
        cx.editor.set_error(|| {
            format!(
                "Cannot replay from register [{}] because already replaying from same register",
                reg
            )
        });
        return;
    }

    let keys: Vec<KeyEvent> = if let Some(keys) = cx
        .editor
        .registers
        .read(reg, cx.editor)
        .filter(|values| values.len() == 1)
        .map(|mut values| values.next().unwrap())
    {
        match input::parse_macro(&keys) {
            Ok(keys) => keys,
            Err(err) => {
                cx.editor.set_error(|| format!("Invalid macro: {}", err));
                return;
            }
        }
    } else {
        cx.editor.set_error(|| format!("Register [{}] empty", reg));
        return;
    };

    // Once the macro has been fully validated, it's marked as being under replay
    // to ensure we don't fall into infinite recursion.
    cx.editor.macro_replaying.push(reg);

    let count = cx.count();
    cx.callback.push(Box::new(move |compositor, cx| {
        for _ in 0..count {
            for &key in keys.iter() {
                compositor.handle_event(&compositor::Event::Key(key), cx);
            }
        }
        // The macro under replay is cleared at the end of the callback, not in the
        // macro replay context, or it will not correctly protect the user from
        // replaying recursively.
        cx.editor.macro_replaying.pop();
    }));
}

fn goto_word(cx: &mut Context) {
    jump_to_word(cx, Movement::Move)
}

fn extend_to_word(cx: &mut Context) {
    jump_to_word(cx, Movement::Extend)
}

fn jump_to_label(cx: &mut Context, labels: Vec<Range>, behaviour: Movement) {
    let doc = doc!(cx.editor);
    let alphabet = &cx.editor.config().jump_label_alphabet;
    if labels.is_empty() {
        return;
    }
    let alphabet_char = |i| {
        let mut res = Tendril::new();
        res.push(alphabet[i]);
        res
    };

    // Add label for each jump candidate to the View as virtual text.
    let text = doc.text().slice(..);
    let mut overlays: Vec<_> = labels
        .iter()
        .enumerate()
        .flat_map(|(i, range)| {
            [
                Overlay::new(range.from(), alphabet_char(i / alphabet.len())),
                Overlay::new(
                    graphemes::next_grapheme_boundary(text, range.from()),
                    alphabet_char(i % alphabet.len()),
                ),
            ]
        })
        .collect();
    overlays.sort_unstable_by_key(|overlay| overlay.char_idx);
    let (view, doc) = current!(cx.editor);
    doc.set_jump_labels(view.id, overlays);

    // Accept two characters matching a visible label. Jump to the candidate
    // for that label if it exists.
    let primary_selection = doc.selection(view.id).primary();
    let view_id = view.id;
    let doc = doc.id();
    cx.on_next_key(move |cx, event| {
        let alphabet = &cx.editor.config().jump_label_alphabet;
        let Some(i) = event
            .char()
            .filter(|_| event.modifiers.is_empty())
            .and_then(|ch| alphabet.iter().position(|&it| it == ch))
        else {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            return;
        };
        let outer = i * alphabet.len();
        // Bail if the given character cannot be a jump label.
        if outer > labels.len() {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            return;
        }
        cx.on_next_key(move |cx, event| {
            doc_mut!(cx.editor, &doc).remove_jump_labels(view_id);
            let alphabet = &cx.editor.config().jump_label_alphabet;
            let Some(inner) = event
                .char()
                .filter(|_| event.modifiers.is_empty())
                .and_then(|ch| alphabet.iter().position(|&it| it == ch))
            else {
                return;
            };
            if let Some(mut range) = labels.get(outer + inner).copied() {
                range = if behaviour == Movement::Extend {
                    let anchor = if range.anchor < range.head {
                        let from = primary_selection.from();
                        if range.anchor < from {
                            range.anchor
                        } else {
                            from
                        }
                    } else {
                        let to = primary_selection.to();
                        if range.anchor > to {
                            range.anchor
                        } else {
                            to
                        }
                    };
                    Range::new(anchor, range.head)
                } else {
                    range.with_direction(Direction::Forward)
                };
                let doc = doc_mut!(cx.editor, &doc);
                let view = view_mut!(cx.editor, view_id);
                push_jump(view, doc);
                doc.set_selection(view_id, range.into());
            }
        });
    });
}

fn jump_to_word(cx: &mut Context, behaviour: Movement) {
    // Calculate the jump candidates: ranges for any visible words with two or
    // more characters.
    let alphabet = &cx.editor.config().jump_label_alphabet;
    if alphabet.is_empty() {
        return;
    }

    let jump_label_limit = alphabet.len() * alphabet.len();
    let mut words = Vec::with_capacity(jump_label_limit);
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);

    // This is not necessarily exact if there is virtual text like soft wrap.
    // It's ok though because the extra jump labels will not be rendered.
    let start = text.line_to_char(text.char_to_line(doc.view_offset(view.id).anchor));
    let end = text.line_to_char(view.estimate_last_doc_line(doc) + 1);

    let primary_selection = doc.selection(view.id).primary();
    let cursor = primary_selection.cursor(text);
    let mut cursor_fwd = Range::point(cursor);
    let mut cursor_rev = Range::point(cursor);
    if text.get_char(cursor).is_some_and(|c| !c.is_whitespace()) {
        let cursor_word_end = core_movement::move_next_word_end(text, cursor_fwd, 1);
        //  single grapheme words need a special case
        if cursor_word_end.anchor == cursor {
            cursor_fwd = cursor_word_end;
        }
        let cursor_word_start = core_movement::move_prev_word_start(text, cursor_rev, 1);
        if cursor_word_start.anchor == next_grapheme_boundary(text, cursor) {
            cursor_rev = cursor_word_start;
        }
    }
    'outer: loop {
        let mut changed = false;
        while cursor_fwd.head < end {
            cursor_fwd = core_movement::move_next_word_end(text, cursor_fwd, 1);
            // The cursor is on a word that is atleast two graphemes long and
            // madeup of word characters. The latter condition is needed because
            // move_next_word_end simply treats a sequence of characters from
            // the same char class as a word so `=<` would also count as a word.
            let add_label = text
                .slice(..cursor_fwd.head)
                .graphemes_rev()
                .take(2)
                .take_while(|g| g.chars().all(char_is_word))
                .count()
                == 2;
            if !add_label {
                continue;
            }
            changed = true;
            // skip any leading whitespace
            cursor_fwd.anchor += text
                .chars_at(cursor_fwd.anchor)
                .take_while(|&c| !char_is_word(c))
                .count();
            words.push(cursor_fwd);
            if words.len() == jump_label_limit {
                break 'outer;
            }
            break;
        }
        while cursor_rev.head > start {
            cursor_rev = core_movement::move_prev_word_start(text, cursor_rev, 1);
            // The cursor is on a word that is atleast two graphemes long and
            // madeup of word characters. The latter condition is needed because
            // move_prev_word_start simply treats a sequence of characters from
            // the same char class as a word so `=<` would also count as a word.
            let add_label = text
                .slice(cursor_rev.head..)
                .graphemes()
                .take(2)
                .take_while(|g| g.chars().all(char_is_word))
                .count()
                == 2;
            if !add_label {
                continue;
            }
            changed = true;
            cursor_rev.anchor -= text
                .chars_at(cursor_rev.anchor)
                .reversed()
                .take_while(|&c| !char_is_word(c))
                .count();
            words.push(cursor_rev);
            if words.len() == jump_label_limit {
                break 'outer;
            }
            break;
        }
        if !changed {
            break;
        }
    }
    jump_to_label(cx, words, behaviour)
}

fn lsp_or_syntax_symbol_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);

    if doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
        .is_some()
    {
        lsp::symbol_picker(cx);
    } else if doc.syntax().is_some() || doc.is_syntax_pending() {
        syntax_symbol_picker(cx);
    } else {
        cx.editor.set_error(|| {
            "No language server supporting document symbols or syntax info available"
        });
    }
}

fn lsp_or_syntax_workspace_symbol_picker(cx: &mut Context) {
    let doc = doc!(cx.editor);

    if doc
        .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
        .next()
        .is_some()
    {
        lsp::workspace_symbol_picker(cx);
    } else {
        syntax_workspace_symbol_picker(cx);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use editor_core::Rope;

    use super::*;

    #[test]
    fn command_palette_styles_names_and_bindings() {
        let command_style = Style::default().fg(view::graphics::Color::Blue);
        let binding_style = Style::default().fg(view::graphics::Color::Magenta);
        let command = MappableCommand::Typable {
            name: "write".into(),
            args: String::new(),
            doc: String::new(),
        };
        let keys = vec![
            "space".parse::<KeyEvent>().unwrap(),
            "?".parse::<KeyEvent>().unwrap(),
        ];
        let data = CommandPaletteData {
            keymap: HashMap::from([("write".into(), vec![keys])]),
            command_style,
            binding_style,
        };

        let name = command_palette_name(&command, &data);
        let name = &name.content.lines[0].spans[0];
        assert_eq!(name.content, ":write");
        assert_eq!(name.style, tui::style::Style::from(command_style));

        let bindings = command_palette_bindings(&command, &data);
        let bindings = &bindings.content.lines[0].spans[0];
        assert_eq!(bindings.content, "<space>?");
        assert_eq!(bindings.style, tui::style::Style::from(binding_style));
    }

    #[test]
    fn global_search_match_selection_uses_match_range() {
        let text = Rope::from("héllo search term world\n");
        let start_col = "héllo ".chars().count();
        let end_col = "héllo search".chars().count();

        let selection =
            selection_for_global_search_match(text.slice(..), 0, start_col, 0, end_col).unwrap();

        assert_eq!(selection.primary().fragment(text.slice(..)), "search");
    }
}

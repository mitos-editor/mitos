//! Buffer and workspace search, prompts, and result selection.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    filter_picker_entry,
    ui::{self, overlay::overlaid, Picker, PickerColumn, PromptEvent},
};
use editor_core::{
    chars::char_is_word,
    graphemes,
    line_ending::line_end_char_index,
    movement::{Direction, Movement},
    regex, LineEnding, Range, RopeReader, RopeSlice, Selection,
};
use futures_util::FutureExt;
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks, BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};
use std::{borrow::Cow, collections::HashSet, path::Path};
use stdx::rope::{self, RopeSliceExt};
use view::{
    align_view,
    document::Mode,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, Editor,
};

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

pub(super) fn search(cx: &mut Context) {
    searcher(cx, Direction::Forward)
}

pub(super) fn rsearch(cx: &mut Context) {
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

pub(super) fn search_next(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Move, Direction::Forward);
}

pub(super) fn search_prev(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Move, Direction::Backward);
}

pub(super) fn extend_search_next(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Extend, Direction::Forward);
}

pub(super) fn extend_search_prev(cx: &mut Context) {
    search_next_or_prev_impl(cx, Movement::Extend, Direction::Backward);
}

pub(super) fn search_selection(cx: &mut Context) {
    search_selection_impl(cx, false)
}

pub(super) fn search_selection_detect_word_boundaries(cx: &mut Context) {
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

pub(super) fn make_search_word_bounded(cx: &mut Context) {
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

pub(super) fn global_search(cx: &mut Context) {
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

pub(super) fn selection_for_global_search_match(
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

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Rope;
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

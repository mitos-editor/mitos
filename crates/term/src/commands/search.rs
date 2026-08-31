//! Buffer and workspace search, prompts, and result selection.

use crate::{
    commands::{context::Context, picker::PathStyleConfig},
    ui::{self, overlay::overlaid, Picker, PickerColumn, PromptEvent},
};
use editor_core::{
    line_ending::line_end_char_index,
    movement::{Direction, Movement},
    LineEnding, RopeSlice, Selection,
};
use futures_util::FutureExt;
use std::{
    ops::ControlFlow,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use stdx::rope::RopeExt as _;
use view::{
    align_view,
    document::Mode,
    features::{
        buffer_search::Query,
        workspace_search::{Request as SearchRequest, WorkspaceSearch},
    },
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, Editor,
};

fn search_completions(cx: &mut Context, reg: Option<char>) -> Vec<String> {
    let mut items = reg
        .and_then(|reg| cx.editor.read_register(reg))
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
    let wrap_around = config.search.wrap_around;
    let movement = if cx.editor.mode() == Mode::Select {
        Movement::Extend
    } else {
        Movement::Move
    };

    // TODO: could probably share with select_on_matches?
    let completions = search_completions(cx, Some(reg));

    ui::search_prompt(
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
        move |cx, query, _, event| {
            if event == PromptEvent::Validate {
                cx.editor.registers.last_search_register = reg;
            } else if event != PromptEvent::Update {
                return;
            }
            cx.editor
                .search_buffer(&query, movement, direction, 1, wrap_around);
        },
    );
}

fn search_next_or_prev_impl(cx: &mut Context, movement: Movement, direction: Direction) {
    let count = cx.count();
    let register = cx
        .register
        .unwrap_or(cx.editor.registers.last_search_register);
    let config = cx.editor.config();
    if let Some(query) = cx.editor.first_register_value(register) {
        let search_config = &config.search;
        let wrap_around = search_config.wrap_around;
        let is_crlf = doc!(cx.editor).line_ending == LineEnding::Crlf;
        match Query::new(&query, search_config.smart_case, is_crlf) {
            Ok(query) => {
                match cx
                    .editor
                    .search_buffer(&query, movement, direction, count, wrap_around)
                {
                    view::editor::SearchOutcome::Found { wrapped: true } => {
                        cx.editor.set_status("Wrapped around document")
                    }
                    view::editor::SearchOutcome::NotFound => {
                        cx.editor.set_error(|| "No more matches")
                    }
                    view::editor::SearchOutcome::Found { wrapped: false } => {}
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
    let register = cx.register.unwrap_or('/');
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);

    let regex = editor_core::search::selection_pattern(
        text,
        doc.selection(view.id),
        detect_word_boundaries,
    );

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
    let regex = match cx.editor.first_register_value(register) {
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

struct GlobalSearchConfig {
    search: Arc<WorkspaceSearch>,
    case_sensitive: AtomicBool,
    style: PathStyleConfig,
}

impl GlobalSearchConfig {
    fn search_config(&self) -> Arc<WorkspaceSearch> {
        Arc::new(WorkspaceSearch {
            smart_case: !self.case_sensitive.load(Ordering::Relaxed),
            ..(*self.search).clone()
        })
    }
}

fn display_match(mut matched: ::search::Match) -> ::search::Match {
    matched.path = stdx::path::get_relative_path(matched.path).into_owned();
    matched
}

fn query_global_search(
    query: &str,
    editor: &mut Editor,
    config: Arc<GlobalSearchConfig>,
    injector: &ui::picker::Injector<::search::Match, GlobalSearchConfig>,
) -> futures_util::future::BoxFuture<'static, anyhow::Result<()>> {
    let request = SearchRequest::new(query, editor, config.search_config());
    editor.clear_status();
    if let Err(error) = &request {
        editor.set_error(|| error.to_string());
    }
    let injector = injector.clone();
    async move {
        let Some(request) = request? else {
            return Ok(());
        };
        let cancelled = injector.clone();
        request
            .run(
                move || cancelled.is_shutdown(),
                move |matched| match injector.push(display_match(matched)) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                },
            )
            .await
    }
    .boxed()
}

fn refresh_global_search(
    query: &str,
    editor: &Editor,
    config: Arc<GlobalSearchConfig>,
    cancel: event::TaskHandle,
) -> futures_util::future::BoxFuture<'static, anyhow::Result<Vec<::search::Match>>> {
    let request = SearchRequest::new(query, editor, config.search_config());
    async move {
        let Some(request) = request? else {
            return Ok(Vec::new());
        };
        Ok(request
            .collect(move || cancel.is_canceled())
            .await?
            .into_iter()
            .map(display_match)
            .collect())
    }
    .boxed()
}

pub(super) fn global_search(cx: &mut Context) {
    let reg = cx.register.unwrap_or('/');
    cx.editor.registers.last_search_register = reg;
    let picker = global_search_picker(cx.editor, cx.jobs, stdx::env::current_working_dir(), reg);
    cx.push_layer(Box::new(overlaid(picker)));
}

fn global_search_picker(
    editor: &mut Editor,
    jobs: &crate::job::Jobs,
    root: std::path::PathBuf,
    reg: char,
) -> Picker<::search::Match, GlobalSearchConfig> {
    use ::search::Match as FileResult;

    let config = editor.config();
    let config = GlobalSearchConfig {
        search: Arc::new(WorkspaceSearch {
            root: stdx::path::canonicalize(root),
            smart_case: config.search.smart_case,
            options: crate::files::picker_options(&config.file_picker),
        }),
        case_sensitive: AtomicBool::new(!config.search.smart_case),
        style: PathStyleConfig::new(editor),
    };

    let initial_case_sensitive = config.case_sensitive.load(Ordering::Relaxed);
    let columns = [
        PickerColumn::new("path", |item: &FileResult, config: &GlobalSearchConfig| {
            config
                .style
                .stylize(Some(&item.path), Some(item.line_start))
        })
        .with_prefix(|item, config| config.style.icon(Some(&item.path))),
        PickerColumn::hidden("contents"),
    ];

    Picker::new(
        editor.redraw().clone(),
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
         }| { Some((path.as_path().into(), Some((*line_start, *line_end)))) },
    )
    .with_quicklist(|_editor, item| {
        Some(QuicklistEntry {
            target: QuicklistTarget::Path(item.path.clone()),
            position: QuicklistPosition::LineColRange {
                start_line: item.line_start,
                start_col: item.match_start_col,
                end_line: item.line_end,
                end_col: item.match_end_col,
            },
        })
    })
    .with_replacement(|cx, items, query, replacement, replace_all, config| {
        let search = ::search::Search::new(query, !config.case_sensitive.load(Ordering::Relaxed))?;
        cx.editor
            .replace_workspace_matches(&search, items, replacement, replace_all)
    })
    .with_case_sensitive_toggle(initial_case_sensitive, |config, enabled| {
        config.case_sensitive.store(enabled, Ordering::Relaxed);
    })
    .with_title("Search")
    .with_history_register(Some(reg))
    .with_dynamic_query(query_global_search, Some(275), jobs)
    .with_file_updates(editor, |config| &config.search.root, refresh_global_search)
    .with_item_key(|item| (item.path.clone(), item.line_start, item.match_start_col))
    .refresh_on_content_changes()
    .refresh_on_document_changes()
}

pub(super) fn selection_for_global_search_match(
    text: RopeSlice,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> Option<Selection> {
    if start_line > end_line || end_line >= text.len_lines(stdx::rope::LINE_TYPE) {
        return None;
    }

    let start = text.line_to_char_idx(start_line).checked_add(start_col)?;
    let end = text.line_to_char_idx(end_line).checked_add(end_col)?;
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

    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn search_queries_and_refreshes_keep_open_buffers_authoritative() -> anyhow::Result<()> {
        use crate::{application::Application, config::Config};
        use std::sync::Arc;

        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.files.watch.enable = false;
        config.editor.files.reload.enable = false;
        let mut app = Application::new(
            Default::default(),
            config,
            language::syntax::Loader::new(
                language_config::Configuration {
                    language: vec![],
                    language_server: Default::default(),
                },
                Default::default(),
            )?,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let dir = tempfile::tempdir()?;
        let root = stdx::path::canonicalize(dir.path());
        let path = root.join("file.txt");
        std::fs::write(&path, "needle\nneedle\n")?;
        let config = Arc::new(GlobalSearchConfig {
            search: Arc::new(WorkspaceSearch {
                root,
                smart_case: true,
                options: ::search::Options::default(),
            }),
            case_sensitive: AtomicBool::new(false),
            style: PathStyleConfig::new(&app.editor),
        });
        let mut task = event::TaskController::new();
        let results =
            refresh_global_search("needle", &app.editor, config.clone(), task.restart()).await?;
        assert_eq!(results.len(), 2);
        std::fs::write(&path, "héllo other\n")?;
        assert!(
            refresh_global_search("needle", &app.editor, config.clone(), task.restart())
                .await?
                .is_empty()
        );
        app.editor.open(&path, view::editor::Action::Load)?;
        std::fs::write(&path, "needle on disk\n")?;
        let results =
            refresh_global_search("other", &app.editor, config.clone(), task.restart()).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_start, 0);
        assert_eq!(results[0].match_start_col, 6);
        let (mut matcher, injector) = Picker::stream(
            app.editor.redraw().clone(),
            [PickerColumn::new(
                "path",
                |item: &::search::Match, _: &GlobalSearchConfig| {
                    item.path.to_string_lossy().into_owned().into()
                },
            )],
            GlobalSearchConfig {
                search: config.search.clone(),
                case_sensitive: AtomicBool::new(false),
                style: PathStyleConfig::new(&app.editor),
            },
        );
        query_global_search("other", &mut app.editor, config.clone(), &injector).await?;
        query_global_search("", &mut app.editor, config.clone(), &injector).await?;
        assert!(
            query_global_search("[", &mut app.editor, config.clone(), &injector)
                .await
                .is_err()
        );
        assert_eq!(
            app.editor.get_status().unwrap().0.as_ref(),
            "Invalid regular expression"
        );
        let error = query_global_search("+=", &mut app.editor, config.clone(), &injector)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("repetition operator missing expression"));
        query_global_search("", &mut app.editor, config.clone(), &injector).await?;
        assert!(app.editor.get_status().is_none());
        drop(injector);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while matcher.tick(10).running {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(matcher.snapshot().item_count(), 1);
        assert_eq!(matcher.snapshot().get_item(0).unwrap().data, &results[0]);
        assert!(
            refresh_global_search("needle", &app.editor, config.clone(), task.restart())
                .await?
                .is_empty()
        );
        assert!(
            refresh_global_search("", &app.editor, config.clone(), task.restart())
                .await?
                .is_empty()
        );
        assert!(
            refresh_global_search("[", &app.editor, config.clone(), task.restart())
                .await
                .is_err()
        );
        let cancel = task.restart();
        task.cancel();
        assert!(refresh_global_search("other", &app.editor, config, cancel)
            .await?
            .is_empty());
        Ok(())
    }
}

#[cfg(all(test, feature = "integration"))]
#[path = "search/replacement_tests.rs"]
mod replacement_tests;

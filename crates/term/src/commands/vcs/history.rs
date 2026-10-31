//! Native history browsing. Pickers own reads; documents own selected immutable snapshots.
use crate::{
    compositor::Component,
    keymap::component::PickerItemAction,
    ui::{overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn},
};
use anyhow::Result;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tui::text::Span;
use vcs::operations::{
    history::{CommitEntry, HistoryCursor, HistoryFile, HistoryPage, RevisionFile},
    GitRepository,
};
use view::{editor::Action, features::change_review::HistoricalReviewRequest, DocumentId, Editor};

#[derive(Clone, PartialEq, Eq)]
enum Row {
    Commit(CommitEntry),
    More,
}
#[derive(Clone)]
struct Data {
    repo: GitRepository,
    buffer: Option<DocumentId>,
    theme: Arc<theme::Theme>,
}

pub(super) fn file_history(editor: &Editor) -> Result<Box<dyn Component>> {
    let (_, doc) = view::current_ref!(editor);
    let path = doc
        .display_metadata()
        .source_path
        .ok_or_else(|| anyhow::anyhow!("A file is required to browse history"))?
        .to_owned();
    let repo = GitRepository::discover(&path)?;
    let buffer = doc.path().map(|_| doc.id());
    browser(editor, repo, Some(path), buffer)
}

pub(super) fn repository_history(editor: &Editor) -> Result<Box<dyn Component>> {
    let (_, doc) = view::current_ref!(editor);
    let path = doc
        .display_metadata()
        .source_path
        .map_or_else(stdx::env::current_working_dir, |path| path.to_owned());
    browser(editor, GitRepository::discover(&path)?, None, None)
}

fn browser(
    editor: &Editor,
    repo: GitRepository,
    path: Option<PathBuf>,
    buffer: Option<DocumentId>,
) -> Result<Box<dyn Component>> {
    let cursor = repo.history_cursor(path.as_deref())?;
    let title = if path.is_some() {
        "File history · first parent · Enter review"
    } else {
        "Git history · Enter files"
    };
    let requested = Arc::new(AtomicBool::new(true));
    let more = requested.clone();
    let data = Data {
        repo: repo.clone(),
        buffer,
        theme: Arc::new(editor.theme.clone()),
    };
    let preview_repo = data.repo.clone();
    let accept = data.clone();
    let actions = data.clone();
    let mut pager = Pager {
        repo: repo.clone(),
        cursor,
        pending: None,
        rows: Vec::new(),
        requested,
        exhausted: false,
    };
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("commit", |row: &Row, data: &Data| match row {
                Row::Commit(c) => Span::styled(c.id.short(), data.theme.get("constant")).into(),
                Row::More => Span::styled("…", data.theme.get("ui.text.inactive")).into(),
            }),
            PickerColumn::new("date", |row: &Row, data| match row {
                Row::Commit(c) => Span::styled(
                    c.date.get(..10).unwrap_or(&c.date),
                    data.theme.get("ui.text.inactive"),
                )
                .into(),
                _ => "".into(),
            }),
            PickerColumn::new("author", |row: &Row, data| match row {
                Row::Commit(c) => {
                    Span::styled(c.author.as_str(), data.theme.get("string.special")).into()
                }
                _ => "".into(),
            }),
            PickerColumn::new("subject", |row: &Row, data| match row {
                Row::Commit(c) => c.subject.as_str().into(),
                Row::More => Span::styled(
                    "Load 100 older commits…",
                    data.theme.get("ui.text.inactive"),
                )
                .into(),
            }),
        ],
        3,
        [],
        data,
        move |cx, row, action| match row {
            Row::More => {
                more.store(true, Ordering::Relaxed);
                PickerCallbackResult::KeepOpen
            }
            Row::Commit(commit) => {
                if let Some(file) = &commit.file {
                    match review_request(cx.editor, &accept.repo, commit, file, 0) {
                        Ok(request) => {
                            cx.editor.open_historical_review(request, action);
                        }
                        Err(error) => {
                            cx.editor.set_error(|| error.to_string());
                            return PickerCallbackResult::KeepOpen;
                        }
                    }
                    PickerCallbackResult::Close
                } else {
                    let data = accept.clone();
                    let commit = commit.clone();
                    PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, cx| {
                        let parent = compositor.last_picker.take();
                        compositor.push(Box::new(overlaid(files_picker(
                            cx.editor, data, commit, 0, None, parent,
                        ))));
                    }))
                }
            }
        },
    )
    .with_title(title)
    .with_help(if path.is_some() {
        |maps| history_help(&maps.history_picker, true)
    } else {
        |maps| history_help(&maps.history_picker, false)
    })
    .with_item_key(|row| match row {
        Row::Commit(c) => c.id.as_str().to_owned(),
        Row::More => "more".into(),
    })
    .with_editor_updates(move |cx| pager.update(cx))
    .with_item_actions(
        |maps| &maps.history_picker,
        move |cx, row, action| {
            let Some(Row::Commit(commit)) = row else {
                return PickerCallbackResult::KeepOpen;
            };
            history_action(cx, &actions, commit, 0, action)
        },
    );
    let picker = if path.is_some() {
        picker.with_custom_preview(crate::ui::change_review::HistoricalReviewPreview::new(
            |row: &Row| match row {
                Row::Commit(commit) if commit.file.is_some() => Some(commit.id.clone()),
                _ => None,
            },
            move |editor, row| {
                let Row::Commit(commit) = row else {
                    anyhow::bail!("Select a commit to preview");
                };
                review_request(
                    editor,
                    &preview_repo,
                    commit,
                    commit.file.as_ref().unwrap(),
                    0,
                )
            },
        ))
    } else {
        picker
    };
    Ok(Box::new(overlaid(picker)))
}

struct Pager {
    repo: GitRepository,
    cursor: Option<HistoryCursor>,
    pending: Option<tokio::sync::oneshot::Receiver<Result<HistoryPage>>>,
    rows: Vec<Row>,
    requested: Arc<AtomicBool>,
    exhausted: bool,
}
impl Pager {
    fn update(&mut self, cx: crate::ui::picker::UpdateContext<'_, Row>) -> Option<Vec<Row>> {
        if cx.resumed && (self.pending.take().is_some() || self.rows.is_empty()) {
            self.requested.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = &mut self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(page) => {
                            self.rows.retain(|row| !matches!(row, Row::More));
                            self.rows.extend(page.commits.into_iter().map(Row::Commit));
                            self.cursor = page.next;
                            if self.rows.len() >= 5000 {
                                self.cursor = None;
                                cx.editor
                                    .set_status("History display limit reached (5000 commits)");
                            }
                            if self.cursor.is_some() {
                                self.rows.push(Row::More);
                            }
                            self.exhausted = self.cursor.is_none();
                            if self.rows.is_empty() {
                                cx.editor.set_status("No commits in this history");
                            }
                            return Some(self.rows.clone());
                        }
                        Err(error) => cx.editor.set_error(|| error.to_string()),
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.pending = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_none()
            && !self.exhausted
            && self.requested.swap(false, Ordering::Relaxed)
        {
            let Some(cursor) = self.cursor.clone() else {
                self.exhausted = true;
                cx.editor.set_status("No commits in this history");
                return Some(Vec::new());
            };
            let repo = self.repo.clone();
            let cancel = cx.task.restart();
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.pending = Some(rx);
            tokio::spawn(async move {
                if let Some(result) =
                    event::cancelable_future(repo.history_page(cursor), cancel).await
                {
                    let _ = tx.send(result);
                    redraw.request();
                }
            });
        }
        None
    }
}

fn review_request(
    editor: &Editor,
    repo: &GitRepository,
    commit: &CommitEntry,
    file: &HistoryFile,
    parent: usize,
) -> Result<HistoricalReviewRequest> {
    let (before, after) = file.endpoints(commit, parent)?;
    HistoricalReviewRequest::revisions(editor, repo.clone(), before, after)
}

fn history_help(
    keys: &ui_core::keymap::Keymap<crate::keymap::component::PickerAction>,
    file: bool,
) -> Vec<String> {
    use crate::keymap::component::{PickerAction as P, PickerItemAction as I};
    let mut actions = vec![(P::Accept, if file { "review" } else { "files" })];
    if file {
        actions.extend([
            (P::Item(I::HistoryOpenVersion), "open version"),
            (P::Item(I::HistoryCompareBuffer), "compare buffer"),
        ]);
    }
    actions.push((P::Item(I::HistoryCompareParent), "compare parent"));
    super::actions::action_hints(keys, &actions)
}

fn history_action<T, D>(
    cx: &mut crate::compositor::Context,
    data: &Data,
    commit: &CommitEntry,
    baseline: usize,
    action: PickerItemAction,
) -> PickerCallbackResult<T, D> {
    if action == PickerItemAction::HistoryCompareParent {
        let data = data.clone();
        let commit = commit.clone();
        return PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, cx| {
            compositor.push(Box::new(overlaid(parent_picker(
                cx.editor, data, commit, baseline,
            ))));
        }));
    }
    if !matches!(
        action,
        PickerItemAction::HistoryOpenVersion | PickerItemAction::HistoryCompareBuffer
    ) {
        return PickerCallbackResult::KeepOpen;
    }
    let result = (|| -> Result<()> {
        let path = commit
            .file
            .as_ref()
            .and_then(|file| file.after.as_ref())
            .ok_or_else(|| anyhow::anyhow!("Select a file that exists in this revision"))?;
        let endpoint = RevisionFile::new(commit.id.clone(), path.clone())?;
        if action == PickerItemAction::HistoryCompareBuffer {
            let buffer = data.buffer.ok_or_else(|| {
                anyhow::anyhow!("Open this file in an editable buffer before comparing")
            })?;
            let request = HistoricalReviewRequest::against_buffer(
                cx.editor,
                data.repo.clone(),
                endpoint,
                buffer,
            )?;
            cx.editor.open_historical_review(request, Action::Replace);
        } else {
            let request = HistoricalReviewRequest::revisions(
                cx.editor,
                data.repo.clone(),
                None,
                Some(endpoint),
            )?;
            cx.editor.open_historical_version(request, Action::Replace);
        }
        Ok(())
    })();
    match result {
        Ok(()) => PickerCallbackResult::Close,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            PickerCallbackResult::KeepOpen
        }
    }
}

fn parent_picker(
    editor: &Editor,
    data: Data,
    commit: CommitEntry,
    baseline: usize,
) -> Picker<usize, CommitEntry> {
    let title = format!("{} · compare parent", commit.id.short());
    let parents = std::iter::once(baseline)
        .chain((0..commit.parents.len()).filter(|parent| *parent != baseline))
        .collect::<Vec<_>>();
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [PickerColumn::new(
            "parent",
            |parent: &usize, commit: &CommitEntry| {
                if commit.parents.is_empty() {
                    "Review initial version".into()
                } else {
                    format!(
                        "Review against parent {} ({})",
                        parent + 1,
                        commit.parents[*parent].short()
                    )
                    .into()
                }
            },
        )],
        0,
        parents,
        commit.clone(),
        move |cx, parent, action| {
            if *parent == baseline
                && let Some(file) = &commit.file
            {
                match review_request(cx.editor, &data.repo, &commit, file, baseline) {
                    Ok(request) => {
                        cx.editor.open_historical_review(request, action);
                    }
                    Err(error) => {
                        cx.editor.set_error(|| error.to_string());
                        return PickerCallbackResult::KeepOpen;
                    }
                }
                return PickerCallbackResult::CloseWithCallback(Box::new(|compositor, _| {
                    compositor.last_picker = compositor.pop();
                }));
            }
            let data = data.clone();
            let commit = commit.clone();
            let parent = *parent;
            PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, cx| {
                compositor.last_picker = compositor.pop();
                let path = commit.file.as_ref().map(|file| file.path().to_owned());
                let browser = compositor.last_picker.take();
                compositor.push(Box::new(overlaid(files_picker(
                    cx.editor, data, commit, parent, path, browser,
                ))));
            }))
        },
    )
    .with_title(title)
}

#[derive(Clone)]
struct Files {
    data: Data,
    commit: CommitEntry,
    parent: usize,
    path: Option<PathBuf>,
}
fn files_picker(
    editor: &Editor,
    data: Data,
    commit: CommitEntry,
    parent: usize,
    path: Option<PathBuf>,
    browser: Option<Box<dyn Component>>,
) -> Picker<HistoryFile, Files> {
    let browser = Rc::new(RefCell::new(browser));
    let title = format!(
        "{} · parent {} · Enter review",
        commit.id.short(),
        parent + 1
    );
    let preview_repo = data.repo.clone();
    let preview_commit = commit.clone();
    let accept = data.clone();
    let selected = commit.clone();
    let action_data = data.clone();
    let action_commit = commit.clone();
    let load = Files {
        data: data.clone(),
        commit: commit.clone(),
        parent,
        path: path.clone(),
    };
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("change", |file: &HistoryFile, data: &Files| {
                let scope = match file.status.as_bytes().first() {
                    Some(b'A') => "diff.plus",
                    Some(b'D') => "diff.minus",
                    Some(b'R' | b'C') => "diff.delta.moved",
                    Some(b'M' | b'T') => "diff.delta",
                    _ => "ui.text",
                };
                Span::styled(file.status.as_str(), data.data.theme.get(scope)).into()
            }),
            PickerColumn::new("path", |file: &HistoryFile, _| {
                file.path().to_string_lossy().into_owned().into()
            }),
        ],
        1,
        [],
        Files {
            data,
            commit,
            parent,
            path,
        },
        move |cx, file, action| match review_request(
            cx.editor,
            &accept.repo,
            &selected,
            file,
            parent,
        ) {
            Ok(request) => {
                cx.editor.open_historical_review(request, action);
                PickerCallbackResult::Close
            }
            Err(error) => {
                cx.editor.set_error(|| error.to_string());
                PickerCallbackResult::KeepOpen
            }
        },
    )
    .with_custom_preview(crate::ui::change_review::HistoricalReviewPreview::new(
        |file: &HistoryFile| Some(file.clone()),
        move |editor, file| review_request(editor, &preview_repo, &preview_commit, file, parent),
    ))
    .with_title(title)
    .with_help(|maps| {
        let mut hints = history_help(&maps.history_picker, true);
        hints.push(format!(
            "{} back",
            crate::keymap::component::action_hint(
                &maps.history_picker,
                &crate::keymap::component::PickerAction::Item(PickerItemAction::HistoryBack)
            )
        ));
        hints
    })
    .with_item_actions(
        |maps| &maps.history_picker,
        move |cx, file, action| {
            if action == PickerItemAction::HistoryBack {
                if browser.borrow().is_none() {
                    return PickerCallbackResult::KeepOpen;
                }
                let browser = browser.clone();
                return PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, _| {
                    if let Some(parent) = browser.borrow_mut().take() {
                        compositor.push(parent);
                    }
                }));
            }
            let Some(file) = file else {
                return PickerCallbackResult::KeepOpen;
            };
            let mut data = action_data.clone();
            let mut commit = action_commit.clone();
            commit.file = Some(file.clone());
            if data.buffer.is_none() {
                let path = data.repo.root().join(commit.file.as_ref().unwrap().path());
                data.buffer = cx.editor.document_by_path(&path).map(|doc| doc.id());
            }
            history_action(cx, &data, &commit, parent, action)
        },
    )
    .with_initial_options(move |cancel| {
        let data = load.clone();
        Box::pin(async move {
            let files = event::cancelable_future(
                data.data.repo.commit_files(&data.commit, data.parent),
                cancel,
            )
            .await
            .ok_or_else(|| anyhow::anyhow!("History file browser closed"))??;
            Ok(files
                .into_iter()
                .filter(|file| {
                    data.path.as_ref().is_none_or(|path| {
                        file.after.as_ref() == Some(path) || file.before.as_ref() == Some(path)
                    })
                })
                .collect())
        })
    })
}

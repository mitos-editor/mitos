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
use vcs::operations::{
    history::{CommitEntry, HistoryCursor, HistoryFile, HistoryPage, RevisionFile},
    GitRepository,
};
use view::{features::change_review::HistoricalReviewRequest, DocumentId, Editor};

#[derive(Clone, PartialEq, Eq)]
enum Row {
    Commit(CommitEntry),
    More,
}
#[derive(Clone)]
struct Data {
    repo: GitRepository,
    buffer: Option<DocumentId>,
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
            PickerColumn::new("commit", |row: &Row, _: &Data| match row {
                Row::Commit(c) => c.id.short().into(),
                Row::More => "…".into(),
            }),
            PickerColumn::new("date", |row: &Row, _| match row {
                Row::Commit(c) => c.date.get(..10).unwrap_or(&c.date).into(),
                _ => "".into(),
            }),
            PickerColumn::new("author", |row: &Row, _| match row {
                Row::Commit(c) => c.author.as_str().into(),
                _ => "".into(),
            }),
            PickerColumn::new("subject", |row: &Row, _| match row {
                Row::Commit(c) => c.subject.as_str().into(),
                Row::More => "Load 100 older commits…".into(),
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
    .with_help(|maps| super::actions::help(&maps.history_picker))
    .with_item_key(|row| match row {
        Row::Commit(c) => c.id.as_str().to_owned(),
        Row::More => "more".into(),
    })
    .with_editor_updates(move |cx| pager.update(cx))
    .with_item_actions(
        |maps| &maps.history_picker,
        move |_, row, action| {
            if action != PickerItemAction::Actions {
                return PickerCallbackResult::KeepOpen;
            }
            let Some(Row::Commit(commit)) = row else {
                return PickerCallbackResult::KeepOpen;
            };
            let data = actions.clone();
            let commit = commit.clone();
            PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, cx| {
                compositor.push(Box::new(overlaid(action_picker(
                    cx.editor, data, commit, 0,
                ))));
            }))
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

#[derive(Clone)]
enum ChoiceKind {
    Review(usize),
    Version,
    Buffer,
}
#[derive(Clone)]
struct Choice {
    label: String,
    kind: ChoiceKind,
}
fn action_picker(
    editor: &Editor,
    data: Data,
    commit: CommitEntry,
    baseline: usize,
) -> Picker<Choice, ()> {
    let title = format!("{} · {}", commit.id.short(), commit.subject);
    let mut choices = vec![Choice {
        label: if commit.parents.is_empty() {
            "Review initial version".into()
        } else {
            format!(
                "Review against parent {} ({})",
                baseline + 1,
                commit.parents[baseline].short()
            )
        },
        kind: ChoiceKind::Review(baseline),
    }];
    for parent in (0..commit.parents.len()).filter(|parent| *parent != baseline) {
        choices.push(Choice {
            label: format!(
                "Review against parent {} ({})",
                parent + 1,
                commit.parents[parent].short()
            ),
            kind: ChoiceKind::Review(parent),
        });
    }
    if commit
        .file
        .as_ref()
        .is_some_and(|file| file.after.is_some())
    {
        choices.push(Choice {
            label: "Open historical version".into(),
            kind: ChoiceKind::Version,
        });
        if data.buffer.is_some() {
            choices.push(Choice {
                label: "Compare revision with current buffer (includes unsaved edits)".into(),
                kind: ChoiceKind::Buffer,
            });
        }
    }
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [PickerColumn::new("action", |choice: &Choice, _: &()| {
            choice.label.as_str().into()
        })],
        0,
        choices,
        (),
        move |cx, choice, action| {
            let result = (|| -> Result<bool> {
                match choice.kind {
                    ChoiceKind::Review(parent) => {
                        if parent == baseline
                            && let Some(file) = &commit.file
                        {
                            cx.editor.open_historical_review(
                                review_request(cx.editor, &data.repo, &commit, file, baseline)?,
                                action,
                            );
                            Ok(false)
                        } else {
                            Ok(true)
                        }
                    }
                    ChoiceKind::Version | ChoiceKind::Buffer => {
                        let path = commit
                            .file
                            .as_ref()
                            .and_then(|file| file.after.as_ref())
                            .ok_or_else(|| {
                                anyhow::anyhow!("This file is deleted in the selected revision")
                            })?;
                        let endpoint = RevisionFile::new(commit.id.clone(), path.clone())?;
                        let request = match choice.kind {
                            ChoiceKind::Buffer => HistoricalReviewRequest::against_buffer(
                                cx.editor,
                                data.repo.clone(),
                                endpoint,
                                data.buffer.unwrap(),
                            )?,
                            _ => HistoricalReviewRequest::revisions(
                                cx.editor,
                                data.repo.clone(),
                                None,
                                Some(endpoint),
                            )?,
                        };
                        if matches!(choice.kind, ChoiceKind::Version) {
                            cx.editor.open_historical_version(request, action);
                        } else {
                            cx.editor.open_historical_review(request, action);
                        }
                        Ok(false)
                    }
                }
            })();
            match result {
                Err(error) => {
                    cx.editor.set_error(|| error.to_string());
                    PickerCallbackResult::KeepOpen
                }
                Ok(files) => {
                    let data = data.clone();
                    let commit = commit.clone();
                    let kind = choice.kind.clone();
                    PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, cx| {
                        // Replace the action picker cache with the originating commit browser.
                        compositor.last_picker = compositor.pop();
                        if files && let ChoiceKind::Review(parent) = kind {
                            let path = commit.file.as_ref().map(|file| file.path().to_owned());
                            let browser = compositor.last_picker.take();
                            compositor.push(Box::new(overlaid(files_picker(
                                cx.editor, data, commit, parent, path, browser,
                            ))));
                        }
                    }))
                }
            }
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
    let menu_data = data.clone();
    let menu_commit = commit.clone();
    let load = Files {
        data: data.clone(),
        commit: commit.clone(),
        parent,
        path: path.clone(),
    };
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("change", |file: &HistoryFile, _: &Files| {
                file.status.as_str().into()
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
        format!(
            "{} {} back ",
            super::actions::help(&maps.history_picker),
            crate::keymap::component::action_hint(
                &maps.history_picker,
                &crate::keymap::component::PickerAction::Item(PickerItemAction::HistoryBack)
            )
        )
    })
    .with_item_actions(
        |maps| &maps.history_picker,
        move |_, file, action| {
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
            if action != PickerItemAction::Actions {
                return PickerCallbackResult::KeepOpen;
            }
            let Some(file) = file else {
                return PickerCallbackResult::KeepOpen;
            };
            let data = menu_data.clone();
            let mut commit = menu_commit.clone();
            commit.file = Some(file.clone());
            PickerCallbackResult::KeepOpenWithCallback(Box::new(move |compositor, cx| {
                let mut data = data;
                if data.buffer.is_none() {
                    let path = data.repo.root().join(commit.file.as_ref().unwrap().path());
                    data.buffer = cx.editor.document_by_path(&path).map(|doc| doc.id());
                }
                compositor.push(Box::new(overlaid(action_picker(
                    cx.editor, data, commit, parent,
                ))));
            }))
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

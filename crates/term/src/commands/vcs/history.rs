//! Native history browsing. Pickers own reads; documents own selected immutable snapshots.
use crate::{
    compositor::Component,
    keymap::component::PickerItemAction,
    ui::{overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn},
};
use anyhow::Result;
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};
use tui::text::Span;
use vcs::operations::{
    history::{CommitEntry, HistoryFile, RevisionFile},
    GitRepository,
};
use view::{features::change_review::HistoricalReviewRequest, DocumentId, Editor};

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Data {
    repo: GitRepository,
    buffer: Option<DocumentId>,
    theme: Arc<theme::Theme>,
    paths: Arc<crate::commands::picker::PathStyleConfig>,
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
    Ok(Box::new(overlaid(browser(
        editor,
        repo,
        Some(path),
        buffer,
    )?)))
}

pub(super) fn repository_history(editor: &Editor) -> Result<Box<dyn Component>> {
    let (_, doc) = view::current_ref!(editor);
    let path = doc
        .display_metadata()
        .source_path
        .map_or_else(stdx::env::current_working_dir, |path| path.to_owned());
    Ok(Box::new(overlaid(browser(
        editor,
        GitRepository::discover(&path)?,
        None,
        None,
    )?)))
}

fn browser(
    editor: &Editor,
    repo: GitRepository,
    path: Option<PathBuf>,
    buffer: Option<DocumentId>,
) -> Result<Picker<Arc<CommitEntry>, Data>> {
    let cursor = repo.history_cursor(path.as_deref())?;
    let title = if path.is_some() {
        "File history · first parent · Enter review"
    } else {
        "Git history · Enter files"
    };
    let data = Data {
        repo: repo.clone(),
        buffer,
        theme: Arc::new(editor.theme.clone()),
        paths: Arc::new(crate::commands::picker::PathStyleConfig::new(editor)),
    };
    let preview_repo = data.repo.clone();
    let accept = data.clone();
    let actions = data.clone();
    let pager = crate::ui::picker::paged::Paged::new(
        cursor,
        [
            "More history available",
            "Loading history…",
            "Complete history",
        ],
        history_row_bytes,
        move |cursor, _cancel| {
            let repo = repo.clone();
            Box::pin(async move {
                let page = repo.history_page(cursor).await?;
                Ok(crate::ui::picker::paged::Page {
                    rows: page.commits,
                    next: page.next,
                })
            })
        },
    )
    .with_suspend(vcs::operations::history::HistoryCursor::suspend);
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("commit", |c: &Arc<CommitEntry>, data: &Data| {
                Span::styled(c.id.short(), data.theme.get("constant")).into()
            }),
            PickerColumn::new("committed", |c: &Arc<CommitEntry>, data: &Data| {
                Span::styled(
                    c.date.get(..10).unwrap_or(&c.date),
                    data.theme.get("ui.text.inactive"),
                )
                .into()
            }),
            PickerColumn::new("author", |c: &Arc<CommitEntry>, data: &Data| {
                Span::styled(c.author.as_str(), data.theme.get("string.special")).into()
            }),
            PickerColumn::new("subject", |c: &Arc<CommitEntry>, _: &Data| {
                c.subject.as_str().into()
            }),
        ],
        3,
        [],
        data,
        move |cx, commit, action| {
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
        },
    )
    .with_title(title)
    .with_help(|maps| super::actions::help(&maps.history_picker))
    .with_item_key(|c| c.id.as_str().to_owned())
    .with_paged_updates(pager)
    .with_item_actions(
        |maps| &maps.history_picker,
        move |_, row, action| {
            if action != PickerItemAction::Actions {
                return PickerCallbackResult::KeepOpen;
            }
            let Some(commit) = row else {
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
            |commit: &Arc<CommitEntry>| commit.file.as_ref().map(|_| commit.id.clone()),
            move |editor, commit| {
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
        picker.with_custom_preview(crate::ui::change_review::CommitReviewPreview::new(
            preview_repo,
        ))
    };
    Ok(picker)
}

/// Conservative retained metadata and matching-text estimate, excluding Git's
/// traversal state and object cache. Shared replay rows count only once.
fn history_row_bytes(row: &Arc<CommitEntry>) -> usize {
    let text = row.id.as_str().len()
        + row.author.len()
        + row.date.len()
        + row.subject.len()
        + row
            .parents
            .iter()
            .map(|id| id.as_str().len())
            .sum::<usize>()
        + row.file.as_ref().map_or(0, |file| {
            file.before
                .iter()
                .chain(&file.after)
                .map(|path| path.as_os_str().as_encoded_bytes().len())
                .sum::<usize>()
        });
    std::mem::size_of::<CommitEntry>() + 128 + text * 5
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
    commit: Arc<CommitEntry>,
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
    commit: Arc<CommitEntry>,
    parent: usize,
    path: Option<PathBuf>,
}
fn files_picker(
    editor: &Editor,
    data: Data,
    commit: Arc<CommitEntry>,
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
            PickerColumn::new("path", |file: &HistoryFile, data: &Files| {
                data.data.paths.stylize(Some(file.path()), None)
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
        let mut hints = super::actions::help(&maps.history_picker);
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
            Arc::make_mut(&mut commit).file = Some(file.clone());
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

//! Selection shell operations with document-owned cancellation and snapshot validation.

use std::{collections::HashMap, path::Path};

use editor_core::{Range, Rope, Selection, SmallVec, Tendril, Transaction};
use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use stdx::rope::RopeSliceExt;
use tokio::process::Command;

use super::request::SelectionRequest;
use crate::{
    events::{DocumentDidChange, DocumentFocusLost, SelectionDidChange},
    Editor, ViewId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBehavior {
    Replace,
    Ignore,
    Insert,
    Append,
    Keep,
}

#[derive(Default)]
pub(crate) struct ShellFilters {
    requests: HashMap<ViewId, TaskController>,
}

impl ShellFilters {
    fn restart(&mut self, view: ViewId) -> TaskHandle {
        self.requests.entry(view).or_default().restart()
    }

    pub(crate) fn clear(&mut self) {
        self.requests.clear();
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        self.requests.remove(&view);
    }
}

impl Editor {
    /// Selection shell operations still executing or waiting to apply their results.
    pub fn has_pending_shell_filters(&self) -> bool {
        self.documents.values().any(|doc| {
            doc.shell_filters
                .requests
                .values()
                .any(TaskController::is_running)
        })
    }

    /// Run a selection command without blocking the editor. A newer command in the same
    /// view replaces it; text, selection, or document/view changes discard its result.
    pub fn request_shell_filter(&mut self, view: ViewId, command: &str, behavior: ShellBehavior) {
        let Some(target) = self.tree.try_get(view) else {
            return;
        };
        let document = target.doc;
        let shell = self.config().shell.clone();
        let directory = stdx::env::current_working_dir();
        let callbacks = self.handlers.callbacks.clone();
        let owner = callbacks.clone();
        let doc = self.document_mut(document).unwrap();
        let cancel = doc.shell_filters.restart(view);
        let request = SelectionRequest::new(doc, view, cancel.clone(), vec![]);
        let text = doc.text().clone();
        let selection = doc.selection(view).clone();
        let command = command.to_owned();
        tokio::spawn(async move {
            let operation = filter(&shell, &command, &directory, &text, &selection, behavior);
            let Some(result) = cancelable_future(operation, &cancel).await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !owner.same_destination(&editor.handlers.callbacks)
                        || request.get(editor).is_none()
                    {
                        return;
                    }
                    let padding = editor.config().scroll.padding;
                    let view = editor.tree.get_mut(request.view_id());
                    let doc = editor.documents.get_mut(&request.document_id()).unwrap();
                    // Retire the request before editing dispatches selection/change hooks.
                    doc.shell_filters.remove_view(view.id);
                    match result {
                        Ok(FilterResult::Transaction(transaction)) => {
                            if crate::editing::apply(doc, &transaction, view.id) {
                                crate::editing::commit(doc, view);
                            }
                        }
                        Ok(FilterResult::Selection(selection)) => {
                            doc.set_selection(view.id, selection);
                        }
                        Ok(FilterResult::Ignored) => {}
                        Err(error) => {
                            editor.set_error(|| error.to_string());
                            return;
                        }
                    }
                    view.ensure_cursor_in_view(doc, padding);
                })
                .await;
        });
    }
}

enum FilterResult {
    Transaction(Transaction),
    Selection(Selection),
    Ignored,
}

async fn filter(
    shell: &[String],
    command: &str,
    directory: &Path,
    text: &Rope,
    selection: &Selection,
    behavior: ShellBehavior,
) -> anyhow::Result<FilterResult> {
    if behavior == ShellBehavior::Keep {
        let mut ranges = SmallVec::with_capacity(selection.len());
        let mut primary = None;
        for (index, range) in selection.ranges().iter().enumerate() {
            match run(
                shell,
                command,
                Some(range.slice(text.slice(..)).into()),
                directory,
            )
            .await
            {
                Ok(_) => {
                    ranges.push(*range);
                    if index >= selection.primary_index() && primary.is_none() {
                        primary = Some(ranges.len() - 1);
                    }
                }
                Err(error) => log::debug!("Shell command failed: {error}"),
            }
        }
        anyhow::ensure!(!ranges.is_empty(), "No selections remaining");
        let primary = primary.unwrap_or(ranges.len() - 1);
        return Ok(FilterResult::Selection(Selection::new(ranges, primary)));
    }

    let pipe = matches!(behavior, ShellBehavior::Replace | ShellBehavior::Ignore);
    let mut changes = Vec::with_capacity(selection.len());
    let mut ranges = SmallVec::with_capacity(selection.len());
    let mut shell_output: Option<Tendril> = None;
    let mut offset = 0isize;
    for range in selection.ranges() {
        let output = if let Some(output) = &shell_output {
            output.clone()
        } else {
            let input = range.slice(text.slice(..));
            let mut output = run(shell, command, pipe.then(|| input.into()), directory).await?;
            if !input.ends_with("\n") && output.ends_with('\n') {
                output.pop();
                if output.ends_with('\r') {
                    output.pop();
                }
            }
            if !pipe {
                shell_output = Some(output.clone());
            }
            output
        };
        if behavior == ShellBehavior::Ignore {
            continue;
        }
        let output_len = output.chars().count();
        let (from, to, deleted_len) = match behavior {
            ShellBehavior::Replace => (range.from(), range.to(), range.len()),
            ShellBehavior::Insert => (range.from(), range.from(), 0),
            ShellBehavior::Append => (range.to(), range.to(), 0),
            ShellBehavior::Ignore | ShellBehavior::Keep => unreachable!(),
        };
        // Selection ranges cannot overlap, so these offsets cannot underflow.
        let anchor = from
            .checked_add_signed(offset)
            .expect("Selection ranges cannot overlap");
        ranges.push(Range::new(anchor, anchor + output_len).with_direction(range.direction()));
        offset = offset
            .checked_add_unsigned(output_len)
            .expect("Selection ranges cannot overlap")
            .checked_sub_unsigned(deleted_len)
            .expect("Selection ranges cannot overlap");
        changes.push((from, to, Some(output)));
    }
    if behavior == ShellBehavior::Ignore {
        return Ok(FilterResult::Ignored);
    }
    Ok(FilterResult::Transaction(
        Transaction::change(text, changes.into_iter())
            .with_selection(Selection::new(ranges, selection.primary_index())),
    ))
}

/// Run in the caller's captured directory. Dropping the future kills the shell and
/// drops its input writer; no detached task retains the process's stdin.
pub async fn run(
    shell: &[String],
    command: &str,
    input: Option<Rope>,
    directory: &Path,
) -> anyhow::Result<Tendril> {
    anyhow::ensure!(!shell.is_empty(), "No shell set");
    let mut process = Command::new(&shell[0]);
    process
        .current_dir(directory)
        .args(&shell[1..])
        .arg(command);
    let output = super::process::output(process, input.as_ref()).await?;
    let output = if !output.status.success() && output.stderr.is_empty() {
        match output.status.code() {
            Some(code) => anyhow::bail!("Shell command failed: status {code}"),
            None => anyhow::bail!("Shell command failed"),
        }
    } else if !output.stderr.is_empty() {
        // Preserve the shell commands' existing preference for stderr over stdout.
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    Ok(Tendril::from(output))
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        // Commands can finish in an unfocused split, but must not follow a view
        // to another document (or become valid again after switching back).
        if event
            .editor
            .tree
            .try_get(event.view)
            .is_none_or(|view| view.doc != event.doc)
            && let Some(doc) = event.editor.document_mut(event.doc)
        {
            doc.shell_filters.remove_view(event.view);
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.shell_filters.clear();
        Ok(())
    });
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.shell_filters.remove_view(event.view);
        Ok(())
    });
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    #[tokio::test]
    async fn commands_use_the_captured_directory_and_preserve_stderr_priority() -> anyhow::Result<()>
    {
        let directory = tempfile::tempdir()?;
        std::fs::write(directory.path().join("marker"), "requested directory")?;
        let shell = vec!["sh".into(), "-c".into()];
        let output = super::run(&shell, "cat marker", None, directory.path()).await?;
        assert_eq!(output.as_str(), "requested directory");
        for command in [
            "printf stdout; printf stderr >&2",
            "printf stderr >&2; exit 1",
        ] {
            assert_eq!(
                super::run(&shell, command, None, directory.path())
                    .await?
                    .as_str(),
                "stderr"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn commands_can_close_stdin_before_consuming_the_selection() -> anyhow::Result<()> {
        let shell = vec!["sh".into(), "-c".into()];
        let input = editor_core::Rope::from_str(&"content\n".repeat(100_000));
        let output = super::run(&shell, "head -n 1", Some(input), Path::new(".")).await?;
        assert_eq!(output.as_str(), "content\n");
        Ok(())
    }
}

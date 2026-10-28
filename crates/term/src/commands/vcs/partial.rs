//! Terminal adapter for captured partial-index requests.

use crate::job::{Callback, Job, Jobs};
use anyhow::Result;
use vcs::operations::FileOperation;
use view::{features::git_operations::partial, Editor};

pub(crate) fn start(
    editor: &mut Editor,
    jobs: &mut Jobs,
    kind: FileOperation,
    lines: bool,
    disk: bool,
    advance: bool,
) -> Result<()> {
    let (request, next) = partial::capture(editor, kind, lines, disk)?;
    start_captured(editor, jobs, (request, next), advance);
    Ok(())
}

pub(crate) fn start_captured(
    editor: &mut Editor,
    jobs: &mut Jobs,
    (request, next): (
        view::features::git_operations::GitRequest,
        Option<partial::NextPartialTarget>,
    ),
    advance: bool,
) {
    editor.set_status("Preparing selected Git changes…");
    jobs.add(
        Job::with_callback(async move {
            let result = request.prepare().await;
            Ok(Callback::Followup(Box::new(move |editor| {
                let task = result.and_then(|prepared| prepared.admit(editor))?;
                Ok(Some(
                    Job::with_callback(async move {
                        let completion = task.execute().await;
                        Ok(Callback::Editor(Box::new(move |editor| {
                            let line = completion.next_partial_line();
                            let succeeded = completion.succeeded();
                            completion.finish(editor);
                            if advance
                                && succeeded
                                && let Some(next) = next
                            {
                                next.select_after(editor, line);
                            }
                        })))
                    })
                    .wait_before_exiting(),
                ))
            })))
        })
        .wait_before_exiting(),
    );
}

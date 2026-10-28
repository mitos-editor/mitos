//! Thin frontend entry points for editor-owned permalink requests.
use crate::{commands::Context, job::Jobs};
use view::Editor;

pub(in crate::commands) fn copy_permalink(cx: &mut Context) {
    request(cx.editor, cx.jobs, None, false);
}
pub(in crate::commands) fn open_permalink(cx: &mut Context) {
    request(cx.editor, cx.jobs, None, true);
}

pub(super) fn request(editor: &mut Editor, jobs: &mut Jobs, remote: Option<String>, open: bool) {
    #[cfg(feature = "git")]
    {
        let request = match view::features::permalink::PermalinkRequest::capture(editor) {
            Ok(request) => request,
            Err(error) => {
                editor.set_error(|| error.to_string());
                return;
            }
        };
        let sender = jobs.sender();
        jobs.callback(async move {
            let response = request.run(remote).await?;
            Ok(crate::job::Callback::EditorCompositor(Box::new(
                move |editor, _| {
                    let Some(result) = response.resolve(editor) else {
                        return;
                    };
                    publish(editor, &sender, result, open);
                },
            )))
        });
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (jobs, remote, open);
        editor.set_error(|| "No Git support included in this build");
    }
}

#[cfg(feature = "git")]
pub(crate) fn request_commit_permalink(
    editor: &mut Editor,
    jobs: &mut Jobs,
    path: std::path::PathBuf,
    workspace: std::path::PathBuf,
    revision: String,
    open: bool,
) {
    let request =
        view::features::permalink::CommitPermalinkRequest::new(editor, path, workspace, revision);
    request_commit(jobs, request, None, open);
}

#[cfg(feature = "git")]
pub(in crate::commands) fn request_commit(
    jobs: &mut Jobs,
    request: view::features::permalink::CommitPermalinkRequest,
    remote: Option<String>,
    open: bool,
) {
    let sender = jobs.sender();
    jobs.callback(async move {
        let response = request.run(remote).await?;
        Ok(crate::job::Callback::EditorCompositor(Box::new(
            move |editor, _| {
                let Some(result) = response.resolve(editor) else {
                    return;
                };
                publish(editor, &sender, result, open);
            },
        )))
    });
}

#[cfg(feature = "git")]
fn publish(
    editor: &mut Editor,
    jobs: &crate::job::JobSender,
    result: anyhow::Result<String>,
    open: bool,
) {
    match result {
        Err(error) => editor.set_error(|| error.to_string()),
        Ok(url) if open => {
            let sender = jobs.clone();
            tokio::spawn(async move {
                match crate::open_external_url_callback(
                    stdx::Url::parse(&url).expect("permalink has an HTTPS scheme"),
                )
                .await
                {
                    Ok(callback) => sender.send_callback(callback).await,
                    Err(error) => {
                        sender
                            .send(move |editor, _| editor.set_error(|| error.to_string()))
                            .await
                    }
                }
            });
        }
        Ok(url) => match editor.registers.write('+', vec![url]) {
            Ok(()) => editor.set_status("Copied Git permalink"),
            Err(error) => editor.set_error(|| error.to_string()),
        },
    }
}

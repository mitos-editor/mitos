//! Working-directory, directory-stack, and workspace-trust commands.

use crate::{commands::lsp::typed::lsp_restart, compositor, config::ConfigEvent, ui::PromptEvent};
use ::command_line::Args;
use anyhow::anyhow;
use std::path::{Path, PathBuf};
use stdx::path::home_dir;

/// Helper function to parse the first argument as a directory
#[inline]
fn parse_first_arg_as_dir(args: &Args, last_cwd: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match args.first().map(AsRef::as_ref) {
        Some("-") => last_cwd.ok_or_else(|| anyhow!("No previous working directory")),
        Some(path) => Ok(stdx::path::expand_tilde(Path::new(path)).into_owned()),
        None => Ok(home_dir()?),
    }
}

/// Helper function to apply a directory change for an already-parsed Path ref
#[inline]
fn apply_directory_change(cx: &mut compositor::Context, dir: &Path) -> anyhow::Result<()> {
    cx.editor.set_cwd(dir).map_err(|err| {
        anyhow!(
            "Could not change working directory to '{}': {err}",
            dir.display()
        )
    })?;

    cx.editor.set_status(format!(
        "Current working directory is now {}",
        stdx::env::current_working_dir().display()
    ));

    Ok(())
}

#[cold]
pub(super) fn change_current_directory(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let dir = parse_first_arg_as_dir(&args, cx.editor.get_last_cwd().map(|p| p.to_path_buf()))?;

    apply_directory_change(cx, &dir)
}

#[cold]
pub(super) fn show_directory_stack(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let serialized_stack = cx
        .editor
        .dir_stack
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" ");

    if !serialized_stack.is_empty() {
        cx.editor.set_status(serialized_stack);
    } else {
        cx.editor.set_error(|| "Stack is empty");
    }

    Ok(())
}

#[cold]
pub(super) fn push_directory(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    // avoid an unbounded directory stack and reallocs for perf
    if cx.editor.dir_stack.len() == cx.editor.dir_stack.capacity() {
        cx.editor.dir_stack.pop_back();
    }

    cx.editor
        .dir_stack
        .push_front(stdx::env::current_working_dir());

    change_current_directory(cx, args, event)
}

#[cold]
pub(super) fn pop_directory(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    if let Some(dir) = cx.editor.dir_stack.pop_front() {
        apply_directory_change(cx, &dir)?;
    } else {
        cx.editor.set_error(|| "Stack is empty");
    }

    Ok(())
}

#[cold]
pub(super) fn show_current_directory(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let cwd = stdx::env::current_working_dir();
    let message = format!("Current working directory is {}", cwd.display());

    if cwd.exists() {
        cx.editor.set_status(message);
    } else {
        cx.editor.set_error(|| format!("{} (deleted)", message));
    }
    Ok(())
}

fn current_workspace(cx: &compositor::Context) -> std::path::PathBuf {
    let (_, doc) = current_ref!(cx.editor);
    doc.workspace_root().to_path_buf()
}

/// Whether the currently focused document's workspace is trusted for git operations (gix
/// `Trust::Full`).
pub(super) fn doc_trust_full(editor: &view::Editor) -> bool {
    let (_, doc) = current_ref!(editor);
    editor
        .workspace_trust
        .query(
            doc.workspace_root(),
            loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted()
}

#[cold]
pub(super) fn trust_workspace(
    cx: &mut compositor::Context,
    args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.trust(&workspace);

    cx.config.updates.send(ConfigEvent::Refresh)?;
    // Restart any LSPs that didn't start because trust was missing.
    lsp_restart(cx, args, event)
}

#[cold]
pub(super) fn untrust_workspace(
    cx: &mut compositor::Context,
    _args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.untrust(&workspace);
    // Drop any workspace overrides that were merged into the live editor config while trust was
    // granted. Running LSPs are not stopped here (use `:lsp-stop` for that); this only handles
    // in-memory config.
    cx.config.updates.send(ConfigEvent::Refresh)?;
    Ok(())
}

#[cold]
pub(super) fn exclude_workspace(
    cx: &mut compositor::Context,
    _args: Args<'_>,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let workspace = current_workspace(cx);
    cx.editor.workspace_trust.exclude(&workspace);
    cx.config.updates.send(ConfigEvent::Refresh)?;
    Ok(())
}

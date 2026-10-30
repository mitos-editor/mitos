//! Commands for conflict navigation, inspection, and undoable buffer resolution.

use anyhow::{bail, Result};
use command_line::Args;

#[cfg(feature = "git")]
use crate::job::Callback;
use crate::{compositor, ui::PromptEvent};

pub(super) fn next(cx: &mut super::Context) {
    if let Err(error) = view::features::conflicts::navigate(cx.editor, true, cx.count()) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(super) fn previous(cx: &mut super::Context) {
    if let Err(error) = view::features::conflicts::navigate(cx.editor, false, cx.count()) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub(super) fn navigate(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> Result<()> {
    if event == PromptEvent::Validate {
        let forward = match args.first() {
            None | Some("next") => true,
            Some("previous") => false,
            _ => bail!("Expected next or previous"),
        };
        view::features::conflicts::navigate(cx.editor, forward, 1)?;
    }
    Ok(())
}

pub(super) fn resolve(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> Result<()> {
    if event == PromptEvent::Validate {
        let choice = match args.first() {
            Some("current") => vcs::conflict::Resolution::Current,
            Some("incoming") => vcs::conflict::Resolution::Incoming,
            Some("both") => vcs::conflict::Resolution::Both,
            _ => bail!("Expected current, incoming, or both"),
        };
        view::features::conflicts::resolve(cx.editor, choice)?;
    }
    Ok(())
}

pub(super) fn source(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = (cx, args);
        bail!("No Git support included in this build");
    }
    #[cfg(feature = "git")]
    {
        let side = match args.first() {
            Some(side @ ("base" | "current" | "incoming")) => side.to_owned(),
            _ => bail!("Expected base, current, or incoming"),
        };
        let (view, doc) = view::current_ref!(cx.editor);
        let path = doc
            .path()
            .ok_or_else(|| anyhow::anyhow!("Buffer has no file path"))?
            .to_owned();
        let workspace = doc.workspace_root().to_owned();
        let trusted = cx
            .editor
            .workspace_trust
            .query(&workspace, workspace::trust::TrustQuery::Git)
            .is_trusted();
        let (view_id, doc_id, version) = (view.id, doc.id(), doc.version());
        let encoding = doc.encoding();
        cx.jobs.callback(async move {
            let result = tokio::task::spawn_blocking(move || {
                vcs::conflict::read_source(&path, &side, trusted)
            })
            .await?;
            Ok(Callback::Editor(Box::new(move |editor| {
                if editor.tree.focus() != view_id
                    || editor
                        .tree
                        .try_get(view_id)
                        .is_none_or(|view| view.document_id() != doc_id)
                    || editor
                        .document(doc_id)
                        .is_none_or(|doc| doc.version() != version || doc.encoding() != encoding)
                    || editor
                        .workspace_trust
                        .query(&workspace, workspace::trust::TrustQuery::Git)
                        .is_trusted()
                        != trusted
                {
                    return;
                }
                match result {
                    Ok((label, bytes)) => {
                        let (text, _, malformed) = encoding.decode(&bytes);
                        if malformed {
                            editor.set_error(|| {
                                "Conflict source cannot be decoded with this file's encoding"
                            });
                            return;
                        }
                        view::features::conflicts::open_source(
                            editor,
                            format!("Conflict: {label}"),
                            editor_core::Rope::from(text.as_ref()),
                        );
                        editor.set_status(format!("Conflict source: {label}"));
                    }
                    Err(error) => editor.set_error(|| error.to_string()),
                }
            })))
        });
        Ok(())
    }
}

//! Inlay actions through the real protocol, editor operations and terminal UI.

use super::super::helpers::{keys, lsp, screen};
use super::Fixture;
use editor_core::{Selection, Transaction, Uri};
use view::{
    current, current_ref,
    editor::Action,
    features::inlay_hints::{self, InlayHintAction, ResolvedInlayHint},
};

async fn resolved(f: &mut Fixture) -> anyhow::Result<ResolvedInlayHint> {
    let view = f.inlays().await?;
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    Ok(inlay_hints::resolve(&mut f.app.editor, target)
        .unwrap()
        .await
        .unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn eager_and_lazy_actions_keep_label_order_and_originating_server() -> anyhow::Result<()> {
    for lazy in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut args = vec!["--inlay-details", "--inlay-second-action"];
        if lazy {
            args.push("--lazy-inlay-actions");
        }
        let mut f = Fixture::with_servers(dir.path(), &args, &["alpha", "beta"]).await?;
        let hint = resolved(&mut f).await?;
        assert_eq!(
            hint.actions(),
            [
                InlayHintAction::LabelLocation(0),
                InlayHintAction::LabelCommand(0),
                InlayHintAction::LabelLocation(1),
                InlayHintAction::LabelCommand(1),
                InlayHintAction::ApplyTextEdits
            ]
        );
        let request = f.wait_message("inlayHint/resolve").await?;
        assert_eq!(request["params"]["label"][0]["location"].is_null(), lazy);
        assert_eq!(request["params"]["label"][0]["command"].is_null(), lazy);
        assert_eq!(request["params"]["textEdits"].is_null(), lazy);
        hint.cache(&mut f.app.editor);
        hint.execute(
            &mut f.app.editor,
            InlayHintAction::LabelCommand(0),
            Action::Replace,
        )?;
        let command = f.wait_message("workspace/executeCommand").await?;
        assert_eq!(command["params"]["command"], "inlay.inspect");
        assert_eq!(
            command["params"]["arguments"],
            serde_json::json!([{"provider": "alpha", "opaque": [null, {"value": true}]}])
        );
        assert!(!lsp::read_log(&lsp::log_path(dir.path(), "beta"))?
            .iter()
            .any(|message| message["method"] == "workspace/executeCommand"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn omitted_resolve_fields_keep_eager_actions() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f =
        Fixture::with_args(dir.path(), &["--inlay-details", "--omit-inlay-actions"]).await?;
    let hint = resolved(&mut f).await?;
    assert_eq!(
        hint.actions(),
        [
            InlayHintAction::LabelLocation(0),
            InlayHintAction::LabelCommand(0),
            InlayHintAction::ApplyTextEdits
        ]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn label_navigation_keeps_encoding_and_return_history() -> anyhow::Result<()> {
    for utf8 in [false, true] {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "😀 target\n")?;
        let uri = Uri::from(target.clone()).to_url().unwrap().to_string();
        let mut args = vec![
            "--inlay-details",
            "--lazy-inlay-actions",
            "--inlay-target",
            uri.as_str(),
        ];
        if utf8 {
            args.push("--utf-8");
        }
        let mut f = Fixture::with_args(dir.path(), &args).await?;
        let hint = resolved(&mut f).await?;
        let origin = hint.document_id();
        let origin_selection = current_ref!(f.app.editor)
            .1
            .selection(hint.view_id())
            .clone();
        hint.execute(
            &mut f.app.editor,
            InlayHintAction::LabelLocation(0),
            Action::Replace,
        )?;
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.path(), Some(target.as_path()));
        assert_eq!(doc.selection(view.id).primary().from(), 2);
        assert_eq!(doc.selection(view.id).primary().to(), 8);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2
        );
        keys(&mut f.app, "<C-o>").await?;
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.id(), origin);
        assert_eq!(doc.selection(view.id), &origin_selection);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hint_edits_apply_together_in_one_undo_step_with_protocol_clamping() -> anyhow::Result<()> {
    for (kind, expected) in [
        ("multiple", "prefix 😀 typed\n"),
        ("clamped", "😀 typed\n"),
        ("line", "😀 original\nbad"),
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::with_args(
            dir.path(),
            &[
                "--inlay-details",
                "--lazy-inlay-actions",
                "--hint-edits",
                kind,
            ],
        )
        .await?;
        let hint = resolved(&mut f).await?;
        hint.execute(
            &mut f.app.editor,
            InlayHintAction::ApplyTextEdits,
            Action::Replace,
        )?;
        assert_eq!(current_ref!(f.app.editor).1.text().to_string(), expected);
        assert!(!hint.is_current(&f.app.editor));
        assert!(hint
            .execute(
                &mut f.app.editor,
                InlayHintAction::ApplyTextEdits,
                Action::Replace
            )
            .is_err());
        assert!(current!(f.app.editor).1.undo());
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            "😀 original\n"
        );
        assert!(current!(f.app.editor).1.redo());
        assert_eq!(current_ref!(f.app.editor).1.text().to_string(), expected);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_hint_edits_preserve_text_and_pending_undo_group() -> anyhow::Result<()> {
    for (kind, utf8) in [
        ("reversed", false),
        ("overlap", false),
        ("interior", false),
        ("interior", true),
    ] {
        let dir = tempfile::tempdir()?;
        let mut args = vec![
            "--inlay-details",
            "--lazy-inlay-actions",
            "--hint-edits",
            kind,
        ];
        if utf8 {
            args.push("--utf-8");
        }
        let mut f = Fixture::with_args(dir.path(), &args).await?;
        let (view, doc) = current!(f.app.editor);
        let change = Transaction::insert(
            doc.text(),
            &Selection::point(doc.text().len_chars() - 1),
            "!".into(),
        );
        view::editing::apply(doc, &change, view.id)?;
        let hint = resolved(&mut f).await?;
        let before = current_ref!(f.app.editor).1.text().to_string();
        assert!(hint
            .execute(
                &mut f.app.editor,
                InlayHintAction::ApplyTextEdits,
                Action::Replace
            )
            .is_err());
        let (_, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.text().to_string(), before);
        assert!(!doc.changes().is_empty());
        assert!(current!(f.app.editor).1.undo());
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            "😀 original\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_reject_edits_and_another_focused_document() -> anyhow::Result<()> {
    for switch in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
        let hint = resolved(&mut f).await?;
        let origin = hint.document_id();
        if switch {
            f.app.editor.new_file(Action::HorizontalSplit);
        } else {
            f.edit();
        }
        let (view, doc) = current_ref!(f.app.editor);
        let active_document = doc.id();
        let active_text = doc.text().to_string();
        let active_selection = doc.selection(view.id).clone();
        for action in hint.actions() {
            assert!(hint
                .execute(&mut f.app.editor, action, Action::Replace)
                .is_err());
        }
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.id(), active_document);
        assert_eq!(doc.text().to_string(), active_text);
        assert_eq!(doc.selection(view.id), &active_selection);
        if switch {
            assert_eq!(
                f.app.editor.document(origin).unwrap().text().to_string(),
                "😀 original\n"
            );
        }
        assert!(!lsp::read_log(&f.log)?
            .iter()
            .any(|message| message["method"] == "workspace/executeCommand"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn action_menu_applies_edits_and_closes_after_background_refresh() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    f.inlays().await?;
    keys(&mut f.app, "<space>?inlay_hints<ret>").await?;
    assert!(screen(&f.app).contains("Apply hint text edits"));
    keys(&mut f.app, "<down><down><down><ret>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "😀 : Typeoriginal\n"
    );

    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    f.inlays().await?;
    keys(&mut f.app, "<space>?inlay_hints<ret>").await?;
    assert!(screen(&f.app).contains("Apply hint text edits"));
    let server = current_ref!(f.app.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    inlay_hints::refresh_for_language_server(&mut f.app.editor, server);
    f.response().await?(&mut f.app.editor);
    keys(&mut f.app, "").await?;
    assert!(!screen(&f.app).contains("Apply hint text edits"));
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "😀 original\n"
    );
    Ok(())
}

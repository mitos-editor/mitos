//! Request lifetime and presentation contracts through the shared editor API.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use stdx::rope::RopeExt as _;

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    editor::Action,
    features::{
        hover::{self, HoverHandler, HoverUpdate},
        inlay_hints::{self, InlayHintsHandler},
    },
};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    test_config,
};

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    log: PathBuf,
}

impl Fixture {
    async fn new(dir: &Path) -> anyhow::Result<Self> {
        Self::with_args(dir, &[]).await
    }

    async fn with_args(dir: &Path, args: &[&str]) -> anyhow::Result<Self> {
        Self::with_servers(dir, args, &["alpha"]).await
    }

    async fn with_servers(dir: &Path, args: &[&str], names: &[&str]) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize-ready"));
        let servers = names
            .iter()
            .map(|name| {
                let mut server =
                    ServerConfig::feature(name, "--language-features", dir).initialize_gate(&gate);
                for arg in args {
                    server = server.arg(arg);
                }
                server.toml()
            })
            .collect::<String>();
        let mut config = test_config();
        config.editor.lsp.inlay_hints.enable = true;
        config.editor.lsp.inlay_hints.length_limit = Some(3.try_into().unwrap());
        let (mut app, callbacks) = lsp::feature_app_with_servers(
            dir,
            "😀 original\n",
            "hover-test",
            names,
            config,
            &servers,
            |handlers, sender, _| {
                handlers.hover = HoverHandler::new(sender.clone());
                handlers.inlay_hints = InlayHintsHandler::new(sender);
            },
        )?;
        gate.release()?;
        lsp::initialize(&mut app, names.len()).await?;
        Ok(Self {
            app,
            callbacks,
            log: lsp::log_path(dir, "alpha"),
        })
    }

    async fn wait_message(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        lsp::wait_message(&self.log, method).await
    }

    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .map(|(_, callback)| callback)
            .context("feature callback queue closed")
    }

    async fn hover(&mut self) -> anyhow::Result<HoverUpdate> {
        hover::request(&mut self.app.editor);
        self.response().await?(&mut self.app.editor);
        hover::next_update(&self.app.editor).context("hover update missing")
    }

    async fn refresh_inlay_hints(&mut self) -> anyhow::Result<()> {
        let (server, call) = tokio::time::timeout(
            Duration::from_secs(5),
            self.app.editor.next_language_server_message(),
        )
        .await?
        .context("LSP message stream closed")?;
        anyhow::ensure!(matches!(&call, lsp_client::Call::MethodCall(call)
            if call.method == "workspace/inlayHint/refresh"));
        self.app.handle_language_server_message(call, server).await;
        Ok(())
    }

    fn edit(&mut self) {
        let (view, doc) = current!(self.app.editor);
        let transaction = Transaction::insert(doc.text(), &Selection::point(0), "new ".into());
        view::editing::apply(doc, &transaction, view.id).unwrap();
    }

    async fn inlays(&mut self) -> anyhow::Result<view::ViewId> {
        inlay_hints::refresh(&mut self.app.editor);
        self.response().await?(&mut self.app.editor);
        Ok(current_ref!(self.app.editor).0.id)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_resolve_exact_original_hint_once_and_retain_metadata() -> anyhow::Result<()>
{
    use lsp_client::{lsp as protocol, OffsetEncoding};
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    let view = f.inlays().await?;
    let request = f.wait_message("initialize").await?;
    assert_eq!(
        request["params"]["capabilities"]["textDocument"]["inlayHint"]["resolveSupport"]
            ["properties"],
        serde_json::json!(["tooltip", "label.tooltip"])
    );
    assert!(!lsp::read_log(&f.log)?
        .iter()
        .any(|message| message["method"] == "inlayHint/resolve"));
    let item = inlay_hints::hints_on_line(&f.app.editor, view, 0).remove(0);
    assert_eq!(item.label, "界é suffix");
    let result = inlay_hints::resolve(&mut f.app.editor, item.target.clone())
        .unwrap()
        .await
        .unwrap();
    assert!(result.is_current(&f.app.editor));
    assert_eq!(result.offset_encoding(), OffsetEncoding::Utf16);
    let request = f.wait_message("inlayHint/resolve").await?;
    assert_eq!(
        request["params"]["position"],
        serde_json::json!({"line": 0, "character": 3})
    );
    assert_eq!(
        request["params"]["data"],
        serde_json::json!({"server": "alpha", "opaque": [null, {"nested": "value"}]})
    );
    assert_eq!(request["params"]["label"][0]["tooltip"], "eager *part*");
    let protocol::InlayHintLabel::LabelParts(parts) = &result.hint().label else {
        panic!("structured label lost")
    };
    assert!(parts[0].location.is_some());
    assert_eq!(parts[0].command.as_ref().unwrap().command, "inlay.inspect");
    assert_eq!(parts[0].value, "界é");
    assert!(result.hint().text_edits.is_some());
    assert!(
        matches!(&result.hint().tooltip, Some(protocol::InlayHintTooltip::MarkupContent(markup)) if markup.kind == protocol::MarkupKind::PlainText)
    );
    result.cache(&mut f.app.editor);
    let second = inlay_hints::resolve(&mut f.app.editor, item.target)
        .unwrap()
        .await
        .unwrap();
    assert!(second.is_current(&f.app.editor));
    assert_eq!(
        lsp::read_log(&f.log)?
            .iter()
            .filter(|message| message["method"] == "inlayHint/resolve")
            .count(),
        1
    );
    assert!(!result.is_current(&f.app.editor)); // selecting another hint cancels the old interaction
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_reject_stale_targets_and_queued_results() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    let view = f.inlays().await?;
    let item = inlay_hints::hints_on_line(&f.app.editor, view, 0).remove(0);
    let result = inlay_hints::resolve(&mut f.app.editor, item.target.clone())
        .unwrap()
        .await
        .unwrap();
    current!(f.app.editor)
        .1
        .set_selection(view, Selection::point(3));
    assert!(!result.is_current(&f.app.editor));
    assert!(inlay_hints::resolve(&mut f.app.editor, item.target).is_none());
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    let server = result.language_server_id();
    inlay_hints::refresh_for_language_server(&mut f.app.editor, server);
    f.response().await?(&mut f.app.editor);
    assert!(!target.is_current(&f.app.editor)); // same version and viewport, different cache
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    f.edit();
    assert!(!target.is_current(&f.app.editor));
    assert!(inlay_hints::hints_on_line(&f.app.editor, view, 0).is_empty());
    assert!(current_ref!(f.app.editor).1.inlay_hints(view).is_some()); // mapped display remains
    assert!(current!(f.app.editor).1.undo());
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "😀 original\n"
    );
    assert!(!target.is_current(&f.app.editor));
    assert!(inlay_hints::hints_on_line(&f.app.editor, view, 0).is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_reject_view_reuse_and_provider_detachment() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    let view = f.inlays().await?;
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    let result = inlay_hints::resolve(&mut f.app.editor, target.clone())
        .unwrap()
        .await
        .unwrap();
    let document = result.document_id();
    f.app.editor.new_file(Action::Replace);
    assert!(!result.is_current(&f.app.editor));
    assert!(!target.is_current(&f.app.editor));
    f.app.editor.switch(document, Action::Replace);
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    assert!(!target.is_current(&f.app.editor));
    assert!(inlay_hints::resolve(&mut f.app.editor, target).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_resolution_is_canceled_by_edits_while_waiting() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("resolve-ready"));
    let mut f = Fixture::with_args(
        dir.path(),
        &[
            "--inlay-details",
            "--resolve-gate",
            gate.path().to_str().unwrap(),
        ],
    )
    .await?;
    let view = f.inlays().await?;
    let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
        .remove(0)
        .target;
    let future = inlay_hints::resolve(&mut f.app.editor, target).unwrap();
    f.wait_message("inlayHint/resolve").await?;
    f.edit();
    assert!(tokio::time::timeout(Duration::from_secs(5), future)
        .await?
        .is_none());
    gate.release()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dismissing_pending_inlay_details_prevents_late_popups() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("resolve-ready"));
    let mut f = Fixture::with_args(
        dir.path(),
        &[
            "--inlay-details",
            "--resolve-gate",
            gate.path().to_str().unwrap(),
        ],
    )
    .await?;
    let view = f.inlays().await?;
    let selection = current_ref!(f.app.editor).1.selection(view).clone();
    keys(&mut f.app, "<space>?inlay_hints<ret>").await?;
    f.wait_message("inlayHint/resolve").await?;
    assert!(screen(&f.app).contains("Loading inlay hint"));
    keys(&mut f.app, "<esc>").await?;
    assert_eq!(current_ref!(f.app.editor).1.selection(view), &selection);
    assert!(!screen(&f.app).contains("Loading inlay hint"));
    gate.release()?;
    f.hover().await?; // transport barrier after the delayed resolve response
    keys(&mut f.app, "").await?;
    assert!(!screen(&f.app).contains("literal whole hint"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_pending_inlay_details_keeps_only_the_new_interaction() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("resolve-ready"));
    let mut f = Fixture::with_args(
        dir.path(),
        &[
            "--inlay-details",
            "--resolve-gate",
            gate.path().to_str().unwrap(),
        ],
    )
    .await?;
    f.inlays().await?;
    keys(&mut f.app, "<space>?inlay_hints<ret>").await?;
    f.wait_message("inlayHint/resolve").await?;
    keys(&mut f.app, "<esc><space>?inlay_hints<ret>").await?;
    assert!(screen(&f.app).contains("Loading inlay hint"));
    gate.release()?;
    f.hover().await?;
    keys(&mut f.app, "").await?;
    assert!(screen(&f.app).contains("literal whole hint"));
    assert!(!screen(&f.app).contains("Loading inlay hint"));
    assert_eq!(
        lsp::read_log(&f.log)?
            .iter()
            .filter(|message| message["method"] == "inlayHint/resolve")
            .count(),
        2
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_preserve_same_anchor_kind_order_and_per_hint_padding() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--mixed-inlays"]).await?;
    let view_id = f.inlays().await?;
    let (view, doc) = current_ref!(f.app.editor);
    let labels: Vec<_> = inlay_hints::hints_on_line(&f.app.editor, view_id, 0)
        .into_iter()
        .map(|hint| hint.label)
        .collect();
    assert_eq!(labels, ["P", "T", "O"]);
    let layout = view.layout(doc, Some(&f.app.editor.theme));
    let text: String = text_layout::doc_formatter::DocumentFormatter::new_at_prev_checkpoint(
        layout.text,
        &layout.format,
        &layout.annotations,
        0,
    )
    .filter(|grapheme| grapheme.is_virtual())
    .map(|grapheme| grapheme.raw.to_string())
    .collect();
    assert_eq!(text, " P  TO ");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_command_renders_markup_and_closes_stale_details() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-details"]).await?;
    let view = f.inlays().await?;
    keys(&mut f.app, "<space>?inlay_hints<ret>").await?;
    f.wait_message("inlayHint/resolve").await?;
    keys(&mut f.app, "").await?;
    let contents = screen(&f.app);
    // The test backend stores a blank trailing cell for the wide first grapheme.
    assert!(contents.contains("é suffix"), "{contents}");
    assert!(contents.contains("**literal whole hint**"), "{contents}");
    assert!(contents.contains("resolved part"), "{contents}");
    assert!(!contents.contains("**resolved part**"), "{contents}");
    let server = current_ref!(f.app.editor)
        .1
        .language_servers()
        .next()
        .unwrap()
        .id();
    inlay_hints::refresh_for_language_server(&mut f.app.editor, server);
    f.response().await?(&mut f.app.editor);
    keys(&mut f.app, "").await?;
    assert!(!screen(&f.app).contains("literal whole hint"));
    assert_eq!(inlay_hints::hints_on_line(&f.app.editor, view, 0).len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_command_chooses_one_hint_in_response_order() -> anyhow::Result<()> {
    use super::helpers::{keys, screen};
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--mixed-inlays"]).await?;
    f.inlays().await?;
    keys(&mut f.app, "<space>?inlay_hints<ret><down><ret>").await?;
    keys(&mut f.app, "").await?;
    assert!(screen(&f.app).contains("Type documentation"));
    assert!(!screen(&f.app).contains("Parameter documentation"));
    assert!(!lsp::read_log(&f.log)?
        .iter()
        .any(|message| message["method"] == "inlayHint/resolve"));
    keys(&mut f.app, "<esc>").await?;
    assert!(!screen(&f.app).contains("Type documentation"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_details_work_without_a_resolve_provider_and_retry_errors() -> anyhow::Result<()> {
    for args in [
        vec!["--inlay-details", "--no-inlay-resolve"],
        vec!["--inlay-details", "--resolve-error"],
        vec!["--inlay-details", "--changed-hint"],
    ] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::with_args(dir.path(), &args).await?;
        let view = f.inlays().await?;
        let target = inlay_hints::hints_on_line(&f.app.editor, view, 0)
            .remove(0)
            .target;
        for _ in 0..2 {
            let result = inlay_hints::resolve(&mut f.app.editor, target.clone())
                .unwrap()
                .await
                .unwrap();
            assert!(result.is_current(&f.app.editor));
            assert!(result.hint().tooltip.is_none());
            result.cache(&mut f.app.editor);
        }
        let expected = if args.contains(&"--no-inlay-resolve") {
            0
        } else {
            2
        };
        assert_eq!(
            lsp::read_log(&f.log)?
                .iter()
                .filter(|message| message["method"] == "inlayHint/resolve")
                .count(),
            expected
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_edits_before_publication_and_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    hover::request(&mut f.app.editor);
    let response = f.response().await?;
    f.edit();
    response(&mut f.app.editor);
    assert!(hover::next_update(&f.app.editor).is_none());
    let update = f.hover().await?;
    f.edit();
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_replacement_and_selection_changes_invalidate_queued_updates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let old = f.hover().await?;
    let new = f.hover().await?;
    assert!(old.resolve(&f.app.editor).is_none());
    assert!(new.resolve(&f.app.editor).is_some());
    let update = f.hover().await?;
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(2));
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_display_changes_supersede_requests_and_reject_stale_results() -> anyhow::Result<()> {
    use hover::HoverDisplay;
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    hover::request_with(&mut f.app.editor, HoverDisplay::Buffer);
    f.response().await?(&mut f.app.editor);
    let buffer_update = hover::next_update(&f.app.editor).unwrap();
    assert_eq!(buffer_update.display(), HoverDisplay::Buffer);
    let popup_update = f.hover().await?;
    assert!(buffer_update.resolve(&f.app.editor).is_none());
    assert_eq!(popup_update.display(), HoverDisplay::Popup);
    assert!(popup_update.resolve(&f.app.editor).is_some());

    hover::request_with(&mut f.app.editor, HoverDisplay::Buffer);
    f.response().await?(&mut f.app.editor);
    let buffer_update = hover::next_update(&f.app.editor).unwrap();
    f.edit();
    assert!(buffer_update.resolve(&f.app.editor).is_none());

    hover::request_with(&mut f.app.editor, HoverDisplay::Buffer);
    f.response().await?(&mut f.app.editor);
    let buffer_update = hover::next_update(&f.app.editor).unwrap();
    f.app.editor.new_file(Action::HorizontalSplit);
    assert!(buffer_update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_view_switches_and_detached_servers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.new_file(Action::HorizontalSplit);
    assert!(update.resolve(&f.app.editor).is_none());
    f.app.editor.switch(document, Action::Replace);
    let update = f.hover().await?;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_edits_and_reuse_pending_and_completed_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    inlay_hints::refresh(&mut f.app.editor);
    assert!(f.callbacks.try_recv().is_err());
    f.edit();
    response(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    let hints = doc.inlay_hints(view.id).unwrap();
    assert_eq!(hints.type_inlay_hints[0].char_idx, 3);
    assert_eq!(hints.type_inlay_hints[0].text, "界é…");
    inlay_hints::refresh(&mut f.app.editor);
    assert!(f.callbacks.try_recv().is_err());
    // The hover response is a transport barrier: prior inlay requests have reached
    // the server, so the log can prove both pending and completed requests were reused.
    f.hover().await?;
    let requests = std::fs::read_to_string(&f.log)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "textDocument/inlayHint")
            .count(),
        2
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hint_server_refresh_rejects_queued_hints_without_an_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-refresh"]).await?;
    let (_, doc) = current_ref!(f.app.editor);
    let document = doc.id();
    let version = doc.version();
    f.app.editor.switch(document, Action::HorizontalSplit);
    inlay_hints::refresh(&mut f.app.editor);
    let old = f.response().await?;
    let other_old = f.response().await?;
    f.refresh_inlay_hints().await?;
    old(&mut f.app.editor);
    other_old(&mut f.app.editor);
    let views: Vec<_> = f.app.editor.tree.views().map(|(view, _)| view.id).collect();
    let doc = f.app.editor.document(document).unwrap();
    assert!(views.iter().all(|&view| doc.inlay_hints(view).is_none()));
    f.response().await?(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let doc = f.app.editor.document(document).unwrap();
    assert!(views.iter().all(|&view| doc.inlay_hints(view).is_some()));
    assert_eq!(doc.version(), version);
    // A hover response is a transport barrier for the refresh acknowledgement.
    f.hover().await?;
    let requests = lsp::read_log(&f.log)?;
    assert!(requests
        .iter()
        .any(|request| request["id"] == "inlay-refresh"
            && request
                .get("result")
                .is_some_and(serde_json::Value::is_null)
            && request.get("error").is_none()));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "textDocument/inlayHint")
            .count(),
        4
    );
    let initialize = requests
        .iter()
        .find(|request| request["method"] == "initialize")
        .unwrap();
    assert_eq!(
        initialize["params"]["capabilities"]["workspace"]["inlayHint"]["refreshSupport"],
        true
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hint_server_refresh_invalidates_hidden_document_caches() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--inlay-refresh"]).await?;
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    let document = doc.id();
    let view_id = view.id;
    assert!(doc.inlay_hints(view_id).is_some());
    f.app.editor.new_file(Action::Replace);
    assert!(f
        .app
        .editor
        .document(document)
        .unwrap()
        .inlay_hints(view_id)
        .is_some());
    f.refresh_inlay_hints().await?;
    assert!(f
        .app
        .editor
        .document(document)
        .unwrap()
        .inlay_hints(view_id)
        .is_none());
    assert!(f.callbacks.try_recv().is_err());
    f.app.editor.switch(document, Action::Replace);
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(document)
        .unwrap()
        .inlay_hints(view_id)
        .is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hint_refresh_from_an_unselected_provider_preserves_pending_hints(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_servers(dir.path(), &[], &["alpha", "beta"]).await?;
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    let server = current_ref!(f.app.editor)
        .1
        .language_servers()
        .find(|server| server.name() == "beta")
        .unwrap()
        .id();
    let call = lsp_client::Call::MethodCall(serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": "unselected-refresh", "method": "workspace/inlayHint/refresh", "params": null
    }))?);
    f.app.handle_language_server_message(call, server).await;
    response(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_some());
    inlay_hints::refresh(&mut f.app.editor);
    assert!(f.callbacks.try_recv().is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_view_reuse_and_server_detachment() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    let (view, doc) = current_ref!(f.app.editor);
    let view_id = view.id;
    let document = doc.id();
    f.app.editor.new_file(Action::Replace);
    response(&mut f.app.editor);
    assert!(f
        .app
        .editor
        .document(document)
        .unwrap()
        .inlay_hints(view_id)
        .is_none());
    f.app.editor.switch(document, Action::Replace);
    inlay_hints::refresh(&mut f.app.editor);
    let response = f.response().await?;
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("alpha");
    response(&mut f.app.editor);
    assert!(current_ref!(f.app.editor).1.inlay_hints(view_id).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_refresh_each_split_after_an_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let mut config = (*f.app.editor.config()).clone();
    config.lsp.inlay_hints.length_limit = None;
    f.app
        .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.switch(document, Action::HorizontalSplit);
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    f.edit();
    inlay_hints::refresh(&mut f.app.editor);
    // Publishing one split's result must not mark the other split current.
    f.response().await?(&mut f.app.editor);
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let views: Vec<_> = f.app.editor.tree.views().map(|(view, _)| view.id).collect();
    assert_eq!(views.len(), 2);
    let doc = f.app.editor.document(document).unwrap();
    for view in views {
        assert!(doc.inlay_hints(view).unwrap().type_inlay_hints[0]
            .text
            .contains("new "));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_rejects_renamed_and_closed_documents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.set_doc_path(
        current_ref!(f.app.editor).1.id(),
        &dir.path().join("renamed.hover-test"),
    );
    assert!(update.resolve(&f.app.editor).is_none());
    let update = f.hover().await?;
    f.app.editor.new_file(Action::Replace);
    assert!(f.app.editor.close_document(document, true).is_ok());
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_reject_superseded_viewports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    {
        let (view, doc) = current!(f.app.editor);
        view.area.height = 10;
        let text = "😀 line\n".repeat(200);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &transaction, view.id).unwrap();
    }
    inlay_hints::refresh(&mut f.app.editor);
    let old = f.response().await?;
    let (view, doc) = current!(f.app.editor);
    doc.set_view_offset(
        view.id,
        view::view::ViewPosition {
            anchor: doc.text().line_to_char_idx(100),
            ..Default::default()
        },
    );
    inlay_hints::refresh(&mut f.app.editor);
    old(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).unwrap().id.first_line > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_buffers_cancels_inflight_hover_requests() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::with_args(dir.path(), &["--hold-hover"]).await?;
    hover::request(&mut f.app.editor);
    let request = f.wait_message("textDocument/hover").await?;
    f.app.editor.new_file(Action::Replace);
    let cancel = f.wait_message("$/cancelRequest").await?;
    assert_eq!(cancel["params"]["id"], request["id"]);
    assert!(hover::next_update(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_invalidates_hover_and_inlay_hints() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    inlay_hints::refresh(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_some());
    hover::request(&mut f.app.editor);
    let callback = f.response().await?;
    disable_language_features(&mut f.app.editor)?;
    callback(&mut f.app.editor);
    assert!(hover::next_update(&f.app.editor).is_none());
    let (view, doc) = current_ref!(f.app.editor);
    assert!(doc.inlay_hints(view.id).is_none());
    Ok(())
}

fn disable_language_features(editor: &mut view::Editor) -> anyhow::Result<()> {
    let loader = super::helpers::test_syntax_loader(Some(
        r#"
        [[language]]
        name = "hover-test"
        scope = "source.hover-test"
        file-types = ["hover-test"]
        roots = []
        language-servers = [{ name = "alpha", except-features = ["hover", "inlay-hints"] }]
    "#
        .into(),
    ));
    editor.apply_language_config(loader, editor.theme.clone())?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_rejects_hover_awaiting_presentation() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let update = f.hover().await?;
    disable_language_features(&mut f.app.editor)?;
    assert!(update.resolve(&f.app.editor).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_publish_for_unfocused_views_after_selection_changes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path()).await?;
    let (view, doc) = current_ref!(f.app.editor);
    let background = view.id;
    let document = doc.id();
    f.app.editor.switch(document, Action::HorizontalSplit);
    let focused = f.app.editor.tree.focus();
    assert_ne!(focused, background);
    inlay_hints::refresh(&mut f.app.editor);
    let first = f.response().await?;
    let second = f.response().await?;
    f.app
        .editor
        .document_mut(document)
        .unwrap()
        .set_selection(background, Selection::point(4));
    first(&mut f.app.editor);
    second(&mut f.app.editor);
    assert_eq!(f.app.editor.tree.focus(), focused);
    let doc = f.app.editor.document(document).unwrap();
    assert!(doc.inlay_hints(background).is_some());
    assert!(doc.inlay_hints(focused).is_some());
    Ok(())
}

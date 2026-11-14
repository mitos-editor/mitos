//! Real transport coverage for hierarchy trees, lazy loading, and source navigation.
use super::helpers::{
    self,
    lsp::{self, Gate, ServerConfig},
    test_config, AppBuilder,
};
use editor_core::{Selection, Transaction};
use std::path::PathBuf;
use view::{current, current_ref, editor::Action, features::hierarchy::HierarchyKind};

struct Fixture {
    app: term::application::TestApplication,
    path: PathBuf,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(args: &[&str]) -> anyhow::Result<Self> {
        Self::with_servers(&["hierarchy"], args).await
    }

    async fn with_servers(names: &[&str], args: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let gate = Gate::new(dir.path().join("ready"));
        let servers = names
            .iter()
            .map(|name| {
                let mut server =
                    ServerConfig::feature(name, "--hierarchy", dir.path()).initialize_gate(&gate);
                for arg in args {
                    server = server.arg(arg);
                }
                server.toml()
            })
            .collect::<String>();
        let mut config = test_config();
        config.editor.lsp.enable = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader(
                "hierarchy-test",
                "hierarchy-test",
                names,
                &servers,
            ))
            .build()?;
        let path = stdx::path::canonicalize(dir.path().join("file.hierarchy-test"));
        std::fs::write(&path, "😀 target root_source\n")?;
        for suffix in ["caller", "callee", "parent", "child"] {
            std::fs::write(
                path.with_extension(format!("hierarchy-test.{suffix}")),
                format!("😀 {suffix:6} {suffix}_source\n"),
            )?;
        }
        app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        lsp::initialize(&mut app, names.len()).await?;
        let log = lsp::log_path(dir.path(), names[0]);
        Ok(Self {
            app,
            path,
            log,
            _dir: dir,
        })
    }

    fn screen(&self) -> String {
        self.app
            .terminal_backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    async fn wait(&mut self, text: &str) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            helpers::keys(&mut self.app, "").await?;
            let screen = self.screen();
            if screen.contains(text) {
                return Ok(());
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "Missing {text}: {screen}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    fn count(&self, method: &str) -> usize {
        lsp::read_log(&self.log)
            .unwrap()
            .iter()
            .filter(|msg| msg["method"] == method)
            .count()
    }

    async fn wait_for_requests(&mut self, method: &str, count: usize) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.count(method) < count {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "Missing request {method}"
            );
            helpers::keys(&mut self.app, "").await?;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok(())
    }

    async fn close(mut self) -> anyhow::Result<()> {
        helpers::keys(&mut self.app, "<esc>").await?;
        assert!(self.app.close().await.is_empty());
        Ok(())
    }
}

#[test]
fn health_reports_negotiated_hierarchy_capabilities() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("config.toml"),
        "[editor.workspace-trust]\nlevel = 'insecure'\n",
    )?;
    for supported in [true, false] {
        let mut server = ServerConfig::feature("hierarchy", "--hierarchy", dir.path());
        if !supported {
            server = server.arg("--no-hierarchy");
        }
        std::fs::write(
            dir.path().join("languages.toml"),
            format!(
                "{}\n[[language]]\nname = 'hierarchy-test'\nscope = 'source.hierarchy-test'\nfile-types = ['hierarchy-test']\nlanguage-servers = ['hierarchy']\n",
                server.toml()
            ),
        )?;
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ms"))
            .args(["--health", "hierarchy-test"])
            .env("MITOS_CONFIG_DIR", dir.path())
            .current_dir(dir.path())
            .output()?;
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout)?;
        let (_, features) = stdout.split_once("✓ Supported:").unwrap();
        let (advertised, remaining) = features.split_once("− Not advertised:").unwrap();
        let (unadvertised, _) = remaining
            .split_once("○ Disabled by configuration:")
            .unwrap();
        for feature in ["call-hierarchy", "type-hierarchy"] {
            assert_eq!(advertised.contains(feature), supported, "{stdout}");
            assert_eq!(unadvertised.contains(feature), !supported, "{stdout}");
        }
        let log = lsp::read_log(&lsp::log_path(dir.path(), "hierarchy"))?;
        assert!(!log
            .iter()
            .any(|msg| msg["method"] == "textDocument/didOpen"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn call_hierarchy_expands_lazily_caches_collapses_and_previews_utf16_targets(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    helpers::keys(&mut f.app, "gH").await?;
    f.wait("Incoming calls").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 0);
    helpers::keys(&mut f.app, "<down><tab>").await?;
    f.wait("caller").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 1);
    helpers::keys(&mut f.app, "<tab>").await?; // collapse
    helpers::keys(&mut f.app, "<tab>").await?; // cached expansion
    f.wait("caller").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 1);
    helpers::keys(&mut f.app, "<down>").await?;
    f.wait("caller_source").await?; // source preview on the right
    helpers::keys(&mut f.app, "<tab>").await?;
    f.wait("target (recursive)").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 2);
    helpers::keys(&mut f.app, "<ret>").await?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(
        doc.path(),
        Some(f.path.with_extension("hierarchy-test.caller").as_path())
    );
    // The fixture returns character 3 in UTF-16, after an astral character + space.
    assert_eq!(doc.selection(view.id).primary().from(), 2);
    assert_eq!(doc.selection(view.id).primary().to(), 8);
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn outgoing_calls_and_both_type_directions_keep_the_tree_and_source_previews(
) -> anyhow::Result<()> {
    for (keys, section, name, method) in [
        (
            "gH",
            "Outgoing calls",
            "callee",
            "callHierarchy/outgoingCalls",
        ),
        ("gT", "Supertypes", "parent", "typeHierarchy/supertypes"),
        ("gT", "Subtypes", "child", "typeHierarchy/subtypes"),
    ] {
        let mut f = Fixture::new(&[]).await?;
        helpers::keys(&mut f.app, keys).await?;
        f.wait(section).await?;
        let select = if section == "Supertypes" {
            "<down>"
        } else {
            "<down><down>"
        };
        helpers::keys(&mut f.app, &format!("{select}<ret>")).await?; // section Enter expands
        f.wait(name).await?;
        helpers::keys(&mut f.app, "<down>").await?;
        f.wait(&format!("{name}_source")).await?;
        assert_eq!(f.count(method), 1);
        let log = lsp::read_log(&f.log)?;
        let initialize = log
            .iter()
            .find(|msg| msg["method"] == "initialize")
            .unwrap();
        assert_eq!(
            initialize["params"]["capabilities"]["textDocument"]["typeHierarchy"]
                ["dynamicRegistration"],
            false
        );
        let child_request = log.iter().find(|msg| msg["method"] == method).unwrap();
        assert_eq!(
            child_request["params"]["item"]["data"]["token"],
            serde_json::json!([1,{"opaque":true}])
        );
        f.close().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_search_retains_ancestors_and_cached_collapsed_children() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    helpers::keys(&mut f.app, "gH").await?;
    f.wait("Incoming calls").await?;
    helpers::keys(&mut f.app, "<down><tab>").await?;
    f.wait("caller").await?;
    helpers::keys(&mut f.app, "<tab>caller").await?;
    f.wait("caller").await?;
    let screen = f.screen();
    assert!(screen.contains("Incoming calls"), "{screen}");
    assert!(screen.contains("target"), "{screen}");
    assert!(!screen.contains("Outgoing calls"), "{screen}");
    assert_eq!(f.count("callHierarchy/incomingCalls"), 1);
    helpers::keys(&mut f.app, "<ret>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.path(),
        Some(f.path.with_extension("hierarchy-test.caller").as_path())
    );
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_rejects_stale_preparation_and_invalidates_cached_trees_on_edits(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    let (view, doc) = current_ref!(f.app.editor);
    let (document, view) = (doc.id(), view.id);
    let task = f
        .app
        .editor
        .hierarchy(document, view, HierarchyKind::Call)?;
    let update = task.run().await.unwrap();
    current!(f.app.editor)
        .1
        .set_selection(view, Selection::point(2));
    assert!(update.resolve(&f.app.editor).is_none());
    helpers::keys(&mut f.app, "gH").await?;
    f.wait("Incoming calls").await?;
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
    view::editing::apply(doc, &transaction, view.id).unwrap();
    f.wait("Hierarchy changed").await?;
    assert!(!f.screen().contains("Incoming calls"));
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_handles_null_empty_and_unsupported_providers() -> anyhow::Result<()> {
    for (arg, expected) in [
        ("--null-hierarchy", "No hierarchy found"),
        ("--no-hierarchy", "No configured language server supports"),
        ("--empty-hierarchy", "Incoming calls (none)"),
    ] {
        let mut f = Fixture::new(&[arg]).await?;
        helpers::keys(&mut f.app, "gH").await?;
        if arg == "--empty-hierarchy" {
            f.wait("Incoming calls").await?;
            helpers::keys(&mut f.app, "<down><tab>").await?;
        }
        f.wait(expected).await?;
        f.close().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_hierarchy_cancels_expansion_and_reopening_retries_it() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("children-ready"));
    let mut f = Fixture::new(&["--children-gate", gate.path().to_str().unwrap()]).await?;
    helpers::keys(&mut f.app, "gH").await?;
    f.wait("Incoming calls").await?;
    helpers::keys(&mut f.app, "<down><tab>").await?;
    f.wait("Incoming calls (loading").await?;
    helpers::keys(&mut f.app, "<esc>").await?;
    gate.release()?;
    helpers::keys(&mut f.app, "").await?;
    assert!(!f.screen().contains("Call Hierarchy"));
    helpers::keys(&mut f.app, "<space>'").await?;
    f.wait("caller").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 2);
    assert!(lsp::read_log(&f.log)?
        .iter()
        .any(|msg| msg["method"] == "$/cancelRequest"));
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_keeps_identical_symbols_from_different_providers_separate() -> anyhow::Result<()>
{
    let mut f = Fixture::with_servers(&["first", "second"], &[]).await?;
    let (view, doc) = current_ref!(f.app.editor);
    let (document, view) = (doc.id(), view.id);
    let task = f
        .app
        .editor
        .hierarchy(document, view, HierarchyKind::Type)?;
    let (session, roots) = task.run().await.unwrap().resolve(&f.app.editor).unwrap();
    assert_eq!(roots.len(), 2);
    assert!(!roots[0].same_symbol(&roots[1]));
    for root in roots {
        let children = session
            .children(
                &f.app.editor,
                &root,
                view::features::hierarchy::HierarchyDirection::Subtypes,
            )?
            .await?;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].detail, root.detail);
    }
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("first");
    let task = f
        .app
        .editor
        .hierarchy(document, view, HierarchyKind::Call)?;
    let (session, roots) = task.run().await.unwrap().resolve(&f.app.editor).unwrap();
    current!(f.app.editor)
        .1
        .remove_language_server_by_name("second");
    assert!(!session.item_is_current(&f.app.editor, &roots[0]));
    assert!(session
        .children(
            &f.app.editor,
            &roots[0],
            view::features::hierarchy::HierarchyDirection::Incoming
        )
        .is_err());
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_expansion_can_retry_without_requesting_in_a_loop() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["--children-error"]).await?;
    helpers::keys(&mut f.app, "gH").await?;
    f.wait("Incoming calls").await?;
    helpers::keys(&mut f.app, "<down><tab>").await?;
    f.wait("Incoming calls (failed").await?;
    helpers::keys(&mut f.app, "").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 1);
    helpers::keys(&mut f.app, "<tab>").await?;
    // Row replacements publish asynchronously; wait before sending another item action.
    f.wait("▸ Incoming calls (failed").await?;
    helpers::keys(&mut f.app, "<tab>").await?;
    f.wait_for_requests("callHierarchy/incomingCalls", 2)
        .await?;
    f.wait("Incoming calls (failed").await?;
    assert_eq!(f.count("callHierarchy/incomingCalls"), 2);
    f.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_can_stay_open_after_jumping_to_a_related_document() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[]).await?;
    helpers::keys(&mut f.app, "gT").await?;
    f.wait("Supertypes").await?;
    helpers::keys(&mut f.app, "<down><tab>").await?;
    f.wait("parent").await?;
    helpers::keys(&mut f.app, "<down><A-ret>").await?;
    f.wait("Type Hierarchy").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.path(),
        Some(f.path.with_extension("hierarchy-test.parent").as_path())
    );
    assert!(f.screen().contains("Supertypes"));
    helpers::keys(&mut f.app, "<C-v>").await?;
    assert_eq!(f.app.editor.tree.views().count(), 2);
    assert!(!f.screen().contains("Type Hierarchy"));
    let (view, doc) = current!(f.app.editor);
    let transaction = Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
    view::editing::apply(doc, &transaction, view.id).unwrap();
    helpers::keys(&mut f.app, "<space>'").await?;
    f.wait("Hierarchy changed").await?;
    f.close().await
}

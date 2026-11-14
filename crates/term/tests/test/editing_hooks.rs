//! Real transport coverage for typing and bounded pre-save edits.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use editor_core::{Selection, Transaction};
use term::application::TestApplication;
use view::{
    current, current_ref,
    editor::Action,
    save::{self, SaveProgress, WriteOptions},
};

use super::helpers::{
    lsp::{self, Gate, ServerConfig},
    run_event_loop_until_idle, test_config, AppBuilder,
};

struct Fixture {
    app: TestApplication,
    path: PathBuf,
    log: PathBuf,
}

impl Fixture {
    async fn new(dir: &Path, args: &[&str], on_type: bool) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize"));
        let mut server =
            ServerConfig::feature("hooks", "--editing-hooks", dir).initialize_gate(&gate);
        for arg in args {
            server = server.arg(arg);
        }
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.format.on_type = on_type;
        config.editor.completion.auto_trigger = false;
        config.editor.files.auto_save.focus_lost = true;
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(lsp::syntax_loader(
                "hooks-test",
                "hooks-test",
                &["hooks"],
                &server.toml(),
            ))
            .build()?;
        let path = dir.join("document.hooks-test");
        std::fs::write(&path, "😀 foo\n")?;
        app.editor.open(&path, Action::Replace)?;
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(5));
        Ok(Self {
            app,
            path,
            log: lsp::log_path(dir, "hooks"),
        })
    }

    fn text(&self) -> String {
        current_ref!(self.app.editor).1.text().to_string()
    }

    fn replace(&mut self, text: &str) -> anyhow::Result<()> {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &change, view.id)?;
        Ok(())
    }

    fn prepare(&mut self, format: bool) -> anyhow::Result<save::PreparedSave> {
        let (view, doc) = current_ref!(self.app.editor);
        let (view, doc) = (view.id, doc.id());
        Ok(save::prepare(
            &mut self.app.editor,
            doc,
            Some(view),
            None,
            WriteOptions {
                force: false,
                auto_format: format,
                code_actions: false,
            },
        )?)
    }

    async fn idle(&mut self) -> anyhow::Result<()> {
        tokio::time::timeout(
            Duration::from_secs(5),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn on_type_formats_only_triggers_and_keeps_typing_undo_group() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &[], true).await?;
    f.app.editor.enter_insert_mode();
    f.app.editor.insert_char(';')?;
    f.idle().await?;
    assert_eq!(f.text(), "😀 formatted;\n");
    let request = lsp::wait_message(&f.log, "textDocument/onTypeFormatting").await?;
    assert_eq!(request["params"]["position"]["character"], 7);
    assert_eq!(request["params"]["ch"], ";");
    f.app.editor.enter_normal_mode();
    assert!(f.app.editor.change_history(buffer::HistoryAction::Undo));
    assert_eq!(f.text(), "😀 foo\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn on_type_handles_newlines_and_skips_disabled_or_multiple_cursors() -> anyhow::Result<()> {
    for mode in ["newline", "disabled", "multiple", "nontrigger"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), &[], mode != "disabled").await?;
        f.app.editor.enter_insert_mode();
        if mode == "multiple" {
            let (view, doc) = current!(f.app.editor);
            doc.set_selection(
                view.id,
                Selection::new(
                    vec![editor_core::Range::point(2), editor_core::Range::point(5)].into(),
                    0,
                ),
            );
        }
        if mode == "newline" {
            f.app.editor.insert_newline()?;
        } else {
            f.app
                .editor
                .insert_char(if mode == "nontrigger" { 'x' } else { ';' })?;
        }
        f.idle().await?;
        let messages = lsp::read_log(&f.log)?;
        let requests: Vec<_> = messages
            .iter()
            .filter(|m| m["method"] == "textDocument/onTypeFormatting")
            .collect();
        if mode == "newline" {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["params"]["ch"], "\n");
            assert!(f.text().contains("formatted"));
        } else {
            assert!(requests.is_empty(), "{mode}");
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn on_type_rejects_late_results_after_edits_selection_or_view_changes() -> anyhow::Result<()>
{
    for change in ["edit", "selection", "view", "server", "mode"] {
        let dir = tempfile::tempdir()?;
        let gate = Gate::new(dir.path().join("response"));
        let mut f = Fixture::new(
            dir.path(),
            &["--hook-response-gate", gate.path().to_str().unwrap()],
            true,
        )
        .await?;
        f.app.editor.enter_insert_mode();
        f.app.editor.insert_char(';')?;
        lsp::wait_message(&f.log, "textDocument/onTypeFormatting").await?;
        let id = current_ref!(f.app.editor).1.id();
        match change {
            "edit" => f.app.editor.insert_char('x')?,
            "selection" => {
                let (view, doc) = current!(f.app.editor);
                doc.set_selection(view.id, Selection::point(0));
            }
            "view" => {
                f.app.editor.new_file(Action::Replace);
                f.app.editor.switch(id, Action::Replace);
            }
            "server" => {
                let server = f
                    .app
                    .editor
                    .document(id)
                    .unwrap()
                    .language_servers()
                    .next()
                    .unwrap()
                    .id();
                f.app.editor.handle_language_server_exit(server);
            }
            "mode" => f.app.editor.enter_normal_mode(),
            _ => unreachable!(),
        }
        gate.release()?;
        f.idle().await?;
        assert!(
            !f.app
                .editor
                .document(id)
                .unwrap()
                .text()
                .to_string()
                .contains("formatted"),
            "{change}"
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn presave_runs_after_formatting_and_before_did_save() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &["--document-formatting"], false).await?;
    let request = f.prepare(true)?;
    f.app.editor.submit_save(request)?;
    f.app.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&f.path)?, "😀 saved\n");
    lsp::wait_message(&f.log, "textDocument/didSave").await?;
    let messages = lsp::read_log(&f.log)?;
    let index = |method| messages.iter().position(|m| m["method"] == method).unwrap();
    assert!(index("textDocument/formatting") < index("textDocument/willSave"));
    assert!(index("textDocument/willSave") < index("textDocument/willSaveWaitUntil"));
    assert!(index("textDocument/willSaveWaitUntil") < index("textDocument/didSave"));
    assert_eq!(
        messages[index("textDocument/didSave")]["params"]["text"],
        "😀 saved\n"
    );
    let initialize = &messages[index("initialize")]["params"]["capabilities"]["textDocument"];
    assert_eq!(initialize["synchronization"]["willSave"], true);
    assert_eq!(initialize["synchronization"]["willSaveWaitUntil"], true);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn presave_providers_run_once_in_configured_order_against_latest_text() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let gate = Gate::new(dir.path().join("initialize"));
    let servers = ["first", "second"]
        .into_iter()
        .map(|name| {
            ServerConfig::feature(name, "--editing-hooks", dir.path())
                .initialize_gate(&gate)
                .arg("--append-hook-name")
                .toml()
        })
        .collect::<String>();
    let mut config = test_config();
    config.editor.lsp.enable = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(lsp::syntax_loader(
            "hooks-test",
            "hooks-test",
            &["first", "second"],
            &servers,
        ))
        .build()?;
    let path = dir.path().join("document.hooks-test");
    std::fs::write(&path, "😀 foo\n")?;
    let id = app.editor.open(&path, Action::Replace)?;
    gate.release()?;
    lsp::initialize(&mut app, 2).await?;
    let view = current_ref!(app.editor).0.id;
    let request = save::prepare(
        &mut app.editor,
        id,
        Some(view),
        None,
        WriteOptions {
            force: false,
            auto_format: false,
            code_actions: false,
        },
    )?;
    app.editor.submit_save(request)?;
    app.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "😀 foo first second\n");
    for name in ["first", "second"] {
        let messages = lsp::read_log(&lsp::log_path(dir.path(), name))?;
        assert_eq!(
            messages
                .iter()
                .filter(|m| m["method"] == "textDocument/willSaveWaitUntil")
                .count(),
            1
        );
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn presave_failures_invalid_edits_and_timeouts_still_save() -> anyhow::Result<()> {
    for flag in ["--hooks-error", "--hooks-invalid", "--hooks-ignore"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), &[flag], false).await?;
        f.replace("😀 original\n")?;
        let started = std::time::Instant::now();
        let request = f.prepare(false)?;
        f.app.editor.submit_save(request)?;
        f.app.editor.flush_writes().await?;
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(std::fs::read_to_string(&f.path)?, "😀 original\n", "{flag}");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn presave_discards_stale_edits_and_writes_current_text() -> anyhow::Result<()> {
    for change in ["edit", "server", "uri"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), &[], false).await?;
        let request = f.prepare(false)?;
        let SaveProgress::Pending(task) = save::submit(&mut f.app.editor, request)? else {
            panic!("pre-save request missing")
        };
        let update = task.run().await?;
        let id = current_ref!(f.app.editor).1.id();
        match change {
            "edit" => f.replace("😀 newer\n")?,
            "server" => {
                let server = f
                    .app
                    .editor
                    .document(id)
                    .unwrap()
                    .language_servers()
                    .next()
                    .unwrap()
                    .id();
                f.app.editor.handle_language_server_exit(server);
            }
            "uri" => {
                let path = dir.path().join("renamed.hooks-test");
                f.app.editor.set_doc_path(id, &path);
                f.path = path;
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            update.apply(&mut f.app.editor)?,
            SaveProgress::Queued
        ));
        f.app.editor.flush_writes().await?;
        let expected = if change == "edit" {
            "😀 newer\n"
        } else {
            "😀 foo\n"
        };
        assert_eq!(std::fs::read_to_string(&f.path)?, expected, "{change}");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn presave_respects_server_interest_and_manual_delay_focus_reasons() -> anyhow::Result<()> {
    for flag in ["--will-save-only", "--no-save-hooks"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), &[flag], false).await?;
        let request = f.prepare(false)?;
        f.app.editor.submit_save(request)?;
        f.app.editor.flush_writes().await?;
        lsp::wait_message(&f.log, "textDocument/didSave").await?;
        let messages = lsp::read_log(&f.log)?;
        assert!(!messages
            .iter()
            .any(|m| m["method"] == "textDocument/willSaveWaitUntil"));
        assert_eq!(
            messages
                .iter()
                .any(|m| m["method"] == "textDocument/willSave"),
            flag == "--will-save-only"
        );
        assert!(f.app.close().await.is_empty());
    }
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), &[], false).await?;
    let request = f.prepare(false)?;
    f.app.editor.submit_save(request)?;
    f.app.editor.flush_writes().await?;
    f.replace("😀 delay\n")?;
    save::auto_save(&mut f.app.editor)?;
    f.app.editor.flush_writes().await?;
    f.replace("😀 focus\n")?;
    view::features::auto_save::focus_lost(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    let messages = lsp::read_log(&f.log)?;
    let reasons: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "textDocument/willSave")
        .map(|m| m["params"]["reason"].as_i64().unwrap())
        .collect();
    assert_eq!(reasons, [1, 2, 3]);
    assert_eq!(std::fs::read_to_string(&f.path)?, "😀 saved\n");
    assert!(f.app.close().await.is_empty());
    Ok(())
}

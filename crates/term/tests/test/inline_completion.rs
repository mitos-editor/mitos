use std::{path::Path, time::Duration};
use stdx::rope::RopeExt as _;

use anyhow::Context as _;
use editor_core::Selection;
use lsp_client::lsp::InlineCompletionTriggerKind;
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    document::Mode,
    editor::EditorEvent,
    features::{
        completion::{self, CompletionHandler},
        inline_completion::{self, InlineCompletionHandler},
    },
};

use super::helpers::lsp::{self, Gate, ServerConfig};
use super::helpers::test_config;

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    completion_callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    resolve_gate: Gate,
}

impl Fixture {
    async fn new(dir: &Path, array: bool, text: &str) -> anyhow::Result<Self> {
        Self::with_options(dir, array, text, &[]).await
    }

    async fn with_options(
        dir: &Path,
        array: bool,
        text: &str,
        options: &[&str],
    ) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize"));
        gate.close()?;
        let mut server =
            ServerConfig::feature("inline", "--inline-completion", dir).initialize_gate(&gate);
        let resolve_gate = Gate::new(dir.join("resolve-ready"));
        resolve_gate.release()?;
        server = server.arg("--resolve-gate").arg(resolve_gate.path());
        for option in options {
            server = server.arg(option);
        }
        if array {
            server = server.arg("--array");
        }
        let mut config = test_config();
        config.editor.completion.auto_trigger = false;
        config.keys = toml::from_str(
            r#"[insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_previous"
[insert.g]
C-n = "inline_completion_accept"
C-p = "move_char_left"
[insert.z]
h = "move_char_left"
"#,
        )?;
        config.editor.inline_completion.auto_trigger = true;
        config.editor.inline_completion.trigger_delay = Duration::from_millis(5);
        let (completion_sender, completion_callbacks) =
            super::helpers::callbacks::unbounded(|blocking, callback| (blocking, callback));
        let (mut app, callbacks) = lsp::feature_app_with_servers(
            dir,
            text,
            "inline-test",
            &["inline"],
            config,
            &server.toml(),
            |handlers, sender, config| {
                handlers.inline_completions = InlineCompletionHandler::new(sender);
                handlers.completions = CompletionHandler::new(completion_sender, config);
            },
        )?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(4));
        app.editor.set_mode(Mode::Insert);
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self {
            app,
            callbacks,
            completion_callbacks,
            resolve_gate,
        })
    }

    async fn completion_callback(&mut self) -> anyhow::Result<()> {
        let (_, callback) =
            tokio::time::timeout(Duration::from_secs(5), self.completion_callbacks.recv())
                .await?
                .context("completion callback destination closed")?;
        callback(&mut self.app.editor);
        while let Some(update) = completion::next_update(&mut self.app.editor) {
            self.app
                .handle_editor_event(EditorEvent::Completion(update))
                .await;
        }
        Ok(())
    }

    async fn open_menu(&mut self) -> anyhow::Result<()> {
        self.key("<C-x>").await?;
        while !completion::is_shown(&self.app.editor) {
            self.completion_callback().await?;
        }
        Ok(())
    }

    async fn resolve_selection(&mut self, key: &str) -> anyhow::Result<()> {
        self.resolve_gate.close()?;
        self.key(key).await?;
        self.trigger();
        self.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            super::helpers::run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        assert!(self.callbacks.try_recv().is_err());
        assert!(self.preview().is_none());
        self.resolve_gate.release()?;
        self.completion_callback().await?;
        Ok(())
    }
    async fn key(&mut self, input: &str) -> anyhow::Result<()> {
        for key in ui_core::input::parse_macro(input)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(tui::backend::native::adapt(key));
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(tui::backend::native::adapt(key));
            self.app.handle_terminal_events(Ok(event)).await;
        }
        Ok(())
    }
    fn trigger(&mut self) {
        inline_completion::trigger(&mut self.app.editor, InlineCompletionTriggerKind::Invoked);
    }
    async fn next(&mut self) -> anyhow::Result<(bool, EditorCallback)> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("inline callback destination closed")
    }
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        super::helpers::callbacks::response(&mut self.app, &mut self.callbacks).await
    }
    fn preview(&self) -> Option<&view::document::InlineCompletion> {
        let (view, doc) = current_ref!(self.app.editor);
        doc.inline_completion(view.id)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_previews_cycles_and_accepts_utf16_replacements() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.trigger();
    f.response().await?(&mut f.app.editor);
    assert_eq!(f.preview().unwrap().lines, ["int(界)", "\treturn 1!"]);
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.text().to_string(), "🙂 pr!\nnext\n");
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        4
    );
    f.app.editor.theme = theme::Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.text" = "white"
        "ui.selection" = { bg = "black" }
        "ui.cursor" = { fg = "white", bg = "black" }
        "ui.virtual" = "blue"
        "ui.virtual.inlay-hint" = "green"
    "#,
    )?);
    // Ghost text paints over the suffix without moving the insertion cursor.
    f.app.editor.reset_idle_timer();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::helpers::run_event_loop_until_idle(&mut f.app),
    )
    .await?;
    let cursor = f.app.editor.cursor().0.unwrap();
    let (view, doc) = current_ref!(f.app.editor);
    let return_col = view.inner_area(doc).x + doc.tab_width() as u16;
    let buffer = f.app.terminal_backend().buffer();
    assert_eq!(buffer[(cursor.col as u16, cursor.row as u16)].symbol(), "i");
    assert_eq!(
        buffer[(cursor.col as u16, cursor.row as u16)].fg,
        tui::style::Color::Green
    );
    assert_eq!(
        buffer[(cursor.col as u16 + 3, cursor.row as u16)].symbol(),
        "("
    );
    assert_eq!(buffer[(return_col, cursor.row as u16 + 1)].symbol(), "r");
    f.key("<A-n>").await?;
    assert_eq!(f.preview().unwrap().text, "# alternate");
    f.key("<A-p>").await?;
    f.key("<C-y>").await?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.text().to_string(), "🙂 print(界)\n\treturn 1!\nnext\n");
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        20
    );
    let request: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?
            .lines()
            .next()
            .unwrap(),
    )?;
    assert_eq!(request["position"]["character"], 5);
    assert_eq!(request["context"]["triggerKind"], 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_rejects_queued_results_after_cursor_move_or_dismissal(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), true, "🙂 pr!\nnext\n").await?;
    f.trigger();
    let response = f.response().await?;
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(3));
    doc.set_selection(view.id, Selection::point(4));
    response(&mut f.app.editor);
    assert!(f.preview().is_none());
    f.trigger();
    let response = f.response().await?;
    inline_completion::dismiss(&mut f.app.editor);
    response(&mut f.app.editor);
    assert!(f.preview().is_none());
    f.trigger();
    f.response().await?(&mut f.app.editor);
    assert!(f.preview().is_some());
    f.app.editor.enter_normal_mode();
    assert!(f.preview().is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_automatic_requests_and_queued_triggers_respect_dismissal(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.key(" ").await?;
    f.response().await?(&mut f.app.editor);
    assert!(f.preview().is_some());
    let request: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?
            .lines()
            .next()
            .unwrap(),
    )?;
    assert_eq!(request["context"]["triggerKind"], 2);
    f.key(" ").await?;
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    inline_completion::dismiss(&mut f.app.editor);
    callback(&mut f.app.editor);
    assert!(f.preview().is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_renders_at_eof_without_a_final_newline() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr").await?;
    f.trigger();
    f.response().await?(&mut f.app.editor);
    assert_eq!(f.preview().unwrap().lines, ["int(界)", "\treturn 1"]);
    f.app.editor.reset_idle_timer();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::helpers::run_event_loop_until_idle(&mut f.app),
    )
    .await?;
    let cursor = f.app.editor.cursor().0.unwrap();
    assert_eq!(
        f.app.terminal_backend().buffer()[(cursor.col as u16, cursor.row as u16)].symbol(),
        "i"
    );
    f.key("<C-y>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "🙂 print(界)\n\treturn 1"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_acceptance_repeats_the_edit_without_waiting_for_the_server(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr\n🙂 pr\n").await?;
    f.app.editor.enter_normal_mode();
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(view.id, Selection::point(3));
    f.key("a<C-g>").await?;
    f.response().await?(&mut f.app.editor);
    f.key("<C-y><esc>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "🙂 print(界)\n\treturn 1\n🙂 pr\n"
    );
    let (view, doc) = current!(f.app.editor);
    doc.set_selection(
        view.id,
        Selection::point(doc.text().line_to_char_idx(2) + 3),
    );
    f.key(".").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        "🙂 print(界)\n\treturn 1\n🙂 print(界)\n\treturn 1\n"
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_movement_cancels_a_debounced_inline_request() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.key("x").await?;
    let (scheduling, callback) = f.next().await?;
    assert!(scheduling);
    f.key("<left>").await?;
    callback(&mut f.app.editor);
    assert!(f.preview().is_none());
    assert!(std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?.is_empty());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_completion_preserves_a_soft_wrapped_document_suffix() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let suffix = " suffix_word".repeat(15);
    let text = format!("🙂 pr{suffix}\nnext\n");
    let mut f = Fixture::new(dir.path(), false, &text).await?;
    let mut config = (*f.app.editor.config()).clone();
    config.soft_wrap.enable = Some(true);
    f.app
        .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
    f.key("<C-g>").await?;
    f.response().await?(&mut f.app.editor);
    f.app.editor.reset_idle_timer();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::helpers::run_event_loop_until_idle(&mut f.app),
    )
    .await?;
    let (view, doc) = current_ref!(f.app.editor);
    let inner = view.inner_area(doc);
    let buffer = f.app.terminal_backend().buffer();
    let visible: String = (inner.y..inner.bottom())
        .flat_map(|y| (inner.x..inner.right()).map(move |x| buffer[(x, y)].symbol()))
        .collect();
    assert_eq!(visible.matches("suffix_word").count(), 15, "{visible}");
    assert_eq!(doc.text().to_string(), text);
    f.key("<C-y>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.text().to_string(),
        format!("🙂 print(界)\n\treturn 1{suffix}\nnext\n")
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_requests_extend_the_selected_completion_using_live_utf16_ranges(
) -> anyhow::Result<()> {
    for preview in [true, false] {
        let dir = tempfile::tempdir()?;
        let options = if preview {
            &[][..]
        } else {
            &["--no-resolve"][..]
        };
        let mut f = Fixture::with_options(dir.path(), false, "🙂 pr!\nnext\n", options).await?;
        let mut config = (*f.app.editor.config()).clone();
        config.completion.preview_insert = preview;
        f.app
            .apply_config_update(term::config::ConfigEvent::UpdateEditor(Box::new(config)));
        f.open_menu().await?;
        f.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            super::helpers::run_event_loop_until_idle(&mut f.app),
        )
        .await?;
        if preview {
            f.resolve_selection("<C-n>").await?;
        } else {
            f.key("<C-n>").await?;
        }
        let old = f.response().await?;
        let first: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?
                .lines()
                .last()
                .unwrap(),
        )?;
        let first = &first["context"]["selectedCompletionInfo"];
        assert_eq!(
            first["range"],
            serde_json::json!({"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 5}})
        );
        if preview {
            f.resolve_selection("<C-n>").await?;
        } else {
            f.key("<C-n>").await?;
        }
        old(&mut f.app.editor);
        assert!(f.preview().is_none());
        f.response().await?(&mut f.app.editor);
        let last: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?
                .lines()
                .last()
                .unwrap(),
        )?;
        let selected = &last["context"]["selectedCompletionInfo"];
        assert_ne!(first["text"], selected["text"]);
        assert_eq!(
            f.preview().unwrap().text,
            format!("{}(界)", selected["text"].as_str().unwrap())
        );
        assert_eq!(current_ref!(f.app.editor).1.text(), "🙂 pr!\nnext\n");
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            4
        );
        assert!(f
            .app
            .editor
            .completion_presentation(doc)
            .inline_completion(view.id)
            .is_some());
        f.key("<C-e>").await?;
        assert!(f.preview().is_none());
        assert!(view::features::completion::is_shown(&f.app.editor));
        f.key("<C-g>").await?;
        f.response().await?(&mut f.app.editor);
        f.key(if preview { "g<C-n>" } else { "<C-y>" }).await?;
        assert_eq!(
            current_ref!(f.app.editor).1.text().to_string(),
            format!("🙂 {}(界)!\nnext\n", selected["text"].as_str().unwrap())
        );
        assert!(!view::features::completion::is_shown(&f.app.editor));
        f.key("<esc>u").await?;
        assert_eq!(current_ref!(f.app.editor).1.text(), "🙂 pr!\nnext\n");
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn follow_up_inline_requests_after_ordinary_completion_acceptance() -> anyhow::Result<()> {
    assert_follow_up_after_completion_acceptance(false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn follow_up_inline_requests_after_inline_extension_acceptance() -> anyhow::Result<()> {
    assert_follow_up_after_completion_acceptance(true).await
}

async fn assert_follow_up_after_completion_acceptance(extension: bool) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f =
        Fixture::with_options(dir.path(), false, "🙂 pr!\nnext\n", &["--no-resolve"]).await?;
    f.open_menu().await?;
    f.key("<C-n>").await?;
    let selected = f.response().await?;
    if extension {
        selected(&mut f.app.editor);
        assert!(f.preview().is_some());
        f.key("<C-y>").await?;
    } else {
        f.key("<ret>").await?;
        selected(&mut f.app.editor);
        assert!(f.preview().is_none());
    }
    f.app.editor.reset_idle_timer();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::helpers::run_event_loop_until_idle(&mut f.app),
    )
    .await?;
    assert!(!completion::is_shown(&f.app.editor));
    f.response().await?(&mut f.app.editor);
    let requests = lsp::read_log(&lsp::log_path(dir.path(), "inline"))?;
    let follow_up = requests
        .last()
        .context("missing follow-up inline request")?;
    assert_eq!(follow_up["context"]["triggerKind"], 2);
    assert!(follow_up["context"].get("selectedCompletionInfo").is_none());
    let (view, doc) = current_ref!(f.app.editor);
    let server = doc.language_servers().next().unwrap();
    assert_eq!(
        follow_up["position"],
        serde_json::to_value(doc.position(view.id, server.offset_encoding()))?
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dismissing_the_completion_menu_cancels_its_queued_inline_result() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f =
        Fixture::with_options(dir.path(), false, "🙂 pr!\nnext\n", &["--no-resolve"]).await?;
    f.open_menu().await?;
    f.key("<C-n>").await?;
    let selected = f.response().await?;
    f.key("<C-c>").await?;
    selected(&mut f.app.editor);
    assert!(f.preview().is_none());
    assert!(!completion::is_shown(&f.app.editor));
    assert_eq!(current_ref!(f.app.editor).1.text(), "🙂 pr!\nnext\n");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), f.callbacks.recv())
            .await
            .is_err()
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_insert_prefixes_still_accept_the_selected_menu_item() -> anyhow::Result<()> {
    for (prefix, command) in [("z", "h"), ("g", "<C-p>")] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
        f.open_menu().await?;
        f.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            super::helpers::run_event_loop_until_idle(&mut f.app),
        )
        .await?;
        f.resolve_selection("<C-n>").await?;
        f.response().await?(&mut f.app.editor);
        let request: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?
                .lines()
                .last()
                .unwrap(),
        )?;
        let selected = request["context"]["selectedCompletionInfo"]["text"]
            .as_str()
            .unwrap();
        let expected = format!("🙂 {selected}!\nnext\n");
        f.key(prefix).await?;
        if prefix == "g" {
            assert_eq!(current_ref!(f.app.editor).1.text(), "🙂 pr!\nnext\n");
            assert!(view::features::completion::is_shown(&f.app.editor));
        } else {
            assert_eq!(current_ref!(f.app.editor).1.text().to_string(), expected);
            assert!(!view::features::completion::is_shown(&f.app.editor));
        }
        f.key(command).await?;
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.text().to_string(), expected);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2 + selected.chars().count() - 1
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_additional_edits_or_failing_resolution_keeps_normal_completion_available(
) -> anyhow::Result<()> {
    for option in ["--resolve-additional-edits", "--resolve-error"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::with_options(dir.path(), false, "🙂 pr!\nnext\n", &[option]).await?;
        f.open_menu().await?;
        f.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            super::helpers::run_event_loop_until_idle(&mut f.app),
        )
        .await?;
        f.resolve_selection("<C-n>").await?;
        f.trigger();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), f.callbacks.recv())
                .await
                .is_err()
        );
        let doc = current_ref!(f.app.editor).1;
        let expected = f.app.editor.completion_presentation(doc).text().to_string();
        assert!(expected.contains("print"));
        assert_eq!(
            expected.starts_with("use fruit;\n"),
            option == "--resolve-additional-edits"
        );
        assert_eq!(doc.text(), "🙂 pr!\nnext\n");
        assert!(f.preview().is_none());
        f.key("<ret>").await?;
        assert_eq!(current_ref!(f.app.editor).1.text().to_string(), expected);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_requests_do_not_discard_selected_snippets_or_additional_edits() -> anyhow::Result<()>
{
    for text in ["🙂 pr! snippet\n", "x.fo postfix\n"] {
        let dir = tempfile::tempdir()?;
        let mut f = Fixture::new(dir.path(), false, text).await?;
        f.open_menu().await?;
        f.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(5),
            super::helpers::run_event_loop_until_idle(&mut f.app),
        )
        .await?;
        let before = std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?;
        f.key("<C-n>").await?;
        f.trigger();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), f.callbacks.recv())
                .await
                .is_err()
        );
        assert!(f.preview().is_none());
        assert_eq!(
            std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?,
            before
        );
        assert_eq!(current_ref!(f.app.editor).1.text().to_string(), text);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_view_cancels_its_queued_inline_completion() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.trigger();
    let response = f.response().await?;
    let (view, doc) = current!(f.app.editor);
    let view_id = view.id;
    doc.remove_view(view_id);
    response(&mut f.app.editor);
    assert!(f.preview().is_none());
    // Restore the still-visible fixture view before the application's normal shutdown.
    current!(f.app.editor).1.ensure_view_init(view_id);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

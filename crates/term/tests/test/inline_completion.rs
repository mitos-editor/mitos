use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::Selection;
use lsp_client::lsp::InlineCompletionTriggerKind;
use term::application::Application;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback,
    current, current_ref,
    document::Mode,
    editor::Action,
    handlers::inline_completion::{self, InlineCompletionHandler},
};

use super::helpers::lsp::{self, Gate, ServerConfig};
use super::helpers::{test_config, AppBuilder};

struct Fixture {
    app: Application,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
}

impl Fixture {
    async fn new(dir: &Path, array: bool, text: &str) -> anyhow::Result<Self> {
        let gate = Gate::new(dir.join("initialize"));
        gate.close()?;
        let mut server =
            ServerConfig::feature("inline", "--inline-completion", dir).initialize_gate(&gate);
        if array {
            server = server.arg("--array");
        }
        let loader = lsp::syntax_loader("inline-test", "inline-test", &["inline"], &server.toml());
        let mut config = test_config();
        config.editor.lsp.enable = true;
        config.editor.auto_completion = false;
        config.keys = toml::from_str(
            r#"[insert]
C-y = "inline_completion_accept"
C-e = "inline_completion_dismiss"
C-g = "inline_completion_trigger"
A-n = "inline_completion_next"
A-p = "inline_completion_prev"
"#,
        )?;
        config.editor.inline_completion_auto_trigger = true;
        config.editor.inline_completion_timeout = Duration::from_millis(5);
        let (sender, callbacks) =
            super::helpers::callbacks::unbounded(|blocking, callback| (blocking, callback));
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_lang_loader(loader)
            .with_handler_setup(move |handlers, _| {
                handlers.inline_completions = InlineCompletionHandler::new(sender);
            })
            .build()?;
        let path = dir.join("document.inline-test");
        std::fs::write(&path, text)?;
        app.editor.open(&path, Action::Replace)?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(4));
        app.editor.mode = Mode::Insert;
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        Ok(Self { app, callbacks })
    }
    async fn key(&mut self, input: &str) -> anyhow::Result<()> {
        for key in ui_core::input::parse_macro(input)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(key.into());
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(key.into());
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
        loop {
            let (scheduling, callback) = self.next().await?;
            if scheduling {
                callback(&mut self.app.editor);
            } else {
                return Ok(callback);
            }
        }
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
    f.app.editor.theme = view::theme::Theme::from(toml::from_str::<toml::Value>(
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
    doc.set_selection(view.id, Selection::point(doc.text().line_to_char(2) + 3));
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
        .handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
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
async fn inline_requests_do_not_use_the_ordinary_completion_popup_preview() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.key("<C-x>").await?;
    f.app.editor.reset_idle_timer();
    tokio::time::timeout(
        Duration::from_secs(5),
        super::helpers::run_event_loop_until_idle(&mut f.app),
    )
    .await?;
    f.key("<C-n>").await?;
    assert!(matches!(
        f.app.editor.last_completion,
        Some(view::editor::CompleteAction::Selected { .. })
    ));
    assert!(current_ref!(f.app.editor)
        .1
        .text()
        .to_string()
        .contains("print"));
    let before = std::fs::read_to_string(lsp::log_path(dir.path(), "inline"))?;
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
    f.key("<C-g>").await?;
    assert!(f.preview().is_none());
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_inline_completion_rejects_a_queued_response() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path(), false, "🙂 pr!\nnext\n").await?;
    f.trigger();
    let response = f.response().await?;
    f.app
        .editor
        .replace_inline_completion_handler(InlineCompletionHandler::new(
            view::callbacks::EditorCallbackSender::new(|_| async {}, |_| {}),
        ));
    response(&mut f.app.editor);
    assert!(f.preview().is_none());
    assert!(f.app.close().await.is_empty());
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

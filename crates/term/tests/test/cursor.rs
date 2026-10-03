use super::helpers::{test_config, AppBuilder};
use term::application::Application;
use tui::{backend::Backend, style::Color};
use ui_core::input::parse_macro;
use view::{current_ref, theme::Theme};

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

async fn keys(app: &mut Application, input: &str) {
    for key in parse_macro(input).unwrap() {
        app.handle_terminal_events(Ok(Event::Key(KeyEvent::from(key))))
            .await;
    }
}

fn assert_editor_cursor(app: &Application, native: bool) {
    let backend = app.terminal_backend();
    assert_eq!(backend.cursor_visible(), native);
    let pos = app.editor.cursor().0.unwrap();
    assert_eq!(
        backend.clone().get_cursor_position().unwrap(),
        (pos.col as u16, pos.row as u16).into(),
        "mode {:?}, native {native}",
        app.editor.mode()
    );
    let cell = &backend.buffer()[(pos.col as u16, pos.row as u16)];
    if !native {
        assert_eq!(cell.bg, Color::Yellow);
        assert_eq!(cell.fg, Color::Black);
    }
}

fn cursor_app(cursorline: bool, normal: &str, text: &str) -> Application {
    let mut config = test_config();
    config.editor.cursorline = cursorline;
    config.editor.cursor_shape = toml::from_str(&format!(
        "normal = '{normal}'\ninsert = 'bar'\nselect = 'underline'"
    ))
    .unwrap();
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(editor_core::syntax::Loader::default())
        .with_input_text(text)
        .build()
        .unwrap();
    app.editor.theme = Theme::from(
        toml::from_str::<toml::Value>(
            r#"
        "ui.text" = { fg = "white" }
        "ui.selection" = { bg = "green" }
        "ui.cursor" = { fg = "black", bg = "yellow" }
        "ui.cursorline.primary" = { bg = "blue" }
    "#,
        )
        .unwrap(),
    );
    app
}

#[tokio::test]
async fn cursor_modes_preserve_the_theme_and_input_method_position() {
    for cursorline in [false, true] {
        let mut app = cursor_app(cursorline, "block", "#[a|]#界b\n");
        keys(&mut app, "<esc>").await;
        assert_editor_cursor(&app, false);
        keys(&mut app, "l").await;
        assert_editor_cursor(&app, false);
        let pos = app.editor.cursor().0.unwrap();
        assert_eq!(
            app.terminal_backend().buffer()[(pos.col as u16, pos.row as u16)].symbol(),
            "界"
        );
        keys(&mut app, "i").await;
        assert_editor_cursor(&app, true);
        keys(&mut app, "<esc>v").await;
        assert_editor_cursor(&app, true);
        keys(&mut app, "<esc>l").await;
        assert_editor_cursor(&app, false);
        assert!(app.close().await.is_empty());
    }
}

#[tokio::test]
async fn prompts_and_pickers_own_the_cursor_without_erasing_the_editor_marker() {
    for shape in ["block", "bar", "underline"] {
        let mut app = cursor_app(true, shape, "#[a|]#界b\n");
        keys(&mut app, "<esc>").await;
        for open in ["/", ":", "<space>b"] {
            keys(&mut app, open).await;
            let backend = app.terminal_backend();
            assert!(backend.cursor_visible());
            let pos = app.editor.cursor().0.unwrap();
            let terminal_pos = backend.clone().get_cursor_position().unwrap();
            assert_ne!(terminal_pos, (pos.col as u16, pos.row as u16).into());
            // Full-screen pickers cover the editor; prompts leave its marker visible.
            if open != "<space>b" {
                assert_eq!(
                    backend.buffer()[(pos.col as u16, pos.row as u16)].bg,
                    Color::Yellow
                );
            }
            keys(&mut app, "<esc>").await;
            assert_editor_cursor(&app, shape != "block");
        }
        assert!(app.close().await.is_empty());
    }
}

#[tokio::test]
async fn terminal_focus_transfers_the_primary_cursor_but_keeps_secondary_markers() {
    let mut app = cursor_app(true, "block", "#[a|]#界b\n");
    {
        let (view, doc) = view::current!(app.editor);
        doc.set_selection(
            view.id,
            editor_core::Selection::new(
                vec![editor_core::Range::point(0), editor_core::Range::point(2)].into(),
                0,
            ),
        );
    }
    keys(&mut app, "<esc>").await;
    assert_editor_cursor(&app, false);
    #[cfg(not(windows))]
    let (lost, gained) = (Event::FocusOut, Event::FocusIn);
    #[cfg(windows)]
    let (lost, gained) = (Event::FocusLost, Event::FocusGained);
    app.handle_terminal_events(Ok(lost)).await;
    assert_editor_cursor(&app, true);
    let (view, doc) = current_ref!(app.editor);
    let inner = view.inner_area(doc);
    assert_eq!(
        app.terminal_backend().buffer()[(inner.x + 3, inner.y)].bg,
        Color::Yellow
    );
    app.handle_terminal_events(Ok(gained)).await;
    assert_editor_cursor(&app, false);
    assert!(app.close().await.is_empty());
}

#[tokio::test]
async fn eof_cursor_remains_visible_when_a_prompt_takes_focus() {
    for shape in ["block", "bar", "underline"] {
        let mut app = cursor_app(true, shape, "#[|]#");
        keys(&mut app, "<esc>").await;
        assert_editor_cursor(&app, shape != "block");
        keys(&mut app, "/").await;
        let pos = app.editor.cursor().0.unwrap();
        assert_eq!(
            app.terminal_backend().buffer()[(pos.col as u16, pos.row as u16)].bg,
            Color::Yellow
        );
        keys(&mut app, "<esc>").await;
        assert_editor_cursor(&app, shape != "block");
        assert!(app.close().await.is_empty());
    }
}

#[tokio::test]
async fn inlay_hints_do_not_capture_the_document_cursor() {
    use editor_core::text_annotations::InlineAnnotation;
    use view::document::{DocumentInlayHints, DocumentInlayHintsId};

    for shape in ["block", "bar"] {
        let mut app = cursor_app(true, shape, "#[a|]#b\n");
        keys(&mut app, "<esc>").await;
        let (view, doc) = view::current!(app.editor);
        let mut hints = DocumentInlayHints::empty_with_id(DocumentInlayHintsId {
            first_line: 0,
            last_line: 1,
        });
        hints
            .other_inlay_hints
            .push(InlineAnnotation::new(0, "hint: "));
        doc.set_inlay_hints(view.id, hints);
        #[cfg(not(windows))]
        let focus = Event::FocusIn;
        #[cfg(windows)]
        let focus = Event::FocusGained;
        app.handle_terminal_events(Ok(focus)).await;
        assert_editor_cursor(&app, shape != "block");
        let pos = app
            .terminal_backend()
            .clone()
            .get_cursor_position()
            .unwrap();
        assert_eq!(app.terminal_backend().buffer()[pos].symbol(), "a");
        assert!(app.close().await.is_empty());
    }
}

#[tokio::test]
async fn selection_dialog_hides_the_editor_native_cursor() {
    let mut app = cursor_app(true, "bar", "#[a|]#b\n");
    keys(&mut app, "<esc>").await;
    assert_editor_cursor(&app, true);
    let request = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "window/showMessageRequest",
        "params": {"type": 3, "message": "Choose", "actions": [{"title": "OK"}]}
    }))
    .unwrap();
    app.handle_language_server_message(request, Default::default())
        .await;
    keys(&mut app, "<down>").await;
    assert!(!app.terminal_backend().cursor_visible());
    keys(&mut app, "<esc>").await;
    assert_editor_cursor(&app, true);
    assert!(app.close().await.is_empty());
}

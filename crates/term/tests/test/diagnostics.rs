use diagnostics::{Diagnostic, DiagnosticProvider, NumberOrString, Severity};
use editor_config::DiagnosticFilter;
use theme::{Color, Modifier, Theme};
use view::{current, current_ref};

use super::helpers::{keys, test_config, AppBuilder};

#[tokio::test(flavor = "multi_thread")]
async fn overlay_diagnostics_preserve_severity_styles() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.diagnostics.end_of_line.severity = DiagnosticFilter::Disable;
    config.editor.diagnostics.inline.cursor_line = DiagnosticFilter::Disable;
    config.editor.diagnostics.inline.other_lines = DiagnosticFilter::Disable;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[x|]#\n")
        .build()?;
    app.editor.theme = Theme::from(toml::from_str::<toml::Value>(
        r#"
        "ui.background" = { fg = "gray", bg = "black" }
        "ui.gutter" = "gray"
        "ui.gutter.selected" = "light-gray"
        "ui.cursor" = { modifiers = ["reversed"] }
        "ui.selection" = { bg = "blue" }
        warning = { fg = "yellow", modifiers = ["bold"] }
        error = { fg = "red", modifiers = ["bold"] }
        info = { fg = "blue", modifiers = ["bold"] }
        hint = { fg = "green", modifiers = ["bold"] }
        "#,
    )?);

    for (severity, color) in [
        (Some(Severity::Error), Color::Red),
        (Some(Severity::Warning), Color::Yellow),
        (Some(Severity::Info), Color::Blue),
        (Some(Severity::Hint), Color::Green),
        (None, Color::Yellow),
    ] {
        current!(app.editor).1.replace_diagnostics(
            [Diagnostic {
                range: editor_core::Range::new(0, 1).into(),
                ends_at_word: false,
                starts_at_word: false,
                zero_width: false,
                line: 0,
                message: "diagnostic message\nsecond line".into(),
                severity,
                code: Some(NumberOrString::String("test-code".into())),
                provider: DiagnosticProvider::Spelling,
                tags: vec![],
                source: None,
                data: None,
            }],
            &[],
            None,
        );
        keys(&mut app, "<esc>").await?;
        let (view, doc) = current_ref!(app.editor);
        let area = view.inner_area(doc);
        let buffer = app.terminal_backend().buffer();
        for (row, expected) in ["diagnostic message", "second line", "(test-code)"]
            .into_iter()
            .enumerate()
        {
            let y = area.y + 1 + row as u16;
            let x = area.right() - expected.len() as u16;
            let actual: String = (x..area.right()).map(|x| buffer[(x, y)].symbol()).collect();
            assert_eq!(actual, expected);
            for x in x..area.right() {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.fg, color, "{severity:?}: {expected}");
                assert_eq!(cell.bg, Color::Black);
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
        }
    }
    Ok(())
}

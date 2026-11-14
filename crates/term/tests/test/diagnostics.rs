use diagnostics::{Diagnostic, DiagnosticProvider, NumberOrString, Severity};
use editor_config::DiagnosticFilter;
use stdx::rope::RopeExt as _;
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
                details: None,
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

fn finding(message: &str) -> Diagnostic {
    Diagnostic {
        range: diagnostics::Range { start: 0, end: 1 },
        ends_at_word: false,
        starts_at_word: false,
        zero_width: false,
        line: 0,
        message: message.into(),
        severity: Some(Severity::Error),
        code: Some(NumberOrString::String("E1".into())),
        provider: DiagnosticProvider::Spelling,
        tags: vec![],
        source: Some("test".into()),
        data: None,
        details: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn full_diagnostic_buffer_is_untruncated_readonly_and_keeps_the_source() -> anyhow::Result<()>
{
    use super::helpers::{keys, AppBuilder};
    let mut app = AppBuilder::new()
        .with_input_text("#[x|]# source\n")
        .build()?;
    let source = current_ref!(app.editor).1.id();
    let message = format!(
        "{}\nEND OF LONG DIAGNOSTIC",
        "a long compiler error line\n".repeat(200)
    );
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([finding(&message)], &[], None);
    keys(&mut app, "<space><A-d>").await?;
    let details = current_ref!(app.editor).1.id();
    let text = current_ref!(app.editor).1.text().to_string();
    assert!(text.contains(&message));
    assert!(text.contains("ERROR · test (E1)"));
    assert!(current_ref!(app.editor).1.is_readonly());
    assert!(current_ref!(app.editor).1.path().is_none());
    assert!(!current_ref!(app.editor).1.is_modified());
    assert!(current_ref!(app.editor)
        .1
        .display_name()
        .starts_with("[diagnostic:"));
    keys(&mut app, "iX<esc>").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), text);
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([], &[], None);
    assert_eq!(
        app.editor.document(details).unwrap().text().to_string(),
        text
    );
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), source);
    keys(&mut app, "<C-i>:buffer-close<ret>").await?;
    assert!(app.editor.document(details).is_none());
    assert_eq!(current_ref!(app.editor).1.id(), source);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn related_locations_use_target_encoding_without_a_language_server() -> anyhow::Result<()> {
    use super::helpers::{keys, AppBuilder};
    use diagnostics::{DiagnosticDetails, Range, RelatedInformation};
    use editor_core::{OffsetEncoding, Position, Selection};
    use std::sync::Arc;
    let directory = tempfile::tempdir()?;
    let target = stdx::path::normalize(directory.path().join("related.txt"));
    std::fs::write(&target, "😀 target\n")?;
    let mut app = AppBuilder::new()
        .with_input_text("#[x|]# source\n")
        .build()?;
    let source = current_ref!(app.editor).1.id();
    let mut diagnostic = finding("😀 full message");
    diagnostic.details = Some(Arc::new(DiagnosticDetails {
        related_information: vec![RelatedInformation {
            uri: stdx::Url::from_file_path(&target).unwrap(),
            range: Range {
                start: Position { row: 0, col: 3 },
                end: Position { row: 0, col: 9 },
            },
            offset_encoding: OffsetEncoding::Utf16,
            message: "😀 declared here".into(),
        }],
        documentation: Some(stdx::Url::parse("https://example.com/E1")?),
        rendered: None,
    }));
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([diagnostic], &[], None);
    keys(&mut app, "<space><A-d>").await?;
    let details = current_ref!(app.editor).1.id();
    let (view, doc) = current!(app.editor);
    assert_eq!(doc.reference_links().len(), 2);
    assert!(doc
        .text()
        .slice_chars(doc.reference_links()[0].start..doc.reference_links()[0].end)
        .to_string()
        .ends_with("related.txt:1:4"));
    doc.set_selection(view.id, Selection::point(doc.reference_links()[0].start));
    keys(&mut app, "<C-w>F").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.path(), Some(target.as_path()));
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "target"
    );
    keys(&mut app, "<C-o>").await?;
    assert_eq!(current_ref!(app.editor).1.id(), details);
    // Documentation resolves to the supplied URL; do not launch a browser in tests.
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(doc.reference_links()[1].start));
    use view::features::document_links::{prepare_navigation, FileNavigation, NavigationTarget};
    let FileNavigation::Ready(update) = prepare_navigation(&mut app.editor) else {
        panic!("reference links never need LSP resolution");
    };
    assert_eq!(
        update.into_targets(&app.editor).unwrap(),
        vec![NavigationTarget::Url(stdx::Url::parse(
            "https://example.com/E1"
        )?)]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diagnostic_picker_opens_rendered_output_in_a_split_and_can_be_resumed(
) -> anyhow::Result<()> {
    use super::helpers::{keys, AppBuilder};
    use diagnostics::DiagnosticDetails;
    use std::sync::Arc;
    let mut app = AppBuilder::new().with_input_text("#[x|]#\n").build()?;
    let source = current_ref!(app.editor).1.id();
    let mut selected = finding("chosen diagnostic");
    selected.details = Some(Arc::new(DiagnosticDetails {
        rendered: Some("full compiler output\nincluding notes\n".into()),
        ..Default::default()
    }));
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([finding("other diagnostic"), selected], &[], None);
    keys(&mut app, "<space>dchosen<A-s>").await?;
    assert_eq!(app.editor.tree.views().count(), 2);
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("full compiler output\nincluding notes"));
    assert_ne!(current_ref!(app.editor).1.id(), source);
    // The existing last-picker flow preserves the selected diagnostic and query.
    keys(&mut app, "<space>'<A-d>").await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("full compiler output\nincluding notes"));
    assert_eq!(app.editor.tree.views().count(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diagnostic_details_choose_overlapping_findings_and_fall_back_to_cursor_line(
) -> anyhow::Result<()> {
    use super::helpers::{keys, AppBuilder};
    let mut app = AppBuilder::new().with_input_text("#[x|]# more\n").build()?;
    let source = current_ref!(app.editor).1.id();
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([finding("first"), finding("second")], &[], None);
    keys(&mut app, "<space><A-d>").await?;
    assert_eq!(
        current_ref!(app.editor).1.id(),
        source,
        "multiple diagnostics require a choice"
    );
    keys(&mut app, "second<ret>").await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("second"));
    keys(&mut app, "<C-o>").await?;
    app.editor
        .document_mut(source)
        .unwrap()
        .replace_diagnostics([finding("line fallback")], &[], None);
    keys(&mut app, "gl<space><A-d>").await?;
    assert!(current_ref!(app.editor)
        .1
        .text()
        .to_string()
        .contains("line fallback"));
    Ok(())
}

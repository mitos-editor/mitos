use super::helpers::{keys, AppBuilder};
use theme::{Color, Modifier};
use view::{current_ref, doc, document::Mode, editor::Action};

#[tokio::test(flavor = "multi_thread")]
async fn pager_renders_styles_searches_copies_and_quits_without_modifying_text(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.editor
        .new_pager("plain \x1b[31;1m赤red\x1b[m end\n", Action::Replace);
    keys(&mut app, "l").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let red = &backend.buffer()[(area.x + 8, area.y)];
    assert_eq!(red.symbol(), "r");
    assert_eq!(red.fg, Color::Indexed(1));
    assert!(red.modifier.contains(Modifier::BOLD));
    let plain = &backend.buffer()[(area.x + 12, area.y)];
    assert_eq!(plain.symbol(), "e");
    assert!(!plain.modifier.contains(Modifier::BOLD));

    keys(&mut app, "/red<ret>y").await?;
    assert_eq!(app.editor.first_register_value('"').as_deref(), Some("red"));
    keys(&mut app, "idu").await?;
    assert_eq!(app.editor.mode(), Mode::Normal);
    let doc = doc!(app.editor);
    assert_eq!(doc.text().to_string(), "plain 赤red end\n");
    assert!(doc.is_readonly());
    assert!(!doc.is_modified());
    assert!(doc.uri().is_none());

    // A q consumed by a pending character motion must not close the pager.
    keys(&mut app, "fq").await?;
    assert!(!app.editor.should_close());
    keys(&mut app, "q").await?;
    assert!(app.editor.should_close());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pager_restores_styles_when_the_viewport_starts_inside_a_colored_run() -> anyhow::Result<()>
{
    let mut app = AppBuilder::new().build()?;
    let input = format!(
        "\x1b[38;2;12;34;56m{}\x1b[mplain\n",
        "colored\n".repeat(400)
    );
    app.editor.new_pager(&input, Action::Replace);
    keys(&mut app, "300gg").await?;
    let (view, doc) = current_ref!(app.editor);
    let area = view.inner_area(doc);
    let backend = app.terminal_backend();
    let cell = &backend.buffer()[(area.x + 1, area.y)];
    assert_eq!(cell.symbol(), "o");
    assert_eq!(cell.fg, Color::Rgb(12, 34, 56));
    keys(&mut app, "q").await?;
    assert!(app.close().await.is_empty());
    Ok(())
}

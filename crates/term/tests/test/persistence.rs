//! `[editor.persistence]` across separate editor sessions sharing one state directory.
use std::path::Path;

use editor_core::Position;
use stdx::rope::RopeExt as _;
use term::application::TestApplication as Application;
use view::{current_ref, editor::Action};

use super::helpers::{isolation, keys, test_config, AppBuilder};

fn config(enable: bool) -> term::config::Config {
    let mut config = test_config();
    let persistence = &mut config.editor.persistence;
    persistence.old_files = enable;
    persistence.commands = enable;
    persistence.search = enable;
    persistence.clipboard = enable;
    config
}

async fn session(file: &Path, pos: Option<Position>, enable: bool) -> anyhow::Result<Application> {
    let mut app = AppBuilder::new()
        .with_config(config(enable))
        .with_file(file, pos)
        .build()?;
    keys(&mut app, "").await?;
    Ok(app)
}

async fn quit(mut app: Application) -> anyhow::Result<()> {
    let errors = app.close().await;
    anyhow::ensure!(errors.is_empty(), "{errors:?}");
    Ok(())
}

fn cursor_line(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    doc.text().char_to_line_idx(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    )
}

fn first(app: &Application, register: char) -> Option<String> {
    let mut values = app.editor.registers.read(register)?;
    values.next().map(|value| value.into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn files_reopen_where_they_were_left_with_history_and_yanks() -> anyhow::Result<()> {
    let Some(root) = isolation::workspace(
        "test::persistence::files_reopen_where_they_were_left_with_history_and_yanks",
    )?
    else {
        return Ok(());
    };
    let file = root.join("workspace/notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\nfour\nfive\nneedle\n")?;
    let other = root.join("workspace/other.txt");
    std::fs::write(&other, "a\nb\nc\n")?;

    // Session 1: move to line 4, yank it, search and run a command.
    let mut app = session(&file, None, true).await?;
    keys(&mut app, "3jxy/needle<ret>3k:set mouse false<ret>").await?;
    assert_eq!(cursor_line(&app), 2);
    quit(app).await?;

    // Session 2: the file reopens on line 3, with history and the yank kept.
    let mut app = session(&file, None, true).await?;
    assert_eq!(cursor_line(&app), 2);
    assert_eq!(first(&app, ':').as_deref(), Some("set mouse false"));
    assert_eq!(first(&app, '/').as_deref(), Some("needle"));
    assert_eq!(first(&app, '"').as_deref(), Some("four\n"));
    // Closing a buffer remembers its position for reopening in the same session.
    keys(&mut app, ":o other.txt<ret>jj:bc<ret>").await?;
    app.editor.open(&other, Action::Replace)?;
    assert_eq!(cursor_line(&app), 2);
    quit(app).await?;

    // An explicit position wins over the saved one.
    let app = session(&file, Some(Position::new(0, 0)), true).await?;
    assert_eq!(cursor_line(&app), 0);
    quit(app).await?;

    // With persistence off, nothing is restored.
    let app = session(&file, None, false).await?;
    assert_eq!(cursor_line(&app), 0);
    assert!(first(&app, ':').is_none());
    quit(app).await?;
    Ok(())
}

/// Start with several files, like `ms a.txt b.txt:1`.
async fn session_with(files: &[(&Path, Option<Position>)]) -> anyhow::Result<Application> {
    let mut builder = AppBuilder::new().with_config(config(true));
    for (file, pos) in files {
        builder = builder.with_file(*file, *pos);
    }
    let mut app = builder.build()?;
    keys(&mut app, "").await?;
    Ok(app)
}

fn show(app: &mut Application, file: &Path) {
    let id = app.editor.document_id_by_path(file).expect("file is open");
    app.editor.switch(id, Action::Replace);
}

#[tokio::test(flavor = "multi_thread")]
async fn requested_lines_win_for_files_opened_in_the_background() -> anyhow::Result<()> {
    let Some(root) = isolation::workspace(
        "test::persistence::requested_lines_win_for_files_opened_in_the_background",
    )?
    else {
        return Ok(());
    };
    let a = root.join("workspace/a.txt");
    let b = root.join("workspace/b.txt");
    std::fs::write(&a, "1\n2\n3\n4\n")?;
    std::fs::write(&b, "1\n2\n3\n4\n")?;

    // Leave `b.txt` on line 3.
    let mut app = session(&b, None, true).await?;
    keys(&mut app, "jj").await?;
    quit(app).await?;

    // `ms a.txt b.txt:1` opens `b.txt` in the background on line 1.
    let mut app = session_with(&[(&a, None), (&b, Some(Position::new(0, 0)))]).await?;
    show(&mut app, &b);
    assert_eq!(cursor_line(&app), 0);
    quit(app).await?;

    // Without a line, the background file still reopens where it was left.
    let mut app = session(&b, None, true).await?;
    keys(&mut app, "jj").await?;
    quit(app).await?;
    let mut app = session_with(&[(&a, None), (&b, None)]).await?;
    show(&mut app, &b);
    assert_eq!(cursor_line(&app), 2);
    quit(app).await?;
    Ok(())
}

fn top_of_view(app: &Application) -> usize {
    let (view, doc) = current_ref!(app.editor);
    doc.view_offset(view.id).anchor
}

#[tokio::test(flavor = "multi_thread")]
async fn background_files_restore_cursor_and_scroll_when_shown() -> anyhow::Result<()> {
    let Some(root) = isolation::workspace(
        "test::persistence::background_files_restore_cursor_and_scroll_when_shown",
    )?
    else {
        return Ok(());
    };
    let a = root.join("workspace/a.txt");
    let b = root.join("workspace/b.txt");
    let lines: String = (1..=400).map(|line| format!("{line}\n")).collect();
    std::fs::write(&a, &lines)?;
    std::fs::write(&b, &lines)?;

    // Leave `b.txt` on line 300, scrolled so that line is at the top. Scrolling only
    // as far as needed to show the cursor would put it at the bottom instead.
    let mut app = session(&b, None, true).await?;
    keys(&mut app, ":goto 300<ret>zt").await?;
    let (line, top) = (cursor_line(&app), top_of_view(&app));
    assert_eq!(line, 299);
    assert!(top > 0, "the view should be scrolled");
    quit(app).await?;

    // `ms a.txt b.txt`, then switch to `b.txt`: both the cursor and the scroll come back.
    let mut app = session_with(&[(&a, None), (&b, None)]).await?;
    show(&mut app, &b);
    assert_eq!(cursor_line(&app), line);
    assert_eq!(top_of_view(&app), top);
    quit(app).await?;
    Ok(())
}

//! File explorer actions through real keys: create, move, copy, delete and yank paths.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use term::application::Application;
use view::{current_ref, editor::Action};

use super::helpers::{keys, test_config, AppBuilder};

/// An app editing `a.txt` in a fresh directory with `b.txt` and `sub/c.txt` beside it,
/// with the explorer open on that directory.
async fn explorer() -> anyhow::Result<(Application, tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = stdx::path::canonicalize(dir.path());
    std::fs::write(root.join("a.txt"), "a\n")?;
    std::fs::write(root.join("b.txt"), "b\n")?;
    std::fs::create_dir(root.join("sub"))?;
    std::fs::write(root.join("sub/c.txt"), "c\n")?;
    let mut app = AppBuilder::new().with_config(test_config()).build()?;
    app.editor.open(&root.join("a.txt"), Action::Replace)?;
    keys(&mut app, "<space>E").await?;
    Ok((app, dir, root))
}

fn screen(app: &Application) -> String {
    let buffer = app.terminal_backend().buffer();
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Settle the event loop until the rendered screen satisfies `done`.
async fn wait_for(app: &mut Application, done: impl Fn(&str) -> bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done(&screen(app)) {
            keys(app, "").await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out; screen:\n{}", screen(app)))?
}

/// Filter the explorer to one entry, so the following action applies to it.
async fn select(app: &mut Application, name: &str) -> anyhow::Result<()> {
    // `C-u` pages the picker; `C-w` clears a query without path separators.
    keys(app, &format!("<C-w><C-w>{name}")).await
}

fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_n_creates_files_and_directories_in_the_shown_directory() -> anyhow::Result<()> {
    let (mut app, _dir, root) = explorer().await?;
    keys(&mut app, "<A-n>new.txt<ret>").await?;
    assert!(root.join("new.txt").is_file());
    // A trailing separator creates a directory, with missing parents.
    keys(&mut app, "<A-n>deep/er/<ret>").await?;
    assert!(root.join("deep/er").is_dir());
    // The explorer lists the new entries without a file watcher event.
    wait_for(&mut app, |screen| {
        screen.contains("new.txt") && screen.contains("deep/")
    })
    .await?;
    // An existing path is never replaced.
    keys(&mut app, "<A-n>b.txt<ret>").await?;
    assert_eq!(std::fs::read_to_string(root.join("b.txt"))?, "b\n");
    assert!(screen(&app).contains("already exists"), "{}", screen(&app));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_m_renames_before_the_extension_and_retargets_open_buffers() -> anyhow::Result<()> {
    let (mut app, _dir, root) = explorer().await?;
    select(&mut app, "a.txt").await?;
    // The prompt holds the path with the cursor before `.txt`.
    keys(&mut app, "<A-m>-renamed<ret>").await?;
    assert_eq!(names(&root), ["a-renamed.txt", "b.txt", "sub"]);
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.path(), Some(root.join("a-renamed.txt").as_path()));
    // Moving onto a directory moves into it.
    select(&mut app, "b.txt").await?;
    keys(&mut app, "<A-m><C-e><C-u>").await?;
    keys(&mut app, &format!("{}<ret>", root.join("sub").display())).await?;
    assert!(
        root.join("sub/b.txt").is_file(),
        "{:?}\n{}",
        names(&root),
        screen(&app)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_c_copies_files_but_not_directories() -> anyhow::Result<()> {
    let (mut app, _dir, root) = explorer().await?;
    select(&mut app, "b.txt").await?;
    keys(&mut app, "<A-c>-copy<ret>").await?;
    assert_eq!(std::fs::read_to_string(root.join("b-copy.txt"))?, "b\n");
    assert!(root.join("b.txt").is_file());
    wait_for(&mut app, |screen| screen.contains("b-copy.txt")).await?;

    // Copying onto an existing file is refused and leaves it alone.
    select(&mut app, "b.txt").await?;
    keys(&mut app, "<A-c><C-e><C-u>").await?;
    keys(&mut app, &format!("{}<ret>", root.join("a.txt").display())).await?;
    assert_eq!(std::fs::read_to_string(root.join("a.txt"))?, "a\n");
    assert!(screen(&app).contains("already exists"), "{}", screen(&app));

    select(&mut app, "sub").await?;
    keys(&mut app, "<A-c>").await?;
    assert!(
        screen(&app).contains("Copying directories is not supported"),
        "{}",
        screen(&app)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn alt_d_deletes_only_after_yes_and_removes_directories_with_their_contents(
) -> anyhow::Result<()> {
    let (mut app, _dir, root) = explorer().await?;
    select(&mut app, "b.txt").await?;
    keys(&mut app, "<A-d>n<ret>").await?;
    assert!(root.join("b.txt").exists());
    keys(&mut app, "<A-d><esc>").await?;
    assert!(root.join("b.txt").exists());
    keys(&mut app, "<A-d>y<ret>").await?;
    assert!(!root.join("b.txt").exists());

    select(&mut app, "sub").await?;
    keys(&mut app, "<A-d>").await?;
    assert!(
        screen(&app).contains("and its contents?"),
        "{}",
        screen(&app)
    );
    keys(&mut app, "y<ret>").await?;
    assert_eq!(names(&root), ["a.txt"]);
    wait_for(&mut app, |screen| !screen.contains("sub/")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_entry_cannot_be_moved_or_deleted_and_alt_y_yanks_paths() -> anyhow::Result<()> {
    let (mut app, _dir, root) = explorer().await?;
    select(&mut app, "..").await?;
    keys(&mut app, "<A-d>").await?;
    assert!(screen(&app).contains("Select a file or directory first"));
    assert_eq!(names(&root), ["a.txt", "b.txt", "sub"]);

    select(&mut app, "b.txt").await?;
    keys(&mut app, "<A-y>").await?;
    let yanked: Vec<_> = app
        .editor
        .registers
        .read('"')
        .expect("default register")
        .map(|value| value.into_owned())
        .collect();
    let expected = stdx::path::get_relative_path(root.join("b.txt"));
    assert_eq!(yanked, [expected.to_string_lossy().into_owned()]);
    Ok(())
}

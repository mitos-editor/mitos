use super::*;
use crate::{
    application::TestApplication,
    compositor::{Context, Event},
    ui::picker::test::{all_items, application, draw, frame, until},
};
use std::process::Command;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "History Test")
        .env("GIT_AUTHOR_EMAIL", "history@example.com")
        .env("GIT_COMMITTER_NAME", "History Test")
        .env("GIT_COMMITTER_EMAIL", "history@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn repository(count: usize) -> (tempfile::TempDir, GitRepository) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]);
    let tree = git(dir.path(), &["mktree"]);
    let mut head = git(dir.path(), &["commit-tree", &tree, "-m", "oldest-needle"]);
    for index in 1..count {
        head = git(
            dir.path(),
            &[
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-m",
                &format!("commit-{index:03}"),
            ],
        );
    }
    git(dir.path(), &["update-ref", "HEAD", &head]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    (dir, repo)
}

fn press(picker: &mut Picker<CommitEntry, Data>, app: &mut TestApplication, key: &str) -> String {
    let config = crate::config::Config::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = crate::job::Jobs::new();
    picker.handle_event(
        &Event::Key(key.parse().unwrap()),
        &mut Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        },
    );
    screen(picker, app)
}

fn screen(picker: &mut Picker<CommitEntry, Data>, app: &mut TestApplication) -> String {
    frame(picker, app)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn history_scroll_loads_older_commits_and_preserves_selection() -> Result<()> {
    let (_dir, repo) = repository(205);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    assert!(screen(&mut picker, &mut app).contains("More history available"));
    assert!(
        screen(&mut picker, &mut app).contains("100/?"),
        "partial history must not claim a total"
    );
    assert_eq!(picker.selection().unwrap().subject, "commit-204");
    // Remaining at the top does not scan the entire repository.
    for _ in 0..5 {
        draw(&mut picker, &mut app);
    }
    assert_eq!(all_items(&picker).count(), 100);
    for _ in 0..85 {
        press(&mut picker, &mut app, "down");
    }
    let selected = picker.selection().unwrap().id.clone();
    until(&mut picker, &mut app, |p| all_items(p).count() == 200).await?;
    assert_eq!(picker.selection().unwrap().id, selected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_search_finds_a_commit_beyond_the_first_page() -> Result<()> {
    let (_dir, repo) = repository(205);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    for ch in "oldest-needle".chars() {
        press(&mut picker, &mut app, &ch.to_string());
    }
    // Drive only requested redraws: no polling timer or further keypress may
    // be needed to continue searching through the remaining pages.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            app.editor.redraw().start_frame();
            draw(&mut picker, &mut app);
            if all_items(&picker).count() == 205
                && picker
                    .selection()
                    .is_some_and(|c| c.subject == "oldest-needle")
            {
                break;
            }
            app.editor.redraw().requested().await;
        }
    })
    .await?;
    assert!(screen(&mut picker, &mut app).contains("Complete history"));
    assert!(
        screen(&mut picker, &mut app).contains("1/205"),
        "only exhausted history may show an exact total"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_history_search_stops_loading_more_pages() -> Result<()> {
    let (_dir, repo) = repository(405);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    // Inspect every rendered frame, including the keypress frame: the next
    // render may already consume the completed page and hide its loading status.
    let mut rendered = press(&mut picker, &mut app, "z");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !rendered.contains("Loading history") {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            rendered = screen(&mut picker, &mut app);
        }
    })
    .await?;
    // The already requested page can finish, but clearing must prevent a third.
    press(&mut picker, &mut app, "backspace");
    until(&mut picker, &mut app, |p| all_items(p).count() == 200).await?;
    for _ in 0..10 {
        draw(&mut picker, &mut app);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(all_items(&picker).count(), 200);
    assert!(screen(&mut picker, &mut app).contains("More history available"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_limit_remains_visibly_incomplete() -> Result<()> {
    let (_dir, repo) = repository(1);
    let cursor = repo.history_cursor(None)?;
    let commit = repo
        .history_page(cursor.clone().unwrap())
        .await?
        .commits
        .remove(0);
    let mut pager = Pager {
        repo,
        cursor: cursor.clone(),
        pending: None,
        rows: vec![commit.clone(); HISTORY_DISPLAY_LIMIT - 1],
        failed: false,
        capped: false,
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    tx.send(Ok(HistoryPage {
        commits: vec![commit],
        next: cursor,
    }))
    .ok()
    .unwrap();
    pager.pending = Some(rx);
    let mut app = application(false)?;
    let mut task = event::TaskController::new();
    let mut status = None;
    let mut incomplete = false;
    let rows = pager
        .update(crate::ui::picker::UpdateContext {
            editor: &mut app.editor,
            query: "needle",
            near_end: true,
            status: &mut status,
            incomplete: &mut incomplete,
            selected: None,
            task: &mut task,
            resumed: false,
        })
        .unwrap();
    assert_eq!(rows.len(), HISTORY_DISPLAY_LIMIT);
    assert_eq!(
        status.as_deref(),
        Some("5000 loaded; history limit reached")
    );
    assert!(
        incomplete,
        "a capped source must never report an exact total"
    );
    assert!(pager.cursor.is_none());
    assert!(pager.pending.is_none());
    Ok(())
}

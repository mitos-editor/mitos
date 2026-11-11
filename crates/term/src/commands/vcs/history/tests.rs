use super::super::test_support::{git, screen};
use super::*;
use crate::{
    application::TestApplication,
    compositor::Context,
    ui::picker::test::{all_items, application, draw, until, until_with_timeout},
};
use std::process::Command;

fn repository(count: usize) -> (tempfile::TempDir, GitRepository) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]);
    // A single import keeps large-history coverage cheap on every platform.
    use std::io::Write;
    use std::process::Stdio;
    let mut input = String::new();
    for index in 0..count {
        let subject = if index == 0 {
            "oldest-needle".into()
        } else {
            format!("commit-{index:03}")
        };
        input.push_str(&format!(
            "commit refs/heads/history-test\nmark :{}\ncommitter History Test <history@example.com> {} +0000\ndata {}\n{}\n",
            index + 1, 1_700_000_000 + index, subject.len(), subject,
        ));
        if index > 0 {
            input.push_str(&format!("from :{index}\n"));
        }
        input.push('\n');
    }
    input.push_str("done\n");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["fast-import", "--quiet", "--done"])
        .env_remove("GIT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(
        dir.path(),
        &["symbolic-ref", "HEAD", "refs/heads/history-test"],
    );
    let repo = GitRepository::discover(dir.path()).unwrap();
    (dir, repo)
}

fn press(
    picker: &mut Picker<Arc<CommitEntry>, Data>,
    app: &mut TestApplication,
    key: &str,
) -> String {
    super::super::test_support::press(picker, app, key);
    screen(picker, app)
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
async fn history_search_reaches_past_five_thousand_commits() -> Result<()> {
    let (_dir, repo) = repository(5_005);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    for ch in "oldest-needle".chars() {
        press(&mut picker, &mut app, &ch.to_string());
    }
    // Searching 51 pages needs more time on Windows under parallel CI load.
    until_with_timeout(
        &mut picker,
        &mut app,
        std::time::Duration::from_secs(30),
        |p| {
            all_items(p).count() == 5_005
                && p.selection()
                    .is_some_and(|row| row.subject == "oldest-needle")
        },
    )
    .await?;
    assert!(screen(&mut picker, &mut app).contains("Complete history"));
    assert!(screen(&mut picker, &mut app).contains("1/5005"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_loading_can_be_paused_and_continued_with_the_shared_action() -> Result<()> {
    let (_dir, repo) = repository(305);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    press(&mut picker, &mut app, "C-x l");
    assert!(screen(&mut picker, &mut app).contains("Loading paused"));
    for ch in "oldest-needle".chars() {
        press(&mut picker, &mut app, &ch.to_string());
    }
    for _ in 0..10 {
        draw(&mut picker, &mut app);
    }
    assert_eq!(all_items(&picker).count(), 100);
    press(&mut picker, &mut app, "C-x l");
    until(&mut picker, &mut app, |p| all_items(p).count() == 305).await?;
    assert!(screen(&mut picker, &mut app).contains("Complete history"));
    Ok(())
}

/// End-to-end measurements include traversal, matching, rendering, and explicit
/// continuation at each soft budget. Fixture creation is outside the timer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual history scaling measurement"]
async fn history_scaling() -> Result<()> {
    for count in [5_000, 50_000, 100_000] {
        let (_dir, repo) = repository(count);
        let mut app = application(false)?;
        let mut picker = browser(&app.editor, repo, None, None)?;
        let start = std::time::Instant::now();
        for ch in "oldest-needle".chars() {
            press(&mut picker, &mut app, &ch.to_string());
        }
        let mut pauses = 0;
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
            loop {
                app.editor.redraw().start_frame();
                draw(&mut picker, &mut app);
                if crate::ui::picker::test::loaded_count(&picker) == count {
                    break;
                }
                if crate::ui::picker::test::source_status(&picker)
                    .is_some_and(|status| status.starts_with("Memory budget reached"))
                {
                    pauses += 1;
                    press(&mut picker, &mut app, "C-x l");
                }
                app.editor.redraw().requested().await;
            }
        })
        .await?;
        let bytes: usize = all_items(&picker).map(history_row_bytes).sum();
        eprintln!(
            "{count} history rows: {:?}, estimated {} MiB, {pauses} budget pauses",
            start.elapsed(),
            bytes / (1024 * 1024)
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_scrolling_reaches_past_five_thousand_without_losing_selection() -> Result<()> {
    let (_dir, repo) = repository(5_005);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    for expected in (200..=5_000).step_by(100).chain([5_005]) {
        press(&mut picker, &mut app, "C-x G");
        let selected = picker.selection().unwrap().id.clone();
        until(&mut picker, &mut app, |p| all_items(p).count() == expected).await?;
        assert_eq!(picker.selection().unwrap().id, selected);
    }
    assert!(screen(&mut picker, &mut app).contains("Complete history"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_history_renders_commit_preview_on_the_right() -> Result<()> {
    let (_dir, repo) = repository(1);
    let mut app = application(false)?;
    let mut picker = browser(&app.editor, repo, None, None)?;
    until(&mut picker, &mut app, |p| all_items(p).count() == 1).await?;
    let config = crate::config::Config::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = crate::job::Jobs::new();
    let mut cx = Context {
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
    };
    let area = view::graphics::Rect::new(0, 0, 140, 30);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let mut surface = tui::buffer::Buffer::empty(area);
            picker.update_and_render(area, &mut surface, &mut cx);
            let right: String = (0..area.height)
                .flat_map(|y| (area.width / 2..area.width).map(move |x| (x, y)))
                .map(|position| surface[position].symbol())
                .collect();
            if right.contains("No changed files") {
                assert!(right.contains("oldest-needle"));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

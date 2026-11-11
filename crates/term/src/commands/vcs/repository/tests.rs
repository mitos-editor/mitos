use super::super::test_support::{git, key, screen};
use super::*;
use crate::{
    application::TestApplication,
    compositor,
    job::Jobs,
    ui::picker::test::{all_items, application, draw, frame, until},
};
use vcs::operations::repository::BRANCH_PAGE_SIZE;
use view::graphics::Color;

async fn finish(
    app: &mut TestApplication,
    compositor: &mut compositor::Compositor,
    jobs: &mut Jobs,
) -> Result<()> {
    use futures_util::StreamExt;
    while let Some(callback) = jobs.wait_futures.next().await {
        jobs.try_handle_callback(
            crate::config::Context {
                current: &crate::config::Config::default(),
                updates: &tokio::sync::mpsc::unbounded_channel().0,
            },
            &mut app.editor,
            &mut app.interaction,
            compositor,
            callback,
        )?;
    }
    Ok(())
}

#[test]
fn create_actions_work_without_a_selected_row() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(dir.path())
        .status()?;
    let repo = GitRepository::discover(dir.path())?;
    assert!(matches!(
        branch_action(&repo, dir.path(), None, PickerItemAction::CreateBranch),
        PickerCallbackResult::KeepOpenWithCallback(_)
    ));
    assert!(matches!(
        worktree_action(&repo, dir.path(), None, PickerItemAction::CreateWorktree),
        PickerCallbackResult::KeepOpenWithCallback(_)
    ));
    assert!(matches!(
        branch_action(&repo, dir.path(), None, PickerItemAction::DeleteBranch),
        PickerCallbackResult::KeepOpen
    ));
    assert!(matches!(
        worktree_action(&repo, dir.path(), None, PickerItemAction::RemoveWorktree),
        PickerCallbackResult::KeepOpen
    ));
    for action in [
        PickerItemAction::LockWorktree,
        PickerItemAction::UnlockWorktree,
    ] {
        assert!(matches!(
            worktree_action(&repo, dir.path(), None, action),
            PickerCallbackResult::KeepOpen
        ));
        let main = Worktree {
            path: dir.path().into(),
            branch: None,
            main: true,
            locked: false,
            prunable: false,
            divergence: None,
        };
        assert!(matches!(
            worktree_action(&repo, dir.path(), Some(&main), action),
            PickerCallbackResult::KeepOpen
        ));
        let linked = Worktree {
            main: false,
            ..main
        };
        assert!(matches!(
            worktree_action(&repo, dir.path(), Some(&linked), action),
            PickerCallbackResult::KeepOpenWithCallback(_)
        ));
    }
    Ok(())
}

#[test]
fn footer_hints_follow_remapped_and_unbound_actions() {
    let config = crate::config::Config::load(
        Ok(
            &"[keys.branch-picker]\nA-n = 'unbound'\nC-t = 'create_branch'\nA-d = 'unbound'"
                .to_owned(),
        ),
        Ok(String::new()),
    )
    .unwrap();
    let hints = super::super::actions::action_hints(
        &config.component_keys.branch_picker,
        &[
            (PickerAction::Item(PickerItemAction::CreateBranch), "create"),
            (PickerAction::Item(PickerItemAction::DeleteBranch), "delete"),
        ],
    );
    assert_eq!(hints, ["<C-t> create"]);
}

#[test]
fn repository_picker_keymaps_omit_loading_controls_and_preserve_other_shared_bindings() {
    use crate::keymap::component::action_hint;
    let config = crate::config::Config::default();
    for keys in [
        &config.component_keys.branch_picker,
        &config.component_keys.worktree_picker,
    ] {
        assert_eq!(action_hint(keys, &PickerAction::ToggleLoading), "unbound");
    }
    let source = "[keys.picker]\nC-l = 'move_end'\nF4 = 'toggle_loading'".to_owned();
    let config = crate::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
    for keys in [
        &config.component_keys.branch_picker,
        &config.component_keys.worktree_picker,
    ] {
        assert_eq!(action_hint(keys, &PickerAction::ToggleLoading), "unbound");
        assert!(matches!(keys.0.get(&"C-l".parse().unwrap()),
            Some(ui_core::keymap::Binding::Actions(actions)) if actions == &[PickerAction::MoveEnd]));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pickers_render_divergence_against_main() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo");
    std::fs::create_dir(&root)?;
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["commit", "--allow-empty", "-m", "initial"]);
    git(&root, &["switch", "-c", "topic"]);
    git(&root, &["commit", "--allow-empty", "-m", "topic"]);
    git(&root, &["switch", "main"]);
    git(&root, &["commit", "--allow-empty", "-m", "main"]);
    let linked = parent.path().join("linked");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "topic",
        ],
    );
    let repo = GitRepository::discover(&root)?;
    let mut app = application(true)?;
    let mut picker = branches(&app.editor, repo.clone(), root.clone());
    until(&mut picker, &mut app, |picker| {
        all_items(picker).count() == 2
    })
    .await?;
    let rendered = screen(&mut picker, &mut app);
    assert!(rendered.contains("vs base"), "{rendered}");
    assert!(rendered.contains("↑0 ↓0 main"), "{rendered}");
    assert!(rendered.contains("↑1 ↓1 main"), "{rendered}");
    git(&root, &["commit", "--allow-empty", "-m", "advance main"]);
    until(&mut picker, &mut app, |picker| {
        all_items(picker).any(|branch| {
            branch.name == "topic"
                && branch
                    .divergence
                    .as_ref()
                    .is_some_and(|counts| counts.behind == 2)
        })
    })
    .await?;
    assert!(screen(&mut picker, &mut app).contains("↑1 ↓2 main"));
    let mut picker = worktrees(&app.editor, repo, root);
    until(&mut picker, &mut app, |picker| {
        all_items(picker).count() == 2
    })
    .await?;
    let rendered = screen(&mut picker, &mut app);
    assert!(rendered.contains("vs base"), "{rendered}");
    assert!(rendered.contains("↑0 ↓0 main"), "{rendered}");
    assert!(rendered.contains("↑1 ↓2 main"), "{rendered}");
    assert!(rendered.contains("(detached)"), "{rendered}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn branch_picker_creates_confirms_deletion_and_refreshes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["commit", "--allow-empty", "-m", "initial"]);
    let repo = GitRepository::discover(dir.path())?;
    let mut app = application(false)?;
    let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
    let mut jobs = Jobs::new();
    let mut picker = branches(&app.editor, repo.clone(), dir.path().into());
    until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    compositor.push(Box::new(overlaid(picker)));
    key(&mut app, &mut compositor, &mut jobs, "A-n");
    assert!(compositor.find::<ui::Prompt>().is_some());
    for letter in ["t", "o", "p", "i", "c", "ret"] {
        key(&mut app, &mut compositor, &mut jobs, letter);
    }
    finish(&mut app, &mut compositor, &mut jobs).await?;
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    let picker = &mut compositor
        .find::<ui::overlay::Overlay<Picker<Arc<Branch>, RepositoryData>>>()
        .unwrap()
        .content;
    until(picker, &mut app, |picker| {
        crate::ui::picker::test::items(picker).count() == 2
    })
    .await?;
    picker.to_end();
    assert_eq!(picker.selection().unwrap().name, "topic");
    key(&mut app, &mut compositor, &mut jobs, "A-d");
    key(&mut app, &mut compositor, &mut jobs, "ret"); // Cancel by default.
    assert!(!app.editor.handlers().git_operations.busy());
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    key(&mut app, &mut compositor, &mut jobs, "A-d");
    key(&mut app, &mut compositor, &mut jobs, "down");
    key(&mut app, &mut compositor, &mut jobs, "ret");
    finish(&mut app, &mut compositor, &mut jobs).await?;
    assert!(!repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    let picker = &mut compositor
        .find::<ui::overlay::Overlay<Picker<Arc<Branch>, RepositoryData>>>()
        .unwrap()
        .content;
    until(picker, &mut app, |picker| {
        picker
            .selection()
            .is_some_and(|branch| branch.name == "main")
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_picker_creates_on_a_branch_and_confirms_removal() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("repo");
    std::fs::create_dir(&root)?;
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["commit", "--allow-empty", "-m", "initial"]);
    git(&root, &["branch", "topic"]);
    let repo = GitRepository::discover(&root)?;
    let mut app = application(false)?;
    let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
    let mut jobs = Jobs::new();
    let mut picker = worktrees(&app.editor, repo.clone(), root);
    until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    compositor.push(Box::new(overlaid(picker)));
    key(&mut app, &mut compositor, &mut jobs, "A-n");
    for ch in "../linked".chars() {
        key(&mut app, &mut compositor, &mut jobs, &ch.to_string());
    }
    key(&mut app, &mut compositor, &mut jobs, "ret");
    assert!(compositor.find::<ui::Prompt>().is_some());
    assert!(!app.editor.handlers().git_operations.busy());
    for ch in "topic".chars() {
        key(&mut app, &mut compositor, &mut jobs, &ch.to_string());
    }
    key(&mut app, &mut compositor, &mut jobs, "ret");
    finish(&mut app, &mut compositor, &mut jobs).await?;
    let linked = stdx::path::canonicalize_existing(&parent.path().join("linked"));
    assert!(repo
        .worktrees()
        .await?
        .iter()
        .any(|tree| tree.path == linked && tree.branch.as_deref() == Some("topic")));
    let picker = &mut compositor
        .find::<ui::overlay::Overlay<Picker<Worktree, RepositoryData>>>()
        .unwrap()
        .content;
    until(picker, &mut app, |picker| {
        crate::ui::picker::test::items(picker).count() == 2
    })
    .await?;
    picker.to_end();
    assert_eq!(picker.selection().unwrap().path, linked);
    key(&mut app, &mut compositor, &mut jobs, "A-d");
    key(&mut app, &mut compositor, &mut jobs, "ret");
    assert!(linked.exists());
    assert!(!app.editor.handlers().git_operations.busy());
    key(&mut app, &mut compositor, &mut jobs, "A-d");
    key(&mut app, &mut compositor, &mut jobs, "down");
    key(&mut app, &mut compositor, &mut jobs, "ret");
    finish(&mut app, &mut compositor, &mut jobs).await?;
    assert!(!linked.exists());
    assert!(repo
        .branches()
        .await?
        .iter()
        .any(|branch| branch.name == "topic"));
    let picker = &mut compositor
        .find::<ui::overlay::Overlay<Picker<Worktree, RepositoryData>>>()
        .unwrap()
        .content;
    until(picker, &mut app, |picker| {
        crate::ui::picker::test::items(picker).count() == 1
    })
    .await?;
    Ok(())
}

fn repository() -> Result<(tempfile::TempDir, GitRepository)> {
    let dir = tempfile::tempdir()?;
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(dir.path())
        .output()?;
    assert!(output.status.success());
    for index in 0..205 {
        std::fs::write(
            dir.path().join(format!(".git/refs/heads/topic-{index:03}")),
            "1111111111111111111111111111111111111111\n",
        )?;
    }
    let git = GitRepository::discover(dir.path())?;
    Ok((dir, git))
}

fn press(picker: &mut Picker<Arc<Branch>, RepositoryData>, app: &mut TestApplication, key: &str) {
    super::super::test_support::press(picker, app, key);
    draw(picker, app);
}

#[tokio::test(flavor = "multi_thread")]
async fn branches_load_on_scroll_and_search_beyond_the_first_page() -> Result<()> {
    let (_dir, repo) = repository()?;
    let mut app = application(false)?;
    let mut picker = branches(&app.editor, repo.clone(), repo.root().to_owned());
    until(&mut picker, &mut app, |p| {
        all_items(p).count() == BRANCH_PAGE_SIZE
    })
    .await?;
    let rendered = screen(&mut picker, &mut app);
    assert!(rendered.contains("More branches available"));
    assert!(!rendered.contains("<C-l>"));
    assert!(!rendered.contains("pause"));
    press(&mut picker, &mut app, "C-l");
    assert!(screen(&mut picker, &mut app).contains("100/?"));
    for _ in 0..5 {
        draw(&mut picker, &mut app);
    }
    assert_eq!(all_items(&picker).count(), 100);
    for _ in 0..85 {
        press(&mut picker, &mut app, "down");
    }
    let selected = picker.selection().unwrap().name.clone();
    until(&mut picker, &mut app, |p| all_items(p).count() == 200).await?;
    assert_eq!(picker.selection().unwrap().name, selected);
    for ch in "topic-204".chars() {
        press(&mut picker, &mut app, &ch.to_string());
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            app.editor.redraw().start_frame();
            draw(&mut picker, &mut app);
            if all_items(&picker).count() == 205
                && picker.selection().is_some_and(|b| b.name == "topic-204")
            {
                break;
            }
            app.editor.redraw().requested().await;
        }
    })
    .await?;
    assert!(screen(&mut picker, &mut app).contains("Complete branch list"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn branch_refresh_replaces_old_pages_after_repository_changes() -> Result<()> {
    let (dir, repo) = repository()?;
    let mut app = application(true)?;
    let mut picker = branches(&app.editor, repo.clone(), repo.root().to_owned());
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    for index in 0..205 {
        std::fs::remove_file(dir.path().join(format!(".git/refs/heads/topic-{index:03}")))?;
    }
    std::fs::write(
        dir.path().join(".git/refs/heads/replacement"),
        "1111111111111111111111111111111111111111\n",
    )?;
    // Finish the old listing through search, then let polling replace its pages.
    press(&mut picker, &mut app, "r");
    until(&mut picker, &mut app, |p| {
        let rows: Vec<_> = all_items(p).collect();
        rows.len() == 1 && rows[0].name == "replacement"
    })
    .await?;
    // Polling may already have started another refresh. Assert the published
    // selection rather than a transient loading status from another frame.
    assert_eq!(picker.selection().unwrap().name, "replacement");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn branch_polling_preserves_the_loaded_prefix_and_selection() -> Result<()> {
    let (dir, repo) = repository()?;
    let mut app = application(true)?;
    let mut picker = branches(&app.editor, repo.clone(), repo.root().to_owned());
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    for _ in 0..120 {
        press(&mut picker, &mut app, "down");
    }
    until(&mut picker, &mut app, |p| all_items(p).count() == 200).await?;
    let selected = picker.selection().unwrap().name.clone();
    // Change metadata in the first page. Seeing it proves polling finished a
    // refresh, without assuming a loading status survives another redraw.
    let config = dir.path().join(".git/config");
    let mut contents = std::fs::read_to_string(&config)?;
    contents.push_str("\n[branch \"topic-000\"]\nremote = .\nmerge = refs/heads/refreshed\n");
    std::fs::write(config, contents)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            draw(&mut picker, &mut app);
            assert_eq!(all_items(&picker).count(), 200);
            assert_eq!(picker.selection().unwrap().name, selected);
            if all_items(&picker)
                .any(|branch| branch.name == "topic-000" && branch.upstream == "refreshed")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

fn assert_colour(surface: &tui::buffer::Buffer, text: &str, colour: Color) {
    // Skip the border, query, separator, and column headings.
    for y in 4..surface.area.height {
        let row: String = (0..surface.area.width)
            .map(|x| surface[(x, y)].symbol())
            .collect();
        if let Some(offset) = row.find(text) {
            let offset = row[..offset].chars().count();
            for x in offset..offset + text.len() {
                assert_eq!(
                    surface[(x as u16, y)].fg,
                    colour,
                    "{text}: row {y} column {x}: {row}"
                );
            }
            return;
        }
    }
    panic!("Missing picker text: {text}");
}

#[tokio::test]
async fn repository_pickers_preserve_semantic_colours_on_selected_rows() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let output = std::process::Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(dir.path())
        .output()?;
    ensure!(output.status.success(), "git init failed");
    std::fs::write(
        dir.path().join(".git/refs/heads/main"),
        "1111111111111111111111111111111111111111\n",
    )?;
    let repo = GitRepository::discover(dir.path())?;
    let mut app = application(false)?;
    app.editor.theme = theme::Theme::from(toml::Value::Table(toml::toml! {
        "ui.text" = "white"
        "ui.text.directory" = "blue"
        "ui.text.focus" = { modifiers = ["bold"] }
        "diff.plus" = "green"
        "string.special.symbol" = "cyan"
        "ui.selection" = { bg = "black" }
        "ui.highlight" = { bg = "black" }
        "special" = { modifiers = ["bold"] }
    }));
    let mut branches = branches(&app.editor, repo.clone(), dir.path().into());
    until(&mut branches, &mut app, |p| p.selection().is_some()).await?;
    let surface = frame(&mut branches, &mut app);
    assert_colour(&surface, "main", Color::Green);
    assert_colour(&surface, "*", Color::Green);
    assert!(screen(&mut branches, &mut app).contains("—"));
    let mut worktrees = worktrees(&app.editor, repo, dir.path().into());
    until(&mut worktrees, &mut app, |p| p.selection().is_some()).await?;
    let surface = frame(&mut worktrees, &mut app);
    assert_colour(&surface, std::path::MAIN_SEPARATOR_STR, Color::Blue);
    assert!(screen(&mut worktrees, &mut app).contains("—"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resumed_branch_picker_replaces_the_live_generation_atomically() -> Result<()> {
    let (dir, repo) = repository()?;
    let mut app = application(true)?;
    let mut picker = branches(&app.editor, repo.clone(), repo.root().to_owned());
    until(&mut picker, &mut app, |p| all_items(p).count() == 100).await?;
    press(&mut picker, &mut app, "down");
    let selected = picker.selection().unwrap().name.clone();
    picker.suspend();
    for index in 0..205 {
        let name = format!("topic-{index:03}");
        if name != selected {
            std::fs::remove_file(dir.path().join(format!(".git/refs/heads/{name}")))?;
        }
    }
    std::fs::write(
        dir.path().join(".git/refs/heads/replacement"),
        "1111111111111111111111111111111111111111\n",
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            draw(&mut picker, &mut app);
            let count = all_items(&picker).count();
            assert!(count == 100 || count == 2);
            assert_eq!(picker.selection().unwrap().name, selected);
            if count == 2 {
                assert!(all_items(&picker).any(|branch| branch.name == "replacement"));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

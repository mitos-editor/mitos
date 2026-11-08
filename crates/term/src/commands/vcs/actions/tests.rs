use super::*;
use crate::{
    application::TestApplication,
    ui::picker::test::{application, until},
};

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn repo() -> anyhow::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    git(dir.path(), &["config", "user.name", "test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.path().join("file.txt"), "head\n")?;
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    Ok(dir)
}

fn key(
    app: &mut TestApplication,
    compositor: &mut compositor::Compositor,
    jobs: &mut crate::job::Jobs,
    key: &str,
) {
    let config = crate::config::Config::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut app.editor,
        interaction: &mut app.interaction,
        jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    assert!(compositor.handle_event(&Event::Key(key.parse().unwrap()), &mut cx));
}

#[tokio::test(flavor = "multi_thread")]
async fn stash_application_confirms_and_finishes_after_the_picker_closes() -> anyhow::Result<()> {
    let dir = repo()?;
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(dir.path().join("file.txt"), "stashed\n")?;
    repo.prepare_stash(true)?.execute().await?;
    let entries = repo.stashes().await?;
    let mut app = application(false)?;
    let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
    let mut jobs = crate::job::Jobs::new();
    let mut picker = stash_picker(
        &app.editor,
        repo.clone(),
        dir.path().into(),
        entries.clone(),
    );
    until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    compositor.push(Box::new(overlaid(picker)));
    key(&mut app, &mut compositor, &mut jobs, "A-s");
    assert!(compositor.find::<ActionMenu>().is_some());
    key(&mut app, &mut compositor, &mut jobs, "ret"); // Cancel is selected initially.
    assert!(!app.editor.handlers().git_operations.busy());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "head\n"
    );
    key(&mut app, &mut compositor, &mut jobs, "A-s");
    key(&mut app, &mut compositor, &mut jobs, "down");
    key(&mut app, &mut compositor, &mut jobs, "ret");
    assert!(app.editor.handlers().git_operations.busy());
    compositor.pop(); // A started operation belongs to the editor, not this picker.
    while let Some(callback) = futures_util::StreamExt::next(&mut jobs.wait_futures).await {
        jobs.try_handle_callback(
            crate::config::Context {
                current: &crate::config::Config::default(),
                updates: &tokio::sync::mpsc::unbounded_channel().0,
            },
            &mut app.editor,
            &mut app.interaction,
            &mut compositor,
            callback,
        )?;
    }
    assert!(!app.editor.handlers().git_operations.busy());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "stashed\n"
    );
    assert_eq!(repo.stashes().await?, entries);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_actions_are_available_without_a_selected_file() -> anyhow::Result<()> {
    let dir = repo()?;
    let mut app = application(false)?;
    let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
    let mut jobs = crate::job::Jobs::new();
    let scope = vcs::ChangedFileScope::Repository(dir.path().into());
    let workspace = dir.path().to_owned();
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [PickerColumn::new(
            "file",
            |file: &vcs::FileStatus, _: &()| file.path().display().to_string().into(),
        )],
        0,
        [],
        (),
        |_, _, _| {},
    )
    .with_item_actions(
        |maps| &maps.git_picker,
        move |cx, status, action| file_action(cx, status, None, action, &scope, &workspace),
    );
    until(&mut picker, &mut app, |_| true).await?;
    compositor.push(Box::new(overlaid(picker)));
    key(&mut app, &mut compositor, &mut jobs, "A-a");
    assert!(compositor.find::<ActionMenu>().is_some());
    key(&mut app, &mut compositor, &mut jobs, "esc");
    assert!(compositor.find::<ActionMenu>().is_none());
    assert!(!app.editor.handlers().git_operations.busy());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_actions_refresh_the_stash_picker_without_watchers_and_preserve_selection(
) -> anyhow::Result<()> {
    let dir = repo()?;
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(dir.path().join("file.txt"), "first stash\n")?;
    repo.prepare_stash(true)?.execute().await?;
    let entries = repo.stashes().await?;
    let selected = entries[0].id().to_owned();
    let mut app = application(false)?;
    let mut picker = stash_picker(&app.editor, repo.clone(), dir.path().into(), entries);
    until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    std::fs::write(dir.path().join("file.txt"), "second stash\n")?;
    let request = GitRequest::capture(
        &app.editor,
        dir.path().into(),
        GitAction::Stash(dir.path().into()),
    )?;
    request
        .prepare()
        .await?
        .admit(&mut app.editor)?
        .execute()
        .await
        .finish(&mut app.editor);
    until(&mut picker, &mut app, |picker| {
        crate::ui::picker::test::items(picker).count() == 2
    })
    .await?;
    assert_eq!(picker.selection().unwrap().id(), selected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stash_inspection_opens_native_review_without_changing_git_state() -> anyhow::Result<()> {
    let dir = repo()?;
    let repo = GitRepository::discover(dir.path())?;
    std::fs::write(dir.path().join("file.txt"), "saved change\n")?;
    repo.prepare_stash(true)?.execute().await?;
    let entries = repo.stashes().await?;
    let index = std::fs::read(dir.path().join(".git/index"))?;
    let mut app = application(false)?;
    let mut compositor = compositor::Compositor::new(view::graphics::Rect::new(0, 0, 80, 24));
    let mut jobs = crate::job::Jobs::new();
    let mut picker = stash_picker(
        &app.editor,
        repo.clone(),
        dir.path().into(),
        entries.clone(),
    );
    until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
    compositor.push(Box::new(overlaid(picker)));
    key(&mut app, &mut compositor, &mut jobs, "ret");
    let layer = compositor.pop().unwrap();
    let mut files = layer
        .as_boxed_any()
        .downcast::<ui::overlay::Overlay<Picker<StashFile, StashFiles>>>()
        .map_err(|_| anyhow::anyhow!("Expected stash files picker"))?;
    until(&mut files.content, &mut app, |picker| {
        picker.selection().is_some()
    })
    .await?;
    compositor.push(files);
    key(&mut app, &mut compositor, &mut jobs, "ret");
    assert!(view::current_ref!(app.editor).1.review().is_some());
    assert!(compositor
        .find::<ui::Popup<ui::change_review::ReviewView>>()
        .is_none());
    assert!(
        compositor.last_picker.is_some(),
        "the file browser remains restorable"
    );
    assert_eq!(std::fs::read(dir.path().join(".git/index"))?, index);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt"))?,
        "head\n"
    );
    assert_eq!(repo.stashes().await?, entries);
    Ok(())
}

fn frame(
    component: &mut dyn Component,
    app: &mut TestApplication,
    jobs: &mut crate::job::Jobs,
    config: &crate::config::Config,
    area: view::graphics::Rect,
) -> Vec<String> {
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut cx = Context {
        config: crate::config::Context {
            current: config,
            updates: &updates,
        },
        editor: &mut app.editor,
        interaction: &mut app.interaction,
        jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    let mut surface = tui::buffer::Buffer::empty(area);
    component.required_size((area.width, area.height));
    component.update_and_render(area, &mut surface, &mut cx);
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| surface[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn direct_git_shortcuts_clip_to_one_row_follow_remaps_and_preserve_paging(
) -> anyhow::Result<()> {
    use editor_config::BorderStyle;
    let mut app = application(false)?;
    let mut jobs = crate::job::Jobs::new();
    for remapped in [false, true] {
        let config = crate::config::Config::load(
            Ok(&if remapped {
                "[keys.git-picker]\nA-s = 'unbound'\nC-j = 'stage'\nA-z = 'unbound'".to_owned()
            } else {
                String::new()
            }),
            Ok(String::new()),
        )
        .unwrap();
        for width in [40, 120] {
            for border in [BorderStyle::None, BorderStyle::Rounded] {
                let mut editor_config = (*app.editor.config()).clone();
                editor_config.border_style = border;
                editor_config.picker.show_keybindings = true;
                app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(
                    editor_config,
                )));
                let mut picker = Picker::new(
                    app.editor.redraw().clone(),
                    [PickerColumn::new("name", |item: &usize, _: &()| {
                        format!("row {item}").into()
                    })],
                    0,
                    0..50,
                    (),
                    |_, _, _| {},
                )
                .with_help(|maps| file_help(&maps.git_picker));
                until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
                let rows = frame(
                    &mut picker,
                    &mut app,
                    &mut jobs,
                    &config,
                    view::graphics::Rect::new(0, 0, width, 12),
                );
                let screen = rows.join("\n");
                assert!(screen.contains("<ret> open"), "{screen}");
                assert!(screen.contains(if remapped {
                    "<C-j> stage"
                } else {
                    "<A-s> stage"
                }));
                if width == 120 {
                    for hint in ["<A-u> unstage", "<A-l> stashes", "<A-a> actions"] {
                        assert!(screen.contains(hint), "Missing {hint:?}: {screen}");
                    }
                    assert_eq!(screen.contains("<A-z> stash"), !remapped);
                } else {
                    assert!(!screen.contains("actions"), "{screen}");
                }
                assert!(rows[..rows.len() - 1]
                    .iter()
                    .all(|row| !row.contains("stage")));
                let visible = rows.iter().filter(|row| row.contains("row ")).count();
                assert!(visible > 0);
                let mut editor_config = (*app.editor.config()).clone();
                editor_config.picker.show_keybindings = false;
                app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(
                    editor_config,
                )));
                let hidden = frame(
                    &mut picker,
                    &mut app,
                    &mut jobs,
                    &config,
                    view::graphics::Rect::new(0, 0, width, 12),
                );
                assert!(!hidden.join("\n").contains("stage"));
                assert_eq!(
                    hidden.iter().filter(|row| row.contains("row ")).count(),
                    visible
                );
                assert_eq!(picker.selection(), Some(&0));
                picker.page_down();
                assert_eq!(picker.selection(), Some(&visible));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn action_choosers_keep_long_option_names_visible() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut jobs = crate::job::Jobs::new();
    let config = crate::config::Config::default();
    for width in [40, 80] {
        let mut menu = ActionMenu::new(
            "Git actions".into(),
            vec![
                Choice::new("Stage selected file on disk", Command::Cancel),
                Choice::new("Stash all repository changes…", Command::Cancel),
            ],
            std::path::PathBuf::new(),
        );
        let screen = frame(
            &mut menu,
            &mut app,
            &mut jobs,
            &config,
            view::graphics::Rect::new(0, 0, width, 20),
        )
        .join("\n");
        assert!(screen.contains("Stage selected file on disk"), "{screen}");
        assert!(screen.contains("Stash all repository changes…"), "{screen}");
    }
    Ok(())
}

#[tokio::test]
async fn branch_confirmation_frames_options_with_current_border_preference() -> anyhow::Result<()> {
    use editor_config::BorderStyle;

    let dir = repo()?;
    let repository = GitRepository::discover(dir.path())?;
    let mut app = application(false)?;
    let mut jobs = crate::job::Jobs::new();
    let config = crate::config::Config::default();
    let mut menu = confirm_branch_deletion(repository, "topic".into(), dir.path().into());

    for (border, left, right) in [
        (BorderStyle::Rounded, "│", "│"),
        (BorderStyle::Double, "║", "║"),
        (BorderStyle::Plain, "│", "│"),
        (BorderStyle::Thick, "┃", "┃"),
        (BorderStyle::None, " ", " "),
    ] {
        let mut editor_config = (*app.editor.config()).clone();
        editor_config.border_style = border;
        app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(
            editor_config,
        )));
        for width in [40, 80] {
            let rows = frame(
                menu.as_mut(),
                &mut app,
                &mut jobs,
                &config,
                view::graphics::Rect::new(0, 0, width, 20),
            );
            let screen = rows.join("\n");
            assert!(screen.contains("Delete branch topic?"), "{screen}");
            assert!(!screen.contains("are kept"), "{screen}");
            assert!(!screen.contains("Delete merged branch"), "{screen}");
            for label in ["Cancel", "Delete branch"] {
                let row = rows
                    .iter()
                    .find(|row| row.contains(label) && !row.contains("topic"))
                    .unwrap();
                let trimmed = row.trim();
                if border == BorderStyle::None {
                    assert_eq!(trimmed.trim_start_matches("> "), label, "{screen}");
                    assert!(
                        !screen.contains('│') && !screen.contains('╭') && !screen.contains('║'),
                        "{screen}"
                    );
                } else {
                    assert!(
                        trimmed.starts_with(left) && trimmed.ends_with(right),
                        "{screen}"
                    );
                }
            }
        }
        for (width, height) in [(1, 1), (8, 4)] {
            frame(
                menu.as_mut(),
                &mut app,
                &mut jobs,
                &config,
                view::graphics::Rect::new(0, 0, width, height),
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn branch_confirmation_arrow_follows_selection() -> anyhow::Result<()> {
    let dir = repo()?;
    let repository = GitRepository::discover(dir.path())?;
    let mut app = application(false)?;
    let mut jobs = crate::job::Jobs::new();
    let config = crate::config::Config::default();
    let area = view::graphics::Rect::new(0, 0, 40, 12);
    let mut compositor = compositor::Compositor::new(area);
    compositor.push(confirm_branch_deletion(
        repository,
        "topic".into(),
        dir.path().into(),
    ));

    for label in ["Cancel", "Delete branch", "Cancel"] {
        let screen = frame(
            compositor.find::<ActionMenu>().unwrap(),
            &mut app,
            &mut jobs,
            &config,
            area,
        )
        .join("\n");
        assert!(screen.contains(&format!("> {label}")), "{screen}");
        assert_eq!(screen.matches('>').count(), 1, "{screen}");
        key(&mut app, &mut compositor, &mut jobs, "down");
    }
    Ok(())
}

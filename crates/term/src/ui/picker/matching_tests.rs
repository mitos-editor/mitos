use super::test::application;
use super::*;
use crate::{config::Config as AppConfig, job::Jobs};
use std::{path::PathBuf, time::Duration};

async fn settle<T: Send + Sync, D: Send + Sync>(picker: &mut Picker<T, D>, editor: &Editor) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            picker.poll_query_options(editor);
            picker.publish_options();
            let status = picker.results.matcher.tick(10);
            if picker.query_options.pending.is_none()
                && !status.running
                && picker.results.matcher.active_injectors() == 0
                && !picker.results.is_refreshing()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn matched<T: Send + Sync + Clone, D>(picker: &Picker<T, D>) -> Vec<T> {
    let snapshot = picker.results.matcher.snapshot();
    snapshot
        .matched_items(..)
        .map(|item| item.data.clone())
        .collect()
}

#[tokio::test]
async fn file_picker_match_text_excludes_icons() -> anyhow::Result<()> {
    let app = application(false)?;
    let dir = tempfile::tempdir()?;
    let relative = PathBuf::from("src").join("café.rs");
    let path = dir.path().join(&relative);
    let picker = ui::file_picker(&app.editor, dir.path().to_owned());
    for icons in [false, true] {
        let data = ui::FilePickerData {
            root: dir.path().to_owned(),
            theme: Arc::new(app.editor.theme.clone()),
            icons,
            files: picker.results.editor_data.files.clone(),
        };
        assert_eq!(
            picker.columns[0].format_text(&path, &data),
            relative.to_string_lossy()
        );
        assert_eq!((picker.columns[0].prefix)(&path, &data).is_some(), icons);
    }
    Ok(())
}

#[tokio::test]
async fn file_explorer_match_text_excludes_icons() -> anyhow::Result<()> {
    let app = application(false)?;
    let dir = tempfile::tempdir()?;
    let picker = ui::file_explorer(dir.path().to_owned(), &app.editor)?;
    for show_icons in [false, true] {
        let data = ui::file_explorer::ExplorerData {
            root: dir.path().to_owned(),
            theme: Arc::new(app.editor.theme.clone()),
            show_icons,
            tree: None,
        };
        for (name, is_dir, expected) in [("café.rs", false, "café.rs"), ("src", true, "src/")] {
            let item = (dir.path().join(name), is_dir);
            assert_eq!(picker.columns[0].format_text(&item, &data), expected);
            assert_eq!(
                (picker.columns[0].prefix)(&item, &data).is_some(),
                show_icons
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn embedded_picker_waits_for_unpublished_directory_rows() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    let mut picker =
        ui::file_explorer::picker(dir.path().to_owned(), &app.editor, true)?.embedded(|_, _| {});
    test::draw(&mut picker, &mut app);
    settle(&mut picker, &app.editor).await;
    // Keep the replacement incomplete regardless of worker scheduling.
    picker.refresh_options(matched(&picker), picker.results.editor_data.clone());
    let held = picker
        .results
        .pending_options
        .as_ref()
        .unwrap()
        .matcher
        .injector();
    picker.matching = false;
    assert!(picker.has_pending_render());
    drop(held);
    settle(&mut picker, &app.editor).await;
    assert!(!picker.has_pending_render());
    Ok(())
}

#[tokio::test]
async fn tree_search_preserves_navigation_on_refresh_and_resets_it_for_a_new_query(
) -> anyhow::Result<()> {
    let app = application(false)?;
    let dir = tempfile::tempdir()?;
    for name in ["alpha/a_test.rs", "zeta/test.rs"] {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, "")?;
    }
    let mut picker = ui::file_explorer::picker(dir.path().to_owned(), &app.editor, true)?;
    let tree = picker.editor_data().tree.as_ref().unwrap().clone();
    picker.prompt.set_line("test".into(), &app.editor);
    picker.handle_prompt_change(false);
    picker.refresh_query_options(&app.editor, false);
    settle(&mut picker, &app.editor).await;
    assert_eq!(
        picker.selection().unwrap().0,
        dir.path().join("alpha/a_test.rs")
    );

    picker.results.cursor = 3;
    assert_eq!(
        picker.selection().unwrap().0,
        dir.path().join("zeta/test.rs")
    );
    std::fs::write(dir.path().join("alpha/0_test.rs"), "")?;
    tree.clear_index();
    picker.refresh_query_options(&app.editor, true);
    settle(&mut picker, &app.editor).await;
    assert_eq!(
        picker.selection().unwrap().0,
        dir.path().join("zeta/test.rs")
    );

    picker.refresh_query_options(&app.editor, true);
    picker.results.cursor = 2;
    settle(&mut picker, &app.editor).await;
    assert_eq!(
        picker.selection().unwrap().0,
        dir.path().join("alpha/a_test.rs")
    );

    // Fuzzy matching still accepts noncontiguous characters, but each new
    // query starts at the first match instead of retaining a later selection.
    picker.prompt.set_line("ts".into(), &app.editor);
    picker.handle_prompt_change(false);
    picker.refresh_query_options(&app.editor, false);
    settle(&mut picker, &app.editor).await;
    assert_eq!(
        picker.selection().unwrap().0,
        dir.path().join("alpha/0_test.rs")
    );
    Ok(())
}

#[tokio::test]
async fn decorative_prefixes_do_not_match_and_preserve_unicode_highlights() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("path", |item: &PathBuf, _: &()| {
            item.to_string_lossy().into_owned().into()
        })
        .with_prefix(|_, _| {
            Some(Span::styled(
                "🦀 icon ",
                tui::style::Style::default().fg(tui::style::Color::Red),
            ))
        })],
        0,
        [PathBuf::from("café.rs"), PathBuf::from("café.txt")],
        (),
        |_, _, _| {},
    );
    picker.results.reparse(0, "^icon", false);
    settle(&mut picker, &app.editor).await;
    assert!(matched(&picker).is_empty());
    picker.results.reparse(0, "^café", false);
    settle(&mut picker, &app.editor).await;
    assert_eq!(
        matched(&picker),
        [PathBuf::from("café.rs"), PathBuf::from("café.txt")]
    );

    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = Jobs::new();
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
    let area = Rect::new(0, 0, 70, 20);
    let mut surface = Surface::empty(area);
    picker.update_and_render(area, &mut surface, &mut cx);
    let row = surface
        .content
        .chunks(area.width as usize)
        .find(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains("icon café.txt")
        })
        .expect("decorated result is visible");
    let start = row
        .windows(4)
        .position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "café")
        .unwrap();
    let highlight = cx
        .editor
        .theme
        .get("ui.picker.match")
        .add_modifier(Modifier::BOLD);
    for cell in &row[start..start + 4] {
        assert!(cell.modifier.contains(tui::style::Modifier::BOLD));
        if let Some(fg) = highlight.fg {
            assert_eq!(cell.fg, fg);
        }
    }
    let icon = row.iter().find(|cell| cell.symbol() == "🦀").unwrap();
    assert_eq!(icon.fg, tui::style::Color::Red);
    assert!(!icon.modifier.contains(tui::style::Modifier::BOLD));
    assert!(!row[start + 4].modifier.contains(tui::style::Modifier::BOLD));

    let path = PathBuf::from("a/very/long/directory/path/café.txt");
    picker
        .results
        .reset(vec![path], Arc::new(()), &picker.columns);
    picker.results.reparse(0, "café", false);
    settle(&mut picker, cx.editor).await;
    for width in [28, 70, 28] {
        let area = Rect::new(0, 0, width, 20);
        let mut surface = Surface::empty(area);
        picker.update_and_render(area, &mut surface, &mut cx);
        let row = surface
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .find(|row| row.contains("café.txt"))
            .expect("filename stays visible after resizing");
        assert!(row.contains('🦀') && row.contains(" icon "));
        assert_eq!(row.contains('…'), width == 28);
    }
    Ok(())
}

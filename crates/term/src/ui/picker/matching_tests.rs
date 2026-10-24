use super::test::application;
use super::*;
use crate::{config::Config as AppConfig, job::Jobs};
use std::{path::PathBuf, time::Duration};

async fn settle<T: Send + Sync, D: Send + Sync>(picker: &mut Picker<T, D>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = picker.results.matcher.tick(10);
            if !status.running && picker.results.matcher.active_injectors() == 0 {
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
async fn file_picker_match_text_and_order_ignore_icons() -> anyhow::Result<()> {
    let app = application(false)?;
    let dir = tempfile::tempdir()?;
    let names = [
        "core.rs",
        "cr.rs",
        "café.rs",
        "🦀.rs",
        "README.md",
        "src/core.rs",
    ];
    let paths: Vec<_> = names.iter().map(|name| dir.path().join(name)).collect();
    for path in &paths {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, "")?;
    }
    let mut picker = ui::file_picker(&app.editor, dir.path().to_owned());
    settle(&mut picker).await;
    let patterns = ["", "core", "^core", "cr", "rs$", "!md", "café", "🦀"];
    let mut expected = Vec::new();
    for icons in [false, true] {
        picker.results.reset(
            paths.clone(),
            Arc::new(ui::FilePickerData {
                root: dir.path().to_owned(),
                theme: Arc::new(app.editor.theme.clone()),
                icons,
            }),
            &picker.columns,
        );
        for (index, pattern) in patterns.iter().enumerate() {
            picker.results.reparse(0, pattern, false);
            settle(&mut picker).await;
            let snapshot = picker.results.matcher.snapshot();
            for item in snapshot.matched_items(..) {
                assert_eq!(
                    item.matcher_columns[0].slice(..).to_string(),
                    item.data
                        .strip_prefix(dir.path())?
                        .components()
                        .collect::<PathBuf>()
                        .to_string_lossy(),
                );
            }
            if icons {
                assert_eq!(matched(&picker), expected[index], "query: {pattern}");
            } else {
                expected.push(matched(&picker));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn file_explorer_match_text_and_order_ignore_icons() -> anyhow::Result<()> {
    let app = application(false)?;
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("core"))?;
    std::fs::write(dir.path().join("core.rs"), "")?;
    std::fs::write(dir.path().join("cr.rs"), "")?;
    let mut picker = ui::file_explorer(dir.path().to_owned(), &app.editor)?;
    settle(&mut picker).await;
    let options = matched(&picker);
    let mut expected = Vec::new();
    for icons in [false, true] {
        picker.results.reset(
            options.clone(),
            Arc::new((
                dir.path().to_owned(),
                Arc::new(app.editor.theme.clone()),
                icons,
            )),
            &picker.columns,
        );
        for (index, pattern) in ["", "core", "^core", "cr"].iter().enumerate() {
            picker.results.reparse(0, pattern, false);
            settle(&mut picker).await;
            for item in picker.results.matcher.snapshot().matched_items(..) {
                let (path, is_dir) = item.data;
                let name = path
                    .file_name()
                    .map_or_else(|| "..".into(), |name| name.to_string_lossy());
                let text = if *is_dir {
                    format!("{name}/")
                } else {
                    name.into_owned()
                };
                assert_eq!(item.matcher_columns[0].slice(..).to_string(), text);
            }
            if icons {
                assert_eq!(matched(&picker), expected[index], "query: {pattern}");
            } else {
                expected.push(matched(&picker));
            }
        }
    }
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
    settle(&mut picker).await;
    assert!(matched(&picker).is_empty());
    picker.results.reparse(0, "^café", false);
    settle(&mut picker).await;
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
        jobs: &mut jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    let area = Rect::new(0, 0, 70, 20);
    let mut surface = Surface::empty(area);
    picker.render(area, &mut surface, &mut cx);
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
    let highlight = cx.editor.theme.get("special").add_modifier(Modifier::BOLD);
    for cell in &row[start..start + 4] {
        assert!(cell.modifier.contains(tui::style::Modifier::BOLD));
        if let Some(fg) = highlight.fg {
            assert_eq!(cell.fg, fg.into());
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
    settle(&mut picker).await;
    for width in [28, 70, 28] {
        let area = Rect::new(0, 0, width, 20);
        let mut surface = Surface::empty(area);
        picker.render(area, &mut surface, &mut cx);
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

#[tokio::test]
async fn implicit_directory_browser_respects_the_configured_kind() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let dir = tempfile::tempdir()?;
    let picker = ui::directory_picker(&app.editor, dir.path().to_owned(), Action::Replace)?;
    assert!(picker.as_any().is::<ui::overlay::Overlay<ui::FilePicker>>());
    let mut config = (*app.editor.config()).clone();
    config.files.directory_opener = editor_config::DirectoryOpener::FileExplorer;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    let picker = ui::directory_picker(&app.editor, dir.path().to_owned(), Action::Replace)?;
    assert!(picker
        .as_any()
        .is::<ui::overlay::Overlay<ui::FileExplorer>>());
    Ok(())
}

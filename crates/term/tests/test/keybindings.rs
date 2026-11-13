use super::helpers::test;

#[tokio::test(flavor = "multi_thread")]
async fn line_jump_aliases_preserve_selection_anchors() -> anyhow::Result<()> {
    for keys in ["v3G", "v3gg", "vG", "vge"] {
        test(("#[o|]#ne\ntwo\nthree", keys, "#[one\ntwo\nt|]#hree")).await?;
    }
    test(("#[o|]#ne\ntwo\nthree", "G", "one\ntwo\n#[t|]#hree")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn view_search_aliases_add_matches_in_select_mode() -> anyhow::Result<()> {
    for prefix in ["", "z", "Z"] {
        test((
            "#[one|]# two one",
            format!("*v{prefix}n"),
            "#(one|)# two #[one|]#",
        ))
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_insert_prefixes_does_not_insert_control_keys_or_restart_them(
) -> anyhow::Result<()> {
    use super::helpers::{self, AppBuilder};
    for sequence in ["i<C-g><esc>x", "i<C-g>!x"] {
        let mut app = AppBuilder::new().build()?;
        helpers::keys(&mut app, sequence).await?;
        let expected = if sequence.contains('!') {
            "!x\n"
        } else {
            "x\n"
        };
        assert_eq!(
            view::doc!(app.editor).text().to_string(),
            expected.replace('\n', editor_core::NATIVE_LINE_ENDING.as_str())
        );
        assert_eq!(app.editor.mode(), view::document::Mode::Insert);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn save_shortcuts_preserve_the_editor_mode() -> anyhow::Result<()> {
    use super::helpers::{self, AppBuilder};
    use view::document::Mode;
    for (enter, mode) in [("", Mode::Normal), ("v", Mode::Select), ("i", Mode::Insert)] {
        let file = tempfile::NamedTempFile::new()?;
        let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
        helpers::keys(&mut app, &format!("ihello<esc>{enter}<C-s>")).await?;
        assert_eq!(
            std::fs::read_to_string(file.path())?,
            format!("hello{}", editor_core::NATIVE_LINE_ENDING.as_str())
        );
        assert_eq!(app.editor.mode(), mode);
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_checkpoint_splits_undo_without_leaving_insert_mode() -> anyhow::Result<()> {
    use super::helpers::{self, AppBuilder};
    let mut app = AppBuilder::new().build()?;
    helpers::keys(&mut app, "ione<C-g>utwo<esc>u").await?;
    assert_eq!(
        view::doc!(app.editor).text().to_string(),
        format!("one{}", editor_core::NATIVE_LINE_ENDING.as_str())
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_search_uses_file_picker_root_from_a_nested_directory() -> anyhow::Result<()> {
    use super::helpers::{self, AppBuilder};
    let Some(root) = helpers::isolation::workspace(
        "test::keybindings::workspace_search_uses_file_picker_root_from_a_nested_directory",
    )?
    else {
        return Ok(());
    };
    let workspace = root.join("workspace");
    let nested = workspace.join("nested");
    std::fs::create_dir(&nested)?;
    std::fs::write(workspace.join("parent.txt"), "unique_needle\n")?;
    stdx::env::set_current_working_dir(&nested)?;
    let mut app = AppBuilder::new().build()?;
    helpers::keys(&mut app, "<space>/unique_needle").await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            helpers::keys(&mut app, "").await?;
            let screen: String = app
                .terminal_backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if screen.contains("parent.txt") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    helpers::keys(&mut app, "<esc><space>fparent.txt<ret>").await?;
    assert_eq!(
        view::doc!(app.editor).path(),
        Some(workspace.join("parent.txt").as_path())
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

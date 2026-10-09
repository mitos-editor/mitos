//! Backend hooks at real terminal save/close boundaries.
use super::helpers::{test_config, test_key_sequences, AppBuilder};

#[path = "../../../view/tests/support/plugin_guest.rs"]
mod guest;

use guest::{observing, status, Route};

#[tokio::test(flavor = "multi_thread")]
async fn normal_save_and_write_quit_deliver_saved_text_before_shutdown() -> anyhow::Result<()> {
    for write_quit in [false, true] {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("document.txt");
        std::fs::write(&source, "original\n")?;
        let save_dir = tempfile::tempdir()?;
        let mut config = test_config();
        config.plugins.insert(
            "a-save-observer".into(),
            observing(
                save_dir.path(),
                &["document-saved"],
                &[Route {
                    event: "document-saved",
                    response: status("saved text observed"),
                    expected: vec![
                        r#""text":"Xoriginal\n""#.into(),
                        r#""saved_revision":1"#.into(),
                        r#""snapshot_available":true"#.into(),
                    ],
                    ..Route::default()
                }],
                Some("document-saved"),
            )?,
        );
        let close_dir = tempfile::tempdir()?;
        if write_quit {
            config.plugins.insert(
                "b-close-observer".into(),
                observing(
                    close_dir.path(),
                    &["post-command"],
                    &[
                        Route {
                            event: "post-command",
                            filter: Some(r#""command":"write-quit""#.into()),
                            response: status("closed command observed"),
                            expected: vec![
                                r#""text":"Xoriginal\n""#.into(),
                                r#""view":null"#.into(),
                                r#""outcome":"success""#.into(),
                            ],
                            ..Route::default()
                        },
                        Route {
                            event: "shutdown",
                            response: status("shutdown after close command"),
                            ..Route::default()
                        },
                    ],
                    Some("post-command"),
                )?,
            );
        }
        let mut app = AppBuilder::new()
            .with_config(config)
            .with_file(&source, None)
            .build()?;
        let keys = if write_quit {
            "iX<esc>:wq<ret>"
        } else {
            "iX<esc>:write<ret>"
        };
        let assert_saved = |app: &term::application::Application| {
            assert_eq!(app.editor.get_status().unwrap().0, "saved text observed");
        };
        test_key_sequences(
            &mut app,
            vec![(
                Some(keys),
                (!write_quit).then_some(&assert_saved as &dyn Fn(&term::application::Application)),
            )],
            write_quit,
        )
        .await?;
        assert_eq!(std::fs::read_to_string(&source)?, "Xoriginal\n");
        assert_eq!(
            app.editor.error_revision(),
            0,
            "both real guests must observe their event before shutdown"
        );
        if write_quit {
            assert_eq!(
                app.editor.get_status().unwrap().0,
                "shutdown after close command"
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_last_split_keeps_its_post_command_hook() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut config = test_config();
    config.keys = toml::from_str("[normal]\nF12 = 'wclose'\n")?;
    config.plugins.insert(
        "observer".into(),
        observing(
            dir.path(),
            &["post-command"],
            &[
                Route {
                    event: "post-command",
                    filter: Some(r#""command":"wclose""#.into()),
                    response: status("last split close observed"),
                    expected: vec![
                        r#""document":{"id":"#.into(),
                        r#""view":null"#.into(),
                        r#""outcome":"success""#.into(),
                    ],
                    ..Route::default()
                },
                Route {
                    event: "shutdown",
                    response: status("shutdown after last split"),
                    ..Route::default()
                },
            ],
            Some("post-command"),
        )?,
    );
    let mut app = AppBuilder::new().with_config(config).build()?;
    test_key_sequences(&mut app, vec![(Some("<F12>"), None)], true).await?;
    assert_eq!(app.editor.error_revision(), 0);
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "shutdown after last split"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn callbacks_accepted_before_close_can_save_before_plugin_shutdown() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("document.txt");
    std::fs::write(&source, "original\n")?;
    let guest_dir = tempfile::tempdir()?;
    let mut config = test_config();
    config.plugins.insert(
        "observer".into(),
        observing(
            guest_dir.path(),
            &["document-saved"],
            &[
                Route {
                    event: "document-saved",
                    response: status("queued write observed"),
                    expected: vec![r#""text":"queued-save\n""#.into()],
                    ..Route::default()
                },
                Route {
                    event: "shutdown",
                    response: status("shutdown after queued write"),
                    ..Route::default()
                },
            ],
            Some("document-saved"),
        )?,
    );
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&source, None)
        .build()?;
    let (view, doc) = view::current!(app.editor);
    let id = doc.id();
    let transaction = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("queued-save\n".into()))].into_iter(),
    );
    assert!(doc.apply(&transaction, view.id));
    doc.append_changes_to_history(view);
    term::job::dispatch_callback(term::job::Callback::Editor(Box::new(move |editor| {
        editor.save(id, None::<std::path::PathBuf>, false).unwrap();
    })))
    .await;
    assert_eq!(
        app.editor.write_count, 0,
        "write is still in the accepted callback"
    );
    assert!(app.close().await.is_empty());
    assert_eq!(std::fs::read_to_string(source)?, "queued-save\n");
    assert_eq!(app.editor.error_revision(), 0);
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "shutdown after queued write"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_required_jobs_do_not_skip_other_accepted_writes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("document.txt");
    std::fs::write(&source, "original\n")?;
    let guest_dir = tempfile::tempdir()?;
    let mut config = test_config();
    config.plugins.insert(
        "observer".into(),
        observing(
            guest_dir.path(),
            &["document-saved"],
            &[
                Route {
                    event: "document-saved",
                    response: status("required write observed"),
                    ..Route::default()
                },
                Route {
                    event: "shutdown",
                    response: status("shutdown after required write"),
                    ..Route::default()
                },
            ],
            Some("document-saved"),
        )?,
    );
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&source, None)
        .build()?;
    let (view, doc) = view::current!(app.editor);
    let id = doc.id();
    let transaction = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("required-save\n".into()))].into_iter(),
    );
    assert!(doc.apply(&transaction, view.id));
    doc.append_changes_to_history(view);
    term::job::dispatch_callback(term::job::Callback::Followup(Box::new(|_| {
        Some(
            term::job::Job::new(async { anyhow::bail!("expected required job failure") })
                .wait_before_exiting(),
        )
    })))
    .await;
    term::job::dispatch_callback(term::job::Callback::Followup(Box::new(move |_| {
        Some(
            term::job::Job::with_callback(async move {
                Ok(term::job::Callback::Editor(Box::new(move |editor| {
                    editor.save(id, None::<std::path::PathBuf>, false).unwrap();
                })))
            })
            .wait_before_exiting(),
        )
    })))
    .await;
    let errors = app.close().await;
    assert_eq!(errors.len(), 1);
    assert!(errors[0]
        .to_string()
        .contains("expected required job failure"));
    assert_eq!(std::fs::read_to_string(source)?, "required-save\n");
    assert_eq!(
        app.editor.get_status().unwrap().0,
        "shutdown after required write"
    );
    assert_eq!(
        app.editor.error_revision(),
        0,
        "guest shutdown still follows the successful save hook"
    );
    Ok(())
}

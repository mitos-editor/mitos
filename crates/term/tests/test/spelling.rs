use std::{fs, time::Duration};

use editor_core::{diagnostic::DiagnosticProvider, Range, Selection, Transaction};
use term::{
    application::Application,
    ui::{Menu, Popup},
};
use view::{
    action::Action, current, current_ref, handlers::spelling::IgnoredWordsFile,
    quicklist::QuicklistTarget,
};

use super::helpers::*;

fn mistakes(app: &Application) -> Vec<String> {
    let (_, doc) = current_ref!(app.editor);
    doc.diagnostics()
        .iter()
        .filter(|d| d.provider == DiagnosticProvider::Spelling)
        .map(|d| doc.text().slice(d.range.start..d.range.end).to_string())
        .collect()
}

async fn wait_for_mistakes(app: &mut Application, expected: &[&str]) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(app).await;
            if mistakes(app) == expected {
                break;
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("expected {expected:?}, got {:?}", mistakes(app)))
}

fn replace(app: &mut Application, start: usize, end: usize, replacement: &str) {
    let (view, doc) = current!(app.editor);
    let tx = Transaction::change(
        doc.text(),
        [(start, end, Some(replacement.into()))].into_iter(),
    );
    doc.apply(&tx, view.id);
    doc.append_changes_to_history(view);
}

async fn keys(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    for key in ui_core::input::parse_macro(keys)? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(key.into());
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(key.into());
        app.handle_terminal_events(Ok(event)).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(app)).await?;
    Ok(())
}

async fn selected_code_action(app: &mut Application) -> anyhow::Result<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    term::job::dispatch(move |_, compositor| {
        let title = compositor
            .find_id::<Popup<Menu<Action>>>("code-action")
            .and_then(|popup| popup.contents().selection())
            .map(|action| action.title().to_owned());
        let _ = tx.send(title);
    })
    .await;
    app.editor.reset_idle_timer();
    run_event_loop_until_idle(app).await;
    Ok(rx.await?)
}

async fn open_corrections(app: &mut Application) -> anyhow::Result<()> {
    keys(app, "<space>a").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while selected_code_action(app).await?.is_none() {}
        anyhow::Ok(())
    })
    .await??;
    assert_status_not_error(&app.editor);
    Ok(())
}

async fn choose_correction(app: &mut Application, title: &str) -> anyhow::Result<()> {
    let first = selected_code_action(app).await?.unwrap();
    let mut selected = first.clone();
    loop {
        if selected == title {
            return keys(app, "<ret>").await;
        }
        keys(app, "<C-n>").await?;
        selected = selected_code_action(app).await?.unwrap();
        anyhow::ensure!(selected != first, "missing code action {title:?}");
    }
}

fn selection(app: &Application) -> &Selection {
    let (view, doc) = current_ref!(app.editor);
    doc.selection(view.id)
}

fn select(app: &mut Application, selection: Selection) {
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, selection);
}

async fn navigation_app() -> anyhow::Result<Application> {
    let mut app = AppBuilder::new()
        .with_input_text("#[🚀|]# teh hello quik world wrld\n")
        .build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["teh", "quik", "wrld"]).await?;
    // A non-spelling diagnostic between the findings must be skipped, even with the same source
    // label. Navigation and textobjects should filter by provider, not severity or source text.
    let (_, doc) = current!(app.editor);
    let mut diagnostic = doc.diagnostics()[0].clone();
    diagnostic.range = editor_core::diagnostic::Range { start: 6, end: 11 };
    let provider = DiagnosticProvider::Lsp {
        server_id: Default::default(),
        identifier: None,
    };
    diagnostic.provider = provider.clone();
    doc.replace_diagnostics([diagnostic], &[], Some(&provider));
    Ok(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_navigation_skips_other_diagnostics_and_respects_counts_and_boundaries(
) -> anyhow::Result<()> {
    let mut app = navigation_app().await?;
    keys(&mut app, "]s").await?;
    assert_eq!(selection(&app), &Selection::single(2, 5));
    keys(&mut app, "<A-.>").await?;
    assert_eq!(selection(&app), &Selection::single(12, 16));
    // Reversing direction skips the selected finding rather than selecting it again.
    keys(&mut app, "[s").await?;
    assert_eq!(selection(&app), &Selection::single(5, 2));
    keys(&mut app, "[s").await?;
    assert_eq!(selection(&app), &Selection::single(5, 2));
    keys(&mut app, "2]s").await?;
    assert_eq!(selection(&app), &Selection::single(23, 27));
    keys(&mut app, "]s").await?;
    assert_eq!(selection(&app), &Selection::single(23, 27));
    keys(&mut app, "99[s").await?;
    assert_eq!(selection(&app), &Selection::single(5, 2));
    keys(&mut app, "99]s").await?;
    assert_eq!(selection(&app), &Selection::single(23, 27));
    // A cursor immediately after a finding can navigate back to it.
    select(&mut app, Selection::point(16));
    keys(&mut app, "[s").await?;
    assert_eq!(selection(&app), &Selection::single(16, 12));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_navigation_moves_each_cursor_and_extends_selections() -> anyhow::Result<()> {
    let mut app = navigation_app().await?;
    select(
        &mut app,
        Selection::new(vec![Range::point(0), Range::point(11)].into(), 1),
    );
    keys(&mut app, "]s").await?;
    assert_eq!(
        selection(&app),
        &Selection::new(vec![Range::new(2, 5), Range::new(12, 16)].into(), 1)
    );
    select(
        &mut app,
        Selection::new(vec![Range::point(12), Range::point(27)].into(), 0),
    );
    keys(&mut app, "[s").await?;
    assert_eq!(
        selection(&app),
        &Selection::new(vec![Range::new(5, 2), Range::new(27, 23)].into(), 0)
    );
    select(&mut app, Selection::single(0, 1));
    keys(&mut app, "v2]s").await?;
    assert_eq!(selection(&app), &Selection::single(0, 16));
    keys(&mut app, "[s").await?;
    assert_eq!(selection(&app), &Selection::single(0, 5));
    select(&mut app, Selection::single(28, 27));
    keys(&mut app, "2[s").await?;
    assert_eq!(selection(&app), &Selection::single(28, 12));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_textobjects_select_findings_and_preserve_direction() -> anyhow::Result<()> {
    let mut app = navigation_app().await?;
    select(&mut app, Selection::single(3, 4));
    keys(&mut app, "mis").await?;
    assert_eq!(selection(&app), &Selection::single(2, 5));
    select(&mut app, Selection::single(4, 3));
    keys(&mut app, "mas").await?;
    assert_eq!(selection(&app), &Selection::single(5, 2));
    // Outside a finding, the textobject leaves the selection alone.
    select(&mut app, Selection::single(6, 7));
    keys(&mut app, "mis").await?;
    assert_eq!(selection(&app), &Selection::single(6, 7));
    select(&mut app, Selection::single(0, 17));
    keys(&mut app, "mIs").await?;
    assert_eq!(
        selection(&app),
        &Selection::new(vec![Range::new(2, 5), Range::new(12, 16)].into(), 0)
    );
    // The partially selected first finding and the non-spelling diagnostic are excluded.
    select(&mut app, Selection::single(28, 3));
    keys(&mut app, "mAs").await?;
    assert_eq!(
        selection(&app),
        &Selection::new(vec![Range::new(16, 12), Range::new(27, 23)].into(), 0)
    );
    keys(&mut app, ":spelling off<ret>").await?;
    let before = selection(&app).clone();
    keys(&mut app, "[s]smis").await?;
    assert_eq!(selection(&app), &before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opt_in_commands_corrections_and_undo() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("t#[e|]#h quik\n")
        .build()?;
    run_event_loop_until_idle(&mut app).await;
    assert!(mistakes(&app).is_empty());
    keys(&mut app, ":spelling en_US en_US<ret>").await?;
    assert_eq!(current_ref!(app.editor).1.spelling_languages.len(), 1);
    wait_for_mistakes(&mut app, &["teh", "quik"]).await?;
    // A later dictionary with no useful corrections must not erase earlier suggestions.
    let second = "second_dictionary".parse()?;
    app.editor.dictionaries.insert(
        second,
        std::sync::Arc::new(view::Dictionary::new("SET UTF-8\n", "1\nworld\n").unwrap()),
    );
    current!(app.editor)
        .1
        .spelling_languages
        .push("second_dictionary".parse()?);
    let actions = app.editor.spelling_actions().await?;
    let correction = actions
        .iter()
        .find(|a| a.title() == "Replace 'teh' with 'the'")
        .unwrap();
    // Exercise the real command and popup with LSP disabled, rather than invoking the action
    // helper directly. Corrections from the first dictionary survive the later empty results.
    open_corrections(&mut app).await?;
    choose_correction(&mut app, "Replace 'teh' with 'the'").await?;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "the quik\n");
    wait_for_mistakes(&mut app, &["quik"]).await?;
    keys(&mut app, "u").await?;
    wait_for_mistakes(&mut app, &["teh", "quik"]).await?;
    // A menu captured before an edit must not overwrite a newer buffer version.
    correction.execute(&mut app.editor);
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "teh quik\n");
    assert_eq!(mistakes(&app), ["teh", "quik"]);
    let id = current_ref!(app.editor).1.id();
    let pending = app.editor.handlers.spelling.open_request(id);
    keys(&mut app, ":spelling off<ret>").await?;
    assert!(pending.is_canceled());
    wait_for_mistakes(&mut app, &[]).await?;
    assert!(app.editor.spelling_actions().await?.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_correction_menu_can_be_dismissed_and_replaces_unicode_ranges(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("🚀 w#[r|]#ld hello\n")
        .build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["wrld"]).await?;

    open_corrections(&mut app).await?;
    // The personal-dictionary action is reachable through the same menu. Dismiss it without
    // writing to the user's dictionary.
    keys(&mut app, "<C-p>").await?;
    assert_eq!(
        selected_code_action(&mut app).await?.as_deref(),
        Some("Add 'wrld' to dictionary 'en_US'")
    );
    keys(&mut app, "<esc>").await?;
    assert!(selected_code_action(&mut app).await?.is_none());
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "🚀 wrld hello\n"
    );

    open_corrections(&mut app).await?;
    choose_correction(&mut app, "Replace 'wrld' with 'world'").await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "🚀 world hello\n"
    );
    wait_for_mistakes(&mut app, &[]).await?;
    keys(&mut app, "u").await?;
    wait_for_mistakes(&mut app, &["wrld"]).await?;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "🚀 wrld hello\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_spelling_corrections_keep_their_original_document_version() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[t|]#eh\n").build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["teh"]).await?;

    // Request creation snapshots the target before the future is polled. Editing remains
    // possible while it is pending, and the old replacement cannot overwrite the newer text.
    let pending = app.editor.spelling_actions();
    replace(&mut app, 0, 3, "hello");
    let actions = pending.await?;
    actions
        .iter()
        .find(|action| action.title() == "Replace 'teh' with 'the'")
        .unwrap()
        .execute(&mut app.editor);
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "hello\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn code_actions_report_no_actions_outside_spelling_findings() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[h|]#ello teh\n")
        .build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["teh"]).await?;
    keys(&mut app, "<space>a").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app
            .editor
            .get_status()
            .is_some_and(|(message, _)| message == "No code actions available")
        {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    assert!(selected_code_action(&mut app).await?.is_none());
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "hello teh\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn session_ignore_survives_edits_and_settings_changes_without_persisting(
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[Z|]#orblé zorblé ZORBLÉ quik\n")
        .build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["Zorblé", "zorblé", "ZORBLÉ", "quik"]).await?;
    let language = "en_US".parse()?;
    let dictionary = app.editor.dictionaries[&language].clone();
    let personal_path = loader::personal_dictionary_file("en_US");
    let personal_before = fs::read(&personal_path).ok();
    let original = current_ref!(app.editor).1.text().to_string();
    let version = current_ref!(app.editor).1.version();
    let config = (*app.editor.config()).clone();

    // An overlapping LSP diagnostic must survive ignoring the spelling finding.
    let (doc_id, provider) = {
        let (_, doc) = current!(app.editor);
        let mut diagnostic = doc.diagnostics()[0].clone();
        let provider = DiagnosticProvider::Lsp {
            server_id: Default::default(),
            identifier: None,
        };
        diagnostic.provider = provider.clone();
        doc.replace_diagnostics([diagnostic], &[], Some(&provider));
        (doc.id(), provider)
    };
    open_corrections(&mut app).await?;
    let pending = app.editor.handlers.spelling.open_request(doc_id);
    choose_correction(&mut app, "Ignore 'Zorblé' for this session (en_US)").await?;
    assert!(pending.is_canceled());
    wait_for_mistakes(&mut app, &["quik"]).await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), original);
    assert_eq!(doc.version(), version);
    assert!(doc
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.provider == provider));
    assert!(std::sync::Arc::ptr_eq(
        &dictionary,
        &app.editor.dictionaries[&language]
    ));
    assert!(!dictionary.check("Zorblé"));
    assert_eq!(app.editor.config().spelling, config.spelling);
    assert!(personal_before == fs::read(&personal_path).ok());

    // An incremental edit still ignores exact words, but not words with additional suffixes.
    let end = current_ref!(app.editor).1.text().len_chars() - 1;
    replace(&mut app, end, end, " zorblé zorblés");
    wait_for_mistakes(&mut app, &["quik", "zorblés"]).await?;
    keys(&mut app, ":spelling off<ret>").await?;
    wait_for_mistakes(&mut app, &[]).await?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["quik", "zorblés"]).await?;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    wait_for_mistakes(&mut app, &["quik", "zorblés"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn session_ignore_is_shared_only_by_buffers_using_its_language() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[z|]#orble quik\n")
        .build()?;
    for language in ["session_a", "session_b"] {
        app.editor.dictionaries.insert(
            language.parse()?,
            std::sync::Arc::new(view::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap()),
        );
    }
    keys(&mut app, ":spelling session_a session_b<ret>").await?;
    wait_for_mistakes(&mut app, &["zorble", "quik"]).await?;
    let first = current_ref!(app.editor).1.id();

    keys(
        &mut app,
        ":new<ret>izorble quik<esc>:spelling session_a<ret>",
    )
    .await?;
    wait_for_mistakes(&mut app, &["zorble", "quik"]).await?;
    let second = current_ref!(app.editor).1.id();
    keys(
        &mut app,
        ":new<ret>izorble quik<esc>:spelling session_b<ret>",
    )
    .await?;
    wait_for_mistakes(&mut app, &["zorble", "quik"]).await?;
    let third = current_ref!(app.editor).1.id();

    app.editor.switch(first, view::editor::Action::Replace);
    open_corrections(&mut app).await?;
    choose_correction(&mut app, "Ignore 'zorble' for this session (session_a)").await?;
    wait_for_mistakes(&mut app, &["quik"]).await?;
    // Both buffers using session_a are refreshed, including the one that wasn't focused.
    app.editor.switch(second, view::editor::Action::Replace);
    wait_for_mistakes(&mut app, &["quik"]).await?;
    app.editor.switch(third, view::editor::Action::Replace);
    assert_eq!(mistakes(&app), ["zorble", "quik"]);

    keys(
        &mut app,
        ":new<ret>izorble quik<esc>:spelling session_a<ret>",
    )
    .await?;
    wait_for_mistakes(&mut app, &["quik"]).await?;
    Ok(())
}

async fn app_with_ignore_file(path: &std::path::Path) -> anyhow::Result<Application> {
    let mut app = AppBuilder::new()
        .with_input_text("#[Z|]#orblé ZORBLÉ quik\n")
        .build()?;
    // Install the same state the dictionary loader publishes, with a temporary user ignore file.
    let language: editor_core::SpellingLanguage = "en_US".parse()?;
    app.editor.dictionaries.insert(
        language.clone(),
        std::sync::Arc::new(view::Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap()),
    );
    app.editor
        .handlers
        .spelling
        .ignored_word_files
        .insert(language, IgnoredWordsFile::load(path.to_owned())?);
    keys(&mut app, ":spelling en_US<ret>").await?;
    Ok(app)
}

#[test]
fn forever_ignore_survives_editor_restart() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("spelling/en_US.ignore");

    // Separate runtimes give each editor fresh handlers and event queues, just like a restart.
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut app = app_with_ignore_file(&path).await?;
        wait_for_mistakes(&mut app, &["Zorblé", "ZORBLÉ", "quik"]).await?;
        let version = current_ref!(app.editor).1.version();
        let dictionary = app.editor.dictionaries[&"en_US".parse()?].clone();
        open_corrections(&mut app).await?;
        choose_correction(&mut app, "Ignore 'Zorblé' forever (en_US)").await?;
        wait_for_mistakes(&mut app, &["quik"]).await?;
        assert_eq!(current_ref!(app.editor).1.version(), version);
        assert!(!dictionary.check("Zorblé"));
        assert_eq!(fs::read_to_string(&path)?, "zorblé\n");

        // A session-only ignore for another word must not be saved with the permanent entry.
        keys(&mut app, "]s").await?;
        open_corrections(&mut app).await?;
        choose_correction(&mut app, "Ignore 'quik' for this session (en_US)").await?;
        wait_for_mistakes(&mut app, &[]).await?;
        assert_eq!(fs::read_to_string(&path)?, "zorblé\n");
        anyhow::Ok(())
    })?;

    tokio::runtime::Runtime::new()?.block_on(async {
        let mut app = app_with_ignore_file(&path).await?;
        // Only the persistent entry survives; a fresh editor flags the session-only word again.
        wait_for_mistakes(&mut app, &["quik"]).await?;
        let dictionary = app.editor.dictionaries[&"en_US".parse()?].clone();
        app.editor
            .dictionaries
            .insert("second_language".parse()?, dictionary);
        keys(&mut app, ":spelling second_language<ret>").await?;
        wait_for_mistakes(&mut app, &["Zorblé", "ZORBLÉ", "quik"]).await?;
        keys(&mut app, ":spelling en_US second_language<ret>").await?;
        wait_for_mistakes(&mut app, &["quik"]).await?;
        anyhow::Ok(())
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn forever_ignore_reports_save_failures_without_suppressing_findings() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("en_US.ignore");
    let mut app = app_with_ignore_file(&path).await?;
    wait_for_mistakes(&mut app, &["Zorblé", "ZORBLÉ", "quik"]).await?;
    // Turn the file path into a directory after loading, so the write reliably fails even as root.
    fs::create_dir(&path)?;
    open_corrections(&mut app).await?;
    choose_correction(&mut app, "Ignore 'Zorblé' forever (en_US)").await?;
    assert!(
        app.editor.get_status().is_some_and(
            |(message, _)| message.contains("Could not save spelling ignore for 'en_US'")
        )
    );
    assert_eq!(mistakes(&app), ["Zorblé", "ZORBLÉ", "quik"]);
    // Force a new scan to catch an accidental in-memory ignore on the failed save path.
    let doc_id = current_ref!(app.editor).1.id();
    app.editor.refresh_spelling(doc_id);
    wait_for_mistakes(&mut app, &["Zorblé", "ZORBLÉ", "quik"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_dictionaries_report_an_error_and_bad_names_preserve_settings() -> anyhow::Result<()>
{
    let mut app = AppBuilder::new().with_input_text("#[t|]#eh\n").build()?;
    keys(&mut app, ":spelling mitos_nonexistent_dictionary<ret>").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !app
            .editor
            .get_status()
            .is_some_and(|(message, _)| message.contains("Could not load spelling dictionary"))
        {
            app.editor.reset_idle_timer();
            run_event_loop_until_idle(&mut app).await;
        }
    })
    .await?;
    keys(&mut app, ":spelling ../../invalid<ret>").await?;
    assert_eq!(
        current_ref!(app.editor).1.spelling_languages[0].as_str(),
        "mitos_nonexistent_dictionary"
    );
    assert!(mistakes(&app).is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spelling_events_and_completions_stay_with_their_editor() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.spelling.languages = Some(vec!["en_US".parse()?]);
    let mut first = AppBuilder::new()
        .with_config(config.clone())
        .with_input_text("#[t|]#eh hello\n")
        .build()?;
    let mut second = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[q|]#uik world\n")
        .build()?;
    // Both dictionary loading and initial checks must return to their own editor,
    // even when document IDs collide and the global queue selects another app.
    assert_eq!(
        current_ref!(first.editor).1.id(),
        current_ref!(second.editor).1.id()
    );
    wait_for_mistakes(&mut first, &["teh"]).await?;
    wait_for_mistakes(&mut second, &["quik"]).await?;

    // Document change hooks must also route edits to the correct debounce worker.
    replace(&mut first, 0, 3, "quik");
    replace(&mut second, 0, 4, "teh");
    wait_for_mistakes(&mut first, &["quik"]).await?;
    wait_for_mistakes(&mut second, &["teh"]).await?;
    assert_eq!(mistakes(&first), ["quik"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scratch_buffers_follow_config_and_feed_diagnostics_quicklists() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.spelling.languages = Some(vec!["en_US".parse()?]);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text("#[t|]#eh quik\n")
        .build()?;
    wait_for_mistakes(&mut app, &["teh", "quik"]).await?;
    keys(&mut app, "<space>d").await?;
    keys(&mut app, "<C-q><esc>").await?;
    assert_eq!(app.editor.quicklist.entries().len(), 2);
    let id = current_ref!(app.editor).1.id();
    assert!(app
        .editor
        .quicklist
        .entries()
        .iter()
        .all(|e| e.target == QuicklistTarget::Document(id)));

    let mut config = (*app.editor.config()).clone();
    config.spelling.words = vec!["TEH".into()];
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config.clone())));
    wait_for_mistakes(&mut app, &["quik"]).await?;
    config.spelling.languages = Some(Vec::new());
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    wait_for_mistakes(&mut app, &[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editorconfig_and_manual_override_take_precedence() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fs::write(
        dir.path().join(".editorconfig"),
        "root = true\n[*]\nspelling_language = en_US\n",
    )?;
    let path = dir.path().join("test.txt");
    fs::write(&path, "teh\n")?;
    let mut config = test_config();
    config.editor.spelling.languages = Some(vec!["missing_dictionary".parse()?]);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(path, None)
        .build()?;
    wait_for_mistakes(&mut app, &["teh"]).await?;
    assert_eq!(
        current_ref!(app.editor).1.spelling_languages[0].as_str(),
        "en_US"
    );
    let mut config = (*app.editor.config()).clone();
    config.spelling.languages = Some(Vec::new());
    config.editor_config = false;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config.clone())));
    wait_for_mistakes(&mut app, &[]).await?;
    config.editor_config = true;
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    wait_for_mistakes(&mut app, &["teh"]).await?;
    keys(&mut app, ":spelling off<ret>").await?;
    let config = (*app.editor.config()).clone();
    app.handle_config_events(view::editor::ConfigEvent::Update(Box::new(config)));
    wait_for_mistakes(&mut app, &[]).await?;
    assert!(current_ref!(app.editor).1.spelling_languages.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn incremental_edits_keep_distant_diagnostics_and_whole_tokens() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.spelling.languages = Some(vec!["en_US".parse()?]);
    let text = format!(
        "#[t|]#he {} quik {} teh\n",
        "hello ".repeat(12),
        "world ".repeat(12)
    );
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(text)
        .build()?;
    wait_for_mistakes(&mut app, &["quik", "teh"]).await?;
    replace(&mut app, 0, 3, "teh");
    wait_for_mistakes(&mut app, &["teh", "quik", "teh"]).await?;
    replace(&mut app, 0, 3, "the");
    wait_for_mistakes(&mut app, &["quik", "teh"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn coalesced_edits_restore_diagnostics_even_when_text_is_unchanged() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[t|]#eh quik\n")
        .build()?;
    keys(&mut app, ":spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["teh", "quik"]).await?;
    // Deleting a diagnostic's range removes it immediately. Restoring the text before the
    // debounce expires must restore the diagnostic too, despite the empty net text diff.
    replace(&mut app, 0, 3, "");
    replace(&mut app, 0, 0, "teh");
    assert_eq!(mistakes(&app), ["quik"]);
    wait_for_mistakes(&mut app, &["teh", "quik"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn python_docstrings_recheck_when_their_statement_position_changes() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_input_text("#[\"|]#\"\"teh hello\"\"\"\nvalue = \"quik\"\n")
        .build()?;
    keys(&mut app, ":lang python<ret>:spelling en_US<ret>").await?;
    wait_for_mistakes(&mut app, &["teh"]).await?;

    // The same string becomes an ordinary expression once another statement precedes it.
    replace(&mut app, 0, 0, "pass\n");
    wait_for_mistakes(&mut app, &[]).await?;
    replace(&mut app, 0, 5, "");
    wait_for_mistakes(&mut app, &["teh"]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_changes_recheck_prose_beyond_the_edit_window() -> anyhow::Result<()> {
    let mut config = test_config();
    config.editor.spelling.languages = Some(vec!["en_US".parse()?]);
    let text = format!("#[/|]#* {} teh */\n", "hello ".repeat(30));
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(text)
        .build()?;
    keys(&mut app, ":lang rust<ret>").await?;
    wait_for_mistakes(&mut app, &["teh"]).await?;
    replace(&mut app, 0, 2, "  ");
    wait_for_mistakes(&mut app, &[]).await?;
    replace(&mut app, 0, 2, "/*");
    wait_for_mistakes(&mut app, &["teh"]).await?;
    let (view, doc) = current!(app.editor);
    doc.set_selection(view.id, Selection::point(0));
    keys(&mut app, ":lang text<ret>").await?;
    wait_for_mistakes(&mut app, &["teh"]).await?;
    Ok(())
}

use std::{path::Path, process::Command, time::Duration};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use view::{
    config::InlineBlameShow, current, current_ref, document::LineBlameError, editor::Action,
};

use super::Fixture;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(f: &Fixture, title: &str) {
    git(f.dir.path(), &["add", "-A"]);
    git(f.dir.path(), &["commit", "-m", title]);
}

fn prepare(f: &mut Fixture, title: &str) {
    git(f.dir.path(), &["init"]);
    commit(f, title);
    current!(f.editor)
        .1
        .refresh_vcs(&f.editor.diff_providers, true);
}

async fn wait_for_blame(f: &mut Fixture) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(f.editor).1.file_blame().is_none() {
            let callback = f.callbacks.recv().await.context("callback queue closed")?;
            callback(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await
    .context("blame did not reach its editor")??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_blame_uses_each_editors_callback_and_maps_unsaved_lines() -> anyhow::Result<()> {
    let mut a = Fixture::new("first\nsecond\n")?;
    let mut b = Fixture::new("other\n")?;
    prepare(&mut a, "first editor");
    prepare(&mut b, "second editor");
    let id_a = current_ref!(a.editor).1.id();
    let id_b = current_ref!(b.editor).1.id();
    assert!(current_ref!(a.editor).1.file_blame().is_none());
    a.editor.blame_line(id_a, 0);
    b.editor.blame_line(id_b, 0);
    wait_for_blame(&mut a).await?;
    assert!(current_ref!(b.editor).1.file_blame().is_none());
    wait_for_blame(&mut b).await?;
    assert_eq!(
        current_ref!(a.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "first editor"
    );
    assert_eq!(
        current_ref!(b.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "second editor"
    );

    let (view, doc) = current!(a.editor);
    let change = Transaction::change(doc.text(), [(0, 0, Some("inserted\n".into()))].into_iter())
        .with_selection(Selection::point(0));
    assert!(doc.apply(&change, view.id));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let doc = current_ref!(a.editor).1;
            if doc.diff_handle().unwrap().load().doc() == doc.text() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let doc = current_ref!(a.editor).1;
    assert!(matches!(
        doc.line_blame(0, "{title}"),
        Err(LineBlameError::NotCommittedYet)
    ));
    assert_eq!(
        doc.line_blame(1, "{title}").unwrap().trim_end(),
        "first editor"
    );
    assert_eq!(
        doc.line_blame(2, "{title}").unwrap().trim_end(),
        "first editor"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn visible_blame_refreshes_after_head_changes_and_file_opens() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    prepare(&mut f, "before commit");
    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "before commit"
    );
    let path = current_ref!(f.editor).1.path().unwrap().to_path_buf();
    std::fs::write(&path, "after\n")?;
    commit(&f, "after commit");
    let (view, doc) = current!(f.editor);
    doc.reload(view, &f.editor.diff_providers, true)?;
    assert!(doc.file_blame().is_none());
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "after commit"
    );

    // Files opened while annotations are visible fetch blame through the shared hooks.
    let another = f.dir.path().join("another.words");
    std::fs::write(&another, "another\n")?;
    commit(&f, "new file");
    f.editor.open(&another, Action::Replace)?;
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "new file"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_blame_is_discarded_after_a_path_change() -> anyhow::Result<()> {
    let (sender, mut callbacks) = crate::support::callback_channel();
    let mut f = Fixture::with_handler_setup(
        "before\n",
        "language = []",
        loader::syntax::Resources::default(),
        |_| {},
        move |handlers, _| handlers.blame = view::handlers::blame::BlameHandler::new(sender),
    )?;
    prepare(&mut f, "before commit");
    let doc_id = current_ref!(f.editor).1.id();
    f.editor.blame_line(doc_id, 0);
    let callback = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .context("blame callback missing")?;
    let another = f.dir.path().join("another.words");
    std::fs::write(&another, "another\n")?;
    f.editor.set_doc_path(doc_id, &another);
    callback(&mut f.editor);
    assert!(current_ref!(f.editor).1.file_blame().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_requests_share_pending_work_and_display_the_latest_line() -> anyhow::Result<()> {
    let (sender, mut callbacks) = crate::support::callback_channel();
    let mut f = Fixture::with_handler_setup(
        "first\nsecond\n",
        "language = []",
        loader::syntax::Resources::default(),
        |config| config.inline_blame.format = "{title}".into(),
        move |handlers, _| handlers.blame = view::handlers::blame::BlameHandler::new(sender),
    )?;
    prepare(&mut f, "initial");
    let doc_id = current_ref!(f.editor).1.id();
    f.editor.blame_line(doc_id, 0);
    // Keep the result pending on the editor queue while more requests arrive.
    let callback = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .context("blame callback missing")?;
    for _ in 0..100 {
        f.editor.blame_line(doc_id, 0);
    }
    // The trailing empty line has no committed blame.
    f.editor.blame_line(doc_id, 2);
    callback(&mut f.editor);
    assert_eq!(
        f.editor.get_status().unwrap().0.as_ref(),
        "Not committed yet"
    );
    assert!(current_ref!(f.editor).1.file_blame().is_some());
    // Visibility toggles reuse the cached result.
    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    f.configure(|config| config.inline_blame.show = InlineBlameShow::Never);
    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), callbacks.recv())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_reuses_blame_when_head_is_unchanged() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    prepare(&mut f, "initial");
    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    wait_for_blame(&mut f).await?;
    let original = current_ref!(f.editor)
        .1
        .file_blame()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let path = current_ref!(f.editor).1.path().unwrap().to_path_buf();
    std::fs::write(&path, "inserted\nbefore\n")?;
    let (view, doc) = current!(f.editor);
    doc.reload(view, &f.editor.diff_providers, true)?;
    assert!(matches!(
        doc.line_blame(0, "{title}"),
        Err(LineBlameError::NotReadyYet)
    ));
    wait_for_blame(&mut f).await?;
    let doc = current_ref!(f.editor).1;
    let refreshed = doc.file_blame().unwrap().as_ref().unwrap();
    assert!(std::sync::Arc::ptr_eq(&original, refreshed));
    // Wait for the independent diff worker before testing the updated line mapping.
    tokio::time::timeout(Duration::from_secs(5), async {
        while current_ref!(f.editor).1.diff_handle().unwrap().load().doc()
            != current_ref!(f.editor).1.text()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    assert_eq!(
        current_ref!(f.editor).1.line_blame(1, "{title}").unwrap(),
        "initial"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hiding_blame_discards_debounced_repository_refreshes() -> anyhow::Result<()> {
    let (sender, mut callbacks) = crate::support::callback_channel();
    let mut f = Fixture::with_handler_setup(
        "before\n",
        "language = []",
        loader::syntax::Resources::default(),
        |_| {},
        move |handlers, _| handlers.blame = view::handlers::blame::BlameHandler::new(sender),
    )?;
    prepare(&mut f, "initial");
    let id = current_ref!(f.editor).1.id();
    f.editor.blame_line(id, 0);
    let callback = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .context("blame missing")?;
    callback(&mut f.editor);
    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    // A burst of refreshes becomes a single hook completion.
    for _ in 0..20 {
        current!(f.editor)
            .1
            .refresh_vcs(&f.editor.diff_providers, true);
    }
    let refresh = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .context("refresh missing")?;
    f.configure(|config| config.inline_blame.show = InlineBlameShow::Never);
    refresh(&mut f.editor);
    assert!(current_ref!(f.editor).1.file_blame().is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), callbacks.recv())
            .await
            .is_err()
    );
    // Explicit requests still work while annotations are hidden.
    f.editor.blame_line(id, 0);
    let callback = tokio::time::timeout(Duration::from_secs(10), callbacks.recv())
        .await?
        .context("manual blame missing")?;
    callback(&mut f.editor);
    assert!(current_ref!(f.editor).1.file_blame().is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn visibility_fetches_splits_and_buffer_switches_without_prefetching_hidden_files(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("first\n")?;
    prepare(&mut f, "initial");
    let first = current_ref!(f.editor).1.id();
    let second_path = f.dir.path().join("second.words");
    let hidden_path = f.dir.path().join("hidden.words");
    std::fs::write(&second_path, "second\n")?;
    std::fs::write(&hidden_path, "hidden\n")?;
    commit(&f, "more files");
    let second = f.editor.open(&second_path, Action::VerticalSplit)?;
    let hidden = f.editor.open(&hidden_path, Action::Load)?;
    for id in [first, second, hidden] {
        assert!(f.editor.document(id).unwrap().file_blame().is_none());
    }

    f.configure(|config| config.inline_blame.show = InlineBlameShow::CursorLine);
    tokio::time::timeout(Duration::from_secs(10), async {
        while [first, second]
            .into_iter()
            .any(|id| f.editor.document(id).unwrap().file_blame().is_none())
        {
            let callback = f.callbacks.recv().await.context("blame callback missing")?;
            callback(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(f.editor.document(hidden).unwrap().file_blame().is_none());

    f.editor.switch(hidden, Action::Replace);
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor).1.line_blame(0, "{title}").unwrap(),
        "more files"
    );

    // Opening a file in the background while blame is enabled still skips it.
    let background_path = f.dir.path().join("background.words");
    std::fs::write(&background_path, "background\n")?;
    commit(&f, "background file");
    let background = f.editor.open(&background_path, Action::Load)?;
    let callback = tokio::time::timeout(Duration::from_secs(10), f.callbacks.recv())
        .await?
        .context("open visibility check missing")?;
    callback(&mut f.editor);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), f.callbacks.recv())
            .await
            .is_err()
    );
    assert!(f
        .editor
        .document(background)
        .unwrap()
        .file_blame()
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn configured_visible_blame_fetches_without_a_manual_toggle() -> anyhow::Result<()> {
    let mut f = Fixture::with_config(
        "committed\n",
        "language = []",
        loader::syntax::Resources::default(),
        |config| config.inline_blame.show = InlineBlameShow::CursorLine,
    )?;
    prepare(&mut f, "initial");
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor).1.line_blame(0, "{title}").unwrap(),
        "initial"
    );
    Ok(())
}

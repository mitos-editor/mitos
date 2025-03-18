use std::{path::Path, process::Command, time::Duration};

use anyhow::Context as _;
use editor_core::{Selection, Transaction};
use view::{current, current_ref, document::LineBlameError, editor::Action};

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
        while current_ref!(f.editor).1.file_blame.is_none() {
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
    assert!(current_ref!(a.editor).1.file_blame.is_none());
    a.editor.blame_line(id_a, 0);
    b.editor.blame_line(id_b, 0);
    wait_for_blame(&mut a).await?;
    assert!(current_ref!(b.editor).1.file_blame.is_none());
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
async fn auto_fetch_refreshes_open_documents_after_head_changes() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    prepare(&mut f, "before commit");
    f.configure(|config| config.inline_blame.auto_fetch = true);
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
    assert!(doc.file_blame.is_none());
    wait_for_blame(&mut f).await?;
    assert_eq!(
        current_ref!(f.editor)
            .1
            .line_blame(0, "{title}")
            .unwrap()
            .trim_end(),
        "after commit"
    );

    // Documents opened after enabling auto-fetch are handled by the shared open hook.
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
    assert!(current_ref!(f.editor).1.file_blame.is_none());
    Ok(())
}

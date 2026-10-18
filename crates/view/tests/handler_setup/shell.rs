use std::{path::Path, time::Duration};

use anyhow::Context as _;
use editor_core::{Range, Selection};
use view::{
    callbacks::EditorCallback, current, current_ref, editor::Action, features::shell::ShellBehavior,
};

use super::Fixture;

fn fixture(text: &str) -> anyhow::Result<Fixture> {
    Fixture::with_config(
        text,
        "language = []",
        loader::syntax::Resources::default(),
        |config| {
            config.completion.word.enable = false;
            config.shell = vec!["sh".into(), "-c".into()];
        },
    )
}

fn request(fixture: &mut Fixture, command: &str, behavior: ShellBehavior) {
    let view = current_ref!(fixture.editor).0.id;
    fixture.editor.request_shell_filter(view, command, behavior);
}

async fn next(fixture: &mut Fixture) -> anyhow::Result<EditorCallback> {
    tokio::time::timeout(Duration::from_secs(5), fixture.callbacks.recv())
        .await?
        .context("shell callback queue closed")
}

async fn publish(fixture: &mut Fixture) -> anyhow::Result<()> {
    next(fixture).await?(&mut fixture.editor);
    Ok(())
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn piping_preserves_selection_direction_primary_and_one_undo_group() -> anyhow::Result<()> {
    let mut f = fixture("éa bb\n")?;
    let (view, doc) = current!(f.editor);
    let selection = Selection::new(vec![Range::new(2, 0), Range::new(3, 5)].into(), 1);
    doc.set_selection(view.id, selection.clone());
    request(&mut f, "cat; printf '!'", ShellBehavior::Replace);
    publish(&mut f).await?;
    let (view, doc) = current!(f.editor);
    assert_eq!(doc.text().to_string(), "éa! bb!\n");
    assert_eq!(
        doc.selection(view.id),
        &Selection::new(vec![Range::new(3, 0), Range::new(4, 7)].into(), 1)
    );
    assert!(view::editing::undo(doc, view));
    assert_eq!(doc.text().to_string(), "éa bb\n");
    assert_eq!(doc.selection(view.id), &selection);
    assert!(!view::editing::undo(doc, view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inserting_and_appending_reuse_one_output_and_trim_the_final_line_ending(
) -> anyhow::Result<()> {
    for (behavior, expected, expected_selection) in [
        (
            ShellBehavior::Insert,
            "outaa outbb\n",
            Selection::new(vec![Range::new(0, 3), Range::new(6, 9)].into(), 0),
        ),
        (
            ShellBehavior::Append,
            "aaout bbout\n",
            Selection::new(vec![Range::new(2, 5), Range::new(8, 11)].into(), 0),
        ),
    ] {
        let mut f = fixture("aa bb\n")?;
        let (view, doc) = current!(f.editor);
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::new(0, 2), Range::new(3, 5)].into(), 0),
        );
        let counter = f.dir.path().join("runs");
        request(
            &mut f,
            &format!("printf x >> {}; printf 'out\\r\\n'", quoted(&counter)),
            behavior,
        );
        publish(&mut f).await?;
        let (view, doc) = current_ref!(f.editor);
        assert_eq!(doc.text().to_string(), expected);
        assert_eq!(doc.selection(view.id), &expected_selection);
        assert_eq!(std::fs::read_to_string(&counter)?, "x");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn keeping_selections_uses_each_fragment_and_retains_the_nearest_primary(
) -> anyhow::Result<()> {
    let mut f = fixture("keep drop keep\n")?;
    let (view, doc) = current!(f.editor);
    doc.set_selection(
        view.id,
        Selection::new(
            vec![Range::new(0, 4), Range::new(5, 9), Range::new(10, 14)].into(),
            1,
        ),
    );
    request(&mut f, "grep -q '^keep$'", ShellBehavior::Keep);
    publish(&mut f).await?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text().to_string(), "keep drop keep\n");
    assert_eq!(
        doc.selection(view.id),
        &Selection::new(vec![Range::new(0, 4), Range::new(10, 14)].into(), 1)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn piping_without_replacement_writes_each_fragment_without_editing() -> anyhow::Result<()> {
    let mut f = fixture("aa bb\n")?;
    let (view, doc) = current!(f.editor);
    let selection = Selection::new(vec![Range::new(0, 2), Range::new(3, 5)].into(), 1);
    doc.set_selection(view.id, selection.clone());
    let output = f.dir.path().join("output");
    request(
        &mut f,
        &format!("cat >> {}", quoted(&output)),
        ShellBehavior::Ignore,
    );
    publish(&mut f).await?;
    let (view, doc) = current!(f.editor);
    assert_eq!(std::fs::read_to_string(output)?, "aabb");
    assert_eq!(doc.text().to_string(), "aa bb\n");
    assert_eq!(doc.selection(view.id), &selection);
    assert!(!view::editing::undo(doc, view));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delayed_shell_leaves_the_editor_available() -> anyhow::Result<()> {
    let mut f = fixture("before\n")?;
    let release = f.dir.path().join("release");
    request(
        &mut f,
        &format!(
            "while [ ! -f {} ]; do sleep 0.01; done; printf after",
            quoted(&release)
        ),
        ShellBehavior::Replace,
    );
    f.editor.set_status("editor is available");
    assert_eq!(current_ref!(f.editor).1.text().to_string(), "before\n");
    std::fs::write(&release, "")?;
    publish(&mut f).await?;
    assert_eq!(current_ref!(f.editor).1.text().to_string(), "afterefore\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_filter_results_require_the_original_text_and_selection() -> anyhow::Result<()> {
    for change_text in [true, false] {
        let mut f = fixture("before\n")?;
        request(&mut f, "printf output", ShellBehavior::Replace);
        let callback = next(&mut f).await?;
        if change_text {
            f.replace("new typing\n");
        } else {
            let (view, doc) = current!(f.editor);
            doc.set_selection(view.id, Selection::point(2));
            // Returning to the old selection must not revive a canceled operation.
            doc.set_selection(view.id, Selection::point(0));
        }
        let expected = current_ref!(f.editor).1.text().clone();
        callback(&mut f.editor);
        assert_eq!(current_ref!(f.editor).1.text(), &expected);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_filter_results_do_not_follow_rebound_or_closed_views() -> anyhow::Result<()> {
    for close_view in [true, false] {
        let mut f = fixture("before\n")?;
        let (view, doc) = current_ref!(f.editor);
        let (origin, document) = (view.id, doc.id());
        request(&mut f, "printf output", ShellBehavior::Replace);
        let callback = next(&mut f).await?;
        if close_view {
            f.editor.switch(document, Action::VerticalSplit);
            f.editor.close(origin);
        } else {
            f.editor.new_file(Action::Replace);
            f.editor.switch(document, Action::Replace);
        }
        callback(&mut f.editor);
        assert_eq!(
            f.editor.document(document).unwrap().text().to_string(),
            "before\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_filter_replaces_queued_results_and_errors() -> anyhow::Result<()> {
    for first in ["printf obsolete", "exit 1"] {
        let mut f = fixture("before\n")?;
        request(&mut f, first, ShellBehavior::Replace);
        let old = next(&mut f).await?;
        request(&mut f, "printf current", ShellBehavior::Replace);
        old(&mut f.editor);
        assert!(f.editor.get_status().is_none());
        publish(&mut f).await?;
        assert_eq!(
            current_ref!(f.editor).1.text().to_string(),
            "currentefore\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn filters_finish_in_the_originating_unfocused_split() -> anyhow::Result<()> {
    let mut f = fixture("before\n")?;
    let source = current_ref!(f.editor).1.id();
    request(&mut f, "printf output", ShellBehavior::Replace);
    let callback = next(&mut f).await?;
    let other = f.editor.new_file(Action::VerticalSplit);
    let other_text = f.editor.document(other).unwrap().text().clone();
    callback(&mut f.editor);
    assert_eq!(
        f.editor.document(source).unwrap().text().to_string(),
        "outputefore\n"
    );
    assert_eq!(f.editor.document(other).unwrap().text(), &other_text);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_or_reloading_a_document_discards_queued_filters() -> anyhow::Result<()> {
    for rename in [true, false] {
        let mut f = fixture("before\n")?;
        let document = current_ref!(f.editor).1.id();
        request(&mut f, "printf output", ShellBehavior::Replace);
        let callback = next(&mut f).await?;
        if rename {
            let original = f
                .editor
                .document(document)
                .unwrap()
                .path()
                .unwrap()
                .to_owned();
            f.editor
                .set_doc_path(document, &f.dir.path().join("renamed.words"));
            f.editor.set_doc_path(document, &original);
        } else {
            let (_, doc) = current!(f.editor);
            // Even an unchanged reload ends the operation's editing context.
            doc.reload(&f.editor.diff_providers, true)?;
        }
        callback(&mut f.editor);
        assert_eq!(
            f.editor.document(document).unwrap().text().to_string(),
            "before\n"
        );
        assert!(!f.editor.has_pending_shell_filters());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_filter_results_cannot_edit_another_editor() -> anyhow::Result<()> {
    let mut first = fixture("before\n")?;
    let mut second = fixture("before\n")?;
    let second_id = current_ref!(second.editor).1.id();
    second
        .editor
        .set_doc_path(second_id, current_ref!(first.editor).1.path().unwrap());
    request(&mut first, "printf output", ShellBehavior::Replace);
    next(&mut first).await?(&mut second.editor);
    assert_eq!(current_ref!(second.editor).1.text().to_string(), "before\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_changes_and_document_closure_stop_running_shells() -> anyhow::Result<()> {
    for close in [false, true] {
        let mut f = fixture("before\n")?;
        let pid_file = f.dir.path().join("pid");
        request(
            &mut f,
            &format!("printf '%s' $$ > {}; exec sleep 60", quoted(&pid_file)),
            ShellBehavior::Replace,
        );
        wait_until(|| {
            pid_file.exists() && std::fs::read_to_string(&pid_file).is_ok_and(|s| !s.is_empty())
        })
        .await?;
        let pid = std::fs::read_to_string(&pid_file)?.parse::<i32>()?;
        if close {
            f.close_current()?;
        } else {
            let (view, doc) = current!(f.editor);
            doc.set_selection(view.id, Selection::point(1));
        }
        wait_until(|| unsafe { libc::kill(pid, 0) } != 0).await?;
    }
    Ok(())
}

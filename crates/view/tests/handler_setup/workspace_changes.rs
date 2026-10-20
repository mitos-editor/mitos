use std::path::Path;

use super::Fixture;
use view::{
    editor::Action,
    features::workspace_changes::{Documents, Policy, WorkspaceChanges},
    file_watcher::{CanonicalPathBuf, Event, EventType, Events, Watcher},
};

fn deliver(editor: &mut view::Editor, path: &Path, ty: EventType) {
    editor.handle_file_events(&Events::from(vec![Event {
        path: CanonicalPathBuf::assert_canonicalized(path),
        ty,
    }]));
}

#[tokio::test]
async fn file_membership_content_and_ignore_changes_follow_consumer_policy() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let parent = f.dir.path().canonicalize()?;
    let root = parent.join("workspace");
    std::fs::create_dir(&root)?;
    let file = root.join("file.rs");
    let mut membership = WorkspaceChanges::new(&f.editor, &root, Policy::default());
    let mut contents = WorkspaceChanges::new(
        &f.editor,
        &root,
        Policy {
            contents: true,
            ignore_files: true,
            ..Policy::default()
        },
    );
    deliver(&mut f.editor, &file, EventType::Modified);
    assert!(!membership.changed(&f.editor, &root));
    assert!(contents.changed(&f.editor, &root));
    deliver(&mut f.editor, &file, EventType::Create);
    assert!(membership.changed(&f.editor, &root));
    assert!(contents.changed(&f.editor, &root));
    deliver(&mut f.editor, &file, EventType::Tempfile);
    assert!(!membership.changed(&f.editor, &root));
    assert!(!contents.changed(&f.editor, &root));
    deliver(&mut f.editor, &parent.join("other.rs"), EventType::Create);
    assert!(!contents.changed(&f.editor, &root));
    deliver(
        &mut f.editor,
        &parent.join(".gitignore"),
        EventType::Modified,
    );
    assert!(!membership.changed(&f.editor, &root));
    assert!(contents.changed(&f.editor, &root));
    Ok(())
}

#[tokio::test]
async fn document_scope_includes_unsaved_edits_and_scratch_buffers() -> anyhow::Result<()> {
    let mut f = Fixture::new("original")?;
    let root = f.dir.path().canonicalize()?;
    let mut workspace = WorkspaceChanges::new(
        &f.editor,
        &root,
        Policy {
            documents: Documents::Workspace,
            ..Policy::default()
        },
    );
    let mut all = WorkspaceChanges::new(
        &f.editor,
        &root,
        Policy {
            documents: Documents::All,
            ..Policy::default()
        },
    );
    f.replace("unsaved");
    assert!(workspace.changed(&f.editor, &root));
    assert!(all.changed(&f.editor, &root));
    assert!(!workspace.changed(&f.editor, &root));
    f.editor.new_file(Action::Replace);
    assert!(!workspace.changed(&f.editor, &root));
    assert!(all.changed(&f.editor, &root));
    f.replace("scratch");
    assert!(!workspace.changed(&f.editor, &root));
    assert!(all.changed(&f.editor, &root));
    Ok(())
}

#[tokio::test]
async fn event_loss_and_watcher_replacement_invalidate_and_resume_delivery() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let other = Fixture::new("")?;
    let root = f.dir.path().canonicalize()?;
    let file = root.join("new.rs");
    let mut changes = WorkspaceChanges::new(&f.editor, &root, Policy::default());
    deliver(&mut f.editor, &file, EventType::Create);
    assert!(!changes.changed(&other.editor, &root));
    assert!(changes.changed(&f.editor, &root));
    // Lost batches could have contained a relevant event even if those remaining do not.
    let outside = other.dir.path().canonicalize()?.join("other.rs");
    for _ in 0..100 {
        deliver(&mut f.editor, &outside, EventType::Modified);
    }
    assert!(changes.changed(&f.editor, &root));
    assert!(!changes.changed(&f.editor, &root));
    f.editor.file_watcher = Watcher::new(
        &f.editor.config().files.watch,
        f.editor.callback_sender().clone(),
    );
    assert!(changes.changed(&f.editor, &root));
    assert!(!changes.changed(&f.editor, &root));
    deliver(&mut f.editor, &file, EventType::Create);
    assert!(changes.changed(&f.editor, &root));
    Ok(())
}

#[tokio::test]
async fn polling_respects_configuration_and_refresh_acknowledgement() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    let root = f.dir.path().canonicalize()?;
    f.configure(|config| {
        config.files.reload.poll.enable = true;
        config.files.reload.poll.interval = 100;
    });
    let mut changes = WorkspaceChanges::new(&f.editor, &root, Policy::default());
    assert!(!changes.poll_due(&f.editor));
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(changes.poll_due(&f.editor));
    changes.mark_refreshed();
    assert!(!changes.poll_due(&f.editor));
    f.configure(|config| config.files.reload.poll.enable = false);
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(!changes.poll_due(&f.editor));
    Ok(())
}

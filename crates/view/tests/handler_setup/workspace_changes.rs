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

#[cfg(unix)]
#[tokio::test]
async fn cached_paths_follow_symlink_replacement_events() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new("")?;
    let parent = f.dir.path().canonicalize()?;
    let inside = parent.join("inside");
    let outside = parent.join("outside");
    std::fs::create_dir(&inside)?;
    std::fs::create_dir(&outside)?;
    std::fs::write(inside.join("test.words"), "inside")?;
    std::fs::write(outside.join("test.words"), "outside")?;
    let alias = parent.join("alias");
    symlink(&inside, &alias)?;
    f.editor.open(&alias.join("test.words"), Action::Replace)?;
    let mut changes = WorkspaceChanges::new(
        &f.editor,
        &inside,
        Policy {
            documents: Documents::Workspace,
            ..Policy::default()
        },
    );
    assert!(!changes.changed(&f.editor, &inside));
    std::fs::remove_file(&alias)?;
    symlink(&outside, &alias)?;
    deliver(&mut f.editor, &alias, EventType::Create);
    assert!(changes.changed(&f.editor, &inside));
    assert!(!changes.changed(&f.editor, &inside));
    Ok(())
}

#[tokio::test]
async fn cached_paths_follow_renames_and_closed_documents() -> anyhow::Result<()> {
    let mut f = Fixture::new("original")?;
    let root = f.dir.path().canonicalize()?;
    let mut changes = WorkspaceChanges::new(
        &f.editor,
        &root,
        Policy {
            documents: Documents::Workspace,
            ..Policy::default()
        },
    );
    let id = view::current_ref!(f.editor).1.id();
    let renamed = root.join("renamed.words");
    let source = f.editor.document(id).unwrap().path().unwrap().to_owned();
    f.editor
        .request_file_operation(view::editor::FileOperation::Move {
            from: source,
            to: renamed,
        })?;
    assert!(changes.changed(&f.editor, &root));
    assert!(!changes.changed(&f.editor, &root));
    assert!(f.editor.close_document(id, true).is_ok());
    assert!(changes.changed(&f.editor, &root));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn polling_refreshes_cached_symlink_paths_without_events() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new("")?;
    f.configure(|config| {
        config.files.reload.poll.enable = true;
        config.files.reload.poll.interval = 100;
    });
    let parent = f.dir.path().canonicalize()?;
    let inside = parent.join("inside");
    let outside = parent.join("outside");
    std::fs::create_dir(&inside)?;
    std::fs::create_dir(&outside)?;
    std::fs::write(inside.join("test.words"), "inside")?;
    std::fs::write(outside.join("test.words"), "outside")?;
    let alias = parent.join("alias");
    symlink(&inside, &alias)?;
    f.editor.open(&alias.join("test.words"), Action::Replace)?;
    let mut changes = WorkspaceChanges::new(
        &f.editor,
        &inside,
        Policy {
            documents: Documents::Workspace,
            ..Policy::default()
        },
    );
    assert!(!changes.changed(&f.editor, &inside));
    std::fs::remove_file(&alias)?;
    symlink(&outside, &alias)?;
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(changes.changed(&f.editor, &inside));
    assert!(!changes.changed(&f.editor, &inside));
    Ok(())
}

#[tokio::test]
#[ignore = "manual release-mode performance probe"]
async fn unchanged_workspace_sampling_performance() -> anyhow::Result<()> {
    let mut f = Fixture::new("")?;
    for i in 0..1000 {
        let path = f.dir.path().join(format!("buffer-{i}.txt"));
        std::fs::write(&path, "text")?;
        f.editor.open(&path, Action::Load)?;
    }
    let root = f.dir.path().canonicalize()?;
    let mut changes = WorkspaceChanges::new(
        &f.editor,
        &root,
        Policy {
            documents: Documents::Workspace,
            ..Policy::default()
        },
    );
    let mut times = Vec::new();
    for _ in 0..101 {
        let start = std::time::Instant::now();
        assert!(!std::hint::black_box(changes.changed(&f.editor, &root)));
        times.push(start.elapsed());
    }
    times.sort_unstable();
    eprintln!("1000 buffers, unchanged sample: median {:?}", times[50]);
    Ok(())
}

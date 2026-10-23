use std::{path::PathBuf, time::Duration};

use editor_core::Selection;
use term::{
    application::Application,
    config::{Config, ConfigLoadError},
};
use ui_core::input::parse_macro;
use view::{
    current_ref,
    editor::Action,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
};

use super::helpers::{run_event_loop_until_idle, AppBuilder};

struct Fixture {
    app: Application,
    _dir: tempfile::TempDir,
    paths: Vec<PathBuf>,
}

impl Fixture {
    fn new(names: &[&str]) -> anyhow::Result<Self> {
        Self::with_keys(names, "")
    }

    fn with_keys(names: &[&str], keys: &str) -> anyhow::Result<Self> {
        let mut config = super::helpers::test_config();
        let configured =
            Config::load(Ok(&keys.to_owned()), Err(ConfigLoadError::default())).unwrap();
        config.keys = configured.keys;
        config.component_keys = configured.component_keys;
        let mut app = AppBuilder::new().with_config(config).build()?;
        let dir = tempfile::tempdir()?;
        let paths = names
            .iter()
            .map(|name| dir.path().join(name))
            .collect::<Vec<_>>();
        for path in &paths {
            std::fs::write(path, "alpha\nbeta\ngamma\n")?;
        }
        let paths = paths
            .into_iter()
            .map(stdx::path::canonicalize)
            .collect::<Vec<_>>();
        app.editor.replace_quicklist(
            paths
                .iter()
                .map(|path| QuicklistEntry {
                    target: QuicklistTarget::Path(path.clone()),
                    position: QuicklistPosition::LineRange { start: 0, end: 0 },
                })
                .collect(),
        );
        Ok(Self {
            app,
            _dir: dir,
            paths,
        })
    }

    async fn keys(&mut self, keys: &str) -> anyhow::Result<()> {
        for key in parse_macro(keys)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(key.into());
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(key.into());
            self.app.handle_terminal_events(Ok(event)).await;
        }
        self.app.editor.reset_idle_timer();
        tokio::time::timeout(
            Duration::from_secs(10),
            run_event_loop_until_idle(&mut self.app),
        )
        .await?;
        Ok(())
    }

    async fn finish(&mut self) {
        assert!(self.app.close().await.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_add_saved_location_preserves_selection_and_existing_entries(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&["first.txt", "second.txt"])?;
    let id = f.app.editor.open(&f.paths[0], Action::Replace)?;
    let view_id = f.app.editor.tree.focus();
    let selection = Selection::single(10, 6);
    f.app
        .editor
        .document_mut(id)
        .unwrap()
        .set_selection(view_id, selection.clone());
    f.app.editor.quicklist.set_current(Some(0));
    let original = f.app.editor.quicklist.entries().to_vec();
    f.keys("v<A-q>").await?;
    assert_eq!(&f.app.editor.quicklist.entries()[..2], original.as_slice());
    assert_eq!(f.app.editor.quicklist.current(), Some(0));
    assert_eq!(
        f.app.editor.quicklist.entries()[2],
        QuicklistEntry {
            target: QuicklistTarget::Path(f.paths[0].clone()),
            position: QuicklistPosition::Selection(selection.clone()),
        }
    );
    assert!(f.app.editor.close_document(id, false).is_ok());
    f.app.editor.quicklist.set_current(Some(1));
    f.keys("]q").await?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.path(), Some(f.paths[0].as_path()));
    assert_eq!(doc.selection(view.id), &selection);
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_add_unsaved_location_restores_unicode_selection() -> anyhow::Result<()> {
    let mut f = Fixture::new(&[])?;
    f.keys("iαβγ<esc>").await?;
    let (view, doc) = view::current!(f.app.editor);
    let id = doc.id();
    let selection = Selection::single(3, 1);
    doc.set_selection(view.id, selection.clone());
    f.keys("<A-q>").await?;
    assert_eq!(
        f.app.editor.quicklist.entries(),
        &[QuicklistEntry {
            target: QuicklistTarget::Document(id),
            position: QuicklistPosition::Selection(selection.clone()),
        }]
    );
    f.keys("gg<space>q").await?;
    f.keys("<C-q>").await?;
    f.keys("<esc>]q").await?;
    let (view, doc) = current_ref!(f.app.editor);
    assert_eq!(doc.id(), id);
    assert_eq!(doc.selection(view.id), &selection);
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_delete_filtered_row_preserves_query_and_nearest_row() -> anyhow::Result<()> {
    for (movement, removed, accepted) in [("<down>", 2, 3), ("<end>", 3, 2)] {
        let mut f = Fixture::new(&[
            "unrelated.txt",
            "pick-aaa.txt",
            "pick-bbb.txt",
            "pick-ccc.txt",
        ])?;
        let current = current_ref!(f.app.editor).1.id();
        f.keys("<space>q").await?;
        // Use a literal filename fragment so temporary paths cannot add rows.
        f.keys("'pick-").await?;
        f.keys(movement).await?;
        f.keys("<C-x>").await?;
        assert_eq!(current_ref!(f.app.editor).1.id(), current);
        assert_eq!(f.app.editor.quicklist.entries().len(), 3);
        assert!(!f
            .app
            .editor
            .quicklist
            .entries()
            .iter()
            .any(|entry| entry.target == QuicklistTarget::Path(f.paths[removed].clone())));
        f.keys("<ret>").await?;
        assert_eq!(
            current_ref!(f.app.editor).1.path(),
            Some(f.paths[accepted].as_path())
        );
        // The picker must set the fresh index after removal.
        let index = f.app.editor.quicklist.current().unwrap();
        assert_eq!(
            f.app.editor.quicklist.entries()[index].target,
            QuicklistTarget::Path(f.paths[accepted].clone())
        );
        f.finish().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_delete_active_entry_continues_navigation_without_moving_cursor(
) -> anyhow::Result<()> {
    for (motion, expected) in [("]q", 2), ("[q", 0)] {
        let mut f = Fixture::new(&["first.txt", "second.txt", "third.txt"])?;
        f.keys("]q]q").await?;
        let (view, doc) = current_ref!(f.app.editor);
        let current = doc.id();
        let selection = doc.selection(view.id).clone();
        f.keys("<space>q").await?;
        f.keys("<C-x>").await?;
        let (view, doc) = current_ref!(f.app.editor);
        assert_eq!(doc.id(), current);
        assert_eq!(doc.selection(view.id), &selection);
        assert_eq!(f.app.editor.quicklist.current(), None);
        f.keys("<esc>").await?;
        f.keys(motion).await?;
        assert_eq!(
            current_ref!(f.app.editor).1.path(),
            Some(f.paths[expected].as_path())
        );
        f.finish().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_deletion_uses_picker_specific_bindings_and_handles_empty_results(
) -> anyhow::Result<()> {
    let mut f = Fixture::with_keys(
        &["only.txt"],
        "[keys.quicklist-picker]\nC-x = 'unbound'\nC-r = 'remove_entry'",
    )?;
    f.keys("<space>q").await?;
    f.keys("<C-x>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 1);
    f.keys("missing").await?;
    f.keys("<C-r>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 1);
    f.keys("<C-a><C-k>").await?;
    f.keys("<C-r>").await?;
    assert!(f.app.editor.quicklist.entries().is_empty());
    f.keys("<C-r><ret><esc>ix<esc>").await?;
    assert!(current_ref!(f.app.editor)
        .1
        .text()
        .to_string()
        .starts_with('x'));
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_resumed_picker_refreshes_after_replacement() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["first.txt", "second.txt", "third.txt"])?;
    f.keys("<space>q").await?;
    f.keys("<esc>").await?;
    let replacement = f.app.editor.quicklist.entries()[2].clone();
    f.app.editor.replace_quicklist(vec![replacement]);
    f.keys("<space>'").await?;
    f.keys("<C-x>").await?;
    assert!(f.app.editor.quicklist.entries().is_empty());
    f.keys("<esc>").await?;
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_deletion_preserves_remaining_duplicate_rows() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["duplicate.txt"])?;
    let entry = f.app.editor.quicklist.entries()[0].clone();
    f.app
        .editor
        .replace_quicklist(vec![entry.clone(), entry.clone(), entry]);
    f.keys("<space>q").await?;
    f.keys("<C-x>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 2);
    f.keys("<C-q>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 2);
    f.keys("<C-x>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 1);
    f.keys("<ret>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.path(),
        Some(f.paths[0].as_path())
    );
    assert_eq!(f.app.editor.quicklist.current(), Some(0));
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_deletion_stops_the_action_list() -> anyhow::Result<()> {
    let mut f = Fixture::with_keys(
        &["first.txt", "second.txt"],
        "[keys.quicklist-picker]\nC-r = ['remove_entry', 'accept']",
    )?;
    let current = current_ref!(f.app.editor).1.id();
    f.keys("<space>q").await?;
    f.keys("<C-r>").await?;
    assert_eq!(f.app.editor.quicklist.entries().len(), 1);
    assert_eq!(current_ref!(f.app.editor).1.id(), current);
    f.keys("<ret>").await?;
    assert_eq!(
        current_ref!(f.app.editor).1.path(),
        Some(f.paths[1].as_path())
    );
    f.finish().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quicklist_resumed_picker_cannot_repopulate_from_stale_rows() -> anyhow::Result<()> {
    let mut f = Fixture::new(&["first.txt", "second.txt"])?;
    f.keys("<space>q").await?;
    f.keys("<esc>").await?;
    let replacement = vec![f.app.editor.quicklist.entries()[1].clone()];
    f.app.editor.replace_quicklist(replacement.clone());
    // Resume and capture before the first redraw can refresh the cached rows.
    f.keys("<space>'<C-q>").await?;
    assert_eq!(f.app.editor.quicklist.entries(), replacement.as_slice());
    f.keys("<C-x>").await?;
    assert!(f.app.editor.quicklist.entries().is_empty());
    f.keys("<esc>").await?;
    f.finish().await;
    Ok(())
}

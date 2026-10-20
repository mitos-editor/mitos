use std::{
    ops::ControlFlow,
    sync::{Arc, Mutex},
};

use super::Fixture;
use view::{
    current_ref,
    editor::Action,
    features::syntax_symbols::{Symbol, WorkspaceSymbols},
};

fn fixture() -> anyhow::Result<Fixture> {
    let mut fixture = Fixture::with_languages(
        "",
        r#"
        [[language]]
        name = "rust"
        scope = "source.rust"
        file-types = ["rs"]
        roots = []
    "#,
    )?;
    std::fs::create_dir(fixture.dir.path().join(".mitos"))?;
    let source = fixture.dir.path().join("open.rs");
    std::fs::write(&source, "fn sample_disk() {}\n")?;
    fixture.editor.open(&source, Action::Replace)?;
    fixture.replace_and_commit("fn sample_unsaved() {}\n");
    Ok(fixture)
}

async fn names(
    session: &Arc<WorkspaceSymbols>,
    editor: &mut view::Editor,
) -> anyhow::Result<Vec<String>> {
    let results = Arc::new(Mutex::new(Vec::new()));
    let sink = results.clone();
    session
        .query(
            editor,
            "sample",
            move |symbol| {
                sink.lock().unwrap().push(symbol.tag.name);
                ControlFlow::Continue(())
            },
            || false,
        )
        .await?;
    let mut names = results.lock().unwrap().clone();
    names.sort();
    Ok(names)
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbols_use_unsaved_buffers_and_refresh_cached_disk_files() -> anyhow::Result<()>
{
    let mut fixture = fixture()?;
    let disk = fixture.dir.path().join("disk.rs");
    std::fs::write(&disk, "fn sample_original() {}\n")?;
    let session = Arc::new(WorkspaceSymbols::new(
        &fixture.editor,
        current_ref!(fixture.editor).1.id(),
        search::Options {
            parents: false,
            git_global: false,
            ..Default::default()
        },
    ));
    assert_eq!(
        names(&session, &mut fixture.editor).await?,
        ["sample_original", "sample_unsaved"]
    );
    assert!(!session.refresh(&fixture.editor));
    std::fs::write(&disk, "fn sample_updated() {}\n")?;
    let events = view::file_watcher::Events::from(vec![view::file_watcher::Event {
        path: view::file_watcher::CanonicalPathBuf::assert_canonicalized(&disk.canonicalize()?),
        ty: view::file_watcher::EventType::Modified,
    }]);
    fixture.editor.handle_file_events(&events);
    assert!(session.refresh(&fixture.editor));
    assert_eq!(
        names(&session, &mut fixture.editor).await?,
        ["sample_unsaved", "sample_updated"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbol_queries_respect_cancellation_and_editor_ownership() -> anyhow::Result<()>
{
    let mut fixture = fixture()?;
    let mut other = Fixture::new("")?;
    let session = Arc::new(WorkspaceSymbols::new(
        &fixture.editor,
        current_ref!(fixture.editor).1.id(),
        search::Options::default(),
    ));
    session
        .query(
            &mut fixture.editor,
            "sample",
            |_: Symbol| panic!("canceled query emitted a symbol"),
            || true,
        )
        .await?;
    assert!(session
        .query(
            &mut other.editor,
            "sample",
            |_: Symbol| panic!("foreign query emitted a symbol"),
            || false
        )
        .await
        .is_err());
    Ok(())
}

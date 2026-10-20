use super::Fixture;
use editor_core::Transaction;
use std::{
    ops::ControlFlow,
    sync::{Arc, Mutex},
};
use view::{
    current,
    features::workspace_search::{Request, WorkspaceSearch},
};

#[tokio::test(flavor = "multi_thread")]
async fn queries_capture_unsaved_buffers_before_worker_execution() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("disk\n")?;
    let config = Arc::new(WorkspaceSearch {
        root: stdx::path::normalize(fixture.dir.path().canonicalize()?),
        smart_case: true,
        options: search::Options::default(),
    });
    let (view, doc) = current!(fixture.editor);
    let change = Transaction::change(
        doc.text(),
        [(0, 5, Some("héllo needle\n".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id).unwrap();
    let collected = Request::new("needle", &fixture.editor, config.clone())?.unwrap();
    let streamed = Request::new("needle", &fixture.editor, config.clone())?.unwrap();
    let (view, doc) = current!(fixture.editor);
    let change = Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("changed\n".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id).unwrap();
    let collected = collected.collect(|| false).await?;
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].match_start_col, 6);
    assert!(collected[0].path.is_absolute());
    let results = Arc::new(Mutex::new(Vec::new()));
    let output = results.clone();
    streamed
        .run(
            || false,
            move |matched| {
                output.lock().unwrap().push(matched);
                ControlFlow::Continue(())
            },
        )
        .await?;
    assert_eq!(*results.lock().unwrap(), collected);
    assert!(Request::new("needle", &fixture.editor, config)?
        .unwrap()
        .collect(|| false)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_validate_patterns_and_honor_cancellation() -> anyhow::Result<()> {
    let fixture = Fixture::new("needle\n")?;
    let config = Arc::new(WorkspaceSearch {
        root: fixture.dir.path().to_owned(),
        smart_case: true,
        options: search::Options::default(),
    });
    assert!(Request::new("", &fixture.editor, config.clone())?.is_none());
    assert!(Request::new("[", &fixture.editor, config.clone()).is_err());
    assert!(Request::new("needle", &fixture.editor, config)?
        .unwrap()
        .collect(|| true)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_query_future_stops_its_blocking_worker() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Finished(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Finished {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let fixture = Fixture::new("needle\nneedle\nneedle\n")?;
    let config = Arc::new(WorkspaceSearch {
        root: fixture.dir.path().to_owned(),
        smart_case: true,
        options: search::Options::default(),
    });
    let request = Request::new("needle", &fixture.editor, config)?.unwrap();
    let (started_tx, started) = tokio::sync::oneshot::channel();
    let started_tx = Mutex::new(Some(started_tx));
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let (finished, done) = tokio::sync::oneshot::channel();
    let finished = Finished(Some(finished));
    let count = Arc::new(AtomicUsize::new(0));
    let emitted = count.clone();
    let task = tokio::spawn(request.run(
        || false,
        move |_| {
            let _ = &finished;
            if emitted.fetch_add(1, Ordering::Relaxed) == 0 {
                let _ = started_tx.lock().unwrap().take().unwrap().send(());
                released.lock().unwrap().recv().unwrap();
            }
            ControlFlow::Continue(())
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), started).await??;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.send(())?;
    tokio::time::timeout(std::time::Duration::from_secs(5), done).await??;
    assert_eq!(count.load(Ordering::Relaxed), 1);
    Ok(())
}

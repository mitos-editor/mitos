use ropey::Rope;
use tokio::task::JoinHandle;

use crate::diff::{DiffHandle, Hunk};

impl DiffHandle {
    fn new_test(diff_base: &str, doc: &str) -> (DiffHandle, JoinHandle<()>) {
        DiffHandle::new_with_handle(
            Rope::from_str(diff_base),
            Rope::from_str(doc),
            event::Redraw::default(),
        )
    }
    async fn into_diff(self, handle: JoinHandle<()>) -> Vec<Hunk> {
        let diff = self.diff;
        // dropping the channel terminates the task
        drop(self.channel);
        handle.await.unwrap();
        let diff = diff.read();
        Vec::clone(&diff.hunks)
    }
}

#[tokio::test]
async fn readiness_tracks_initial_and_queued_snapshots() {
    let (differ, worker) = DiffHandle::new_test("base\n", "base\n");
    assert!(!differ.load().is_current());
    tokio::task::yield_now().await;
    assert!(differ.load().is_current());

    // A previously loaded diff also becomes stale as soon as an update is queued.
    let snapshot = differ.load();
    differ.update_document(Rope::from_str("first\n"), false);
    differ.update_document(Rope::from_str("last\n"), false);
    differ.update_diff_base(Rope::from_str("new base\n"));
    assert!(!snapshot.is_current());
    drop(snapshot);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !differ.load().is_current() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(differ.load().doc(), "last\n");
    assert_eq!(differ.load().diff_base(), "new base\n");
    differ.into_diff(worker).await;
}

#[tokio::test]
async fn append_line() {
    let (differ, handle) = DiffHandle::new_test("foo\n", "foo\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..1,
            after: 1..2
        }]
    )
}

#[tokio::test]
async fn prepend_line() {
    let (differ, handle) = DiffHandle::new_test("foo\n", "bar\nfoo\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 0..0,
            after: 0..1
        }]
    )
}

#[tokio::test]
async fn modify() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\n", "foo bar\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 0..1,
            after: 0..1
        }]
    )
}

#[tokio::test]
async fn delete_line() {
    let (differ, handle) = DiffHandle::new_test("foo\nfoo bar\nbar\n", "foo\nbar\n");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..2,
            after: 1..1
        }]
    )
}

#[tokio::test]
async fn delete_line_and_modify() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\ntest\nfoo", "foo\ntest\nfoo bar");
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

#[tokio::test]
async fn add_use() {
    let (differ, handle) = DiffHandle::new_test(
        "use ropey::Rope;\nuse tokio::task::JoinHandle;\n",
        "use ropey::Rope;\nuse ropey::RopeSlice;\nuse tokio::task::JoinHandle;\n",
    );
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[Hunk {
            before: 1..1,
            after: 1..2
        },]
    )
}

#[tokio::test]
async fn update_document() {
    let (differ, handle) = DiffHandle::new_test("foo\nbar\ntest\nfoo", "foo\nbar\ntest\nfoo");
    differ.update_document(Rope::from_str("foo\ntest\nfoo bar"), false);
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

#[tokio::test]
async fn update_base() {
    let (differ, handle) = DiffHandle::new_test("foo\ntest\nfoo bar", "foo\ntest\nfoo bar");
    differ.update_diff_base(Rope::from_str("foo\nbar\ntest\nfoo"));
    let line_diffs = differ.into_diff(handle).await;
    assert_eq!(
        &line_diffs,
        &[
            Hunk {
                before: 1..2,
                after: 1..1
            },
            Hunk {
                before: 3..4,
                after: 2..3
            },
        ]
    )
}

#[tokio::test]
async fn background_diff_completion_notifies_only_its_owner() {
    let first = event::Redraw::default();
    let second = event::Redraw::default();
    let (diff, worker) = DiffHandle::new_with_handle(
        Rope::from_str("base\n"),
        Rope::from_str("base\n"),
        first.clone(),
    );
    diff.update_diff_base(Rope::from_str("changed\n"));
    tokio::time::timeout(std::time::Duration::from_secs(1), first.requested())
        .await
        .unwrap();
    assert!(!diff.load().is_empty());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), second.requested())
            .await
            .is_err()
    );
    drop(diff);
    tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
}

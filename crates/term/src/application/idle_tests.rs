use super::ready_input;
use futures_util::FutureExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[tokio::test]
async fn idle_probe_reads_queued_input_after_cooperative_budget_exhaustion() {
    // A spawned task has a cooperative budget. Stay in the same poll after it
    // runs out, exactly as an immediately ready idle-timer branch can do.
    tokio::spawn(async {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(42).unwrap();
        let mut input = UnboundedReceiverStream::new(rx);
        let mut exhausted = false;
        for _ in 0..1_024 {
            if tokio::task::consume_budget().now_or_never().is_none() {
                exhausted = true;
                break;
            }
        }
        assert!(exhausted, "the test must exercise exhausted budget");
        assert_eq!(ready_input(&mut input), Some(42));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn idle_probe_does_not_wait_for_missing_input() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut input = UnboundedReceiverStream::new(rx);
    assert_eq!(ready_input(&mut input), None);
    tx.send(7).unwrap();
    assert_eq!(ready_input(&mut input), Some(7));
    drop(tx);
    assert_eq!(ready_input(&mut input), None);
}

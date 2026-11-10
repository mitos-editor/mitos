//! Shared lifetime for demand-driven Git workers. Suspending drops native state;
//! cursors retain only the identity and offset needed to replay on their next read.
use anyhow::{Context, Result};
use std::sync::Mutex;
use tokio::sync::mpsc;

struct Running<R> {
    sender: mpsc::Sender<R>,
    task: event::TaskController,
}

pub(super) struct Worker<R>(Mutex<Option<Running<R>>>);

impl<R> Default for Worker<R> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<R: Send + 'static> Worker<R> {
    pub fn sender(
        &self,
        name: &str,
        run: impl FnOnce(mpsc::Receiver<R>, event::TaskHandle) + Send + 'static,
    ) -> Result<mpsc::Sender<R>> {
        let mut running = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Git worker lock poisoned"))?;
        if running
            .as_ref()
            .is_none_or(|worker| worker.sender.is_closed())
        {
            let (sender, receiver) = mpsc::channel(1);
            let mut task = event::TaskController::new();
            let cancel = task.restart();
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || run(receiver, cancel))
                .context("Start Git worker")?;
            *running = Some(Running { sender, task });
        }
        Ok(running.as_ref().unwrap().sender.clone())
    }

    pub fn suspend(&self) {
        // Dropping the sender wakes an idle worker. Cancellation also stops a
        // worker whose current caller still owns a channel handle.
        if let Ok(mut running) = self.0.lock()
            && let Some(mut worker) = running.take()
        {
            worker.task.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as sync;
    use std::time::Duration;

    #[test]
    fn suspension_cancels_an_active_worker_even_while_a_caller_retains_its_sender() -> Result<()> {
        let worker = Worker::<()>::default();
        let (started, running) = sync::channel();
        let (stopped, released) = sync::channel();
        let sender = worker.sender("test-git-worker", move |mut requests, cancel| {
            requests.blocking_recv().unwrap();
            started.send(()).unwrap();
            while !cancel.is_canceled() {
                std::thread::yield_now();
            }
            drop(requests);
            stopped.send(()).unwrap();
        })?;
        sender.blocking_send(())?;
        running.recv_timeout(Duration::from_secs(5))?;
        worker.suspend();
        released.recv_timeout(Duration::from_secs(5))?;
        assert!(sender.is_closed());
        let (restarted, fresh) = sync::channel();
        let sender = worker.sender("test-git-worker", move |mut requests, _cancel| {
            requests.blocking_recv().unwrap();
            restarted.send(()).unwrap();
        })?;
        sender.blocking_send(())?;
        fresh.recv_timeout(Duration::from_secs(5))?;
        Ok(())
    }
}

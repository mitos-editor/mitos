//! Owned signals and synchronization for drawing one editor.

use std::{future::Future, sync::Arc};

use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};
use tokio::sync::Notify;

/// Redraw requests and frame synchronization belonging to one frontend.
/// Clones retain that destination across tasks, threads, and runtime boundaries.
#[derive(Clone, Debug, Default)]
pub struct Redraw {
    notify: Arc<Notify>,
    render_lock: Arc<RwLock<()>>,
}

pub type RenderLockGuard = ArcRwLockReadGuard<RawRwLock, ()>;

impl Redraw {
    /// Requests coalesce until consumed. The editor applies its frame-rate limit.
    pub fn request(&self) {
        self.notify.notify_one();
    }

    pub fn callback(&self) -> impl Fn() + Send + Sync + 'static + use<> {
        let notify = self.notify.clone();
        move || notify.notify_one()
    }

    pub fn requested(&self) -> impl Future<Output = ()> + '_ {
        self.notify.notified()
    }

    /// Wait for outstanding computations and consume the pending redraw permit.
    pub fn start_frame(&self) {
        drop(self.render_lock.write());
        let notify = self.notify.notified();
        tokio::pin!(notify);
        notify.enable();
    }

    pub fn lock_frame(&self) -> RenderLockGuard {
        self.render_lock.read_arc()
    }

    pub fn on_drop(&self) -> RequestRedrawOnDrop {
        RequestRedrawOnDrop(self.notify.clone())
    }
}

/// Requests a redraw of its originating frontend when dropped.
#[derive(Clone)]
pub struct RequestRedrawOnDrop(Arc<Notify>);

impl Drop for RequestRedrawOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn background_requests_and_drop_guards_keep_their_owner() {
        let first = Redraw::default();
        let second = Redraw::default();
        std::thread::spawn(first.callback()).join().unwrap();
        tokio::time::timeout(Duration::from_secs(1), first.requested())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), second.requested())
                .await
                .is_err()
        );
        let guard = first.on_drop();
        std::thread::spawn(move || drop(guard)).join().unwrap();
        tokio::time::timeout(Duration::from_secs(1), first.requested())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), second.requested())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn requests_coalesce_and_frames_drain_only_their_own_signal() {
        let first = Redraw::default();
        let second = Redraw::default();
        for _ in 0..10 {
            first.request();
        }
        tokio::time::timeout(Duration::from_secs(1), first.requested())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), first.requested())
                .await
                .is_err()
        );
        first.request();
        second.request();
        first.start_frame();
        tokio::time::timeout(Duration::from_secs(1), second.requested())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), first.requested())
                .await
                .is_err()
        );
    }

    #[test]
    fn frame_guards_block_only_their_owner_and_outlive_its_handle() {
        let first = Redraw::default();
        let second = Redraw::default();
        let guard = first.lock_frame();
        assert!(first.render_lock.try_write().is_none());
        assert!(second.render_lock.try_write().is_some());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            first.start_frame();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
        drop(guard);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let guard = second.lock_frame();
        drop(second);
        drop(guard);
    }
}

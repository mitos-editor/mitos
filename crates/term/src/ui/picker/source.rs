//! Source completion is independent of matching and producer handle lifetimes.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    #[cfg(any(feature = "git", test))]
    More,
    Loading,
    #[default]
    Complete,
    Paused,
    Failed,
}

#[derive(Clone, Default)]
pub(super) struct Progress(Arc<Mutex<State>>);

impl Progress {
    pub fn get(&self) -> State {
        *self.0.lock().unwrap()
    }
    pub fn report(&self, state: State, version: usize, current: &AtomicUsize) -> bool {
        let mut status = self.0.lock().unwrap();
        if current.load(Ordering::Relaxed) == version
            && !(state == State::Complete && *status == State::Failed)
        {
            *status = state;
            true
        } else {
            false
        }
    }
    pub fn set(&self, state: State) {
        *self.0.lock().unwrap() = state;
    }
}

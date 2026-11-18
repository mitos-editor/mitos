//! Explicit user actions, retained only while their operation or completion is alive.

use std::sync::{Arc, Weak};

use tokio::time::Instant;

use crate::{DocumentId, Editor};

/// A snapshot of one running user action. `None` targets the workspace.
#[derive(Clone, Copy, Debug)]
pub struct Activity {
    pub label: &'static str,
    pub document: Option<DocumentId>,
    pub started: Instant,
}

struct State {
    activity: Activity,
    _redraw: event::RequestRedrawOnDrop,
}

/// Keep this guard through result application; dropping it retires the activity.
#[must_use = "keep the activity guard until the action finishes or is canceled"]
pub struct ActivityGuard {
    _state: Arc<State>,
}

#[derive(Default)]
pub(crate) struct Activities(Vec<Weak<State>>);

impl Activities {
    fn start(
        &mut self,
        label: &'static str,
        document: Option<DocumentId>,
        redraw: &event::Redraw,
    ) -> ActivityGuard {
        self.0.retain(|activity| activity.strong_count() != 0);
        let state = Arc::new(State {
            activity: Activity {
                label,
                document,
                started: Instant::now(),
            },
            _redraw: redraw.on_drop(),
        });
        self.0.push(Arc::downgrade(&state));
        redraw.request();
        ActivityGuard { _state: state }
    }

    fn iter(&self) -> impl Iterator<Item = Activity> + '_ {
        self.0
            .iter()
            .filter_map(Weak::upgrade)
            .map(|state| state.activity)
    }
}

impl Editor {
    /// Register an explicit command, independently of background jobs and LSP progress.
    pub fn start_activity(
        &mut self,
        label: &'static str,
        document: Option<DocumentId>,
    ) -> ActivityGuard {
        let redraw = self.redraw().clone();
        self.activities.start(label, document, &redraw)
    }

    /// Running actions in start order, including those still awaiting result application.
    pub fn activities(&self) -> impl Iterator<Item = Activity> + '_ {
        self.activities.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guards_keep_concurrent_actions_independent_and_redraw_on_drop() {
        let redraw = event::Redraw::default();
        let mut first = Activities::default();
        let second = Activities::default();
        let formatting = first.start("Formatting", Some(DocumentId::default()), &redraw);
        let git = first.start("Git", None, &redraw);
        assert_eq!(
            first.iter().map(|a| a.label).collect::<Vec<_>>(),
            ["Formatting", "Git"]
        );
        assert!(second.iter().next().is_none());
        redraw.start_frame();
        drop(formatting);
        assert_eq!(first.iter().map(|a| a.label).collect::<Vec<_>>(), ["Git"]);
        tokio::time::timeout(std::time::Duration::from_secs(1), redraw.requested())
            .await
            .unwrap();
        drop(git);
        assert!(first.iter().next().is_none());
        let _next = first.start("Next", None, &redraw);
        assert_eq!(first.0.len(), 1);
    }
}

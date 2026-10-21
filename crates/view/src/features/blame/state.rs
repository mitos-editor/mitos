//! Committed blame snapshots, request lifetimes and publication rules.

use std::{path::PathBuf, sync::Arc};

use event::{cancelable_future, TaskController, TaskHandle};
use tokio::sync::Semaphore;
use vcs::FileBlame;

use crate::features::blame::BlameHandler;
use crate::Document;

// Running blocking work retains its permit even when its request is canceled.
static WORKERS: Semaphore = Semaphore::const_new(2);

#[derive(Default)]
pub(crate) struct DocumentBlame {
    cache: Option<anyhow::Result<Arc<FileBlame>>>,
    retained: Option<Arc<FileBlame>>,
    trust_full: Option<bool>,
    request: TaskController,
    status_line: Option<u32>,
    pub(super) handler: Option<BlameHandler>,
}

impl DocumentBlame {
    pub(crate) fn presentation(&self) -> Self {
        Self {
            cache: self
                .cache
                .as_ref()
                .and_then(|cache| cache.as_ref().ok())
                .cloned()
                .map(Ok),
            retained: self.retained.clone(),
            trust_full: self.trust_full,
            status_line: self.status_line,
            ..Self::default()
        }
    }

    pub(crate) fn bind_handler(&mut self, handler: BlameHandler) {
        self.handler = Some(handler);
    }
    pub(crate) fn handler(&self) -> Option<&BlameHandler> {
        self.handler.as_ref()
    }

    pub(crate) fn clear(&mut self) {
        self.request.cancel();
        self.status_line = None;
        self.cache = None;
        self.retained = None;
        self.trust_full = None;
    }

    /// Hide stale annotations until the worker has checked the current HEAD.
    pub(crate) fn refresh(&mut self) {
        self.request.cancel();
        self.status_line = None;
        if let Some(Ok(cache)) = self.cache.take() {
            self.retained = Some(cache);
        }
    }
}

pub(crate) struct BlameRequest {
    path: PathBuf,
    trust_full: bool,
    cached: Option<Arc<FileBlame>>,
    cancel: TaskHandle,
}

impl BlameRequest {
    pub(crate) async fn compute(&self) -> Option<anyhow::Result<Arc<FileBlame>>> {
        let result = cancelable_future(
            async {
                let permit = WORKERS.acquire().await?;
                let cancel = self.cancel.clone();
                let path = self.path.clone();
                let cached = self.cached.clone();
                let trust_full = self.trust_full;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    if cancel.is_canceled() {
                        return Ok(None);
                    }
                    FileBlame::try_refresh(path, trust_full, cached).map(Some)
                })
                .await?
            },
            &self.cancel,
        )
        .await?;
        match result {
            Ok(Some(blame)) => Some(Ok(blame)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }

    /// Install only on the requesting document and return its latest status request.
    pub(crate) fn complete(
        self,
        doc: &mut Document,
        result: anyhow::Result<Arc<FileBlame>>,
        trust_full: bool,
    ) -> Option<u32> {
        if self.cancel.is_canceled() || doc.path() != Some(&self.path) || doc.is_binary() {
            return None;
        }
        if self.trust_full != trust_full {
            doc.blame.refresh();
            return None;
        }
        let line = doc.blame.status_line.take();
        doc.set_file_blame(result);
        line
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LineBlameError<'a> {
    #[error("Not committed yet")]
    NotCommittedYet,
    #[error("Unable to get blame for line {0}: {1}")]
    NoFileBlame(u32, &'a anyhow::Error),
    #[error("The blame for this file is not ready yet. Try again in a few seconds")]
    NotReadyYet,
}

impl Document {
    /// Discard cached and pending blame after path or trust changes.
    pub(crate) fn invalidate_blame(&mut self) {
        self.blame.clear();
    }

    /// Effective workspace decisions can change without changing configuration fields.
    pub(crate) fn update_blame_trust(&mut self, trust_full: bool) -> bool {
        let changed = self.blame.trust_full.is_some_and(|old| old != trust_full);
        if changed {
            self.invalidate_blame();
        }
        self.blame.trust_full = Some(trust_full);
        changed
    }

    /// Get the line blame for this view
    pub fn line_blame(&self, cursor_line: u32, format: &str) -> Result<String, LineBlameError<'_>> {
        self.line_blame_details(cursor_line)
            .map(|blame| blame.parse_format(format))
    }

    /// Resolve an unchanged buffer line to its committed metadata.
    pub fn line_blame_details(
        &self,
        cursor_line: u32,
    ) -> Result<&vcs::LineBlame, LineBlameError<'_>> {
        let file_blame = match &self.blame.cache {
            None => return Err(LineBlameError::NotReadyYet),
            Some(result) => match result {
                Err(err) => {
                    return Err(LineBlameError::NoFileBlame(
                        // convert 0-based line into 1-based line
                        cursor_line.saturating_add(1),
                        err,
                    ));
                }
                Ok(file_blame) => file_blame,
            },
        };

        let diff = self
            .diff_handle()
            .and_then(|handle| handle.try_load())
            .ok_or(LineBlameError::NotReadyYet)?;
        if !diff.is_current() {
            return Err(LineBlameError::NotReadyYet);
        }
        let base_line = diff
            .base_line(cursor_line)
            .ok_or(LineBlameError::NotCommittedYet)?;
        let line_blame = file_blame
            .blame_for_line(base_line)
            .ok_or(LineBlameError::NotCommittedYet)?;
        Ok(line_blame)
    }

    pub fn file_blame(&self) -> Option<&anyhow::Result<Arc<FileBlame>>> {
        self.blame.cache.as_ref()
    }

    /// Install a computed snapshot without allowing pending results to overwrite it.
    pub fn set_file_blame(&mut self, result: anyhow::Result<Arc<FileBlame>>) {
        self.blame.request.cancel();
        self.blame.status_line = None;
        self.blame.retained = None;
        self.blame.cache = Some(result);
    }

    pub(crate) fn blame_request(
        &mut self,
        trust_full: bool,
        line: Option<u32>,
    ) -> Option<BlameRequest> {
        self.update_blame_trust(trust_full);
        if self.is_binary() || self.blame.cache.is_some() {
            return None;
        }
        let path = self.path()?.to_owned();
        if line.is_some() {
            self.blame.status_line = line;
        }
        if self.blame.request.is_running() {
            return None;
        }
        Some(BlameRequest {
            path,
            trust_full,
            cached: self.blame.retained.clone(),
            cancel: self.blame.request.restart(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canceled_requests_waiting_for_workers_do_not_start() {
        let permits = WORKERS.acquire_many(2).await.unwrap();
        let mut controller = TaskController::new();
        let request = BlameRequest {
            path: "missing".into(),
            trust_full: false,
            cached: None,
            cancel: controller.restart(),
        };
        let pending = request.compute();
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending)
                .await
                .is_err()
        );
        controller.cancel();
        assert!(pending.await.is_none());
        drop(permits);
    }
}

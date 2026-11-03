//! Editor-owned save progress and lifecycle barriers.
use super::Editor;
use crate::{document::DocumentSavedEventResult, save};

impl Editor {
    /// Accept a prepared save, retaining asynchronous stages until completion.
    /// Plain-write submission failures are returned immediately.
    pub fn submit_save(&mut self, request: save::PreparedSave) -> Result<(), save::SaveError> {
        let progress = save::submit(self, request)?;
        self.saves.retain(progress);
        Ok(())
    }

    pub(super) fn advance_save(
        &mut self,
        completion: save::Completion,
    ) -> Option<DocumentSavedEventResult> {
        let result = match completion {
            save::Completion::Write(result) => result,
            save::Completion::Stage(update) => {
                self.redraw().request();
                match update.and_then(|update| update.apply(self)) {
                    Ok(progress) => {
                        self.saves.retain(progress);
                        return None;
                    }
                    Err(error) => {
                        let message = format!("Async job failed: {error}");
                        Err(anyhow::Error::new(error).context(message))
                    }
                }
            }
        };
        if result.is_err() {
            self.cancel_after_writes();
        }
        Some(result)
    }

    /// Run after accepted saves and writes complete. Frontends with additional
    /// required work can use defer_after_writes until that work finishes too.
    pub fn after_writes(&mut self, action: save::AfterWrites) -> anyhow::Result<()> {
        if self.has_pending_writes() {
            self.defer_after_writes(action);
            Ok(())
        } else {
            action(self)
        }
    }

    /// Retain a write barrier while additional frontend-owned required work runs.
    pub fn defer_after_writes(&mut self, action: save::AfterWrites) {
        self.saves.after_writes.push_back(action);
    }

    pub fn cancel_after_writes(&mut self) {
        self.saves.after_writes.clear();
    }

    /// Run ready write barriers once additional frontend-owned work is complete.
    pub fn run_after_writes(&mut self) {
        while !self.has_pending_writes() {
            let Some(action) = self.saves.after_writes.pop_front() else {
                break;
            };
            if let Err(error) = action(self) {
                self.cancel_after_writes();
                self.set_error(|| error.to_string());
                break;
            }
        }
    }

    /// Drain accepted saves and writes, preserving the first error while all
    /// remaining operations finish. Servers requiring reverse requests need an
    /// event pump using wait_event instead, so those requests can be dispatched.
    pub async fn flush_writes(&mut self) -> anyhow::Result<()> {
        let mut first_error = None;
        while self.has_pending_writes() {
            let Some(completion) = save::next(&mut self.writes, &mut self.saves).await else {
                anyhow::bail!("save queue closed with unfinished writes");
            };
            let Some(result) = self.advance_save(completion) else {
                continue;
            };
            match result {
                Ok(event) => self.complete_document_write(&event),
                Err(error) => {
                    self.set_error(|| error.to_string());
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

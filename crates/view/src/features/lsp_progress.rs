//! Language-server work progress and completion effects, independent of presentation.

use crate::Editor;
use lsp_client::{lsp, LanguageServerId};

/// Optional status text for a frontend. Completion can clear its progress message.
pub struct ProgressUpdate {
    pub message: Option<String>,
    pub completed: bool,
}

impl Editor {
    pub fn language_server_is_progressing(&self, server: LanguageServerId) -> bool {
        self.handlers.lsp_progress.is_progressing(server)
    }

    pub fn progressing_language_servers(&self) -> impl Iterator<Item = LanguageServerId> + '_ {
        self.handlers.lsp_progress.progressing_servers()
    }

    pub fn create_language_server_progress(
        &mut self,
        server: LanguageServerId,
        token: lsp::ProgressToken,
    ) {
        if self.language_server_by_id(server).is_some() {
            self.handlers.lsp_progress.create(server, token);
        }
    }

    /// Process progress even when a frontend suppresses messages or has a prompt open.
    pub fn handle_language_server_progress(
        &mut self,
        server: LanguageServerId,
        params: lsp::ProgressParams,
    ) -> Option<ProgressUpdate> {
        let name = self.language_server_by_id(server)?.name().to_owned();
        let lsp::ProgressParams {
            token,
            value: lsp::ProgressParamsValue::WorkDone(work),
        } = params;
        let map = &mut self.handlers.lsp_progress;
        let title = map.title(server, &token).cloned();
        let (title, message, percentage) = match &work {
            lsp::WorkDoneProgress::Begin(begin) => (
                Some(begin.title.as_str()),
                begin.message.as_deref(),
                begin.percentage,
            ),
            lsp::WorkDoneProgress::Report(report) => (
                title.as_deref(),
                report.message.as_deref(),
                report.percentage,
            ),
            lsp::WorkDoneProgress::End(end) => (None, end.message.as_deref(), None),
        };
        let message = (title.is_some() || message.is_some() || percentage.is_some()).then(|| {
            use std::fmt::Write;
            let mut status = format!("{name}: ");
            if let Some(percentage) = percentage {
                write!(status, "{percentage:>2}% ").unwrap();
            }
            if let Some(title) = title {
                status.push_str(title);
            }
            if title.is_some() && message.is_some() {
                status.push_str(" ⋅ ");
            }
            if let Some(message) = message {
                status.push_str(message);
            }
            status
        });
        let mut completed = false;
        match work {
            lsp::WorkDoneProgress::Begin(begin) => map.begin(server, token, begin),
            lsp::WorkDoneProgress::Report(report) => map.update(server, token, report),
            lsp::WorkDoneProgress::End(_) => {
                completed =
                    map.end_progress(server, &token).is_some() && !map.is_progressing(server);
            }
        }
        if completed {
            super::diagnostics::pull::request_all_document_diagnostics_for_language_server(
                self, server,
            );
        }
        Some(ProgressUpdate { message, completed })
    }
}

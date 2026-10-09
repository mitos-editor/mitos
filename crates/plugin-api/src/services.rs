use std::{future::Future, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{ErrorCode, ServiceError};

pub type HostFuture<T> = Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    pub generation: u64,
    pub document: u64,
    pub version: i32,
    pub start: u64,
    pub end: u64,
    pub max_bytes: usize,
}

/// Closed job kinds let hosts authorize every external operation explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum JobRequest {
    Timer {
        milliseconds: u64,
    },
    Search {
        /// Index into the user's granted read roots, not an ambient guest path.
        root: u32,
        query: String,
    },
    Process {
        command: String,
        args: Vec<String>,
        input: String,
        root: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum JobOutput {
    Timer,
    Search {
        matches: Vec<SearchMatch>,
        truncated: bool,
    },
    Process {
        status: i32,
        stdout: String,
        stderr: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum JobPoll {
    Pending,
    Ready { output: JobOutput },
    Failed { error: ServiceError },
    Finished,
}

/// Jobs are owned and revoked with a plugin generation. Dropping/cancelling a
/// resource must also cancel its child work; a failed result cannot apply effects.
/// `cancel` completes only after child tasks/processes have stopped and cleanup
/// has finished. Hosts retain the job in the generation registry until then.
pub trait HostJob: Send + Sync + 'static {
    fn poll(&self) -> HostFuture<JobPoll>;
    fn cancel(&self) -> HostFuture<()>;
}

/// Bound by the host to one editor, plugin, generation, and permission set.
///
/// Editor adapters perform only short captures on their callback path; text
/// materialization and external work happen on background executors. The host
/// checks generation, live grants, targets, revisions, and sizes on every call.
pub trait HostServices: Send + Sync + 'static {
    fn read_document(&self, request: ReadRequest) -> HostFuture<String>;

    fn editor_request(
        &self,
        _request: crate::editor::EditorRequest,
    ) -> HostFuture<crate::editor::EditorReply> {
        Box::pin(async { Err(unsupported("editor service is unavailable")) })
    }

    fn read_file(&self, _root: u32, _path: String) -> HostFuture<String> {
        Box::pin(async { Err(unsupported("workspace file reads are unavailable")) })
    }

    fn write_file(&self, _root: u32, _path: String, _value: String) -> HostFuture<()> {
        Box::pin(async { Err(unsupported("workspace file writes are unavailable")) })
    }

    /// Creation is cancellation-safe: register ownership before starting work,
    /// return the job promptly, and run long operations inside that job. Dropping
    /// this future must never leave an untracked task or native child running.
    fn start_job(&self, _request: JobRequest) -> HostFuture<Arc<dyn HostJob>> {
        Box::pin(async { Err(unsupported("job service is unavailable")) })
    }

    fn storage_read(&self, _key: String) -> HostFuture<Option<String>> {
        Box::pin(async { Err(unsupported("storage service is unavailable")) })
    }

    fn storage_write(&self, _key: String, _value: String) -> HostFuture<()> {
        Box::pin(async { Err(unsupported("storage service is unavailable")) })
    }
}

fn unsupported(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::UnsupportedInterface, message)
}

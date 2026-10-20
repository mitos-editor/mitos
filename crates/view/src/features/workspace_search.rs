//! Workspace text requests own compiled patterns, buffer snapshots, and worker execution.
//! Frontends own replacement, display paths, and result publication.
use crate::Editor;
use editor_core::Rope;
use std::{collections::HashMap, ops::ControlFlow, path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct WorkspaceSearch {
    pub root: PathBuf,
    pub smart_case: bool,
    pub options: search::Options,
}

/// One query's compiled pattern and open-buffer snapshots, shared by both result paths.
pub struct Request {
    search: ::search::Search,
    documents: HashMap<std::path::PathBuf, Rope>,
    config: Arc<WorkspaceSearch>,
}

impl Request {
    pub fn new(
        query: &str,
        editor: &Editor,
        config: Arc<WorkspaceSearch>,
    ) -> anyhow::Result<Option<Self>> {
        if query.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(
            config.root.exists(),
            "Current working directory does not exist"
        );
        let search = ::search::Search::new(query, config.smart_case).map_err(|err| {
            log::info!("Failed to compile search pattern in global search: {}", err);
            anyhow::anyhow!("Failed to compile regex")
        })?;
        let documents = editor
            .documents()
            .filter_map(|doc| Some((doc.path()?.to_owned(), doc.text().clone())))
            .collect();
        Ok(Some(Self {
            search,
            documents,
            config,
        }))
    }

    /// Stream results on a blocking worker. Dropping the future cancels traversal.
    pub async fn run(
        self,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
        emit: impl Fn(search::Match) -> ControlFlow<()> + Send + Sync + 'static,
    ) -> anyhow::Result<()> {
        let mut task = event::TaskController::new();
        let cancel = task.restart();
        tokio::task::spawn_blocking(move || {
            self.search.run(
                &self.config.root,
                &self.config.options,
                &self.documents,
                || cancel.is_canceled() || cancelled(),
                emit,
            )
        })
        .await?;
        Ok(())
    }

    /// Collect a sorted replacement through the streaming execution path.
    pub async fn collect(
        self,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> anyhow::Result<Vec<search::Match>> {
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = results.clone();
        self.run(cancelled, move |matched| {
            output.lock().unwrap().push(matched);
            ControlFlow::Continue(())
        })
        .await?;
        let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        results.sort_by(|a, b| {
            (&a.path, a.line_start, a.match_start_col).cmp(&(
                &b.path,
                b.line_start,
                b.match_start_col,
            ))
        });
        Ok(results)
    }
}

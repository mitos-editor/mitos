//! Named workspace commands, retained output, and compiler locations.
//!
//! One editor owns one task. Workers publish bounded chunks through the editor's
//! callback queue; generation checks prevent old runs from replacing new results.

mod parser;
mod process;

use self::{
    output::OutputBuffer,
    parser::{Parser, StreamDecoder},
};
use super::output;
use crate::{editor::Action, quicklist::QuicklistEntry, DocumentId, Editor};
use anyhow::{ensure, Context, Result};
use event::TaskController;
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};
use tokio::{process::Command, sync::mpsc};
use workspace::trust::TrustQuery;

const LOCATION_LIMIT: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Cancelling,
    Succeeded,
    Failed(String),
    Cancelled,
}

impl TaskStatus {
    fn running(&self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }
}

#[derive(Clone)]
struct ResolvedTask {
    name: String,
    command: String,
    workspace: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    shell: Vec<String>,
    error_pattern: Option<String>,
}

struct Run {
    id: u64,
    task: ResolvedTask,
    output: OutputBuffer,
    locations: Vec<QuicklistEntry>,
    status: TaskStatus,
}

#[derive(Default)]
pub(crate) struct Tasks {
    generation: u64,
    process: TaskController,
    run: Option<Run>,
}

impl Editor {
    pub fn start_task(&mut self, name: &str) -> Result<DocumentId> {
        let config = self.config();
        let task = config
            .tasks
            .get(name)
            .with_context(|| format!("Unknown task {name:?}"))?;
        let workspace = self.workspace().root().to_path_buf();
        let task = ResolvedTask {
            name: name.to_owned(),
            command: task.command.clone(),
            cwd: task
                .cwd
                .as_ref()
                .map_or_else(|| workspace.clone(), |cwd| workspace.join(cwd)),
            workspace,
            env: task.env.clone(),
            shell: config.shell.clone(),
            error_pattern: task.error_pattern.clone(),
        };
        drop(config);
        self.launch_task(task)
    }

    pub fn rerun_task(&mut self) -> Result<DocumentId> {
        let task = self
            .tasks
            .run
            .as_ref()
            .context("No previous task to rerun")?
            .task
            .clone();
        self.launch_task(task)
    }

    fn launch_task(&mut self, task: ResolvedTask) -> Result<DocumentId> {
        ensure!(
            !self.task_running(),
            "A task is running; use :task-cancel first"
        );
        ensure!(
            self.workspace_trust
                .query(&task.workspace, TrustQuery::LocalConfig)
                .is_trusted(),
            "Workspace tasks require trust; run :workspace-trust"
        );
        ensure!(
            !task.command.trim().is_empty(),
            "Task command must not be empty"
        );
        let parser = Parser::new(task.error_pattern.as_deref(), task.cwd.clone())?;
        let (program, args) = task.shell.split_first().context("No shell configured")?;
        let mut command = Command::new(program);
        command
            .args(args)
            .arg(&task.command)
            .current_dir(&task.cwd)
            .envs(&task.env);

        let mut output = self
            .tasks
            .run
            .take()
            .map(|run| run.output)
            .unwrap_or_default();
        let label = format!("Task: {}", task.name);
        output.clear(self, &label);
        output.append(
            self,
            &format!("$ {}\nDirectory: {}\n\n", task.command, task.cwd.display()),
        );
        let document = output
            .document()
            .filter(|id| self.tree.views().any(|(view, _)| view.doc == *id))
            .unwrap_or_else(|| output.open(self, &label, Action::HorizontalSplit));
        self.tasks.generation += 1;
        let id = self.tasks.generation;
        let cancel = self.tasks.process.restart();
        self.tasks.run = Some(Run {
            id,
            task,
            output,
            locations: Vec::new(),
            status: TaskStatus::Running,
        });
        let callbacks = self.callback_sender().clone();
        tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel(16);
            let process = process::run(command, cancel, tx);
            tokio::pin!(process);
            let mut result = None;
            let mut stdout = StreamDecoder::default();
            let mut stderr = StreamDecoder::default();
            let mut seen = HashSet::new();
            let mut pending = String::new();
            let mut locations = Vec::new();
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(33));
            loop {
                let flush = tokio::select! {
                    outcome = &mut process, if result.is_none() => { result = Some(outcome); false }
                    chunk = rx.recv() => {
                        match chunk {
                            Some(chunk) => {
                                let decoder = match chunk.stream {
                                    process::Stream::Stdout => &mut stdout,
                                    process::Stream::Stderr => &mut stderr,
                                };
                                let (text, entries) = decoder.push(&chunk.bytes, false, &parser);
                                pending.push_str(&text);
                                collect_locations(entries, &mut seen, &mut locations);
                            }
                            None if result.is_some() => break,
                            None => { result = Some((&mut process).await); break; }
                        }
                        false
                    }
                    _ = ticks.tick() => true
                };
                if pending.len() >= 32 * 1024 || (flush && !pending.is_empty()) {
                    let text = std::mem::take(&mut pending);
                    let entries = std::mem::take(&mut locations);
                    callbacks
                        .send(move |editor| editor.publish_task_output(id, &text, entries))
                        .await;
                }
            }
            for decoder in [&mut stdout, &mut stderr] {
                let (text, entries) = decoder.push(&[], true, &parser);
                pending.push_str(&text);
                collect_locations(entries, &mut seen, &mut locations);
            }
            callbacks
                .send(move |editor| {
                    editor.publish_task_output(id, &pending, locations);
                    editor
                        .finish_task(id, result.expect("process completed before channel closed"));
                })
                .await;
        });
        Ok(document)
    }

    fn publish_task_output(&mut self, id: u64, text: &str, entries: Vec<QuicklistEntry>) {
        let Some(mut run) = self.tasks.run.take() else {
            return;
        };
        if run.id == id {
            run.output.append(self, text);
            run.locations.extend(entries);
        }
        self.tasks.run = Some(run);
    }

    fn finish_task(&mut self, id: u64, result: Result<Option<std::process::ExitStatus>>) {
        let Some(mut run) = self.tasks.run.take() else {
            return;
        };
        if run.id == id {
            let (status, message) = match result {
                Ok(None) => (TaskStatus::Cancelled, "cancelled".to_owned()),
                Ok(Some(status)) if status.success() => {
                    (TaskStatus::Succeeded, "exited successfully (0)".to_owned())
                }
                Ok(Some(status)) => {
                    let message = format!("exited with {status}");
                    (TaskStatus::Failed(message.clone()), message)
                }
                Err(error) => {
                    let message = format!("failed: {error:#}");
                    (TaskStatus::Failed(message.clone()), message)
                }
            };
            run.status = status;
            run.output
                .append(self, &format!("\n[Task {}: {message}]\n", run.task.name));
            self.quicklist.replace(run.locations.clone());
            self.set_status(format!(
                "Task {}: {message}; {} locations in quicklist",
                run.task.name,
                run.locations.len()
            ));
        }
        self.tasks.run = Some(run);
    }

    pub fn task_running(&self) -> bool {
        self.tasks
            .run
            .as_ref()
            .is_some_and(|run| run.status.running())
    }

    pub fn task_status(&self) -> Option<&TaskStatus> {
        self.tasks.run.as_ref().map(|run| &run.status)
    }

    pub fn task_output_text(&self) -> Option<&str> {
        self.tasks.run.as_ref().map(|run| run.output.text())
    }

    pub fn cancel_task(&mut self) -> Result<()> {
        ensure!(self.task_running(), "No task is running");
        self.tasks.run.as_mut().unwrap().status = TaskStatus::Cancelling;
        self.tasks.process.cancel();
        self.set_status("Cancelling task");
        Ok(())
    }

    pub fn open_task_output(&mut self, action: Action) -> Result<DocumentId> {
        let mut run = self.tasks.run.take().context("No task output available")?;
        let id = run
            .output
            .open(self, &format!("Task: {}", run.task.name), action);
        self.tasks.run = Some(run);
        Ok(id)
    }

    pub fn task_errors(&mut self) -> Result<usize> {
        let run = self.tasks.run.as_ref().context("No task has been run")?;
        let count = run.locations.len();
        self.quicklist.replace(run.locations.clone());
        Ok(count)
    }
}

fn collect_locations(
    entries: Vec<QuicklistEntry>,
    seen: &mut HashSet<(PathBuf, usize, usize)>,
    locations: &mut Vec<QuicklistEntry>,
) {
    for entry in entries {
        if seen.len() == LOCATION_LIMIT {
            break;
        }
        if let (
            crate::quicklist::QuicklistTarget::Path(path),
            crate::quicklist::QuicklistPosition::LineColRange {
                start_line,
                start_col,
                ..
            },
        ) = (&entry.target, &entry.position)
            && seen.insert((path.clone(), *start_line, *start_col))
        {
            locations.push(entry);
        }
    }
}

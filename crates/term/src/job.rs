use arc_swap::ArcSwapOption;
use event::status::StatusMessage;
use event::{runtime_local, send_blocking};
use std::{cell::RefCell, sync::Arc};
use view::callbacks::{EditorCallback, EditorCallbackSender, InvocationTasks, TaskOutcome};
use view::Editor;

use crate::{
    commands::{CommandCompletion, CommandToken},
    compositor::Compositor,
};

use futures_util::future::{BoxFuture, Future, FutureExt};
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type EditorCompositorCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;
pub type EditorCallbackFollowup = Box<dyn FnOnce(&mut Editor) -> Option<Job> + Send>;

runtime_local! {
    static JOB_QUEUE: ArcSwapOption<Sender<Callback>> = ArcSwapOption::const_empty();
}

fn callback_sender() -> Arc<Sender<Callback>> {
    JOB_QUEUE.load_full().expect("job queue is not initialized")
}

pub async fn dispatch_callback(job: Callback) {
    let _ = callback_sender().send(job).await;
}

pub async fn dispatch(job: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    let _ = callback_sender()
        .send(Callback::EditorCompositor(Box::new(job)))
        .await;
}

pub fn dispatch_blocking(job: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    let jobs = callback_sender();
    send_blocking(&jobs, Callback::EditorCompositor(Box::new(job)))
}

/// Record a native continuation that was safely discarded without displaying
/// an error (for example, a formatter whose original document was closed).
pub(crate) fn cancel_invocation(editor: &Editor, reason: &str) {
    if let Some(observer) = editor.invocation_tasks() {
        observer.started();
        observer.finished(TaskOutcome::Cancelled(reason.to_owned()));
    }
}

pub enum Callback {
    EditorCompositor(EditorCompositorCallback),
    Editor(EditorCallback),
    Followup(EditorCallbackFollowup),
    /// A job result remains owned until its mutation/follow-up has been applied.
    Tracked(CommandTask, anyhow::Result<Option<Box<Callback>>>),
}

pub struct CommandTask {
    token: CommandToken,
    active: bool,
}

impl CommandTask {
    fn new(token: CommandToken) -> Self {
        token.started();
        Self {
            token,
            active: true,
        }
    }

    fn finish(mut self, outcome: TaskOutcome) {
        self.active = false;
        self.token.finished(outcome);
    }
}

impl Drop for CommandTask {
    fn drop(&mut self) {
        if self.active {
            self.token.finished(TaskOutcome::Cancelled(
                "job result was not delivered".into(),
            ));
        }
    }
}

pub(crate) struct CommandScope {
    previous: Option<CommandToken>,
    observer: Option<Arc<dyn InvocationTasks>>,
}

pub type JobFuture = BoxFuture<'static, anyhow::Result<Option<Callback>>>;

pub struct Job {
    pub future: BoxFuture<'static, anyhow::Result<Option<Callback>>>,
    /// Do we need to wait for this job to finish before exiting?
    pub wait: bool,
}

pub struct Jobs {
    sender: Arc<Sender<Callback>>,
    /// jobs that need to complete before we exit.
    pub wait_futures: FuturesUnordered<JobFuture>,
    pub callbacks: Receiver<Callback>,
    pub status_messages: Receiver<StatusMessage>,
    command: RefCell<Option<CommandToken>>,
    commands: RefCell<Vec<CommandToken>>,
    command_jobs: RefCell<Vec<tokio::task::AbortHandle>>,
}

impl Job {
    pub fn new<F: Future<Output = anyhow::Result<()>> + Send + 'static>(f: F) -> Self {
        Self {
            future: f.map(|r| r.map(|()| None)).boxed(),
            wait: false,
        }
    }

    pub fn with_callback<F: Future<Output = anyhow::Result<Callback>> + Send + 'static>(
        f: F,
    ) -> Self {
        Self {
            future: f.map(|r| r.map(Some)).boxed(),
            wait: false,
        }
    }

    pub fn wait_before_exiting(mut self) -> Self {
        self.wait = true;
        self
    }
}

impl Jobs {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (tx, rx) = channel(1024);
        let sender = Arc::new(tx);
        let status_messages = event::status::setup();
        Self {
            sender,
            wait_futures: FuturesUnordered::new(),
            callbacks: rx,
            status_messages,
            command: RefCell::new(None),
            commands: RefCell::new(Vec::new()),
            command_jobs: RefCell::new(Vec::new()),
        }
    }

    /// Use this editor's queue for callbacks dispatched by event hooks.
    /// Creating another job collection does not replace this queue.
    pub(crate) fn set_current(&self) {
        JOB_QUEUE.store(Some(self.sender.clone()));
    }

    /// Bind editor-only completions to this queue, independent of global dispatch.
    pub fn editor_callback_sender(&self) -> EditorCallbackSender {
        let sender = self.sender.clone();
        let blocking_sender = sender.clone();
        let try_sender = sender.clone();
        EditorCallbackSender::new(
            move |callback| {
                let sender = sender.clone();
                async move {
                    let _ = sender.send(Callback::Editor(callback)).await;
                }
            },
            move |callback| send_blocking(&blocking_sender, Callback::Editor(callback)),
        )
        .with_try_send(move |callback| {
            try_sender
                .try_send(Callback::Editor(callback))
                .map_err(|err| match err.into_inner() {
                    Callback::Editor(callback) => callback,
                    _ => unreachable!(),
                })
        })
    }

    pub fn spawn<F: Future<Output = anyhow::Result<()>> + Send + 'static>(&mut self, f: F) {
        self.add(Job::new(f));
    }

    pub fn callback<F: Future<Output = anyhow::Result<Callback>> + Send + 'static>(
        &mut self,
        f: F,
    ) {
        self.add(Job::with_callback(f));
    }

    pub(crate) fn begin_command(
        &self,
        editor: &Editor,
        completion: CommandCompletion,
    ) -> CommandToken {
        let parent = self
            .command
            .borrow()
            .clone()
            .map(|token| Arc::new(token) as Arc<dyn InvocationTasks>)
            .or_else(|| editor.invocation_tasks());
        let token = CommandToken::new(completion, parent);
        self.commands.borrow_mut().push(token.clone());
        token
    }

    pub(crate) fn should_track_command(&self, editor: &Editor) -> bool {
        self.command.borrow().is_some()
            || editor.invocation_tasks().is_some()
            || editor.plugin_event_interested(plugin_api::Event::PostCommand)
            || editor.plugin_event_interested(plugin_api::Event::ModeChanged)
    }

    pub(crate) fn enter_command(&self, editor: &mut Editor, token: CommandToken) -> CommandScope {
        token.rebase(editor);
        let observer = editor.replace_invocation_tasks(Some(Arc::new(token.clone())));
        let previous = self.command.replace(Some(token));
        CommandScope { previous, observer }
    }

    pub(crate) fn leave_command(&self, editor: &mut Editor, scope: CommandScope) {
        // A child command already observed its changes. Do not attribute them
        // again to an enclosing macro when it resumes.
        if let Some(previous) = &scope.previous {
            previous.rebase(editor);
        }
        self.command.replace(scope.previous);
        editor.replace_invocation_tasks(scope.observer);
    }

    /// Publish completed invocations on the editor's mutation path. Nested
    /// macro commands release their parent here, so a second pass may be ready.
    pub(crate) fn poll_commands(&self, editor: &Editor) {
        loop {
            let tokens = self.commands.borrow().clone();
            let mut changed = false;
            for token in tokens {
                changed |= token.publish_if_ready(editor);
            }
            self.commands
                .borrow_mut()
                .retain(|token| !token.published());
            if !changed {
                break;
            }
        }
        self.command_jobs
            .borrow_mut()
            .retain(|job| !job.is_finished());
    }

    pub(crate) fn cancel_commands(&self, editor: &Editor) {
        self.poll_commands(editor);
        for job in self.command_jobs.borrow_mut().drain(..) {
            job.abort();
        }
        for token in self.commands.borrow().iter() {
            token.cancel_pending();
        }
        self.poll_commands(editor);
    }

    pub fn handle_callback(
        &self,
        editor: &mut Editor,
        compositor: &mut Compositor,
        call: anyhow::Result<Option<Callback>>,
    ) -> Option<Job> {
        match self.apply_callback(editor, Some(compositor), call) {
            Ok(job) => job,
            Err(e) => {
                editor.set_error(|| format!("Async job failed: {}", e));
                None
            }
        }
    }

    fn apply_callback(
        &self,
        editor: &mut Editor,
        compositor: Option<&mut Compositor>,
        call: anyhow::Result<Option<Callback>>,
    ) -> anyhow::Result<Option<Job>> {
        match call {
            Ok(None) => Ok(None),
            Ok(Some(call)) => match call {
                Callback::EditorCompositor(call) => {
                    if let Some(compositor) = compositor {
                        call(editor, compositor);
                    }
                    Ok(None)
                }
                Callback::Editor(call) => {
                    call(editor);
                    Ok(None)
                }
                Callback::Followup(call) => Ok(call(editor)),
                Callback::Tracked(task, result) => {
                    let token = task.token.clone();
                    let scope = self.enter_command(editor, token.clone());
                    let missing_compositor = compositor.is_none()
                        && matches!(&result, Ok(Some(callback)) if matches!(callback.as_ref(), Callback::EditorCompositor(_)));
                    let result = self.apply_callback(
                        editor,
                        compositor,
                        result.map(|callback| callback.map(|callback| *callback)),
                    );
                    // The follow-up must inherit ownership before its parent
                    // task completes, including formatting -> write chains.
                    let result = result.map(|job| {
                        if let Some(job) = job {
                            self.add(job);
                        }
                    });
                    token.capture_effects(editor);
                    self.leave_command(editor, scope);
                    task.finish(match &result {
                        Err(error) => TaskOutcome::Error(error.to_string()),
                        Ok(_) if missing_compositor => {
                            TaskOutcome::Cancelled("compositor unavailable".into())
                        }
                        Ok(_) => TaskOutcome::Success,
                    });
                    result.map(|()| None)
                }
            },
            Err(e) => Err(e),
        }
    }

    pub fn add(&self, mut j: Job) {
        let tracked = self.command.borrow().clone();
        if let Some(token) = tracked.clone() {
            let task = CommandTask::new(token);
            let future = j.future;
            j.future = async move {
                let result = future.await.map(|callback| callback.map(Box::new));
                Ok(Some(Callback::Tracked(task, result)))
            }
            .boxed();
        }
        if j.wait {
            self.wait_futures.push(j.future);
        } else {
            // Keep each job attached to its originating editor, even if a new
            // editor replaces the runtime's default dispatch queue meanwhile.
            let sender = self.sender.clone();
            let handle = tokio::spawn(async move {
                match j.future.await {
                    Ok(Some(cb)) => {
                        let _ = sender.send(cb).await;
                    }
                    Ok(None) => (),
                    Err(err) => event::status::report(err).await,
                }
            });
            if tracked.is_some() {
                self.command_jobs.borrow_mut().push(handle.abort_handle());
            }
        }
    }

    /// Blocks until all the jobs that need to be waited on are done.
    pub async fn finish(
        &mut self,
        editor: &mut Editor,
        mut compositor: Option<&mut Compositor>,
    ) -> anyhow::Result<()> {
        log::debug!("waiting on jobs...");
        let mut first_error = None;

        while let Some(result) = self.wait_futures.next().await {
            match self.apply_callback(editor, compositor.as_deref_mut(), result) {
                Ok(Some(job)) if job.wait => self.add(job),
                Ok(_) => (),
                Err(error) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "integration")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_commands_skip_tokens_and_existing_observers_keep_eventual_outcomes(
    ) -> anyhow::Result<()> {
        use crate::{
            application::Application, args::Args, commands::Context, commands::MappableCommand,
            config::Config,
        };
        use std::sync::{atomic::AtomicUsize, atomic::Ordering, Mutex};

        #[derive(Default)]
        struct Observer {
            started: AtomicUsize,
            outcomes: Mutex<Vec<TaskOutcome>>,
            detached: AtomicUsize,
        }
        impl InvocationTasks for Observer {
            fn started(&self) {
                self.started.fetch_add(1, Ordering::Relaxed);
            }
            fn finished(&self, outcome: TaskOutcome) {
                self.outcomes.lock().unwrap().push(outcome);
            }
            fn detached(&self) {
                self.detached.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn failure(cx: &mut Context) {
            cx.jobs
                .add(Job::new(async { anyhow::bail!("owned failure") }).wait_before_exiting());
        }
        fn detached(cx: &mut Context) {
            cx.editor.invocation_tasks().unwrap().detached();
        }

        let _guard = QUEUE_TEST_LOCK.lock().await;
        let mut config = Config::default();
        config.editor.lsp.enable = false;
        config.editor.file_watcher.enable = false;
        config.editor.auto_reload.enable = false;
        let mut app = Application::new(
            Args::default(),
            config.clone(),
            editor_core::syntax::Loader::new(
                toml::from_str("language = []")?,
                loader::syntax::Resources::default(),
            )?,
            loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        )?;
        let mut jobs = Jobs::new();
        let (updates, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dispatch = |editor: &mut Editor, jobs: &mut Jobs, command: MappableCommand| {
            let mut cx = Context {
                config: crate::config::Context {
                    current: &config,
                    updates: &updates,
                },
                editor,
                jobs,
                count: None,
                register: None,
                callback: Vec::new(),
                on_next_key_callback: None,
            };
            command.execute(&mut cx);
            assert!(cx.callback.is_empty());
        };

        assert!(!jobs.should_track_command(&app.editor));
        dispatch(
            &mut app.editor,
            &mut jobs,
            MappableCommand::Static {
                name: "idle-command",
                fun: |_| {},
                doc: "test",
            },
        );
        dispatch(&mut app.editor, &mut jobs, ":echo idle".parse()?);
        assert!(jobs.commands.borrow().is_empty());

        let observer = Arc::new(Observer::default());
        app.editor.replace_invocation_tasks(Some(observer.clone()));
        assert!(jobs.should_track_command(&app.editor));
        dispatch(
            &mut app.editor,
            &mut jobs,
            MappableCommand::Static {
                name: "owned-command",
                fun: failure,
                doc: "test",
            },
        );
        jobs.poll_commands(&app.editor);
        assert_eq!(observer.started.load(Ordering::Relaxed), 1);
        assert!(observer.outcomes.lock().unwrap().is_empty());
        assert!(jobs.finish(&mut app.editor, None).await.is_err());
        jobs.poll_commands(&app.editor);
        assert_eq!(
            *observer.outcomes.lock().unwrap(),
            vec![TaskOutcome::Error("owned failure".into())]
        );
        assert!(jobs.commands.borrow().is_empty());

        dispatch(
            &mut app.editor,
            &mut jobs,
            MappableCommand::Static {
                name: "detached-command",
                fun: detached,
                doc: "test",
            },
        );
        jobs.poll_commands(&app.editor);
        assert_eq!(observer.detached.load(Ordering::Relaxed), 1);
        assert_eq!(observer.started.load(Ordering::Relaxed), 2);
        assert_eq!(observer.outcomes.lock().unwrap().len(), 2);
        app.editor.replace_invocation_tasks(None);
        assert!(!jobs.should_track_command(&app.editor));
        assert!(app.close().await.is_empty());
        Ok(())
    }

    // Without the integration-test feature, the selected queue is process-global.
    static QUEUE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn editor_sender_keeps_its_queue_and_backpressure() {
        let _guard = QUEUE_TEST_LOCK.lock().await;
        let mut first = Jobs::new();
        let sender = first.editor_callback_sender();
        let mut second = Jobs::new();
        second.set_current();

        sender.send_blocking(|_| {});
        assert!(matches!(
            first.callbacks.try_recv(),
            Ok(Callback::Editor(_))
        ));
        assert!(second.callbacks.try_recv().is_err());

        for _ in 0..1024 {
            sender.send(|_| {}).await;
        }
        let mut pending = std::pin::pin!(sender.send(|_| {}));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert!(matches!(
            first.callbacks.recv().await,
            Some(Callback::Editor(_))
        ));
        pending.await;
        assert!(second.callbacks.try_recv().is_err());

        // Shutdown drops queued completions and future sends finish without blocking.
        drop(first);
        sender.send(|_| {}).await;
        sender.send_blocking(|_| {});
    }

    #[tokio::test]
    async fn callbacks_follow_the_selected_queue_and_jobs_keep_their_owner() {
        let _guard = QUEUE_TEST_LOCK.lock().await;
        let mut first = Jobs::new();
        first.set_current();
        let mut second = Jobs::new();

        // Creating a temporary collection must not redirect event-hook callbacks.
        dispatch_callback(Callback::Editor(Box::new(|_| {}))).await;
        assert!(first.callbacks.try_recv().is_ok());
        assert!(second.callbacks.try_recv().is_err());

        second.set_current();
        first.callback(async { Ok(Callback::Editor(Box::new(|_| {}))) });
        tokio::time::timeout(std::time::Duration::from_secs(1), first.callbacks.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(second.callbacks.try_recv().is_err());

        dispatch_callback(Callback::Editor(Box::new(|_| {}))).await;
        assert!(second.callbacks.try_recv().is_ok());
    }
}

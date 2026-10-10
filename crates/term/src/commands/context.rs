//! Terminal command context, next-key callbacks, and job/compositor adapters.

use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use ui_core::input::KeyEvent;
use view::Editor;

use crate::{
    compositor::{self, Component, Compositor},
    events::CommandOrigin,
    job::{self, Callback, Jobs},
};

/// Owned metadata, so a callback can finish the same invocation after dispatch.
#[derive(Clone)]
pub(super) struct CommandInvocation {
    pub origin: CommandOrigin,
    pub count: Option<usize>,
    pub register: Option<char>,
    pub custom_command: Option<String>,
}

impl CommandInvocation {
    pub fn prompt() -> Self {
        Self {
            origin: CommandOrigin::Prompt,
            count: None,
            register: None,
            custom_command: None,
        }
    }
}

pub(crate) struct CommandCompletion {
    pub command: String,
    pub args: Vec<String>,
    pub raw_args: String,
    pub flags: std::collections::BTreeMap<String, String>,
    invocation: CommandInvocation,
    view: view::ViewId,
    document: Option<view::DocumentId>,
    binding_revision: u64,
    event_source: view::plugins::PluginEventSource,
    error_revision: u64,
    error: Option<String>,
    mode: view::document::Mode,
}

impl CommandCompletion {
    pub(super) fn new(editor: &Editor, command: &str, invocation: CommandInvocation) -> Self {
        Self {
            command: command.to_owned(),
            args: Vec::new(),
            raw_args: String::new(),
            flags: Default::default(),
            invocation,
            view: editor.tree.focus,
            document: editor.tree.try_get(editor.tree.focus).map(|view| view.doc),
            binding_revision: editor
                .tree
                .try_get(editor.tree.focus)
                .map_or(0, |view| view.binding_revision()),
            event_source: editor.plugin_event_source(),
            error_revision: editor.error_revision(),
            error: None,
            mode: editor.mode(),
        }
    }

    fn capture_effects(&mut self, editor: &Editor) {
        self.error = self.error.take().or_else(|| {
            (editor.error_revision() != self.error_revision && editor.is_err())
                .then(|| editor.get_status().unwrap().0.to_string())
        });
        self.error_revision = editor.error_revision();
        let mode = editor.mode();
        if mode != self.mode {
            self.queue_event(
                editor,
                plugin_api::Event::ModeChanged,
                serde_json::json!({
                    "old-mode": self.mode.to_string(),
                    "new-mode": mode.to_string(),
                    "command": self.command,
                    "origin": self.invocation.origin,
                }),
            );
            self.mode = mode;
        }
    }

    pub fn finish(self, editor: &Editor, error: Option<String>, cancelled: bool) {
        self.finish_outcome(editor, error, cancelled, false);
    }

    fn rebase(&mut self, editor: &Editor) {
        self.error_revision = editor.error_revision();
        self.mode = editor.mode();
    }

    fn finish_outcome(
        mut self,
        editor: &Editor,
        error: Option<String>,
        cancelled: bool,
        accepted: bool,
    ) {
        self.capture_effects(editor);
        let error = error.or_else(|| self.error.take());
        self.queue_event(editor,
            plugin_api::Event::PostCommand,
            serde_json::json!({
                "command": self.command,
                "args": self.args,
                "raw-args": self.raw_args,
                "flags": self.flags,
                "count": self.invocation.count,
                "register": self.invocation.register,
                "origin": self.invocation.origin,
                "custom-command": self.invocation.custom_command,
                "outcome": if error.is_some() { "error" } else if cancelled { "cancelled" } else if accepted { "accepted" } else { "success" },
                "error": error,
            }),
        );
    }

    fn queue_event(&self, editor: &Editor, event: plugin_api::Event, data: serde_json::Value) {
        if let Some(document) = self.document {
            editor.queue_plugin_event_for_view_binding_from_source(
                &self.event_source,
                event,
                self.view,
                document,
                self.binding_revision,
                data,
            );
        } else {
            editor.queue_plugin_event_for_view(event, self.view, data);
        }
    }
}

/// One command owns its compositor continuations, native jobs, and writes.
/// Workers only update this small state; publication happens on the editor path.
#[derive(Clone)]
pub(crate) struct CommandToken(Arc<Mutex<CommandState>>);

struct CommandState {
    completion: Option<CommandCompletion>,
    pending: usize,
    error: Option<String>,
    cancelled: bool,
    accepted: bool,
    parent: Option<Arc<dyn view::callbacks::InvocationTasks>>,
}

impl CommandToken {
    pub(crate) fn new(
        completion: CommandCompletion,
        parent: Option<Arc<dyn view::callbacks::InvocationTasks>>,
    ) -> Self {
        if let Some(parent) = &parent {
            parent.started();
        }
        Self(Arc::new(Mutex::new(CommandState {
            completion: Some(completion),
            pending: 1,
            error: None,
            cancelled: false,
            accepted: false,
            parent,
        })))
    }

    pub(crate) fn metadata(&self, update: impl FnOnce(&mut CommandCompletion)) {
        if let Some(completion) = &mut self.0.lock().unwrap().completion {
            update(completion);
        }
    }

    pub(crate) fn rebase(&self, editor: &Editor) {
        self.metadata(|completion| completion.rebase(editor));
    }

    pub(crate) fn capture_effects(&self, editor: &Editor) {
        self.metadata(|completion| completion.capture_effects(editor));
    }

    pub(crate) fn mark_accepted(&self) {
        self.0.lock().unwrap().accepted = true;
    }

    pub(crate) fn finish_dispatch(&self, editor: &Editor, error: Option<String>, cancelled: bool) {
        self.capture_effects(editor);
        let outcome = if let Some(error) = error {
            view::callbacks::TaskOutcome::Error(error)
        } else if cancelled {
            view::callbacks::TaskOutcome::Cancelled("command cancelled".into())
        } else {
            view::callbacks::TaskOutcome::Success
        };
        view::callbacks::InvocationTasks::finished(self, outcome);
    }

    pub(crate) fn cancel_pending(&self) {
        let mut state = self.0.lock().unwrap();
        if state.pending != 0 {
            state.cancelled = true;
            state.pending = 0;
        }
    }

    pub(crate) fn publish_if_ready(&self, editor: &Editor) -> bool {
        let (mut completion, error, cancelled, accepted, parent) = {
            let mut state = self.0.lock().unwrap();
            if state.pending != 0 {
                return false;
            }
            let Some(completion) = state.completion.take() else {
                return false;
            };
            (
                completion,
                state.error.take(),
                state.cancelled,
                state.accepted,
                state.parent.take(),
            )
        };
        // Unrelated input/status changes after dispatch belong to other invocations.
        completion.rebase(editor);
        let error = error.or_else(|| completion.error.clone());
        let outcome = if let Some(error) = &error {
            view::callbacks::TaskOutcome::Error(error.clone())
        } else if cancelled {
            view::callbacks::TaskOutcome::Cancelled("command cancelled".into())
        } else {
            view::callbacks::TaskOutcome::Success
        };
        completion.finish_outcome(editor, error, cancelled, accepted);
        if let Some(parent) = parent {
            if accepted {
                parent.detached();
            }
            parent.finished(outcome);
        }
        true
    }

    pub(crate) fn published(&self) -> bool {
        self.0.lock().unwrap().completion.is_none()
    }
}

impl view::callbacks::InvocationTasks for CommandToken {
    fn detached(&self) {
        self.mark_accepted();
    }

    fn started(&self) {
        let mut state = self.0.lock().unwrap();
        if state.completion.is_some() {
            state.pending += 1;
        }
    }
    fn finished(&self, outcome: view::callbacks::TaskOutcome) {
        let mut state = self.0.lock().unwrap();
        match outcome {
            view::callbacks::TaskOutcome::Error(error) => {
                state.error.get_or_insert(error);
            }
            view::callbacks::TaskOutcome::Cancelled(_) => state.cancelled = true,
            view::callbacks::TaskOutcome::Success => (),
        }
        state.pending = state.pending.saturating_sub(1);
    }
}

pub type OnKeyCallback = Box<dyn FnOnce(&mut Context, KeyEvent)>;
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum OnKeyCallbackKind {
    PseudoPending,
    Fallback,
}

pub struct Context<'a> {
    pub config: crate::config::Context<'a>,
    pub register: Option<char>,
    pub count: Option<NonZeroUsize>,
    pub editor: &'a mut Editor,

    pub callback: Vec<crate::compositor::Callback>,
    pub on_next_key_callback: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    pub jobs: &'a mut Jobs,
}

impl Context<'_> {
    pub(super) fn invocation(&self, origin: CommandOrigin) -> CommandInvocation {
        CommandInvocation {
            origin,
            count: self.count.map(NonZeroUsize::get),
            register: self.register,
            custom_command: None,
        }
    }

    /// Finish dispatch after compositor work or a required follow-up key; owned
    /// native tasks still have to settle before the completion can be published.
    pub(super) fn complete_command(
        &mut self,
        completion: CommandToken,
        cancelled: bool,
        callback_start: usize,
    ) {
        completion.capture_effects(self.editor);
        if self.editor.should_close() {
            // EditorView drops presentation callbacks after the last view is
            // closed. The completed close command must still be observable.
            completion.finish_dispatch(self.editor, None, cancelled);
        } else if let Some((callback, kind)) = self.on_next_key_callback.take() {
            self.on_next_key_callback = Some((
                Box::new(move |cx, key| {
                    let scope = cx.jobs.enter_command(cx.editor, completion.clone());
                    let cancelled = cancelled
                        || matches!(
                            key.code,
                            ui_core::keyboard::KeyCode::Esc | ui_core::keyboard::KeyCode::Null
                        );
                    let callback_start = cx.callback.len();
                    callback(cx, key);
                    completion.capture_effects(cx.editor);
                    cx.jobs.leave_command(cx.editor, scope);
                    cx.complete_command(completion, cancelled, callback_start);
                }),
                kind,
            ));
        } else if self.callback.len() > callback_start {
            let callbacks = self.callback.split_off(callback_start);
            self.callback.push(Box::new(move |compositor, cx| {
                let scope = cx.jobs.enter_command(cx.editor, completion.clone());
                for callback in callbacks {
                    callback(compositor, cx);
                }
                completion.capture_effects(cx.editor);
                cx.jobs.leave_command(cx.editor, scope);
                completion.finish_dispatch(cx.editor, None, cancelled);
            }));
        } else {
            completion.finish_dispatch(self.editor, None, cancelled);
        }
    }

    /// Push a new component onto the compositor.
    pub fn push_layer(&mut self, component: Box<dyn Component>) {
        self.callback
            .push(Box::new(|compositor: &mut Compositor, _| {
                compositor.push(component)
            }));
    }

    /// Call `replace_or_push` on the Compositor
    pub fn replace_or_push_layer<T: Component>(&mut self, id: &'static str, component: T) {
        self.callback
            .push(Box::new(move |compositor: &mut Compositor, _| {
                compositor.replace_or_push(id, component);
            }));
    }

    #[inline]
    pub fn on_next_key(
        &mut self,
        on_next_key_callback: impl FnOnce(&mut Context, KeyEvent) + 'static,
    ) {
        self.on_next_key_callback = Some((
            Box::new(on_next_key_callback),
            OnKeyCallbackKind::PseudoPending,
        ));
    }

    #[inline]
    pub fn on_next_key_fallback(
        &mut self,
        on_next_key_callback: impl FnOnce(&mut Context, KeyEvent) + 'static,
    ) {
        self.on_next_key_callback =
            Some((Box::new(on_next_key_callback), OnKeyCallbackKind::Fallback));
    }

    #[inline]
    pub fn callback<T, F>(
        &mut self,
        call: impl Future<Output = lsp_client::Result<T>> + 'static + Send,
        callback: F,
    ) where
        T: Send + 'static,
        F: FnOnce(&mut Editor, &mut Compositor, T) + Send + 'static,
    {
        self.jobs.callback(make_job_callback(call, callback));
    }

    /// Returns 1 if no explicit count was provided
    #[inline]
    pub fn count(&self) -> usize {
        self.count.map_or(1, |v| v.get())
    }

    /// Waits on all pending jobs, and then tries to flush all pending write
    /// operations for all documents.
    pub fn block_try_flush_writes(&mut self) -> anyhow::Result<()> {
        compositor::Context {
            config: self.config,
            editor: self.editor,
            jobs: self.jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        }
        .block_try_flush_writes()
    }
}

#[inline]
pub(super) fn make_job_callback<T, F>(
    call: impl Future<Output = lsp_client::Result<T>> + 'static + Send,
    callback: F,
) -> std::pin::Pin<Box<impl Future<Output = Result<Callback, anyhow::Error>>>>
where
    T: Send + 'static,
    F: FnOnce(&mut Editor, &mut Compositor, T) + Send + 'static,
{
    Box::pin(async move {
        let response = call.await?;
        let call: job::Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                callback(editor, compositor, response)
            },
        ));
        Ok(call)
    })
}

#[cfg(all(test, feature = "integration"))]
#[path = "../../../view/tests/support/plugin_guest.rs"]
mod command_test_guest;

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;
    use crate::{
        application::Application, args::Args, commands::MappableCommand, config::Config, job::Job,
    };

    use super::command_test_guest as guest;

    fn followup(cx: &mut Context) {
        cx.jobs.add(
            Job::with_callback(async {
                Ok(Callback::Followup(Box::new(|_| {
                    Some(
                        Job::with_callback(async {
                            Ok(Callback::Editor(Box::new(|editor| editor.exit_code = 42)))
                        })
                        .wait_before_exiting(),
                    )
                })))
            })
            .wait_before_exiting(),
        );
    }

    fn failure(cx: &mut Context) {
        cx.jobs.add(
            Job::new(async { anyhow::bail!("controlled task failure") }).wait_before_exiting(),
        );
    }

    fn never(cx: &mut Context) {
        cx.jobs.spawn(std::future::pending());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn command_outcomes_wait_for_owned_jobs_and_followups() -> anyhow::Result<()> {
        let _permit = guest::compilation_permit().await;
        for (run, outcome) in [
            (followup as fn(&mut Context), "success"),
            (failure as fn(&mut Context), "error"),
            (never as fn(&mut Context), "cancelled"),
        ] {
            let directory = tempfile::tempdir()?;
            let mut config = Config::default();
            config.editor.lsp.enable = false;
            config.editor.file_watcher.enable = false;
            config.editor.auto_reload.enable = false;
            config.editor.word_completion.enable = false;
            config.plugins.insert(
                "observer".into(),
                guest::observing(
                    directory.path(),
                    &["post-command"],
                    &[guest::Route {
                        event: "post-command",
                        filter: Some("\"command\":\"queued-command\"".into()),
                        response: guest::status("owned result observed"),
                        expected: vec![
                            format!("\"outcome\":\"{outcome}\""),
                            "\"origin\":\"programmatic\"".into(),
                        ],
                    }],
                    Some("post-command"),
                )?,
            );
            let loader = editor_core::syntax::Loader::new(
                toml::from_str("language = []")?,
                loader::syntax::Resources::default(),
            )?;
            let mut app = Application::new(
                Args::default(),
                config.clone(),
                loader,
                loader::workspace_trust::WorkspaceTrust::fully_trusted(),
            )?;
            let mut input = futures_util::stream::pending();
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                app.event_loop_until_idle(&mut input),
            )
            .await?;
            let mut jobs = Jobs::new();
            let (updates, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut cx = Context {
                config: crate::config::Context {
                    current: &config,
                    updates: &updates,
                },
                register: None,
                count: None,
                editor: &mut app.editor,
                callback: Vec::new(),
                on_next_key_callback: None,
                jobs: &mut jobs,
            };
            MappableCommand::Static {
                name: "queued-command",
                fun: run,
                doc: "test command",
            }
            .execute(&mut cx);
            drop(cx);
            app.editor.set_status("still pending");
            jobs.poll_commands(&app.editor);
            app.editor.poll_plugin_events();
            assert_eq!(app.editor.get_status().unwrap().0, "still pending");

            // Status changes outside the owned callback cannot change this result.
            app.editor.set_error(|| "unrelated status");
            if outcome == "cancelled" {
                jobs.cancel_commands(&app.editor);
            } else {
                let result = jobs.finish(&mut app.editor, None).await;
                assert_eq!(result.is_err(), outcome == "error");
                if outcome == "success" {
                    assert_eq!(app.editor.exit_code, 42);
                }
                jobs.poll_commands(&app.editor);
            }
            app.editor.poll_plugin_events();
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                app.event_loop_until_idle(&mut input),
            )
            .await?;
            assert_eq!(app.editor.get_status().unwrap().0, "owned result observed");
            jobs.poll_commands(&app.editor);
            app.editor.poll_plugin_events();
            assert_eq!(app.editor.get_status().unwrap().0, "owned result observed");
            assert!(app.close().await.is_empty());
        }
        Ok(())
    }
}

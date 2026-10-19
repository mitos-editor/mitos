//! Execution commands dispatched against the selected debugger session.
use super::requests::RequestKind;
use super::{DebuggerContext, DebuggerTask};
use crate::Editor;
use anyhow::{bail, Context as _, Result};
use dap::{requests, ThreadId};
use futures_util::future::BoxFuture;
use std::future::Future;

enum ControlEffect {
    ResumeThread,
    ResumeAll,
    Unchanged,
}

fn control_effect<T: Send + 'static>(
    request: impl Future<Output = dap::Result<T>> + Send + 'static,
    effect: ControlEffect,
) -> BoxFuture<'static, dap::Result<ControlEffect>> {
    Box::pin(async move {
        request.await?;
        Ok(effect)
    })
}

#[derive(Clone, Copy)]
pub enum DebuggerCommand {
    Continue,
    StepIn,
    StepOut,
    Next,
    Pause(ThreadId),
    Restart,
    Terminate,
    Exceptions(bool),
}

impl Editor {
    pub fn debugger_evaluate(
        &mut self,
        expression: String,
    ) -> Result<DebuggerTask<requests::EvaluateResponse>> {
        let (context, sender) = self.debugger_context(RequestKind::Evaluate)?;
        let frame_id = context
            .frame
            .context("Cannot find current stack frame to access variables")?;
        let cache = self.debugger_inspection_cache(context.id, sender.clone());
        Ok(DebuggerTask::new(context, async move {
            let response = sender
                .request::<requests::Evaluate>(requests::EvaluateArguments {
                    expression,
                    frame_id: Some(frame_id),
                    context: None,
                    format: None,
                })
                .await?;
            cache.invalidate();
            Ok(response)
        }))
    }

    /// Queue a control request against the active session. Replies cannot mutate a replacement.
    pub fn debugger_command(&mut self, command: DebuggerCommand) -> Result<()> {
        if matches!(command, DebuggerCommand::Terminate)
            && let Some(session) = self.debugger.get_active_session()
            && self.debugger.cancel_startup(session.id())
        {
            return Ok(());
        }
        let session = self
            .debugger
            .get_active_session()
            .context("Debugger is not running")?;
        session.lifecycle.ensure_command_available()?;
        let capabilities = session.capabilities().context("Debugger is initializing")?;
        if matches!(command, DebuggerCommand::Restart) {
            anyhow::ensure!(
                capabilities.supports_restart_request.unwrap_or(false),
                "Debugger does not support session restarts"
            );
            anyhow::ensure!(
                session.launch.is_some(),
                "No arguments found with which to restart the session"
            );
        }
        if matches!(command, DebuggerCommand::Terminate) {
            let id = session.id();
            self.terminate_debugger_session(id);
            return Ok(());
        }
        if let DebuggerCommand::Exceptions(enabled) = command {
            return self.configure_debugger_exceptions(session.id(), enabled);
        }
        let thread = session.selected_thread();
        if matches!(
            command,
            DebuggerCommand::Continue
                | DebuggerCommand::StepIn
                | DebuggerCommand::StepOut
                | DebuggerCommand::Next
        ) && thread.is_none()
        {
            bail!("Currently active thread is not stopped. Switch the thread.");
        }
        let id = session.id();
        let client = session.client.as_ref().context("Debugger is connecting")?;
        let future: BoxFuture<'static, dap::Result<ControlEffect>> = match command {
            DebuggerCommand::Continue => {
                let response = client.continue_thread(thread.unwrap());
                Box::pin(async move {
                    Ok(if response.await?.all_threads_continued.unwrap_or(true) {
                        ControlEffect::ResumeAll
                    } else {
                        ControlEffect::ResumeThread
                    })
                })
            }
            DebuggerCommand::StepIn => {
                control_effect(client.step_in(thread.unwrap()), ControlEffect::ResumeThread)
            }
            DebuggerCommand::StepOut => control_effect(
                client.step_out(thread.unwrap()),
                ControlEffect::ResumeThread,
            ),
            DebuggerCommand::Next => {
                control_effect(client.next(thread.unwrap()), ControlEffect::ResumeThread)
            }
            DebuggerCommand::Pause(thread) => {
                control_effect(client.pause(thread), ControlEffect::Unchanged)
            }
            DebuggerCommand::Restart => control_effect(
                client.restart(session.launch.as_ref().unwrap().arguments.clone()),
                ControlEffect::ResumeAll,
            ),
            DebuggerCommand::Terminate => unreachable!("shutdown handled by the lifecycle owner"),
            DebuggerCommand::Exceptions(_) => {
                unreachable!("configuration has its own request lifetime")
            }
        };
        self.debugger.invalidate_execution(id);
        let (context, _) = self.debugger_context(RequestKind::Control)?;
        let restarting = matches!(command, DebuggerCommand::Restart);
        self.spawn_debugger_task(DebuggerTask::new(context, future), move |editor, effect| {
            if restarting {
                editor.debugger.sessions[id].sources = Default::default();
            }
            match effect {
                ControlEffect::ResumeThread => editor
                    .debugger
                    .get_session_mut(id)
                    .unwrap()
                    .resume_thread(thread.unwrap()),
                ControlEffect::ResumeAll => editor
                    .debugger
                    .get_session_mut(id)
                    .unwrap()
                    .resume_all_threads(),
                ControlEffect::Unchanged => return,
            }
            editor.debugger.invalidate_execution(id);
        });
        Ok(())
    }

    pub fn pause_debugger_thread(
        &mut self,
        context: &DebuggerContext,
        thread: ThreadId,
    ) -> Result<()> {
        if context.is_current(self) {
            self.debugger_command(DebuggerCommand::Pause(thread))
        } else {
            Ok(())
        }
    }
}

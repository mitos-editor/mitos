//! Session settings are serialized independently of execution and frame inspection.

use super::operations::RequestKind;
use super::DebuggerTask;
use crate::Editor;
use anyhow::Result;
use dap::{requests, DebugAdapterId};

#[derive(Default)]
pub(super) struct SessionConfiguration {
    exceptions: Option<bool>,
    pub(super) in_flight: bool,
    dirty: bool,
}

impl Editor {
    pub(super) fn configure_debugger_exceptions(
        &mut self,
        id: DebugAdapterId,
        enabled: bool,
    ) -> Result<()> {
        let state = &mut self.debugger.sessions[id].configuration;
        state.exceptions = Some(enabled);
        if state.in_flight {
            state.dirty = true;
            return Ok(());
        }
        let filters = if enabled {
            self.debugger.sessions[id]
                .capabilities()
                .and_then(|caps| caps.exception_breakpoint_filters.as_ref())
                .map(|filters| filters.iter().map(|filter| filter.filter.clone()).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let (context, sender) = self.debugger_context_for(id, RequestKind::Configuration)?;
        let state = &mut self.debugger.sessions[id].configuration;
        state.in_flight = true;
        state.dirty = false;
        let task = DebuggerTask::new(
            context,
            sender.request::<requests::SetExceptionBreakpoints>(
                requests::SetExceptionBreakpointsArguments { filters },
            ),
        );
        let callbacks = self.callback_sender().clone();
        tokio::spawn(async move {
            let Some(update) = task.run().await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    let Some(result) = update.into_result(editor) else {
                        return;
                    };
                    let state = &mut editor.debugger.sessions[id].configuration;
                    state.in_flight = false;
                    if state.dirty {
                        editor.resume_debugger_configuration(id);
                    } else {
                        if let Err(error) = result {
                            if editor
                                .debugger
                                .get_active_session()
                                .is_some_and(|session| session.id() == id)
                            {
                                editor.set_error(|| {
                                    format!("Could not configure exception breakpoints: {error}")
                                });
                            } else {
                                log::warn!("Exception configuration failed for {id}: {error}");
                            }
                        }
                        editor.finish_breakpoint_configuration(id);
                    }
                })
                .await;
        });
        Ok(())
    }

    pub(super) fn suspend_debugger_configuration(&mut self, id: DebugAdapterId) {
        self.debugger.sessions[id].requests.cancel_configuration();
        self.debugger.sessions[id].configuration.in_flight = false;
    }

    pub(super) fn resume_debugger_configuration(&mut self, id: DebugAdapterId) {
        if let Some(enabled) = self.debugger.sessions[id].configuration.exceptions
            && let Err(error) = self.configure_debugger_exceptions(id, enabled)
        {
            log::warn!("Could not resume debugger configuration: {error}");
        }
    }
}

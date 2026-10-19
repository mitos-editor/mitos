//! Thread listing, frame selection, and trace refresh for frontend pickers.
use super::requests::RequestKind;
use super::{DebuggerContext, DebuggerTask};
use crate::Editor;
use anyhow::{Context as _, Result};
use dap::{requests, DebugAdapterId, ThreadId};

pub struct DebuggerFrames {
    pub context: DebuggerContext,
    pub frames: Vec<(usize, dap::StackFrame)>,
    pub thread_state: String,
    pub has_more: bool,
}

impl Editor {
    /// List threads for a frontend picker without blocking its event loop.
    pub fn debugger_threads(&mut self) -> Result<DebuggerTask<Vec<dap::Thread>>> {
        let (context, sender) = self.debugger_context(RequestKind::Threads)?;
        Ok(DebuggerTask::new(context, async move {
            Ok(sender
                .request::<requests::Threads>(Some(requests::ThreadsArguments {}))
                .await?
                .threads)
        }))
    }

    pub fn select_debugger_thread(
        &mut self,
        context: &DebuggerContext,
        thread: ThreadId,
    ) -> Result<()> {
        if !context.is_current(self) {
            return Ok(());
        }
        self.debugger.invalidate_inspection(context.id);
        let (context, sender) = self.debugger_context(RequestKind::SelectThread)?;
        let id = context.id;
        let paging = self.debugger.sessions[id].stack_paging();
        let task = DebuggerTask::new(context, async move {
            super::stack::request_stack_trace(sender, thread, 0, paging).await
        });
        self.spawn_debugger_task(task, move |editor, response| {
            let session = editor.debugger.get_session_mut(id).unwrap();
            session.execution.select_thread(thread, response);
            let frame = session.current_stack_frame().cloned();
            editor.debugger.invalidate_inspection(id);
            if let Some(frame) = frame {
                editor.navigate_debugger_frame(id, &frame);
            }
        });
        Ok(())
    }

    pub(super) fn refresh_debugger_stack(&mut self, id: DebugAdapterId) {
        let Ok((mut context, sender)) = self.debugger_context_for(id, RequestKind::RefreshStack)
        else {
            return;
        };
        let Some(thread) = context.thread else { return };
        let previous = context.frame;
        self.debugger.sessions[id]
            .requests
            .cancel(RequestKind::SelectThread);
        context.lifetimes.push(self.debugger.inspection_handle(id));
        let session = &self.debugger.sessions[id];
        let paging = session.stack_paging();
        let loaded = session
            .stack_frames()
            .get(&thread)
            .map_or(0, |trace| trace.frames().len());
        let task = DebuggerTask::new(context, async move {
            super::stack::refresh_stack_trace(sender, thread, paging, loaded).await
        });
        self.spawn_debugger_task(task, move |editor, response| {
            let session = &mut editor.debugger.sessions[id];
            // A frame picker holds indices into the previous trace. Revoke it even
            // when the selected frame survives the refresh.
            session.requests.cancel(RequestKind::SelectThread);
            session.execution.refresh_stack_frames(thread, response);
            if session.current_stack_frame().map(|frame| frame.id) != previous {
                editor.debugger.invalidate_inspection(id);
            }
        });
    }

    pub fn debugger_stack_frames(&mut self) -> Result<DebuggerFrames> {
        let (context, _) = self.debugger_context(RequestKind::SelectThread)?;
        self.debugger_frames_snapshot(context)
    }

    pub(super) fn debugger_frames_snapshot(
        &self,
        context: DebuggerContext,
    ) -> Result<DebuggerFrames> {
        let session = self
            .debugger
            .get_session(context.id)
            .context("Debugger is not running")?;
        let thread = session
            .selected_thread()
            .context("No thread is currently active")?;
        let trace = session
            .stack_frames()
            .get(&thread)
            .context("No stack frames for the active thread")?;
        let has_more = trace.has_more();
        let frames = trace.frames().iter().cloned().enumerate().collect();
        let state = session
            .thread_states()
            .get(&thread)
            .map(|state| state.as_str().to_owned())
            .unwrap_or_else(|| "unknown".into());
        Ok(DebuggerFrames {
            context,
            frames,
            thread_state: state,
            has_more,
        })
    }

    pub fn select_debugger_frame(&mut self, context: &DebuggerContext, index: usize) -> Result<()> {
        if !context.is_current(self) {
            return Ok(());
        }
        let session = self.debugger.get_session_mut(context.id).unwrap();
        let frame = session
            .execution
            .select_frame(index)
            .context("Stack frame is no longer available")?
            .clone();
        self.debugger.invalidate_inspection(context.id);
        self.navigate_debugger_frame(context.id, &frame);
        Ok(())
    }
}

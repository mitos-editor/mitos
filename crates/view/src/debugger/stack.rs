//! Bounded traces and guarded continuation of the selected thread's loaded prefix.
use super::{DebugSession, DebuggerContext, DebuggerFrames, DebuggerTask};
use crate::Editor;
use anyhow::{Context as _, Result};
use dap::{requests, RequestSender, ThreadId};

const PAGE_SIZE: usize = 32;

impl DebugSession {
    pub(super) fn stack_paging(&self) -> bool {
        self.capabilities()
            .and_then(|caps| caps.supports_delayed_stack_trace_loading)
            .unwrap_or(false)
    }
}

pub(super) async fn request_stack_trace(
    sender: RequestSender,
    thread: ThreadId,
    start: usize,
    paging: bool,
) -> dap::Result<::debugger::StackFrames> {
    let mut response = sender
        .request::<requests::StackTrace>(requests::StackTraceArguments {
            thread_id: thread,
            start_frame: paging.then_some(start),
            levels: paging.then_some(PAGE_SIZE),
            format: None,
        })
        .await?;
    for source in response
        .stack_frames
        .iter_mut()
        .filter_map(|frame| frame.source.as_mut())
    {
        super::source::normalize_path(source);
    }
    Ok(::debugger::StackFrames::new(
        response.stack_frames,
        response.total_frames,
        start,
        paging.then_some(PAGE_SIZE),
    ))
}

pub(super) async fn refresh_stack_trace(
    sender: RequestSender,
    thread: ThreadId,
    paging: bool,
    loaded: usize,
) -> dap::Result<::debugger::StackFrames> {
    let mut trace = request_stack_trace(sender.clone(), thread, 0, paging).await?;
    while trace.has_more() && trace.frames().len() < loaded {
        let start = trace.frames().len();
        let page = request_stack_trace(sender.clone(), thread, start, paging).await?;
        trace.append(start, page);
    }
    Ok(trace)
}

/// An adapter page can only extend the original session, stop, selection, and prefix.
pub struct DebuggerStackPage {
    context: DebuggerContext,
    thread: ThreadId,
    start: usize,
    frames: ::debugger::StackFrames,
}

impl DebuggerStackPage {
    pub fn apply(self, editor: &mut Editor) -> Option<DebuggerFrames> {
        if !self.context.is_current(editor) {
            return None;
        }
        let session = &mut editor.debugger.sessions[self.context.id];
        if !session
            .execution
            .append_stack_frames(self.thread, self.start, self.frames)
        {
            return None;
        }
        editor.debugger_frames_snapshot(self.context).ok()
    }
}

impl Editor {
    pub fn debugger_more_stack_frames(
        &self,
        frames: &DebuggerFrames,
    ) -> Result<DebuggerTask<DebuggerStackPage>> {
        anyhow::ensure!(
            frames.context.is_current(self),
            "Debugger frames are no longer current"
        );
        anyhow::ensure!(frames.has_more, "All stack frames are already loaded");
        let session = &self.debugger.sessions[frames.context.id];
        let thread = session
            .selected_thread()
            .context("No thread is currently active")?;
        let sender = session
            .client
            .as_ref()
            .context("Debugger is connecting")?
            .request_sender();
        let context = frames.context.clone();
        let start = frames.frames.len();
        Ok(DebuggerTask::new(context.clone(), async move {
            let frames = request_stack_trace(sender, thread, start, true).await?;
            Ok(DebuggerStackPage {
                context,
                thread,
                start,
                frames,
            })
        }))
    }
}

//! Terminal command context, next-key callbacks, and job/compositor adapters.

use std::{future::Future, num::NonZeroUsize};

use ui_core::input::KeyEvent;
use view::Editor;

use crate::{
    compositor::{self, Component, Compositor},
    job::{self, Callback, Jobs},
};

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

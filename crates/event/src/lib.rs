//! `event` contains systems that allow (often async) communication between
//! different editor components without strongly coupling them. Specifically
//! it allows defining synchronous hooks that run when certain editor events
//! occur.
//!
//! The core of the event system is hook callbacks and the [`Event`] trait. A
//! hook is essentially just a closure `Fn(event: &mut impl Event) -> Result<()>`
//! that gets called every time an appropriate event is dispatched. The implementation
//! details of the [`Event`] trait are considered private. The [`events`] macro is
//! provided which automatically declares event types. Similarly the `register_hook`
//! macro should be used to (safely) declare event hooks.
//!
//! Hooks run synchronously which can be advantageous since they can modify the
//! current editor state right away (for example to immediately hide the completion
//! popup). However, they cannot contain their own state without locking since
//! they only receive immutable references. For handlers that want to track state, do
//! expensive background computations, or debounce, an [`AsyncHook`] is preferable.
//! Async hooks are based around channels that receive events specific to
//! that `AsyncHook` (usually an enum). These events can be sent by synchronous
//! hooks. Due to some limitations around tokio channels the [`send_blocking`]
//! function exported in this crate should be used instead of the builtin
//! `blocking_send`.
//!
//! In addition to the core event system, this crate contains some message queues
//! that allow transfer of data back to the main event loop from async hooks and
//! hooks that may not have access to all application data (for example in view).
//! This includes the ability to control rendering ([`Redraw::lock_frame`], [`Redraw::request`]) and
//! display status messages ([`status`]).
//!
//! Hooks that need to update a frontend capture its explicit callback destination.
//! Shared editor services use `view::callbacks::EditorCallbackSender`; terminal
//! presentation uses `term::job::JobSender`, bound to the owning application.

#![forbid(unsafe_code)]

use anyhow::Result;
pub use cancel::{cancelable_future, TaskController, TaskHandle};
pub use debounce::{send_blocking, AsyncHook};
pub use redraw::{Redraw, RenderLockGuard, RequestRedrawOnDrop};
pub use registry::{Event, HookRegistration};

#[doc(hidden)]
pub type HookResult = Result<()>;

mod cancel;
mod debounce;
mod redraw;
mod registry;
#[doc(hidden)]
pub mod runtime;
pub mod status;

#[cfg(test)]
mod test;

/// Registers an event type before hooks for that type are installed.
///
/// Built-in hooks are installed during startup. Owned subscriptions may also be
/// registered and removed during event dispatch.
pub fn register_event<E: Event + 'static>() {
    registry::register_event::<E>()
}

/// Registers a permanent hook. Prefer [`register_hook!`] to infer the event's
/// higher-ranked callback type from its argument.
pub fn register_hook<E: Event>(hook: Box<E::Hook>) {
    registry::register::<E>(hook);
}

/// Registers a typed hook owned by its returned subscription.
/// Prefer [`register_owned_hook!`] for closure type inference.
pub fn register_owned_hook<E: Event>(hook: Box<E::Hook>) -> HookRegistration {
    registry::register_owned::<E>(hook)
}

/// Registers a callback for an event identified only by its dynamic name.
/// Retain the returned registration for as long as the callback should run.
pub fn register_dynamic_hook(
    hook: impl Fn() -> Result<()> + 'static + Send + Sync,
    id: &str,
) -> Result<HookRegistration> {
    registry::register_dynamic(hook, id)
}

/// Dispatches an event synchronously to every registered hook of its type.
///
/// The caller reports failures to its owning editor after the event's mutable
/// borrows end. Call [`HookErrors::log`] when no status UI exists.
pub fn dispatch(e: impl Event) -> HookErrors {
    registry::dispatch(e)
}

/// Failures from synchronous observers, delivered after every hook has run.
#[must_use = "report hook errors to the owning editor, or explicitly log them"]
pub struct HookErrors {
    event: &'static str,
    errors: Vec<anyhow::Error>,
}

impl HookErrors {
    pub fn report_to(self, status: &status::StatusSender) {
        for error in self.errors {
            log::error!("{} hook failed: {error:#?}", self.event);
            status.report_blocking(error);
        }
    }

    /// Log failures for standalone operations with no status destination.
    pub fn log(self) {
        for error in self.errors {
            log::error!("{} hook failed: {error:#?}", self.event);
        }
    }
}

/// Macro to declare events
///
/// # Examples
///
/// ``` no-compile
/// events! {
///     FileWrite(&Path)
///     ViewScrolled{ view: View, new_pos: ViewOffset }
///     DocumentChanged<'a> { old_doc: &'a Rope, doc: &'a mut Document, changes: &'a ChangeSet  }
/// }
///
/// fn init() {
///    register_event::<FileWrite>();
///    register_event::<ViewScrolled>();
///    register_event::<DocumentChanged>();
/// }
///
/// fn save(path: &Path, content: &str){
///     std::fs::write(path, content);
///     dispatch(FileWrite(path)).log();
/// }
/// ```
#[macro_export]
macro_rules! events {
    ($name: ident<$($lt: lifetime),*> { $($data:ident : $data_ty:ty),* } $($rem:tt)*) => {
        pub struct $name<$($lt),*> { $(pub $data: $data_ty),* }
        impl $crate::Event for $crate::events!(@replace_lt $name, $('_, $lt),*) {
            const ID: &'static str = stringify!($name);
            type Static = $crate::events!(@replace_lt $name, $('static, $lt),*);
            type Hook = dyn for<$($lt),*> Fn(&mut $name<$($lt),*>) -> $crate::HookResult + Send + Sync;
            fn call(&mut self, hook: &Self::Hook) -> $crate::HookResult { hook(self) }
        }
        $crate::events!{ $($rem)* }
    };
    ($name: ident { $($data:ident : $data_ty:ty),* } $($rem:tt)*) => {
        pub struct $name { $(pub $data: $data_ty),* }
        impl $crate::Event for $name {
            const ID: &'static str = stringify!($name);
            type Static = Self;
            type Hook = dyn Fn(&mut $name) -> $crate::HookResult + Send + Sync;
            fn call(&mut self, hook: &Self::Hook) -> $crate::HookResult { hook(self) }
        }
        $crate::events!{ $($rem)* }
    };
    () => {};
    (@replace_lt $name: ident, $($lt1: lifetime, $lt2: lifetime),* ) => {$name<$($lt1),*>};
}

/// Register a permanent typed hook. The event's higher-ranked callback type
/// checks borrowed lifetimes at compile time, including through type aliases.
///
/// ```compile_fail
/// event::events! { Borrowed<'a> { text: &'a str } }
/// type StaticEvent = Borrowed<'static>;
/// event::register_hook!(move |event: &mut StaticEvent| {
///     let _: &'static str = event.text;
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! register_hook {
    (move |$event:ident: &mut $event_ty:ty| $body:expr) => {
        $crate::register_hook::<$event_ty>(::std::boxed::Box::new(move |$event: &mut $event_ty| {
            $body
        }))
    };
}

/// Register a typed hook owned by the returned [`HookRegistration`].
#[macro_export]
macro_rules! register_owned_hook {
    (move |$event:ident: &mut $event_ty:ty| $body:expr) => {
        $crate::register_owned_hook::<$event_ty>(::std::boxed::Box::new(
            move |$event: &mut $event_ty| $body,
        ))
    };
}

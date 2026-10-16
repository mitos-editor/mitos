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

use anyhow::Result;
pub use cancel::{cancelable_future, TaskController, TaskHandle};
pub use debounce::{send_blocking, AsyncHook};
pub use redraw::{Redraw, RenderLockGuard, RequestRedrawOnDrop};
pub use registry::{Event, HookRegistration};

mod cancel;
mod debounce;
mod hook;
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
    registry::with_mut(|registry| registry.register_event::<E>())
}

/// Registers a hook that will be called when an event of type `E` is dispatched.
/// This function should usually not be used directly, use the [`register_hook`]
/// macro instead.
///
///
/// # Safety
///
/// `hook` must be totally generic over all lifetime parameters of `E`. For
/// example if `E` was a known type `Foo<'a, 'b>`, then the correct trait bound
/// would be `F: for<'a, 'b, 'c> Fn(&'a mut Foo<'b, 'c>)`, but there is no way to
/// express that kind of constraint for a generic type with the Rust type system
/// as of this writing.
pub unsafe fn register_hook_raw<E: Event>(
    hook: impl Fn(&mut E) -> Result<()> + 'static + Send + Sync,
) {
    unsafe { registry::register(hook) };
}

/// Registers an owned, statically typed hook. Prefer [`register_owned_hook!`].
///
/// # Safety
/// The callback must obey the same lifetime requirements as [`register_hook_raw`].
pub unsafe fn register_owned_hook_raw<E: Event>(
    hook: impl Fn(&mut E) -> Result<()> + 'static + Send + Sync,
) -> HookRegistration {
    unsafe { registry::register_owned(hook) }
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
        unsafe impl<$($lt),*> $crate::Event for $name<$($lt),*> {
            const ID: &'static str = stringify!($name);
            const LIFETIMES: usize = $crate::events!(@sum $(1, $lt),*);
            type Static = $crate::events!(@replace_lt $name, $('static, $lt),*);
        }
        $crate::events!{ $($rem)* }
    };
    ($name: ident { $($data:ident : $data_ty:ty),* } $($rem:tt)*) => {
        pub struct $name { $(pub $data: $data_ty),* }
        unsafe impl $crate::Event for $name {
            const ID: &'static str = stringify!($name);
            const LIFETIMES: usize = 0;
            type Static = Self;
        }
        $crate::events!{ $($rem)* }
    };
    () => {};
    (@replace_lt $name: ident, $($lt1: lifetime, $lt2: lifetime),* ) => {$name<$($lt1),*>};
    (@sum $($val: expr, $lt1: lifetime),* ) => {0 $(+ $val)*};
}

/// Safely register statically typed event hooks
#[macro_export]
macro_rules! register_hook {
    (move |$($args:tt)*) => {
        $crate::register_hook!(@register register_hook_raw move |$($args)*)
    };
    // Safety: this is safe because we fully control the type of the event here and
    // ensure all lifetime arguments are fully generic and the correct number of lifetime arguments
    // is present
    (@register $register:ident move |$event:ident: &mut $event_ty: ident<$($lt: lifetime),*>| $body: expr) => {{
        let val = move |$event: &mut $event_ty<$($lt),*>| $body;
        unsafe {
            // Lifetimes are a bit of a pain. We want to allow events being
            // non-static. Lifetimes don't actually exist at runtime so its
            // fine to essentially transmute the lifetimes as long as we can
            // prove soundness. The hook must therefore accept any combination
            // of lifetimes. In other words fn(&'_ mut Event<'_, '_>) is ok
            // but examples like fn(&'_ mut Event<'_, 'static>) or fn<'a>(&'a
            // mut Event<'a, 'a>) are not. To make this safe we use a macro to
            // forbid the user from specifying lifetimes manually (all lifetimes
            // specified are always function generics and passed to the event so
            // lifetimes can't be used multiple times and using 'static causes a
            // syntax error).
            //
            // There is one soundness hole tough: Type Aliases allow
            // "accidentally" creating these problems. For example:
            //
            // type Event2  = Event<'static>.
            // type Event2<'a>  = Event<'a, a>.
            //
            // These cases can be caught by counting the number of lifetimes
            // parameters at the parameter declaration site and then at the hook
            // declaration site. By asserting the number of lifetime parameters
            // are equal we can catch all bad type aliases under one assumption:
            // There are no unused lifetime parameters. Introducing a static
            // would reduce the number of arguments of the alias by one in the
            // above example Event2 has zero lifetime arguments while the original
            // event has one lifetime argument. Similar logic applies to using
            // a lifetime argument multiple times. The ASSERT below performs a
            // a compile time assertion to ensure exactly this property.
            //
            // With unused lifetime arguments it is still one way to cause unsound code:
            //
            // type Event2<'a, 'b> = Event<'a, 'a>;
            //
            // However, this case will always emit a compiler warning/cause CI
            // failures so a user would have to introduce #[allow(unused)] which
            // is easily caught in review (and a very theoretical case anyway).
            // If we want to be pedantic we can simply compile mitos with
            // forbid(unused). All of this is just a safety net to prevent
            // very theoretical misuse. This won't come up in real code (and is
            // easily caught in review).
            #[allow(unused)]
            const ASSERT: () = {
                if <$event_ty as $crate::Event>::LIFETIMES != 0 + $crate::events!(@sum $(1, $lt),*){
                    panic!("invalid type alias");
                }
            };
            $crate::$register::<$crate::events!(@replace_lt $event_ty, $('static, $lt),*)>(val)
        }
    }};
    (@register $register:ident move |$event:ident: &mut $event_ty: ident| $body: expr) => {{
        let val = move |$event: &mut $event_ty| $body;
        unsafe {
            #[allow(unused)]
            const ASSERT: () = {
                if <$event_ty as $crate::Event>::LIFETIMES != 0{
                    panic!("invalid type alias");
                }
            };
            $crate::$register::<$event_ty>(val)
        }
    }};
}

/// Register a typed hook owned by the returned [`HookRegistration`].
#[macro_export]
macro_rules! register_owned_hook {
    (move |$($args:tt)*) => {
        $crate::register_hook!(@register register_owned_hook_raw move |$($args)*)
    };
}

//! rust dynamic dispatch is extremely limited so we have to build our
//! own vtable implementation. Otherwise implementing the event system would not be possible.
//! The erased allocation retains its destructor so owned registrations release captured state.

use anyhow::Result;
use std::ptr::{self, NonNull};

use crate::Event;

/// Opaque handle type that represents an erased type parameter.
///
/// If extern types were stable, this could be implemented as `extern { pub type Opaque; }` but
/// until then we can use this.
///
/// Care should be taken that we don't use a concrete instance of this. It should only be used
/// through a reference, so we can maintain something else's lifetime.
struct Opaque(());

pub(crate) struct ErasedHook {
    data: NonNull<Opaque>,
    call: unsafe fn(NonNull<Opaque>, NonNull<Opaque>, NonNull<Opaque>),
    drop: unsafe fn(NonNull<Opaque>),
}

impl ErasedHook {
    pub(crate) fn new_dynamic<H: Fn() -> Result<()> + 'static + Send + Sync>(
        hook: H,
    ) -> ErasedHook {
        unsafe fn call<F: Fn() -> Result<()> + 'static + Send + Sync>(
            hook: NonNull<Opaque>,
            _event: NonNull<Opaque>,
            result: NonNull<Opaque>,
        ) {
            unsafe {
                let hook: NonNull<F> = hook.cast();
                let result: NonNull<Result<()>> = result.cast();
                let hook: &F = hook.as_ref();
                let res = hook();
                ptr::write(result.as_ptr(), res)
            }
        }

        unsafe {
            ErasedHook {
                data: NonNull::new_unchecked(Box::into_raw(Box::new(hook)) as *mut Opaque),
                call: call::<H>,
                drop: drop_hook::<H>,
            }
        }
    }

    pub(crate) fn new<E: Event, F: Fn(&mut E) -> Result<()> + Send + Sync + 'static>(
        hook: F,
    ) -> ErasedHook {
        unsafe fn call<E: Event, F: Fn(&mut E) -> Result<()>>(
            hook: NonNull<Opaque>,
            event: NonNull<Opaque>,
            result: NonNull<Opaque>,
        ) {
            unsafe {
                let hook: NonNull<F> = hook.cast();
                let mut event: NonNull<E> = event.cast();
                let result: NonNull<Result<()>> = result.cast();
                let hook: &F = hook.as_ref();
                let res = hook(event.as_mut());
                ptr::write(result.as_ptr(), res)
            }
        }

        unsafe {
            ErasedHook {
                data: NonNull::new_unchecked(Box::into_raw(Box::new(hook)) as *mut Opaque),
                call: call::<E, F>,
                drop: drop_hook::<F>,
            }
        }
    }

    pub(crate) unsafe fn call<E: Event>(&self, event: &mut E) -> Result<()> {
        let mut res = Ok(());

        unsafe {
            (self.call)(
                self.data,
                NonNull::from(event).cast(),
                NonNull::from(&mut res).cast(),
            );
        }
        res
    }
}

unsafe fn drop_hook<F>(hook: NonNull<Opaque>) {
    unsafe { drop(Box::from_raw(hook.cast::<F>().as_ptr())) };
}

impl Drop for ErasedHook {
    fn drop(&mut self) {
        unsafe { (self.drop)(self.data) };
    }
}

// Both constructors require Send + Sync captures; erased calls only borrow them.
unsafe impl Sync for ErasedHook {}
unsafe impl Send for ErasedHook {}

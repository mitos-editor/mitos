//! Explicitly owned typed hook lists. Dispatch snapshots callbacks before running user code.

use std::{
    any::{Any, TypeId},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::Result;
use hashbrown::HashMap;
use parking_lot::RwLock;

type HookSnapshot<H> = Arc<Vec<Arc<Hook<H>>>>;

struct Hook<H: ?Sized> {
    callback: Box<H>,
    active: AtomicBool,
}

struct Hooks<H: ?Sized> {
    callbacks: HookSnapshot<H>,
}

impl<H: ?Sized> Default for Hooks<H> {
    fn default() -> Self {
        Self {
            callbacks: Arc::new(Vec::new()),
        }
    }
}

type Registry = HashMap<TypeId, Box<dyn Any + Send + Sync>>;

/// Synchronous hooks belonging to one editor. Clones retain the same destination,
/// independently of threads and Tokio runtimes.
#[derive(Clone, Default)]
pub struct EventHub(Arc<RwLock<Registry>>);

impl EventHub {
    fn insert<E: Event>(&self, hook: Arc<Hook<E::Hook>>) {
        let mut registry = self.0.write();
        let hooks = registry
            .entry(TypeId::of::<E::Static>())
            .or_insert_with(|| Box::new(Hooks::<E::Hook>::default()))
            .downcast_mut::<Hooks<E::Hook>>()
            .expect("event hook type mismatch");
        Arc::make_mut(&mut hooks.callbacks).push(hook);
    }

    /// Register a permanent hook. Prefer [`crate::register_hook!`] for borrowed event inference.
    pub fn register_hook<E: Event>(&self, callback: Box<E::Hook>) {
        // Keep a reference outside the lock: failed insertion must not drop user captures inside it.
        let hook = Arc::new(Hook {
            callback,
            active: AtomicBool::new(true),
        });
        self.insert::<E>(hook.clone());
    }

    /// Register a hook until the returned subscription is dropped.
    pub fn register_owned_hook<E: Event>(&self, callback: Box<E::Hook>) -> HookRegistration {
        let hook = Arc::new(Hook {
            callback,
            active: AtomicBool::new(true),
        });
        self.insert::<E>(hook.clone());
        let registry = Arc::downgrade(&self.0);
        HookRegistration(Some(Box::new(move || {
            hook.active.store(false, Ordering::Release);
            if let Some(registry) = registry.upgrade() {
                let mut registry = registry.write();
                let hooks = registry
                    .get_mut(&TypeId::of::<E::Static>())
                    .and_then(|hooks| hooks.downcast_mut::<Hooks<E::Hook>>())
                    .expect("registered event hook list");
                Arc::make_mut(&mut hooks.callbacks)
                    .retain(|registered| !Arc::ptr_eq(registered, &hook));
            }
            // The closure retains the hook until the lock is released. Dropping captures
            // may unregister other subscriptions or dispatch another event.
        })))
    }

    /// Run every active observer in registration order, accumulating failures.
    /// Hooks added during dispatch first participate in the next dispatch.
    pub fn dispatch<E: Event>(&self, mut event: E) -> crate::HookErrors {
        let hooks = self
            .0
            .read()
            .get(&TypeId::of::<E::Static>())
            .map(|hooks| {
                hooks
                    .downcast_ref::<Hooks<E::Hook>>()
                    .expect("event hook type mismatch")
                    .callbacks
                    .clone()
            })
            .unwrap_or_default();
        let mut errors = Vec::new();
        for hook in hooks.iter() {
            if hook.active.load(Ordering::Acquire)
                && let Err(error) = event.call(&hook.callback)
            {
                errors.push(error);
            }
        }
        crate::HookErrors {
            event: E::ID,
            errors,
        }
    }
}

/// An event's callback accepts every combination of its borrowed lifetimes.
/// [`crate::events!`] supplies the higher-ranked type; aliases cannot narrow it.
pub trait Event: Sized {
    const ID: &'static str;
    type Static: Event<Hook = Self::Hook> + 'static;
    type Hook: ?Sized + Send + Sync + 'static;
    fn call(&mut self, hook: &Self::Hook) -> Result<()>;
}

/// Dropping a subscription prevents later calls and releases captures once any
/// callback already running finishes. Removal is safe during dispatch.
#[must_use = "dropping the registration immediately unsubscribes its hook"]
pub struct HookRegistration(Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for HookRegistration {
    fn drop(&mut self) {
        if let Some(unsubscribe) = self.0.take() {
            unsubscribe();
        }
    }
}

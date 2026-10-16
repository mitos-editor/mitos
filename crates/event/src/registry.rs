//! A global registry where events are registered and can be
//! subscribed to by registering hooks. The registry identifies event
//! types using their type name so multiple event with the same type name
//! may not be registered (will cause a panic to ensure soundness)

use std::{
    any::TypeId,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, Weak,
    },
};

use anyhow::{bail, Result};
use hashbrown::hash_map::Entry;
use hashbrown::HashMap;
use parking_lot::RwLock;

use crate::hook::ErasedHook;
use crate::runtime_local;

type Hooks = Arc<Vec<Arc<Hook>>>;

pub struct Registry {
    events: HashMap<&'static str, TypeId, foldhash::fast::FixedState>,
    handlers: HashMap<&'static str, Hooks, foldhash::fast::FixedState>,
}

pub(crate) struct Hook {
    callback: ErasedHook,
    active: AtomicBool,
}

impl Hook {
    fn new(callback: ErasedHook) -> Arc<Self> {
        Arc::new(Self {
            callback,
            active: AtomicBool::new(true),
        })
    }
}

/// Owns an event subscription. Dropping it removes the hook from future dispatches.
/// A callback already selected by an in-progress dispatch may finish; its captures
/// are released when no dispatch retains it.
///
/// Dispatch snapshots hooks in registration order and checks whether each hook
/// is still active before calling it. New subscriptions take effect on the next
/// dispatch.
#[must_use = "dropping the registration immediately unsubscribes its hook"]
pub struct HookRegistration {
    registry: Weak<RwLock<Registry>>,
    event: &'static str,
    hook: Arc<Hook>,
}

impl Drop for HookRegistration {
    fn drop(&mut self) {
        self.hook.active.store(false, Ordering::Release);
        if let Some(registry) = self.registry.upgrade() {
            Arc::make_mut(registry.write().handlers.get_mut(self.event).unwrap())
                .retain(|hook| !Arc::ptr_eq(hook, &self.hook));
        }
        // The registration retains the callback until after the registry lock
        // is released, so destructors may safely register or remove other hooks.
    }
}

impl Registry {
    pub fn register_event<E: Event + 'static>(&mut self) {
        let ty = TypeId::of::<E>();
        assert_eq!(ty, TypeId::of::<E::Static>());
        match self.events.entry(E::ID) {
            Entry::Occupied(entry) => {
                if entry.get() == &ty {
                    // don't warn during tests to avoid log spam
                    #[cfg(not(feature = "integration_test"))]
                    panic!("Event {} was registered multiple times", E::ID);
                } else {
                    panic!("Multiple events with ID {} were registered", E::ID);
                }
            }
            Entry::Vacant(ent) => {
                ent.insert(ty);
                self.handlers.insert(E::ID, Arc::new(Vec::new()));
            }
        }
    }

    /// # Safety
    /// The erased callback must accept `E`, generically over its lifetimes.
    unsafe fn insert_hook<E: Event>(&mut self, hook: Arc<Hook>) {
        // ensure event type ids match so we can rely on them always matching
        let id = E::ID;
        let Some(&event_id) = self.events.get(id) else {
            panic!("Tried to register handler for unknown event {id}");
        };
        assert!(
            TypeId::of::<E::Static>() == event_id,
            "Tried to register invalid hook for event {id}"
        );
        Arc::make_mut(self.handlers.get_mut(id).unwrap()).push(hook);
    }

    fn snapshot<E: Event>(&self) -> Hooks {
        let Some(hooks) = self.handlers.get(E::ID) else {
            log::error!("Dispatched unknown event {}", E::ID);
            return Arc::default();
        };
        assert_eq!(
            TypeId::of::<E::Static>(),
            self.events[E::ID],
            "Tried to dispatch invalid event {}",
            E::ID
        );
        hooks.clone()
    }
}

runtime_local! {
    static REGISTRY: LazyLock<Arc<RwLock<Registry>>> = LazyLock::new(|| Arc::new(RwLock::new(Registry {
        // hardcoded random number is good enough here we don't care about DOS resistance
        // and avoids the additional complexity of `Option<Registry>`
        events: HashMap::with_hasher(foldhash::fast::FixedState::with_seed(72536814787)),
        handlers: HashMap::with_hasher(foldhash::fast::FixedState::with_seed(72536814787)),
    })));
}

pub(crate) fn with_mut<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    f(&mut REGISTRY.write())
}

pub(crate) unsafe fn register<E: Event>(
    hook: impl Fn(&mut E) -> Result<()> + 'static + Send + Sync,
) -> Arc<Hook> {
    // Keep an owner outside the lock while validating/inserting. If registration
    // panics, captured subscriptions must be dropped only after the guard unwinds.
    let hook = Hook::new(ErasedHook::new(hook));
    unsafe { REGISTRY.write().insert_hook::<E>(hook.clone()) };
    hook
}

pub(crate) unsafe fn register_owned<E: Event>(
    hook: impl Fn(&mut E) -> Result<()> + 'static + Send + Sync,
) -> HookRegistration {
    let hook = unsafe { register(hook) };
    HookRegistration {
        registry: Arc::downgrade(&REGISTRY),
        event: E::ID,
        hook,
    }
}

pub(crate) fn register_dynamic(
    hook: impl Fn() -> Result<()> + 'static + Send + Sync,
    id: &str,
) -> Result<HookRegistration> {
    let event = REGISTRY.read().events.get_key_value(id).map(|(&id, _)| id);
    let Some(event) = event else {
        bail!("Tried to register handler for unknown event {id}");
    };
    // Construct/drop user-owned captures outside the registry lock, including
    // the unknown-event error path above.
    let hook = Hook::new(ErasedHook::new_dynamic(hook));
    Arc::make_mut(REGISTRY.write().handlers.get_mut(event).unwrap()).push(hook.clone());
    Ok(HookRegistration {
        registry: Arc::downgrade(&REGISTRY),
        event,
        hook,
    })
}

pub(crate) fn dispatch<E: Event>(mut event: E) {
    // Never hold the registry lock while calling user code. Hooks may dispatch
    // recursively, subscribe, or remove their own registration.
    let hooks = REGISTRY.read().snapshot::<E>();
    for hook in hooks.iter() {
        if hook.active.load(Ordering::Acquire) {
            // Safety: snapshot validates the event's static type before erasure.
            if let Err(err) = unsafe { hook.callback.call(&mut event) } {
                log::error!("{} hook failed: {err:#?}", E::ID);
                crate::status::report_blocking(err);
            }
        }
    }
}

/// # Safety
/// The number of specified lifetimes and the static type *must* be correct.
/// This is ensured automatically by the [`events`](crate::events)
/// macro.
pub unsafe trait Event: Sized {
    /// Globally unique (case sensitive)  string that identifies this type.
    /// A good candidate is the events type name
    const ID: &'static str;
    const LIFETIMES: usize;
    type Static: Event + 'static;
}

//! Typed hook lists selected by event identity. Only the list is type-erased;
//! dispatch and borrowed event arguments remain statically typed.

use std::{
    any::{Any, TypeId},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock,
    },
};

use anyhow::{bail, Result};
use hashbrown::{hash_map::Entry, HashMap};
use parking_lot::RwLock;

use crate::runtime_local;

type DynamicHook = Arc<dyn Fn() -> Result<()> + Send + Sync>;

// Dynamic hooks carry no event reference, so they can share the typed list
// without erasing any argument lifetime.
enum Callback<H: ?Sized> {
    Typed(Box<H>),
    Dynamic(DynamicHook),
}

struct Hook<H: ?Sized> {
    callback: Callback<H>,
    active: AtomicBool,
}

impl<H: ?Sized> Hook<H> {
    fn new(callback: Callback<H>) -> Arc<Self> {
        Arc::new(Self {
            callback,
            active: AtomicBool::new(true),
        })
    }
}

type HookSnapshot<H> = Arc<Vec<Arc<Hook<H>>>>;

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

impl<H: ?Sized> Hooks<H> {
    fn insert(&mut self, hook: Arc<Hook<H>>) {
        Arc::make_mut(&mut self.callbacks).push(hook);
    }
}

trait RegisteredHooks: Any + Send + Sync {
    fn insert_dynamic(&mut self, event: &'static str, callback: DynamicHook) -> HookRegistration;
}

impl<H: ?Sized + Send + Sync + 'static> RegisteredHooks for Hooks<H> {
    fn insert_dynamic(&mut self, event: &'static str, callback: DynamicHook) -> HookRegistration {
        let hook = Hook::new(Callback::Dynamic(callback));
        self.insert(hook.clone());
        subscription(event, hook)
    }
}

struct RegisteredEvent {
    ty: TypeId,
    hooks: Box<dyn RegisteredHooks>,
}

pub(crate) struct Registry {
    events: HashMap<&'static str, RegisteredEvent, foldhash::fast::FixedState>,
}

impl Registry {
    pub(crate) fn register_event<E: Event>(&mut self) {
        let ty = TypeId::of::<E::Static>();
        match self.events.entry(E::ID) {
            Entry::Occupied(entry) => {
                assert_eq!(entry.get().ty, ty, "Multiple events with ID {}", E::ID);
                #[cfg(not(feature = "integration_test"))]
                panic!("Event {} was registered multiple times", E::ID);
            }
            Entry::Vacant(entry) => {
                entry.insert(RegisteredEvent {
                    ty,
                    hooks: Box::new(Hooks::<E::Hook>::default()),
                });
            }
        }
    }

    fn snapshot<E: Event>(&self) -> HookSnapshot<E::Hook> {
        let Some(event) = self.events.get(E::ID) else {
            log::error!("Dispatched unknown event {}", E::ID);
            return Arc::default();
        };
        assert_eq!(
            event.ty,
            TypeId::of::<E::Static>(),
            "Invalid event {}",
            E::ID
        );
        let hooks: &dyn Any = &*event.hooks;
        hooks
            .downcast_ref::<Hooks<E::Hook>>()
            .expect("event hook type mismatch")
            .callbacks
            .clone()
    }

    fn hooks_mut<H: ?Sized + Send + Sync + 'static>(&mut self, id: &str) -> &mut Hooks<H> {
        let event = self
            .events
            .get_mut(id)
            .expect("Tried to register handler for unknown event");
        let hooks: &mut dyn Any = &mut *event.hooks;
        hooks.downcast_mut().expect("event hook type mismatch")
    }
}

runtime_local! {
    static REGISTRY: LazyLock<Arc<RwLock<Registry>>> = LazyLock::new(|| Arc::new(RwLock::new(Registry {
        events: HashMap::with_hasher(foldhash::fast::FixedState::with_seed(72536814787)),
    })));
}

pub(crate) fn register_event<E: Event>() {
    REGISTRY.write().register_event::<E>();
}

pub(crate) fn register<E: Event>(callback: Box<E::Hook>) {
    // Retain the hook outside the lock while validating/inserting so failed
    // registration releases user captures after the lock unwinds.
    let hook = Hook::new(Callback::Typed(callback));
    REGISTRY
        .write()
        .hooks_mut::<E::Hook>(E::ID)
        .insert(hook.clone());
}

pub(crate) fn register_owned<E: Event>(callback: Box<E::Hook>) -> HookRegistration {
    let hook = Hook::new(Callback::Typed(callback));
    REGISTRY
        .write()
        .hooks_mut::<E::Hook>(E::ID)
        .insert(hook.clone());
    subscription(E::ID, hook)
}

fn subscription<H: ?Sized + Send + Sync + 'static>(
    event: &'static str,
    hook: Arc<Hook<H>>,
) -> HookRegistration {
    let registry = Arc::downgrade(&REGISTRY);
    HookRegistration(Some(Box::new(move || {
        hook.active.store(false, Ordering::Release);
        if let Some(registry) = registry.upgrade() {
            Arc::make_mut(&mut registry.write().hooks_mut::<H>(event).callbacks)
                .retain(|registered| !Arc::ptr_eq(registered, &hook));
        }
        // The closure retains the hook until after the lock is released, so
        // dropping captures may safely unregister other subscriptions.
    })))
}

pub(crate) fn register_dynamic(
    hook: impl Fn() -> Result<()> + 'static + Send + Sync,
    id: &str,
) -> Result<HookRegistration> {
    let callback: DynamicHook = Arc::new(hook);
    let mut registry = REGISTRY.write();
    let Some((&event, _)) = registry.events.get_key_value(id) else {
        bail!("Tried to register handler for unknown event {id}");
    };
    Ok(registry
        .events
        .get_mut(event)
        .unwrap()
        .hooks
        .insert_dynamic(event, callback.clone()))
}

pub(crate) fn dispatch<E: Event>(mut event: E) -> crate::HookErrors {
    let mut errors = Vec::new();
    // Take the typed snapshot under one read lock and release it before user code.
    // Removed hooks are skipped; new hooks participate in later dispatches.
    let hooks = REGISTRY.read().snapshot::<E>();
    for hook in hooks.iter() {
        if hook.active.load(Ordering::Acquire) {
            let result = match &hook.callback {
                Callback::Typed(callback) => event.call(callback),
                Callback::Dynamic(callback) => callback(),
            };
            if let Err(error) = result {
                errors.push(error);
            }
        }
    }
    crate::HookErrors {
        event: E::ID,
        errors,
    }
}

/// An event's callback type accepts every combination of its borrowed lifetimes.
/// The [`events!`](crate::events) macro supplies this higher-ranked type and its
/// typed invocation; aliases cannot narrow the callback to a `'static` borrow.
pub trait Event: Sized {
    const ID: &'static str;
    type Static: Event<Hook = Self::Hook> + 'static;
    type Hook: ?Sized + Send + Sync + 'static;
    fn call(&mut self, hook: &Self::Hook) -> Result<()>;
}

/// Owns a subscription. Dropping it prevents later calls and releases captures
/// once any callback already running has finished. Removal is safe during dispatch.
#[must_use = "dropping the registration immediately unsubscribes its hook"]
pub struct HookRegistration(Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for HookRegistration {
    fn drop(&mut self) {
        if let Some(unsubscribe) = self.0.take() {
            unsubscribe();
        }
    }
}

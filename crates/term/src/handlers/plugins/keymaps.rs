//! Per-frontend keymap registrations layered beneath configured user bindings.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    rc::Rc,
    sync::Arc,
};

use arc_swap::{
    access::{Constant, DynAccess, DynGuard},
    ArcSwap,
};
use plugin_api::{
    ui::{KeymapMode, KeymapUpdate, UiOwner, MAX_PLUGIN_KEYBINDINGS},
    ErrorCode, ServiceError,
};
use ui_core::input::{KeyCode, KeyEvent, KeyModifiers};
use view::document::Mode;

use crate::{
    commands::MappableCommand,
    keymap::{KeyTrie, KeyTrieNode, Keymaps},
};

type Map = HashMap<Mode, KeyTrie>;

#[derive(Default)]
struct Registrations {
    revision: u64,
    entries: BTreeMap<UiOwner, KeymapUpdate>,
}

#[derive(Default)]
pub(super) struct ScopedKeymaps {
    base: Option<Rc<dyn DynAccess<Map>>>,
    registrations: Rc<RefCell<Registrations>>,
}

impl ScopedKeymaps {
    pub fn register(
        &mut self,
        update: KeymapUpdate,
        keymaps: &mut Keymaps,
    ) -> Result<(), ServiceError> {
        if update.bindings.len() > MAX_PLUGIN_KEYBINDINGS {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "too many plugin keybindings",
            ));
        }
        self.install(keymaps);
        let base = self.base.as_ref().unwrap().load();
        let mut entries = self.registrations.borrow().entries.clone();
        entries.insert(update.identity.owner.clone(), update);
        // Validate the entire replacement before changing any active keymap.
        compile(&base, entries.values(), false)?;
        let mut registrations = self.registrations.borrow_mut();
        registrations.entries = entries;
        registrations.revision = registrations.revision.wrapping_add(1);
        drop(registrations);
        reset_pending(keymaps);
        Ok(())
    }

    pub fn retain(
        &mut self,
        keymaps: &mut Keymaps,
        mut current: impl FnMut(&UiOwner) -> bool,
    ) -> bool {
        let mut registrations = self.registrations.borrow_mut();
        let before = registrations.entries.len();
        registrations.entries.retain(|owner, _| current(owner));
        let changed = before != registrations.entries.len();
        if changed {
            registrations.revision = registrations.revision.wrapping_add(1);
            drop(registrations);
            reset_pending(keymaps);
        }
        changed
    }

    fn install(&mut self, keymaps: &mut Keymaps) {
        if self.base.is_some() {
            return;
        }
        let base: Rc<dyn DynAccess<Map>> = Rc::from(std::mem::replace(
            &mut keymaps.map,
            Box::new(Constant(Map::new())),
        ));
        keymaps.map = Box::new(PluginMapAccess {
            base: base.clone(),
            registrations: self.registrations.clone(),
            cached_base: RefCell::new(None),
            cached_revision: RefCell::new(0),
            cached: ArcSwap::from_pointee(Map::new()),
        });
        self.base = Some(base);
    }
}

fn reset_pending(keymaps: &mut Keymaps) {
    // Use the same cancellation path as native Escape without executing its
    // mapped command. Sticky/pending nodes must not retain unloaded bindings.
    let _ = keymaps.get(
        Mode::Normal,
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
        },
    );
    keymaps.sticky = None;
}

struct PluginMapAccess {
    base: Rc<dyn DynAccess<Map>>,
    registrations: Rc<RefCell<Registrations>>,
    // DynGuard owns its config snapshot. Retaining it makes pointer comparison
    // safe from allocator reuse and avoids comparing every binding per key.
    cached_base: RefCell<Option<DynGuard<Map>>>,
    cached_revision: RefCell<u64>,
    cached: ArcSwap<Map>,
}

impl DynAccess<Map> for PluginMapAccess {
    fn load(&self) -> DynGuard<Map> {
        let base = self.base.load();
        let registrations = self.registrations.borrow();
        if registrations.entries.is_empty() {
            self.cached_base.borrow_mut().take();
            return base;
        }
        let changed = self
            .cached_base
            .borrow()
            .as_ref()
            .is_none_or(|cached| !std::ptr::eq(&**cached, &*base))
            || *self.cached_revision.borrow() != registrations.revision;
        if changed {
            // A later user configuration always takes precedence. Registrations
            // themselves were validated atomically when they were accepted.
            let map = compile(&base, registrations.entries.values(), true)
                .expect("accepted plugin keybindings remain syntactically valid");
            self.cached.store(Arc::new(map));
            *self.cached_base.borrow_mut() = Some(base);
            *self.cached_revision.borrow_mut() = registrations.revision;
        }
        DynAccess::load(&self.cached)
    }
}

fn compile<'a>(
    base: &Map,
    updates: impl Iterator<Item = &'a KeymapUpdate>,
    skip_conflicts: bool,
) -> Result<Map, ServiceError> {
    let mut map = base.clone();
    for update in updates {
        for binding in &update.bindings {
            if binding.keys.is_empty()
                || binding.keys.len() > 8
                || binding.keys.iter().any(|key| key.len() > 64)
                || binding.command.is_empty()
                || binding.command.len() > 128
                || !binding
                    .command
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            {
                return Err(ServiceError::new(
                    ErrorCode::InvalidRequest,
                    "invalid plugin keybinding",
                ));
            }
            let keys = binding
                .keys
                .iter()
                .map(|key| {
                    key.parse::<KeyEvent>().map_err(|error| {
                        ServiceError::new(ErrorCode::InvalidRequest, error.to_string())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mode = match binding.mode {
                KeymapMode::Normal => Mode::Normal,
                KeymapMode::Select => Mode::Select,
                KeymapMode::Insert => Mode::Insert,
            };
            let command = MappableCommand::Typable {
                name: format!("{}.{}", update.identity.owner.plugin, binding.command),
                args: String::new(),
                doc: String::new(),
            };
            let mut root = map
                .get(&mode)
                .cloned()
                .unwrap_or_else(|| KeyTrie::Node(KeyTrieNode::default()));
            if let Err(error) = insert(&mut root, &keys, command) {
                if skip_conflicts {
                    continue;
                }
                return Err(error);
            }
            map.insert(mode, root);
        }
    }
    Ok(map)
}

fn insert(
    trie: &mut KeyTrie,
    keys: &[KeyEvent],
    command: MappableCommand,
) -> Result<(), ServiceError> {
    let Some(node) = trie.node_mut() else {
        return Err(ServiceError::new(
            ErrorCode::InvalidRequest,
            "plugin binding conflicts with an existing command",
        ));
    };
    if keys.len() == 1 {
        if node.contains_key(&keys[0]) {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                "plugin binding conflicts with an existing binding",
            ));
        }
        node.insert(keys[0], KeyTrie::MappableCommand(command));
        return Ok(());
    }
    let next = node
        .entry(keys[0])
        .or_insert_with(|| KeyTrie::Node(KeyTrieNode::default()));
    insert(next, &keys[1..], command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::ui::{PluginKeybinding, UiIdentity};

    fn update(plugin: &str, key: &str) -> KeymapUpdate {
        KeymapUpdate {
            identity: UiIdentity {
                owner: UiOwner {
                    plugin: plugin.into(),
                    generation: 1,
                },
                request: 1,
                token: 1,
            },
            bindings: vec![PluginKeybinding {
                mode: KeymapMode::Normal,
                keys: vec![key.into()],
                command: "run".into(),
            }],
        }
    }

    #[test]
    fn registration_is_atomic_and_unload_restores_configuration() {
        let mut keys = Keymaps::default();
        let mut scoped = ScopedKeymaps::default();
        scoped
            .register(update("fixture", "F12"), &mut keys)
            .unwrap();
        assert!(
            matches!(keys.get(Mode::Normal, "F12".parse().unwrap()), crate::keymap::KeymapResult::Matched(command) if command.name() == "fixture.run")
        );
        let mut bad = update("fixture", "F11");
        bad.bindings
            .push(update("fixture", "i").bindings.pop().unwrap());
        assert!(scoped.register(bad, &mut keys).is_err());
        assert!(matches!(
            keys.get(Mode::Normal, "F12".parse().unwrap()),
            crate::keymap::KeymapResult::Matched(_)
        ));
        assert!(matches!(
            keys.get(Mode::Normal, "F11".parse().unwrap()),
            crate::keymap::KeymapResult::NotFound
        ));
        assert!(scoped.retain(&mut keys, |_| false));
        assert!(matches!(
            keys.get(Mode::Normal, "F12".parse().unwrap()),
            crate::keymap::KeymapResult::NotFound
        ));
        assert!(
            matches!(keys.get(Mode::Normal, "i".parse().unwrap()), crate::keymap::KeymapResult::Matched(command) if command.name() == "insert_mode")
        );
    }

    #[test]
    fn user_config_reload_takes_precedence_and_editors_do_not_share_bindings() {
        let config = Arc::new(ArcSwap::from_pointee(crate::keymap::default()));
        let mut keys = Keymaps::new(Box::new(config.clone()));
        let mut other = Keymaps::default();
        let mut scoped = ScopedKeymaps::default();
        scoped
            .register(update("fixture", "F12"), &mut keys)
            .unwrap();
        assert!(matches!(
            other.get(Mode::Normal, "F12".parse().unwrap()),
            crate::keymap::KeymapResult::NotFound
        ));
        let mut replacement = crate::keymap::default();
        replacement
            .get_mut(&Mode::Normal)
            .unwrap()
            .node_mut()
            .unwrap()
            .insert(
                "F12".parse().unwrap(),
                KeyTrie::MappableCommand(MappableCommand::no_op),
            );
        config.store(Arc::new(replacement));
        assert!(
            matches!(keys.get(Mode::Normal, "F12".parse().unwrap()), crate::keymap::KeymapResult::Matched(command) if command.name() == "no_op")
        );
    }
}

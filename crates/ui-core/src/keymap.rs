//! Typed keymap storage, decoding, merging, and prefix resolution for every frontend component.

use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use indexmap::IndexMap;
use serde::{
    de::{self, DeserializeSeed, IntoDeserializer},
    Deserialize, Deserializer,
};
use std::{
    fmt,
    ops::{Deref, DerefMut},
};

/// Action-specific configuration rules. Key traversal and merging are shared.
pub trait KeymapAction: Sized {
    const LABELS: bool = false;
    const SPECIAL_BINDINGS: bool = true;
    fn validate_sequence(_actions: &[Self]) -> Result<(), String> {
        Ok(())
    }
    fn with_label(self, _label: String) -> Result<Self, String> {
        Ok(self)
    }
}

#[derive(Debug, Clone)]
pub struct Keymap<A> {
    pub name: String,
    map: IndexMap<KeyEvent, Binding<A>>,
    pub is_sticky: bool,
}

impl<A> Default for Keymap<A> {
    fn default() -> Self {
        Self::new("", IndexMap::new())
    }
}

impl<A> Keymap<A> {
    pub fn new(name: &str, map: IndexMap<KeyEvent, Binding<A>>) -> Self {
        Self {
            name: name.to_owned(),
            map,
            is_sticky: false,
        }
    }

    pub fn merge(&mut self, other: Self) {
        if !other.name.is_empty() {
            self.name = other.name;
        }
        for (key, binding) in other.map {
            match (self.map.get_mut(&key), binding) {
                (Some(Binding::Node(keys)), Binding::Node(other)) => keys.merge(other),
                (_, binding) => {
                    self.map.insert(key, binding);
                }
            }
        }
    }

    pub fn at(&self, path: &[KeyEvent]) -> Option<&Self> {
        let mut map = self;
        for key in path {
            map = map.get(key)?.node()?;
        }
        Some(map)
    }

    pub fn map_actions<B>(self, map: impl Fn(A) -> B + Copy) -> Keymap<B> {
        Keymap {
            name: self.name,
            is_sticky: self.is_sticky,
            map: self
                .map
                .into_iter()
                .map(|(key, binding)| {
                    let binding = match binding {
                        Binding::Action(action) => Binding::Action(map(action)),
                        Binding::Sequence(label, actions) => {
                            Binding::Sequence(label, actions.into_iter().map(map).collect())
                        }
                        Binding::Node(keys) => Binding::Node(keys.map_actions(map)),
                        Binding::Unbound => Binding::Unbound,
                    };
                    (key, binding)
                })
                .collect(),
        }
    }
}

impl<A: PartialEq> PartialEq for Keymap<A> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}
impl<A: Eq> Eq for Keymap<A> {}
impl<A> Deref for Keymap<A> {
    type Target = IndexMap<KeyEvent, Binding<A>>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}
impl<A> DerefMut for Keymap<A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding<A> {
    Action(A),
    Sequence(String, Vec<A>),
    Node(Keymap<A>),
    Unbound,
}

impl<A> Binding<A> {
    pub const DEFAULT_SEQUENCE_LABEL: &'static str = "[Multiple commands]";
    pub fn sequence(actions: Vec<A>) -> Self {
        Self::Sequence(Self::DEFAULT_SEQUENCE_LABEL.into(), actions)
    }
    pub fn node(&self) -> Option<&Keymap<A>> {
        if let Self::Node(node) = self {
            Some(node)
        } else {
            None
        }
    }
    pub fn node_mut(&mut self) -> Option<&mut Keymap<A>> {
        if let Self::Node(node) = self {
            Some(node)
        } else {
            None
        }
    }
    pub fn merge_nodes(&mut self, other: Self) {
        self.node_mut().unwrap().merge(other.into_node());
    }
    fn into_node(self) -> Keymap<A> {
        match self {
            Self::Node(node) => node,
            _ => panic!("expected a keymap table"),
        }
    }
    pub fn search(&self, keys: &[KeyEvent]) -> Option<&Self> {
        let mut binding = self;
        for key in keys {
            binding = binding.node()?.get(key)?;
        }
        Some(binding)
    }
    pub fn actions(&self) -> Option<&[A]> {
        match self {
            Self::Action(action) => Some(std::slice::from_ref(action)),
            Self::Sequence(_, actions) => Some(actions),
            _ => None,
        }
    }
}

impl<'de, A: Deserialize<'de> + KeymapAction> Deserialize<'de> for Keymap<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Binding::deserialize(deserializer)? {
            Binding::Node(node) => Ok(node),
            _ => Err(de::Error::custom("expected a keymap table")),
        }
    }
}
impl<'de, A: Deserialize<'de> + KeymapAction> Deserialize<'de> for Binding<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BindingVisitor {
            allow_map: true,
            action: std::marker::PhantomData,
        }
        .deserialize(deserializer)
    }
}
struct BindingVisitor<A> {
    allow_map: bool,
    action: std::marker::PhantomData<A>,
}
impl<'de, A: Deserialize<'de> + KeymapAction> DeserializeSeed<'de> for BindingVisitor<A> {
    type Value = Binding<A>;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de, A: Deserialize<'de> + KeymapAction> de::Visitor<'de> for BindingVisitor<A> {
    type Value = Binding<A>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an action, list of actions, or sub-keymap")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "no_op" if A::SPECIAL_BINDINGS => Ok(Binding::sequence(Vec::new())),
            "unbound" if A::SPECIAL_BINDINGS => Ok(Binding::Unbound),
            _ => A::deserialize(value.into_deserializer()).map(Binding::Action),
        }
    }
    fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
        let mut actions = Vec::new();
        while let Some(value) = seq.next_element::<String>()? {
            if A::SPECIAL_BINDINGS {
                if value == "no_op" {
                    continue;
                }
                if value == "unbound" {
                    return Err(de::Error::custom(
                        "unbound cannot be used in an action sequence",
                    ));
                }
            }
            actions.push(A::deserialize(value.as_str().into_deserializer())?);
        }
        A::validate_sequence(&actions).map_err(de::Error::custom)?;
        Ok(Binding::sequence(actions))
    }
    fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        if !self.allow_map {
            return Err(de::Error::custom(
                "command must be a string or list of commands",
            ));
        }
        let mut label = String::new();
        let mut command = None;
        let mut bindings = IndexMap::new();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "label" if A::LABELS => label = map.next_value()?,
                "command" if A::LABELS => {
                    command = Some(map.next_value_seed(BindingVisitor::<A> {
                        allow_map: false,
                        action: std::marker::PhantomData,
                    })?)
                }
                _ => {
                    let key = key.parse::<KeyEvent>().map_err(de::Error::custom)?;
                    if bindings.insert(key, map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate key binding"));
                    }
                }
            }
        }
        if command.is_some() && !bindings.is_empty() {
            return Err(de::Error::custom(
                "a command binding cannot also contain a sub-keymap",
            ));
        }
        match command {
            None => Ok(Binding::Node(Keymap::new(&label, bindings))),
            Some(Binding::Action(action)) => action
                .with_label(label)
                .map(Binding::Action)
                .map_err(de::Error::custom),
            Some(Binding::Sequence(default_label, actions)) => Ok(Binding::Sequence(
                if label.is_empty() {
                    default_label
                } else {
                    label
                },
                actions,
            )),
            _ => Err(de::Error::custom(
                "command must be a string or list of commands",
            )),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup<'a, A> {
    Actions(&'a [A]),
    Pending,
    Unbound,
}

/// Resolution retains canceled input for editors that replay invalid insert prefixes.
pub enum Resolution<'a, A> {
    Matched(&'a Binding<A>),
    Pending(&'a Keymap<A>),
    Unbound,
    Cancelled(Vec<KeyEvent>),
}

/// Prefixes are paths into the current map; reload never retains obsolete bindings.
#[derive(Debug, Default)]
pub struct KeymapState {
    pending: Vec<KeyEvent>,
    sticky: Vec<KeyEvent>,
}
impl KeymapState {
    pub fn clear(&mut self) {
        self.pending.clear();
        self.sticky.clear();
    }
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn pending(&self) -> &[KeyEvent] {
        &self.pending
    }
    pub fn is_sticky(&self) -> bool {
        !self.sticky.is_empty()
    }
    pub fn sticky<'a, A>(&self, map: &'a Keymap<A>) -> Option<&'a Keymap<A>> {
        (!self.sticky.is_empty())
            .then(|| map.at(&self.sticky))
            .flatten()
    }
    pub fn active<'a, A>(&self, map: &'a Keymap<A>) -> Option<&'a Keymap<A>> {
        self.sticky(map).unwrap_or(map).at(&self.pending)
    }
    pub fn lookup<'a, A>(&mut self, map: &'a Keymap<A>, key: KeyEvent) -> Lookup<'a, A> {
        match self.resolve(map, key) {
            Resolution::Matched(binding) => Lookup::Actions(binding.actions().unwrap()),
            Resolution::Pending(_) => Lookup::Pending,
            Resolution::Unbound => Lookup::Unbound,
            Resolution::Cancelled(_) => Lookup::Actions(&[]),
        }
    }
    pub fn resolve<'a, A>(&mut self, map: &'a Keymap<A>, mut key: KeyEvent) -> Resolution<'a, A> {
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            if self.is_pending() {
                return Resolution::Cancelled(std::mem::take(&mut self.pending));
            }
            self.sticky.clear();
        }
        if !self.sticky.is_empty() && self.sticky(map).is_none() {
            self.clear();
        }
        let keys = match self.active(map) {
            Some(keys) => keys,
            None => {
                self.pending.clear();
                self.sticky(map).unwrap_or(map)
            }
        };
        // Prefer explicit Shift bindings before normalizing character events.
        if matches!(key.code, KeyCode::Char(_))
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && !keys.contains_key(&key)
        {
            key.modifiers.remove(KeyModifiers::SHIFT);
        }
        match keys.get(&key) {
            Some(Binding::Node(node)) => {
                self.pending.push(key);
                if node.is_sticky {
                    self.sticky.append(&mut self.pending);
                }
                Resolution::Pending(node)
            }
            Some(Binding::Unbound) => {
                self.pending.clear();
                Resolution::Unbound
            }
            Some(binding) => {
                self.pending.clear();
                Resolution::Matched(binding)
            }
            None if self.is_pending() => {
                self.pending.push(key);
                Resolution::Cancelled(std::mem::take(&mut self.pending))
            }
            None => Resolution::Unbound,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> KeyEvent {
        value.parse().unwrap()
    }
    fn action(value: u8) -> Binding<u8> {
        Binding::Action(value)
    }
    fn map(entries: &[(&str, Binding<u8>)]) -> Keymap<u8> {
        Keymap::new(
            "",
            entries
                .iter()
                .map(|(k, binding)| (key(k), binding.clone()))
                .collect(),
        )
    }

    #[test]
    fn prefixes_cancel_without_leaking_input_and_state_is_per_instance() {
        let keys = map(&[("g", Binding::Node(map(&[("j", action(1))])))]);
        let mut state = KeymapState::default();
        let mut other = KeymapState::default();
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        assert_eq!(other.lookup(&keys, key("j")), Lookup::Unbound);
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[1]));
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        assert_eq!(state.lookup(&keys, key("esc")), Lookup::Actions(&[]));
        assert!(!state.is_pending());
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        assert_eq!(state.lookup(&keys, key("x")), Lookup::Actions(&[]));
        assert!(!state.is_pending());
        assert_eq!(state.lookup(&keys, key("x")), Lookup::Unbound);
    }

    #[test]
    fn merge_preserves_nested_defaults_and_distinguishes_no_op_from_unbound() {
        let mut keys = map(&[(
            "g",
            Binding::Node(map(&[("j", action(1)), ("k", action(2))])),
        )]);
        keys.merge(map(&[(
            "g",
            Binding::Node(map(&[
                ("j", Binding::Unbound),
                ("x", Binding::sequence(vec![])),
            ])),
        )]));
        let mut state = KeymapState::default();
        for (suffix, expected) in [
            ("j", Lookup::Unbound),
            ("k", Lookup::Actions(&[2])),
            ("x", Lookup::Actions(&[])),
        ] {
            assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
            assert_eq!(state.lookup(&keys, key(suffix)), expected);
        }
        keys.merge(map(&[("g", action(3))]));
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Actions(&[3]));
    }

    #[test]
    fn shifted_character_events_match_configured_bindings_and_prefixes() {
        let keys = map(&[
            ("C-S-j", action(1)),
            ("G", Binding::Node(map(&[("J", action(2))]))),
            ("S-space", action(3)),
        ]);
        let mut state = KeymapState::default();
        let shifted = |code, modifiers| KeyEvent {
            code: KeyCode::Char(code),
            modifiers: modifiers | KeyModifiers::SHIFT,
        };
        assert_eq!(
            state.lookup(&keys, shifted('J', KeyModifiers::CONTROL)),
            Lookup::Actions(&[1])
        );
        assert_eq!(
            state.lookup(&keys, shifted('G', KeyModifiers::NONE)),
            Lookup::Pending
        );
        assert_eq!(
            state.lookup(&keys, shifted('J', KeyModifiers::NONE)),
            Lookup::Actions(&[2])
        );
        assert_eq!(
            state.lookup(&keys, shifted(' ', KeyModifiers::NONE)),
            Lookup::Actions(&[3])
        );
        assert_eq!(state.lookup(&keys, key("space")), Lookup::Unbound);
    }

    #[test]
    fn pending_lookup_uses_reloaded_bindings_and_recovers_removed_prefixes() {
        let mut keys = map(&[("g", Binding::Node(map(&[("j", action(1))])))]);
        let mut state = KeymapState::default();
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        keys.merge(map(&[("g", Binding::Node(map(&[("j", action(2))])))]));
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[2]));
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        let keys = map(&[("j", action(3))]);
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[3]));
        assert!(!state.is_pending());
    }
    #[test]
    fn sticky_prefixes_use_current_bindings_and_escape_returns_to_the_root() {
        let mut nested = map(&[("j", action(1))]);
        nested.is_sticky = true;
        let mut keys = map(&[("z", Binding::Node(nested)), ("j", action(9))]);
        let mut state = KeymapState::default();
        assert_eq!(state.lookup(&keys, key("z")), Lookup::Pending);
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[1]));
        keys.get_mut(&key("z"))
            .unwrap()
            .node_mut()
            .unwrap()
            .insert(key("j"), action(2));
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[2]));
        assert_eq!(state.lookup(&keys, key("esc")), Lookup::Unbound);
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[9]));
        assert_eq!(state.lookup(&keys, key("z")), Lookup::Pending);
        keys.shift_remove(&key("z"));
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[9]));
    }

    #[test]
    fn canceled_prefixes_retain_original_keys_for_insert_mode_fallback() {
        let keys = map(&[("g", Binding::Node(map(&[("j", action(1))])))]);
        let mut state = KeymapState::default();
        assert!(matches!(
            state.resolve(&keys, key("g")),
            Resolution::Pending(_)
        ));
        assert!(
            matches!(state.resolve(&keys, key("x")), Resolution::Cancelled(keys) if keys == [key("g"), key("x")])
        );
        assert!(matches!(
            state.resolve(&keys, key("g")),
            Resolution::Pending(_)
        ));
        assert!(
            matches!(state.resolve(&keys, key("esc")), Resolution::Cancelled(keys) if keys == [key("g")])
        );
    }
}

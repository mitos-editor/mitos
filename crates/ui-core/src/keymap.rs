//! Typed keymap trees, parsing, merging, and prefix lookup shared by frontends.

use std::{fmt, marker::PhantomData, ops::Deref};

use indexmap::IndexMap;
use serde::{
    de::{self, DeserializeOwned, IntoDeserializer},
    Deserialize, Deserializer,
};

use crate::input::{KeyCode, KeyEvent, KeyModifiers};

/// Configuration policy for action names, sequences, and optional binding labels.
pub trait KeymapPolicy<A> {
    const LABELS: bool = false;
    const SPECIAL_ACTIONS: bool = false;
    const SEQUENCE_LABEL: &'static str = "";
    fn parse(value: &str) -> Result<A, String>;
    fn label(_action: A, _label: String) -> Result<A, String> {
        Err("binding labels are not supported".into())
    }
    fn validate_sequence(_actions: &[A]) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentPolicy;

impl<A: DeserializeOwned> KeymapPolicy<A> for ComponentPolicy {
    const SPECIAL_ACTIONS: bool = true;
    fn parse(value: &str) -> Result<A, String> {
        A::deserialize(value.into_deserializer())
            .map_err(|error: de::value::Error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding<A, P = ComponentPolicy> {
    Action(A),
    Sequence(String, Vec<A>),
    Node(Keymap<A, P>),
    /// Let the component's normal fallback handle the key.
    Unbound,
}

#[derive(Debug, Clone)]
pub struct Keymap<A, P = ComponentPolicy> {
    pub name: String,
    pub is_sticky: bool,
    pub bindings: IndexMap<KeyEvent, Binding<A, P>>,
    policy: PhantomData<P>,
}

impl<A: PartialEq, P: PartialEq> PartialEq for Keymap<A, P> {
    fn eq(&self, other: &Self) -> bool {
        self.bindings == other.bindings
    }
}
impl<A: Eq, P: Eq> Eq for Keymap<A, P> {}

impl<A, P> Default for Keymap<A, P> {
    fn default() -> Self {
        Self::new("", IndexMap::new())
    }
}

impl<A, P> Deref for Keymap<A, P> {
    type Target = IndexMap<KeyEvent, Binding<A, P>>;
    fn deref(&self) -> &Self::Target {
        &self.bindings
    }
}

impl<A, P> Keymap<A, P> {
    pub fn new(name: &str, bindings: IndexMap<KeyEvent, Binding<A, P>>) -> Self {
        Self {
            name: name.into(),
            is_sticky: false,
            bindings,
            policy: PhantomData,
        }
    }

    /// Transform action values without changing prefixes or binding metadata.
    pub fn map_actions<B>(self, map: impl Fn(A) -> B + Copy) -> Keymap<B, P> {
        Keymap {
            name: self.name,
            is_sticky: self.is_sticky,
            policy: PhantomData,
            bindings: self
                .bindings
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

    pub fn merge(&mut self, other: Self) {
        if !other.name.is_empty() {
            self.name = other.name;
        }
        for (key, binding) in other.bindings {
            match (self.bindings.get_mut(&key), binding) {
                (Some(Binding::Node(keys)), Binding::Node(other)) => keys.merge(other),
                (_, binding) => {
                    self.bindings.insert(key, binding);
                }
            }
        }
    }

    pub fn at(&self, path: &[KeyEvent]) -> Option<&Self> {
        path.iter()
            .try_fold(self, |keys, key| keys.get(key)?.node())
    }
}

impl<A, P> Binding<A, P> {
    pub fn actions(&self) -> Option<&[A]> {
        match self {
            Self::Action(action) => Some(std::slice::from_ref(action)),
            Self::Sequence(_, actions) => Some(actions),
            _ => None,
        }
    }
    pub fn node(&self) -> Option<&Keymap<A, P>> {
        match self {
            Self::Node(keys) => Some(keys),
            _ => None,
        }
    }
    pub fn node_mut(&mut self) -> Option<&mut Keymap<A, P>> {
        match self {
            Self::Node(keys) => Some(keys),
            _ => None,
        }
    }
    pub fn merge_nodes(&mut self, other: Self) {
        let Self::Node(other) = other else {
            panic!("expected a keymap node")
        };
        self.node_mut()
            .expect("expected a keymap node")
            .merge(other);
    }
    pub fn search(&self, path: &[KeyEvent]) -> Option<&Self> {
        path.iter()
            .try_fold(self, |binding, key| binding.node()?.get(key))
    }
}

impl<A, P: KeymapPolicy<A>> Binding<A, P> {
    pub const DEFAULT_SEQUENCE_LABEL: &'static str = P::SEQUENCE_LABEL;
    pub fn sequence(actions: Vec<A>) -> Self {
        Self::Sequence(P::SEQUENCE_LABEL.into(), actions)
    }

    fn parse_action<E: de::Error>(value: &str) -> Result<Self, E> {
        match value {
            "no_op" if P::SPECIAL_ACTIONS => Ok(Self::sequence(Vec::new())),
            "unbound" if P::SPECIAL_ACTIONS => Ok(Self::Unbound),
            value => P::parse(value).map(Self::Action).map_err(E::custom),
        }
    }
    fn parse_sequence<E: de::Error>(values: Vec<String>) -> Result<Self, E> {
        let mut actions = Vec::new();
        for value in values {
            if P::SPECIAL_ACTIONS && value == "no_op" {
                continue;
            }
            if P::SPECIAL_ACTIONS && value == "unbound" {
                return Err(E::custom("unbound cannot be used in an action sequence"));
            }
            actions.push(P::parse(&value).map_err(E::custom)?);
        }
        P::validate_sequence(&actions).map_err(E::custom)?;
        Ok(Self::sequence(actions))
    }
}

impl<'de, A, P: KeymapPolicy<A>> Deserialize<'de> for Keymap<A, P> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Binding::deserialize(deserializer)? {
            Binding::Node(keys) => Ok(keys),
            _ => Err(de::Error::custom("expected a keymap table")),
        }
    }
}

impl<'de, A, P: KeymapPolicy<A>> Deserialize<'de> for Binding<A, P> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<A, P>(PhantomData<(A, P)>);
        impl<'de, A, P: KeymapPolicy<A>> de::Visitor<'de> for Visitor<A, P> {
            type Value = Binding<A, P>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an action, list of actions, or sub-keymap")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Binding::parse_action(value)
            }
            fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                let mut actions = Vec::new();
                while let Some(action) = seq.next_element::<String>()? {
                    actions.push(action);
                }
                Binding::parse_sequence(actions)
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                #[derive(Deserialize)]
                #[serde(untagged)]
                enum Command {
                    Action(String),
                    Sequence(Vec<String>),
                }
                let mut keys = Keymap::default();
                let mut command = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "label" if P::LABELS => keys.name = map.next_value()?,
                        "command" if P::LABELS => {
                            command = Some(match map.next_value::<Command>()? {
                                Command::Action(value) => Binding::parse_action(&value)?,
                                Command::Sequence(values) => Binding::parse_sequence(values)?,
                            });
                        }
                        _ => {
                            let key = key.parse().map_err(de::Error::custom)?;
                            if keys.bindings.insert(key, map.next_value()?).is_some() {
                                return Err(de::Error::custom("duplicate key binding"));
                            }
                        }
                    }
                }
                match command {
                    Some(_) if !keys.bindings.is_empty() => Err(de::Error::custom(
                        "a command binding cannot also contain a sub-keymap",
                    )),
                    Some(Binding::Action(action)) if !keys.name.is_empty() => {
                        P::label(action, keys.name)
                            .map(Binding::Action)
                            .map_err(de::Error::custom)
                    }
                    Some(Binding::Sequence(_, actions)) if !keys.name.is_empty() => {
                        Ok(Binding::Sequence(keys.name, actions))
                    }
                    Some(binding) => Ok(binding),
                    None => Ok(Binding::Node(keys)),
                }
            }
        }
        deserializer.deserialize_any(Visitor(PhantomData))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup<'a, A> {
    Actions(&'a [A]),
    Pending,
    Unbound,
}

pub enum BindingLookup<'a, A, P> {
    Matched(&'a Binding<A, P>),
    Pending(&'a Keymap<A, P>),
    Unbound,
    Cancelled(Vec<KeyEvent>),
}

/// Prefix state always resolves against the caller's current keymap.
#[derive(Debug, Default)]
pub struct KeymapState {
    pending: Vec<KeyEvent>,
}

impl KeymapState {
    pub fn clear(&mut self) {
        self.pending.clear();
    }
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn pending(&self) -> &[KeyEvent] {
        &self.pending
    }

    pub fn lookup_binding<'a, A, P>(
        &mut self,
        map: &'a Keymap<A, P>,
        mut key: KeyEvent,
    ) -> BindingLookup<'a, A, P> {
        if self.is_pending() && key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            return BindingLookup::Cancelled(std::mem::take(&mut self.pending));
        }
        let keys = match map.at(&self.pending) {
            Some(keys) => keys,
            None => {
                self.clear();
                map
            }
        };
        if matches!(key.code, KeyCode::Char(_))
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && !keys.contains_key(&key)
        {
            key.modifiers.remove(KeyModifiers::SHIFT);
        }
        match keys.get(&key) {
            Some(Binding::Node(keys)) => {
                self.pending.push(key);
                BindingLookup::Pending(keys)
            }
            Some(Binding::Unbound) => {
                self.clear();
                BindingLookup::Unbound
            }
            Some(binding) => {
                self.clear();
                BindingLookup::Matched(binding)
            }
            None if self.is_pending() => {
                self.pending.push(key);
                BindingLookup::Cancelled(std::mem::take(&mut self.pending))
            }
            None => BindingLookup::Unbound,
        }
    }

    pub fn lookup<'a, A>(&mut self, map: &'a Keymap<A>, key: KeyEvent) -> Lookup<'a, A> {
        match self.lookup_binding(map, key) {
            BindingLookup::Matched(binding) => Lookup::Actions(binding.actions().unwrap()),
            BindingLookup::Pending(_) => Lookup::Pending,
            BindingLookup::Unbound => Lookup::Unbound,
            BindingLookup::Cancelled(_) => Lookup::Actions(&[]),
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
                ("x", Binding::Sequence(String::new(), vec![])),
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
}

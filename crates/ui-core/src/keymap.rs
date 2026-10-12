//! Typed key bindings independent of component behavior and editor state.

use std::{collections::HashMap, fmt};

use serde::{
    de::{self, IntoDeserializer},
    Deserialize, Deserializer,
};

use crate::input::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding<A> {
    Actions(Vec<A>),
    Keys(Keymap<A>),
    /// Let the component's normal fallback handle the key.
    Unbound,
}

/// An empty action list consumes a key without changing component state (`no_op`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap<A>(pub HashMap<KeyEvent, Binding<A>>);

impl<'de, A: Deserialize<'de>> Deserialize<'de> for Keymap<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Binding::deserialize(deserializer)? {
            Binding::Keys(keys) => Ok(keys),
            _ => Err(de::Error::custom("expected a keymap table")),
        }
    }
}

impl<A> Default for Keymap<A> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<A> Keymap<A> {
    /// Transform action types while preserving keys, prefixes, and unbound keys.
    pub fn map_actions<B>(self, map: impl Fn(A) -> B + Copy) -> Keymap<B> {
        Keymap(
            self.0
                .into_iter()
                .map(|(key, binding)| {
                    let binding = match binding {
                        Binding::Actions(actions) => {
                            Binding::Actions(actions.into_iter().map(map).collect())
                        }
                        Binding::Keys(keys) => Binding::Keys(keys.map_actions(map)),
                        Binding::Unbound => Binding::Unbound,
                    };
                    (key, binding)
                })
                .collect(),
        )
    }

    pub fn merge(&mut self, other: Self) {
        for (key, binding) in other.0 {
            match (self.0.get_mut(&key), binding) {
                (Some(Binding::Keys(keys)), Binding::Keys(other)) => keys.merge(other),
                (_, binding) => {
                    self.0.insert(key, binding);
                }
            }
        }
    }

    fn at(&self, path: &[KeyEvent]) -> Option<&Self> {
        let mut keys = self;
        for key in path {
            let Binding::Keys(next) = keys.0.get(key)? else {
                return None;
            };
            keys = next;
        }
        Some(keys)
    }
}

impl<'de, A: Deserialize<'de>> Deserialize<'de> for Binding<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<A>(std::marker::PhantomData<A>);
        impl<'de, A: Deserialize<'de>> de::Visitor<'de> for Visitor<A> {
            type Value = Binding<A>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a component action, list of actions, or sub-keymap")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "no_op" => Ok(Binding::Actions(Vec::new())),
                    "unbound" => Ok(Binding::Unbound),
                    _ => A::deserialize(value.into_deserializer())
                        .map(|action| Binding::Actions(vec![action])),
                }
            }

            fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                let mut actions = Vec::new();
                while let Some(value) = seq.next_element::<String>()? {
                    if value == "no_op" {
                        continue;
                    }
                    if value == "unbound" {
                        return Err(de::Error::custom(
                            "unbound cannot be used in an action sequence",
                        ));
                    }
                    actions.push(A::deserialize(value.as_str().into_deserializer())?);
                }
                Ok(Binding::Actions(actions))
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut keys = HashMap::new();
                while let Some((key, binding)) = map.next_entry()? {
                    if keys.insert(key, binding).is_some() {
                        return Err(de::Error::custom("duplicate key binding"));
                    }
                }
                Ok(Binding::Keys(Keymap(keys)))
            }
        }
        deserializer.deserialize_any(Visitor(std::marker::PhantomData))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup<'a, A> {
    Actions(&'a [A]),
    Pending,
    Unbound,
}

/// Per-component prefix state. Lookup uses the current map, so config reloads
/// cannot leave a component executing actions from an obsolete snapshot.
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

    pub fn lookup<'a, A>(&mut self, map: &'a Keymap<A>, mut key: KeyEvent) -> Lookup<'a, A> {
        if self.is_pending() && key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.clear();
            return Lookup::Actions(&[]);
        }
        let keys = match map.at(&self.pending) {
            Some(keys) => keys,
            None => {
                self.clear();
                map
            }
        };
        // Terminal protocols can report Shift alongside a character that already
        // includes the shift (for example, `C-J`). Prefer an explicit binding,
        // then try the character form used by configured keys and editor keymaps.
        if matches!(key.code, KeyCode::Char(_))
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && !keys.0.contains_key(&key)
        {
            key.modifiers.remove(KeyModifiers::SHIFT);
        }
        match keys.0.get(&key) {
            Some(Binding::Actions(actions)) => {
                self.clear();
                Lookup::Actions(actions)
            }
            Some(Binding::Keys(_)) => {
                self.pending.push(key);
                Lookup::Pending
            }
            Some(Binding::Unbound) => {
                self.clear();
                Lookup::Unbound
            }
            None if self.is_pending() => {
                // An invalid suffix cancels and consumes the prefix. It must not
                // accidentally edit text or invoke a command in a layer below.
                self.clear();
                Lookup::Actions(&[])
            }
            None => Lookup::Unbound,
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
        Binding::Actions(vec![value])
    }
    fn map(entries: &[(&str, Binding<u8>)]) -> Keymap<u8> {
        Keymap(
            entries
                .iter()
                .map(|(k, binding)| (key(k), binding.clone()))
                .collect(),
        )
    }

    #[test]
    fn prefixes_cancel_without_leaking_input_and_state_is_per_instance() {
        let keys = map(&[("g", Binding::Keys(map(&[("j", action(1))])))]);
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
            Binding::Keys(map(&[("j", action(1)), ("k", action(2))])),
        )]);
        keys.merge(map(&[(
            "g",
            Binding::Keys(map(&[
                ("j", Binding::Unbound),
                ("x", Binding::Actions(vec![])),
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
            ("G", Binding::Keys(map(&[("J", action(2))]))),
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
        let mut keys = map(&[("g", Binding::Keys(map(&[("j", action(1))])))]);
        let mut state = KeymapState::default();
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        keys.merge(map(&[("g", Binding::Keys(map(&[("j", action(2))])))]));
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[2]));
        assert_eq!(state.lookup(&keys, key("g")), Lookup::Pending);
        let keys = map(&[("j", action(3))]);
        assert_eq!(state.lookup(&keys, key("j")), Lookup::Actions(&[3]));
        assert!(!state.is_pending());
    }
}

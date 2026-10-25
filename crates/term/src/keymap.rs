pub mod component;
pub mod default;
pub mod macros;

pub use crate::commands::MappableCommand;
pub use default::default;

use arc_swap::{
    access::{DynAccess, DynGuard},
    ArcSwap,
};
use macros::key;
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use ui_core::{
    input::KeyEvent,
    keymap::{BindingLookup, KeymapPolicy, KeymapState},
};
use view::{document::Mode, info::Info};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorPolicy;

impl KeymapPolicy<MappableCommand> for EditorPolicy {
    const LABELS: bool = true;
    const SEQUENCE_LABEL: &'static str = "[Multiple commands]";
    fn parse(value: &str) -> Result<MappableCommand, String> {
        value
            .parse::<MappableCommand>()
            .map_err(|error| error.to_string())
    }
    fn label(action: MappableCommand, label: String) -> Result<MappableCommand, String> {
        action.with_label(label).map_err(|error| error.to_string())
    }
    fn validate_sequence(actions: &[MappableCommand]) -> Result<(), String> {
        if actions
            .iter()
            .any(|action| matches!(action, MappableCommand::Macro { .. }))
        {
            return Err("macro keybindings may not be used in command sequences".into());
        }
        Ok(())
    }
}

pub type KeyTrie = ui_core::keymap::Binding<MappableCommand, EditorPolicy>;
pub type KeyTrieNode = ui_core::keymap::Keymap<MappableCommand, EditorPolicy>;

pub trait KeyTrieNodeExt {
    fn infobox(&self) -> Info;
}
impl KeyTrieNodeExt for KeyTrieNode {
    fn infobox(&self) -> Info {
        let mut body: Vec<(BTreeSet<KeyEvent>, &str)> = Vec::with_capacity(self.len());
        for (&key, trie) in self.iter() {
            let desc = match trie {
                KeyTrie::Action(cmd) => {
                    if cmd.name() == "no_op" {
                        continue;
                    }
                    cmd.doc()
                }
                KeyTrie::Node(n) => &n.name,
                KeyTrie::Sequence(label, _) => label,
                KeyTrie::Unbound => continue,
            };
            match body.iter().position(|(_, d)| d == &desc) {
                Some(pos) => {
                    body[pos].0.insert(key);
                }
                None => body.push((BTreeSet::from([key]), desc)),
            }
        }

        let body: Vec<_> = body
            .into_iter()
            .map(|(events, desc)| {
                let events = events.iter().map(ToString::to_string).collect::<Vec<_>>();
                (events.join(", "), desc)
            })
            .collect();
        Info::new(self.name.clone(), &body)
    }
}

pub trait KeyTrieExt {
    fn reverse_map(&self) -> ReverseKeymap;
}
impl KeyTrieExt for KeyTrie {
    fn reverse_map(&self) -> ReverseKeymap {
        // recursively visit all nodes in keymap
        fn map_node(cmd_map: &mut ReverseKeymap, node: &KeyTrie, keys: &mut Vec<KeyEvent>) {
            match node {
                KeyTrie::Action(MappableCommand::Macro { .. }) => {}
                KeyTrie::Action(cmd) => {
                    let name = cmd.name();
                    if name != "no_op" {
                        cmd_map.entry(name.into()).or_default().push(keys.clone())
                    }
                }
                KeyTrie::Node(next) => {
                    for (key, trie) in &next.bindings {
                        keys.push(*key);
                        map_node(cmd_map, trie, keys);
                        keys.pop();
                    }
                }
                KeyTrie::Sequence(..) | KeyTrie::Unbound => {}
            };
        }

        let mut res = HashMap::new();
        map_node(&mut res, self, &mut Vec::new());
        res
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeymapResult {
    /// Needs more keys to execute a command. Contains valid keys for next keystroke.
    Pending(KeyTrieNode),
    Matched(MappableCommand),
    /// Matched a sequence of commands to execute.
    MatchedSequence(Vec<MappableCommand>),
    /// Key was not found in the root keymap
    NotFound,
    /// Key is invalid in combination with previous keys. Contains keys leading upto
    /// and including current (invalid) key.
    Cancelled(Vec<KeyEvent>),
}

/// A map of command names to keybinds that will execute the command.
pub type ReverseKeymap = HashMap<String, Vec<Vec<KeyEvent>>>;

pub struct Keymaps {
    pub map: Box<dyn DynAccess<HashMap<Mode, KeyTrie>>>,
    /// Stores pending keys waiting for the next key. This is relative to a
    /// sticky node if one is in use.
    state: KeymapState,
    /// Active sticky prefix resolves through the live map instead of retaining a tree snapshot.
    sticky: Option<(Mode, Vec<KeyEvent>)>,
}

impl Keymaps {
    pub fn new(map: Box<dyn DynAccess<HashMap<Mode, KeyTrie>>>) -> Self {
        Self {
            map,
            state: KeymapState::default(),
            sticky: None,
        }
    }

    pub fn map(&self) -> DynGuard<HashMap<Mode, KeyTrie>> {
        self.map.load()
    }

    /// Returns list of keys waiting to be disambiguated in current mode.
    pub fn pending(&self) -> &[KeyEvent] {
        self.state.pending()
    }

    pub fn sticky(&self) -> Option<Info> {
        let (mode, path) = self.sticky.as_ref()?;
        self.map
            .load()
            .get(mode)?
            .search(path)?
            .node()
            .map(KeyTrieNodeExt::infobox)
    }

    pub fn contains_key(&self, mode: Mode, key: KeyEvent) -> bool {
        let keymaps = &*self.map();
        let keymap = &keymaps[&mode];
        keymap
            .search(self.pending())
            .and_then(KeyTrie::node)
            .is_some_and(|node| node.contains_key(&key))
    }

    /// Lookup `key` in the keymap to try and find a command to execute. Escape
    /// key cancels pending keystrokes. If there are no pending keystrokes but a
    /// sticky node is in use, it will be cleared.
    pub fn get(&mut self, mode: Mode, key: KeyEvent) -> KeymapResult {
        if key!(Esc) == key && !self.state.is_pending() {
            self.sticky = None;
        }
        let keymaps = self.map.load();
        let root = match self
            .sticky
            .as_ref()
            .and_then(|(mode, path)| keymaps.get(mode)?.search(path)?.node())
        {
            Some(node) => node,
            None => {
                self.sticky = None;
                keymaps[&mode].node().unwrap()
            }
        };
        match self.state.lookup_binding(root, key) {
            BindingLookup::Pending(node) => {
                let node = node.clone();
                if node.is_sticky {
                    self.sticky
                        .get_or_insert_with(|| (mode, Vec::new()))
                        .1
                        .extend_from_slice(self.state.pending());
                    self.state.clear();
                }
                KeymapResult::Pending(node)
            }
            BindingLookup::Matched(KeyTrie::Action(command)) => {
                KeymapResult::Matched(command.clone())
            }
            BindingLookup::Matched(KeyTrie::Sequence(_, commands)) => {
                KeymapResult::MatchedSequence(commands.clone())
            }
            BindingLookup::Matched(_) => unreachable!("lookup returns only action bindings"),
            BindingLookup::Unbound => KeymapResult::NotFound,
            BindingLookup::Cancelled(keys) => KeymapResult::Cancelled(keys),
        }
    }
}

impl Default for Keymaps {
    fn default() -> Self {
        Self::new(Box::new(ArcSwap::new(Arc::new(default()))))
    }
}

/// Merge default config keys with user overwritten keys for custom user config.
pub fn merge_keys(dst: &mut HashMap<Mode, KeyTrie>, mut delta: HashMap<Mode, KeyTrie>) {
    for (mode, keys) in dst {
        keys.merge_nodes(
            delta
                .remove(mode)
                .unwrap_or_else(|| KeyTrie::Node(KeyTrieNode::default())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::macros::keymap;
    use super::*;
    use crate::commands::MappableCommand;
    use arc_swap::access::Constant;
    use editor_core::hashmap;
    use indexmap::indexmap;
    use ui_core::input::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn shared_lookup_preserves_sticky_prefixes_and_editor_cancellation_keys() {
        let keys = hashmap! { Mode::Normal => keymap!({ "Normal"
            "g" => { "Sticky" sticky=true
                "j" => move_line_down,
                "x" => { "Nested"
                    "k" => move_line_up,
                },
            },
        }) };
        let mode = Mode::Normal;
        let down = KeymapResult::Matched(MappableCommand::move_line_down);
        let up = KeymapResult::Matched(MappableCommand::move_line_up);
        let source = Arc::new(ArcSwap::from_pointee(keys));
        let mut maps = Keymaps::new(Box::new(source.clone()));
        assert!(matches!(
            maps.get(mode, key!('g')),
            KeymapResult::Pending(_)
        ));
        assert_eq!(maps.get(mode, key!('j')), down);
        assert!(matches!(
            maps.get(mode, key!('x')),
            KeymapResult::Pending(_)
        ));
        assert_eq!(
            maps.get(mode, key!(Esc)),
            KeymapResult::Cancelled(vec![key!('x')])
        );
        assert!(maps.sticky().is_some());
        source.store(Arc::new(hashmap! { Mode::Normal => keymap!({ "Reloaded"
            "g" => { "Sticky" sticky=true
                "j" => move_line_up,
            },
        }) }));
        assert_eq!(maps.get(mode, key!('j')), up);
        assert_eq!(maps.get(mode, key!(Esc)), KeymapResult::NotFound);
        assert!(maps.sticky().is_none());
    }

    #[test]
    #[should_panic]
    fn duplicate_keys_should_panic() {
        keymap!({ "Normal mode"
            "i" => normal_mode,
            "i" => goto_definition,
        });
    }

    #[test]
    fn check_duplicate_keys_in_default_keymap() {
        // will panic on duplicate keys, assumes that `Keymaps` uses keymap! macro
        Keymaps::default();
    }

    #[test]
    fn merge_partial_keys() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "i" => normal_mode,
                "无" => insert_mode,
                "z" => jump_backward,
                "g" => { "Merge into goto mode"
                    "$" => goto_line_end,
                    "g" => delete_char_forward,
                },
            })
        };
        let mut merged_keyamp = default();
        merge_keys(&mut merged_keyamp, keymap.clone());
        assert_ne!(keymap, merged_keyamp);

        let mut keymap = Keymaps::new(Box::new(Constant(merged_keyamp.clone())));
        assert_eq!(
            keymap.get(Mode::Normal, key!('i')),
            KeymapResult::Matched(MappableCommand::normal_mode),
            "Leaf should replace leaf"
        );
        assert_eq!(
            keymap.get(Mode::Normal, key!('无')),
            KeymapResult::Matched(MappableCommand::insert_mode),
            "New leaf should be present in merged keymap"
        );
        // Assumes that z is a node in the default keymap
        assert_eq!(
            keymap.get(Mode::Normal, key!('z')),
            KeymapResult::Matched(MappableCommand::jump_backward),
            "Leaf should replace node"
        );

        let keymap = merged_keyamp.get_mut(&Mode::Normal).unwrap();
        // Assumes that `g` is a node in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('$')]).unwrap(),
            &KeyTrie::Action(MappableCommand::goto_line_end),
            "Leaf should be present in merged subnode"
        );
        // Assumes that `gg` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('g')]).unwrap(),
            &KeyTrie::Action(MappableCommand::delete_char_forward),
            "Leaf should replace old leaf in merged subnode"
        );
        // Assumes that `ge` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('e')]).unwrap(),
            &KeyTrie::Action(MappableCommand::goto_last_line),
            "Old leaves in subnode should be present in merged node"
        );

        assert!(
            merged_keyamp
                .get(&Mode::Normal)
                .and_then(|key_trie| key_trie.node())
                .unwrap()
                .len()
                > 1
        );
        assert!(!merged_keyamp
            .get(&Mode::Insert)
            .and_then(|key_trie| key_trie.node())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn order_should_be_set() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "space" => { ""
                    "s" => { ""
                        "v" => vsplit,
                        "c" => hsplit,
                    },
                },
            })
        };
        let mut merged_keymap = default();
        merge_keys(&mut merged_keymap, keymap.clone());
        assert_ne!(keymap, merged_keymap);
        let keymap = merged_keymap.get_mut(&Mode::Normal).unwrap();
        // Make sure mapping works
        assert_eq!(
            keymap.search(&[key!(' '), key!('s'), key!('v')]).unwrap(),
            &KeyTrie::Action(MappableCommand::vsplit),
            "Leaf should be present in merged subnode"
        );
        // Merged nodes were ordered at the end
        let node = keymap.search(&[key!(' '), key!('s')]).unwrap();
        assert_eq!(
            node.node().unwrap().keys().copied().collect::<Vec<_>>(),
            vec![key!('v'), key!('c')]
        );
    }

    #[test]
    fn aliased_modes_are_same_in_default_keymap() {
        let keymaps = Keymaps::default().map();
        let root = keymaps.get(&Mode::Normal).unwrap();
        assert_eq!(
            root.search(&[key!(' '), key!('w')]).unwrap(),
            root.search(&["C-w".parse::<KeyEvent>().unwrap()]).unwrap(),
            "Mismatch for window mode on `Space-w` and `Ctrl-w`"
        );
        assert_eq!(
            root.search(&[key!('z')]).unwrap(),
            root.search(&[key!('Z')]).unwrap(),
            "Mismatch for view mode on `z` and `Z`"
        );
    }

    #[test]
    fn reverse_map() {
        let normal_mode = keymap!({ "Normal mode"
            "i" => insert_mode,
            "g" => { "Goto"
                "g" => goto_file_start,
                "e" => goto_file_end,
            },
            "j" | "k" => move_line_down,
        });
        let keymap = normal_mode;
        let mut reverse_map = keymap.reverse_map();

        // sort keybindings in order to have consistent tests
        // HashMaps can be compared but we can still get different ordering of bindings
        // for commands that have multiple bindings assigned
        for v in reverse_map.values_mut() {
            v.sort()
        }

        assert_eq!(
            reverse_map,
            HashMap::from([
                ("insert_mode".to_string(), vec![vec![key!('i')]]),
                (
                    "goto_file_start".to_string(),
                    vec![vec![key!('g'), key!('g')]]
                ),
                (
                    "goto_file_end".to_string(),
                    vec![vec![key!('g'), key!('e')]]
                ),
                (
                    "move_line_down".to_string(),
                    vec![vec![key!('j')], vec![key!('k')]]
                ),
            ]),
            "Mismatch"
        )
    }

    /// Deserialize into KeyTrieNode
    #[test]
    fn deserialize_node() {
        let keys = r#"
"+" = "select_all"
a = "append_mode"
        "#;
        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key!('+') => KeyTrie::Action(
                    MappableCommand::select_all
                ),
                key!('a') => KeyTrie::Action(
                    MappableCommand::append_mode
                ),
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));

        // Other fields in KeyTrieNode CANNOT be deserialized
        let invalid = r#"
name = "name"
is_sticky = false
        "#;
        let result = toml::from_str::<KeyTrieNode>(invalid);
        assert!(result.is_err_and(|error| error.message().contains("Invalid key code 'is_sticky'")));
    }

    #[test]
    fn escaped_keymap() {
        let keys = r#"
"+" = [
    "select_all",
    ":pipe sed -E 's/\\s+$//g'",
]
        "#;

        let key = KeyEvent {
            code: KeyCode::Char('+'),
            modifiers: KeyModifiers::NONE,
        };

        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key => KeyTrie::sequence(vec!{
                    MappableCommand::select_all,
                    MappableCommand::Typable {
                        name: "pipe".to_string(),
                        args: "sed -E 's/\\s+$//g'".to_string(),
                        doc: "".to_string(),
                    },
                })
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));
    }
}

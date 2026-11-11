//! Read-only descriptions of the same maps used by input dispatch.

use super::{component::ComponentKeymaps, KeyTrie, MappableCommand};
use serde::Serialize;
use ui_core::{
    input::KeyEvent,
    keyboard::KeyModifiers,
    keymap::{Binding, Keymap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    User,
    Workspace,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub table: String,
    pub keys: Vec<KeyEvent>,
    pub action: String,
    pub description: String,
    pub prefix: bool,
}

impl Definition {
    pub fn sequence(&self) -> String {
        sequence(&self.keys)
    }
}

pub fn sequence(keys: &[KeyEvent]) -> String {
    keys.iter().map(KeyEvent::key_sequence_format).collect()
}

pub fn action_name(action: &impl Serialize) -> String {
    serde_json::to_value(action)
        .expect("component actions serialize")
        .as_str()
        .expect("component actions have names")
        .to_owned()
}

pub fn component<A: Serialize>(table: &str, keys: &Keymap<A>) -> Vec<Definition> {
    fn collect<A: Serialize>(
        table: &str,
        keys: &Keymap<A>,
        prefix: &mut Vec<KeyEvent>,
        output: &mut Vec<Definition>,
    ) {
        for (key, binding) in &keys.0 {
            prefix.push(*key);
            let (action, description, is_prefix) = match binding {
                Binding::Actions(actions) => {
                    let action = if actions.is_empty() {
                        "no_op".into()
                    } else {
                        actions
                            .iter()
                            .map(action_name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let description = action.replace('_', " ");
                    (action, description, false)
                }
                Binding::Keys(_) => ("prefix".into(), "More actions".into(), true),
                Binding::Unbound => (
                    "unbound".into(),
                    "Use this component's input fallback".into(),
                    false,
                ),
            };
            output.push(Definition {
                table: table.to_owned(),
                keys: prefix.clone(),
                action,
                description,
                prefix: is_prefix,
            });
            if let Binding::Keys(child) = binding {
                collect(table, child, prefix, output);
            }
            prefix.pop();
        }
    }
    let mut output = Vec::new();
    collect(table, keys, &mut Vec::new(), &mut output);
    output
}

pub fn filter<A: Clone>(keys: &Keymap<A>, available: impl Fn(&A) -> bool + Copy) -> Keymap<A> {
    Keymap(
        keys.0
            .iter()
            .filter_map(|(key, binding)| {
                let binding = match binding {
                    Binding::Actions(actions) if actions.iter().all(available) => {
                        Binding::Actions(actions.clone())
                    }
                    Binding::Actions(_) => return None,
                    Binding::Keys(child) => {
                        let child = filter(child, available);
                        if child.0.is_empty() {
                            return None;
                        }
                        Binding::Keys(child)
                    }
                    Binding::Unbound => Binding::Unbound,
                };
                Some((*key, binding))
            })
            .collect(),
    )
}

pub fn prefix_info<A>(keys: &Keymap<A>, description: impl Fn(&A) -> String) -> view::info::Info {
    let mut rows = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (key, binding) in &keys.0 {
        let label = match binding {
            Binding::Actions(actions) if !actions.is_empty() => actions
                .iter()
                .map(&description)
                .collect::<Vec<_>>()
                .join("; "),
            Binding::Keys(_) => "More actions".into(),
            _ => continue,
        };
        rows.entry(label)
            .or_default()
            .push(key.key_sequence_format());
    }
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(label, mut keys)| {
            keys.sort();
            (keys.join(", "), label)
        })
        .collect();
    view::info::Info::new("Actions", &rows)
}

pub fn editor(table: &str, keys: &KeyTrie) -> Vec<Definition> {
    fn command(command: &MappableCommand) -> String {
        match command {
            MappableCommand::Typable { name, args, .. } => {
                format!(":{name} {args}").trim_end().into()
            }
            _ => command.name().to_owned(),
        }
    }
    fn collect(
        table: &str,
        keys: &KeyTrie,
        prefix: &mut Vec<KeyEvent>,
        output: &mut Vec<Definition>,
    ) {
        let KeyTrie::Node(node) = keys else { return };
        for (key, binding) in node.iter() {
            prefix.push(*key);
            let (action, description, is_prefix) = match binding {
                KeyTrie::MappableCommand(cmd) => (command(cmd), cmd.doc().to_owned(), false),
                KeyTrie::Sequence(label, commands) => (
                    commands.iter().map(command).collect::<Vec<_>>().join(", "),
                    label.clone(),
                    false,
                ),
                KeyTrie::Node(node) => (
                    "prefix".into(),
                    format!(
                        "{}{}",
                        node.name,
                        if node.is_sticky { " (sticky)" } else { "" }
                    ),
                    true,
                ),
            };
            output.push(Definition {
                table: table.to_owned(),
                keys: prefix.clone(),
                action,
                description,
                prefix: is_prefix,
            });
            collect(table, binding, prefix, output);
            prefix.pop();
        }
    }
    let mut output = Vec::new();
    collect(table, keys, &mut Vec::new(), &mut output);
    output
}

pub fn components(maps: &ComponentKeymaps) -> Vec<Definition> {
    let mut output = Vec::new();
    macro_rules! table {
        ($name:literal, $field:ident) => {
            output.extend(component(concat!("keys.", $name), &maps.$field));
        };
    }
    table!("prompt", prompt);
    table!("picker", picker);
    table!("buffer-picker", buffer_picker);
    table!("quicklist-picker", quicklist_picker);
    table!("git-picker", git_picker);
    table!("branch-picker", branch_picker);
    table!("worktree-picker", worktree_picker);
    table!("stash-picker", stash_picker);
    table!("history-picker", history_picker);
    table!("file-explorer", file_explorer);
    table!("sidebar", sidebar);
    table!("sidebar-filter", sidebar_filter);
    table!("menu", menu);
    table!("popup", popup);
    table!("hover", hover);
    table!("signature-help", signature_help);
    table!("blame", blame);
    table!("diff", diff);
    output
}

#[derive(Debug, Clone, Copy)]
pub enum Fallback {
    None,
    Text,
    Prompt,
    Literal,
    LiteralText,
    Consume,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub bindings: Vec<Definition>,
    pub pending: Vec<KeyEvent>,
    pub origin_prefix: Vec<KeyEvent>,
    pub fallback: Fallback,
    pub canonicalize_chars: bool,
    pub ignored_keys: Vec<KeyEvent>,
}

impl Context {
    pub fn new(name: impl Into<String>, bindings: Vec<Definition>) -> Self {
        Self {
            name: name.into(),
            bindings,
            pending: Vec::new(),
            origin_prefix: Vec::new(),
            fallback: Fallback::None,
            canonicalize_chars: false,
            ignored_keys: Vec::new(),
        }
    }

    pub fn literal(name: &str, text_only: bool) -> Self {
        let mut context = Self::new(
            name,
            [crate::key!(Esc), crate::ctrl!('c')]
                .into_iter()
                .map(|key| Definition {
                    table: "input".into(),
                    keys: vec![key],
                    action: "cancel literal input".into(),
                    description: "Cancel the current character or register input".into(),
                    prefix: false,
                })
                .collect(),
        );
        context.fallback = if text_only {
            Fallback::LiteralText
        } else {
            Fallback::Literal
        };
        context
    }
}

pub struct Resolution<'a> {
    pub context: &'a Context,
    pub definition: Option<&'a Definition>,
    pub action: &'a str,
    pub prefix: bool,
}

pub fn resolve<'a>(contexts: &'a [Context], input: &[KeyEvent]) -> Option<Resolution<'a>> {
    // Pending continuations own input, including keys normally handled by
    // their children. Components report that context first.
    for context in contexts {
        if input
            .first()
            .is_some_and(|key| context.ignored_keys.contains(key))
        {
            continue;
        }
        if !context.pending.is_empty()
            && input.len() == 1
            && matches!(input[0], crate::key!(Esc) | crate::ctrl!('c'))
        {
            return Some(Resolution {
                context,
                definition: None,
                action: "cancel key sequence",
                prefix: false,
            });
        }
        let mut path: Vec<_> = context.pending.iter().chain(input).copied().collect();
        for index in 0..path.len() {
            if path[index].char().is_some()
                && path[index].modifiers.contains(KeyModifiers::SHIFT)
                && (context.canonicalize_chars
                    || !context
                        .bindings
                        .iter()
                        .any(|binding| binding.keys.starts_with(&path[..=index])))
            {
                path[index].modifiers.remove(KeyModifiers::SHIFT);
            }
        }
        let binding = context.bindings.iter().find(|binding| binding.keys == path);
        let unbound = binding.is_some_and(|binding| binding.action == "unbound");
        if let Some(binding) = binding.filter(|_| !unbound) {
            return Some(Resolution {
                context,
                definition: Some(binding),
                action: &binding.action,
                prefix: binding.prefix,
            });
        }
        if !context.pending.is_empty() && !unbound {
            return Some(Resolution {
                context,
                definition: None,
                action: "cancel key sequence",
                prefix: false,
            });
        }
        let text = input.len() == 1
            && input[0].char().is_some()
            && !input[0]
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let action = match context.fallback {
            Fallback::Text if text => "insert text",
            Fallback::Prompt if text => "insert query text",
            Fallback::Prompt => "ignore unbound key",
            Fallback::Literal => "literal command input",
            Fallback::LiteralText if text => "literal command input",
            Fallback::Consume => "ignore unbound key",
            _ => continue,
        };
        return Some(Resolution {
            context,
            definition: None,
            action,
            prefix: false,
        });
    }
    None
}

fn parent(table: &str) -> Option<&'static str> {
    match table {
        "keys.sidebar" | "keys.sidebar-filter" => Some("keys.file-explorer"),
        "keys.buffer-picker"
        | "keys.quicklist-picker"
        | "keys.git-picker"
        | "keys.branch-picker"
        | "keys.worktree-picker"
        | "keys.stash-picker"
        | "keys.history-picker"
        | "keys.file-explorer" => Some("keys.picker"),
        _ => None,
    }
}

/// Reconstruct precedence from explicit layers and the same built-in maps.
/// A picker-specific default is applied after shared picker configuration.
pub fn origins(
    definition: &Definition,
    configured: &[(Source, Definition)],
    defaults: &[Definition],
) -> Vec<String> {
    fn collect(
        table: &str,
        keys: &[KeyEvent],
        configured: &[(Source, Definition)],
        defaults: &[Definition],
        output: &mut Vec<String>,
    ) {
        for source in [Source::Workspace, Source::User] {
            if let Some((_, binding)) = configured.iter().rev().find(|(layer, binding)| {
                *layer == source && binding.table == table && binding.keys == keys
            }) {
                output.push(format!("{} [{}]: {}", source.name(), table, binding.action));
            }
        }
        let binding = defaults
            .iter()
            .find(|binding| binding.table == table && binding.keys == keys);
        let inherited = parent(table);
        let parent_binding = inherited.and_then(|parent| {
            defaults
                .iter()
                .find(|binding| binding.table == parent && binding.keys == keys)
        });
        if let Some(binding) = binding
            && parent_binding.is_none_or(|parent| parent.action != binding.action)
        {
            output.push(format!("built-in [{}]: {}", table, binding.action));
        }
        if let Some(parent) = inherited {
            collect(parent, keys, configured, defaults, output);
        }
    }
    let mut output = Vec::new();
    collect(
        &definition.table,
        &definition.keys,
        configured,
        defaults,
        &mut output,
    );
    if output.is_empty() {
        output.push(format!(
            "runtime [{}]: {}",
            definition.table, definition.action
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, keymap::component::PickerAction};

    #[test]
    fn pending_prefix_owns_cancellation_and_shift_normalization() {
        let config = Config::default();
        let mut context = Context::new(
            "Picker",
            component("keys.picker", &config.component_keys.picker),
        );
        context.pending = vec!["C-x".parse().unwrap()];
        for key in ["esc", "C-c", "!"] {
            assert_eq!(
                resolve(&[context.clone()], &[key.parse().unwrap()])
                    .unwrap()
                    .action,
                "cancel key sequence"
            );
        }
        assert_eq!(
            resolve(&[context], &["S-G".parse().unwrap()])
                .unwrap()
                .action,
            "move_end"
        );
    }

    #[test]
    fn inspection_tracks_explicit_layers_and_specific_inheritance() {
        let user = "[keys.picker.C-x]\ns = 'move_up'\n[keys.buffer-picker.C-x]\nd = 'close_buffer'"
            .to_owned();
        let local = "[keys.picker.C-x]\ns = 'move_down'".to_owned();
        let config = Config::load(Ok(&user), Ok(local)).unwrap();
        let defaults = components(&Config::default().component_keys);
        let definitions = component("keys.buffer-picker", &config.component_keys.buffer_picker);
        let binding = |suffix: &str| {
            definitions
                .iter()
                .find(|binding| binding.keys == ["C-x".parse().unwrap(), suffix.parse().unwrap()])
                .unwrap()
        };
        let sources = origins(binding("s"), &config.keybinding_sources, &defaults);
        assert!(
            sources[0].starts_with("workspace [keys.picker]: move_down"),
            "{sources:?}"
        );
        assert!(sources[1].starts_with("user [keys.picker]: move_up"));
        let sources = origins(binding("d"), &config.keybinding_sources, &defaults);
        assert!(
            sources[0].starts_with("user [keys.buffer-picker]: close_buffer"),
            "{sources:?}"
        );
        assert!(sources[1].starts_with("built-in [keys.buffer-picker]"));
        let keys = crate::keymap::component::ComponentKeymaps::default();
        assert_eq!(action_name(&PickerAction::Accept), "accept");
        assert!(component("keys.picker", &keys.picker)
            .iter()
            .any(|binding| binding.action == "inspect_key"));
    }

    #[test]
    fn text_fallback_and_unbound_respect_context_order() {
        let mut prompt = Context::new("Prompt", vec![]);
        prompt.bindings.push(Definition {
            table: "keys.prompt".into(),
            keys: vec!["j".parse().unwrap()],
            action: "unbound".into(),
            description: "Use text fallback".into(),
            prefix: false,
        });
        prompt.fallback = Fallback::Prompt;
        let mut editor = Context::new(
            "Editor",
            editor(
                "keys.normal",
                &crate::keymap::default()[&view::document::Mode::Normal],
            ),
        );
        editor.canonicalize_chars = true;
        assert_eq!(
            resolve(&[prompt.clone(), editor.clone()], &["j".parse().unwrap()])
                .unwrap()
                .action,
            "insert query text"
        );
        assert_eq!(
            resolve(&[prompt, editor], &["C-s".parse().unwrap()])
                .unwrap()
                .action,
            "ignore unbound key"
        );
    }
}

//! Command representation, parsing, and execution for terminal keybindings.

use std::fmt;

use anyhow::{anyhow, ensure};
use serde::de::{self, Deserialize, Deserializer};
use ui_core::input::{self, KeyEvent};

use super::{catalog::TYPABLE_COMMAND_MAP, command_line::execute_command_line, context::Context};
use crate::{compositor, ui::PromptEvent};

/// MappableCommands are commands that can be bound to keys, executable in
/// normal, insert or select mode.
///
/// There are three kinds:
///
/// * Static: commands usually bound to keys and used for editing, movement,
///   etc., for example `move_char_left`.
/// * Typable: commands executable from command mode, prefixed with a `:`,
///   for example `:write!`.
/// * Macro: a sequence of keys to execute, for example `@miw`.
#[derive(Clone)]
pub enum MappableCommand {
    Typable {
        name: String,
        args: String,
        doc: String,
    },
    Static {
        name: &'static str,
        fun: fn(cx: &mut Context),
        doc: &'static str,
    },
    Macro {
        name: String,
        keys: Vec<KeyEvent>,
    },
}

impl MappableCommand {
    pub fn execute(&self, cx: &mut Context) {
        match &self {
            Self::Typable { name, args, doc: _ } => {
                let mut command_cx = compositor::Context {
                    config: cx.config,
                    editor: cx.editor,
                    jobs: cx.jobs,
                    scroll: None,
                    image_picker: None,
                    is_cursor_owner: false,
                };
                // Explicit keybindings and palette entries keep targeting their
                // registered command even when configuration defines an alias.
                let input = format!("^{name} {args}");
                match execute_command_line(&mut command_cx, &input, PromptEvent::Validate) {
                    Ok(callbacks) => cx.callback.extend(callbacks),
                    Err(err) => command_cx.editor.set_error(|| err.to_string()),
                }
            }
            Self::Static { fun, .. } => (fun)(cx),
            Self::Macro { keys, .. } => {
                // Protect against recursive macros.
                if cx.editor.macro_replaying.contains(&'@') {
                    cx.editor.set_error(|| {
                        "Cannot execute macro because the [@] register is already playing a macro"
                    });
                    return;
                }
                cx.editor.macro_replaying.push('@');
                let keys = keys.clone();
                cx.callback.push(Box::new(move |compositor, cx| {
                    for key in keys.into_iter() {
                        compositor.handle_event(&compositor::Event::Key(key), cx);
                    }
                    cx.editor.macro_replaying.pop();
                }));
            }
        }
    }

    pub fn name(&self) -> &str {
        match &self {
            Self::Typable { name, .. } => name,
            Self::Static { name, .. } => name,
            Self::Macro { name, .. } => name,
        }
    }

    pub fn doc(&self) -> &str {
        match &self {
            Self::Typable { doc, .. } => doc,
            Self::Static { doc, .. } => doc,
            Self::Macro { name, .. } => name,
        }
    }
}

impl fmt::Debug for MappableCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MappableCommand::Static { name, .. } => {
                f.debug_tuple("MappableCommand").field(name).finish()
            }
            MappableCommand::Typable { name, args, .. } => f
                .debug_tuple("MappableCommand")
                .field(name)
                .field(args)
                .finish(),
            MappableCommand::Macro { name, keys, .. } => f
                .debug_tuple("MappableCommand")
                .field(name)
                .field(keys)
                .finish(),
        }
    }
}

impl fmt::Display for MappableCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for MappableCommand {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(suffix) = s.strip_prefix(':') {
            let (name, args, _) = command_line::split(suffix);
            ensure!(!name.is_empty(), "Expected typable command name");
            let command = TYPABLE_COMMAND_MAP.get(name).map(|cmd| {
                let doc = if args.is_empty() {
                    cmd.doc.to_string()
                } else {
                    format!(":{} {:?}", cmd.name, args)
                };
                MappableCommand::Typable {
                    name: cmd.name.to_owned(),
                    doc,
                    args: args.to_string(),
                }
            });
            // Like Helix PR #8675, defer plugin command resolution until execution.
            // Qualified names keep ordinary built-in typos invalid in configuration.
            command
                .or_else(|| {
                    is_plugin_command(name).then(|| Self::Typable {
                        name: name.to_owned(),
                        args: args.to_owned(),
                        doc: format!(":{name}"),
                    })
                })
                .ok_or_else(|| anyhow!("No TypableCommand named '{}'", s))
        } else if let Some(suffix) = s.strip_prefix('@') {
            input::parse_macro(suffix).map(|keys| Self::Macro {
                name: s.to_string(),
                keys,
            })
        } else {
            MappableCommand::STATIC_COMMAND_LIST
                .iter()
                .find(|cmd| cmd.name() == s)
                .cloned()
                .ok_or_else(|| anyhow!("No command named '{}'", s))
        }
    }
}

fn is_plugin_command(name: &str) -> bool {
    let Some((plugin, command)) = name.split_once('.') else {
        return false;
    };
    let valid_component = |component: &str| {
        !component.is_empty()
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    valid_component(plugin) && valid_component(command)
}

impl<'de> Deserialize<'de> for MappableCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl PartialEq for MappableCommand {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                MappableCommand::Typable {
                    name: first_name,
                    args: first_args,
                    ..
                },
                MappableCommand::Typable {
                    name: second_name,
                    args: second_args,
                    ..
                },
            ) => first_name == second_name && first_args == second_args,
            (
                MappableCommand::Static {
                    name: first_name, ..
                },
                MappableCommand::Static {
                    name: second_name, ..
                },
            ) => first_name == second_name,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defers_qualified_plugin_keybinding_resolution() {
        let command: MappableCommand = ":example-plugin.say_hello 'hello world'".parse().unwrap();
        let MappableCommand::Typable { name, args, .. } = command else {
            panic!("expected a typable plugin command");
        };
        assert_eq!(name, "example-plugin.say_hello");
        assert_eq!(args, "'hello world'");
    }

    #[test]
    fn rejects_unqualified_and_malformed_unknown_commands() {
        for command in [
            ":wriet",
            ":example.",
            ":.hello",
            ":a.b.c",
            ":plug.hello/there",
        ] {
            assert!(command.parse::<MappableCommand>().is_err(), "{command}");
        }
        assert!(":write".parse::<MappableCommand>().is_ok());
    }
}

//! Command representation, parsing, and execution for terminal keybindings.

use std::fmt;

use anyhow::{anyhow, ensure};
use command_line::Args;
use serde::de::{self, Deserialize, Deserializer};
use ui_core::input::{self, KeyEvent};

use super::{context::Context, typed};
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
                if let Some(command) = typed::TYPABLE_COMMAND_MAP.get(name.as_str()) {
                    let mut cx = compositor::Context {
                        config: cx.config,
                        editor: cx.editor,
                        jobs: cx.jobs,
                        scroll: None,
                        image_picker: None,
                    };
                    if let Err(e) = typed::execute_command(
                        &mut cx,
                        command,
                        args,
                        &Args::empty(),
                        PromptEvent::Validate,
                    ) {
                        cx.editor.set_error(|| format!("{}", e));
                    }
                } else {
                    cx.editor.set_error(|| format!("no such command: '{name}'"));
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
            typed::TYPABLE_COMMAND_MAP
                .get(name)
                .map(|cmd| {
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

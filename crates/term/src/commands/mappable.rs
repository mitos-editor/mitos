//! Command representation, parsing, and execution for terminal keybindings.

use std::fmt;

use ::command_line;
use anyhow::{anyhow, ensure};
use serde::de::{self, Deserialize, Deserializer};
use ui_core::input::{self, KeyEvent};

use super::{catalog::TYPABLE_COMMAND_MAP, command_line::execute_keybinding, context::Context};
use crate::compositor;

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
        preserves_completion: bool,
    },
    Macro {
        name: String,
        keys: Vec<KeyEvent>,
        doc: String,
    },
}

impl MappableCommand {
    /// Resolve typable keybindings against the configured command table later.
    pub(crate) fn from_keybinding(input: &str) -> anyhow::Result<Self> {
        let Some(suffix) = input.strip_prefix(':') else {
            return input.parse();
        };
        let (name, args, _) = command_line::split(suffix);
        ensure!(!name.is_empty(), "Expected typable command name");
        let builtin = name
            .strip_prefix(editor_config::CustomCommand::ESCAPE)
            .unwrap_or(name);
        let doc = TYPABLE_COMMAND_MAP
            .get(builtin)
            .filter(|_| args.is_empty())
            .map_or_else(|| input.to_owned(), |command| command.doc.to_owned());
        Ok(Self::Typable {
            name: name.to_owned(),
            args: args.to_owned(),
            doc,
        })
    }

    pub fn execute(&self, cx: &mut Context) {
        self.execute_inner(cx);
        if cx.editor.mode() == view::document::Mode::Insert
            && !matches!(
                self,
                Self::Static {
                    preserves_completion: true,
                    ..
                }
            )
        {
            cx.editor.cancel_completions();
        }
        cx.flush_completion_updates();
    }

    fn execute_inner(&self, cx: &mut Context) {
        match &self {
            Self::Typable { name, args, doc: _ } => {
                let mut context = compositor::Context {
                    config: cx.config,
                    editor: cx.editor,
                    interaction: cx.interaction,
                    jobs: cx.jobs,
                    scroll: None,
                    image_picker: None,
                    is_cursor_owner: false,
                };
                match execute_keybinding(&mut context, name, args) {
                    Ok(callbacks) => cx.callback.extend(callbacks),
                    Err(error) => cx.editor.set_error(|| error.to_string()),
                }
            }
            Self::Static { fun, .. } => (fun)(cx),
            Self::Macro { keys, .. } => {
                // Protect against recursive macros.
                if cx.interaction.macro_replaying.contains(&'@') {
                    cx.editor.set_error(|| {
                        "Cannot execute macro because the [@] register is already playing a macro"
                    });
                    return;
                }
                cx.interaction.macro_replaying.push('@');
                let keys = keys.clone();
                cx.callback.push(Box::new(move |compositor, cx| {
                    for key in keys.into_iter() {
                        compositor.handle_event(&compositor::Event::Key(key), cx);
                    }
                    cx.interaction.macro_replaying.pop();
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

    /// Override the keybinding description without changing its command identity.
    pub fn with_label(mut self, label: String) -> anyhow::Result<Self> {
        if !label.is_empty() {
            match &mut self {
                Self::Typable { doc, .. } | Self::Macro { doc, .. } => *doc = label,
                Self::Static { .. } => {
                    return Err(anyhow!(
                        "custom labels are only available for typable commands and macros"
                    ));
                }
            }
        }
        Ok(self)
    }

    pub fn doc(&self) -> &str {
        match &self {
            Self::Typable { doc, .. } => doc,
            Self::Static { doc, .. } => doc,
            Self::Macro { doc, .. } => doc,
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
            TYPABLE_COMMAND_MAP
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
                doc: s.to_string(),
            })
        } else {
            MappableCommand::STATIC_COMMAND_LIST
                .iter()
                .find(|cmd| cmd.name() == s)
                .cloned()
                .or_else(|| {
                    Self::STATIC_COMMAND_ALIASES
                        .iter()
                        .find(|(alias, _)| *alias == s)
                        .map(|(_, command)| command.clone())
                })
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

#[cfg(test)]
mod tests {
    use super::MappableCommand;
    use std::collections::HashSet;

    #[test]
    fn static_aliases_resolve_to_canonical_commands() {
        let mut names: HashSet<_> = MappableCommand::STATIC_COMMAND_LIST
            .iter()
            .map(MappableCommand::name)
            .collect();
        for (alias, command) in MappableCommand::STATIC_COMMAND_ALIASES {
            assert!(names.insert(alias), "Duplicate command name: {alias}");
            assert_eq!(alias.parse::<MappableCommand>().unwrap(), *command);
            assert_eq!(command.name().parse::<MappableCommand>().unwrap(), *command);
            assert!(MappableCommand::STATIC_COMMAND_LIST.contains(command));
        }
    }

    #[test]
    fn legacy_vcs_keybindings_deserialize_to_canonical_commands() {
        let legacy: crate::keymap::KeyTrie = toml::from_str(
            r#"
            h = "git_history"
            d = "diff"
            c = ["changed_file_picker", "diff_refresh"]
            "#,
        )
        .unwrap();
        let canonical: crate::keymap::KeyTrie = toml::from_str(
            r#"
            h = "vcs_history_picker"
            d = "vcs_diff"
            c = ["vcs_changed_file_picker", "vcs_diff_refresh"]
            "#,
        )
        .unwrap();
        assert_eq!(legacy, canonical);
    }

    #[test]
    fn legacy_diagnostic_keybindings_deserialize_to_canonical_commands() {
        let legacy: crate::keymap::KeyTrie = toml::from_str(
            r#"
            a = "goto_first_diag"
            b = "goto_last_diag"
            c = "goto_next_diag"
            d = "goto_prev_diag"
            "#,
        )
        .unwrap();
        let canonical: crate::keymap::KeyTrie = toml::from_str(
            r#"
            a = "goto_first_diagnostic"
            b = "goto_last_diagnostic"
            c = "goto_next_diagnostic"
            d = "goto_prev_diagnostic"
            "#,
        )
        .unwrap();
        assert_eq!(legacy, canonical);
    }

    #[test]
    fn descriptive_command_names_preserve_legacy_keybindings() {
        let legacy: crate::keymap::KeyTrie = toml::from_str(
            r#"
            a = ["join_selections", "rotate_selections_forward", "align_view_middle"]
            b = ["yank_main_selection_to_clipboard", "rsearch", "global_search"]
            c = ["hsplit", "wclose", "till_prev_char"]
            "#,
        )
        .unwrap();
        let canonical: crate::keymap::KeyTrie = toml::from_str(
            r#"
            a = ["join_lines", "cycle_primary_selection_forward", "center_view_horizontally"]
            b = ["yank_primary_selection_to_clipboard", "search_backward", "search_picker_in_current_working_directory"]
            c = ["split_horizontal", "close_view", "find_till_previous_char"]
            "#,
        )
        .unwrap();
        assert_eq!(legacy, canonical);
    }

    #[test]
    fn descriptive_typable_names_preserve_aliases_and_arguments() {
        for (old, new) in [
            ("hsplit notes.txt", "split-horizontal notes.txt"),
            ("sp notes.txt", "split-horizontal notes.txt"),
            ("vsplit notes.txt", "split-vertical notes.txt"),
            ("vs notes.txt", "split-vertical notes.txt"),
            ("hsplit-new", "split-horizontal-new"),
            ("vsplit-new", "split-vertical-new"),
            ("cd src", "change-current-working-directory src"),
            (
                "change-current-directory src",
                "change-current-working-directory src",
            ),
            ("pwd", "show-current-working-directory"),
            ("show-directory", "show-current-working-directory"),
        ] {
            assert_eq!(
                format!(":{old}").parse::<MappableCommand>().unwrap(),
                format!(":{new}").parse::<MappableCommand>().unwrap(),
            );
        }
    }

    #[test]
    fn legacy_typable_vcs_names_resolve_to_canonical_commands() {
        for (old, new) in [
            ("blame", "vcs-blame"),
            ("show-change", "vcs-show-change"),
            ("diff head", "vcs-diff head"),
            ("git-history", "vcs-history"),
            ("file-history", "vcs-file-history"),
            ("git-file-history", "vcs-file-history"),
            ("diff-refresh", "vcs-diff-refresh"),
            ("diff-open-source", "vcs-diff-open-source"),
            ("diff-open-file", "vcs-diff-open-file"),
            ("restore-change", "vcs-restore-change"),
        ] {
            assert_eq!(
                format!(":{old}").parse::<MappableCommand>().unwrap(),
                format!(":{new}").parse::<MappableCommand>().unwrap(),
            );
        }
    }
}

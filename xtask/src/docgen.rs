use crate::helpers;
use crate::path;
use crate::DynError;

use term::commands::MappableCommand;
use term::commands::TYPABLE_COMMAND_LIST;
use term::health::TsFeature;
use view::document::Mode;

use std::collections::HashSet;
use std::fs;

pub const COMMANDS_MD_OUTPUT: &str = "commands.md";
pub const LANG_SUPPORT_MD_OUTPUT: &str = "lang-support.md";

const COMMANDS_HEADER: &str = r#"---
title: Commands
description: Reference for Mitos typable and static commands.
---

- [Typable commands](#typable-commands)
- [Static commands](#static-commands)

## Typable commands

Typable commands are used from command mode and may take arguments. Command mode can be activated by pressing `:`. The built-in typable commands are:

"#;

const STATIC_COMMANDS_HEADER: &str = r#"
## Static commands

Static commands take no arguments and can be bound to keys. Static commands can also be executed from the command picker (`<space>?`). The built-in static commands are:

"#;

const LANGUAGE_SUPPORT_HEADER: &str = r#"---
title: Language support
description: Language features and default language servers supported by Mitos.
---

The following languages and Language Servers are supported. To use
Language Server features, you must first [configure][lsp-config-wiki] the
appropriate Language Server.

You can check the language support in your installed Mitos version with `ms --health`.

Also see the [Language Configuration][lang-config] docs and the [Adding
Languages][adding-languages] guide for more language configuration information.

"#;

const LANGUAGE_SUPPORT_LINKS: &str = r#"
[lsp-config-wiki]: https://github.com/helix-editor/helix/wiki/Language-Server-Configurations
[lang-config]: /languages
[adding-languages]: /guides/adding-languages
"#;

fn md_table_heading(cols: &[String]) -> String {
    let mut header = String::new();
    header += &md_table_row(cols);
    header += &md_table_row(&vec!["---".to_string(); cols.len()]);
    header
}

fn md_table_row(cols: &[String]) -> String {
    format!("| {} |\n", cols.join(" | "))
}

fn md_mono(s: &str) -> String {
    format!("`{}`", s)
}

pub fn typable_commands() -> Result<String, DynError> {
    let mut md = String::new();
    md.push_str(&md_table_heading(&[
        "Name".to_owned(),
        "Description".to_owned(),
    ]));

    // escape | so it doesn't get rendered as a column separator
    let cmdify = |s: &str| format!("`:{}`", s.replace('|', "\\|"));

    for cmd in TYPABLE_COMMAND_LIST {
        let names = std::iter::once(&cmd.name)
            .chain(cmd.aliases.iter())
            .map(|a| cmdify(a))
            .collect::<Vec<_>>()
            .join(", ");

        let doc = cmd.doc.replace('\n', "<br>");

        md.push_str(&md_table_row(&[names.to_owned(), doc.to_owned()]));
    }

    Ok(md)
}

pub fn commands() -> Result<String, DynError> {
    Ok(format!(
        "{}{}{}{}",
        COMMANDS_HEADER,
        typable_commands()?,
        STATIC_COMMANDS_HEADER,
        static_commands()?
    ))
}

pub fn static_commands() -> Result<String, DynError> {
    let mut md = String::new();
    let keymap = term::keymap::default();
    let keymaps = [
        ("normal", keymap[&Mode::Normal].reverse_map()),
        ("select", keymap[&Mode::Select].reverse_map()),
        ("insert", keymap[&Mode::Insert].reverse_map()),
    ];

    md.push_str(&md_table_heading(&[
        "Name".to_owned(),
        "Description".to_owned(),
        "Default keybinds".to_owned(),
    ]));

    for cmd in MappableCommand::STATIC_COMMAND_LIST {
        let keymap_strings: Vec<_> = keymaps
            .iter()
            .map(|(mode, keymap)| {
                let bindings = keymap
                    .get(cmd.name())
                    .map(|bindings| {
                        let mut bind_strings: Vec<_> = bindings
                            .iter()
                            .map(|bind| {
                                let keys = &bind
                                    .iter()
                                    .map(|key| key.key_sequence_format())
                                    .collect::<String>()
                                    // escape | so it doesn't get rendered as a column separator
                                    .replace('|', "\\|");
                                format!("`` {} ``", keys)
                            })
                            .collect();
                        // sort for stable output. sorting by length puts simple
                        // keybindings first and groups similar keys together
                        bind_strings.sort_by_key(|s| (s.len(), s.to_owned()));
                        bind_strings.join(", ")
                    })
                    .unwrap_or_default();

                (mode, bindings)
            })
            .collect();

        let keymap_string = keymap_strings
            .iter()
            .filter(|(_, bindings)| !bindings.is_empty())
            .map(|(mode, bindings)| format!("{}: {}", mode, bindings))
            .collect::<Vec<_>>()
            .join(", ");

        md.push_str(&md_table_row(&[
            md_mono(cmd.name()),
            cmd.doc().to_owned(),
            keymap_string,
        ]));
    }

    Ok(md)
}

pub fn lang_features() -> Result<String, DynError> {
    let mut md = String::new();
    let ts_features = TsFeature::all();

    let mut cols = vec!["Language".to_owned()];
    cols.append(
        &mut ts_features
            .iter()
            .map(|t| t.long_title().to_string())
            .collect::<Vec<_>>(),
    );
    cols.push("Default language servers".to_owned());

    md.push_str(&md_table_heading(&cols));
    let config = editor_core::config::default_lang_config();

    let mut langs = config
        .language
        .iter()
        .map(|l| l.language_id.clone())
        .collect::<Vec<_>>();
    langs.sort_unstable();

    let mut ts_features_to_langs = Vec::new();
    for &feat in ts_features {
        ts_features_to_langs.push((feat, helpers::ts_lang_support(feat)));
    }

    let mut row = Vec::new();
    for lang in langs {
        let lc = config
            .language
            .iter()
            .find(|l| l.language_id == lang)
            .unwrap(); // lang comes from config
        row.push(lc.language_id.clone());

        for (_feat, support_list) in &ts_features_to_langs {
            row.push(
                if support_list.contains(&lang) {
                    "✓"
                } else {
                    ""
                }
                .to_owned(),
            );
        }
        let mut seen_commands = HashSet::new();
        let mut commands = String::new();
        for ls_config in lc
            .language_servers
            .iter()
            .filter_map(|ls| config.language_server.get(&ls.name))
        {
            let command = &ls_config.command;
            if !seen_commands.insert(command) {
                continue;
            }

            if !commands.is_empty() {
                commands.push_str(", ");
            }

            commands.push_str(&md_mono(command));
        }
        row.push(commands);

        md.push_str(&md_table_row(&row));
        row.clear();
    }

    Ok(md)
}

pub fn language_support() -> Result<String, DynError> {
    Ok(format!(
        "{}{}{}",
        LANGUAGE_SUPPORT_HEADER,
        lang_features()?,
        LANGUAGE_SUPPORT_LINKS
    ))
}

pub fn write(filename: &str, data: &str) {
    let error = format!("Could not write to {}", filename);
    let path = path::website_docs().join(filename);
    fs::write(path, data).expect(&error);
}

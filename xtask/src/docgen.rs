use crate::helpers;
use crate::path;
use crate::DynError;

use language::queries::QueryFeature;
use term::commands::MappableCommand;
use term::commands::TYPABLE_COMMAND_LIST;
use view::document::Mode;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use term::keymap::{component::ComponentKeymaps, KeyTrie};
use ui_core::keymap::{Binding, Keymap};

pub const TYPABLE_COMMANDS_MD_OUTPUT: &str = "typable-cmd.md";
pub const STATIC_COMMANDS_MD_OUTPUT: &str = "static-cmd.md";
pub const LANG_SUPPORT_MD_OUTPUT: &str = "lang-support.md";

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
    let ts_features = QueryFeature::all();

    let mut cols = vec!["Language".to_owned()];
    cols.append(
        &mut ts_features
            .iter()
            .map(|t| t.long_title().to_string())
            .collect::<Vec<_>>(),
    );
    cols.push("Default language servers".to_owned());

    md.push_str(&md_table_heading(&cols));
    let config = language::loader::default_lang_config();

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
        let mut commands = String::new();
        for group in lc.language_server_groups() {
            let mut seen_commands = HashSet::new();
            let entries = group
                .iter()
                .filter_map(|server| config.language_server.get(&server.name))
                .filter(|server| seen_commands.insert(&server.command))
                .map(|server| md_mono(&server.command))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            if !commands.is_empty() {
                commands.push_str(", ");
            }
            if group[0].alternative_group.is_some() {
                commands.push_str("first available: ");
                commands.push_str(&entries.join(" → "));
            } else {
                commands.push_str(&entries[0]);
            }
        }
        row.push(commands);

        md.push_str(&md_table_row(&row));
        row.clear();
    }

    Ok(md)
}

pub fn write(filename: &str, data: &str) {
    let error = format!("Could not write to {}", filename);
    let path = path::book_gen().join(filename);
    fs::write(path, data).expect(&error);
}

/// Render the actual trie rather than the reverse map, which omits sequences.
fn keymap_table(md: &mut String, trie: &KeyTrie, prefix: &str, depth: usize) {
    let KeyTrie::Node(node) = trie else { return };
    md.push_str(&md_table_heading(&[
        "Key".to_owned(),
        "Description".to_owned(),
        "Command".to_owned(),
    ]));
    for (key, binding) in node.iter() {
        let keys = key_code(&format!("{prefix}{}", key.key_sequence_format()));
        let (description, commands) = match binding {
            KeyTrie::MappableCommand(cmd) => (table_text(cmd.doc()), command_code(cmd)),
            KeyTrie::Sequence(label, commands) => (
                table_text(label),
                commands
                    .iter()
                    .map(command_code)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            KeyTrie::Node(child) => (
                format!(
                    "{}{}",
                    table_text(&child.infobox().title),
                    if child.is_sticky {
                        " (sticky; Escape to exit)"
                    } else {
                        ""
                    }
                ),
                "Prefix".to_owned(),
            ),
        };
        md.push_str(&md_table_row(&[keys, description, commands]));
    }
    for (key, binding) in node.iter() {
        if let KeyTrie::Node(child) = binding {
            let prefix = format!("{prefix}{}", key.key_sequence_format());
            md.push_str(&format!(
                "\n{} {} — {}\n\n",
                "#".repeat(depth),
                key_code(&prefix),
                child.infobox().title
            ));
            keymap_table(md, binding, &prefix, depth + 1);
        }
    }
}

fn table_text(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

fn key_code(text: &str) -> String {
    // Double backticks also accommodate the literal backtick key.
    format!("`` {} ``", table_text(text))
}

fn command_code(command: &MappableCommand) -> String {
    let text = match command {
        MappableCommand::Typable { name, args, .. } => {
            format!(":{name} {args}").trim_end().to_owned()
        }
        _ => command.name().to_owned(),
    };
    key_code(&text)
}

pub fn keymaps() -> Vec<(String, String)> {
    let defaults = term::keymap::default();
    let mut outputs = Vec::new();
    for (name, mode) in [
        ("normal", Mode::Normal),
        ("select", Mode::Select),
        ("insert", Mode::Insert),
    ] {
        let mut md = String::from("<!-- Generated by cargo xtask docgen. Do not edit. -->\n\n");
        keymap_table(&mut md, &defaults[&mode], "", 3);
        outputs.push((format!("keymap-{name}.md"), md));
    }
    outputs
}

fn component_table<A: serde::Serialize>(keys: &Keymap<A>) -> Result<String, DynError> {
    fn collect<A: serde::Serialize>(
        keys: &Keymap<A>,
        prefix: &str,
        rows: &mut BTreeMap<String, Vec<String>>,
    ) -> Result<(), DynError> {
        for (key, binding) in &keys.0 {
            let prefix = format!("{prefix}{}", key.key_sequence_format());
            match binding {
                Binding::Actions(actions) => {
                    let actions = actions
                        .iter()
                        .map(|action| {
                            let value = serde_json::to_value(action)?;
                            Ok(key_code(
                                value.as_str().ok_or("expected a named component action")?,
                            ))
                        })
                        .collect::<Result<Vec<_>, DynError>>()?
                        .join(", ");
                    rows.entry(actions).or_default().push(prefix);
                }
                Binding::Keys(keys) => collect(keys, &prefix, rows)?,
                Binding::Unbound => {}
            }
        }
        Ok(())
    }
    let mut rows = BTreeMap::new();
    collect(keys, "", &mut rows)?;
    let mut md = md_table_heading(&["Key".to_owned(), "Action".to_owned()]);
    for (action, mut keys) in rows {
        keys.sort_by_key(|key| (key.len(), key.clone()));
        md.push_str(&md_table_row(&[
            keys.iter()
                .map(|key| key_code(key))
                .collect::<Vec<_>>()
                .join(", "),
            action,
        ]));
    }
    Ok(md)
}

pub fn component_keymaps() -> Result<String, DynError> {
    let keys = ComponentKeymaps::default();
    let mut md = String::from("<!-- Generated by cargo xtask docgen. Do not edit. -->\n\n");
    macro_rules! section {
        ($title:literal, $field:ident) => {
            md.push_str(concat!("## ", $title, "\n\n"));
            md.push_str(&component_table(&keys.$field)?);
            md.push('\n');
        };
    }
    section!("Picker", picker);
    section!("Buffer picker", buffer_picker);
    section!("Prompt", prompt);
    section!("Completion menu", menu);
    section!("Popup", popup);
    section!("Hover", hover);
    section!("Signature-help popup", signature_help);
    section!("Blame", blame);
    section!("Diff popup", diff);
    md.pop();
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_preserves_prefixes_sequences_and_typable_arguments() {
        let keys: KeyTrie = toml::from_str(
            r#"
            [g]
            "|" = { label = "Save | close", command = [":write notes.txt", ":quit"] }
            "`" = "switch_to_lowercase"
        "#,
        )
        .unwrap();
        let mut md = String::new();
        keymap_table(&mut md, &keys, "", 3);
        assert!(
            md.contains("| `` g\\| `` | Save \\| close | `` :write notes.txt ``, `` :quit `` |")
        );
        assert!(md.contains("| `` g` `` |"));
        assert!(md.contains("`` switch_to_lowercase ``"));
    }

    #[test]
    fn component_bindings_are_sorted_and_keep_action_sequences() {
        use term::keymap::component::PromptAction;
        let keys: Keymap<PromptAction> = toml::from_str(
            r#"
            "C-c" = "cancel"
            esc = "cancel"
            [g]
            x = ["move_start", "delete_to_end"]
        "#,
        )
        .unwrap();
        let md = component_table(&keys).unwrap();
        assert!(md.contains("| `` <C-c> ``, `` <esc> `` | `` cancel `` |"));
        assert!(md.contains("| `` gx `` | `` move_start ``, `` delete_to_end `` |"));
    }
}

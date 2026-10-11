//! Compatibility normalization runs on each source before layering configuration.

use toml::Value;

mod rewrite;
pub use rewrite::{file, source, FileMigration};

type Moves = Vec<(String, String)>;

/// Legacy editor paths and their canonical equivalents. Container aliases also
/// apply to command paths beneath them, such as `auto-reload.poll.interval`.
pub const EDITOR_PATHS: &[(&str, &str)] = &[
    ("scrolloff", "scroll.padding"),
    ("scroll-lines", "scroll.wheel-lines"),
    ("mouse-yank-register", "mouse.yank-register"),
    ("middle-click-paste", "mouse.middle-click-paste"),
    ("cursorline", "cursor.highlight-line"),
    ("cursorcolumn", "cursor.highlight-column"),
    ("cursor-shape", "cursor.shape"),
    ("auto-completion", "completion.auto-trigger"),
    ("path-completion", "completion.path.enable"),
    ("word-completion", "completion.word"),
    ("completion-timeout", "completion.trigger-delay"),
    ("completion-trigger-len", "completion.trigger-length"),
    ("preview-completion-insert", "completion.preview-insert"),
    ("completion-replace", "completion.replace"),
    (
        "inline-completion-timeout",
        "inline-completion.trigger-delay",
    ),
    (
        "inline-completion-auto-trigger",
        "inline-completion.auto-trigger",
    ),
    ("auto-format", "format.on-save"),
    ("default-yank-register", "clipboard.default-yank-register"),
    ("clipboard-provider", "clipboard.provider"),
    ("auto-save", "files.auto-save"),
    ("auto-reload", "files.reload"),
    ("file-watcher", "files.watch"),
    ("default-line-ending", "files.default-line-ending"),
    ("editor-config", "files.editor-config"),
    ("atomic-save", "files.save.atomic"),
    ("insert-final-newline", "files.save.insert-final-newline"),
    ("trim-final-newlines", "files.save.trim-final-newlines"),
    (
        "trim-trailing-whitespace",
        "files.save.trim-trailing-whitespace",
    ),
    ("color-modes", "statusline.mode.color"),
    ("smart-tab", "indent.smart-tab"),
    ("indent-heuristic", "indent.heuristic"),
    ("inline-diagnostics", "diagnostics.inline"),
    (
        "end-of-line-diagnostics",
        "diagnostics.end-of-line.severity",
    ),
    ("workspace-lsp-roots", "lsp.workspace-roots"),
    ("lsp.display-progress-messages", "lsp.progress.enable"),
    ("lsp.display-messages", "lsp.messages.enable"),
    ("lsp.auto-signature-help", "lsp.signature-help.auto-trigger"),
    (
        "lsp.display-signature-help-docs",
        "lsp.signature-help.show-documentation",
    ),
    ("lsp.display-inlay-hints", "lsp.inlay-hints.enable"),
    (
        "lsp.auto-document-highlight",
        "lsp.document-highlight.auto-trigger",
    ),
    (
        "lsp.inlay-hints-length-limit",
        "lsp.inlay-hints.length-limit",
    ),
    ("lsp.display-color-swatches", "lsp.color-swatches.enable"),
    ("lsp.display-code-lenses", "lsp.code-lenses.enable"),
    ("lsp.snippets", "lsp.completion.snippets"),
    (
        "lsp.goto-reference-include-declaration",
        "lsp.references.include-declaration",
    ),
];

/// Canonical path relative to `[editor]`, also used by runtime commands.
pub fn editor_path(path: &str) -> String {
    for (old, new) in EDITOR_PATHS {
        if path == *old {
            return (*new).into();
        }
        if let Some(suffix) = path.strip_prefix(old).and_then(|s| s.strip_prefix('.')) {
            return format!("{new}.{suffix}");
        }
    }
    match path {
        "mouse" => "mouse.enable",
        "bufferline" => "bufferline.show",
        "rainbow-brackets" => "rainbow-brackets.enable",
        "auto-pairs" => "auto-pairs.enable",
        "true-color" => "terminal.true-color",
        "undercurl" => "terminal.undercurl",
        "kitty-keyboard-protocol" => "terminal.keyboard.kitty-protocol",
        "terminal.command" => "terminal.launcher.command",
        "terminal.args" => "terminal.launcher.args",
        _ => return path.into(),
    }
    .into()
}

fn error(message: impl std::fmt::Display) -> toml::de::Error {
    serde::de::Error::custom(message)
}

fn take(value: &mut Value, path: &str) -> Option<Value> {
    let (key, rest) = path.split_once('.').unwrap_or((path, ""));
    let table = value.as_table_mut()?;
    if rest.is_empty() {
        table.remove(key)
    } else {
        take(table.get_mut(key)?, rest)
    }
}

fn put_parts(
    value: &mut Value,
    keys: &[&str],
    full_path: &str,
    item: Value,
) -> Result<(), toml::de::Error> {
    let (key, rest) = keys.split_first().unwrap();
    let table = value
        .as_table_mut()
        .ok_or_else(|| error(format!("expected table for `{full_path}`")))?;
    if rest.is_empty() {
        if table.contains_key(*key) {
            return Err(error(format!(
                "duplicate configuration setting `{full_path}` (legacy and canonical paths)"
            )));
        }
        table.insert((*key).into(), item);
        Ok(())
    } else {
        put_parts(
            table
                .entry((*key).to_owned())
                .or_insert_with(|| Value::Table(Default::default())),
            rest,
            full_path,
            item,
        )
    }
}

fn rename(
    value: &mut Value,
    old: &str,
    new: &str,
    moves: &mut Moves,
) -> Result<(), toml::de::Error> {
    if let Some(item) = take(value, old) {
        // A container alias may coexist with a canonical table when they set
        // different leaves; duplicate leaves are always rejected.
        insert_leaves(value, new, item)?;
        moves.push((old.into(), new.into()));
    }
    Ok(())
}

fn insert_leaves(value: &mut Value, path: &str, item: Value) -> Result<(), toml::de::Error> {
    fn insert(value: &mut Value, keys: &mut Vec<&str>, item: Value) -> Result<(), toml::de::Error> {
        if let Value::Table(table) = item {
            // Preserve even empty tables for strict schema validation.
            if table.is_empty() {
                let existing = keys.iter().try_fold(&*value, |value, key| value.get(*key));
                if existing.is_some_and(Value::is_table) {
                    return Ok(());
                }
                put_parts(value, keys, &keys.join("."), Value::Table(table))
            } else {
                for (key, item) in table {
                    // A TOML key may contain a literal dot, e.g. an auto-pair.
                    let mut child_keys = keys.clone();
                    child_keys.push(&key);
                    insert(value, &mut child_keys, item)?;
                }
                Ok(())
            }
        } else {
            put_parts(value, keys, &keys.join("."), item)
        }
    }
    insert(value, &mut path.split('.').collect(), item)
}

fn scalar(
    value: &mut Value,
    key: &str,
    leaf: &str,
    moves: &mut Moves,
) -> Result<(), toml::de::Error> {
    if value.get(key).is_some_and(|v| !v.is_table()) {
        rename(value, key, &format!("{key}.{leaf}"), moves)?;
    }
    Ok(())
}

fn auto_pairs(value: &mut Value, moves: &mut Moves) -> Result<(), toml::de::Error> {
    let Some(item) = value.get("auto-pairs") else {
        return Ok(());
    };
    let leaf = if item.is_bool() {
        "enable"
    } else if item
        .as_table()
        .is_some_and(|pairs| pairs.keys().all(|k| k.chars().count() == 1))
    {
        "pairs"
    } else {
        return Ok(());
    };
    rename(value, "auto-pairs", &format!("auto-pairs.{leaf}"), moves)
}

/// Normalize one editor table, preserving omitted fields for inheritance.
pub fn editor(value: &mut Value) -> Result<(), toml::de::Error> {
    editor_moves(value, &mut Vec::new())
}

fn editor_moves(value: &mut Value, moves: &mut Moves) -> Result<(), toml::de::Error> {
    scalar(value, "mouse", "enable", moves)?;
    if value.get("auto-save").is_some_and(Value::is_bool) {
        rename(value, "auto-save", "auto-save.focus-lost", moves)?;
    }
    for (old, new) in EDITOR_PATHS {
        rename(value, old, new, moves)?;
    }
    scalar(value, "bufferline", "show", moves)?;
    scalar(value, "rainbow-brackets", "enable", moves)?;
    auto_pairs(value, moves)?;
    if value.get("gutters").is_some_and(Value::is_array) {
        rename(value, "gutters", "gutters.layout", moves)?;
    }
    // The old boolean shorthand only enables save-on-focus-loss.
    if value
        .get("files")
        .and_then(|v| v.get("auto-save"))
        .is_some_and(Value::is_bool)
    {
        rename(
            value,
            "files.auto-save",
            "files.auto-save.focus-lost",
            moves,
        )?;
    }
    Ok(())
}

/// Normalize one config.toml source, including frontend-owned terminal settings.
pub fn config(value: &mut Value) -> Result<(), toml::de::Error> {
    config_moves(value, &mut Vec::new())
}

fn config_moves(value: &mut Value, moves: &mut Moves) -> Result<(), toml::de::Error> {
    for (old, new) in [
        ("editor.true-color", "terminal.true-color"),
        ("editor.undercurl", "terminal.undercurl"),
        (
            "editor.kitty-keyboard-protocol",
            "terminal.keyboard.kitty-protocol",
        ),
        ("editor.terminal", "terminal.launcher"),
    ] {
        rename(value, old, new, moves)?;
    }
    if let Some(editor_value) = value.get_mut("editor") {
        let mut editor_changes = Vec::new();
        editor_moves(editor_value, &mut editor_changes)?;
        moves.extend(
            editor_changes
                .into_iter()
                .map(|(old, new)| (format!("editor.{old}"), format!("editor.{new}"))),
        );
    }
    Ok(())
}

/// Editor tables merge recursively; ordinary arrays replace earlier values.
pub fn merge(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Table(mut left), Value::Table(right)) => {
            for (key, value) in right {
                let value = match left.remove(&key) {
                    Some(old) => merge(old, value),
                    None => value,
                };
                left.insert(key, value);
            }
            Value::Table(left)
        }
        (_, right) => right,
    }
}

/// Only language and grammar definitions are arrays merged by name. All
/// feature lists (servers, format actions, rulers, etc.) replace earlier lists.
pub fn merge_languages(mut left: Value, mut right: Value) -> Value {
    if !left.is_table() || !right.is_table() {
        return merge(left, right);
    }
    for key in ["language", "grammar"] {
        if let Some(Value::Array(entries)) = right.get(key) {
            let entries = entries.clone();
            right.as_table_mut().unwrap().remove(key);
            let table = left.as_table_mut().unwrap();
            let current = table
                .entry(key.to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(current) = current.as_array_mut() else {
                table.insert(key.into(), Value::Array(entries));
                continue;
            };
            for entry in entries {
                if let Some(index) = current.iter().position(|old| {
                    entry.get("name").is_some() && old.get("name") == entry.get("name")
                }) {
                    // Grammar sources choose a local or Git variant. Replace
                    // that table as a unit, preserving the existing policy.
                    let source = (key == "grammar")
                        .then(|| entry.get("source").cloned())
                        .flatten();
                    current[index] = merge(current[index].clone(), entry);
                    if let Some(source) = source {
                        current[index]
                            .as_table_mut()
                            .unwrap()
                            .insert("source".into(), source);
                    }
                } else {
                    current.push(entry);
                }
            }
        }
    }
    merge(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_source_overrides_replace_the_previous_variant() {
        let local =
            toml::from_str("[[grammar]]\nname = 'example'\nsource.path = '/tmp/example'").unwrap();
        let git = toml::from_str("[[grammar]]\nname = 'example'\nsource = { git = 'https://example.com/grammar', rev = 'main' }").unwrap();
        let merged = merge_languages(local, git);
        let source = &merged["grammar"][0]["source"];
        assert!(source.get("path").is_none());
        assert_eq!(source["rev"].as_str(), Some("main"));
    }
}

//! Lossless migration using the same moves as compatibility loading.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use toml_edit::{DocumentMut, Item, Key, Table};

/// Rewrite legacy settings without adding defaults or touching unrelated tables.
/// Already canonical sources are returned unchanged as `None`.
pub fn source(input: &str) -> Result<Option<String>> {
    let mut expected: toml::Value = toml::from_str(input)?;
    let mut moves = Vec::new();
    super::config_moves(&mut expected, &mut moves)?;
    if moves.is_empty() {
        return Ok(None);
    }

    let mut document: DocumentMut = input.parse()?;
    for (old, new) in moves {
        let (key, item) = take(document.as_item_mut(), &old)
            .with_context(|| format!("could not migrate `{old}`"))?;
        insert(document.as_item_mut(), &new, key, item)?;
    }
    let output = document.to_string();
    // Check the serialized document itself before allowing a disk write.
    let actual: toml::Value = toml::from_str(&output)?;
    ensure!(actual == expected, "migration changed configuration values");
    Ok(Some(output))
}

fn take(parent: &mut Item, path: &str) -> Option<(Key, Item)> {
    let (key, rest) = path.split_once('.').unwrap_or((path, ""));
    let table = parent.as_table_like_mut()?;
    let result = if rest.is_empty() {
        let key = table.key(key)?.clone();
        let item = table.remove(key.get())?;
        Some((key, item))
    } else {
        take(table.get_mut(key)?, rest)
    };
    // Removing a whole subsection can leave an implicit ancestor empty.
    // Keep that table explicit so serialization preserves the normalized value.
    if result.is_some()
        && let Some(table) = parent.as_table_mut()
        && table.is_empty()
    {
        table.set_implicit(false);
        table.set_dotted(false);
    }
    result
}

fn insert(parent: &mut Item, path: &str, key: Key, item: Item) -> Result<()> {
    let (name, rest) = path.split_once('.').unwrap_or((path, ""));
    let inline = parent.is_inline_table();
    let table = parent
        .as_table_like_mut()
        .with_context(|| format!("expected table for `{path}`"))?;
    if !rest.is_empty() {
        let child = table.entry(name).or_insert_with(|| {
            if inline {
                Item::Value(toml_edit::Value::InlineTable(Default::default()))
            } else {
                let mut table = Table::new();
                table.set_implicit(true);
                Item::Table(table)
            }
        });
        return insert(child, rest, key, item);
    }

    let key = if key.get() == name {
        key
    } else {
        Key::new(name).with_leaf_decor(key.leaf_decor().clone())
    };
    insert_key(parent, key, item)
}

// Keys within a moved table are literal, including quoted dots and auto-pairs.
fn insert_key(parent: &mut Item, mut key: Key, item: Item) -> Result<()> {
    let inline = parent.is_inline_table();
    let table = parent
        .as_table_like_mut()
        .context("expected destination table")?;
    if let Some(existing) = table.get_mut(key.get()) {
        // Disjoint legacy and canonical tables may coexist. Duplicate leaves
        // have already been rejected by the shared normalization pass.
        let old = item.as_table_like().context("expected migrated table")?;
        for (name, child) in old.iter() {
            let key = old.key(name).unwrap().clone();
            insert_key(existing, key, child.clone())?;
        }
        // Container-level comments would otherwise disappear when the
        // legacy table is merged into an existing canonical table.
        let mut comments = String::new();
        let decor = item
            .as_table()
            .map(Table::decor)
            .or_else(|| item.as_value().map(toml_edit::Value::decor));
        for raw in [
            key.leaf_decor().prefix(),
            decor.and_then(|d| d.prefix()),
            decor.and_then(|d| d.suffix()),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(text) = raw.as_str().filter(|s| s.contains('#')) {
                comments.push_str(text);
                if !text.ends_with('\n') {
                    comments.push('\n');
                }
            }
        }
        if !comments.is_empty() {
            if let Some(table) = existing.as_table_mut() {
                prepend_comments(table.decor_mut(), &comments);
            } else {
                let mut key = table.key_mut(key.get()).unwrap();
                prepend_comments(key.leaf_decor_mut(), &comments);
            }
        }
    } else {
        // Converting a header-style table to an inline value clears its
        // decorators. Keep those comments beside the new inline entry.
        if inline && let Some(table) = item.as_table() {
            let mut comments = String::new();
            table_comments(table, &mut comments);
            prepend_comments(key.leaf_decor_mut(), &comments);
        }
        if inline {
            let value = item
                .into_value()
                .map_err(|_| anyhow::anyhow!("expected inline value"))?;
            // TableLike::entry_format does not preserve key decorators for
            // inline tables; use the formatted insertion API directly.
            parent
                .as_inline_table_mut()
                .unwrap()
                .insert_formatted(&key, value);
        } else {
            parent.as_table_mut().unwrap().insert_formatted(&key, item);
        }
    }
    Ok(())
}

fn table_comments(table: &Table, comments: &mut String) {
    fn add(decor: &toml_edit::Decor, comments: &mut String) {
        for raw in [decor.prefix(), decor.suffix()].into_iter().flatten() {
            if let Some(text) = raw.as_str().filter(|s| s.contains('#')) {
                comments.push_str(text);
                if !text.ends_with('\n') {
                    comments.push('\n');
                }
            }
        }
    }
    add(table.decor(), comments);
    for (name, item) in table.iter() {
        let key = table.key(name).unwrap();
        add(key.leaf_decor(), comments);
        add(key.dotted_decor(), comments);
        if let Some(table) = item.as_table() {
            table_comments(table, comments);
        } else if let Some(value) = item.as_value() {
            add(value.decor(), comments);
        }
    }
}

fn prepend_comments(decor: &mut toml_edit::Decor, comments: &str) {
    let prefix = decor.prefix().and_then(|p| p.as_str()).unwrap_or("");
    decor.set_prefix(format!("{comments}{prefix}"));
}

/// Exact bytes written by a migration, also used to preserve workspace trust.
#[derive(Debug)]
pub struct FileMigration {
    pub target: PathBuf,
    pub backup: PathBuf,
    pub original: String,
    pub updated: String,
}

/// Migrate one file after frontend validation, backing up the original and
/// replacing it atomically. Resolve symlinks so dotfile-manager links survive.
/// Returns the migration details, or `None` when no migration was needed.
pub fn file(
    path: &Path,
    validate: impl FnOnce(&str) -> Result<()>,
) -> Result<Option<FileMigration>> {
    let path = fs::canonicalize(path)
        .with_context(|| format!("could not resolve config {}", path.display()))?;
    let original = fs::read_to_string(&path)?;
    let Some(output) = source(&original)? else {
        validate(&original)?;
        return Ok(None);
    };
    validate(&output)
        .context("migrated configuration is invalid; original file was not changed")?;
    let parent = path.parent().context("config has no parent directory")?;
    let permissions = fs::metadata(&path)?.permissions();

    let mut replacement = tempfile::NamedTempFile::new_in(parent)?;
    replacement.as_file().set_permissions(permissions.clone())?;
    replacement.write_all(output.as_bytes())?;
    replacement.as_file().sync_all()?;

    let mut backup = tempfile::NamedTempFile::new_in(parent)?;
    backup.as_file().set_permissions(permissions)?;
    backup.write_all(original.as_bytes())?;
    backup.as_file().sync_all()?;
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".bak");
    let backup_base = PathBuf::from(backup_name);
    let mut backup_path = backup_base.clone();
    let mut number = 0_u64;
    loop {
        match backup.persist_noclobber(&backup_path) {
            Ok(_) => break,
            Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => {
                backup = err.file;
                number += 1;
                let mut name = backup_base.as_os_str().to_os_string();
                name.push(format!(".{number}"));
                backup_path = name.into();
            }
            Err(err) => return Err(err.error.into()),
        }
    }
    ensure!(
        fs::read_to_string(&path)? == original,
        "config changed during migration; retry the command"
    );
    replacement.persist(&path)?;
    Ok(Some(FileMigration {
        target: path,
        backup: backup_path,
        original,
        updated: output,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_comments_keybindings_and_custom_commands() {
        let input = r#"# My settings
theme = 'onedark'

[editor]
# Show open buffers
bufferline = 'always' # Keep this comment
mouse = false
auto-save = true
true-color = true
auto-pairs = { '(' = ')', '.' = '.' }
gutters = ['diff', 'diagnostics']

# LSP settings
[editor.lsp] # Servers
display-messages = false # Quiet

# Bindings
[keys.normal]
'C-s' = ':write'

[commands]
':save' = ':write!'
"#;
        let output = source(input).unwrap().unwrap();
        for comment in [
            "# My settings",
            "# Show open buffers",
            "# Keep this comment",
            "# LSP settings",
            "# Servers",
            "# Quiet",
            "# Bindings",
        ] {
            assert!(output.contains(comment), "{comment}\n{output}");
        }
        assert!(output.contains("[keys.normal]\n'C-s' = ':write'"));
        assert!(output.contains("[commands]\n':save' = ':write!'"));
        let value: toml::Value = toml::from_str(&output).unwrap();
        assert_eq!(
            value["editor"]["bufferline"]["show"].as_str(),
            Some("always")
        );
        assert_eq!(
            value["editor"]["auto-pairs"]["pairs"]["."].as_str(),
            Some(".")
        );
        assert!(source(&output).unwrap().is_none());
    }

    #[test]
    fn handles_inline_dotted_and_mixed_containers() {
        for input in [
            "[editor.terminal]\nargs = ['split-window']",
            "editor = { mouse = false, auto-save = true, lsp = { display-messages = false } }",
            "editor.lsp.display-messages = false\neditor.auto-reload.poll.interval = 3000",
            "[editor.word-completion]\nenable = false\n[editor.completion.word]\ntrigger-length = 3",
            "[editor]\nword-completion = { enable = false } # Legacy\ncompletion.word = { trigger-length = 3 }",
            "[editor]\nauto-pairs = false\nrainbow-brackets = true\n[editor.terminal]\ncommand = 'tmux'\nargs = ['split-window']",
            "[editor.auto-pairs]\n'(' = ')'\n'.' = '.'",
            "[editor.clipboard-provider.custom]\nyank = { command = 'pbcopy', args = [] }\npaste = { command = 'pbpaste' }",
            "[editor]\nword-completion = {}\n[editor.completion.word]\nenable = false",
        ] {
            let output = source(input).unwrap_or_else(|err| panic!("{input}\n{err:?}")).unwrap();
            assert!(source(&output).unwrap().is_none(), "{output}");
        }
        assert!(source("[editor.lsp]\ndisplay-messages = true\nmessages.enable = false").is_err());
        assert!(source("this is not TOML").is_err());
    }

    #[test]
    fn preserves_comments_when_merging_containers() {
        for input in [
            "[editor]\n# Legacy setting\nword-completion = { enable = false } # Legacy table\ncompletion.word = { trigger-length = 3 } # Canonical table\n",
            "# Legacy setting\n[editor.word-completion] # Legacy table\nenable = false\n# Canonical table\n[editor.completion.word]\ntrigger-length = 3\n",
        ] {
            let output = source(input).unwrap().unwrap();
            for comment in ["# Legacy setting", "# Legacy table", "# Canonical table"] {
                assert!(output.contains(comment), "{output}");
            }
        }
    }

    #[test]
    fn preserves_comments_when_moving_a_table_into_an_inline_parent() {
        let input = "[editor]\ncompletion = { path = { enable = false } }\n# Legacy header\n[editor.word-completion] # Header suffix\n# Body\nenable = false # Body suffix\n";
        let output = source(input).unwrap().unwrap();
        for comment in [
            "# Legacy header",
            "# Header suffix",
            "# Body",
            "# Body suffix",
        ] {
            assert!(output.contains(comment), "{output}");
        }
        assert!(source(&output).unwrap().is_none());
    }

    #[test]
    fn backs_up_without_overwriting_and_skips_canonical_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[editor]\nscrolloff = 5\n";
        fs::write(&path, original).unwrap();
        let first = file(&path, |_| Ok(())).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&first.backup).unwrap(), original);
        assert!(file(&path, |_| Ok(())).unwrap().is_none());
        fs::write(&path, "[editor]\nscrolloff = 7\n").unwrap();
        let second = file(&path, |_| Ok(())).unwrap().unwrap();
        assert_ne!(first.backup, second.backup);
        assert_eq!(fs::read_to_string(first.backup).unwrap(), original);
    }

    #[test]
    fn validation_failure_leaves_original_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[editor]\nscrolloff = 'bad'\n";
        fs::write(&path, original).unwrap();
        assert!(file(&path, |_| anyhow::bail!("bad config")).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_symlinks_and_file_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfile.toml");
        let link = dir.path().join("config.toml");
        fs::write(&target, "[editor]\nscrolloff = 7\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &link).unwrap();
        let backup = file(&link, |_| Ok(())).unwrap().unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(backup.backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

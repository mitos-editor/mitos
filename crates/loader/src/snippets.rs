//! User snippet configuration discovery and reading.
use anyhow::{Context, Result};
mod catalog;

pub use catalog::{Catalog, UserSnippet};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Explicit user configuration source; workspace files are relative to each document.
#[derive(Debug, Clone)]
pub struct Resources {
    pub user_file: PathBuf,
}
impl Default for Resources {
    fn default() -> Self {
        Self {
            user_file: user_file(),
        }
    }
}

pub fn workspace_file(root: &Path) -> PathBuf {
    root.join(".mitos").join("snippets.toml")
}

pub fn user_file() -> PathBuf {
    crate::config_dir().join("snippets.toml")
}

/// The sibling collection directory, for example `snippets/` beside `snippets.toml`.
pub fn directory(file: &Path) -> PathBuf {
    file.with_extension("")
}

/// Select a configuration file for editing, keeping named files inside the collection.
pub fn edit_file(base: &Path, relative: Option<&Path>) -> Result<PathBuf> {
    let Some(relative) = relative else {
        return Ok(base.to_path_buf());
    };
    anyhow::ensure!(
        relative.components().all(|component| matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )) && relative
            .extension()
            .is_some_and(|extension| extension == "toml"),
        "snippet file must be a relative .toml path inside the snippets directory"
    );
    Ok(directory(base).join(relative))
}

/// Read the optional primary file and every `.toml` file under its sibling directory.
/// The entire collection is validated before it is returned to the editor.
pub fn load(path: &Path) -> Result<Catalog> {
    let mut files = Vec::new();
    collect_files(&directory(path), &mut files)?;
    files.sort();
    let mut catalogs = vec![load_file(path, true)?];
    for file in files {
        catalogs.push(load_file(&file, false)?);
    }
    Catalog::from_sources(catalogs)
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot inspect snippets directory {}", directory.display())
            })
        }
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot read snippets directory {}", directory.display()))
        }
    };
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("cannot list snippets directory {}", directory.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot inspect snippets {}", path.display()))?;
        if kind.is_dir() {
            collect_files(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            // Read file symlinks (common in dotfile collections), but never follow
            // directory symlinks, which can escape the collection or form cycles.
            if kind.is_file()
                || (kind.is_symlink()
                    && std::fs::metadata(&path)
                        .with_context(|| format!("cannot read snippet link {}", path.display()))?
                        .is_file())
            {
                files.push(path);
            }
        }
    }
    Ok(())
}

fn load_file(path: &Path, optional: bool) -> Result<Catalog> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error)
            if optional
                && error.kind() == ErrorKind::NotFound
                && std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == ErrorKind::NotFound) =>
        {
            return Ok(Catalog::default())
        }
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read snippets {}", path.display()))
        }
    };
    Catalog::parse_with_source(&source, Some(&path.to_string_lossy()))
        .with_context(|| format!("invalid snippets {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_is_empty_but_invalid_and_unreadable_files_report_their_path() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("snippets.toml");
        assert_eq!(load(&file)?.matching("text").count(), 0);
        std::fs::write(&file, "invalid = true")?;
        assert!(format!("{:#}", load(&file).unwrap_err()).contains(&file.display().to_string()));
        assert!(load(directory.path()).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    fn write(file: &Path, name: &str) -> Result<()> {
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(
            file,
            format!("[[snippet]]\nname={name:?}\nprefix='same'\nbody=['first', '$0']"),
        )?;
        Ok(())
    }

    #[test]
    fn loads_primary_and_nested_files_with_sources_and_stable_name_order() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let base = temporary.path().join("snippets.toml");
        write(&directory(&base).join("z.toml"), "Z")?;
        write(&directory(&base).join("team").join("a.toml"), "A")?;
        std::fs::write(directory(&base).join("notes.txt"), "not TOML")?;
        let catalog = load(&base)?;
        assert_eq!(
            catalog
                .matching("text")
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "Z"]
        );
        let first = catalog.matching("text").next().unwrap();
        assert_eq!(first.body, "first\n$0");
        assert_eq!(
            first.source.as_deref(),
            Some(
                directory(&base)
                    .join("team")
                    .join("a.toml")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        write(&base, "Main")?;
        assert_eq!(load(&base)?.matching("text").count(), 3);
        Ok(())
    }

    #[test]
    fn duplicate_names_report_both_files_and_invalid_members_reject_the_collection() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let base = temporary.path().join("snippets.toml");
        let extra = directory(&base).join("other.toml");
        write(&base, "same")?;
        write(&extra, "same")?;
        let error = format!("{:#}", load(&base).unwrap_err());
        assert!(error.contains("duplicate snippet name"));
        assert!(error.contains(base.to_string_lossy().as_ref()));
        assert!(error.contains(extra.to_string_lossy().as_ref()));
        std::fs::write(&extra, "invalid = true")?;
        let error = format!("{:#}", load(&base).unwrap_err());
        assert!(error.contains(extra.to_string_lossy().as_ref()));
        Ok(())
    }

    #[test]
    fn editing_named_files_stays_inside_the_collection() {
        let base = Path::new("config/snippets.toml");
        assert_eq!(edit_file(base, None).unwrap(), base);
        assert_eq!(
            edit_file(base, Some(Path::new("team/rust.toml"))).unwrap(),
            Path::new("config/snippets/team/rust.toml")
        );
        for name in ["../other.toml", "/other.toml", "rust.json", ""] {
            assert!(edit_file(base, Some(Path::new(name))).is_err(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_symlinks_are_loaded_without_following_directory_cycles_or_broken_links() -> Result<()> {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir()?;
        let base = temporary.path().join("snippets.toml");
        let collection = directory(&base);
        std::fs::create_dir_all(&collection)?;
        let external = temporary.path().join("external.toml");
        write(&external, "linked")?;
        symlink(&external, collection.join("linked.toml"))?;
        symlink(&collection, collection.join("cycle"))?;
        assert_eq!(load(&base)?.matching("text").count(), 1);
        std::fs::remove_file(&external)?;
        assert!(load(&base).is_err());
        std::fs::remove_file(collection.join("linked.toml"))?;
        symlink(&external, &base)?;
        assert!(load(&base).is_err());
        Ok(())
    }
}

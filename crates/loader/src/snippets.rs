//! User snippet configuration discovery and reading.
use anyhow::{Context, Result};
use snippets::Catalog;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub fn user_file() -> PathBuf {
    crate::config_dir().join("snippets.toml")
}

pub fn load(path: &Path) -> Result<Catalog> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Catalog::default()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read snippets {}", path.display()))
        }
    };
    Catalog::parse(&source).with_context(|| format!("invalid snippets {}", path.display()))
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

//! Persistent spelling ignores, separate from the dictionary's suggestion vocabulary.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

/// A loaded user ignore list that persists new entries before exposing them to scans.
#[derive(Debug)]
pub struct IgnoredWordsFile {
    path: PathBuf,
    words: HashSet<String>,
}

impl IgnoredWordsFile {
    /// Read a UTF-8 word list. Missing files are empty lists; other errors are reported.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        let words = text
            .lines()
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect();
        Ok(Self { path, words })
    }

    /// Words in this loaded snapshot, normalized to lowercase.
    pub fn words(&self) -> impl Iterator<Item = &String> {
        self.words.iter()
    }

    /// Persist a word before publishing it to checks. Appending preserves entries written by
    /// other editor instances since this list was loaded.
    pub fn insert(&mut self, word: &str) -> io::Result<bool> {
        let word = word.trim().to_lowercase();
        if word.is_empty() || word.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected one spelling word",
            ));
        }
        if self.words.contains(&word) {
            return Ok(false);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;
        let mut entry = String::new();
        // Hand-edited lists need not end with a newline. Never join the new word onto the old one.
        if file.metadata()?.len() != 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                entry.push('\n');
            }
        }
        entry.push_str(&word);
        entry.push('\n');
        file.write_all(entry.as_bytes())?;
        self.words.insert(word);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_ignores_round_trip_and_preserve_external_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spelling/en_US.ignore");
        let mut ignores = IgnoredWordsFile::load(path.clone()).unwrap();
        assert!(ignores.words.is_empty());
        assert!(!path.exists());
        assert!(ignores.insert("Zorblé").unwrap());
        assert!(!ignores.insert("ZORBLÉ").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "zorblé\n");

        // Another editor or a manual edit may add words after this instance loads its snapshot.
        fs::write(&path, "zorblé\n  ExternalWord  ").unwrap();
        assert!(ignores.insert("another").unwrap());
        let reloaded = IgnoredWordsFile::load(path).unwrap();
        assert_eq!(
            reloaded.words,
            HashSet::from(["zorblé".into(), "externalword".into(), "another".into()])
        );
    }

    #[test]
    fn persistent_ignore_failures_do_not_change_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("en_US.ignore");
        let mut ignores = IgnoredWordsFile::load(path.clone()).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(ignores.insert("zorble").is_err());
        assert!(ignores.words.is_empty());
        assert!(IgnoredWordsFile::load(path).is_err());
    }
}

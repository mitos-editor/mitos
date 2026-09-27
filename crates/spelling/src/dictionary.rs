//! Dictionary loading, suggestions, and personal vocabulary snapshots.

use std::{collections::HashSet, path::Path, sync::Arc};

use crate::Dictionary;

/// Load a Hunspell dictionary and its personal vocabulary from explicit paths.
/// Resource discovery and precedence are chosen by the caller.
pub fn load_dictionary(aff: &Path, dic: &Path, personal: &Path) -> anyhow::Result<Dictionary> {
    let aff = std::fs::read_to_string(aff)?;
    let dic = std::fs::read_to_string(dic)?;
    let mut dictionary = Dictionary::new(&aff, &dic)
        .map_err(|err| anyhow::anyhow!("could not parse dictionary: {err:?}"))?;
    load_personal_dictionary(&mut dictionary, personal)?;
    Ok(dictionary)
}

/// Suggestions in dictionary order, preserving the first occurrence of duplicates.
pub fn suggestions<'a>(
    word: &str,
    dictionaries: impl IntoIterator<Item = &'a Dictionary>,
) -> Vec<String> {
    let mut suggestions = Vec::new();
    for dictionary in dictionaries {
        let mut candidates = Vec::new();
        dictionary.suggest(word, &mut candidates);
        suggestions.extend(candidates);
    }
    let mut seen = HashSet::new();
    suggestions.retain(|suggestion| seen.insert(suggestion.clone()));
    suggestions
}

/// Save an accepted word and publish a new dictionary snapshot. The personal dictionary file is
/// created if needed and read back when the dictionary is loaded in a later session.
pub fn add_personal_word(
    dictionary: &mut Arc<Dictionary>,
    path: &Path,
    word: &str,
) -> anyhow::Result<()> {
    // Workers keep immutable snapshots, so accepting a word never waits for a running scan.
    // Publish only after both validation and persistence succeed.
    let mut updated = (**dictionary).clone();
    updated
        .add(word)
        .map_err(|err| anyhow::anyhow!("could not add '{word}': {err:?}"))?;
    append_personal_word(path, word)?;
    *dictionary = Arc::new(updated);
    Ok(())
}

fn append_personal_word(path: &Path, word: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{word}")
}

/// Load accepted words from a personal dictionary, allowing the file to be absent.
pub fn load_personal_dictionary(dictionary: &mut Dictionary, path: &Path) -> std::io::Result<()> {
    use std::io::{BufRead as _, BufReader, ErrorKind};
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for line in BufReader::new(file).lines() {
        let word = line?;
        let word = word.trim();
        if !word.is_empty()
            && let Err(err) = dictionary.add(word)
        {
            log::warn!("ignoring personal dictionary entry {word:?}: {err:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_words_persist_and_remain_separate_by_language() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictionaries/en_US.txt");
        append_personal_word(&path, "Mitos").unwrap();
        append_personal_word(&path, "spellbook").unwrap();
        let mut dictionary = Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        assert!(!dictionary.check("Mitos"));
        load_personal_dictionary(&mut dictionary, &dir.path().join("de_DE.txt")).unwrap();
        assert!(!dictionary.check("Mitos"));
        load_personal_dictionary(&mut dictionary, &path).unwrap();
        assert!(dictionary.check("Mitos"));
        assert!(dictionary.check("spellbook"));
        assert!(dictionary.check("hello"));
    }

    #[test]
    fn accepting_a_word_preserves_snapshots_and_requires_a_successful_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("en_US.txt");
        let mut dictionary = Arc::new(Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap());
        let snapshot = dictionary.clone();
        // A directory cannot be opened as an append-only file, even when running as root.
        assert!(add_personal_word(&mut dictionary, dir.path(), "Mitos").is_err());
        assert!(Arc::ptr_eq(&snapshot, &dictionary));
        assert!(!dictionary.check("Mitos"));

        add_personal_word(&mut dictionary, &path, "Mitos").unwrap();
        assert!(dictionary.check("Mitos"));
        assert!(!snapshot.check("Mitos"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Mitos\n");
    }
}

//! Feature APIs work with explicit inputs, without an editor or async runtime.

use editor_core::{syntax::config::SpellingConfig, Rope};
use spelling::{check_text, expand_check_window, load_dictionary, suggestions, Dictionary};

#[test]
fn dictionary_loading_uses_supplied_affixes_vocabulary_and_personal_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let aff = dir.path().join("selected.aff");
    let dic = dir.path().join("selected.dic");
    let personal = dir.path().join("personal.txt");
    std::fs::write(&aff, "SET UTF-8\nSFX S Y 1\nSFX S 0 s .\n")?;
    std::fs::write(&dic, "1\ncat/S\n")?;

    let original = load_dictionary(&aff, &dic, &personal)?;
    assert!(original.check("cats"));
    assert!(!original.check("zorblé"));
    assert!(!personal.exists());

    std::fs::write(&personal, "zorblé\n")?;
    let loaded = load_dictionary(&aff, &dic, &personal)?;
    assert!(loaded.check("cats"));
    assert!(loaded.check("zorblé"));
    assert!(!original.check("zorblé"));

    assert!(load_dictionary(&dir.path().join("missing.aff"), &dic, &personal).is_err());
    assert!(load_dictionary(&aff, &dir.path().join("missing.dic"), &personal).is_err());
    std::fs::write(&aff, "SET UTF-8\nFLAG invalid\n")?;
    assert!(load_dictionary(&aff, &dic, &personal)
        .unwrap_err()
        .to_string()
        .contains("could not parse dictionary"));
    Ok(())
}

#[test]
fn suggestions_preserve_dictionary_order_without_duplicates() {
    let first = Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
    let second = Dictionary::new("SET UTF-8\n", "1\nhelp\n").unwrap();
    assert_eq!(
        suggestions("helo", [&first, &second, &first]),
        ["hello", "help"]
    );
    assert_eq!(
        suggestions("helo", [&second, &first, &second]),
        ["help", "hello"]
    );
    assert!(suggestions("helo", []).is_empty());
}

#[test]
fn full_scan_respects_settings_unicode_and_cancellation() {
    let loader = editor_core::config::default_lang_loader(loader::syntax::Resources::new(vec![]));
    let dictionary = Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
    let text = Rope::from_str("🚀 quik projectword https://exampel.org/typo hello");
    let config = SpellingConfig {
        words: vec!["projectword".into()],
        ..Default::default()
    };
    let diagnostics = check_text(
        &[&dictionary],
        text.slice(..),
        None,
        &loader,
        &config,
        || false,
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        (diagnostics[0].range.start, diagnostics[0].range.end),
        (2, 6)
    );
    assert!(check_text(
        &[&dictionary],
        text.slice(..),
        None,
        &loader,
        &config,
        || true
    )
    .is_empty());
}

#[test]
fn incremental_windows_respect_the_callers_budget_and_include_whole_tokens() {
    let text = Rope::from_str("hello quik");
    assert_eq!(expand_check_window(text.slice(..), 8..9, 4), Some(6..10));
    assert_eq!(expand_check_window(text.slice(..), 8..9, 3), None);
    assert_eq!(expand_check_window(text.slice(..), 0..10, 4), None);
}

#[test]
fn compiled_filters_honor_merged_language_settings() {
    let global = SpellingConfig {
        words: vec!["Mitos".into()],
        ignore_regexes: vec!["^[A-Z_]+$".into()],
        min_word_length: Some(2),
        ..Default::default()
    };
    let local = SpellingConfig {
        words: vec!["Tokio".into()],
        ignore_regexes: vec!["^v[0-9]+$".into()],
        min_word_length: Some(3),
        ..Default::default()
    };
    let filter = spelling::SpellingFilter::new(&global.merged(Some(&local)));
    for word in ["mitos", "TOKIO", "CONSTANT_NAME", "v123", "ab"] {
        assert!(filter.ignores(word), "{word}");
    }
    assert!(!filter.ignores("teh"));
}

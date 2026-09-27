//! Spell checking as a non-LSP diagnostic source.
//!
//! Adapted from Michael Davis's [Helix PR #15910](https://github.com/helix-editor/helix/pull/15910),
//! revision `1849ccc29792484a1e00bcc48ee8018ca058cf33`.
//!
//! Owns spelling state, dictionary loading, debounced checks, and result publication.
//! Scanning and background coordination live in this feature's private modules;
//! frontends supply an editor-bound completion sender and present actions and diagnostics.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    future::Future,
    path::Path,
    sync::Arc,
};

use editor_core::{
    diagnostic::DiagnosticProvider, syntax::config::SpellingConfig, ChangeSet, SpellingLanguage,
    Tendril, Transaction,
};
use event::{register_hook, send_blocking, AsyncHook, TaskController, TaskHandle};
use tokio::sync::mpsc::Sender;

use crate::{
    action::Action,
    callbacks::EditorCallbackSender,
    events::{ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen},
    Dictionary, Document, DocumentId, Editor,
};

mod ignore;
mod scan;
mod worker;
pub use ignore::IgnoredWordsFile;

#[derive(Debug)]
pub struct SpellingHandler {
    pub(crate) event_tx: Sender<SpellingEvent>,
    callbacks: EditorCallbackSender,
    /// In-flight full-document checks, keyed by document. Starting a new full check for a document
    /// cancels the previous one (incremental checks run synchronously and need no cancellation).
    requests: HashMap<DocumentId, TaskController>,
    /// Languages whose dictionary is currently being loaded, so the same one isn't loaded twice
    /// concurrently.
    loading_dictionaries: HashSet<SpellingLanguage>,
    /// Lowercased words ignored for this editor session, scoped to each spelling language.
    /// Ignoring a word does not add it to dictionary suggestions or persistence.
    ignored_words: HashMap<SpellingLanguage, HashSet<String>>,
    /// Loaded user ignore files, one per dictionary language.
    pub ignored_word_files: HashMap<SpellingLanguage, IgnoredWordsFile>,
}

impl SpellingHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let event_tx = worker::SpellingHook::new(callbacks.clone()).spawn();
        Self {
            event_tx,
            callbacks,
            requests: HashMap::new(),
            loading_dictionaries: HashSet::new(),
            ignored_words: HashMap::new(),
            ignored_word_files: HashMap::new(),
        }
    }

    /// Registers a new in-flight full check for `document`, cancelling any previous one, and
    /// returns a handle the background task uses to observe cancellation.
    pub fn open_request(&mut self, document: DocumentId) -> TaskHandle {
        let mut controller = TaskController::new();
        let handle = controller.restart();
        self.requests.insert(document, controller);
        handle
    }
}

#[derive(Debug)]
pub enum SpellingEvent {
    /// A dictionary finished loading; (re-)check the open documents that use it.
    DictionaryLoaded { language: SpellingLanguage },
    /// A document was opened or its spelling settings changed; check it in full.
    CheckRequested { doc: DocumentId },
    /// A document was closed; discard its pending edits.
    DocumentClosed { doc: DocumentId },
    /// A document changed; re-check the regions around the change or rescan in full.
    /// The version identifies the text these ranges apply to.
    DocumentChanged {
        doc: DocumentId,
        changes: ChangeSet,
        version: i32,
    },
}

/// Spelling actions sort after LSP code actions (which use a higher priority).
const SPELLING_ACTION_PRIORITY: u8 = 0;

/// Save an accepted word and publish a new dictionary snapshot. The personal dictionary file is
/// created if needed and read back when the dictionary is loaded in a later session.
fn add_personal_word(
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

impl Editor {
    /// Resolve spelling settings for a document, including session and persistent ignores.
    /// Both full and incremental scans use this snapshot without changing persisted settings.
    pub fn spelling_config(&self, doc: &Document) -> SpellingConfig {
        let mut config = self.config().spelling.merged(
            doc.language_config()
                .and_then(|config| config.spelling.as_ref()),
        );
        for language in &doc.spelling_languages {
            if let Some(words) = self.handlers.spelling.ignored_words.get(language) {
                config.words.extend(words.iter().cloned());
            }
            if let Some(file) = self.handlers.spelling.ignored_word_files.get(language) {
                config.words.extend(file.words().cloned());
            }
        }
        config
    }

    fn ignore_spelling_word(&mut self, language: &SpellingLanguage, word: &str) {
        if !self
            .handlers
            .spelling
            .ignored_words
            .entry(language.clone())
            .or_default()
            .insert(word.to_lowercase())
        {
            return;
        }
        self.recheck_spelling_language(language);
    }

    fn ignore_spelling_word_forever(&mut self, language: &SpellingLanguage, word: &str) {
        let Some(file) = self.handlers.spelling.ignored_word_files.get_mut(language) else {
            self.set_error(|| format!("Spelling ignore list '{language}' is not loaded"));
            return;
        };
        match file.insert(word) {
            Ok(true) => self.recheck_spelling_language(language),
            Ok(false) => {}
            Err(err) => {
                self.set_error(|| format!("Could not save spelling ignore for '{language}': {err}"))
            }
        }
    }

    fn recheck_spelling_language(&mut self, language: &SpellingLanguage) {
        let documents: Vec<_> = self
            .documents()
            .filter(|doc| doc.spelling_languages.contains(language))
            .map(Document::id)
            .collect();
        for doc in documents {
            // Invalidate snapshots before queuing new checks so an older result cannot restore
            // the ignored findings. CheckRequested bypasses the edit debounce.
            self.handlers.spelling.requests.remove(&doc);
            send_blocking(
                &self.handlers.spelling.event_tx,
                SpellingEvent::CheckRequested { doc },
            );
        }
    }

    /// Invalidate old checks and diagnostics after changing a document's spelling settings.
    pub fn refresh_spelling(&mut self, doc_id: DocumentId) {
        self.handlers.spelling.requests.remove(&doc_id);
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        doc.detect_spelling_languages();
        doc.replace_diagnostics([], &[], Some(&DiagnosticProvider::Spelling));
        if !doc.spelling_languages.is_empty() {
            send_blocking(
                &self.handlers.spelling.event_tx,
                SpellingEvent::CheckRequested { doc: doc_id },
            );
        }
        event::dispatch(crate::events::DiagnosticsDidChange {
            editor: self,
            doc: doc_id,
        });
    }

    /// Capture the spelling findings overlapping the primary selection, then generate corrections,
    /// ignore actions, and personal-dictionary actions on a blocking worker. The future owns its
    /// snapshot so the editor can keep processing input while suggestions are computed.
    pub fn spelling_actions(&self) -> impl Future<Output = anyhow::Result<Vec<Action>>> + use<> {
        let (view, doc) = current_ref!(self);
        // The dictionaries this document is checked against, in configuration order.
        let dictionaries: Vec<(SpellingLanguage, _)> = doc
            .spelling_languages
            .iter()
            .filter_map(|language| {
                Some((language.clone(), self.dictionaries.get(language)?.clone()))
            })
            .collect();
        let doc_id = doc.id();
        let view_id = view.id;
        let version = doc.version();
        let selection = doc.selection(view_id).primary();
        let text = doc.text();
        let words: Vec<_> = doc
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.provider == DiagnosticProvider::Spelling
                    && selection.overlaps(&editor_core::Range::new(
                        diagnostic.range.start,
                        diagnostic.range.end,
                    ))
            })
            .map(|diagnostic| {
                let range = diagnostic.range;
                let word = Cow::<str>::from(text.slice(range.start..range.end)).into_owned();
                (range, word)
            })
            .collect();

        async move {
            if dictionaries.is_empty() || words.is_empty() {
                return Ok(Vec::new());
            }
            Ok(tokio::task::spawn_blocking(move || {
                let mut suggestions = Vec::new();
                let mut actions = Vec::new();
                for (range, word) in words {
                    // Offer the suggestions from every dictionary, in order, without duplicates.
                    suggestions.clear();
                    for (_, dictionary) in &dictionaries {
                        let mut candidates = Vec::new();
                        dictionary.suggest(&word, &mut candidates);
                        suggestions.extend(candidates);
                    }
                    let mut seen = HashSet::new();
                    suggestions.retain(|suggestion| seen.insert(suggestion.clone()));
                    for suggestion in &suggestions {
                        let suggestion = suggestion.clone();
                        let title = format!("Replace '{word}' with '{suggestion}'");
                        actions.push(Action::new(
                            title,
                            SPELLING_ACTION_PRIORITY,
                            move |editor| {
                                let Some(doc) = editor.documents.get_mut(&doc_id) else {
                                    return;
                                };
                                let Some(view) = editor.tree.try_get(view_id) else {
                                    return;
                                };
                                // A file reload or edit may have invalidated the menu's captured range.
                                if doc.version() != version || view.doc != doc_id {
                                    return;
                                }
                                let view = editor.tree.get_mut(view_id);
                                let transaction = Transaction::change(
                                    doc.text(),
                                    std::iter::once((
                                        range.start,
                                        range.end,
                                        Some(Tendril::from(&*suggestion)),
                                    )),
                                );
                                doc.apply(&transaction, view_id);
                                doc.append_changes_to_history(view);
                            },
                        ));
                    }

                    // Ignores and personal words target one spelling language at a time.
                    for (language, _) in &dictionaries {
                        let ignored_language = language.clone();
                        let ignored_word = word.clone();
                        actions.push(Action::new(
                            format!("Ignore '{word}' for this session ({language})"),
                            SPELLING_ACTION_PRIORITY,
                            move |editor| {
                                editor.ignore_spelling_word(&ignored_language, &ignored_word)
                            },
                        ));
                        let ignored_language = language.clone();
                        let ignored_word = word.clone();
                        actions.push(Action::new(
                            format!("Ignore '{word}' forever ({language})"),
                            SPELLING_ACTION_PRIORITY,
                            move |editor| {
                                editor
                                    .ignore_spelling_word_forever(&ignored_language, &ignored_word)
                            },
                        ));
                        let language = language.clone();
                        let word = word.clone();
                        let title = format!("Add '{word}' to dictionary '{language}'");
                        actions.push(Action::new(
                            title,
                            SPELLING_ACTION_PRIORITY,
                            move |editor| {
                                let Some(dictionary) = editor.dictionaries.get_mut(&language)
                                else {
                                    return;
                                };
                                let path = loader::personal_dictionary_file(language.as_str());
                                if let Err(err) = add_personal_word(dictionary, &path, &word) {
                                    log::error!(
                                "could not persist '{word}' to the personal dictionary: {err}"
                            );
                                    editor.set_error(|| {
                                format!("Could not save personal dictionary '{language}': {err}")
                            });
                                    return;
                                }
                                // The dictionary's contents changed; re-check the open documents using it.
                                send_blocking(
                                    &editor.handlers.spelling.event_tx,
                                    SpellingEvent::DictionaryLoaded {
                                        language: language.clone(),
                                    },
                                );
                            },
                        ));
                    }
                }

                actions
            })
            .await?)
        }
    }
}

/// Register spelling hooks once; each event supplies its owning editor or document queue.
pub fn register_hooks() {
    event::runtime_local! {
        static REGISTER: std::sync::Once = std::sync::Once::new();
    }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidOpen<'_>| {
            let doc = doc!(event.editor, &event.doc);
            if !doc.spelling_languages.is_empty() {
                send_blocking(
                    &event.editor.handlers.spelling.event_tx,
                    SpellingEvent::CheckRequested { doc: event.doc },
                );
            }
            Ok(())
        });

        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            // Mirror the word index: ignore synthetic edits so they don't churn the diagnostics.
            if !event.ghost_transaction
                && !event.doc.spelling_languages.is_empty()
                && let Some(tx) = &event.doc.spelling_events
            {
                send_blocking(
                    tx,
                    SpellingEvent::DocumentChanged {
                        doc: event.doc.id(),
                        changes: event.changes.clone(),
                        version: event.doc.version(),
                    },
                );
            }
            Ok(())
        });

        register_hook!(move |event: &mut DocumentDidClose<'_>| {
            // Cancel any in-flight full check for the closed document.
            event
                .editor
                .handlers
                .spelling
                .requests
                .remove(&event.doc.id());
            send_blocking(
                &event.editor.handlers.spelling.event_tx,
                SpellingEvent::DocumentClosed {
                    doc: event.doc.id(),
                },
            );
            Ok(())
        });

        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            let doc_ids: Vec<_> = event.editor.documents().map(|doc| doc.id()).collect();
            for doc_id in doc_ids {
                event.editor.refresh_spelling(doc_id);
            }
            Ok(())
        });
    });
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

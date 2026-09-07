//! Spell checking as a non-LSP diagnostic source.
//!
//! Adapted from Michael Davis's [Helix PR #15910](https://github.com/helix-editor/helix/pull/15910),
//! revision `1849ccc29792484a1e00bcc48ee8018ca058cf33`.
//!
//! Checking a word is cheap, so the work is split by where it is cheapest to run. A small plain-text edit is
//! re-checked incrementally and synchronously on the main loop: only the regions around the change
//! (`ChangeSet::changed_ranges`) are re-tokenized and spliced in with `splice_diagnostics`. A
//! full check (document open, dictionary load, syntax edits, or a large or fragmented edit) runs off the main
//! thread and replaces the spelling diagnostics wholesale, discarding its result if the document
//! changed while it ran.

use std::{collections::HashMap, future::Future, ops::Range, sync::Arc, time::Duration};

use anyhow::Context as _;
use editor_core::{
    diagnostic::{Diagnostic, DiagnosticProvider},
    syntax::{
        config::{SpellingConfig, SpellingFilter},
        Loader,
    },
    ChangeSet, Operation, Rope, SpellingLanguage, Syntax,
};
use event::{cancelable_future, register_hook, send_blocking, AsyncHook, TaskHandle};
use tokio::time::Instant;
use view::{
    events::{ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen},
    handlers::{
        spelling::{IgnoredWordsFile, SpellingEvent},
        Handlers,
    },
    Dictionary, DocumentId, Editor,
};

use crate::job;

mod scan;
use scan::{check_region, expand_check_window, spell_check_regions};

const PROVIDER: DiagnosticProvider = DiagnosticProvider::Spelling;

/// How long to wait after the last change before re-checking.
const DEBOUNCE: Duration = Duration::from_secs(1);
/// Char padding around each edit; windows are then expanded to include complete tokens.
const WINDOW_PADDING: usize = 50;
/// Maximum edited or scanned characters for work on the editor thread.
const MAX_INCREMENTAL_CHARS: usize = 1000;
/// A change with more separate edits than this (e.g. a multi-cursor edit) would blanket the
/// document in re-check windows; it is cheaper to rescan wholesale once.
const MANY_EDIT_OPS: usize = 64;

#[derive(Debug)]
struct Change {
    /// Several edits may remove diagnostics even when their net text diff is empty. In that case
    /// rescan in full instead of trying to recover affected ranges from the final text.
    changes: Option<ChangeSet>,
    version: i32,
}

#[derive(Debug, Default)]
pub(super) struct SpellingHook {
    changes: HashMap<DocumentId, Change>,
}

impl AsyncHook for SpellingHook {
    type Event = SpellingEvent;

    fn handle_event(&mut self, event: Self::Event, timeout: Option<Instant>) -> Option<Instant> {
        match event {
            SpellingEvent::DictionaryLoaded { language } => {
                job::dispatch_blocking(move |editor, _| {
                    let docs: Vec<_> = editor
                        .documents()
                        .filter(|doc| doc.spelling_languages.contains(&language))
                        .map(|doc| doc.id())
                        .collect();
                    for doc in docs {
                        check_document(editor, doc);
                    }
                });
                timeout
            }
            SpellingEvent::CheckRequested { doc } => {
                self.changes.remove(&doc);
                job::dispatch_blocking(move |editor, _| check_document(editor, doc));
                timeout
            }
            SpellingEvent::DocumentClosed { doc } => {
                self.changes.remove(&doc);
                timeout
            }
            SpellingEvent::DocumentChanged {
                doc,
                changes,
                version,
            } => {
                if let Some(pending) = self.changes.get_mut(&doc) {
                    pending.version = version;
                    pending.changes = None;
                } else {
                    self.changes.insert(
                        doc,
                        Change {
                            changes: Some(changes),
                            version,
                        },
                    );
                }
                Some(Instant::now() + DEBOUNCE)
            }
        }
    }

    fn finish_debounce(&mut self) {
        for (doc, change) in self.changes.drain() {
            job::dispatch_blocking(move |editor, _| match change.changes {
                Some(changes) => recheck_document(editor, doc, changes, change.version),
                None => check_document(editor, doc),
            });
        }
    }
}

/// Re-checks a document incrementally around `changes`, falling back to a full rescan when the
/// document has moved on since the snapshot (`version`) or the change is too large/fragmented to
/// be worth doing incrementally.
fn recheck_document(editor: &mut Editor, doc_id: DocumentId, changes: ChangeSet, version: i32) {
    let Some(doc) = editor.documents.get(&doc_id) else {
        return;
    };
    if doc.spelling_languages.is_empty() || doc.is_syntax_pending() {
        return;
    }
    // A syntax edit can change prose boundaries far beyond the edited word (for example an
    // opening comment or Markdown fence). Recheck those trees in full off the main loop.
    // Finish a pending initial/full scan before relying on incremental diagnostic coverage.
    if doc.version() != version
        || doc.syntax().is_some()
        || editor.handlers.spelling.requests.contains_key(&doc_id)
        || needs_full_scan(&changes)
    {
        check_document(editor, doc_id);
        return;
    }

    let text = doc.text().clone();
    // Not stale, so the changed ranges line up with `text`. Collect the regions to re-check,
    // merging overlapping windows so no word is checked (or emitted) twice.
    let mut regions: Vec<Range<usize>> = Vec::new();
    for (_, new_range) in changes.changed_ranges(WINDOW_PADDING) {
        let Some(new_range) = expand_check_window(text.slice(..), new_range) else {
            check_document(editor, doc_id);
            return;
        };
        match regions.last_mut() {
            Some(last) if new_range.start <= last.end => last.end = last.end.max(new_range.end),
            _ => regions.push(new_range),
        }
    }

    // Even small edits can cover a large amount of text after expanding to whole tokens.
    if regions.iter().map(|range| range.len()).sum::<usize>() > MAX_INCREMENTAL_CHARS {
        check_document(editor, doc_id);
        return;
    }
    let languages = doc.spelling_languages.clone();
    let config = editor.spelling_config(doc);
    let Some(dictionaries) = lookup_dictionaries(editor, &languages) else {
        return;
    };
    let filter = SpellingFilter::new(&config);
    let dictionaries: Vec<&Dictionary> = dictionaries.iter().map(AsRef::as_ref).collect();
    let mut diagnostics = Vec::new();
    for region in &regions {
        check_region(
            &dictionaries,
            &filter,
            text.slice(..),
            region.clone(),
            &mut diagnostics,
            || false,
        );
    }

    let doc = editor.documents.get_mut(&doc_id).unwrap();
    doc.splice_diagnostics(diagnostics, &regions, &PROVIDER);
    event::dispatch(view::events::DiagnosticsDidChange {
        editor,
        doc: doc_id,
    });
}

/// Checks an entire document off the main loop and replaces its spelling diagnostics wholesale.
fn check_document(editor: &mut Editor, doc_id: DocumentId) {
    let Some(doc) = editor.documents.get(&doc_id) else {
        return;
    };
    if doc.spelling_languages.is_empty() || doc.is_syntax_pending() {
        return;
    }
    let languages = doc.spelling_languages.clone();
    let version = doc.version();
    let text = doc.text().clone();
    // Cloning the syntax bumps a few refcounts on its (persistent) trees; cheap enough to snapshot
    // for the off-thread check.
    let syntax = doc.syntax().cloned();
    let config = editor.spelling_config(doc);
    let loader = editor.syn_loader.load_full();
    let Some(dictionaries) = lookup_dictionaries(editor, &languages) else {
        return;
    };

    let cancel = editor.handlers.spelling.open_request(doc_id);
    let future = check_text(dictionaries, text, syntax, loader, config, cancel.clone());

    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, &cancel).await else {
            return;
        };
        job::dispatch_blocking(move |editor, _| {
            // Cancellation can happen after the worker finishes but before this callback.
            if cancel.is_canceled() {
                return;
            }
            editor.handlers.spelling.requests.remove(&doc_id);
            let diagnostics = match result {
                Ok(diagnostics) => diagnostics,
                Err(err) => {
                    log::error!("spell check task panicked: {err}");
                    return;
                }
            };
            let Some(doc) = editor.documents.get_mut(&doc_id) else {
                return;
            };
            if doc.version() != version {
                // Incremental checks require complete coverage, so finish the full check first.
                check_document(editor, doc_id);
                return;
            }
            if doc.spelling_languages != languages {
                return;
            }
            doc.replace_diagnostics(diagnostics, &[], Some(&PROVIDER));
            event::dispatch(view::events::DiagnosticsDidChange {
                editor,
                doc: doc_id,
            });
        });
    });
}

/// Returns the dictionaries for `languages`, or `None` if any are not loaded yet (after kicking off
/// the missing loads). Checking waits until all are present so a not-yet-loaded dictionary can't
/// cause false positives; the load completion re-checks via [`SpellingEvent::DictionaryLoaded`].
fn lookup_dictionaries(
    editor: &mut Editor,
    languages: &[SpellingLanguage],
) -> Option<Vec<Arc<Dictionary>>> {
    let mut dictionaries = Vec::with_capacity(languages.len());
    let mut missing = false;
    for language in languages {
        // Call through for every language so all missing loads are kicked off, not just the first.
        match lookup_dictionary(editor, language.clone()) {
            Some(dictionary) => dictionaries.push(dictionary),
            None => missing = true,
        }
    }
    (!missing).then_some(dictionaries)
}

/// Returns the dictionary for `language`, kicking off an async load (once) if it isn't loaded yet.
fn lookup_dictionary(editor: &mut Editor, language: SpellingLanguage) -> Option<Arc<Dictionary>> {
    if let Some(dictionary) = editor.dictionaries.get(&language) {
        return Some(dictionary.clone());
    }
    if editor
        .handlers
        .spelling
        .loading_dictionaries
        .insert(language.clone())
    {
        load_dictionary(language);
    }
    None
}

fn load_dictionary(language: SpellingLanguage) {
    tokio::task::spawn_blocking(move || {
        let load = || -> anyhow::Result<(Dictionary, IgnoredWordsFile)> {
            let aff = std::fs::read_to_string(loader::runtime_file(format!(
                "dictionaries/{language}/{language}.aff"
            )))?;
            let dic = std::fs::read_to_string(loader::runtime_file(format!(
                "dictionaries/{language}/{language}.dic"
            )))?;
            let mut dictionary = Dictionary::new(&aff, &dic)
                .map_err(|err| anyhow::anyhow!("could not parse dictionary: {err:?}"))?;

            view::handlers::spelling::load_personal_dictionary(
                &mut dictionary,
                &loader::personal_dictionary_file(language.as_str()),
            )?;

            let path = loader::spelling_ignore_file(language.as_str());
            let ignored_words = IgnoredWordsFile::load(path.clone()).with_context(|| {
                format!("could not read spelling ignore file '{}'", path.display())
            })?;

            Ok((dictionary, ignored_words))
        };

        match load() {
            Ok((dictionary, ignored_words)) => job::dispatch_blocking(move |editor, _| {
                editor
                    .handlers
                    .spelling
                    .loading_dictionaries
                    .remove(&language);
                editor
                    .dictionaries
                    .insert(language.clone(), Arc::new(dictionary));
                editor
                    .handlers
                    .spelling
                    .ignored_word_files
                    .insert(language.clone(), ignored_words);
                send_blocking(
                    &editor.handlers.spelling.event_tx,
                    SpellingEvent::DictionaryLoaded { language },
                );
            }),
            Err(err) => {
                log::error!("could not load spelling dictionary '{language}': {err:#}");
                // Allow a later check to retry the load.
                job::dispatch_blocking(move |editor, _| {
                    editor
                        .handlers
                        .spelling
                        .loading_dictionaries
                        .remove(&language);
                    editor.set_error(|| {
                        format!("Could not load spelling dictionary '{language}': {err:#}")
                    });
                });
            }
        }
    });
}

fn check_text(
    dictionaries: Vec<Arc<Dictionary>>,
    text: Rope,
    syntax: Option<Syntax>,
    loader: Arc<Loader>,
    config: SpellingConfig,
    cancel: TaskHandle,
) -> impl Future<Output = Result<Vec<Diagnostic>, tokio::task::JoinError>> {
    tokio::task::spawn_blocking(move || {
        // Dropping a spawn_blocking JoinHandle does not stop its worker. Check cancellation here
        // and during tokenization so superseded scans also release their CPU and snapshots.
        if cancel.is_canceled() {
            return Vec::new();
        }
        let filter = SpellingFilter::new(&config);
        let dictionaries: Vec<&Dictionary> = dictionaries.iter().map(AsRef::as_ref).collect();
        let mut diagnostics = Vec::new();
        for region in spell_check_regions(
            syntax.as_ref(),
            &loader,
            text.slice(..),
            0..text.len_chars(),
        ) {
            if cancel.is_canceled() {
                break;
            }
            check_region(
                &dictionaries,
                &filter,
                text.slice(..),
                region,
                &mut diagnostics,
                || cancel.is_canceled(),
            );
        }
        diagnostics
    })
}

/// Whether a change should be re-scanned wholesale instead of incrementally: a large edit, or one
/// fragmented across many sites (e.g. multi-cursor) whose padded windows would blanket the doc.
fn needs_full_scan(changes: &ChangeSet) -> bool {
    let mut edited_chars = 0;
    let mut edit_ops = 0;
    for op in changes.changes() {
        if !matches!(op, Operation::Retain(_)) {
            edited_chars += op.len_chars();
            edit_ops += 1;
        }
    }
    edited_chars > MAX_INCREMENTAL_CHARS || edit_ops > MANY_EDIT_OPS
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.spelling.event_tx.clone();
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc = doc!(event.editor, &event.doc);
        if !doc.spelling_languages.is_empty() {
            send_blocking(&tx, SpellingEvent::CheckRequested { doc: event.doc });
        }
        Ok(())
    });

    let tx = handlers.spelling.event_tx.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // Mirror the word index: ignore synthetic edits so they don't churn the diagnostics.
        if !event.ghost_transaction && !event.doc.spelling_languages.is_empty() {
            send_blocking(
                &tx,
                SpellingEvent::DocumentChanged {
                    doc: event.doc.id(),
                    changes: event.changes.clone(),
                    version: event.doc.version(),
                },
            );
        }
        Ok(())
    });

    let tx = handlers.spelling.event_tx.clone();
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        // Cancel any in-flight full check for the closed document.
        event
            .editor
            .handlers
            .spelling
            .requests
            .remove(&event.doc.id());
        send_blocking(
            &tx,
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
}

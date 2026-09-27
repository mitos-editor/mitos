//! Spelling algorithms and dictionary storage, independent of editor lifecycle and UI.
//!
//! Callers supply text/syntax snapshots, settings, dictionaries, file paths, and
//! cancellation checks. Scans return diagnostics in character offsets; the editor
//! controls scheduling and validates snapshots before publishing their results.

mod dictionary;
mod ignore;
mod scan;

pub use dictionary::{add_personal_word, load_dictionary, load_personal_dictionary, suggestions};
pub use ignore::IgnoredWordsFile;
pub use scan::{check_region, check_text, expand_check_window, SpellingFilter};
pub use spellbook::Dictionary;

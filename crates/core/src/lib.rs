//! Backend-independent text editing primitives.
//!
//! This crate is the lowest editor-specific layer in Mitos. It owns the data
//! structures and algorithms that operate on document text, but deliberately
//! knows nothing about terminal rendering, editor views, or protocol clients.
//!
//! The central types are [`Rope`], [`Selection`], and [`Transaction`]. Most
//! positions and ranges in this crate are **character indices**, not byte
//! offsets, line/column pairs, or terminal cells. Functions whose names mention
//! bytes, graphemes, visual offsets, or coordinates are explicit crossings of
//! that boundary. Keeping those units straight is especially important when
//! adding Unicode-aware editing behavior.
//!
//! A typical edit is described as a [`Transaction`], whose [`ChangeSet`]
//! transforms both the rope and positions associated with it. Higher layers
//! should prefer transactions over mutating a rope independently so selections,
//! diagnostics, syntax trees, and history can be mapped through the same edit.
//!
//! Major subsystems include:
//!
//! - [`selection`] and [`movement`] for cursor/range semantics;
//! - [`Transaction`] and [`ChangeSet`] for composable changes and position mapping;
//! - [`indent`], [`comment`], and [`textobject`] for text editing
//!   operations. Display layout and annotation-aware positioning live in
//!   the `text-layout` crate.

use std::fmt;

pub use encoding_rs as encoding;

pub mod auto_pairs;
pub mod case_conversion;
pub mod chars;
pub mod comment;
pub mod completion;
pub mod diagnostic;
pub mod diff;
pub mod editor_config;
pub mod fuzzy;
pub mod graphemes;
pub mod highlight;
pub mod history;
pub mod increment;
pub mod indent;
pub mod line_ending;
pub mod macros;
pub mod match_brackets;
pub mod movement;
mod position;
pub mod search;
pub mod selection;
pub mod test;
pub mod textobject;
mod transaction;
pub mod transform;
pub mod uri;
pub mod wrap;

pub mod unicode {
    pub use unicode_general_category as category;
    pub use unicode_segmentation as segmentation;
    pub use unicode_width as width;
}

mod rope_reader;

pub use rope_reader::RopeReader;
pub use ropey::{self, str_utils, Rope, RopeBuilder, RopeSlice};

// pub use tendril::StrTendril as Tendril;
pub use smartstring::SmartString;

/// Compact owned text used for the small strings produced by editing operations.
///
/// Short values are stored inline, while callers can otherwise treat this like
/// owned UTF-8 text.
pub type Tendril = SmartString<smartstring::LazyCompact>;

#[doc(inline)]
pub use regex;

pub use position::{
    coords_at_pos, line_cell_coords_at_pos, pos_at_coords, pos_at_line_cell_coords, Position,
};

pub use selection::{Range, Selection};
pub use smallvec::{smallvec, SmallVec};

pub use completion::CompletionItem;
pub use diagnostic::Diagnostic;

pub use line_ending::{LineEnding, NATIVE_LINE_ENDING};
pub use transaction::{Assoc, Change, ChangeSet, Deletion, Operation, Transaction};

pub use uri::Uri;

/// A spelling dictionary identifier, such as `en_US`.
///
/// This names the dictionary files (`dictionaries/<id>/<id>.{aff,dic}`) under the runtime
/// directories; it is not otherwise interpreted, so it can be any identifier a dictionary is
/// distributed under (`en_US`, `de_DE_frami`, `ca`, ...).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpellingLanguage(SmartString<smartstring::LazyCompact>);

impl SpellingLanguage {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SpellingLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct ParseSpellingLanguageError(String);

impl std::str::FromStr for SpellingLanguage {
    type Err = ParseSpellingLanguageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The identifier is interpolated into a dictionary file path, so restrict it to a single
        // safe path component: non-empty ASCII alphanumerics plus `_` and `-`.
        if !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            Ok(Self(s.into()))
        } else {
            Err(ParseSpellingLanguageError(s.to_owned()))
        }
    }
}

impl fmt::Display for ParseSpellingLanguageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid spelling language '{}': expected a dictionary name of ASCII letters, digits, '_' or '-'",
            self.0
        )
    }
}

impl std::error::Error for ParseSpellingLanguageError {}

impl serde::Serialize for SpellingLanguage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for SpellingLanguage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = <String as serde::Deserialize>::deserialize(deserializer)?;
        string.parse().map_err(serde::de::Error::custom)
    }
}

use std::{borrow::Cow, collections::HashMap, iter};

use anyhow::Result;
use editor_core::{search::SearchSyntax, NATIVE_LINE_ENDING};

use crate::clipboard::Clipboard;
use ::clipboard::{ClipboardBackend, ClipboardError, ClipboardType};

/// Search history retains the entered text as well as its interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHistoryEntry {
    pub text: String,
    pub syntax: SearchSyntax,
}

impl SearchHistoryEntry {
    pub fn regex(text: String) -> Self {
        Self {
            text,
            syntax: SearchSyntax::Regex,
        }
    }
}

#[derive(PartialEq, Eq)]
struct RegisterValue {
    value: String,
    // Registers expose regex patterns to existing consumers, while search prompts
    // restore literal input without exposing or repeatedly escaping the pattern.
    literal: Option<String>,
}

impl From<String> for RegisterValue {
    fn from(value: String) -> Self {
        Self {
            value,
            literal: None,
        }
    }
}

impl RegisterValue {
    fn search_entry(&self) -> SearchHistoryEntry {
        match &self.literal {
            Some(text) => SearchHistoryEntry {
                text: text.clone(),
                syntax: SearchSyntax::Literal,
            },
            None => SearchHistoryEntry::regex(self.value.clone()),
        }
    }
}

/// A key-value store for saving sets of values.
///
/// Each register corresponds to a `char`. Most chars can be used to store any set of
/// values but a few chars are "special registers". Special registers have unique
/// behaviors when read or written to:
///
/// * Black hole (`_`): all values read and written are discarded
/// * Selection indices (`#`): resolved by `Editor::read_register`
/// * Selection contents (`.`): resolved by `Editor::read_register`
/// * Document path (`%`): resolved by `Editor::read_register`
/// * System clipboard (`*`)
/// * Primary clipboard (`+`)
pub struct Registers {
    /// The mapping of register to values.
    /// Values are stored in reverse order when inserted with `Registers::write`.
    /// The order is reversed again in `Registers::read`. This allows us to
    /// efficiently prepend new values in `Registers::push`.
    inner: HashMap<char, Vec<RegisterValue>>,
    clipboard: Clipboard,
    pub last_search_register: char,
}

impl Registers {
    pub fn new(clipboard: Clipboard) -> Self {
        Self {
            inner: Default::default(),
            clipboard,
            last_search_register: '/',
        }
    }

    /// Supply frontend clipboard access without replacing saved register values.
    pub fn set_clipboard_backend(&mut self, backend: Box<dyn ClipboardBackend>) {
        self.clipboard.set_backend(backend);
    }

    /// Read saved values or clipboard contents. The editor resolves contextual registers.
    pub fn read(&self, name: char) -> Option<RegisterValues<'_>> {
        match name {
            '_' => Some(RegisterValues::new(iter::empty())),
            '*' | '+' => Some(read_from_clipboard(
                &self.clipboard,
                self.inner.get(&name),
                clipboard_type(name).unwrap(),
            )),
            _ => self.inner.get(&name).map(|values| {
                RegisterValues::new(
                    values
                        .iter()
                        .map(|entry| Cow::Borrowed(entry.value.as_str()))
                        .rev(),
                )
            }),
        }
    }

    pub fn write(&mut self, name: char, mut values: Vec<String>) -> Result<()> {
        match name {
            '_' => return Ok(()),
            '#' | '.' | '%' => anyhow::bail!("Register {name} does not support writing"),
            _ => {}
        }
        if let Some(clipboard_type) = clipboard_type(name) {
            self.clipboard
                .set_contents(&values.join(NATIVE_LINE_ENDING.as_str()), clipboard_type)?;
        }
        values.reverse();
        self.inner
            .insert(name, values.into_iter().map(RegisterValue::from).collect());
        Ok(())
    }

    pub fn push(&mut self, name: char, value: String) -> Result<()> {
        self.push_value(name, value.into())
    }

    fn push_value(&mut self, name: char, value: RegisterValue) -> Result<()> {
        match name {
            '_' => Ok(()),
            '#' | '.' | '%' => Err(anyhow::anyhow!("Register {name} does not support pushing")),
            '*' | '+' => {
                let clipboard_type = clipboard_type(name).unwrap();
                let contents = self.clipboard.get_contents(clipboard_type)?;
                let saved_values = self.inner.get(&name).map(Vec::as_slice).unwrap_or_default();

                if !contents_are_saved(saved_values, &contents) {
                    anyhow::bail!("Failed to push to register {name}: clipboard does not match register contents");
                }

                let mut updated = value.value.clone();
                if !contents.is_empty() {
                    updated.push_str(NATIVE_LINE_ENDING.as_str());
                }
                updated.push_str(&contents);
                self.clipboard.set_contents(&updated, clipboard_type)?;
                self.inner.entry(name).or_default().push(value);

                Ok(())
            }
            _ => {
                self.inner.entry(name).or_default().push(value);
                Ok(())
            }
        }
    }

    /// Store a prompt history value once, retaining its most recent position.
    /// Clipboard-backed registers keep their external append semantics.
    pub fn push_unique(&mut self, name: char, value: String) -> Result<()> {
        if matches!(name, '_' | '#' | '.' | '%' | '*' | '+') {
            return self.push(name, value);
        }
        let values = self.inner.entry(name).or_default();
        values.retain(|entry| entry.value != value);
        values.push(value.into());
        Ok(())
    }

    /// Store search input while retaining a regex-compatible register value.
    pub fn push_search(&mut self, name: char, entry: SearchHistoryEntry) -> Result<()> {
        let value = RegisterValue {
            value: entry.syntax.pattern(&entry.text).into_owned(),
            literal: (entry.syntax == SearchSyntax::Literal).then_some(entry.text),
        };
        if !matches!(name, '_' | '#' | '.' | '%' | '*' | '+') {
            self.inner
                .entry(name)
                .or_default()
                .retain(|saved| saved != &value);
        }
        self.push_value(name, value)
    }

    /// Saved search input, newest first. External clipboard edits are ordinary regex text.
    pub fn search_history(
        &self,
        name: char,
    ) -> Box<dyn DoubleEndedIterator<Item = SearchHistoryEntry> + '_> {
        if !matches!(name, '*' | '+') {
            return Box::new(
                self.inner
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .rev()
                    .map(RegisterValue::search_entry),
            );
        }
        let values = self
            .read(name)
            .map(|values| values.collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(saved) = self.inner.get(&name)
            && saved.len() == values.len()
            && saved
                .iter()
                .rev()
                .zip(&values)
                .all(|(entry, value)| entry.value == **value)
        {
            return Box::new(saved.iter().rev().map(RegisterValue::search_entry));
        }
        Box::new(
            values
                .into_iter()
                .map(|value| SearchHistoryEntry::regex(value.into_owned())),
        )
    }

    pub fn iter_preview(&self) -> impl Iterator<Item = (char, &str)> {
        self.inner
            .iter()
            .filter(|(name, _)| !matches!(name, '*' | '+'))
            .map(|(name, values)| {
                let preview = values
                    .last()
                    .and_then(|entry| entry.value.lines().next())
                    .unwrap_or("<empty>");

                (*name, preview)
            })
            .chain(
                [
                    ('_', "<empty>"),
                    ('#', "<selection indices>"),
                    ('.', "<selection contents>"),
                    ('%', "<document path>"),
                    ('+', "<system clipboard>"),
                    ('*', "<primary clipboard>"),
                ]
                .iter()
                .copied(),
            )
    }

    pub fn clear(&mut self) {
        self.clear_clipboard(ClipboardType::Clipboard);
        self.clear_clipboard(ClipboardType::Selection);
        self.inner.clear()
    }

    pub fn remove(&mut self, name: char) -> bool {
        match name {
            '*' | '+' => {
                self.clear_clipboard(clipboard_type(name).unwrap());
                self.inner.remove(&name);

                true
            }
            '_' | '#' | '.' | '%' => false,
            _ => self.inner.remove(&name).is_some(),
        }
    }

    fn clear_clipboard(&mut self, clipboard_type: ClipboardType) {
        if let Err(err) = self.clipboard.set_contents("", clipboard_type) {
            log::error!(
                "Failed to clear {} clipboard: {err}",
                match clipboard_type {
                    ClipboardType::Clipboard => "system",
                    ClipboardType::Selection => "primary",
                }
            )
        }
    }

    /// Read the complete system clipboard without splitting saved multi-selection values.
    pub fn clipboard_contents(&self) -> ::clipboard::Result<String> {
        self.clipboard.get_contents(ClipboardType::Clipboard)
    }

    pub fn clipboard_provider_name(&self) -> String {
        self.clipboard.name()
    }
}

fn read_from_clipboard<'a>(
    provider: &Clipboard,
    saved_values: Option<&'a Vec<RegisterValue>>,
    clipboard_type: ClipboardType,
) -> RegisterValues<'a> {
    match provider.get_contents(clipboard_type) {
        Ok(contents) => {
            // If we're pasting the same values that we just yanked, re-use
            // the saved values. This allows pasting multiple selections
            // even when yanked to a clipboard.
            let Some(values) = saved_values else {
                return RegisterValues::new(iter::once(contents.into()));
            };

            if contents_are_saved(values, &contents) {
                RegisterValues::new(
                    values
                        .iter()
                        .map(|entry| Cow::Borrowed(entry.value.as_str()))
                        .rev(),
                )
            } else {
                RegisterValues::new(iter::once(contents.into()))
            }
        }
        Err(ClipboardError::ReadingNotSupported) => match saved_values {
            Some(values) => RegisterValues::new(
                values
                    .iter()
                    .map(|entry| Cow::Borrowed(entry.value.as_str()))
                    .rev(),
            ),
            None => RegisterValues::new(iter::empty()),
        },
        Err(err) => {
            log::error!(
                "Failed to read {} clipboard: {err}",
                match clipboard_type {
                    ClipboardType::Clipboard => "system",
                    ClipboardType::Selection => "primary",
                }
            );

            RegisterValues::new(iter::empty())
        }
    }
}

fn contents_are_saved(saved_values: &[RegisterValue], mut contents: &str) -> bool {
    let line_ending = NATIVE_LINE_ENDING.as_str();
    let mut values = saved_values.iter().rev();

    match values.next() {
        Some(first) if contents.starts_with(&first.value) => {
            contents = &contents[first.value.len()..];
        }
        None if contents.is_empty() => return true,
        _ => return false,
    }

    for value in values {
        if contents.starts_with(line_ending)
            && contents[line_ending.len()..].starts_with(&value.value)
        {
            contents = &contents[line_ending.len() + value.value.len()..];
        } else {
            return false;
        }
    }

    contents.is_empty()
}

// This is a wrapper of an iterator that is both double ended and exact size,
// and can return either owned or borrowed values. Regular registers can
// return borrowed values while some special registers need to return owned
// values.
pub struct RegisterValues<'a> {
    iter: Box<dyn DoubleEndedExactSizeIterator<Item = Cow<'a, str>> + 'a>,
}

impl<'a> RegisterValues<'a> {
    pub(crate) fn new(
        iter: impl DoubleEndedIterator<Item = Cow<'a, str>>
            + ExactSizeIterator<Item = Cow<'a, str>>
            + 'a,
    ) -> Self {
        Self {
            iter: Box::new(iter),
        }
    }
}

impl<'a> Iterator for RegisterValues<'a> {
    type Item = Cow<'a, str>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl DoubleEndedIterator for RegisterValues<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back()
    }
}

impl ExactSizeIterator for RegisterValues<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

// Each RegisterValues iterator is both double ended and exact size. We can't
// type RegisterValues as `Box<dyn DoubleEndedIterator + ExactSizeIterator>`
// because only one non-auto trait is allowed in trait objects. So we need to
// create a new trait that covers both. `RegisterValues` wraps that type so that
// trait only needs to live in this module and not be imported for all register
// callsites.
trait DoubleEndedExactSizeIterator: DoubleEndedIterator + ExactSizeIterator {}

impl<I: DoubleEndedIterator + ExactSizeIterator> DoubleEndedExactSizeIterator for I {}

/// Resolve document-relative registers without consulting the focused view.
pub(crate) fn read_contextual<'a>(
    name: char,
    document: &'a crate::Document,
    selection: Option<&'a editor_core::Selection>,
) -> Result<Option<RegisterValues<'a>>> {
    let selection =
        || selection.ok_or_else(|| anyhow::anyhow!("Register '{name}' requires a document view"));
    Ok(match name {
        '#' => Some(RegisterValues::new(
            (0..selection()?.len()).map(|i| (i + 1).to_string().into()),
        )),
        '.' => Some(RegisterValues::new(
            selection()?.fragments(document.text().slice(..)),
        )),
        '%' => Some(RegisterValues::new(iter::once(document.display_name()))),
        _ => None,
    })
}

fn clipboard_type(name: char) -> Option<ClipboardType> {
    match name {
        '+' => Some(ClipboardType::Clipboard),
        '*' => Some(ClipboardType::Selection),
        _ => None,
    }
}

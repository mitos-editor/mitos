//! Resolve contextual registers against the focused document and view.

use crate::{register::RegisterValues, Editor};
use std::{borrow::Cow, iter};

impl Editor {
    /// Read a saved, clipboard, or contextual register in the focused view.
    pub fn read_register(&self, name: char) -> Option<RegisterValues<'_>> {
        match name {
            '#' => {
                let (view, doc) = current_ref!(self);
                let selections = doc.selection(view.id).len();
                // ExactSizeIterator is implemented for Range<usize> but
                // not RangeInclusive<usize>.
                Some(RegisterValues::new(
                    (0..selections).map(|i| (i + 1).to_string().into()),
                ))
            }
            '.' => {
                let (view, doc) = current_ref!(self);
                let text = doc.text().slice(..);
                Some(RegisterValues::new(doc.selection(view.id).fragments(text)))
            }
            '%' => {
                let path = doc!(self).display_name();
                Some(RegisterValues::new(iter::once(path)))
            }
            _ => self.registers.read(name),
        }
    }

    pub fn first_register_value(&self, name: char) -> Option<Cow<'_, str>> {
        self.read_register(name)
            .and_then(|mut values| values.next())
    }

    pub fn last_register_value(&self, name: char) -> Option<Cow<'_, str>> {
        self.read_register(name)
            .and_then(|mut values| values.next_back())
    }
}

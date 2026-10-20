//! Resolve contextual registers against the focused document and view.

use crate::{register::RegisterValues, Editor};
use std::borrow::Cow;

impl Editor {
    /// Read a saved, clipboard, or contextual register in the focused view.
    pub fn read_register(&self, name: char) -> Option<RegisterValues<'_>> {
        match name {
            '#' | '.' | '%' => {
                let (view, doc) = current_ref!(self);
                crate::register::read_contextual(name, doc, Some(doc.selection(view.id)))
                    .expect("a focused document has a selection")
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

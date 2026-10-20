//! Register-backed edits coordinated by the editor that owns both resources.

use stdx::rope::RopeExt as _;

use crate::{
    document::Mode,
    editing::{
        self,
        insert::{CommentContinuation, Open},
    },
    Editor,
};
use anyhow::Result;
use editor_core::{transform::PastePosition, Transaction};
use std::borrow::Cow;

impl Editor {
    /// Copy each selection into a register, returning the number of copied ranges.
    pub fn yank_selections(&mut self, register: char) -> Result<usize> {
        let (view, doc) = current_ref!(self);
        let values: Vec<_> = doc
            .selection(view.id)
            .fragments(doc.text().slice(..))
            .map(Cow::into_owned)
            .collect();
        let count = values.len();
        self.registers.write(register, values)?;
        Ok(count)
    }

    /// Copy all selections into one value, retaining separators even around empty fragments.
    pub fn yank_joined_selections(&mut self, register: char, separator: &str) -> Result<usize> {
        let (view, doc) = current_ref!(self);
        let selection = doc.selection(view.id);
        let mut joined = String::new();
        for (index, fragment) in selection.fragments(doc.text().slice(..)).enumerate() {
            if index != 0 {
                joined.push_str(separator);
            }
            joined.push_str(&fragment);
        }
        let count = selection.len();
        self.registers.write(register, vec![joined])?;
        Ok(count)
    }

    pub fn yank_primary_selection(&mut self, register: char) -> Result<()> {
        let (view, doc) = current_ref!(self);
        let value = doc
            .selection(view.id)
            .primary()
            .fragment(doc.text().slice(..))
            .into_owned();
        self.registers.write(register, vec![value])
    }

    /// Delete selections, optionally yanking first. Rejected edits leave registers and mode alone.
    pub fn delete_selections(&mut self, register: Option<char>) -> Result<()> {
        self.remove_selected_text(register)?;
        if self.mode() == Mode::Select {
            self.set_mode(Mode::Normal);
        }
        Ok(())
    }

    /// Change selections into insertion points, preserving linewise change behavior.
    pub fn change_selections(&mut self, register: Option<char>, count: usize) -> Result<()> {
        let linewise = self.remove_selected_text(register)?;
        if linewise {
            let (view, doc) = current!(self);
            editing::insert::open_lines(
                doc,
                view.id,
                Open::Above,
                CommentContinuation::Disabled,
                count,
            )?;
        }
        self.set_mode(Mode::Insert);
        Ok(())
    }

    /// Validate text mutation before invoking an external clipboard provider or changing registers.
    fn remove_selected_text(&mut self, register: Option<char>) -> Result<bool> {
        let (view, doc) = current!(self);
        let text = doc.text();
        let selection = doc.selection(view.id);
        let linewise = selection.iter().all(|range| {
            let slice = text.slice(..);
            if range.slice(slice).len_lines(stdx::rope::LINE_TYPE) < 2 {
                return false;
            }
            let (start, end) = range.line_range(slice);
            range.from() == slice.line_to_char_idx(start)
                && range.to()
                    == slice.line_to_char_idx((end + 1).min(slice.len_lines(stdx::rope::LINE_TYPE)))
        });
        let transaction =
            Transaction::delete_by_selection(text, selection, |range| (range.from(), range.to()));
        doc.validate_changes(transaction.changes())?;
        if let Some(register) = register.filter(|&name| name != '_') {
            let values = selection
                .fragments(text.slice(..))
                .map(Cow::into_owned)
                .collect();
            self.registers.write(register, values)?;
        }
        editing::try_apply(doc, &transaction, view.id)?;
        Ok(linewise)
    }

    /// Paste saved, contextual, or clipboard values using the current editing mode.
    /// Missing registers or empty value lists return `false` without committing an undo group.
    pub fn paste_register(
        &mut self,
        register: char,
        position: PastePosition,
        count: usize,
    ) -> Result<bool> {
        let Some(values) = self.read_register(register) else {
            return Ok(false);
        };
        let values: Vec<_> = values.map(Cow::into_owned).collect();
        let mode = self.mode();
        let (view, doc) = current!(self);
        Ok(editing::paste(doc, view, &values, position, count, mode)?)
    }

    /// Replace selected contents from a register as one undo group.
    pub fn replace_from_register(&mut self, register: char, count: usize) -> Result<bool> {
        let Some(values) = self.read_register(register) else {
            return Ok(false);
        };
        let values: Vec<_> = values.map(Cow::into_owned).collect();
        let (view, doc) = current!(self);
        Ok(editing::replace_selection_contents(
            doc, view, &values, count,
        )?)
    }
}

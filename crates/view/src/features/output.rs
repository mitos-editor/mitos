//! Bounded service transcripts displayed through ordinary searchable documents.

use crate::{editor::Action, Document, DocumentId, Editor};
use editor_core::{Rope, Selection, Transaction};

pub const OUTPUT_LIMIT: usize = 1024 * 1024;

/// Retains text independently of its optional editor buffer, so closing the
/// buffer does not discard the transcript or keep a background document alive.
#[derive(Default)]
pub struct OutputBuffer {
    text: String,
    document: Option<DocumentId>,
}

impl OutputBuffer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn document(&self) -> Option<DocumentId> {
        self.document
    }

    pub fn clear(&mut self, editor: &mut Editor, name: &str) {
        self.text.clear();
        if let Some(id) = self.document
            && let Some(doc) = editor.documents.get_mut(&id)
        {
            doc.set_output_name(name.to_owned());
            let transaction = Transaction::change(
                doc.text(),
                std::iter::once((0, doc.text().len_chars(), None)),
            );
            doc.replace_readonly_content(transaction.changes(), true)
                .expect("current output snapshot");
            for (view, _) in editor.tree.views_mut() {
                view.jumps.apply(transaction.changes(), doc);
            }
        }
    }

    pub fn append(&mut self, editor: &mut Editor, text: &str) -> bool {
        let start = tail_start(text, OUTPUT_LIMIT);
        if start > 0 {
            self.text.clear();
        } else {
            let retained = OUTPUT_LIMIT - text.len();
            let remove = tail_start(&self.text, retained);
            self.text.drain(..remove);
        }
        self.text.push_str(&text[start..]);
        self.document
            .is_some_and(|id| editor.append_output(id, text))
    }

    pub fn open(&mut self, editor: &mut Editor, name: &str, action: Action) -> DocumentId {
        let existing = self.document.filter(|id| editor.document(*id).is_some());
        let id = existing.unwrap_or_else(|| {
            let id = editor.create_output_buffer(name);
            editor.append_output(id, &self.text);
            self.document = Some(id);
            id
        });
        editor.switch(id, action);
        if existing.is_none() {
            let view = editor.tree.focus();
            let scrolloff = editor.config().scroll.padding;
            let doc = editor.documents.get_mut(&id).unwrap();
            doc.set_selection(view, Selection::point(doc.text().len_chars()));
            editor
                .tree
                .get_mut(view)
                .ensure_cursor_in_view(doc, scrolloff);
        }
        id
    }
}

impl Editor {
    fn create_output_buffer(&mut self, name: &str) -> DocumentId {
        let doc = Document::from(
            Rope::new(),
            None,
            self.config.clone(),
            self.syn_loader.load_full(),
        )
        .with_output_name(name.to_owned());
        self.new_file_from_document(Action::Load, doc)
    }

    /// Append without edit history or modified state. Follow only views whose
    /// selection was already at the end; reading older output remains stable.
    fn append_output(&mut self, id: DocumentId, text: &str) -> bool {
        let Some(doc) = self.documents.get_mut(&id) else {
            return false;
        };
        if !doc.is_output() || text.is_empty() {
            return false;
        }
        let old_end = doc.text().len_chars();
        let following: Vec<_> = self
            .tree
            .views()
            .filter_map(|(view, _)| {
                (view.doc == id
                    && doc.selection(view.id).primary().is_empty()
                    && doc.selection(view.id).primary().head == old_end)
                    .then_some(view.id)
            })
            .collect();
        let text_start = tail_start(text, OUTPUT_LIMIT);
        let text = &text[text_start..];
        let remove_bytes = if text_start > 0 {
            doc.text().len()
        } else {
            (doc.text().len() + text.len()).saturating_sub(OUTPUT_LIMIT)
        };
        let mut remove_chars = doc.text().byte_to_char_idx(remove_bytes);
        if doc.text().char_to_byte_idx(remove_chars) < remove_bytes {
            remove_chars += 1;
        }
        let changes = Transaction::change(
            doc.text(),
            [
                (0, remove_chars, None),
                (old_end, old_end, Some(text.into())),
            ]
            .into_iter(),
        );
        doc.replace_readonly_content(changes.changes(), false)
            .expect("output changes refer to the current document");
        for (view, _) in self.tree.views_mut() {
            view.jumps.apply(changes.changes(), doc);
        }
        let scrolloff = self.config.load().scroll.padding;
        for view in following {
            doc.set_selection(view, Selection::point(doc.text().len_chars()));
            self.tree
                .get_mut(view)
                .ensure_cursor_in_view(doc, scrolloff);
        }
        self.redraw().request();
        true
    }
}

fn tail_start(text: &str, limit: usize) -> usize {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_follows_the_end_and_preserves_older_reading_positions() {
        let (mut editor, _store) = crate::editor::tests::editor("hello#[|]#");
        let mut output = OutputBuffer::default();
        let id = output.open(&mut editor, "Build", Action::Replace);
        let view = editor.tree.focus();
        output.append(&mut editor, &"line\n".repeat(100));
        assert!(editor.document(id).unwrap().view_offset(view).anchor > 0);
        let doc = editor.document_mut(id).unwrap();
        doc.set_selection(view, Selection::point(0));
        doc.set_view_offset(view, Default::default());
        let selection = doc.selection(view).clone();
        output.append(&mut editor, "another line\n");
        let doc = editor.document(id).unwrap();
        assert_eq!(doc.selection(view), &selection);
        assert_eq!(doc.view_offset(view).anchor, 0);
    }

    #[tokio::test]
    async fn output_is_bounded_readonly_and_reopens_after_closure() {
        let (mut editor, _store) = crate::editor::tests::editor("hello#[|]#");
        let mut output = OutputBuffer::default();
        output.append(&mut editor, "first\nsecond\n");
        let id = output.open(&mut editor, "Build", Action::Replace);
        let doc = editor.document(id).unwrap();
        assert!(doc.is_content_readonly() && !doc.is_modified());
        assert_eq!(doc.text().to_string(), "first\nsecond\n");
        assert!(editor.close_document(id, true).is_ok());
        let reopened = output.open(&mut editor, "Build", Action::VerticalSplit);
        assert_ne!(id, reopened);
        assert!(editor.document(reopened).unwrap().text() == output.text());
        output.append(&mut editor, &"界".repeat(OUTPUT_LIMIT));
        assert!(output.text().len() <= OUTPUT_LIMIT);
        assert!(editor.document(reopened).unwrap().text() == output.text());
    }
}

//! Registered document storage and editor-local ID allocation.

use crate::{Document, DocumentId};
use indexmap::IndexMap;
use std::ops::Index;

/// Documents registered with an editor, in buffer navigation order.
///
/// Individual documents can be borrowed independently of the editor's other
/// fields. Registration, removal, and ID allocation stay private so callers
/// cannot bypass service attachment, write tracking, or close notifications.
pub struct Documents {
    documents: IndexMap<DocumentId, Document>,
    next_id: DocumentId,
}

impl Documents {
    pub(super) fn new() -> Self {
        Self {
            documents: IndexMap::new(),
            next_id: DocumentId::default(),
        }
    }

    pub(super) fn insert(&mut self, mut doc: Document) -> DocumentId {
        let id = self.next_id;
        self.next_id = DocumentId(id.0.checked_add(1).expect("document IDs exhausted"));
        doc.id = id;
        self.documents.insert(id, doc);
        id
    }

    pub(super) fn remove(&mut self, id: &DocumentId) -> Option<Document> {
        self.documents.shift_remove(id)
    }

    /// Move a document to a zero-based position, clamping to the last buffer.
    pub(super) fn move_to(&mut self, id: DocumentId, target: usize) -> bool {
        let Some(current) = self.documents.get_index_of(&id) else {
            return false;
        };
        let target = target.min(self.len() - 1);
        if current == target {
            return false;
        }
        self.documents.move_index(current, target);
        true
    }

    pub fn get(&self, id: &DocumentId) -> Option<&Document> {
        self.documents.get(id)
    }
    pub fn get_mut(&mut self, id: &DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(id)
    }
    pub fn contains_key(&self, id: &DocumentId) -> bool {
        self.documents.contains_key(id)
    }
    pub fn len(&self) -> usize {
        self.documents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &DocumentId> + ExactSizeIterator + Clone {
        self.documents.keys()
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &Document> + ExactSizeIterator {
        self.documents.values()
    }
    pub fn values_mut(
        &mut self,
    ) -> impl DoubleEndedIterator<Item = &mut Document> + ExactSizeIterator {
        self.documents.values_mut()
    }
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&DocumentId, &Document)> + ExactSizeIterator {
        self.documents.iter()
    }
}

impl Index<&DocumentId> for Documents {
    type Output = Document;
    fn index(&self, id: &DocumentId) -> &Document {
        &self.documents[id]
    }
}

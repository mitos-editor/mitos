//! Registered document storage and editor-local ID allocation.

use crate::{Document, DocumentId};
use std::{collections::BTreeMap, ops::Index};

/// Documents registered with an editor.
///
/// Individual documents can be borrowed independently of the editor's other
/// fields. Registration, removal, and ID allocation stay private so callers
/// cannot bypass service attachment, write tracking, or close notifications.
pub struct Documents {
    documents: BTreeMap<DocumentId, Document>,
    next_id: DocumentId,
    document_order: Vec<DocumentId>,
}

impl Documents {
    pub(super) fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
            next_id: DocumentId::default(),
            document_order: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, mut doc: Document) -> DocumentId {
        let id = self.next_id;
        self.next_id = DocumentId(id.0.checked_add(1).expect("document IDs exhausted"));

        doc.id = id;

        let previous = self.documents.insert(id, doc);
        debug_assert!(previous.is_none());
        self.document_order.push(id);

        id
    }

    pub(super) fn remove(&mut self, id: &DocumentId) -> Option<Document> {
        let document = self.documents.remove(id)?;

        let position = self
            .document_order
            .iter()
            .position(|ordered_id| ordered_id == id);

        debug_assert!(
            position.is_some(),
            "registered document missing from document order"
        );

        if let Some(position) = position {
            self.document_order.remove(position);
        }

        Some(document)
    }

    pub fn move_up(&mut self, id: DocumentId, count: usize) -> bool {
        let Some(current) = self.position(id) else {
            return false;
        };

        let Some(last) = self.document_order.len().checked_sub(1) else {
            return false;
        };

        let target = current.saturating_add(count).min(last);
        self.move_to(current, target)
    }

    pub fn move_down(&mut self, id: DocumentId, count: usize) -> bool {
        let Some(current) = self.position(id) else {
            return false;
        };

        let target = current.saturating_sub(count);
        self.move_to(current, target)
    }

    fn move_to(&mut self, current: usize, target: usize) -> bool {
        if current == target {
            return false;
        }

        let id = self.document_order.remove(current);
        self.document_order.insert(target, id);
        true
    }

    fn position(&self, id: DocumentId) -> Option<usize> {
        self.document_order
            .iter()
            .position(|ordered_id| *ordered_id == id)
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
        self.document_order.iter()
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &Document> + ExactSizeIterator {
        self.document_order.iter().map(|id| {
            self.documents
                .get(id)
                .expect("document order contains an unregistered document")
        })
    }

    pub fn values_unordered(
        &self,
    ) -> impl DoubleEndedIterator<Item = &Document> + ExactSizeIterator {
        self.documents.values()
    }

    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&DocumentId, &Document)> + ExactSizeIterator {
        self.document_order.iter().map(|id| {
            let document = self
                .documents
                .get(id)
                .expect("document order contains an unregistered document");

            (id, document)
        })
    }

    pub fn values_mut(
        &mut self,
    ) -> impl DoubleEndedIterator<Item = &mut Document> + ExactSizeIterator {
        // Mutable iteration cannot easily follow `document_order` with a
        // BTreeMap without unsafe code or repeated lookup restrictions.
        //
        // Callers of this method must not rely on iteration order.
        self.documents.values_mut()
    }
}

impl Index<&DocumentId> for Documents {
    type Output = Document;
    fn index(&self, id: &DocumentId) -> &Document {
        &self.documents[id]
    }
}

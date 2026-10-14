//! Bounded document/view catalogs and invocation snapshots.
use super::PluginConflict;
use crate::{Document, Editor, ViewId};
use ::plugins::InvocationTarget;
use anyhow::ensure;
use plugin_api::{
    DocumentInfo, DocumentSnapshot, EditorContext, ErrorCode, SelectionRange, ServiceError,
    StateCatalog, StateQuery, ViewInfo, ViewSnapshot, MAX_STATE_PAGE_SIZE,
};

pub(super) fn snapshot(doc: &Document) -> anyhow::Result<DocumentSnapshot> {
    Ok(DocumentSnapshot {
        id: doc.id().as_u64(),
        version: doc.version(),
        path: doc.path().map(|path| path.to_string_lossy().into_owned()),
        language: doc.language_name().map(str::to_owned),
        char_count: doc.text().len_chars() as u64,
        byte_count: doc.text().len_bytes() as u64,
    })
}

pub(super) fn view_snapshot(doc: &Document, view: ViewId) -> Option<ViewSnapshot> {
    let selection = doc.selections().get(&view)?;
    if selection.len() > 1024 {
        return None;
    }
    Some(ViewSnapshot {
        id: view.as_u64(),
        document: doc.id().as_u64(),
        binding_revision: doc.view_binding_revision(view)?,
        selection_revision: doc.selection_revision(view)?,
        selections: selection
            .iter()
            .map(|range| SelectionRange {
                anchor: range.anchor,
                head: range.head,
            })
            .collect(),
        primary: selection.primary_index(),
    })
}

pub(super) fn bounded_view_snapshot(
    doc: &Document,
    view: ViewId,
) -> anyhow::Result<Option<ViewSnapshot>> {
    if doc
        .selections()
        .get(&view)
        .is_some_and(|selection| selection.len() > 1024)
    {
        return Err(ServiceError::new(
            ErrorCode::ResourceExhausted,
            "view has more than 1024 selections; plugin snapshot omitted",
        )
        .into());
    }
    Ok(view_snapshot(doc, view))
}

impl Editor {
    /// Page catalogs and optionally retrieve the queried document/view snapshots.
    /// Handles and cursors belong to this editor session.
    pub fn plugin_state(&self, query: &StateQuery) -> (EditorContext, StateCatalog) {
        let mut catalog = self.plugin_state_catalog(query);
        let mut context = self.plugin_global_context();
        let requested = (|| -> anyhow::Result<()> {
            if let Some((doc, view)) = self.requested_plugin_state(query)? {
                context.document = Some(snapshot(doc)?);
                context.view = view
                    .map(|view| bounded_view_snapshot(doc, view))
                    .transpose()?
                    .flatten();
            }
            Ok(())
        })();
        if let Err(error) = requested {
            catalog.error = Some(error.to_string());
            context.document = None;
            context.view = None;
        }
        (context, catalog)
    }

    fn requested_plugin_state(
        &self,
        query: &StateQuery,
    ) -> anyhow::Result<Option<(&Document, Option<ViewId>)>> {
        let view = query
            .view
            .map(|id| {
                self.tree
                    .try_get(ViewId::from_u64(id))
                    .ok_or(PluginConflict::ViewClosed(id))
            })
            .transpose()?;
        let document = query
            .document
            .or_else(|| view.map(|view| view.doc.as_u64()));
        let Some(id) = document else { return Ok(None) };
        let doc = self
            .documents
            .values()
            .find(|doc| doc.id().as_u64() == id)
            .ok_or(PluginConflict::DocumentClosed(id))?;
        if let Some(view) = view {
            ensure!(
                view.doc == doc.id(),
                "queried view is bound to another document"
            );
        }
        Ok(Some((doc, view.map(|view| view.id))))
    }

    /// Immediate metadata queries never materialize selection snapshots.
    pub(super) fn plugin_state_catalog(&self, query: &StateQuery) -> StateCatalog {
        let limit = if query.limit == 0 {
            MAX_STATE_PAGE_SIZE
        } else {
            query.limit.min(MAX_STATE_PAGE_SIZE)
        };
        let mut documents = self.documents.values().filter(|doc| {
            query
                .after_document
                .is_none_or(|after| doc.id().as_u64() > after)
        });
        let page = documents.by_ref().take(limit).collect::<Vec<_>>();
        let next_document = documents
            .next()
            .and_then(|_| page.last().map(|doc| doc.id().as_u64()));
        let documents = page
            .into_iter()
            .map(|doc| DocumentInfo {
                id: doc.id().as_u64(),
                version: doc.version(),
                path: doc.path().map(|path| path.to_string_lossy().into_owned()),
                language: doc.language_name().map(str::to_owned),
                readonly: doc.readonly,
                binary: doc.is_binary(),
                bytes: doc.text().len_bytes(),
                chars: doc.text().len_chars(),
            })
            .collect();
        let mut views = self
            .tree
            .views()
            .map(|(view, _)| view)
            .filter(|view| {
                query
                    .after_view
                    .is_none_or(|after| view.id.as_u64() > after)
            })
            .collect::<Vec<_>>();
        views.sort_by_key(|view| view.id.as_u64());
        let next_view = (views.len() > limit).then(|| views[limit - 1].id.as_u64());
        let views = views
            .into_iter()
            .take(limit)
            .map(|view| ViewInfo {
                id: view.id.as_u64(),
                document: view.doc.as_u64(),
                binding_revision: view.binding_revision(),
                selection_revision: self
                    .document(view.doc)
                    .and_then(|doc| doc.selection_revision(view.id))
                    .unwrap_or(0),
            })
            .collect();
        StateCatalog {
            documents,
            views,
            next_document,
            next_view,
            error: self
                .requested_plugin_state(query)
                .err()
                .map(|error| error.to_string()),
        }
    }

    pub(super) fn plugin_context_for_view(
        &self,
        origin: Option<ViewId>,
    ) -> anyhow::Result<EditorContext> {
        let view = origin.and_then(|id| self.tree.try_get(id));
        let doc = view.and_then(|view| self.document(view.doc));
        Ok(EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            document: doc.map(snapshot).transpose()?,
            view: view
                .zip(doc)
                .map(|(view, doc)| bounded_view_snapshot(doc, view.id))
                .transpose()?
                .flatten(),
        })
    }

    pub(super) fn plugin_context(&self) -> anyhow::Result<EditorContext> {
        self.plugin_context_for_view(Some(self.tree.focus))
    }

    pub(super) fn plugin_invocation_target(&self, view: Option<ViewId>) -> InvocationTarget {
        let view = view.and_then(|view| self.tree.try_get(view));
        InvocationTarget {
            call_sequence: None,
            document: view.map(|view| view.doc.as_u64()),
            view: view.map(|view| view.id.as_u64()),
            binding_revision: view.map(|view| view.binding_revision()),
        }
    }

    pub(super) fn plugin_global_context(&self) -> EditorContext {
        EditorContext {
            generation: self.plugins.shared.generation,
            mode: self.mode.to_string(),
            ..EditorContext::default()
        }
    }
}

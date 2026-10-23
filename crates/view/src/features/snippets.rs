//! Editor-owned snippet sources, workspace scoping, and validated insertion handles.
mod variables;

use crate::{Document, DocumentId, Editor, ViewId};
use anyhow::{ensure, Result};
use snippets::{Catalog, UserSnippet};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use workspace::trust::TrustQuery;

#[derive(Default)]
pub(crate) struct SnippetStore {
    resources: loader::snippets::Resources,
    catalog: Arc<Catalog>,
    projects: HashMap<PathBuf, Arc<Catalog>>,
    loaded: bool,
}

/// A picker choice belongs to its original document, selections, and catalog.
#[derive(Clone)]
pub struct SnippetCandidate {
    pub snippet: Arc<UserSnippet>,
    catalog: Arc<Catalog>,
    document: DocumentId,
    view: ViewId,
    version: i32,
    selection: editor_core::Selection,
    uri: Option<editor_core::Uri>,
    scope: String,
    trusted: bool,
    mode: crate::document::Mode,
}

pub(crate) fn language_scope(doc: &Document, view: ViewId) -> &str {
    let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
    doc.language_config_at(doc.text().char_to_byte_idx(cursor))
        .map(|language| language.language_id.as_str())
        .or_else(|| doc.language_name())
        .unwrap_or(crate::document::DEFAULT_LANGUAGE_NAME)
}

impl Editor {
    /// Install validated global definitions and cancel results from the old catalog.
    pub fn set_snippet_catalog(&mut self, catalog: Catalog) {
        super::completion::invalidate(self);
        self.snippets.catalog = Arc::new(catalog);
        self.snippets.projects.clear();
        self.snippets.loaded = true;
    }

    /// Replace configuration discovery for an editor without touching other editors.
    pub fn set_snippet_resources(&mut self, resources: loader::snippets::Resources) {
        super::completion::invalidate(self);
        self.snippets = SnippetStore {
            resources,
            ..Default::default()
        };
    }

    pub fn snippet_user_file(&self) -> &Path {
        &self.snippets.resources.user_file
    }

    /// Reload global and open trusted workspace definitions as one atomic change.
    /// Failed loads preserve the last valid catalogs; trust is checked separately on use.
    pub fn reload_snippets(&mut self) -> Result<()> {
        let catalog = Arc::new(loader::snippets::load(&self.snippets.resources.user_file)?);
        let roots: HashSet<_> = self
            .documents
            .values()
            .map(|doc| doc.workspace_root().to_path_buf())
            .collect();
        let mut projects = HashMap::new();
        for root in roots {
            self.workspace_trust.invalidate(&root);
            if self
                .workspace_trust
                .query(&root, TrustQuery::LocalConfig)
                .is_trusted()
            {
                let project = loader::snippets::load(&loader::snippets::workspace_file(&root))?;
                projects.insert(root, Arc::new(catalog.with_overrides(&project)));
            }
        }
        super::completion::invalidate(self);
        self.snippets.catalog = catalog;
        self.snippets.projects = projects;
        self.snippets.loaded = true;
        Ok(())
    }

    pub(crate) fn snippet_catalog_for(&mut self, document: DocumentId) -> Arc<Catalog> {
        if !self.snippets.loaded {
            self.snippets.loaded = true;
            match loader::snippets::load(&self.snippets.resources.user_file) {
                Ok(catalog) => self.snippets.catalog = Arc::new(catalog),
                Err(error) => self.set_error(|| format!("{error:#}")),
            }
        }
        let Some(doc) = self.document(document) else {
            return self.snippets.catalog.clone();
        };
        let root = doc.workspace_root().to_path_buf();
        if !self
            .workspace_trust
            .query(&root, TrustQuery::LocalConfig)
            .is_trusted()
        {
            // Do not reuse a previously trusted project's definitions after revocation.
            self.snippets.projects.remove(&root);
            return self.snippets.catalog.clone();
        }
        if let Some(catalog) = self.snippets.projects.get(&root) {
            return catalog.clone();
        }
        let catalog = match loader::snippets::load(&loader::snippets::workspace_file(&root)) {
            Ok(project) => Arc::new(self.snippets.catalog.with_overrides(&project)),
            Err(error) => {
                self.set_error(|| format!("{error:#}"));
                self.snippets.catalog.clone()
            }
        };
        self.snippets.projects.insert(root, catalog.clone());
        catalog
    }

    pub fn snippet_candidates(&mut self) -> Vec<SnippetCandidate> {
        let document = doc!(self).id();
        let catalog = self.snippet_catalog_for(document);
        let (view, doc) = current_ref!(self);
        let scope = language_scope(doc, view.id);
        let trusted = self
            .workspace_trust
            .query(doc.workspace_root(), TrustQuery::LocalConfig)
            .is_trusted();
        catalog
            .matching(scope)
            .map(|snippet| SnippetCandidate {
                snippet: snippet.clone(),
                catalog: catalog.clone(),
                document,
                view: view.id,
                mode: self.mode(),
                version: doc.version(),
                selection: doc.selection(view.id).clone(),
                uri: doc.uri(),
                scope: scope.to_owned(),
                trusted,
            })
            .collect()
    }

    pub fn insert_user_snippet(&mut self, candidate: &SnippetCandidate) -> Result<()> {
        let (view, doc) = current_ref!(self);
        ensure!(
            self.mode() == candidate.mode
                && view.id == candidate.view
                && doc.id() == candidate.document
                && doc.version() == candidate.version
                && doc.selection(view.id) == &candidate.selection
                && doc.uri() == candidate.uri
                && language_scope(doc, view.id) == candidate.scope,
            "snippet insertion context changed; reopen the snippet picker"
        );
        let trusted = self
            .workspace_trust
            .query(doc.workspace_root(), TrustQuery::LocalConfig)
            .is_trusted();
        ensure!(
            trusted == candidate.trusted,
            "workspace trust changed; reopen the snippet picker"
        );
        let catalog = self.snippet_catalog_for(candidate.document);
        ensure!(
            Arc::ptr_eq(&catalog, &candidate.catalog),
            "snippet definitions changed; reopen the snippet picker"
        );
        self.insert_snippet(&candidate.snippet.snippet)?;
        Ok(())
    }
}

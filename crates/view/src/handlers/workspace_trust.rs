//! Editor-owned workspace trust prompts and service coordination.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use event::register_hook;
use loader::workspace_trust::{compute_workspace_hash, TrustStatus};

use crate::{editor::ConfigEvent, events::DocumentDidOpen, DocumentId, Editor};

#[derive(Default)]
pub struct WorkspaceTrustHandler {
    prompted: HashSet<PathBuf>,
    pending: HashMap<PathBuf, Arc<()>>,
    requests: VecDeque<TrustRequest>,
}

/// A prompt for one workspace in one editor. Frontends return it when resolving
/// or dismissing the prompt; it cannot authorize another editor or newer state.
#[derive(Debug)]
pub struct TrustRequest {
    workspace: PathBuf,
    owner: Weak<()>,
    hash: Option<String>,
}

impl TrustRequest {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn belongs_to(&self, editor: &Editor) -> bool {
        editor
            .handlers
            .workspace_trust
            .pending
            .get(&self.workspace)
            .is_some_and(|owner| self.owner.ptr_eq(&Arc::downgrade(owner)))
    }

    /// Check again before presenting a request held by the frontend.
    pub fn is_current(&self, editor: &Editor) -> bool {
        self.belongs_to(editor)
            && editor.workspace_trust.prompts_enabled()
            && editor.workspace_trust.status(&self.workspace) == TrustStatus::Untrusted
            && editor.documents().any(|doc| {
                doc.workspace_root() == self.workspace
                    && editor
                        .workspace_trust
                        .restricted_for_doc(&self.workspace, doc.servers_to_load())
            })
            && compute_workspace_hash(&self.workspace) == self.hash
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TrustDecision {
    Trust,
    Untrust,
    Exclude,
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let editor = &mut event.editor;
        let doc = doc!(editor, &event.doc);
        let workspace = doc.workspace_root().to_path_buf();
        let servers_to_load = doc.servers_to_load();

        // Raw status preserves Stale, which capability queries demote to Untrusted.
        if editor.workspace_trust.status(&workspace) == TrustStatus::Stale {
            editor.set_status(
                "Workspace `.mitos/` config changed since `:workspace-trust`. \
                 Local config not loaded. Run `:workspace-trust` to re-allow.",
            );
            return Ok(());
        }
        if !editor
            .workspace_trust
            .restricted_for_doc(&workspace, servers_to_load)
            || !editor.workspace_trust.prompts_enabled()
            || !editor
                .handlers
                .workspace_trust
                .prompted
                .insert(workspace.clone())
        {
            return Ok(());
        }

        editor.workspace_trust.deny_once(&workspace);
        let owner = Arc::new(());
        let request = TrustRequest {
            hash: compute_workspace_hash(&workspace),
            workspace: workspace.clone(),
            owner: Arc::downgrade(&owner),
        };
        let handler = &mut editor.handlers.workspace_trust;
        handler.pending.insert(workspace, owner);
        handler.requests.push_back(request);
        event::request_redraw();
        Ok(())
    });
}

/// Drain prompt requests through the owning editor's event loop.
pub fn next_request(editor: &mut Editor) -> Option<TrustRequest> {
    while let Some(request) = editor.handlers.workspace_trust.requests.pop_front() {
        if request.is_current(editor) {
            return Some(request);
        }
        dismiss_request(editor, &request);
    }
    None
}

/// Dismiss without persisting a decision or allowing another prompt this session.
pub fn dismiss_request(editor: &mut Editor, request: &TrustRequest) {
    if request.belongs_to(editor) {
        editor
            .handlers
            .workspace_trust
            .pending
            .remove(&request.workspace);
    }
}

fn record_decision(editor: &mut Editor, workspace: &Path, decision: TrustDecision) {
    editor.handlers.workspace_trust.pending.remove(workspace);
    match decision {
        TrustDecision::Trust => editor.workspace_trust.trust(workspace),
        TrustDecision::Untrust => editor.workspace_trust.untrust(workspace),
        TrustDecision::Exclude => editor.workspace_trust.exclude(workspace),
    }
}

/// Apply an explicit decision and reload configuration. Revocation and exclusion
/// leave running language servers alone; stopping them is a separate operation.
pub fn apply_decision(
    editor: &mut Editor,
    workspace: &Path,
    decision: TrustDecision,
) -> anyhow::Result<()> {
    record_decision(editor, workspace, decision);
    editor.config_events.0.send(ConfigEvent::Refresh)?;
    Ok(())
}

/// Trust from a command, preserving its current-document server restart behavior.
pub fn trust_and_restart(
    editor: &mut Editor,
    document: DocumentId,
    servers: &[&str],
) -> anyhow::Result<()> {
    let workspace = editor
        .document(document)
        .ok_or_else(|| anyhow::anyhow!("Document no longer exists"))?
        .workspace_root()
        .to_path_buf();
    apply_decision(editor, &workspace, TrustDecision::Trust)?;
    editor.restart_language_servers(document, servers)
}

/// Apply a prompt response only while its editor, workspace and trust state are
/// current. Trusting launches missing servers for open documents without restarting.
pub fn resolve_request(
    editor: &mut Editor,
    request: &TrustRequest,
    decision: TrustDecision,
) -> anyhow::Result<bool> {
    if !request.is_current(editor) {
        dismiss_request(editor, request);
        return Ok(false);
    }
    record_decision(editor, &request.workspace, decision);
    if matches!(decision, TrustDecision::Trust) {
        let documents: Vec<_> = editor.documents.keys().copied().collect();
        for document in documents {
            editor.launch_language_servers(document);
        }
    }
    editor.config_events.0.send(ConfigEvent::Refresh)?;
    Ok(true)
}

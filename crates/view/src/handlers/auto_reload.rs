//! Editor-owned external file changes, polling, and reload confirmations.
//!
//! Adapted from work by Pascal Kuthe and Blaž Hrastnik in
//! [Helix PR #14544](https://github.com/helix-editor/helix/pull/14544),
//! including reload contributions by Anthony Rubick.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Weak},
    time::{Duration, SystemTime},
};

use crate::file_watcher::{EventType, Events};
use event::{register_hook, send_blocking, AsyncHook};
use stdx::path::canonicalize_existing;
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{
    callbacks::EditorCallbackSender, config::Config, events::ConfigDidChange, DocumentId, Editor,
};

#[derive(Debug)]
enum PollEvent {
    PollAfter { interval: u64 },
    Stop,
}

pub struct AutoReloadHandler {
    callbacks: Arc<EditorCallbackSender>,
    events: Sender<PollEvent>,
    confirmations: VecDeque<ReloadRequest>,
}

impl AutoReloadHandler {
    pub fn new(callbacks: EditorCallbackSender, config: &Config) -> Self {
        let callbacks = Arc::new(callbacks);
        let weak = Arc::downgrade(&callbacks);
        let events = PollHandler {
            callbacks: weak.clone(),
        }
        .spawn();
        let handler = Self {
            callbacks,
            events,
            confirmations: VecDeque::new(),
        };
        handler.configure(config);
        handler
    }

    fn configure(&self, config: &Config) {
        let event = if config.auto_reload.poll.enable
            && (config.auto_reload.enable || config.file_watcher.watch_vcs)
        {
            PollEvent::PollAfter {
                interval: config.auto_reload.poll.interval.max(100),
            }
        } else {
            PollEvent::Stop
        };
        send_blocking(&self.events, event);
    }
}

fn dispatch(
    callbacks: &Weak<EditorCallbackSender>,
    callback: impl FnOnce(&mut Editor) + Send + 'static,
) {
    let Some(sender) = callbacks.upgrade() else {
        return;
    };
    let owner = callbacks.clone();
    sender.send_blocking(move |editor| {
        // Also reject callbacks queued before this handler was replaced.
        if owner.ptr_eq(&Arc::downgrade(&editor.handlers.auto_reload.callbacks)) {
            callback(editor);
        }
    });
}

struct PollHandler {
    callbacks: Weak<EditorCallbackSender>,
}

impl AsyncHook for PollHandler {
    type Event = PollEvent;

    fn handle_event(&mut self, event: Self::Event, _: Option<Instant>) -> Option<Instant> {
        match event {
            PollEvent::PollAfter { interval } => {
                Some(Instant::now() + Duration::from_millis(interval))
            }
            PollEvent::Stop => None,
        }
    }

    fn finish_debounce(&mut self) {
        dispatch(&self.callbacks, |editor| {
            if editor.config().auto_reload.poll.enable {
                check_unwatched(editor);
            }
            editor.handlers.auto_reload.configure(&editor.config());
        });
    }
}

/// A request to discard unsaved changes for one observed external modification.
/// Frontends display it and return the user's decision to `resolve_reload`.
#[derive(Debug)]
pub struct ReloadRequest {
    owner: Weak<EditorCallbackSender>,
    doc: DocumentId,
    path: PathBuf,
    display: String,
    version: i32,
    saved: SystemTime,
    modified: SystemTime,
}

impl ReloadRequest {
    pub fn display_name(&self) -> &str {
        &self.display
    }

    fn belongs_to(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.auto_reload.callbacks))
    }

    fn is_current(&self, editor: &Editor) -> bool {
        self.belongs_to(editor)
            && editor.config().auto_reload.enable
            && editor.config().auto_reload.prompt_if_modified
            && editor.document(self.doc).is_some_and(|doc| {
                doc.path() == Some(self.path.as_path())
                    && doc.version() == self.version
                    && doc.last_saved_time() == self.saved
                    && doc.is_modified()
                    && doc.auto_reload_seen_mtime == Some(self.modified)
            })
            && self.path.metadata().and_then(|meta| meta.modified()).ok() == Some(self.modified)
    }

    fn discard_stale(&self, editor: &mut Editor) {
        if self.belongs_to(editor)
            && let Some(doc) = editor.documents.get_mut(&self.doc)
            && doc.path() == Some(self.path.as_path())
            && doc.auto_reload_seen_mtime == Some(self.modified)
        {
            // A later check may ask about the current state, rather than leaving
            // this external modification permanently suppressed by a stale prompt.
            doc.auto_reload_seen_mtime = None;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ReloadDecision {
    Reload,
    Ignore,
}

/// Drain valid requests without requiring a compositor or terminal event loop.
pub fn next_reload_request(editor: &mut Editor) -> Option<ReloadRequest> {
    while let Some(request) = editor.handlers.auto_reload.confirmations.pop_front() {
        if request.is_current(editor) {
            return Some(request);
        }
        request.discard_stale(editor);
    }
    None
}

/// Apply a decision only to the editor and state for which it was requested.
pub fn resolve_reload(editor: &mut Editor, request: ReloadRequest, decision: ReloadDecision) {
    if !request.is_current(editor) {
        request.discard_stale(editor);
        return;
    }
    match decision {
        ReloadDecision::Reload => {
            if let Err(err) = reload_document(editor, request.doc) {
                editor.set_error(|| format!("{} reload failed: {err}", request.display));
            }
        }
        ReloadDecision::Ignore => {
            editor.set_status(format!("{} external changes ignored", request.display))
        }
    }
}

/// Focus checks work even when periodic polling is disabled.
pub fn check_unwatched(editor: &mut Editor) {
    if editor.config().auto_reload.enable {
        let ids: Vec<_> = editor
            .documents()
            .filter(|doc| {
                doc.path()
                    .is_some_and(|path| !editor.file_watcher.is_watching(path))
            })
            .map(|doc| doc.id())
            .collect();
        for id in ids {
            if let Some(doc) = editor.documents.get(&id)
                && let Some(path) = doc.path()
                && let Ok(mtime) = path.metadata().and_then(|meta| meta.modified())
                && mtime != doc.last_saved_time()
                && doc.auto_reload_seen_mtime != Some(mtime)
            {
                editor
                    .language_servers
                    .file_event_handler
                    .file_changed(path.to_path_buf(), lsp_client::lsp::FileChangeType::CHANGED);
            }
            handle_document_change(editor, id);
        }
    }
    if editor.config().file_watcher.watch_vcs && editor.file_watcher.poll_extra_paths() {
        reload_vcs(editor);
    }
}

fn handle_document_change(editor: &mut Editor, doc_id: DocumentId) {
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    let Some(path) = doc.path().map(ToOwned::to_owned) else {
        return;
    };
    let Ok(mtime) = path.metadata().and_then(|meta| meta.modified()) else {
        // Keep the buffer when its file is deleted; a later recreation can reload it.
        return;
    };
    if mtime == doc.last_saved_time() || doc.auto_reload_seen_mtime == Some(mtime) {
        return;
    }
    if doc.is_modified() {
        doc.auto_reload_seen_mtime = Some(mtime);
        let display = doc.display_name().to_string();
        if editor.config().auto_reload.prompt_if_modified {
            let doc = &editor.documents[&doc_id];
            let request = ReloadRequest {
                owner: Arc::downgrade(&editor.handlers.auto_reload.callbacks),
                doc: doc_id,
                path,
                display,
                version: doc.version(),
                saved: doc.last_saved_time(),
                modified: mtime,
            };
            editor.handlers.auto_reload.confirmations.push_back(request);
        } else {
            editor.set_warning(|| {
                format!(
                    "{display} changed externally but has unsaved changes; use :reload to refresh"
                )
            });
        }
    } else if let Err(err) = reload_document(editor, doc_id) {
        editor.set_error(|| format!("{} auto-reload failed: {err}", path.display()));
    }
}

/// Use the existing reload transaction so selections, undo history, and LSPs stay in sync.
fn reload_document(editor: &mut Editor, doc_id: DocumentId) -> anyhow::Result<()> {
    let view_id = editor.get_synced_view_id(doc_id);
    let scrolloff = editor.config().scrolloff;
    let doc = doc_mut!(editor, &doc_id);
    let trust_full = editor
        .workspace_trust
        .query(
            doc.workspace_root(),
            loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted();
    let view = view_mut!(editor, view_id);
    doc.reload(view, &editor.diff_providers, trust_full)?;
    // Reload commits history through one view. Sync every split displaying the
    // document before its jumplist is used against the new text.
    for (view, _) in editor.tree.views_mut() {
        if view.doc == doc_id {
            view.sync_changes(doc);
            view.ensure_cursor_in_view(doc, scrolloff);
        }
    }
    let display = doc.display_name().to_string();
    editor.set_status(format!("{display} reloaded (external changes)"));
    Ok(())
}

fn reload_vcs(editor: &mut Editor) {
    for doc in editor.documents.values_mut() {
        let Some(path) = doc.path().map(ToOwned::to_owned) else {
            continue;
        };
        let trust_full = editor
            .workspace_trust
            .query(
                doc.workspace_root(),
                loader::workspace_trust::TrustQuery::Git,
            )
            .is_trusted();
        doc.refresh_vcs(&editor.diff_providers, trust_full);
        log::debug!("refreshed VCS state for {}", path.display());
    }
    // HEAD may now point to another loose ref.
    editor.refresh_vcs_watches();
}

/// Apply filesystem changes using the editor's current settings.
pub(crate) fn handle_file_events(editor: &mut Editor, events: &Events) {
    let auto_reload = editor.config().auto_reload.enable;
    let watch_vcs = editor.config().file_watcher.watch_vcs;
    let mut vcs_changed = false;
    let mut ignores_changed = false;
    for event in events.iter() {
        if event.ty == EventType::Tempfile {
            continue;
        }
        let path = event.path.as_std_path();
        ignores_changed |= path.file_name().is_some_and(|name| {
            name == ".gitignore" || name == ".ignore" || name == "filesentryignore"
        }) || path.ends_with(".mitos/ignore");
        vcs_changed |= watch_vcs && editor.file_watcher.is_vcs_path(path);
    }
    if auto_reload {
        let changed: std::collections::HashSet<_> = events
            .iter()
            .filter(|event| matches!(event.ty, EventType::Modified | EventType::Create))
            .map(|event| event.path.as_std_path())
            .collect();
        let ids: Vec<_> = editor
            .documents()
            .filter(|doc| {
                doc.path()
                    .is_some_and(|path| changed.contains(canonicalize_existing(path).as_path()))
            })
            .map(|doc| doc.id())
            .collect();
        for id in ids {
            handle_document_change(editor, id);
        }
    }
    if vcs_changed {
        reload_vcs(editor);
    }
    if ignores_changed {
        editor.file_watcher.refresh_filter();
    }
}

pub fn register_hooks() {
    event::runtime_local! {
        static REGISTER: std::sync::Once = std::sync::Once::new();
    }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            event.editor.handlers.auto_reload.configure(event.new);
            Ok(())
        });
    });
}

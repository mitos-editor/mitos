//! Follow cached links and literal paths as one selection-bound navigation operation.

use std::{collections::HashSet, path::PathBuf};

use editor_core::Range;
use event::cancelable_future;
use futures_util::{future::BoxFuture, FutureExt};
use lsp_client::{lsp, LanguageServerId};
use stdx::{path::find_paths, rope::RopeSliceExt, Url};

use crate::{callbacks::EditorCallbackSender, features::request::SelectionRequest, Editor};

/// The frontend opens local paths or presents URLs through its external launcher.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NavigationTarget {
    Path(PathBuf),
    Url(Url),
}

/// Ready targets can be applied immediately; unresolved LSP links require a task.
pub enum FileNavigation {
    Ready(FileNavigationUpdate),
    Pending(FileNavigationTask),
}

struct Origin {
    selection: SelectionRequest,
    binding_revision: u64,
    owner: EditorCallbackSender,
    uses_lsp: bool,
    mode: crate::document::Mode,
}

impl Origin {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner.same_destination(editor.callback_sender())
            && editor.mode() == self.mode
            && (!self.uses_lsp || editor.config().lsp.enable)
            && self.selection.get(editor).is_some_and(|(view, _)| {
                editor.tree.focus == view.id && view.binding_revision() == self.binding_revision
            })
    }
}

/// Dropping a task abandons its pending resolve requests.
#[must_use]
pub struct FileNavigationTask {
    origin: Origin,
    future: BoxFuture<'static, Vec<NavigationTarget>>,
}

/// Queued navigation is validated again before the frontend opens any targets.
#[must_use]
pub struct FileNavigationUpdate {
    origin: Origin,
    targets: Vec<NavigationTarget>,
}

impl FileNavigationTask {
    pub async fn run(self) -> FileNavigationUpdate {
        let targets = cancelable_future(self.future, self.origin.selection.cancel_handle())
            .await
            .unwrap_or_default();
        FileNavigationUpdate {
            origin: self.origin,
            targets,
        }
    }
}

impl FileNavigationUpdate {
    /// Stale results, including results queued before a focus or binding change, are discarded.
    pub fn into_targets(self, editor: &Editor) -> Option<Vec<NavigationTarget>> {
        self.origin.is_current(editor).then_some(self.targets)
    }
}

/// Resolve all targets before applying any navigation, so one selected link cannot
/// invalidate another link in the same operation. LSP links take precedence over paths.
pub fn prepare_navigation(editor: &mut Editor) -> FileNavigation {
    let owner = editor.callback_sender().clone();
    let lsp_enabled = editor.config().lsp.enable;
    let mode = editor.mode();
    let view = editor.tree.get(editor.tree.focus);
    let doc = editor.documents.get_mut(&view.doc).unwrap();
    let ranges = doc.selection(view.id).ranges().to_vec();
    let mut targets = Vec::new();
    let mut seen_links = HashSet::new();
    let mut servers = HashSet::<LanguageServerId>::new();
    let mut resolves = Vec::new();
    let mut fallback = Vec::new();

    for selection in ranges {
        let mut matched = false;
        for link in doc
            .document_links()
            .iter()
            .filter(|link| lsp_enabled && doc.supports_language_server(link.language_server_id))
        {
            if !overlaps(&selection, link.start, link.end) {
                continue;
            }
            matched = true;
            servers.insert(link.language_server_id);
            if let Some(url) = &link.link.target {
                targets.push(NavigationTarget::Url(url.clone()));
            } else if seen_links.insert((link.start, link.end, link.language_server_id)) {
                let Some(server) = doc
                    .language_servers()
                    .find(|server| server.id() == link.language_server_id)
                else {
                    continue;
                };
                if let Some(resolve) = server.resolve_document_link(link.link.clone()) {
                    resolves.push(resolve);
                }
            }
        }
        if !matched {
            fallback.push(selection);
        }
    }
    targets.extend(literal_targets(doc, &fallback));
    let cancel = doc.document_links.start_navigation(view.id);
    let origin = Origin {
        selection: SelectionRequest::new(doc, view.id, cancel, servers.iter().copied().collect()),
        binding_revision: view.binding_revision(),
        owner,
        uses_lsp: !servers.is_empty(),
        mode,
    };
    if resolves.is_empty() {
        deduplicate(&mut targets);
        return FileNavigation::Ready(FileNavigationUpdate { origin, targets });
    }
    let future = async move {
        for response in futures_util::future::join_all(resolves).await {
            match response {
                Ok(lsp::DocumentLink {
                    target: Some(url), ..
                }) => targets.push(NavigationTarget::Url(url)),
                Ok(_) => {}
                Err(error) => log::warn!("Failed to resolve document link: {error}"),
            }
        }
        deduplicate(&mut targets);
        targets
    }
    .boxed();
    FileNavigation::Pending(FileNavigationTask { origin, future })
}

fn deduplicate(targets: &mut Vec<NavigationTarget>) {
    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(target.clone()));
}

fn overlaps(selection: &Range, start: usize, end: usize) -> bool {
    if selection.is_empty() {
        start <= selection.from() && selection.from() < end
    } else {
        selection.from() < end && start < selection.to()
    }
}

fn literal_targets(doc: &crate::Document, ranges: &[Range]) -> Vec<NavigationTarget> {
    let text = doc.text().slice(..);
    let directory = doc
        .path()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| std::path::Path::new(""));
    let paths: Vec<_> = if ranges.len() == 1 && ranges[0].len() == 1 {
        let selection = ranges[0];
        let position = text.char_to_byte(selection.cursor(text));
        let start = text
            .line_to_byte(text.byte_to_line(position))
            .max(text.floor_char_boundary(position.saturating_sub(1000)));
        let end = text
            .line_to_byte(text.byte_to_line(position) + 1)
            .min(text.ceil_char_boundary(position + 1000));
        let search = text.byte_slice(start..end);
        let path = find_paths(search, true)
            .take_while(|range| start + range.start <= position + 1)
            .find(|range| position <= start + range.end)
            .map(|range| search.byte_slice(range).to_string())
            .unwrap_or_else(|| selection.fragment(text).into_owned());
        vec![path]
    } else {
        ranges
            .iter()
            .map(|range| range.fragment(text).trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect()
    };
    paths
        .into_iter()
        .map(|path| {
            if !std::path::Path::new(&path).is_absolute()
                && let Ok(url) = Url::parse(&path)
            {
                return NavigationTarget::Url(url);
            }
            NavigationTarget::Path(directory.join(stdx::path::expand(&path)))
        })
        .collect()
}

//! Manual language navigation and selection, independent of picker presentation.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use editor_core::{syntax::config::LanguageServerFeature, Selection, Uri};
use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use futures_util::{
    future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt,
};
use lsp_client::{lsp, Client, OffsetEncoding};
use parking_lot::Mutex;

use super::request::SelectionRequest;
use crate::{
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
    DocumentId, Editor, ViewId,
};

/// A destination and its originating server's position encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub uri: Uri,
    pub range: lsp::Range,
    pub offset_encoding: OffsetEncoding,
}

impl Location {
    fn from_lsp(location: lsp::Location, offset_encoding: OffsetEncoding) -> Option<Self> {
        match location.uri.try_into() {
            Ok(uri) => Some(Self {
                uri,
                range: location.range,
                offset_encoding,
            }),
            Err(error) => {
                log::warn!("discarding invalid or unsupported URI: {error}");
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationKind {
    Declaration,
    Definition,
    TypeDefinition,
    Implementation,
    References,
    /// Select document highlights from the first supported server in the current document.
    SelectReferences,
}

impl NavigationKind {
    fn feature(self) -> LanguageServerFeature {
        match self {
            Self::Declaration => LanguageServerFeature::GotoDeclaration,
            Self::Definition => LanguageServerFeature::GotoDefinition,
            Self::TypeDefinition => LanguageServerFeature::GotoTypeDefinition,
            Self::Implementation => LanguageServerFeature::GotoImplementation,
            Self::References => LanguageServerFeature::GotoReference,
            Self::SelectReferences => LanguageServerFeature::DocumentHighlight,
        }
    }

    fn not_found(self) -> &'static str {
        match self {
            Self::Declaration => "No declaration found.",
            Self::Definition => "No definition found.",
            Self::TypeDefinition => "No type definition found.",
            Self::Implementation => "No implementation found.",
            Self::References | Self::SelectReferences => "No references found.",
        }
    }

    fn request(
        self,
        server: &Client,
        doc: &crate::Document,
        view: ViewId,
        include_declaration: bool,
    ) -> Option<BoxFuture<'static, lsp_client::Result<Response>>> {
        let position = doc.position(view, server.offset_encoding());
        let identifier = doc.identifier();
        match self {
            Self::Declaration => server
                .goto_declaration(identifier, position, None)
                .map(|future| future.map_ok(definition_locations).boxed()),
            Self::Definition => server
                .goto_definition(identifier, position, None)
                .map(|future| future.map_ok(definition_locations).boxed()),
            Self::TypeDefinition => server
                .goto_type_definition(identifier, position, None)
                .map(|future| future.map_ok(definition_locations).boxed()),
            Self::Implementation => server
                .goto_implementation(identifier, position, None)
                .map(|future| future.map_ok(definition_locations).boxed()),
            Self::References => server
                .goto_reference(identifier, position, include_declaration, None)
                .map(|future| {
                    future
                        .map_ok(|locations| Response::Locations(locations.unwrap_or_default()))
                        .boxed()
                }),
            Self::SelectReferences => server
                .text_document_document_highlight(identifier, position, None)
                .map(|future| {
                    future
                        .map_ok(|highlights| Response::Highlights(highlights.unwrap_or_default()))
                        .boxed()
                }),
        }
    }
}

enum Response {
    Locations(Vec<lsp::Location>),
    Highlights(Vec<lsp::DocumentHighlight>),
}

fn definition_locations(response: Option<lsp::GotoDefinitionResponse>) -> Response {
    Response::Locations(match response {
        Some(lsp::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(lsp::GotoDefinitionResponse::Array(locations)) => locations,
        Some(lsp::GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| lsp::Location::new(link.target_uri, link.target_range))
            .collect(),
        None => Vec::new(),
    })
}

/// A document owns cancellation even if its view or frontend disappears.
#[derive(Default)]
pub(crate) struct NavigationState {
    request: Option<(ViewId, TaskController)>,
}

impl NavigationState {
    fn restart(&mut self, view: ViewId) -> TaskHandle {
        let (target, controller) = self
            .request
            .get_or_insert_with(|| (view, TaskController::default()));
        *target = view;
        controller.restart()
    }

    pub(crate) fn remove_view(&mut self, view: ViewId) {
        if self
            .request
            .as_ref()
            .is_some_and(|(target, _)| *target == view)
        {
            self.clear();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.request = None;
    }
}

#[derive(Default)]
struct Session {
    target: Option<(DocumentId, ViewId)>,
    pending: Option<NavigationUpdate>,
}

struct Shared {
    callbacks: EditorCallbackSender,
    session: Mutex<Session>,
}

/// Coordinates manual navigation for one editor.
pub struct NavigationHandler {
    shared: Arc<Shared>,
}

impl NavigationHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        Self {
            shared: Arc::new(Shared {
                callbacks,
                session: Mutex::new(Session::default()),
            }),
        }
    }
}

struct Request {
    owner: Weak<Shared>,
    target: SelectionRequest,
    kind: NavigationKind,
    mode: Mode,
}

impl Request {
    fn is_current(&self, editor: &Editor) -> bool {
        self.owner
            .ptr_eq(&Arc::downgrade(&editor.handlers.navigation.shared))
            && editor.mode() == self.mode
            && editor.config().lsp.enable
            && self
                .target
                .get(editor)
                .is_some_and(|(view, _)| editor.tree.focus == view.id)
    }
}

fn no_provider(editor: &mut Editor, kind: NavigationKind) {
    if kind == NavigationKind::SelectReferences {
        editor.set_error(|| format!("No configured language server supports {}", kind.feature()));
    } else {
        editor.set_error(|| kind.not_found());
    }
}

fn cancel(editor: &mut Editor) {
    let previous = {
        let mut session = editor.handlers.navigation.shared.session.lock();
        session.pending = None;
        session.target.take()
    };
    if let Some((doc, view)) = previous
        && let Some(doc) = editor.document_mut(doc)
    {
        doc.navigation.remove_view(view);
    }
}

/// Forward a mode transition after applying it to the editor.
pub fn mode_changed(editor: &mut Editor, old: Mode) {
    if editor.mode() != old {
        cancel(editor);
    }
}

/// Request navigation at the focused selection, replacing older navigation work.
pub fn request(editor: &mut Editor, kind: NavigationKind) {
    cancel(editor);
    let shared = editor.handlers.navigation.shared.clone();
    if !editor.config().lsp.enable {
        no_provider(editor, kind);
        return;
    }
    let include_declaration = editor.config().lsp.references.include_declaration;
    let mode = editor.mode();
    let (view, doc) = crate::current!(editor);
    let mut servers = HashSet::new();
    let limit = if kind == NavigationKind::SelectReferences {
        1
    } else {
        usize::MAX
    };
    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(kind.feature())
        .filter(|server| servers.insert(server.id()))
        .take(limit)
        .filter_map(|server| {
            let encoding = server.offset_encoding();
            let future = kind.request(server, doc, view.id, include_declaration)?;
            Some(async move { (encoding, future.await) })
        })
        .collect();
    if futures.is_empty() {
        no_provider(editor, kind);
        return;
    }
    let cancel = doc.navigation.restart(view.id);
    shared.session.lock().target = Some((doc.id(), view.id));
    let request = Request {
        owner: Arc::downgrade(&shared),
        target: SelectionRequest::new(doc, view.id, cancel, servers.into_iter().collect()),
        kind,
        mode,
    };
    let callbacks = shared.callbacks.clone();
    tokio::spawn(async move {
        let mut locations = Vec::new();
        let mut highlights = None;
        loop {
            match cancelable_future(futures.next(), request.target.cancel_handle()).await {
                Some(Some((encoding, Ok(Response::Locations(response))))) => locations.extend(
                    response
                        .into_iter()
                        .filter_map(|location| Location::from_lsp(location, encoding)),
                ),
                Some(Some((encoding, Ok(Response::Highlights(response))))) => {
                    highlights = Some((encoding, response))
                }
                Some(Some((_, Err(error)))) => log::error!("navigation request failed: {error}"),
                Some(None) => break,
                None => return,
            }
        }
        callbacks
            .send(move |editor| {
                if !request.is_current(editor) {
                    return;
                }
                if request.kind == NavigationKind::SelectReferences {
                    if let Some((encoding, highlights)) = highlights {
                        select_highlights(editor, &request, encoding, highlights);
                    }
                } else {
                    editor.handlers.navigation.shared.session.lock().pending =
                        Some(NavigationUpdate { request, locations });
                }
                editor.redraw().request();
            })
            .await;
    });
}

fn select_highlights(
    editor: &mut Editor,
    request: &Request,
    encoding: OffsetEncoding,
    highlights: Vec<lsp::DocumentHighlight>,
) {
    let doc = editor.document_mut(request.target.document_id()).unwrap();
    let cursor = doc
        .selection(request.target.view_id())
        .primary()
        .cursor(doc.text().slice(..));
    let ranges: Vec<_> = highlights
        .into_iter()
        .filter_map(|highlight| {
            lsp_client::util::lsp_range_to_range(doc.text(), highlight.range, encoding)
        })
        .collect();
    if ranges.is_empty() {
        return;
    }
    let primary = ranges
        .iter()
        .rposition(|range| range.contains(cursor))
        .unwrap_or(0);
    doc.set_selection(
        request.target.view_id(),
        Selection::new(ranges.into(), primary),
    );
}

/// Resolve at presentation time so queued results cannot navigate a newer editor state.
pub struct NavigationUpdate {
    request: Request,
    locations: Vec<Location>,
}

impl std::fmt::Debug for NavigationUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavigationUpdate").finish_non_exhaustive()
    }
}

impl NavigationUpdate {
    pub fn resolve(self, editor: &mut Editor) -> Option<Vec<Location>> {
        if !self.request.is_current(editor) {
            return None;
        }
        if self.locations.is_empty() {
            editor.set_error(|| self.request.kind.not_found());
            return None;
        }
        Some(self.locations)
    }
}

pub fn next_update(editor: &Editor) -> Option<NavigationUpdate> {
    editor
        .handlers
        .navigation
        .shared
        .session
        .lock()
        .pending
        .take()
}

pub(crate) fn register_hooks() {
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.navigation.remove_view(event.view);
        Ok(())
    });
    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) {
            doc.navigation.clear();
        }
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !event.new.lsp.enable {
            for doc in event.editor.documents_mut() {
                doc.navigation.clear();
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) {
                doc.navigation.clear();
            }
        }
        Ok(())
    });
}

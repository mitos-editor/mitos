//! Manual language navigation and selection, independent of picker presentation.

mod open;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
};

use editor_core::{Rope, Selection, Uri};
use event::{cancelable_future, register_hook, TaskController, TaskHandle};
use futures_util::{
    future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt,
};
use language_config::LanguageServerFeature;
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub uri: Uri,
    pub range: lsp::Range,
    pub offset_encoding: OffsetEncoding,
}

/// Removes duplicate locations, keeping the rest in their original order.
///
/// When multiple language servers respond to the same request with the same location, the
/// duplicates may still differ in the servers' offset encodings. Locations which share an
/// offset encoding are compared directly. Comparing `character` offsets across different
/// offset encodings however requires the text they point into, so those are only compared
/// when `text_for_uri` provides the text - i.e. when the document is already open. Files
/// are never read from disk just to deduplicate: a goto request can easily return hundreds
/// of locations and showing the occasional duplicate is cheaper than reading all of them.
fn deduplicate_locations(
    locations: &mut Vec<Location>,
    mut text_for_uri: impl FnMut(&Uri) -> Option<Rope>,
) {
    // Exact duplicates.
    let mut seen = HashSet::new();
    locations.retain(|location| seen.insert(location.clone()));

    // Only locations which point to the same lines of the same file with different offset
    // encodings can still be duplicates of each other.
    let mut groups: HashMap<(&Uri, u32, u32), Vec<usize>> = HashMap::new();
    for (index, location) in locations.iter().enumerate() {
        groups
            .entry((
                &location.uri,
                location.range.start.line,
                location.range.end.line,
            ))
            .or_default()
            .push(index);
    }

    let mut duplicates = HashSet::new();
    for ((uri, _, _), group) in groups {
        let offset_encoding = locations[group[0]].offset_encoding;
        if group[1..]
            .iter()
            .all(|&index| locations[index].offset_encoding == offset_encoding)
        {
            continue;
        }
        let Some(text) = text_for_uri(uri) else {
            continue;
        };
        let mut ranges = HashSet::new();
        for &index in &group {
            let location = &locations[index];
            let Some(range) = lsp_client::util::lsp_range_to_range(
                &text,
                location.range,
                location.offset_encoding,
            ) else {
                continue;
            };
            if !ranges.insert((range.anchor, range.head)) {
                duplicates.insert(index);
            }
        }
    }

    if !duplicates.is_empty() {
        let mut index = 0;
        locations.retain(|_| {
            let keep = !duplicates.contains(&index);
            index += 1;
            keep
        });
    }
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
    // A location requested while this document has no displayed view.
    pending_location: Option<Selection>,
}

impl NavigationState {
    pub(crate) fn take_pending_location(&mut self) -> Option<Selection> {
        self.pending_location.take()
    }

    pub(crate) fn map_pending_location(
        &mut self,
        changes: &editor_core::ChangeSet,
        text: editor_core::RopeSlice<'_>,
    ) {
        if let Some(selection) = self.pending_location.take() {
            self.pending_location = Some(selection.map(changes).ensure_invariants(text));
        }
    }

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
                .is_some_and(|(view, _)| editor.tree.focus() == view.id)
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
pub(crate) fn mode_changed(editor: &mut Editor, old: Mode) {
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
    pub fn kind(&self) -> NavigationKind {
        self.request.kind
    }

    pub fn resolve(mut self, editor: &mut Editor) -> Option<Vec<Location>> {
        if !self.request.is_current(editor) {
            return None;
        }
        if self.locations.is_empty() {
            editor.set_error(|| self.request.kind.not_found());
            return None;
        }
        deduplicate_locations(&mut self.locations, |uri| {
            let doc = editor.document_by_path(uri.as_path()?)?;
            Some(doc.text().clone())
        });
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

#[cfg(test)]
mod deduplication_tests {
    use super::*;
    use std::path::PathBuf;

    fn location(
        path: &str,
        offset_encoding: OffsetEncoding,
        (start_line, start_character): (u32, u32),
        (end_line, end_character): (u32, u32),
    ) -> Location {
        Location {
            uri: PathBuf::from(path).into(),
            range: lsp::Range {
                start: lsp::Position {
                    line: start_line,
                    character: start_character,
                },
                end: lsp::Position {
                    line: end_line,
                    character: end_character,
                },
            },
            offset_encoding,
        }
    }

    #[test]
    fn removes_exact_duplicates_without_reading_text() {
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf16, (0, 4), (0, 10)),
            location("/bar.rs", OffsetEncoding::Utf16, (1, 0), (1, 5)),
            location("/foo.rs", OffsetEncoding::Utf16, (0, 4), (0, 10)),
        ];
        deduplicate_locations(&mut locations, |_| panic!("must not look at file contents"));
        assert_eq!(
            locations,
            vec![
                location("/foo.rs", OffsetEncoding::Utf16, (0, 4), (0, 10)),
                location("/bar.rs", OffsetEncoding::Utf16, (1, 0), (1, 5)),
            ]
        );
    }

    #[test]
    fn does_not_read_text_for_locations_on_different_lines() {
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf8, (0, 4), (0, 10)),
            location("/foo.rs", OffsetEncoding::Utf16, (1, 4), (1, 10)),
        ];
        deduplicate_locations(&mut locations, |_| panic!("must not look at file contents"));
        assert_eq!(locations.len(), 2);
    }

    #[test]
    fn does_not_read_text_when_offset_encodings_match() {
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf16, (0, 4), (0, 10)),
            location("/foo.rs", OffsetEncoding::Utf16, (0, 12), (0, 18)),
        ];
        deduplicate_locations(&mut locations, |_| panic!("must not look at file contents"));
        assert_eq!(locations.len(), 2);
    }

    #[test]
    fn compares_locations_across_offset_encodings_by_text_of_open_document() {
        // 'é' is two UTF-8 code units and one UTF-16 code unit, so both ranges
        // point at "foo".
        let text = Rope::from("é foo\n");
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf8, (0, 3), (0, 6)),
            location("/foo.rs", OffsetEncoding::Utf16, (0, 2), (0, 5)),
        ];
        deduplicate_locations(&mut locations, |_| Some(text.clone()));
        assert_eq!(
            locations,
            vec![location("/foo.rs", OffsetEncoding::Utf8, (0, 3), (0, 6))]
        );
    }

    #[test]
    fn does_not_treat_equal_ranges_in_different_offset_encodings_as_duplicates() {
        // Because of the multi-byte 'é' the same character offset resolves to
        // different positions in different offset encodings.
        let text = Rope::from("é foo\n");
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf8, (0, 3), (0, 3)),
            location("/foo.rs", OffsetEncoding::Utf16, (0, 3), (0, 3)),
        ];
        deduplicate_locations(&mut locations, |_| Some(text.clone()));
        assert_eq!(locations.len(), 2);
    }

    #[test]
    fn keeps_locations_in_documents_which_are_not_open() {
        // Different offset encodings can only be compared through the text,
        // which is not read from disk. Duplicates are acceptable here.
        let mut locations = vec![
            location("/foo.rs", OffsetEncoding::Utf8, (0, 3), (0, 6)),
            location("/foo.rs", OffsetEncoding::Utf16, (0, 3), (0, 6)),
        ];
        deduplicate_locations(&mut locations, |_| None);
        assert_eq!(locations.len(), 2);
    }
}

//! View-bound runtime labels for the selected paused frame.

mod fallback;
mod labels;

use std::{ops::Range, sync::Arc};

use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId};

use super::{
    requests::RequestKind, DebugSessionStatus, DebuggerContext, DebuggerSource, DebuggerTask,
};
use crate::{features::request::ViewRequest, Document, Editor, View, ViewId};

const MAX_SOURCE_LENGTH: usize = 500;

/// One bounded, single-line label, sorted by document line.
#[derive(Debug, PartialEq, Eq)]
pub struct DebuggerInlineValue {
    pub line: usize,
    pub text: String,
}

pub(super) struct State {
    target: Arc<Target>,
    values: Vec<DebuggerInlineValue>,
}

struct Target {
    context: DebuggerContext,
    view: ViewRequest,
    source: DebuggerSource,
    lines: Range<usize>,
    provider: Option<LanguageServerId>,
    language: Option<String>,
    has_syntax: bool,
    can_evaluate: bool,
}

fn provider(editor: &Editor, doc: &Document) -> Option<LanguageServerId> {
    if !editor.config().lsp.enable {
        return None;
    }
    doc.language_servers_with_feature(LanguageServerFeature::InlineValues)
        .next()
        .map(|server| server.id())
}

fn can_evaluate(editor: &Editor) -> bool {
    editor
        .debugger
        .get_active_session()
        .and_then(|session| session.capabilities())
        .is_some_and(|capabilities| capabilities.supports_evaluate_for_hovers == Some(true))
}

fn viewport(view: &View, doc: &Document, stopped_line: usize) -> Range<usize> {
    let text = doc.text();
    let first = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
    first
        ..first
            .saturating_add(view.inner_height(doc))
            .min(text.len_lines())
            .min(stopped_line.saturating_add(1))
}

impl Target {
    fn is_current(&self, editor: &Editor) -> bool {
        editor.config().debugger.inline_values
            && self.context.is_current(editor)
            && can_evaluate(editor) == self.can_evaluate
            && self.source.is_current(editor)
            && self.view.get(editor).is_some_and(|(view, doc)| {
                !doc.is_modified()
                    && doc.syntax().is_some() == self.has_syntax
                    && doc
                        .language_config()
                        .map(|language| language.language_id.as_str())
                        == self.language.as_deref()
                    && provider(editor, doc) == self.provider
                    && editor.debugger_frame_in_document(doc).is_some_and(|frame| {
                        viewport(view, doc, frame.line.saturating_sub(1)) == self.lines
                    })
            })
    }
}

impl Editor {
    /// Honor an LSP refresh without invalidating unrelated servers or debugger values.
    pub fn refresh_debugger_inline_values_for_server(&mut self, server: LanguageServerId) {
        let views: Vec<_> = self
            .debugger
            .inline_values
            .iter()
            .filter(|(_, state)| state.target.provider == Some(server))
            .map(|(&view, _)| view)
            .collect();
        for view in views {
            if let Some(state) = self.debugger.inline_values.remove(&view)
                && let Some(session) = self.debugger.sessions.get_mut(state.target.context.id)
            {
                session.requests.remove(RequestKind::InlineValues(view));
            }
        }
        self.refresh_debugger_inline_values();
    }

    /// Current labels only; queued callbacks and stale source snapshots cannot leak into rendering.
    pub fn debugger_inline_values(&self, view: ViewId) -> &[DebuggerInlineValue] {
        self.debugger
            .inline_values
            .get(&view)
            .filter(|state| state.target.is_current(self))
            .map_or(&[], |state| state.values.as_slice())
    }

    /// Refresh visible source views, deduplicating pending and completed requests.
    pub fn refresh_debugger_inline_values(&mut self) {
        let mut states = std::mem::take(&mut self.debugger.inline_values);
        states.retain(|&view, state| {
            if state.target.is_current(self) {
                return true;
            }
            if let Some(session) = self.debugger.sessions.get_mut(state.target.context.id) {
                session.requests.remove(RequestKind::InlineValues(view));
            }
            false
        });
        self.debugger.inline_values = states;
        if !self.config().debugger.inline_values
            || !self
                .debugger
                .get_active_session()
                .is_some_and(|session| session.status() == DebugSessionStatus::Stopped)
        {
            return;
        }
        let views: Vec<_> = self.tree.views().map(|(view, _)| view.id).collect();
        for view in views {
            if !self.debugger.inline_values.contains_key(&view) {
                self.start_debugger_inline_values(view);
            }
        }
    }

    fn start_debugger_inline_values(&mut self, view_id: ViewId) {
        let view = self.tree.get(view_id);
        let doc_id = view.doc;
        let doc = &self.documents[&view.doc];
        if doc.is_modified() {
            return;
        }
        let Some(frame) = self.debugger_frame_in_document(doc) else {
            return;
        };
        let Some(position) = super::source::position_to_char(doc.text(), frame.line, frame.column)
        else {
            return;
        };
        let lines = viewport(view, doc, frame.line - 1);
        if lines.is_empty() {
            return;
        }
        let Some(source) = self.debugger_source(doc.id()) else {
            return;
        };
        let frame = frame.clone();
        let text = doc.text().clone();
        let language = doc
            .language_config()
            .map(|language| language.language_id.clone());
        let has_syntax = doc.syntax().is_some();
        let syntax = doc.syntax().cloned();
        let loader = self.syn_loader.load_full();
        let provider = provider(self, doc);
        let lsp_request = provider.and_then(|id| {
            let server = self.language_server_by_id(id)?;
            let encoding = server.offset_encoding();
            let frame_id = i32::try_from(frame.id).ok()?;
            let stopped = super::source::frame_selection(&text, &frame)?.primary();
            let stopped_location = lsp_client::util::range_to_lsp_range(&text, stopped, encoding);
            let range = lsp_client::util::range_to_lsp_range(
                &text,
                editor_core::Range::new(
                    text.line_to_char(lines.start),
                    text.line_to_char(lines.end),
                ),
                encoding,
            );
            Some((
                server.inline_values(lsp::InlineValueParams {
                    text_document: doc.identifier(),
                    range,
                    context: lsp::InlineValueContext {
                        frame_id,
                        stopped_location,
                    },
                    work_done_progress_params: Default::default(),
                }),
                encoding,
            ))
        });
        let can_evaluate = can_evaluate(self);
        let Ok((mut context, sender)) = self.debugger_context(RequestKind::InlineValues(view_id))
        else {
            return;
        };
        let cache = self.debugger_inspection_cache(context.id, sender.clone());
        let data = cache.current();
        context.lifetimes.push(data.clone());
        let target = Arc::new(Target {
            view: ViewRequest::new(
                &self.documents[&doc_id],
                view_id,
                context.request.clone(),
                provider.into_iter().collect(),
            ),
            context: context.clone(),
            source,
            lines: lines.clone(),
            provider,
            language,
            has_syntax,
            can_evaluate,
        });
        let request_target = target.clone();
        let task = DebuggerTask::new(context.clone(), async move {
            let candidates = if let Some((request, encoding)) = lsp_request {
                match request.await {
                    Ok(values) => Some(labels::from_lsp(
                        &text,
                        encoding,
                        lines.clone(),
                        values.unwrap_or_default(),
                    )),
                    Err(error) => {
                        log::debug!("Inline values request failed: {error}");
                        None
                    }
                }
            } else {
                None
            };
            let candidates = candidates.unwrap_or_else(|| {
                syntax.as_ref().map_or_else(Vec::new, |syntax| {
                    fallback::candidates(&text, syntax, &loader, position, lines)
                })
            });
            let locals = if candidates.iter().any(labels::Candidate::needs_locals) {
                match cache.inline_locals(&frame, data).await {
                    Ok(locals) => locals,
                    Err(error) => {
                        log::debug!("Inline variable lookup failed: {error}");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            Ok(labels::resolve(candidates, &locals, sender, context.frame, can_evaluate).await)
        });
        self.debugger.inline_values.insert(
            view_id,
            State {
                target,
                values: Vec::new(),
            },
        );
        let callbacks = self.callback_sender().clone();
        tokio::spawn(async move {
            let Some(update) = task.run().await else {
                return;
            };
            callbacks
                .send(move |editor| {
                    if !request_target.is_current(editor) {
                        return;
                    }
                    if let Some(Ok(values)) = update.into_result(editor)
                        && let Some(state) = editor.debugger.inline_values.get_mut(&view_id)
                    {
                        state.values = values;
                    }
                })
                .await;
        });
    }
}

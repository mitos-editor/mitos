//! DAP source locations use one-based lines and UTF-16 columns.

mod identity;
pub use identity::DebuggerSource;

use dap::StackFrame;
use editor_core::{line_ending::get_line_ending, Rope, Selection};

/// Convert a DAP source line to an editor line, rejecting the unknown line zero.
pub fn line_index(line: usize) -> Option<usize> {
    line.checked_sub(1)
}

/// Convert a DAP position without allowing its column to cross a line ending.
/// Unknown columns (zero) use the line start; oversized columns use the line end.
/// A column inside a surrogate pair resolves to that character's start.
pub fn position_to_char(text: &Rope, line: usize, column: usize) -> Option<usize> {
    let line = line_index(line)?;
    let content = text.get_line(line)?;
    let end =
        content.len_chars() - get_line_ending(&content).map_or(0, |ending| ending.len_chars());
    let content = content.slice(..end);
    let column = column.saturating_sub(1).min(content.len_utf16_cu());
    Some(text.line_to_char(line) + content.utf16_cu_to_char(column))
}

/// A stack frame's inclusive line range for source previews and gutter markers.
pub fn frame_line_range(frame: &StackFrame) -> Option<(usize, usize)> {
    let start = line_index(frame.line)?;
    let end = frame
        .end_line
        .and_then(line_index)
        .unwrap_or(start)
        .max(start);
    Some((start, end))
}

pub(crate) fn frame_selection(text: &Rope, frame: &StackFrame) -> Option<Selection> {
    let start = position_to_char(text, frame.line, frame.column)?;
    let end = if frame.end_line.is_some() || frame.end_column.is_some() {
        position_to_char(
            text,
            frame.end_line.unwrap_or(frame.line),
            frame.end_column.unwrap_or(0),
        )
        .filter(|end| *end >= start)
        .unwrap_or(start)
    } else {
        start
    };
    Some(Selection::single(start, end))
}

/// A local preview is valid only when the adapter does not require source resolution.
pub fn local_path(frame: &StackFrame) -> Option<&std::path::Path> {
    let source = frame.source.as_ref()?;
    if source
        .source_reference
        .is_some_and(|reference| reference > 0)
    {
        return None;
    }
    source.path.as_deref()
}

#[derive(Default)]
pub(super) struct SessionSources {
    entries: std::collections::HashMap<usize, CachedSource>,
}

struct CachedSource {
    id: ::debugger::SourceId,
    descriptor: dap::Source,
    document: Option<crate::DocumentId>,
    content: std::sync::Arc<tokio::sync::OnceCell<dap::requests::SourceResponse>>,
    _owner: event::TaskController,
    current: event::TaskHandle,
}
impl CachedSource {
    fn new(id: ::debugger::SourceId, descriptor: dap::Source) -> Self {
        let mut owner = event::TaskController::new();
        let current = owner.restart();
        Self {
            id,
            descriptor,
            document: None,
            content: Default::default(),
            _owner: owner,
            current,
        }
    }
}

impl super::DebuggerState {
    /// Intern a source before opening its content or accepting adapter-created markers.
    pub(super) fn source_key(
        &mut self,
        id: dap::DebugAdapterId,
        source: &dap::Source,
    ) -> Option<::debugger::BreakpointSource> {
        if let Some(reference) = source.source_reference.filter(|reference| *reference > 0) {
            if !self.sessions[id].sources.entries.contains_key(&reference) {
                let identity = self.source_ids.allocate();
                self.sessions[id]
                    .sources
                    .entries
                    .insert(reference, CachedSource::new(identity, source.clone()));
            }
            Some(::debugger::BreakpointSource::Adapter(
                self.sessions[id].sources.entries[&reference].id,
            ))
        } else {
            source.path.clone().map(::debugger::BreakpointSource::File)
        }
    }

    pub(super) fn adapter_source_descriptor(
        &self,
        id: dap::DebugAdapterId,
        source: ::debugger::SourceId,
    ) -> Option<dap::Source> {
        self.get_session(id)?
            .sources
            .entries
            .values()
            .find(|cached| cached.id == source)
            .map(|cached| cached.descriptor.clone())
    }

    fn invalidate_source(&mut self, id: dap::DebugAdapterId, reference: usize) {
        let Some(session) = self.sessions.get_mut(id) else {
            return;
        };
        if let Some(source) = session.sources.entries.remove(&reference) {
            let key = ::debugger::BreakpointSource::Adapter(source.id);
            if let Some(state) = session.breakpoints.as_mut() {
                state.remove_source(&key);
            }
            self.breakpoints.remove_source(&key);
        }
    }

    pub(super) fn clear_sources(&mut self, id: dap::DebugAdapterId) {
        let references: Vec<_> = self
            .get_session(id)
            .map(|session| session.sources.entries.keys().copied().collect())
            .unwrap_or_default();
        for reference in references {
            self.invalidate_source(id, reference);
        }
    }
}

impl crate::Editor {
    /// The paused location belongs to the resolved document, never to a coincidental path.
    pub fn debugger_frame_in_document(&self, doc: &crate::Document) -> Option<&StackFrame> {
        let session = self.debugger.get_active_session()?;
        let frame = session.current_stack_frame()?;
        let source = self.debugger_source(doc.id())?;
        source.matches_frame(session.id(), frame).then_some(frame)
    }

    pub(crate) fn navigate_debugger_frame(&mut self, id: dap::DebugAdapterId, frame: &StackFrame) {
        if !self
            .debugger
            .get_active_session()
            .is_some_and(|session| session.id() == id)
            || self.debugger.is_terminating(id)
            || line_index(frame.line).is_none()
        {
            return;
        }
        let Some(source) = frame.source.as_ref() else {
            return;
        };
        let Some(reference) = source.source_reference.filter(|reference| *reference > 0) else {
            crate::handlers::dap::jump_to_stack_frame(self, frame);
            return;
        };
        self.debugger.source_key(id, source);
        let cached = self.debugger.sessions[id]
            .sources
            .entries
            .get(&reference)
            .and_then(|cached| cached.document);
        if let Some(document) = cached.filter(|document| self.document(*document).is_some()) {
            self.show_debugger_source(document, frame);
            return;
        }
        let Ok((mut context, sender)) =
            self.debugger_context_for(id, super::requests::RequestKind::Source)
        else {
            return;
        };
        let Some(view) = self.tree.try_get(self.tree.focus) else {
            return;
        };
        let Some(doc) = self.document(view.doc) else {
            return;
        };
        let navigation = crate::features::request::SelectionRequest::new(
            doc,
            view.id,
            context.request.clone(),
            Vec::new(),
        );
        let mode = self.mode();
        let cached = self.debugger.sessions[id]
            .sources
            .entries
            .get_mut(&reference)
            .unwrap();
        context.lifetimes.push(cached.current.clone());
        let content = cached.content.clone();
        let source = cached.descriptor.clone();
        let frame = frame.clone();
        let name = source
            .name
            .clone()
            .or_else(|| {
                source
                    .path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| format!("source {reference}"));
        let task = super::DebuggerTask::new(context, async move {
            content
                .get_or_try_init(|| {
                    sender.request::<dap::requests::Source>(dap::requests::SourceArguments {
                        source: Some(source),
                        source_reference: reference,
                    })
                })
                .await
                .cloned()
        });
        self.spawn_debugger_task_if(
            task,
            move |editor| {
                editor.mode() == mode
                    && navigation
                        .get(editor)
                        .is_some_and(|(view, _)| view.id == editor.tree.focus)
            },
            move |editor, response| {
                let session = &editor.debugger.sessions[id];
                let title = format!("[debug #{}: {name}]", session.number());
                let mut doc = crate::Document::from(
                    Rope::from_str(&response.content),
                    None,
                    editor.config.clone(),
                    editor.syn_loader.clone(),
                )
                .with_readonly_name(title);
                let loader = editor.syn_loader.load_full();
                if let Some(language) = loader.language_for_filename(std::path::Path::new(&name)) {
                    doc.set_language(Some(loader.language(language).config().clone()), &loader);
                }
                let document = editor.new_file_from_document(crate::editor::Action::Replace, doc);
                editor.debugger.sessions[id]
                    .sources
                    .entries
                    .get_mut(&reference)
                    .unwrap()
                    .document = Some(document);
                editor.show_debugger_source(document, &frame);
            },
        );
    }

    fn show_debugger_source(&mut self, document: crate::DocumentId, frame: &StackFrame) {
        self.switch(document, crate::editor::Action::Replace);
        let (view, doc) = current!(self);
        if let Some(selection) = frame_selection(doc.text(), frame) {
            doc.set_selection(view.id, selection);
            crate::align_view(doc, view, crate::Align::Center);
        }
    }

    pub(crate) fn debugger_source_changed(
        &mut self,
        id: dap::DebugAdapterId,
        event: dap::events::LoadedSourceBody,
    ) {
        if matches!(event.reason.as_str(), "changed" | "removed")
            && let Some(reference) = event.source.source_reference
        {
            self.debugger.invalidate_source(id, reference);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_follow_utf16_and_remain_inside_lf_and_crlf_source_lines() {
        for ending in ["\n", "\r\n"] {
            let text = Rope::from_str(&format!("a😀b{ending}next"));
            for (column, character) in [
                (0, 0),
                (1, 0),
                (2, 1),
                (3, 1),
                (4, 2),
                (5, 3),
                (usize::MAX, 3),
            ] {
                assert_eq!(position_to_char(&text, 1, column), Some(character));
            }
            assert_eq!(
                position_to_char(&text, 2, 1),
                Some(3 + ending.chars().count())
            );
            for line in [0, 3, usize::MAX] {
                assert_eq!(position_to_char(&text, line, 1), None);
            }
        }
        assert_eq!(position_to_char(&Rope::new(), 1, usize::MAX), Some(0));
    }

    #[test]
    fn frame_ranges_support_same_line_end_columns_and_ignore_reversed_ends() {
        let text = Rope::from_str("a😀b");
        let mut frame: StackFrame = serde_json::from_value(
            serde_json::json!({"id":1,"name":"main","line":1,"column":2,"endColumn":4}),
        )
        .unwrap();
        assert_eq!(
            frame_selection(&text, &frame),
            Some(Selection::single(1, 2))
        );
        frame.end_column = Some(1);
        assert_eq!(frame_selection(&text, &frame), Some(Selection::point(1)));
        frame.line = 0;
        assert!(frame_selection(&text, &frame).is_none());
        assert!(frame_line_range(&frame).is_none());
    }
}

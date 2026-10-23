//! Diagnostic navigation shared by frontends.
use crate::Editor;
use diagnostics::Severity;
use editor_core::{movement::Direction, Selection};

impl Editor {
    /// Select an error diagnostic in document order. Boundary jumps select the first/last
    /// error; relative jumps keep the current selection when no further error exists.
    pub fn goto_error(&mut self, direction: Direction, boundary: bool) {
        let (view, doc) = current!(self);
        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        let eligible = |diag: &&diagnostics::Diagnostic| {
            diag.severity == Some(Severity::Error)
                && (boundary
                    || match direction {
                        Direction::Forward => diag.range.start > cursor,
                        Direction::Backward => diag.range.start < cursor,
                    })
        };
        let diag = match direction {
            Direction::Forward => doc.diagnostics().iter().find(eligible),
            Direction::Backward => doc.diagnostics().iter().rfind(eligible),
        };
        let Some(diag) = diag else {
            return;
        };
        let selection = if direction == Direction::Backward && !boundary {
            Selection::single(diag.range.end, diag.range.start)
        } else {
            Selection::single(diag.range.start, diag.range.end)
        };
        view.push_jump(doc, (doc.id(), doc.selection(view.id).clone()));
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics::{Diagnostic, DiagnosticProvider, Range};

    #[tokio::test]
    async fn error_motions_skip_other_severities_and_stop_at_document_boundaries() {
        let (mut editor, _store) = crate::editor::tests::editor("#[|a]#bc def ghi jkl\n");
        let id = current_ref!(&editor).1.id();
        let diagnostic = |start, severity| Diagnostic {
            range: Range {
                start,
                end: start + 1,
            },
            ends_at_word: false,
            starts_at_word: false,
            zero_width: false,
            line: 0,
            message: "diagnostic".into(),
            severity,
            code: None,
            provider: DiagnosticProvider::Spelling,
            tags: Vec::new(),
            source: None,
            data: None,
        };
        editor.document_mut(id).unwrap().replace_diagnostics(
            [
                diagnostic(2, Some(Severity::Warning)),
                diagnostic(4, Some(Severity::Error)),
                diagnostic(7, None),
                diagnostic(10, Some(Severity::Error)),
            ],
            &[],
            None,
        );
        for (direction, boundary, from, to) in [
            (Direction::Forward, false, 4, 5),
            (Direction::Forward, false, 10, 11),
            (Direction::Forward, false, 10, 11),
            (Direction::Backward, false, 4, 5),
            (Direction::Backward, true, 10, 11),
            (Direction::Forward, true, 4, 5),
        ] {
            editor.goto_error(direction, boundary);
            let (view, doc) = current_ref!(&editor);
            let range = doc.selection(view.id).primary();
            assert_eq!((range.from(), range.to()), (from, to));
        }
    }
}

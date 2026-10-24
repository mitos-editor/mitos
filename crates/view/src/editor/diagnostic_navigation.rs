//! Diagnostic navigation shared by frontends.
use crate::Editor;
use diagnostics::Severity;
use editor_core::{movement::Direction, Selection};

impl Editor {
    /// Select a diagnostic of exactly the requested severity in document order.
    /// Boundary jumps select the first/last match; relative jumps keep the current
    /// selection when no further match exists. Diagnostics without severity are skipped.
    pub fn goto_diagnostic_severity(
        &mut self,
        direction: Direction,
        boundary: bool,
        severity: Severity,
    ) {
        let (view, doc) = current!(self);
        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        let eligible = |diag: &&diagnostics::Diagnostic| {
            diag.severity == Some(severity)
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

    fn diagnostic(start: usize, severity: Option<Severity>) -> Diagnostic {
        Diagnostic {
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
        }
    }

    #[tokio::test]
    async fn severity_motions_skip_other_severities_and_stop_at_document_boundaries() {
        let severities = [
            Severity::Error,
            Severity::Warning,
            Severity::Info,
            Severity::Hint,
        ];
        for (index, severity) in severities.into_iter().enumerate() {
            let (mut editor, _store) = crate::editor::tests::editor("#[|a]#bc def ghi jkl\n");
            let id = current_ref!(&editor).1.id();
            editor.document_mut(id).unwrap().replace_diagnostics(
                severities
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, severity)| {
                        [
                            diagnostic(index + 1, Some(severity)),
                            diagnostic(index + 7, Some(severity)),
                        ]
                    })
                    .chain([diagnostic(5, None)]),
                &[],
                None,
            );
            let first = index + 1;
            let last = index + 7;
            for (direction, boundary, anchor, head) in [
                (Direction::Forward, false, first, first + 1),
                (Direction::Forward, false, last, last + 1),
                (Direction::Forward, false, last, last + 1),
                (Direction::Backward, false, first + 1, first),
                (Direction::Backward, false, first + 1, first),
                (Direction::Backward, true, last, last + 1),
                (Direction::Forward, true, first, first + 1),
            ] {
                editor.goto_diagnostic_severity(direction, boundary, severity);
                let (view, doc) = current_ref!(&editor);
                let range = doc.selection(view.id).primary();
                assert_eq!(
                    (range.anchor, range.head),
                    (anchor, head),
                    "{severity:?}, {direction:?}, boundary={boundary}"
                );
            }
        }
    }

    #[tokio::test]
    async fn missing_severity_matches_leave_selection_unchanged() {
        let (mut editor, _store) = crate::editor::tests::editor("#[|a]#bc def ghi jkl\n");
        let (view, doc) = current_ref!(&editor);
        let id = doc.id();
        let initial = doc.selection(view.id).clone();
        for severity in [
            Severity::Error,
            Severity::Warning,
            Severity::Info,
            Severity::Hint,
        ] {
            for diagnostics in [
                vec![],
                vec![diagnostic(2, None)],
                vec![diagnostic(
                    4,
                    Some(if severity == Severity::Error {
                        Severity::Warning
                    } else {
                        Severity::Error
                    }),
                )],
            ] {
                editor
                    .document_mut(id)
                    .unwrap()
                    .replace_diagnostics(diagnostics, &[], None);
                for direction in [Direction::Forward, Direction::Backward] {
                    for boundary in [false, true] {
                        editor.goto_diagnostic_severity(direction, boundary, severity);
                        let (view, doc) = current_ref!(&editor);
                        assert_eq!(doc.selection(view.id), &initial);
                    }
                }
            }
        }
    }
}

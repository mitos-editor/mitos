//! Spelling navigation, finding ranges, and language commands.

use crate::commands::{context::Context, navigation::push_jump};
use editor_core::{diagnostic::DiagnosticProvider, movement::Direction, Range};
use view::{document::Mode, Document};

pub(super) fn spelling_ranges(doc: &Document) -> impl DoubleEndedIterator<Item = Range> + '_ {
    doc.diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.provider == DiagnosticProvider::Spelling)
        .map(|diagnostic| Range::new(diagnostic.range.start, diagnostic.range.end))
}

pub(super) fn goto_next_spelling(cx: &mut Context) {
    goto_spelling(cx, Direction::Forward);
}

pub(super) fn goto_prev_spelling(cx: &mut Context) {
    goto_spelling(cx, Direction::Backward);
}

fn goto_spelling(cx: &mut Context, direction: Direction) {
    let count = cx.count();
    cx.editor.apply_motion(move |editor| {
        let (view, doc) = current!(editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id).clone().transform(|range| {
            let cursor = range.cursor(text);
            // Exclude the finding under the cursor, including when it is already selected.
            // Taking the last available target also clamps counts at the document's boundaries.
            let target = match direction {
                Direction::Forward => spelling_ranges(doc)
                    .filter(|target| target.from() > cursor)
                    .take(count)
                    .last(),
                Direction::Backward => spelling_ranges(doc)
                    .rev()
                    .filter(|target| target.to() <= cursor)
                    .take(count)
                    .last(),
            };
            let Some(target) = target else {
                return range;
            };
            if editor.mode == Mode::Select {
                let head = if target.to() <= range.anchor {
                    target.from()
                } else {
                    target.to()
                };
                Range::new(range.anchor, head)
            } else {
                target.with_direction(direction)
            }
        });
        if selection == *doc.selection(view.id) {
            return;
        }
        push_jump(view, doc);
        doc.set_selection(view.id, selection);
        view.diagnostics_handler
            .immediately_show_diagnostic(doc, view.id);
    });
}

pub(super) mod typed {
    //! Typable spelling commands.

    use crate::{compositor, ui::PromptEvent};
    use ::command_line::Args;

    #[cold]
    pub(in crate::commands) fn spelling_language(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if args.is_empty() {
            let doc = doc!(cx.editor);
            let status = if doc.spelling_languages.is_empty() {
                "off".to_string()
            } else {
                doc.spelling_languages
                    .iter()
                    .map(|language| language.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            cx.editor.set_status(status);
            return Ok(());
        }

        let languages = if args.len() == 1 && &args[0] == "off" {
            Vec::new()
        } else {
            args.iter()
                .map(|arg| arg.parse())
                .collect::<Result<Vec<editor_core::SpellingLanguage>, _>>()?
        };
        let doc = doc_mut!(cx.editor);
        let doc_id = doc.id();
        doc.spelling_language_override = Some(languages);
        cx.editor.refresh_spelling(doc_id);

        Ok(())
    }
}

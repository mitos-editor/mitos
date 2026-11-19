//! Register text decorations that contribute labels or inline content to a pane.

use stdx::rope::RopeExt as _;
use view::{
    document::{Mode, PresentedDocument},
    view::ViewLayout,
    Editor, View,
};

use crate::ui::text_decorations::{self, DecorationManager, InlineDiagnostics};

pub(super) fn add<'a>(
    editor: &'a Editor,
    config: &'a editor_config::Config,
    doc: PresentedDocument<'a>,
    view: &View,
    layout: &ViewLayout,
    is_focused: bool,
    decorations: &mut DecorationManager<'a>,
) {
    let theme = &editor.theme;
    if is_focused
        && editor.mode() == Mode::Insert
        && let Some(completion) = doc.inline_completion(view.id)
    {
        let line = doc.text().char_to_line_idx(completion.cursor);
        let line_end = editor_core::line_ending::line_end_char_index(&doc.text().slice(..), line);
        decorations.add_decoration(
            text_decorations::inline_completion::InlineCompletionDecoration::new(
                completion,
                theme
                    .try_get_exact("ui.virtual.inline-completion")
                    .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
                layout.format.clone(),
                line_end,
            ),
        );
    }
    let document_config = doc.source().config.load();
    if !doc.diagnostics().is_empty() {
        let enable_cursor_line = view
            .diagnostics_handler
            .show_cursorline_diagnostics(doc, view.id);
        let inline_diagnostic_config = view::annotations::diagnostics::prepare_inline_diagnostics(
            &document_config.diagnostics.inline,
            layout.viewport.width,
            enable_cursor_line,
        );
        let primary_cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        decorations.add_decoration(InlineDiagnostics::new(
            doc,
            theme,
            primary_cursor,
            inline_diagnostic_config,
            document_config.diagnostics.end_of_line.severity,
            document_config.icons,
        ));
    }
    if config.inline_blame.show != editor_config::InlineBlameShow::Never
        && matches!(doc.source().file_blame(), Some(Ok(_)))
    {
        decorations.add_decoration(text_decorations::blame::InlineBlame::new(
            doc,
            view.id,
            &config.inline_blame,
            theme
                .try_get_exact("ui.virtual.inline-blame")
                .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
        ));
    }
    let hidden_line = if is_focused && editor.mode() == Mode::Insert {
        doc.inline_completion(view.id)
            .map(|completion| doc.text().char_to_line_idx(completion.cursor))
    } else {
        None
    };
    let debug_values: &[view::debugger::DebuggerInlineValue] = editor
        .debugger_frame_in_document(doc.source())
        .map_or(&[], |_| editor.debugger_inline_values(view.id));
    if !debug_values.is_empty() {
        decorations.add_decoration(text_decorations::debug_values::DebugValues::new(
            doc.text(),
            debug_values,
            theme
                .try_get_exact("ui.virtual.debug-value")
                .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
            hidden_line,
        ));
    }
    if document_config.lsp.enable
        && document_config.lsp.code_lenses.enable
        && !doc.code_lenses().is_empty()
    {
        decorations.add_decoration(text_decorations::code_lenses::CodeLensDecoration::new(
            doc.text(),
            doc.code_lenses(),
            theme
                .try_get_exact("ui.virtual.code-lens")
                .unwrap_or_else(|| theme.get("ui.virtual.inlay-hint")),
            hidden_line,
        ));
    }
}

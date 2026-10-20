//! Document highlight layers shared by editor panes and picker previews.
//! Inputs describe the document and viewport; no editor component or compositor is required.

use editor_config::CursorShapeConfig;
use editor_core::{
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    highlight::OverlayHighlights,
};
use language::syntax;
use std::ops;
use text_layout::text_annotations::TextAnnotations;
use theme::Theme;
use view::{document::Mode, graphics::CursorKind, Document, View};

fn viewport_byte_range(
    text: editor_core::RopeSlice,
    row: usize,
    height: u16,
) -> std::ops::Range<usize> {
    // Calculate viewport byte ranges:
    // Saturating subs to make it inclusive zero indexing.
    let last_line = text.len_lines().saturating_sub(1);
    let last_visible_line = (row + height as usize).saturating_sub(1).min(last_line);
    let start = text.line_to_byte(row.min(last_line));
    let end = text.line_to_byte(last_visible_line + 1);

    start..end
}

/// Get the syntax highlighter for a document in a view represented by the first line
/// and column (`offset`) and the last line. This is done instead of using a view
/// directly to enable rendering syntax highlighted docs anywhere (eg. picker preview)
pub(crate) fn syntax_highlighter<'editor>(
    doc: &'editor Document,
    anchor: usize,
    height: u16,
    loader: &'editor syntax::Loader,
) -> Option<syntax::Highlighter<'editor>> {
    let syntax = doc.syntax()?;
    let text = doc.text().slice(..);
    let row = text.char_to_line(anchor.min(text.len_chars()));
    let range = viewport_byte_range(text, row, height);
    let range = range.start as u32..range.end as u32;

    let highlighter = syntax.highlighter(text, loader, range);
    Some(highlighter)
}

pub(crate) fn overlay_highlights(
    doc: &Document,
    anchor: usize,
    height: u16,
    text_annotations: &TextAnnotations,
) -> OverlayHighlights {
    let text = doc.text().slice(..);
    let row = text.char_to_line(anchor.min(text.len_chars()));

    let mut range = viewport_byte_range(text, row, height);
    range = text.byte_to_char(range.start)..text.byte_to_char(range.end);

    text_annotations.collect_overlay_highlights(range)
}

pub(crate) fn rainbow_highlights(
    doc: &Document,
    anchor: usize,
    height: u16,
    theme: &Theme,
    loader: &syntax::Loader,
) -> Option<OverlayHighlights> {
    let syntax = doc.syntax()?;
    let text = doc.text().slice(..);
    let row = text.char_to_line(anchor.min(text.len_chars()));
    let visible_range = viewport_byte_range(text, row, height);
    let start = syntax::child_for_byte_range(
        &syntax.tree().root_node(),
        visible_range.start as u32..visible_range.end as u32,
    )
    .map_or(visible_range.start as u32, |node| node.start_byte());
    let range = start..visible_range.end as u32;

    Some(syntax.rainbow_highlights(text, theme.rainbow_length(), loader, range))
}

/// Get highlight spans for document diagnostics
pub(crate) fn diagnostic_highlights(
    doc: &Document,
    theme: &Theme,
    overlay_highlights: &mut Vec<OverlayHighlights>,
) {
    // Skip redundant work if no diagnostics.
    if doc.diagnostics().is_empty() {
        return;
    }

    use ::diagnostics::{DiagnosticProvider, DiagnosticTag, Range, Severity};
    let get_scope_of = |scope| {
        theme
            .find_highlight_exact(scope)
            // get one of the themes below as fallback values
            .or_else(|| theme.find_highlight_exact("diagnostic"))
            .or_else(|| theme.find_highlight_exact("ui.cursor"))
            .or_else(|| theme.find_highlight_exact("ui.selection"))
            .expect(
                "at least one of the following scopes must be defined in the theme: `diagnostic`, `ui.cursor`, or `ui.selection`",
            )
    };

    // Diagnostic tags
    let unnecessary = theme.find_highlight_exact("diagnostic.unnecessary");
    let deprecated = theme.find_highlight_exact("diagnostic.deprecated");
    // Spelling diagnostics get their own scope when the theme defines one.
    let spelling = theme.find_highlight_exact("diagnostic.spelling");

    let mut default_vec = Vec::new();
    let mut info_vec = Vec::new();
    let mut hint_vec = Vec::new();
    let mut warning_vec = Vec::new();
    let mut error_vec = Vec::new();
    let mut unnecessary_vec = Vec::new();
    let mut deprecated_vec = Vec::new();
    let mut spelling_vec = Vec::new();

    let push_diagnostic = |vec: &mut Vec<ops::Range<usize>>, range: Range| {
        // If any diagnostic overlaps ranges with the prior diagnostic,
        // merge the two together. Otherwise push a new span.
        match vec.last_mut() {
            Some(existing_range) if range.start <= existing_range.end => {
                // This branch merges overlapping diagnostics, assuming that the current
                // diagnostic starts on range.start or later. If this assertion fails,
                // we will discard some part of `diagnostic`. This implies that
                // `doc.diagnostics()` is not sorted by `diagnostic.range`.
                debug_assert!(existing_range.start <= range.start);
                existing_range.end = range.end.max(existing_range.end)
            }
            _ => vec.push(range.start..range.end),
        }
    };

    for diagnostic in doc.diagnostics() {
        // Spelling diagnostics use their own scope when the theme defines `diagnostic.spelling`;
        // otherwise they fall through to the severity-based styling (`diagnostic.hint`).
        if spelling.is_some() && matches!(diagnostic.provider, DiagnosticProvider::Spelling) {
            push_diagnostic(&mut spelling_vec, diagnostic.range);
            continue;
        }

        // Separate diagnostics into different Vecs by severity.
        let vec = match diagnostic.severity {
            Some(Severity::Info) => &mut info_vec,
            Some(Severity::Hint) => &mut hint_vec,
            Some(Severity::Warning) => &mut warning_vec,
            Some(Severity::Error) => &mut error_vec,
            _ => &mut default_vec,
        };

        // If the diagnostic has tags and a non-warning/error severity, skip rendering
        // the diagnostic as info/hint/default and only render it as unnecessary/deprecated
        // instead. For warning/error diagnostics, render both the severity highlight and
        // the tag highlight.
        if diagnostic.tags.is_empty()
            || matches!(
                diagnostic.severity,
                Some(Severity::Warning | Severity::Error)
            )
        {
            push_diagnostic(vec, diagnostic.range);
        }

        for tag in &diagnostic.tags {
            match tag {
                DiagnosticTag::Unnecessary => {
                    if unnecessary.is_some() {
                        push_diagnostic(&mut unnecessary_vec, diagnostic.range)
                    }
                }
                DiagnosticTag::Deprecated => {
                    if deprecated.is_some() {
                        push_diagnostic(&mut deprecated_vec, diagnostic.range)
                    }
                }
            }
        }
    }

    overlay_highlights.push(OverlayHighlights::Homogeneous {
        highlight: get_scope_of("diagnostic"),
        ranges: default_vec,
    });
    if let Some(highlight) = unnecessary {
        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight,
            ranges: unnecessary_vec,
        });
    }
    if let Some(highlight) = deprecated {
        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight,
            ranges: deprecated_vec,
        });
    }
    overlay_highlights.extend([
        OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic.info"),
            ranges: info_vec,
        },
        OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic.hint"),
            ranges: hint_vec,
        },
        OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic.warning"),
            ranges: warning_vec,
        },
        OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic.error"),
            ranges: error_vec,
        },
    ]);
    if let Some(highlight) = spelling {
        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight,
            ranges: spelling_vec,
        });
    }
}

pub(crate) fn document_highlights(
    doc: &Document,
    view: &View,
    theme: &Theme,
) -> Option<OverlayHighlights> {
    let ranges = doc.document_highlights(view.id)?;
    if ranges.is_empty() {
        return None;
    }

    let highlight = theme
        .find_highlight_exact("ui.highlight")
        .or_else(|| theme.find_highlight_exact("ui.selection"))
        .or_else(|| theme.find_highlight_exact("ui.cursor"))?;

    Some(OverlayHighlights::Homogeneous {
        highlight,
        ranges: ranges.to_vec(),
    })
}

pub(crate) fn document_link_highlights(doc: &Document, theme: &Theme) -> Option<OverlayHighlights> {
    let highlight = theme
        .find_highlight_exact("markup.link.url")
        .or_else(|| theme.find_highlight_exact("markup.link"))?;

    if doc.document_links().is_empty() {
        return None;
    }

    let mut ranges: Vec<ops::Range<usize>> = Vec::new();
    for link in doc.document_links() {
        if link.start >= link.end {
            continue;
        }

        match ranges.last_mut() {
            Some(existing_range) if link.start <= existing_range.end => {
                existing_range.end = existing_range.end.max(link.end);
            }
            _ => ranges.push(link.start..link.end),
        }
    }

    if ranges.is_empty() {
        return None;
    }

    Some(OverlayHighlights::Homogeneous { highlight, ranges })
}

/// Get highlight spans for selections in a document view.
pub(crate) fn selection_highlights(
    mode: Mode,
    doc: &Document,
    view: &View,
    theme: &Theme,
    cursor_shape_config: &CursorShapeConfig,
    is_terminal_focused: bool,
    is_cursor_owner: bool,
) -> OverlayHighlights {
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);
    let primary_idx = selection.primary_index();

    let cursorkind = mode.cursor_shape(cursor_shape_config);
    let cursor_is_block = cursorkind == CursorKind::Block || !is_cursor_owner;
    let draw_primary = primary_cursor_is_software(cursorkind, is_terminal_focused, is_cursor_owner);

    let selection_scope = theme
        .find_highlight_exact("ui.selection")
        .expect("could not find `ui.selection` scope in the theme!");
    let primary_selection_scope = theme
        .find_highlight_exact("ui.selection.primary")
        .unwrap_or(selection_scope);

    let base_cursor_scope = theme
        .find_highlight_exact("ui.cursor")
        .unwrap_or(selection_scope);
    let base_primary_cursor_scope = theme
        .find_highlight("ui.cursor.primary")
        .unwrap_or(base_cursor_scope);

    let cursor_scope = match mode {
        Mode::Insert => theme.find_highlight_exact("ui.cursor.insert"),
        Mode::Select => theme.find_highlight_exact("ui.cursor.select"),
        Mode::Normal => theme.find_highlight_exact("ui.cursor.normal"),
    }
    .unwrap_or(base_cursor_scope);

    let primary_cursor_scope = match mode {
        Mode::Insert => theme.find_highlight_exact("ui.cursor.primary.insert"),
        Mode::Select => theme.find_highlight_exact("ui.cursor.primary.select"),
        Mode::Normal => theme.find_highlight_exact("ui.cursor.primary.normal"),
    }
    .unwrap_or(base_primary_cursor_scope);

    let mut spans = Vec::new();
    for (i, range) in selection.iter().enumerate() {
        let selection_is_primary = i == primary_idx;
        let (cursor_scope, selection_scope) = if selection_is_primary {
            (primary_cursor_scope, primary_selection_scope)
        } else {
            (cursor_scope, selection_scope)
        };

        // Special-case: cursor at end of the rope.
        if range.head == range.anchor && range.head == text.len_chars() {
            if !selection_is_primary || draw_primary {
                // Inactive editor cursors remain visible as software blocks
                // while a prompt or picker owns the terminal cursor.
                spans.push((cursor_scope, range.head..range.head + 1));
            }
            continue;
        }

        let range = range.min_width_1(text);
        if range.head > range.anchor {
            // Standard case.
            let cursor_start = prev_grapheme_boundary(text, range.head);
            // non block cursors look like they exclude the cursor
            let selection_end = if selection_is_primary && !cursor_is_block && mode != Mode::Insert
            {
                range.head
            } else {
                cursor_start
            };
            spans.push((selection_scope, range.anchor..selection_end));
            // Secondary cursors and inactive editor markers are software blocks.
            if !selection_is_primary || draw_primary {
                spans.push((cursor_scope, cursor_start..range.head));
            }
        } else {
            // Reverse case.
            let cursor_end = next_grapheme_boundary(text, range.head);
            // Secondary cursors and inactive editor markers are software blocks.
            if !selection_is_primary || draw_primary {
                spans.push((cursor_scope, range.head..cursor_end));
            }
            // non block cursors look like they exclude the cursor
            let selection_start = if selection_is_primary
                && !cursor_is_block
                && !(mode == Mode::Insert && cursor_end == range.anchor)
            {
                range.head
            } else {
                cursor_end
            };
            spans.push((selection_scope, selection_start..range.anchor));
        }
    }

    OverlayHighlights::Heterogenous { highlights: spans }
}

/// Render brace match, etc (meant for the focused view only)
pub(crate) fn matching_bracket_highlight(
    view: &View,
    doc: &Document,
    theme: &Theme,
) -> Option<OverlayHighlights> {
    // Highlight matching braces
    let syntax = doc.syntax()?;
    let highlight = theme.find_highlight_exact("ui.cursor.match")?;
    let text = doc.text().slice(..);
    let pos = doc.selection(view.id).primary().cursor(text);
    let pos = language::match_brackets::find_matching_bracket(syntax, text, pos)?;
    Some(OverlayHighlights::single(highlight, pos..pos + 1))
}

pub(crate) fn tabstop_highlights(doc: &Document, theme: &Theme) -> Option<OverlayHighlights> {
    let snippet = doc.active_snippet.as_ref()?;
    let highlight = theme.find_highlight_exact("tabstop")?;
    let mut ranges = Vec::new();
    for tabstop in snippet.tabstops() {
        ranges.extend(tabstop.ranges.iter().map(|range| range.start..range.end));
    }
    Some(OverlayHighlights::Homogeneous { highlight, ranges })
}

/// Render bufferline at the top
pub(super) fn primary_cursor_is_software(
    kind: CursorKind,
    terminal_focused: bool,
    is_owner: bool,
) -> bool {
    !is_owner || (kind == CursorKind::Block && terminal_focused)
}

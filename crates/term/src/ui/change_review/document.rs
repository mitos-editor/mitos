//! Review-specific styling over the ordinary editor text renderer. No input or
//! viewport state lives here; document/view coordinates remain authoritative.

use stdx::rope::RopeExt as _;
use view::document::PresentedDocument;

use editor_core::highlight::OverlayHighlights;
use theme::{Style, Theme};
use view::{
    features::change_review::{ReviewLine, ReviewLineKind, ReviewSource},
    graphics::Rect,
    view::ViewLayout,
};

use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};

pub fn line_backgrounds<'a>(doc: PresentedDocument<'a>, theme: &'a Theme) -> impl Decoration + 'a {
    move |renderer: &mut TextRenderer, pos: LinePos| {
        let Some(row) = doc
            .source()
            .review()
            .and_then(|review| review.row(pos.doc_line))
        else {
            return;
        };
        if matches!(row.kind, ReviewLineKind::Added | ReviewLineKind::Removed) {
            let mut style = super::line_style(theme, row.kind);
            style.fg = None;
            renderer.set_style(
                Rect::new(
                    renderer.viewport.x,
                    pos.visual_line,
                    renderer.viewport.width,
                    1,
                ),
                style,
            );
        }
    }
}

/// Highlight visible rows against their original syntax trees. Each side keeps
/// its full multiline/injection context, and only visible source ranges are
/// queried. Resolving scopes at draw time also supports live theme changes.
pub fn syntax_styles(
    doc: PresentedDocument<'_>,
    layout: &ViewLayout,
    theme: &Theme,
) -> Option<Vec<(usize, Style)>> {
    let review = doc.source().review()?;
    let source = review.source()?;
    let text = doc.text();
    let mut styles = Vec::new();
    for index in visible_lines(layout) {
        let start = text.line_to_char_idx(index);
        styles.push((start, theme.get("ui.text")));
        let Some(row) = review.row(index) else {
            continue;
        };
        if let Some(row_styles) = row_syntax_styles(source, row, theme) {
            styles.extend(
                row_styles
                    .into_iter()
                    .map(|(position, style)| (start + position, style)),
            );
        }
    }
    Some(styles)
}

/// Character offsets relative to one projected row, shared by native documents
/// and compact previews. Query the complete original side, never the diff text.
pub(super) fn row_syntax_styles(
    source: &ReviewSource,
    row: &ReviewLine,
    theme: &Theme,
) -> Option<Vec<(usize, Style)>> {
    let (original, syntax, line) = source.syntax_for_row(row)?;
    let loader = source.syntax_loader();
    // Captured reviews can outlive a language-loader or theme reload.
    if loader.scopes().as_slice() != theme.scopes() {
        loader.set_scopes(theme.scopes().to_vec());
    }
    let byte = original.line_to_byte_idx(line, stdx::rope::LINE_TYPE);
    let end = byte + row.text.len();
    let char_start = original.byte_to_char_idx(byte);
    let char_end = original.byte_to_char_idx(end);
    let highlighter = syntax.highlighter(original.slice(..), loader, byte as u32..end as u32);
    let styles: Vec<_> =
        super::super::highlights::syntax_styles(original.slice(..), Some(highlighter), theme)
            .take_while(|(position, _)| *position <= char_end)
            .map(|(position, style)| (position.saturating_sub(char_start), style))
            .collect();
    let Some(changes) = &row.changes else {
        return Some(styles);
    };
    let changes: Vec<_> = changes
        .iter()
        .map(|range| {
            original.byte_to_char_idx(byte + range.start) - char_start
                ..original.byte_to_char_idx(byte + range.end) - char_start
        })
        .collect();
    // Apply word emphasis here so previews and native documents share it.
    // Falling back to diff.plus/minus would overwrite the source syntax color.
    let scope = match row.kind {
        ReviewLineKind::Added => "diff.plus.word",
        ReviewLineKind::Removed => "diff.minus.word",
        _ => return Some(styles),
    };
    let emphasis = theme
        .try_get_exact(scope)
        .unwrap_or_else(|| Style::default().add_modifier(theme::Modifier::BOLD));
    let mut positions: Vec<_> = styles.iter().map(|(position, _)| *position).collect();
    positions.extend(changes.iter().flat_map(|range| [range.start, range.end]));
    positions.sort_unstable();
    positions.dedup();
    let mut syntax = styles.into_iter().peekable();
    let mut style = theme.get("ui.text");
    Some(
        positions
            .into_iter()
            .map(|position| {
                while syntax.peek().is_some_and(|(start, _)| *start <= position) {
                    style = syntax.next().unwrap().1;
                }
                let changed = changes.iter().any(|range| range.contains(&position));
                (
                    position,
                    if changed {
                        style.patch(emphasis)
                    } else {
                        style
                    },
                )
            })
            .collect(),
    )
}

pub fn highlights(
    doc: PresentedDocument<'_>,
    layout: &ViewLayout,
    theme: &Theme,
) -> Option<OverlayHighlights> {
    let review = doc.source().review()?;
    let text = doc.text();
    let mut highlights = Vec::new();
    for index in visible_lines(layout) {
        let Some(row) = review.row(index) else {
            continue;
        };
        let has_syntax = review
            .source()
            .is_some_and(|source| source.syntax_for_row(row).is_some());
        let start = text.line_to_char_idx(index);
        let scope = match row.kind {
            ReviewLineKind::Added => "diff.plus",
            ReviewLineKind::Removed => "diff.minus",
            ReviewLineKind::Header => "diff.delta",
            ReviewLineKind::Notice => "ui.text.inactive",
            ReviewLineKind::Context => continue,
        };
        if let Some(ranges) = review.changes(index) {
            if has_syntax {
                continue;
            }
            let Some(highlight) = theme.find_highlight(&format!("{scope}.word")) else {
                continue;
            };
            for range in ranges {
                highlights.push((highlight, start + range.start..start + range.end));
            }
        } else if !has_syntax && let Some(highlight) = theme.find_highlight(scope) {
            highlights.push((
                highlight,
                start
                    ..start
                        + text
                            .line(index, stdx::rope::LINE_TYPE)
                            .len_chars()
                            .saturating_sub(1),
            ));
        }
    }
    Some(OverlayHighlights::Heterogenous { highlights })
}

/// Include retained closing-line suffixes as well as fold headers.
fn visible_lines<'a>(layout: &ViewLayout<'a>) -> impl Iterator<Item = usize> + use<'a> {
    let text = layout.text;
    layout
        .visible_byte_ranges()
        .into_iter()
        .flat_map(move |range| {
            text.byte_to_line_idx(range.start, stdx::rope::LINE_TYPE)
                ..=text.byte_to_line_idx(range.end - 1, stdx::rope::LINE_TYPE)
        })
}

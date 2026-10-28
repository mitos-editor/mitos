//! Review-specific styling over the ordinary editor text renderer. No input or
//! viewport state lives here; document/view coordinates remain authoritative.

use stdx::rope::RopeExt as _;

use editor_core::highlight::OverlayHighlights;
use theme::{Style, Theme};
use view::{features::change_review::ReviewLineKind, graphics::Rect, view::ViewLayout, Document};

use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};

fn row_info(
    doc: &Document,
    line: usize,
) -> Option<(
    &view::features::change_review::ReviewLine,
    Option<&std::sync::Arc<view::features::change_review::ReviewSource>>,
    usize,
)> {
    if let Some(review) = doc.review() {
        return Some((review.row(line)?, review.source(), 0));
    }
    let section = doc.project_review()?.section(line)?;
    Some((
        section.row(line)?,
        section.snapshot.as_ref().map(|snapshot| &snapshot.source),
        1,
    ))
}

fn has_review(doc: &Document) -> bool {
    doc.review().is_some() || doc.project_review().is_some()
}

pub fn line_backgrounds<'a>(doc: &'a Document, theme: &'a Theme) -> impl Decoration + 'a {
    move |renderer: &mut TextRenderer, pos: LinePos| {
        let Some((row, _, _)) = row_info(doc, pos.doc_line) else {
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
    doc: &Document,
    layout: &ViewLayout,
    theme: &Theme,
) -> Option<Vec<(usize, Style)>> {
    if !has_review(doc) {
        return None;
    }
    let text = doc.text();
    let mut styles = Vec::new();
    for index in visible_lines(layout) {
        let start = text.line_to_char_idx(index);
        styles.push((start, theme.get("ui.text")));
        let Some((row, Some(source), prefix)) = row_info(doc, index) else {
            continue;
        };
        let loader = source.syntax_loader();
        if loader.scopes().as_slice() != theme.scopes() {
            loader.set_scopes(theme.scopes().to_vec());
        }
        let Some((original, syntax, line)) = source.syntax_for_row(row) else {
            continue;
        };
        let byte = original.line_to_byte_idx(line, stdx::rope::LINE_TYPE);
        let end = byte + row.text.len();
        let char_start = original.byte_to_char_idx(byte);
        let char_end = original.byte_to_char_idx(end);
        let highlighter = syntax.highlighter(original.slice(..), loader, byte as u32..end as u32);
        styles.extend(
            super::super::highlights::syntax_styles(original.slice(..), Some(highlighter), theme)
                .take_while(|(position, _)| *position <= char_end)
                .map(|(position, style)| {
                    (start + prefix + position.saturating_sub(char_start), style)
                }),
        );
    }
    Some(styles)
}

pub fn highlights(doc: &Document, layout: &ViewLayout, theme: &Theme) -> Option<OverlayHighlights> {
    if !has_review(doc) {
        return None;
    }
    let text = doc.text();
    let mut highlights = Vec::new();
    for index in visible_lines(layout) {
        let Some((row, source, prefix)) = row_info(doc, index) else {
            continue;
        };
        let has_syntax = source.is_some_and(|source| source.syntax_for_row(row).is_some());
        let start = text.line_to_char_idx(index);
        let scope = match row.kind {
            ReviewLineKind::Added => "diff.plus",
            ReviewLineKind::Removed => "diff.minus",
            ReviewLineKind::Header => "diff.delta",
            ReviewLineKind::Notice => "ui.text.inactive",
            ReviewLineKind::Context => continue,
        };
        if let Some(ranges) = &row.changes {
            let Some(highlight) = theme.find_highlight(&format!("{scope}.word")) else {
                continue;
            };
            for range in ranges {
                let from = row.text[..range.start].chars().count();
                let to = from + row.text[range.clone()].chars().count();
                highlights.push((highlight, start + prefix + from..start + prefix + to));
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

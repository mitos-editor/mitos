//! Editor adapters for the document-independent text renderer.

use super::text_decorations::DecorationManager;
use language::syntax::Highlighter;
use text_layout::{text_annotations::TextAnnotations, DisplayPosition};
use theme::Theme;
use tui::buffer::Buffer as Surface;
use view::{graphics::Rect, view::ViewPosition, Document};

pub use text_render::{render_text, LinePos, RenderSettings, TextRenderer};

#[allow(clippy::too_many_arguments)]
pub fn render_document(
    surface: &mut Surface,
    viewport: Rect,
    doc: &Document,
    offset: ViewPosition,
    doc_annotations: &TextAnnotations,
    syntax_highlighter: Option<Highlighter<'_>>,
    overlay_highlights: Vec<editor_core::highlight::OverlayHighlights>,
    theme: &Theme,
    decorations: DecorationManager,
) {
    let config = doc.config.load();
    let mut renderer = TextRenderer::new(
        surface,
        render_settings(doc, &config),
        theme,
        DisplayPosition::new(offset.vertical_offset, offset.horizontal_offset),
        viewport,
    );
    render_text(
        &mut renderer,
        doc.text().slice(..),
        offset.anchor,
        &doc.text_format(viewport.width, Some(theme)),
        doc_annotations,
        super::highlights::syntax_styles(doc.text().slice(..), syntax_highlighter, theme),
        overlay_highlights,
        theme,
        decorations,
    )
}

pub(crate) fn render_settings<'a>(
    doc: &Document,
    config: &'a editor_config::Config,
) -> RenderSettings<'a> {
    let tab_width = doc.tab_width();
    RenderSettings {
        whitespace: &config.whitespace,
        indent_guides: &config.indent_guides,
        tab_width,
        indent_width: doc.indent_style.indent_width(tab_width),
    }
}

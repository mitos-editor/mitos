//! Editor adapters for the document-independent text renderer.

use super::text_decorations::DecorationManager;
use language::syntax::Highlighter;
use text_layout::DisplayPosition;
use theme::Theme;
use tui::buffer::Buffer as Surface;
use view::{view::ViewLayout, Document};

pub use text_render::{render_text, LinePos, RenderSettings, TextRenderer};

#[allow(clippy::too_many_arguments)]
pub fn render_document(
    surface: &mut Surface,
    doc: &Document,
    layout: &ViewLayout,
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
        DisplayPosition::new(
            layout.offset.vertical_offset,
            layout.offset.horizontal_offset,
        ),
        layout.viewport,
    );
    let review_styles = super::change_review::document::syntax_styles(
        doc,
        layout.offset.anchor,
        layout.viewport.height,
        theme,
    );
    let ordinary_styles = super::highlights::syntax_styles(layout.text, syntax_highlighter, theme)
        .take(if review_styles.is_some() {
            0
        } else {
            usize::MAX
        });
    let syntax_styles = review_styles.into_iter().flatten().chain(ordinary_styles);
    render_text(
        &mut renderer,
        layout.text,
        layout.offset.anchor,
        &layout.format,
        &layout.annotations,
        syntax_styles,
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

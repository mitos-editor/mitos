//! Reuse the formatter's document-to-screen mapping for terminal hyperlinks.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Range,
};
use view::document::PresentedDocument;

use text_layout::doc_formatter::{FormattedGrapheme, GraphemeSource};
use ui_core::hyperlinks::{Hyperlink, HyperlinkBuffer};
use view::ViewId;

use super::{Decoration, TextRenderer};

struct LinkRange<'a> {
    range: Range<usize>,
    identity: u64,
    target: &'a str,
    cell_id: Option<u32>,
}

pub struct DocumentLinks<'a> {
    buffer: &'a mut HyperlinkBuffer,
    ranges: Vec<LinkRange<'a>>,
    next: usize,
}

impl<'a> DocumentLinks<'a> {
    pub fn new(doc: PresentedDocument<'a>, view: ViewId, buffer: &'a mut HyperlinkBuffer) -> Self {
        let mut ranges: Vec<LinkRange<'a>> = Vec::new();
        if doc.source().config.load().lsp.enable {
            for link in doc.document_links() {
                if link.start >= link.end
                    || !doc.supports_language_server(link.language_server_id)
                    || ranges
                        .last()
                        .is_some_and(|previous| link.start < previous.range.end)
                {
                    continue;
                }
                let Some(target) = &link.link.target else {
                    continue;
                };
                let mut identity = DefaultHasher::new();
                (
                    doc.source().id(),
                    view,
                    link.start,
                    link.end,
                    link.language_server_id,
                )
                    .hash(&mut identity);
                if Hyperlink::supports(target.as_str()) {
                    ranges.push(LinkRange {
                        range: link.start..link.end,
                        identity: identity.finish(),
                        target: target.as_str(),
                        cell_id: None,
                    });
                }
            }
        }
        Self {
            buffer,
            ranges,
            next: 0,
        }
    }
}

impl Decoration for DocumentLinks<'_> {
    fn can_skip_unwrapped_tail(&self) -> bool {
        true
    }

    fn reset_pos(&mut self, pos: usize) -> usize {
        self.next = self.ranges.partition_point(|link| link.range.end <= pos);
        self.ranges
            .get(self.next)
            .map_or(usize::MAX, |link| link.range.start.max(pos))
    }

    fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        let link = &mut self.ranges[self.next];
        if matches!(
            grapheme.source,
            GraphemeSource::Document { codepoints: 1.. }
        ) && let Some(area) = renderer.grapheme_area(grapheme)
        {
            let id = *link.cell_id.get_or_insert_with(|| {
                self.buffer
                    .insert(Hyperlink::new(link.identity, link.target.to_owned()).unwrap())
            });
            self.buffer.set(area, id);
        }
        let next = grapheme.char_idx + grapheme.source.doc_chars();
        if next < link.range.end {
            return next;
        }
        self.next += 1;
        self.ranges
            .get(self.next)
            .map_or(usize::MAX, |link| link.range.start.max(next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::Rope;
    use text_layout::{
        doc_formatter::TextFormat,
        text_annotations::{InlineAnnotation, TextAnnotations},
        DisplayPosition,
    };
    use text_render::{render_text, DecorationManager, RenderSettings};
    use theme::Theme;
    use tui::{buffer::Buffer, layout::Rect};

    fn render(
        text: &str,
        range: Range<usize>,
        width: u16,
        offset: usize,
        wrap: bool,
        inline: &[InlineAnnotation],
    ) -> HyperlinkBuffer {
        let text = Rope::from_str(text);
        let config = editor_config::Config::default();
        let theme = Theme::from(toml::Value::Table(toml::Table::new()));
        let viewport = Rect::new(2, 1, width, 4);
        let mut surface = Buffer::empty(viewport);
        let mut links = HyperlinkBuffer::new(viewport);
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(DocumentLinks {
            buffer: &mut links,
            ranges: vec![LinkRange {
                range,
                identity: 42,
                target: "https://example.com",
                cell_id: None,
            }],
            next: 0,
        });
        let mut renderer = TextRenderer::new(
            &mut surface,
            RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width: 4,
                indent_width: 4,
            },
            &theme,
            DisplayPosition::new(0, offset),
            viewport,
        );
        let mut annotations = TextAnnotations::default();
        annotations.add_inline_annotations(inline, None);
        render_text(
            &mut renderer,
            text.slice(..),
            0,
            &TextFormat {
                soft_wrap: wrap,
                viewport_width: width,
                max_wrap: 0,
                max_indent_retain: 0,
                ..Default::default()
            },
            &annotations,
            std::iter::empty(),
            vec![],
            &theme,
            decorations,
        );
        links
    }

    #[test]
    fn unicode_ranges_use_characters_and_skip_virtual_text() {
        let links = render(
            "é界xy",
            1..4,
            12,
            0,
            false,
            &[InlineAnnotation::new(2, "hint")],
        );
        assert!(links.get(2, 1).is_none(), "é precedes the link");
        for x in [3, 4, 9, 10] {
            assert_eq!(links.get(x, 1).unwrap().id, 42);
        }
        for x in 5..9 {
            assert!(links.get(x, 1).is_none(), "virtual text is not a link");
        }
        assert!(links.get(11, 1).is_none(), "EOF is not a link");
    }

    #[test]
    fn horizontal_clipping_does_not_link_a_partial_wide_glyph() {
        let links = render("é界xy", 1..4, 6, 2, false, &[]);
        assert!(links.get(2, 1).is_none());
        assert_eq!(links.get(3, 1).unwrap().id, 42);
        assert_eq!(links.get(4, 1).unwrap().id, 42);
        assert!(links.get(5, 1).is_none());
    }

    #[test]
    fn wrapped_fragments_share_one_identity_and_do_not_link_padding() {
        let links = render("abcdefghijklmnopqrstuvwx", 0..24, 12, 0, true, &[]);
        for y in 1..3 {
            for x in 2..14 {
                assert_eq!(links.get(x, y).unwrap().id, 42, "({x}, {y})");
            }
        }
        assert!(links.get(2, 3).is_none());
    }
}

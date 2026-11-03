use crate::compositor::Component;
use crate::compositor::DrawContext;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Paragraph, Wrap};

use view::graphics::Rect;

pub struct Text {
    pub(crate) contents: tui::text::Text<'static>,
    size: (u16, u16),
    viewport: (u16, u16),
}

impl Text {
    pub fn new(contents: String) -> Self {
        Self {
            contents: tui::text::Text::from(contents),
            size: (0, 0),
            viewport: (0, 0),
        }
    }
}

impl From<tui::text::Text<'static>> for Text {
    fn from(contents: tui::text::Text<'static>) -> Self {
        Self {
            contents,
            size: (0, 0),
            viewport: (0, 0),
        }
    }
}

impl Component for Text {
    fn render(&mut self, area: Rect, surface: &mut Surface, _cx: &mut DrawContext) {
        use tui::widgets::Widget;

        let par = paragraph(self.contents.clone());
        // .scroll(x, y) offsets

        par.render(area, surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        if viewport != self.viewport {
            // PERF: Paragraph currently owns its Text, so exact Ratatui measurement
            // requires this clone. The result is cached until the viewport changes.
            let paragraph = paragraph(self.contents.clone());
            let (width, height) = required_size(&paragraph, viewport.0);
            let height = height.min(viewport.1);
            self.size = (width, height);
            self.viewport = viewport;
        }
        Some(self.size)
    }
}

pub fn paragraph<'a, T>(text: T) -> Paragraph<'a>
where
    T: Into<tui::text::Text<'a>>,
{
    Paragraph::new(text).wrap(Wrap { trim: false })
}

pub fn required_size(paragraph: &Paragraph<'_>, max_text_width: u16) -> (u16, u16) {
    let width = (paragraph.line_width() as u16).min(max_text_width);
    let height = paragraph.line_count(width) as u16;
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_the_same_wrapping_used_for_rendering() {
        let words = paragraph("hello world");
        assert_eq!((5, 2), required_size(&words, 5));

        let long_word = paragraph("abcdefghij");
        assert_eq!((5, 2), required_size(&long_word, 5));
    }

    #[test]
    fn handles_empty_width() {
        let paragraph = paragraph("hello");
        assert_eq!((0, 0), required_size(&paragraph, 0));
    }
}

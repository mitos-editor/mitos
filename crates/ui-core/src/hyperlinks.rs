//! Hyperlink metadata kept alongside, rather than inside, Ratatui cells.

use ratatui_core::layout::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub id: u64,
    uri: String,
}

impl Hyperlink {
    /// Only web links can be opened by the terminal. Control characters must
    /// never reach OSC output, even when a server supplies a malformed URI.
    pub fn new(id: u64, uri: String) -> Option<Self> {
        Self::supports(&uri).then_some(Self { id, uri })
    }

    pub fn supports(uri: &str) -> bool {
        uri.split_once("://").is_some_and(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
        }) && !uri.chars().any(char::is_control)
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }
}

/// One four-byte index per screen cell; each destination is stored once.
#[derive(Debug, Default)]
pub struct HyperlinkBuffer {
    area: Rect,
    cells: Vec<u32>,
    links: Vec<Hyperlink>,
}

impl HyperlinkBuffer {
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            cells: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn insert(&mut self, link: Hyperlink) -> u32 {
        if self.links.is_empty() {
            self.cells
                .resize(self.area.width as usize * self.area.height as usize, 0);
        }
        self.links.push(link);
        u32::try_from(self.links.len()).expect("too many visible hyperlinks")
    }

    pub fn get(&self, x: u16, y: u16) -> Option<&Hyperlink> {
        let index = self.index(x, y)?;
        let id = self.cells.get(index).copied().unwrap_or_default();
        id.checked_sub(1).map(|id| &self.links[id as usize])
    }

    pub fn set(&mut self, area: Rect, id: u32) {
        debug_assert!(id as usize <= self.links.len());
        if self.cells.is_empty() {
            return;
        }
        let area = self.area.intersection(area);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let start = self.index(area.x, y).unwrap();
            self.cells[start..start + area.width as usize].fill(id);
        }
    }

    /// An opaque UI component removes hyperlinks belonging to the layer below.
    pub fn clear(&mut self, area: Rect) {
        self.set(area, 0);
    }

    /// Whether no destinations were registered in this frame.
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        self.area.contains((x, y).into()).then(|| {
            (y - self.area.y) as usize * self.area.width as usize + (x - self.area.x) as usize
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_clipped_and_overlays_clear_only_their_area() {
        let mut links = HyperlinkBuffer::new(Rect::new(2, 3, 4, 2));
        let id = links.insert(Hyperlink::new(1, "https://example.com".into()).unwrap());
        links.set(Rect::new(0, 3, 5, 1), id);
        links.clear(Rect::new(3, 3, 1, 1));
        assert!(links.get(2, 3).is_some());
        assert!(links.get(3, 3).is_none());
        assert!(links.get(4, 3).is_some());
        assert!(links.get(5, 3).is_none());
        assert!(links.get(2, 4).is_none());
        links.clear(Rect::new(20, 20, 2, 2));
    }

    #[test]
    fn rejects_non_web_targets_and_terminal_controls() {
        for uri in [
            "file:///tmp/a",
            "javascript:alert(1)",
            "https://x/\x1b\\",
            "https://x/\x07",
            "https://x/\u{009c}",
        ] {
            assert!(Hyperlink::new(0, uri.into()).is_none());
        }
        assert!(Hyperlink::new(0, "https://example.com/é?q=%20".into()).is_some());
        assert!(Hyperlink::new(0, "HTTPS://example.com".into()).is_some());
    }
}

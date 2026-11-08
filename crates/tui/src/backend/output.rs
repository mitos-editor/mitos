//! Shared style decisions; each native backend retains its own output encoding.
use ratatui::buffer::{Buffer, Cell, CellDiffOption, CellWidth};
use ratatui::style::Modifier;
use ui_core::hyperlinks::HyperlinkBuffer;

/// Metadata-only changes supplement Ratatui's ordinary glyph/style diff.
#[derive(Debug, Default)]
pub(super) struct HyperlinkState {
    pub links: HyperlinkBuffer,
    pub updates: Vec<(u16, u16, Cell)>,
}

impl HyperlinkState {
    pub fn prepare(&mut self, links: HyperlinkBuffer, buffer: &Buffer) {
        self.updates.clear();
        if self.links.is_empty() && links.is_empty() {
            self.links = links;
            return;
        }
        for y in buffer.area.y..buffer.area.bottom() {
            let mut x = buffer.area.x;
            while x < buffer.area.right() {
                let cell = &buffer[(x, y)];
                if cell.diff_option != CellDiffOption::Skip
                    && self.links.get(x, y) != links.get(x, y)
                {
                    self.updates.push((x, y, cell.clone()));
                }
                x = x.saturating_add(cell.cell_width().max(1));
            }
        }
        self.links = links;
    }
}

/// Both streams are in row order. Prefer Ratatui's update when both contain
/// the same cell, so each changed glyph is written at most once.
pub(super) fn merge_updates<'a: 'b, 'b>(
    content: impl Iterator<Item = (u16, u16, &'a Cell)>,
    extra: &'b [(u16, u16, Cell)],
) -> impl Iterator<Item = (u16, u16, &'b Cell)> {
    let mut content = content
        .map(|(x, y, cell)| -> (u16, u16, &'b Cell) { (x, y, cell) })
        .peekable();
    let mut extra = extra.iter().peekable();
    std::iter::from_fn(move || match (content.peek(), extra.peek()) {
        (Some((x, y, _)), Some((ex, ey, _))) if (y, x) <= (ey, ex) => {
            if (x, y) == (ex, ey) {
                extra.next();
            }
            content.next()
        }
        (_, Some(_)) => extra.next().map(|(x, y, cell)| (*x, *y, cell)),
        _ => content.next(),
    })
}

/// OSC 8 is independent of SGR: resetting colors does not close a hyperlink.
#[derive(Default)]
pub(super) struct HyperlinkWriter<'a> {
    current: Option<&'a ui_core::hyperlinks::Hyperlink>,
}

impl<'a> HyperlinkWriter<'a> {
    pub fn set(
        &mut self,
        writer: &mut impl std::io::Write,
        link: Option<&'a ui_core::hyperlinks::Hyperlink>,
    ) -> std::io::Result<()> {
        if self.current == link {
            return Ok(());
        }
        match link {
            Some(link) => write!(writer, "\x1b]8;id=mitos-{:x};{}\x1b\\", link.id, link.uri())?,
            None => writer.write_all(b"\x1b]8;;\x1b\\")?,
        }
        self.current = link;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum ModifierChange {
    Reverse(bool),
    NormalIntensity,
    Bold,
    Dim,
    Italic(bool),
    CrossedOut(bool),
    NoBlink,
    SlowBlink,
    RapidBlink,
    Hidden(bool),
}

pub(super) fn modifier_changes(
    from: Modifier,
    to: Modifier,
) -> impl Iterator<Item = ModifierChange> {
    use ModifierChange::*;
    let removed = from - to;
    let added = to - from;
    [
        removed
            .contains(Modifier::REVERSED)
            .then_some(Reverse(false)),
        removed.contains(Modifier::BOLD).then_some(NormalIntensity),
        (removed.contains(Modifier::BOLD) && to.contains(Modifier::DIM)).then_some(Dim),
        removed.contains(Modifier::ITALIC).then_some(Italic(false)),
        removed.contains(Modifier::DIM).then_some(NormalIntensity),
        removed
            .contains(Modifier::CROSSED_OUT)
            .then_some(CrossedOut(false)),
        removed
            .intersects(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK)
            .then_some(NoBlink),
        removed.contains(Modifier::HIDDEN).then_some(Hidden(false)),
        added.contains(Modifier::REVERSED).then_some(Reverse(true)),
        added.contains(Modifier::BOLD).then_some(Bold),
        added.contains(Modifier::ITALIC).then_some(Italic(true)),
        added.contains(Modifier::DIM).then_some(Dim),
        added
            .contains(Modifier::CROSSED_OUT)
            .then_some(CrossedOut(true)),
        added.contains(Modifier::SLOW_BLINK).then_some(SlowBlink),
        added.contains(Modifier::RAPID_BLINK).then_some(RapidBlink),
        added.contains(Modifier::HIDDEN).then_some(Hidden(true)),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod hyperlink_tests {
    use super::*;
    use ratatui::layout::Rect;
    use ui_core::hyperlinks::Hyperlink;

    fn links(area: Rect, target: &str) -> HyperlinkBuffer {
        let mut links = HyperlinkBuffer::new(area);
        let id = links.insert(Hyperlink::new(42, target.into()).unwrap());
        links.set(area, id);
        links
    }

    #[test]
    fn target_changes_and_removal_repaint_unchanged_labels_once() {
        let buffer = Buffer::with_lines(["label"]);
        let mut state = HyperlinkState::default();
        for target in ["https://example.com/a", "https://example.com/b"] {
            state.prepare(links(buffer.area, target), &buffer);
            assert_eq!(state.updates.len(), 5);
            // Normal glyph updates and metadata updates are merged, not repeated.
            let normal = [(0, 0, &buffer[(0, 0)]), (4, 0, &buffer[(4, 0)])];
            let updates = merge_updates(normal.into_iter(), &state.updates).collect::<Vec<_>>();
            assert_eq!(
                updates.iter().map(|(x, _, _)| *x).collect::<Vec<_>>(),
                [0, 1, 2, 3, 4]
            );
            state.prepare(links(buffer.area, target), &buffer);
            assert!(state.updates.is_empty(), "unchanged links need no writes");
        }
        state.prepare(HyperlinkBuffer::new(buffer.area), &buffer);
        assert_eq!(state.updates.len(), 5);
        state.prepare(HyperlinkBuffer::new(buffer.area), &buffer);
        assert!(state.updates.is_empty());
    }

    #[test]
    fn overlays_resize_and_wide_glyphs_keep_cell_boundaries() {
        let buffer = Buffer::with_lines(["界x"]);
        let mut state = HyperlinkState::default();
        state.prepare(links(buffer.area, "https://example.com"), &buffer);
        assert_eq!(
            state.updates.iter().map(|(x, _, _)| *x).collect::<Vec<_>>(),
            [0, 2]
        );
        let mut covered = links(buffer.area, "https://example.com");
        covered.clear(buffer.area);
        state.prepare(covered, &buffer);
        assert_eq!(state.updates.len(), 2);
        let resized = Buffer::with_lines(["a", "b"]);
        state.prepare(links(resized.area, "https://example.com"), &resized);
        assert_eq!(
            state
                .updates
                .iter()
                .map(|(x, y, _)| (*x, *y))
                .collect::<Vec<_>>(),
            [(0, 0), (0, 1)]
        );
    }

    #[test]
    fn osc_output_coalesces_runs_and_closes_before_plain_text() {
        let first = Hyperlink::new(42, "https://example.com".into()).unwrap();
        let second = Hyperlink::new(43, "https://example.com".into()).unwrap();
        let mut writer = HyperlinkWriter::default();
        let mut bytes = Vec::new();
        writer.set(&mut bytes, Some(&first)).unwrap();
        bytes.extend_from_slice(b"la");
        writer.set(&mut bytes, Some(&first)).unwrap();
        bytes.extend_from_slice(b"bel");
        writer.set(&mut bytes, Some(&second)).unwrap();
        bytes.extend_from_slice(b"other");
        writer.set(&mut bytes, None).unwrap();
        bytes.extend_from_slice(b" plain");
        writer.set(&mut bytes, None).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\x1b]8;id=mitos-2a;https://example.com\x1b\\label\x1b]8;id=mitos-2b;https://example.com\x1b\\other\x1b]8;;\x1b\\ plain");
    }
}

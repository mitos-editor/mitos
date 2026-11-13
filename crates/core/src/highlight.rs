//! Highlight IDs and visual overlays, independent of syntax parsers.
use std::{fmt, num::NonZeroU32, ops};

/// An index into a caller-owned style table. The maximum u32 is reserved so
/// optional IDs retain the same compact representation as a single ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Highlight(NonZeroU32);

impl Highlight {
    pub const MAX: u32 = u32::MAX - 1;

    pub const fn new(value: u32) -> Self {
        assert!(value <= Self::MAX, "highlight ID is reserved");
        Self(NonZeroU32::new(value + 1).unwrap())
    }

    pub const fn get(self) -> u32 {
        self.0.get() - 1
    }

    pub const fn idx(self) -> usize {
        self.get() as usize
    }
}

impl fmt::Debug for Highlight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Highlight").field(&self.get()).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightEvent {
    Refresh,
    Push,
}

/// A set of "overlay" highlights and ranges they apply to.
///
/// As overlays, the styles for the given `Highlight`s are merged on top of the syntax highlights.
#[derive(Debug)]
pub enum OverlayHighlights {
    /// All highlights use a single `Highlight`.
    ///
    /// Note that, currently, all ranges are assumed to be non-overlapping. This could change in
    /// the future though.
    Homogeneous {
        highlight: Highlight,
        ranges: Vec<ops::Range<usize>>,
    },
    /// A collection of different highlights for given ranges.
    ///
    /// Note that the ranges **must be non-overlapping**.
    Heterogenous {
        highlights: Vec<(Highlight, ops::Range<usize>)>,
    },
}

impl OverlayHighlights {
    pub fn single(highlight: Highlight, range: ops::Range<usize>) -> Self {
        Self::Homogeneous {
            highlight,
            ranges: vec![range],
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Homogeneous { ranges, .. } => ranges.is_empty(),
            Self::Heterogenous { highlights } => highlights.is_empty(),
        }
    }
}

#[derive(Debug)]
struct Overlay {
    highlights: OverlayHighlights,
    /// The position of the highlighter into the Vec of ranges of the overlays.
    ///
    /// Used by the `OverlayHighlighter`.
    idx: usize,
    /// The currently active highlight (and the ending character index) for this overlay.
    ///
    /// Used by the `OverlayHighlighter`.
    active_highlight: Option<(Highlight, usize)>,
}

impl Overlay {
    fn new(highlights: OverlayHighlights) -> Option<Self> {
        (!highlights.is_empty()).then_some(Self {
            highlights,
            idx: 0,
            active_highlight: None,
        })
    }

    fn current(&self) -> Option<(Highlight, ops::Range<usize>)> {
        match &self.highlights {
            OverlayHighlights::Homogeneous { highlight, ranges } => ranges
                .get(self.idx)
                .map(|range| (*highlight, range.clone())),
            OverlayHighlights::Heterogenous { highlights } => highlights.get(self.idx).cloned(),
        }
    }

    fn start(&self) -> Option<usize> {
        match &self.highlights {
            OverlayHighlights::Homogeneous { ranges, .. } => {
                ranges.get(self.idx).map(|range| range.start)
            }
            OverlayHighlights::Heterogenous { highlights } => highlights
                .get(self.idx)
                .map(|(_highlight, range)| range.start),
        }
    }
}

/// A collection of highlights to apply when rendering which merge on top of syntax highlights.
#[derive(Debug)]
pub struct OverlayHighlighter {
    overlays: Vec<Overlay>,
    next_highlight_start: usize,
    next_highlight_end: usize,
}

impl OverlayHighlighter {
    pub fn new(overlays: impl IntoIterator<Item = OverlayHighlights>) -> Self {
        let overlays: Vec<_> = overlays.into_iter().filter_map(Overlay::new).collect();
        let next_highlight_start = overlays
            .iter()
            .filter_map(|overlay| overlay.start())
            .min()
            .unwrap_or(usize::MAX);

        Self {
            overlays,
            next_highlight_start,
            next_highlight_end: usize::MAX,
        }
    }

    /// The current position in the overlay highlights.
    ///
    /// This method is meant to be used when treating this type as a cursor over the overlay
    /// highlights.
    ///
    /// `usize::MAX` is returned when there are no more overlay highlights.
    pub fn next_event_offset(&self) -> usize {
        self.next_highlight_start.min(self.next_highlight_end)
    }

    /// Advance to the next event and return all active highlights in overlay order.
    pub fn advance(&mut self) -> impl Iterator<Item = Highlight> + '_ {
        let pos = self.next_event_offset();
        assert_ne!(pos, usize::MAX, "overlay highlighter is exhausted");

        if self.next_highlight_end == pos {
            for overlay in self.overlays.iter_mut() {
                if overlay
                    .active_highlight
                    .is_some_and(|(_highlight, end)| end == pos)
                {
                    overlay.active_highlight.take();
                }
            }
        }

        while self.next_highlight_start == pos {
            for overlay in self.overlays.iter_mut() {
                let Some((highlight, range)) = overlay.current() else {
                    continue;
                };
                if range.start != self.next_highlight_start {
                    continue;
                }

                // If this overlay has a highlight at this start index, set its active highlight
                // and increment the cursor position within the overlay.
                overlay.active_highlight = Some((highlight, range.end));
                overlay.idx += 1;
            }

            self.next_highlight_start = self
                .overlays
                .iter()
                .filter_map(|overlay| overlay.start())
                .min()
                .unwrap_or(usize::MAX);
        }

        self.next_highlight_end = self
            .overlays
            .iter()
            .filter_map(|overlay| Some(overlay.active_highlight?.1))
            .min()
            .unwrap_or(usize::MAX);

        self.overlays
            .iter()
            .flat_map(|overlay| overlay.active_highlight)
            .map(|(highlight, _end)| highlight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_ids_preserve_boundary_values_and_compact_optional_storage() {
        for value in [0, 1, 255, 0x00ff_ffff, Highlight::MAX] {
            let highlight = Highlight::new(value);
            assert_eq!(highlight.get(), value);
            assert_eq!(highlight.idx(), value as usize);
        }
        assert_eq!(size_of::<Option<Highlight>>(), size_of::<u32>());
    }

    #[test]
    fn overlapping_overlays_restore_the_previous_highlight() {
        let outer = Highlight::new(1);
        let inner = Highlight::new(2);
        let mut cursor = OverlayHighlighter::new([
            OverlayHighlights::single(outer, 0..4),
            OverlayHighlights::single(inner, 1..3),
        ]);
        for (position, expected) in [
            (0, vec![outer]),
            (1, vec![outer, inner]),
            (3, vec![outer]),
            (4, vec![]),
        ] {
            assert_eq!(cursor.next_event_offset(), position);
            assert_eq!(cursor.advance().collect::<Vec<_>>(), expected);
        }
        assert_eq!(cursor.next_event_offset(), usize::MAX);
    }

    #[test]
    fn overlay_boundaries_keep_order_and_process_empty_ranges() {
        let first = Highlight::new(1);
        let second = Highlight::new(2);
        let third = Highlight::new(3);
        let mut cursor = OverlayHighlighter::new([
            OverlayHighlights::Heterogenous {
                highlights: vec![(first, 0..2), (third, 2..4)],
            },
            OverlayHighlights::single(second, 2..2),
        ]);

        for (position, expected) in [
            (0, vec![first]),
            (2, vec![third, second]),
            (2, vec![third]),
            (4, vec![]),
        ] {
            assert_eq!(cursor.next_event_offset(), position);
            assert_eq!(cursor.advance().collect::<Vec<_>>(), expected);
        }
        assert_eq!(cursor.next_event_offset(), usize::MAX);
    }
}

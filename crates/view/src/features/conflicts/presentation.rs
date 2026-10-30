//! A document-owned interpretation shared by conflict commands and presentation.

use crate::{Document, ViewId};
use editor_core::Rope;
use stdx::rope::RopeExt as _;
use text_layout::text_annotations::{LineReplacement, TextAnnotations};
use vcs::conflict::{self, Conflict};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictSide {
    Current,
    Base,
    Incoming,
    End,
}

impl ConflictSide {
    pub fn scope(self) -> &'static str {
        match self {
            Self::Current => "ui.virtual.conflict.current",
            Self::Base => "ui.virtual.conflict.base",
            Self::Incoming => "ui.virtual.conflict.incoming",
            Self::End => "ui.virtual.conflict.end",
        }
    }
    pub fn fallback(self) -> &'static str {
        match self {
            Self::Current => "info",
            Self::Incoming => "hint",
            _ => "ui.text.inactive",
        }
    }
}

pub struct ConflictMarker {
    pub line: usize,
    pub side: ConflictSide,
    pub replacement: LineReplacement,
}

#[derive(Default)]
pub struct ConflictPresentation {
    pub regions: Vec<Conflict>,
    pub markers: Vec<ConflictMarker>,
    pub error: Option<String>,
}

impl ConflictPresentation {
    pub fn new(text: &Rope) -> Self {
        if text.len() > vcs::MAX_REVIEW_BYTES {
            return Self::default();
        }
        // Inspect only line prefixes before allocating contiguous text. Most
        // documents contain no markers, and this runs after each text change.
        if !text.slice(..).lines(stdx::rope::LINE_TYPE).any(|line| {
            let mut chars = line.chars();
            let Some(marker @ ('<' | '>')) = chars.next() else {
                return false;
            };
            (0..6).all(|_| chars.next() == Some(marker))
                && chars
                    .next()
                    .is_none_or(|ch| ch == marker || matches!(ch, ' ' | '\r' | '\n'))
        }) {
            return Self::default();
        }
        let source = text.to_string();
        let regions = match conflict::parse(&source) {
            Ok(regions) => regions,
            Err(error) => {
                return Self {
                    error: Some(error.to_string()),
                    ..Self::default()
                }
            }
        };
        let mut markers = Vec::new();
        let label = |value: &str| {
            value
                .chars()
                .filter(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}'))
                .take(100)
                .collect::<String>()
        };
        for (index, region) in regions.iter().enumerate() {
            let items = [
                (
                    Some(&region.markers.current),
                    ConflictSide::Current,
                    format!(
                        "── Conflict {} of {} · Current: {} ──",
                        index + 1,
                        regions.len(),
                        label(&region.current_label)
                    ),
                ),
                (
                    region.markers.base.as_ref(),
                    ConflictSide::Base,
                    "── Base ──".into(),
                ),
                (
                    Some(&region.markers.incoming),
                    ConflictSide::Incoming,
                    format!("── Incoming: {} ──", label(&region.incoming_label)),
                ),
                (
                    Some(&region.markers.end),
                    ConflictSide::End,
                    "── End conflict ──".into(),
                ),
            ];
            for (range, side, heading) in items {
                let Some(range) = range else {
                    continue;
                };
                let start = text.byte_to_char_idx(range.start);
                let content = source[range.clone()].trim_end_matches(['\r', '\n']);
                let replacement = LineReplacement {
                    range: start..start + content.chars().count(),
                    text: heading.clone().into(),
                };
                markers.push(ConflictMarker {
                    line: text.char_to_line_idx(start),
                    side,
                    replacement,
                });
            }
        }
        Self {
            regions,
            markers,
            error: None,
        }
    }

    pub fn active(&self) -> bool {
        !self.regions.is_empty() || self.error.is_some()
    }

    pub fn side_at_line(&self, text: &Rope, line: usize) -> Option<ConflictSide> {
        let pos = text.line_to_byte_idx(
            line.min(text.len_lines(stdx::rope::LINE_TYPE).saturating_sub(1)),
            stdx::rope::LINE_TYPE,
        );
        let index = self
            .regions
            .partition_point(|region| region.whole.end <= pos);
        let region = self.regions.get(index)?;
        if !region.whole.contains(&pos) {
            return None;
        }
        if region.current.contains(&pos) || region.markers.current.contains(&pos) {
            Some(ConflictSide::Current)
        } else if region
            .base
            .as_ref()
            .is_some_and(|range| range.contains(&pos))
            || region
                .markers
                .base
                .as_ref()
                .is_some_and(|range| range.contains(&pos))
        {
            Some(ConflictSide::Base)
        } else if region.incoming.contains(&pos) || region.markers.incoming.contains(&pos) {
            Some(ConflictSide::Incoming)
        } else {
            Some(ConflictSide::End)
        }
    }
}

impl Document {
    pub fn conflict_presentation(&self) -> &ConflictPresentation {
        self.conflicts.get_or_init(|| {
            std::sync::Arc::new({
                if self.is_binary() || self.review().is_some() {
                    ConflictPresentation::default()
                } else {
                    ConflictPresentation::new(self.text())
                }
            })
        })
    }

    pub fn conflict_marker_hidden(&self, view: ViewId, line: usize) -> bool {
        if self.raw_conflict_markers(view) {
            return false;
        }
        let markers = &self.conflict_presentation().markers;
        let index = markers.partition_point(|marker| marker.line < line);
        let Some(marker) = markers.get(index).filter(|marker| marker.line == line) else {
            return false;
        };
        let range = &marker.replacement.range;
        let folds = self.folded_ranges(view);
        let next = folds.partition_point(|fold| fold.range.end <= range.start);
        folds
            .get(next)
            .is_none_or(|fold| fold.range.start >= range.end)
    }

    pub fn annotate_conflicts<'a>(
        &'a self,
        view: ViewId,
        theme: Option<&theme::Theme>,
        annotations: &mut TextAnnotations<'a>,
    ) {
        if self.raw_conflict_markers(view) {
            return;
        }
        for marker in &self.conflict_presentation().markers {
            if !self.conflict_marker_hidden(view, marker.line) {
                continue;
            }
            let highlight = theme.and_then(|theme| {
                theme
                    .find_highlight(marker.side.scope())
                    .or_else(|| theme.find_highlight(marker.side.fallback()))
            });
            annotations.add_line_replacement(&marker.replacement, highlight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_spans_preserve_unicode_crlf_custom_width_and_empty_sides() {
        let text = Rope::from("préfix\r\n<<<<<<<< HEAD\r\n|||||||| original\r\nbase\r\n========\r\n界\r\n>>>>>>>> incoming");
        let model = ConflictPresentation::new(&text);
        assert!(model.error.is_none());
        assert_eq!(model.regions.len(), 1);
        assert_eq!(model.markers.len(), 4);
        assert_eq!(
            model
                .markers
                .iter()
                .map(|marker| marker.line)
                .collect::<Vec<_>>(),
            [1, 2, 4, 6]
        );
        for marker in &model.markers {
            let source = text
                .slice_chars(marker.replacement.range.clone())
                .to_string();
            assert!(!source.contains(['\r', '\n']));
            assert!(source.len() >= 8);
            assert_eq!(model.side_at_line(&text, marker.line), Some(marker.side));
        }
        assert_eq!(model.side_at_line(&text, 5), Some(ConflictSide::Incoming));
        assert_eq!(model.side_at_line(&text, 0), None);
    }

    #[test]
    fn detects_markers_in_chunked_documents_and_ignores_long_normal_lines() {
        let prefix = "ordinary text\n".repeat(400);
        let source = format!("{prefix}<<<<<<< HEAD\na\n=======\nb\n>>>>>>> incoming\n");
        assert_eq!(
            ConflictPresentation::new(&Rope::from(source)).regions.len(),
            1
        );
        assert!(!ConflictPresentation::new(&Rope::from("x".repeat(100_000))).active());
    }

    #[test]
    fn malformed_conflicts_stay_raw_and_normal_separators_are_not_conflicts() {
        for text in [
            "<<<<<<< HEAD\none\n",
            "<<<<<<< HEAD\n<<<<<<< nested\n=======\n>>>>>>> end\n",
        ] {
            let model = ConflictPresentation::new(&Rope::from(text));
            assert!(model.active());
            assert!(model.error.is_some());
            assert!(model.markers.is_empty());
        }
        assert!(
            !ConflictPresentation::new(&Rope::from("Title\n=======\nordinary text\n")).active()
        );
    }
}

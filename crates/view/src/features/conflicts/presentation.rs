//! A document-owned interpretation shared by conflict commands and presentation.

use std::{ops::Range, sync::LazyLock};

use crate::{Document, ViewId};
use editor_core::{
    highlight::{Highlight, OverlayHighlights},
    ChangeSet, Rope, RopeSlice,
};
use stdx::rope::{Regex, RopeExt as _, RopeSliceExt as _};
use text_layout::text_annotations::{InlineAnnotation, TextAnnotations};
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
            Self::Incoming | Self::End => "ui.virtual.conflict.incoming",
        }
    }
    pub fn fallback(self) -> &'static str {
        match self {
            Self::Current => "diff.plus",
            Self::Incoming | Self::End => "info",
            _ => "ui.text.inactive",
        }
    }
    fn highlight(self, theme: &theme::Theme) -> Option<Highlight> {
        theme
            .find_highlight_exact(self.scope())
            .or_else(|| theme.find_highlight_exact("ui.virtual.conflict"))
            .or_else(|| theme.find_highlight(self.fallback()))
    }
}

pub struct ConflictMarker {
    pub line: usize,
    pub side: ConflictSide,
    /// Source character offsets, excluding the line ending.
    pub range: Range<usize>,
}

struct ConflictLineNumbers {
    lines: Range<usize>,
    sections: [Range<usize>; 3],
    display_start: usize,
    removed_lines: usize,
}

#[derive(Default)]
pub struct ConflictPresentation {
    pub regions: Vec<Conflict>,
    pub markers: Vec<ConflictMarker>,
    pub error: Option<String>,
    line_numbers: Vec<ConflictLineNumbers>,
    labels: [Vec<InlineAnnotation>; 2],
}

// The parser validates line boundaries and suffixes. This deliberately broad
// prefilter also works on short edit windows and across rope chunk boundaries.
fn contains_marker_candidate(text: RopeSlice<'_>) -> bool {
    static MARKERS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(?:<{7}(?:<|[ \r\n]|$)|>{7}(?:>|[ \r\n]|$))").unwrap());
    MARKERS.is_match(text.regex_input())
}

impl ConflictPresentation {
    pub fn marker_highlights(
        &self,
        text: &Rope,
        visible: Range<usize>,
        theme: &theme::Theme,
    ) -> OverlayHighlights {
        let model = self;
        let first = model
            .markers
            .partition_point(|marker| marker.range.end <= visible.start);
        let highlights = model.markers[first..]
            .iter()
            .take_while(|marker| marker.range.start < visible.end)
            .filter_map(|marker| {
                let highlight = model
                    .side_at_line(text, marker.line)
                    .and_then(|side| side.highlight(theme))
                    .or_else(|| theme.find_highlight_exact("ui.text"))?;
                Some((highlight, marker.range.clone()))
            })
            .collect();
        OverlayHighlights::Heterogenous { highlights }
    }

    pub fn new(text: &Rope) -> Self {
        if text.len() > vcs::MAX_REVIEW_BYTES {
            return Self::default();
        }
        if !contains_marker_candidate(text.slice(..)) {
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
        let mut labels: [Vec<InlineAnnotation>; 2] = Default::default();
        let mut line_numbers = Vec::with_capacity(regions.len());
        let mut removed_lines = 0;
        for region in &regions {
            let section_lines = |range: &Range<usize>| {
                text.byte_to_line_idx(range.start, stdx::rope::LINE_TYPE)
                    ..text.byte_to_line_idx(range.end, stdx::rope::LINE_TYPE)
            };
            let sections = [
                section_lines(&region.current),
                region.base.as_ref().map_or(0..0, section_lines),
                section_lines(&region.incoming),
            ];
            let lines = text.byte_to_line_idx(region.whole.start, stdx::rope::LINE_TYPE)
                ..text.byte_to_line_idx(region.markers.end.start, stdx::rope::LINE_TYPE) + 1;
            let display_start = lines.start - removed_lines;
            let longest = sections
                .iter()
                .map(|section| section.len())
                .max()
                .unwrap_or(0);
            removed_lines += lines.len() - longest;
            line_numbers.push(ConflictLineNumbers {
                lines,
                sections,
                display_start,
                removed_lines,
            });
            let items = [
                (Some(&region.markers.current), ConflictSide::Current),
                (region.markers.base.as_ref(), ConflictSide::Base),
                (Some(&region.markers.incoming), ConflictSide::Incoming),
                (Some(&region.markers.end), ConflictSide::End),
            ];
            for (range, side) in items {
                let Some(range) = range else {
                    continue;
                };
                let start = text.byte_to_char_idx(range.start);
                let content = source[range.clone()].trim_end_matches(['\r', '\n']);
                let range = start..start + content.chars().count();
                match side {
                    ConflictSide::Current => {
                        labels[0].push(InlineAnnotation::new(range.end, " (Current Change)"))
                    }
                    ConflictSide::End => {
                        labels[1].push(InlineAnnotation::new(range.end, " (Incoming Change)"))
                    }
                    _ => {}
                }
                markers.push(ConflictMarker {
                    line: text.char_to_line_idx(start),
                    side,
                    range,
                });
            }
        }
        Self {
            regions,
            markers,
            error: None,
            line_numbers,
            labels,
        }
    }

    /// Zero-based gutter number. Conflict sections share their first number;
    /// following text advances by the longest section, excluding marker lines.
    /// Markers themselves use that first number when serving as a fold header.
    pub fn line_number(&self, line: usize) -> usize {
        let index = self
            .line_numbers
            .partition_point(|region| region.lines.end <= line);
        if let Some(region) = self
            .line_numbers
            .get(index)
            .filter(|region| region.lines.contains(&line))
        {
            let offset = region
                .sections
                .iter()
                .find(|section| section.contains(&line))
                .map_or(0, |section| line - section.start);
            return region.display_start + offset;
        }
        line - index
            .checked_sub(1)
            .map_or(0, |index| self.line_numbers[index].removed_lines)
    }

    /// Number of document lines after overlapping conflict sections in the gutter.
    pub fn line_count(&self, physical_lines: usize) -> usize {
        physical_lines
            - self
                .line_numbers
                .last()
                .map_or(0, |region| region.removed_lines)
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
        // In diff3, only the outer markers belong to the colored blocks.
        if let Some(base) = &region.markers.base
            && (base.contains(&pos) || region.markers.incoming.contains(&pos))
        {
            return None;
        }
        if region.current.contains(&pos) || region.markers.current.contains(&pos) {
            Some(ConflictSide::Current)
        } else if region
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
    pub(crate) fn invalidate_conflicts(&mut self, before: &Rope, changes: &ChangeSet) {
        // An empty presentation has no positions to remap. Recheck only affected
        // lines, including joins and edits to marker suffixes or line endings.
        let keep_empty = self.conflicts.get().is_some_and(|model| {
            !model.active()
                && before.len() <= vcs::MAX_REVIEW_BYTES
                && self.text().len() <= vcs::MAX_REVIEW_BYTES
                && !changes.changed_ranges(0).any(|(_, range)| {
                    let text = self.text();
                    let first = text.char_to_line_idx(range.start);
                    let after_last = (text.char_to_line_idx(range.end) + 1)
                        .min(text.len_lines(stdx::rope::LINE_TYPE));
                    contains_marker_candidate(text.slice(
                        text.line_to_byte_idx(first, stdx::rope::LINE_TYPE)
                            ..text.line_to_byte_idx(after_last, stdx::rope::LINE_TYPE),
                    ))
                })
        });
        if !keep_empty {
            self.conflicts.take();
        }
    }

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

    pub fn conflict_marker_line_number_hidden(&self, view: ViewId, line: usize) -> bool {
        crate::document::PresentedDocument::from(self)
            .conflict_marker_line_number_hidden(view, line)
    }
    pub fn annotate_conflicts<'a>(
        &'a self,
        theme: Option<&theme::Theme>,
        annotations: &mut TextAnnotations<'a>,
    ) {
        crate::document::PresentedDocument::from(self).annotate_conflicts(theme, annotations);
    }
}

impl<'a> crate::document::PresentedDocument<'a> {
    pub fn conflict_marker_line_number_hidden(&self, view: ViewId, line: usize) -> bool {
        let markers = &self.conflict_presentation().markers;
        let index = markers.partition_point(|marker| marker.line < line);
        let Some(marker) = markers.get(index).filter(|marker| marker.line == line) else {
            return false;
        };
        self.conflict_marker_unfolded(view, marker)
    }

    fn conflict_marker_unfolded(&self, view: ViewId, marker: &ConflictMarker) -> bool {
        let range = &marker.range;
        let folds = self.folded_ranges(view);
        let next = folds.partition_point(|fold| fold.range.end <= range.start);
        folds
            .get(next)
            .is_none_or(|fold| fold.range.start >= range.end)
    }

    /// Color source markers without replacing their text or cursor positions.
    pub fn conflict_marker_highlights(
        &self,
        visible: Range<usize>,
        theme: &theme::Theme,
    ) -> OverlayHighlights {
        self.conflict_presentation()
            .marker_highlights(self.text(), visible, theme)
    }

    pub fn annotate_conflicts(
        self,
        theme: Option<&theme::Theme>,
        annotations: &mut TextAnnotations<'a>,
    ) {
        let presentation = self.conflict_presentation();
        if presentation.regions.is_empty() {
            return;
        }
        for (labels, side) in presentation
            .labels
            .iter()
            .zip([ConflictSide::Current, ConflictSide::End])
        {
            let highlight = theme.and_then(|theme| side.highlight(theme));
            annotations.add_inline_annotations(labels, highlight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(source: &str) -> Document {
        use std::sync::Arc;
        Document::from(
            Rope::from(source),
            None,
            Arc::new(arc_swap::ArcSwap::from_pointee(
                editor_config::Config::default(),
            )),
            Arc::new(language::syntax::Loader::default()),
        )
    }

    fn edit(
        doc: &mut Document,
        changes: impl Iterator<Item = (usize, usize, Option<editor_core::Tendril>)>,
    ) {
        let view = ViewId::default();
        doc.ensure_view_init(view);
        let transaction = editor_core::Transaction::change(doc.text(), changes);
        crate::editing::apply(doc, &transaction, view).unwrap();
    }

    #[test]
    fn plain_edits_keep_empty_cache_but_new_markers_invalidate_it() {
        let mut doc = document("préfix\nordinary text\n");
        assert!(!doc.conflict_presentation().active());
        edit(&mut doc, [(7, 8, Some("x".into()))].into_iter());
        assert!(doc.conflicts.get().is_some());
        assert!(!doc.conflict_presentation().active());

        for (source, start, end, replacement) in [
            ("préfix\n<<<<<< HEAD\n", 7, 7, "<"),
            ("préfix\n<<<x<<<< HEAD\n", 10, 11, ""),
            ("préfix >>>>>>> topic\n", 7, 7, "\n"),
            ("préfix\n<<<<<<<\r\rx\n", 16, 17, ""),
            (
                "préfix\nordinary\n",
                7,
                7,
                "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n",
            ),
        ] {
            let mut doc = document(source);
            assert!(!doc.conflict_presentation().active(), "{source}");
            edit(
                &mut doc,
                [(start, end, Some(replacement.into()))].into_iter(),
            );
            assert!(doc.conflicts.get().is_none(), "{source}");
            assert!(doc.conflict_presentation().active(), "{source}");
        }

        let mut doc = document("préfix\n<<<<<< HEAD\n");
        assert!(!doc.conflict_presentation().active());
        edit(
            &mut doc,
            [(0, 1, Some("界".into())), (7, 7, Some("<".into()))].into_iter(),
        );
        assert!(doc.conflict_presentation().active());
    }

    #[test]
    fn shrinking_below_review_limit_rechecks_previously_hidden_conflicts() {
        let conflict = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n";
        let mut doc = document(&format!("{conflict}{}", "x".repeat(vcs::MAX_REVIEW_BYTES)));
        assert!(!doc.conflict_presentation().active());
        let end = doc.text().len_chars();
        edit(&mut doc, [(conflict.len(), end, None)].into_iter());
        assert_eq!(doc.conflict_presentation().regions.len(), 1);
    }

    #[test]
    fn line_numbers_overlap_sections_and_accumulate_across_conflicts() {
        for newline in ["\n", "\r\n"] {
            let source = [
                "préfix",
                "<<<<<<< HEAD",
                "a",
                "b",
                "||||||| base",
                "old a",
                "old b",
                "old c",
                "=======",
                "界",
                "two",
                "three",
                "four",
                ">>>>>>> topic",
                "middle",
                "<<<<<<< HEAD",
                "=======",
                "incoming",
                ">>>>>>> topic",
                "suffix",
            ]
            .join(newline);
            let model = ConflictPresentation::new(&Rope::from(source));
            assert!(model.error.is_none());
            for (physical, displayed) in [
                (0, 0),
                (2, 1),
                (3, 2),
                (5, 1),
                (6, 2),
                (7, 3),
                (9, 1),
                (10, 2),
                (11, 3),
                (12, 4),
                (14, 5),
                (17, 6),
                (19, 7),
            ] {
                assert_eq!(model.line_number(physical), displayed, "line {physical}");
            }
            assert_eq!(model.line_count(20), 8);
        }
    }

    #[test]
    fn line_numbers_use_current_or_base_when_they_are_longest() {
        for source in [
            "<<<<<<< HEAD\na\nb\n||||||| base\nold\n=======\nc\n>>>>>>> topic\nsuffix",
            "<<<<<<< HEAD\na\n||||||| base\nold a\nold b\n=======\nc\n>>>>>>> topic\nsuffix",
        ] {
            let text = Rope::from(source);
            let model = ConflictPresentation::new(&text);
            assert_eq!(
                model.line_number(text.len_lines(stdx::rope::LINE_TYPE) - 1),
                2
            );
            assert_eq!(model.line_count(text.len_lines(stdx::rope::LINE_TYPE)), 3);
        }
        for ending in ["", "\n"] {
            let text = Rope::from(format!(
                "prefix\n<<<<<<< HEAD\n=======\n>>>>>>> topic{ending}"
            ));
            let model = ConflictPresentation::new(&text);
            assert!(model.error.is_none());
            // Empty sides contribute no lines; an optional trailing EOF row remains.
            assert_eq!(
                model.line_count(text.len_lines(stdx::rope::LINE_TYPE)),
                1 + usize::from(!ending.is_empty())
            );
        }
        let model = ConflictPresentation::new(&Rope::from("<<<<<<< HEAD\nbroken\n"));
        assert_eq!(model.line_number(2), 2);
        assert_eq!(model.line_count(3), 3);
    }

    #[test]
    fn two_way_sections_share_numbers_and_label_only_outer_markers() {
        for newline in ["\n", "\r\n"] {
            for (current, incoming) in [(2, 1), (1, 3), (0, 2), (2, 0), (0, 0)] {
                let source = format!(
                    "prefix{newline}<<<<<<< HEAD{newline}{}======={newline}{}>>>>>>> topic{newline}suffix",
                    format!("ours{newline}").repeat(current),
                    format!("界{newline}").repeat(incoming),
                );
                let text = Rope::from(source);
                let model = ConflictPresentation::new(&text);
                assert!(model.error.is_none());
                assert_eq!(model.regions.len(), 1);
                assert!(model.regions[0].base.is_none());
                assert_eq!(model.markers.len(), 3);
                for (start, length) in [(2, current), (3 + current, incoming)] {
                    for offset in 0..length {
                        assert_eq!(model.line_number(start + offset), 1 + offset);
                    }
                }
                assert_eq!(
                    model.line_number(4 + current + incoming),
                    1 + current.max(incoming)
                );
                assert_eq!(
                    model.line_count(text.len_lines(stdx::rope::LINE_TYPE)),
                    2 + current.max(incoming)
                );
                for (labels, marker, expected) in [
                    (&model.labels[0], &model.markers[0], " (Current Change)"),
                    (&model.labels[1], &model.markers[2], " (Incoming Change)"),
                ] {
                    assert_eq!(labels.len(), 1);
                    assert_eq!(labels[0].char_idx, marker.range.end);
                    assert_eq!(labels[0].text.as_str(), expected);
                }
            }
        }
    }

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
            let source = text.slice_chars(marker.range.clone()).to_string();
            let label = match marker.side {
                ConflictSide::Current => Some(" (Current Change)"),
                ConflictSide::End => Some(" (Incoming Change)"),
                _ => None,
            };
            let annotation = model
                .labels
                .iter()
                .flatten()
                .find(|label| label.char_idx == marker.range.end);
            assert_eq!(annotation.map(|label| label.text.as_str()), label);
            assert!(!source.contains(['\r', '\n']));
            assert!(source.len() >= 8);
            let side = match marker.side {
                ConflictSide::Base | ConflictSide::Incoming => None,
                side => Some(side),
            };
            assert_eq!(model.side_at_line(&text, marker.line), side);
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
        for source in [
            "Title\n=======\nordinary text\n",
            "inline <<<<<<< HEAD\nTitle\n=======\n",
            "<<<<<<<word\nTitle\n=======\n",
        ] {
            assert!(!ConflictPresentation::new(&Rope::from(source)).active());
        }
    }
}

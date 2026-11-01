use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::Range;

use crate::doc_formatter::FormattedGrapheme;
use crate::DisplayPosition;
use editor_core::highlight::{Highlight, OverlayHighlights};
use editor_core::{line_ending::line_end_char_index, RopeSlice, Tendril};
use stdx::rope::RopeExt as _;

/// An inline annotation is continuous text shown
/// on the screen before the grapheme that starts at
/// `char_idx`
#[derive(Debug, Clone)]
pub struct InlineAnnotation {
    pub text: Tendril,
    pub char_idx: usize,
}

impl InlineAnnotation {
    pub fn new(char_idx: usize, text: impl Into<Tendril>) -> Self {
        Self {
            char_idx,
            text: text.into(),
        }
    }
}

/// Represents a **single Grapheme** that is part of the document
/// that start at `char_idx` that will be replaced with
/// a different `grapheme`.
/// If `grapheme` contains multiple graphemes the text
/// will render incorrectly.
/// If you want to overlay multiple graphemes simply
/// use multiple `Overlays`.
///
/// # Examples
///
/// The following examples are valid overlays for the following text:
///
/// `aX͎̊͢͜͝͡bc`
///
/// ```
/// use text_layout::text_annotations::Overlay;
///
/// // replaces a
/// Overlay::new(0, "X");
///
/// // replaces X͎̊͢͜͝͡
/// Overlay::new(1, "\t");
///
/// // replaces b
/// Overlay::new(6, "X̢̢̟͖̲͌̋̇͑͝");
/// ```
///
/// The following examples are invalid uses
///
/// ```
/// use text_layout::text_annotations::Overlay;
///
/// // overlay is not aligned at grapheme boundary
/// Overlay::new(3, "x");
///
/// // overlay contains multiple graphemes
/// Overlay::new(0, "xy");
/// ```
#[derive(Debug, Clone)]
pub struct Overlay {
    pub char_idx: usize,
    pub grapheme: Tendril,
}

impl Overlay {
    pub fn new(char_idx: usize, grapheme: impl Into<Tendril>) -> Self {
        Self {
            char_idx,
            grapheme: grapheme.into(),
        }
    }
}

/// A non-empty, single-line display label replacing a source range in characters.
/// The source range must start and end at grapheme boundaries and exclude line endings.
/// Positions inside the range map to the label's anchor; the underlying text is unchanged.
#[derive(Debug, Clone)]
pub struct LineReplacement {
    pub range: Range<usize>,
    pub text: Tendril,
}

/// A character range replaced by a single selectable ellipsis. Ranges must be
/// nonempty, disjoint, sorted, and aligned to grapheme boundaries. Text after the
/// range, including retained closing tokens and line endings, stays in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conceal {
    pub range: Range<usize>,
}

/// Line annotations allow inserting virtual text lines between normal text
/// lines.  These lines can be filled with text in the rendering code as their
/// contents have no effect beyond visual appearance.
///
/// The height of virtual text is usually not known ahead of time as virtual
/// text often requires softwrapping. Furthermore the height of some virtual
/// text like side-by-side diffs depends on the height of the text (again
/// influenced by softwrap) and other virtual text. Therefore line annotations
/// are computed on the fly instead of ahead of time like other annotations.
///
/// The core of this trait `insert_virtual_lines` function. It is called at the
/// end of every  visual line and allows the `LineAnnotation` to insert empty
/// virtual lines. Apart from that the `LineAnnotation` trait has multiple
/// methods that allow it to track anchors in the document.
///
/// When a new traversal of a document starts `reset_pos` is called. Afterwards
/// the other functions are called with indices that are larger then the
/// one passed to `reset_pos`. This allows performing a binary search (use
/// `partition_point`) in `reset_pos` once and then to only look at the next
/// anchor during each method call.
///
/// The `reset_pos`, `skip_conceal` and `process_anchor` functions all return a
/// `char_idx` anchor. This anchor is stored when transversing the document and
/// when the grapheme at the anchor is traversed the `process_anchor` function
/// is called.
///
/// Each traversal clones and resets its initial session. Clones must have independent
/// mutable progress; borrowed immutable input may be shared.
pub trait LineAnnotation {
    /// Resets the internal position to `char_idx`. This function is called
    /// when a new traversal of a document starts.
    ///
    /// All `char_idx` passed to `insert_virtual_lines` are strictly monotonically increasing
    /// with the first `char_idx` greater or equal to the `char_idx`
    /// passed to this function.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn reset_pos(&mut self, _char_idx: usize) -> usize {
        usize::MAX
    }

    /// Called when a text is concealed that contains an anchor registered by this `LineAnnotation`.
    /// In this case the line decorations  **must** ensure that virtual text anchored within that
    /// char range is skipped.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// **after the end of conceal_end_char_idx**
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.reset_pos(conceal_end_char_idx)
    }

    /// Process an anchor (horizontal position is provided) and returns the next anchor.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn process_anchor(&mut self, _grapheme: &FormattedGrapheme) -> usize {
        usize::MAX
    }

    /// This function is called at the end of a visual line to insert virtual text
    ///
    /// # Returns
    ///
    /// The number of additional virtual lines to reserve
    ///
    /// # Note
    ///
    /// The `line_end_visual_pos` parameter indicates the visual vertical distance
    /// from the start of block where the traversal starts.  This includes the offset
    /// from other `LineAnnotations`. This allows inline annotations to consider
    /// the height of the text and "align" two different documents (like for side
    /// by side diffs).  These annotations that want to "align" two documents should
    /// therefore be added last so that other virtual text is also considered while aligning
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        line_end_visual_pos: DisplayPosition,
        doc_line: usize,
    ) -> DisplayPosition;
}

trait LineAnnotationFactory {
    fn create(&self) -> Box<dyn LineAnnotation + '_>;
}

impl<T: LineAnnotation + Clone> LineAnnotationFactory for T {
    fn create(&self) -> Box<dyn LineAnnotation + '_> {
        Box::new(self.clone())
    }
}

#[derive(Debug)]
struct Layer<'a, A, M> {
    annotations: &'a [A],
    metadata: M,
}

impl<'a, A, M> From<(&'a [A], M)> for Layer<'a, A, M> {
    fn from((annotations, metadata): (&'a [A], M)) -> Self {
        Self {
            annotations,
            metadata,
        }
    }
}

struct LayerCursor<'a, A, M> {
    layer: &'a Layer<'a, A, M>,
    current_index: usize,
}

impl<'a, A, M> LayerCursor<'a, A, M> {
    fn new(layer: &'a Layer<'a, A, M>, pos: usize, get_pos: impl Fn(&A) -> usize) -> Self {
        Self {
            layer,
            current_index: layer
                .annotations
                .partition_point(|annotation| get_pos(annotation) < pos),
        }
    }

    fn consume(&mut self, char_idx: usize, get_char_idx: impl Fn(&A) -> usize) -> Option<&'a A> {
        let annotation = self.layer.annotations.get(self.current_index)?;
        debug_assert!(get_char_idx(annotation) >= char_idx);
        if get_char_idx(annotation) == char_idx {
            self.current_index += 1;
            Some(annotation)
        } else {
            None
        }
    }
}

fn reset_pos<A, M>(layers: &mut [LayerCursor<A, M>], pos: usize, get_pos: impl Fn(&A) -> usize) {
    for cursor in layers {
        cursor.current_index = cursor
            .layer
            .annotations
            .partition_point(|annotation| get_pos(annotation) < pos);
    }
}

#[derive(Debug, Clone, Copy)]
enum InlineHighlights<'a> {
    Uniform(Option<Highlight>),
    PerItem(&'a [Highlight]),
}

/// Annotations that change what is displayed when the document is rendered.
/// Also commonly called virtual text.
#[derive(Default)]
pub struct TextAnnotations<'a> {
    replacements: Vec<(&'a LineReplacement, Option<Highlight>)>,
    inline_annotations: Vec<Layer<'a, InlineAnnotation, InlineHighlights<'a>>>,
    overlays: Vec<Layer<'a, Overlay, Option<Highlight>>>,
    conceals: &'a [Conceal],
    line_annotations: Vec<Box<dyn LineAnnotationFactory + 'a>>,
}

impl Debug for TextAnnotations<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextAnnotations")
            .field("inline_annotations", &self.inline_annotations)
            .field("overlays", &self.overlays)
            .finish_non_exhaustive()
    }
}

impl<'a> TextAnnotations<'a> {
    pub(crate) fn is_empty(&self) -> bool {
        self.replacements.is_empty()
            && self.inline_annotations.is_empty()
            && self.overlays.is_empty()
            && self.conceals.is_empty()
            && self.line_annotations.is_empty()
    }

    /// Install disjoint conceal ranges shared by rendering and positioning.
    pub fn set_conceals(&mut self, conceals: &'a [Conceal]) {
        self.conceals = conceals;
    }

    pub fn conceals(&self) -> &[Conceal] {
        self.conceals
    }

    /// The replacement anchor for a position inside concealed text.
    pub fn conceal_at(&self, char_idx: usize) -> Option<&Conceal> {
        let index = self.conceals.partition_point(|c| c.range.end <= char_idx);
        self.conceals
            .get(index)
            .filter(|c| c.range.start <= char_idx)
    }

    /// Find the source checkpoint for a display block. The visible suffix of a
    /// folded region shares its header's block, including chained folds on that
    /// suffix (for example `} else {`).
    pub fn checkpoint_anchor(&self, text: RopeSlice, mut char_idx: usize) -> usize {
        loop {
            let index = self.conceals.partition_point(|c| c.range.start < char_idx);
            let Some(previous) = self.conceals[..index].last() else {
                return char_idx;
            };
            let line_start = text.line_to_char_idx(text.char_to_line_idx(char_idx));
            if previous.range.end < line_start {
                return char_idx;
            }
            char_idx = previous.range.start;
        }
    }

    /// End of the last source line joined into a display block by concealment.
    pub fn block_end(&self, text: RopeSlice, char_idx: usize) -> usize {
        let mut end = line_end_char_index(&text, text.char_to_line_idx(char_idx));
        while let Some(conceal) = self.conceal_at(end) {
            end = line_end_char_index(&text, text.char_to_line_idx(conceal.range.end));
        }
        end
    }

    pub(crate) fn cursor(&self, pos: usize) -> AnnotationCursor<'_> {
        AnnotationCursor {
            inline_annotations: self
                .inline_annotations
                .iter()
                .map(|layer| LayerCursor::new(layer, pos, |annotation| annotation.char_idx))
                .collect(),
            overlays: self
                .overlays
                .iter()
                .map(|layer| LayerCursor::new(layer, pos, |annotation| annotation.char_idx))
                .collect(),
            line_annotations: self
                .line_annotations
                .iter()
                .map(|factory| {
                    let mut session = factory.create();
                    (session.reset_pos(pos), session)
                })
                .collect(),
        }
    }

    pub fn collect_overlay_highlights(&self, char_range: Range<usize>) -> OverlayHighlights {
        let mut highlights = BTreeMap::new();
        for layer in &self.overlays {
            let first = layer
                .annotations
                .partition_point(|overlay| overlay.char_idx < char_range.start);
            for overlay in layer.annotations[first..]
                .iter()
                .take_while(|overlay| overlay.char_idx < char_range.end)
            {
                if self.conceal_at(overlay.char_idx).is_none() {
                    // Last overlay wins, including an overlay without a highlight.
                    highlights.insert(overlay.char_idx, layer.metadata);
                }
            }
        }
        OverlayHighlights::Heterogenous {
            highlights: highlights
                .into_iter()
                .filter_map(|(pos, highlight)| highlight.map(|highlight| (highlight, pos..pos + 1)))
                .collect(),
        }
    }

    /// Add a single-line replacement. Ranges must be sorted and non-overlapping.
    pub fn add_line_replacement(
        &mut self,
        replacement: &'a LineReplacement,
        highlight: Option<Highlight>,
    ) {
        assert!(!replacement.range.is_empty() && !replacement.text.is_empty());
        assert!(!replacement.text.contains(['\n', '\r']));
        assert!(self
            .replacements
            .last()
            .is_none_or(|(last, _)| last.range.end <= replacement.range.start));
        self.replacements.push((replacement, highlight));
    }

    /// Source anchor used to draw a caret within a replacement label.
    pub fn display_anchor(&self, char_idx: usize) -> usize {
        let index = self
            .replacements
            .partition_point(|(replacement, _)| replacement.range.end <= char_idx);
        self.replacements
            .get(index)
            .filter(|(replacement, _)| {
                replacement.range.start <= char_idx
                    && self.replacement_at(replacement.range.start).is_some()
            })
            .map_or(char_idx, |(replacement, _)| replacement.range.start)
    }

    pub(crate) fn replacement_at(
        &self,
        char_idx: usize,
    ) -> Option<(&'a LineReplacement, Option<Highlight>)> {
        let index = self
            .replacements
            .partition_point(|(replacement, _)| replacement.range.start < char_idx);
        self.replacements
            .get(index)
            .copied()
            .filter(|(replacement, _)| {
                let next = self
                    .conceals
                    .partition_point(|conceal| conceal.range.end <= replacement.range.start);
                replacement.range.start == char_idx
                    && self
                        .conceals
                        .get(next)
                        .is_none_or(|conceal| conceal.range.start >= replacement.range.end)
            })
    }

    /// Add new inline annotations.
    ///
    /// The annotations grapheme will be rendered with `highlight`
    /// patched on top of `ui.text`.
    ///
    /// The annotations **must be sorted** by their `char_idx`.
    /// Multiple annotations with the same `char_idx` are allowed,
    /// they will be display in the order that they are present in the layer.
    ///
    /// If multiple layers contain annotations at the same position
    /// the annotations that belong to the layers added first will be shown first.
    pub fn add_inline_annotations(
        &mut self,
        layer: &'a [InlineAnnotation],
        highlight: Option<Highlight>,
    ) -> &mut Self {
        if !layer.is_empty() {
            self.inline_annotations
                .push((layer, InlineHighlights::Uniform(highlight)).into());
        }
        self
    }

    /// Add sorted inline annotations with one highlight per entry.
    ///
    /// The slices must have equal lengths. Same-position ordering follows the
    /// same rules as [`Self::add_inline_annotations`]. Keeping colors in one
    /// layer avoids searching every document swatch for each rendered grapheme.
    pub fn add_inline_annotations_with_highlights(
        &mut self,
        layer: &'a [InlineAnnotation],
        highlights: &'a [Highlight],
    ) -> &mut Self {
        assert_eq!(layer.len(), highlights.len());
        if !layer.is_empty() {
            self.inline_annotations
                .push((layer, InlineHighlights::PerItem(highlights)).into());
        }
        self
    }

    /// Add new grapheme overlays.
    ///
    /// The overlaid grapheme will be rendered with `highlight`
    /// patched on top of `ui.text`.
    ///
    /// The overlays **must be sorted** by their `char_idx`.
    /// Multiple overlays with the same `char_idx` **are allowed**.
    ///
    /// If multiple layers contain overlay at the same position
    /// the overlay from the layer added last will be show.
    pub fn add_overlay(&mut self, layer: &'a [Overlay], highlight: Option<Highlight>) -> &mut Self {
        if !layer.is_empty() {
            self.overlays.push((layer, highlight).into());
        }
        self
    }

    /// Add an initial line annotation session, cloned and reset for each traversal.
    ///
    /// The line annotations **must be sorted** by their `char_idx`.
    /// Multiple line annotations with the same `char_idx` **are not allowed**.
    pub fn add_line_annotation(&mut self, initial: impl LineAnnotation + Clone + 'a) -> &mut Self {
        self.line_annotations.push(Box::new(initial));
        self
    }

    /// Removes all line annotations, useful for vertical motions
    /// so that virtual text lines are automatically skipped.
    pub fn clear_line_annotations(&mut self) {
        self.line_annotations.clear();
    }
}

/// Mutable annotation state owned by one document traversal.
pub(crate) struct AnnotationCursor<'a> {
    inline_annotations: Vec<LayerCursor<'a, InlineAnnotation, InlineHighlights<'a>>>,
    overlays: Vec<LayerCursor<'a, Overlay, Option<Highlight>>>,
    line_annotations: Vec<(usize, Box<dyn LineAnnotation + 'a>)>,
}

impl Debug for AnnotationCursor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnotationCursor")
            .field("inline_layers", &self.inline_annotations.len())
            .field("overlay_layers", &self.overlays.len())
            .field("line_layers", &self.line_annotations.len())
            .finish()
    }
}

impl<'a> AnnotationCursor<'a> {
    /// Hidden inline/overlay entries are skipped without resetting line sessions.
    pub(crate) fn skip_concealed(&mut self, end: usize) {
        reset_pos(&mut self.inline_annotations, end, |annotation| {
            annotation.char_idx
        });
        reset_pos(&mut self.overlays, end, |annotation| annotation.char_idx);
    }

    pub(crate) fn next_inline_annotation_at(
        &mut self,
        char_idx: usize,
    ) -> Option<(&'a InlineAnnotation, Option<Highlight>)> {
        self.inline_annotations.iter_mut().find_map(|layer| {
            let index = layer.current_index;
            let annotation = layer.consume(char_idx, |annot| annot.char_idx)?;
            let highlight = match layer.layer.metadata {
                InlineHighlights::Uniform(highlight) => highlight,
                InlineHighlights::PerItem(highlights) => Some(highlights[index]),
            };
            Some((annotation, highlight))
        })
    }

    pub(crate) fn overlay_at(
        &mut self,
        char_idx: usize,
    ) -> Option<(&'a Overlay, Option<Highlight>)> {
        let mut overlay = None;
        for layer in &mut self.overlays {
            while let Some(new_overlay) = layer.consume(char_idx, |annot| annot.char_idx) {
                overlay = Some((new_overlay, layer.layer.metadata));
            }
        }
        overlay
    }

    pub(crate) fn process_virtual_text_anchors(&mut self, grapheme: &FormattedGrapheme) {
        for (next_anchor, layer) in &mut self.line_annotations {
            loop {
                match (*next_anchor).cmp(&grapheme.char_idx) {
                    Ordering::Less => {
                        *next_anchor = layer.skip_concealed_anchors(grapheme.char_idx)
                    }
                    Ordering::Equal => *next_anchor = layer.process_anchor(grapheme),
                    Ordering::Greater => break,
                };
            }
        }
    }

    pub(crate) fn virtual_lines_at(
        &mut self,
        char_idx: usize,
        line_end_visual_pos: DisplayPosition,
        doc_line: usize,
    ) -> usize {
        let mut virt_off = DisplayPosition::new(0, 0);
        for (_, layer) in &mut self.line_annotations {
            virt_off +=
                layer.insert_virtual_lines(char_idx, line_end_visual_pos + virt_off, doc_line);
        }
        virt_off.row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc_formatter::{DocumentFormatter, TextFormat};
    use editor_core::Rope;

    #[derive(Clone)]
    struct Gap {
        pending: bool,
    }
    impl LineAnnotation for Gap {
        fn reset_pos(&mut self, pos: usize) -> usize {
            self.pending = false;
            if pos <= 3 {
                3
            } else {
                usize::MAX
            }
        }
        fn process_anchor(&mut self, _: &FormattedGrapheme) -> usize {
            self.pending = true;
            usize::MAX
        }
        fn insert_virtual_lines(
            &mut self,
            _: usize,
            _: DisplayPosition,
            _: usize,
        ) -> DisplayPosition {
            DisplayPosition::new(std::mem::take(&mut self.pending) as usize, 0)
        }
    }

    fn signature(g: FormattedGrapheme<'_>) -> (String, usize, DisplayPosition, bool) {
        (g.raw.to_string(), g.char_idx, g.visual_pos, g.is_virtual())
    }

    #[test]
    fn interleaved_traversals_and_overlay_queries_keep_independent_progress() {
        let text = Rope::from("a界\nbé\nz");
        let inline = [
            InlineAnnotation::new(0, "α"),
            InlineAnnotation::new(3, "hint"),
        ];
        let overlays = [Overlay::new(0, "A"), Overlay::new(3, "B")];
        let mut annotations = TextAnnotations::default();
        annotations
            .add_inline_annotations(&inline, None)
            .add_overlay(&overlays, Some(Highlight::new(1)))
            .add_line_annotation(Gap { pending: false });
        for soft_wrap in [false, true] {
            let format = TextFormat {
                soft_wrap,
                viewport_width: 4,
                max_wrap: 0,
                ..TextFormat::default()
            };
            for anchor in [0, 3] {
                for query_overlays in [false, true] {
                    let formatter = |anchor| {
                        DocumentFormatter::new_at_prev_checkpoint(
                            text.slice(..),
                            &format,
                            &annotations,
                            anchor,
                        )
                    };
                    let expected_left: Vec<_> = formatter(0).map(signature).collect();
                    let expected_right: Vec<_> = formatter(anchor).map(signature).collect();
                    let (mut left, mut right) = (formatter(0), formatter(anchor));
                    let (mut actual_left, mut actual_right) = (Vec::new(), Vec::new());
                    for _ in 0..expected_left.len() + expected_right.len() + 1 {
                        let a = left.next();
                        if query_overlays {
                            annotations.collect_overlay_highlights(0..text.len_chars());
                        }
                        let b = right.next();
                        if a.is_none() && b.is_none() {
                            break;
                        }
                        actual_left.extend(a.map(signature));
                        actual_right.extend(b.map(signature));
                    }
                    assert_eq!(actual_left, expected_left);
                    assert_eq!(actual_right, expected_right);
                }
            }
        }
    }

    #[test]
    fn overlay_highlights_keep_last_layer_precedence_and_skip_hidden_ranges() {
        let base = [
            Overlay::new(0, "A"),
            Overlay::new(2, "B"),
            Overlay::new(4, "C"),
        ];
        let clear = [Overlay::new(0, "D"), Overlay::new(2, "E")];
        let top = [Overlay::new(2, "F")];
        let conceals = [Conceal { range: 3..5 }];
        let mut annotations = TextAnnotations::default();
        annotations.set_conceals(&conceals);
        annotations
            .add_overlay(&base, Some(Highlight::new(1)))
            .add_overlay(&clear, None)
            .add_overlay(&top, Some(Highlight::new(2)));
        let OverlayHighlights::Heterogenous { highlights } =
            annotations.collect_overlay_highlights(0..6)
        else {
            panic!("expected per-position highlights")
        };
        assert_eq!(highlights, [(Highlight::new(2), 2..3)]);
    }
}

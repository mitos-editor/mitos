//! Visual movement and scrolling over a document's prepared view layout.

use crate::{Align, Document, View};
use editor_core::movement::{Direction, Movement};
use text_layout::{char_idx_at_visual_offset, visual_offset_from_block};

/// Which line boundaries a vertical motion follows.
#[derive(Clone, Copy)]
pub enum LineMotion {
    Document,
    Visual,
}

/// How scrolling adjusts selections after moving the viewport.
#[derive(Clone, Copy)]
pub enum ScrollCursor {
    /// Move the primary cursor only when it falls outside the scroll margins.
    KeepVisible,
    /// Move every cursor through the same number of visual text rows.
    Follow,
}

/// Move every selection by document lines or wrapped visual rows.
pub fn move_lines(
    doc: &mut Document,
    view: &View,
    lines: LineMotion,
    direction: Direction,
    count: usize,
    behaviour: Movement,
) {
    let mut layout = view.layout(doc, None);
    let move_fn = match lines {
        LineMotion::Document => text_layout::movement::move_vertically,
        LineMotion::Visual => text_layout::movement::move_vertically_visual,
    };
    let selection = doc.selection(view.id).clone().transform(|range| {
        move_fn(
            layout.text,
            range,
            direction,
            count,
            behaviour,
            &layout.format,
            &mut layout.annotations,
        )
    });
    drop(layout);
    doc.set_selection(view.id, selection);
}

/// Scroll by display rows, preserving other views' selections and offsets.
pub fn scroll(
    doc: &mut Document,
    view: &View,
    count: usize,
    direction: Direction,
    cursor: ScrollCursor,
    behaviour: Movement,
    padding: usize,
) {
    let mut layout = view.layout(doc, None);
    let height = layout.viewport.height as usize;
    if height == 0 || count == 0 {
        return;
    }
    let range = doc.selection(view.id).primary();
    let cursor_pos = range.cursor(layout.text);
    let scrolloff = padding.min(height.saturating_sub(1) / 2);
    let count = count.min(isize::MAX as usize) as isize;
    let delta = match direction {
        Direction::Forward => count,
        Direction::Backward => -count,
    };
    (layout.offset.anchor, layout.offset.vertical_offset) = char_idx_at_visual_offset(
        layout.text,
        layout.offset.anchor,
        (layout.offset.vertical_offset as isize).saturating_add(delta),
        0,
        &layout.format,
        &layout.annotations,
    );
    let offset = layout.offset;
    let selection = match cursor {
        ScrollCursor::Follow => {
            // Cursor motion follows text rows; reserved diagnostic rows only affect the viewport.
            Some(doc.selection(view.id).clone().transform(|range| {
                text_layout::movement::move_vertically_visual(
                    layout.text,
                    range,
                    direction,
                    count as usize,
                    behaviour,
                    &layout.format,
                    &mut layout.annotations,
                )
            }))
        }
        ScrollCursor::KeepVisible => {
            let head = match direction {
                Direction::Forward => {
                    let (head, virtual_rows) = char_idx_at_visual_offset(
                        layout.text,
                        offset.anchor,
                        (offset.vertical_offset + scrolloff) as isize,
                        0,
                        &layout.format,
                        &layout.annotations,
                    );
                    let head = if virtual_rows != 0 {
                        editor_core::graphemes::next_grapheme_boundary(layout.text, head)
                    } else {
                        head
                    };
                    (head > cursor_pos).then_some(head)
                }
                Direction::Backward => {
                    let (head, _) = char_idx_at_visual_offset(
                        layout.text,
                        offset.anchor,
                        (offset.vertical_offset + height - scrolloff - 1) as isize,
                        0,
                        &layout.format,
                        &layout.annotations,
                    );
                    (head < cursor_pos).then_some(head)
                }
            };
            head.map(|head| {
                let selection = doc.selection(view.id).clone();
                let primary = selection.primary_index();
                selection.replace(
                    primary,
                    range.put_cursor(layout.text, head, behaviour == Movement::Extend),
                )
            })
        }
    };
    drop(layout);
    doc.set_view_offset(view.id, offset);
    if let Some(selection) = selection {
        doc.set_selection(view.id, selection);
    }
}

/// Move selections to the top, middle, or bottom visible row within the scroll margins.
pub fn goto_window(
    doc: &mut Document,
    view: &View,
    align: Align,
    count: usize,
    behaviour: Movement,
    padding: usize,
) {
    let layout = view.layout(doc, None);
    let height = layout.viewport.height as usize;
    if height == 0 {
        return;
    }
    let scrolloff = padding.min(height.saturating_sub(1) / 2);
    let last = layout.last_visual_line();
    let count = count.saturating_sub(1);
    let row = match align {
        Align::Top => scrolloff.saturating_add(count),
        Align::Center => last / 2,
        Align::Bottom => last.saturating_sub(scrolloff.saturating_add(count)),
    };
    let row = row.max(scrolloff).min(last.saturating_sub(scrolloff));
    let position = layout
        .position_at_view_coords(row as u16, 0, false)
        .expect("virtual rows are allowed for window motion");
    let selection = doc
        .selection(view.id)
        .clone()
        .transform(|range| range.put_cursor(layout.text, position, behaviour == Movement::Extend));
    drop(layout);
    doc.set_selection(view.id, selection);
}

/// Center the primary cursor horizontally when wrapping is disabled.
pub fn align_middle(doc: &mut Document, view: &View) {
    let layout = view.layout(doc, None);
    if layout.format.soft_wrap {
        return;
    }
    let cursor = doc.selection(view.id).primary().cursor(layout.text);
    let position = visual_offset_from_block(
        layout.text,
        layout.offset.anchor,
        cursor,
        &layout.format,
        &layout.annotations,
    )
    .0;
    let mut offset = layout.offset;
    offset.horizontal_offset = position
        .col
        .saturating_sub(layout.viewport.width as usize / 2);
    drop(layout);
    doc.set_view_offset(view.id, offset);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{events::SelectionDidChange, graphics::Rect, view::ViewPosition, ViewId};
    use arc_swap::ArcSwap;
    use editor_config::{Config, GutterConfig};
    use editor_core::{Range, Rope, Selection};
    use std::sync::Arc;

    fn fixture(text: &str, height: u16) -> (Document, View, View) {
        fixture_with_config(text, height, |_| {})
    }

    fn fixture_with_config(
        text: &str,
        height: u16,
        configure: impl FnOnce(&mut Config),
    ) -> (Document, View, View) {
        event::register_event::<SelectionDidChange>();
        let mut config = Config::default();
        config.statusline.global = true;
        config.soft_wrap.enable = Some(true);
        config.soft_wrap.max_wrap = Some(0);
        config.soft_wrap.max_indent_retain = Some(0);
        configure(&mut config);
        let mut doc = Document::from(
            Rope::from_str(text),
            None,
            Arc::new(ArcSwap::from_pointee(config)),
            Arc::new(language::syntax::Loader::default()),
        );
        let mut ids = slotmap::SlotMap::<ViewId, ()>::with_key();
        let mut make_view = || {
            let mut view = View::new(
                doc.id(),
                GutterConfig {
                    layout: Vec::new(),
                    ..Default::default()
                },
                event::Redraw::default(),
            );
            view.id = ids.insert(());
            view.area = Rect::new(0, 0, 12, height);
            view
        };
        let first = make_view();
        let second = make_view();
        doc.ensure_view_init(first.id);
        doc.ensure_view_init(second.id);
        (doc, first, second)
    }

    #[tokio::test]
    async fn vertical_motion_distinguishes_document_lines_and_wrapped_rows() {
        let (mut doc, view, _) = fixture("abcdefghijklmnopqrstuvwx\nshort\n", 3);
        doc.set_selection(view.id, Selection::point(2));
        move_lines(
            &mut doc,
            &view,
            LineMotion::Visual,
            Direction::Forward,
            1,
            Movement::Move,
        );
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            14
        );
        move_lines(
            &mut doc,
            &view,
            LineMotion::Visual,
            Direction::Backward,
            1,
            Movement::Move,
        );
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2
        );
        move_lines(
            &mut doc,
            &view,
            LineMotion::Document,
            Direction::Forward,
            1,
            Movement::Move,
        );
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            27
        );
        move_lines(
            &mut doc,
            &view,
            LineMotion::Document,
            Direction::Backward,
            1,
            Movement::Extend,
        );
        assert_eq!(doc.selection(view.id).primary().anchor, 28);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            2
        );
    }

    #[tokio::test]
    async fn scrolling_preserves_other_views_and_extends_only_the_origin_selection() {
        let (mut doc, view, other) = fixture("0\n1\n2\n3\n4\n5\n6\n", 3);
        doc.set_selection(view.id, Selection::point(0));
        doc.set_selection(other.id, Selection::point(5));
        scroll(
            &mut doc,
            &view,
            2,
            Direction::Forward,
            ScrollCursor::KeepVisible,
            Movement::Extend,
            0,
        );
        assert_eq!(doc.view_offset(view.id).anchor, 4);
        assert_eq!(doc.selection(view.id).primary().anchor, 0);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            4
        );
        assert_eq!(
            doc.selection(other.id)
                .primary()
                .cursor(doc.text().slice(..)),
            5
        );
        assert_eq!(doc.view_offset(other.id), ViewPosition::default());
    }

    #[tokio::test]
    async fn window_motion_counts_scrolled_wrapped_rows_once() {
        let (mut doc, view, _) = fixture("abcdefghijklmnopqrstuvwxyz0123456789\nlast", 2);
        let offset = ViewPosition {
            vertical_offset: 1,
            ..Default::default()
        };
        doc.set_view_offset(view.id, offset);
        goto_window(&mut doc, &view, Align::Top, 1, Movement::Move, 0);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            12
        );
        goto_window(&mut doc, &view, Align::Bottom, 1, Movement::Move, 0);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            24
        );
        assert_eq!(doc.view_offset(view.id), offset);
    }

    #[tokio::test]
    async fn scrolling_past_diagnostic_rows_preserves_line_ending_boundaries() {
        use diagnostics::{Diagnostic, DiagnosticProvider, Severity};
        use editor_config::DiagnosticFilter;
        for newline in ["\n", "\r\n"] {
            let text = format!("é{newline}ab{newline}cd");
            let (mut doc, view, _) = fixture_with_config(&text, 3, |config| {
                config.diagnostics.inline.min_diagnostic_width = 4;
                config.diagnostics.inline.cursor_line = DiagnosticFilter::Enable(Severity::Warning);
                config.diagnostics.inline.other_lines = DiagnosticFilter::Enable(Severity::Warning);
            });
            doc.replace_diagnostics(
                [Diagnostic {
                    range: diagnostics::Range { start: 0, end: 1 },
                    ends_at_word: false,
                    starts_at_word: false,
                    zero_width: false,
                    line: 0,
                    message: "warning".into(),
                    severity: Some(Severity::Warning),
                    code: None,
                    provider: DiagnosticProvider::Spelling,
                    tags: Vec::new(),
                    source: None,
                    data: None,
                }],
                &[],
                None,
            );
            scroll(
                &mut doc,
                &view,
                1,
                Direction::Forward,
                ScrollCursor::KeepVisible,
                Movement::Move,
                0,
            );
            assert_eq!(
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                1 + newline.len()
            );
        }
    }

    #[tokio::test]
    async fn following_the_viewport_moves_all_cursors_and_preserves_the_primary_range() {
        let (mut doc, view, _) = fixture("0\n1\n2\n3\n4\n", 3);
        doc.set_selection(
            view.id,
            Selection::new(vec![Range::point(0), Range::point(4)].into(), 1),
        );
        scroll(
            &mut doc,
            &view,
            1,
            Direction::Forward,
            ScrollCursor::Follow,
            Movement::Move,
            0,
        );
        let cursors: Vec<_> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor(doc.text().slice(..)))
            .collect();
        assert_eq!(cursors, [2, 6]);
        assert_eq!(doc.selection(view.id).primary_index(), 1);
        assert_eq!(doc.view_offset(view.id).anchor, 2);
    }

    #[tokio::test]
    async fn scrolling_empty_viewports_and_large_counts_keeps_positions_valid() {
        let (mut doc, mut view, _) = fixture("abc\ndef", 0);
        let before = doc.selection(view.id).clone();
        scroll(
            &mut doc,
            &view,
            1,
            Direction::Backward,
            ScrollCursor::KeepVisible,
            Movement::Move,
            5,
        );
        assert_eq!(*doc.selection(view.id), before);
        assert_eq!(doc.view_offset(view.id), ViewPosition::default());
        view.area.height = 1;
        scroll(
            &mut doc,
            &view,
            usize::MAX,
            Direction::Forward,
            ScrollCursor::KeepVisible,
            Movement::Move,
            5,
        );
        assert!(doc.view_offset(view.id).anchor <= doc.text().len_chars());
        assert!(doc.selection(view.id).primary().head <= doc.text().len_chars());
    }
}

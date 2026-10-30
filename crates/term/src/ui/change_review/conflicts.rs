//! Source previews share the editor's formatter, syntax, and conflict decorations.
use crate::{
    compositor::Context,
    ui::{
        document::render_document,
        text_decorations::{self, DecorationManager},
    },
};
use std::cell::RefCell;
use text_layout::{
    char_idx_at_visual_offset, doc_formatter::TextFormat, text_annotations::TextAnnotations,
    visual_offset_from_block,
};
use tui::buffer::Buffer;
use view::{graphics::Rect, Document};

pub(super) struct ConflictView {
    pub title: String,
    pub doc: Document,
    height: RefCell<Option<(TextFormat, usize)>>,
}

impl ConflictView {
    pub fn new(title: String, doc: Document) -> Self {
        Self {
            title,
            doc,
            height: RefCell::new(None),
        }
    }

    pub fn visual_height(&self, width: u16, theme: &theme::Theme) -> usize {
        if width == 0 {
            return 0;
        }
        let format = self.doc.text_format(width, Some(theme));
        if let Some((_, height)) = self
            .height
            .borrow()
            .as_ref()
            .filter(|(old, _)| *old == format)
        {
            return *height;
        }
        let mut annotations = TextAnnotations::default();
        self.doc
            .annotate_conflicts(view::ViewId::default(), Some(theme), &mut annotations);
        let height = visual_offset_from_block(
            self.doc.text().slice(..),
            0,
            usize::MAX,
            &format,
            &annotations,
        )
        .0
        .row + 1;
        *self.height.borrow_mut() = Some((format, height));
        height
    }

    pub fn draw(&self, area: Rect, surface: &mut Buffer, cx: &Context, scroll: usize) {
        if area.is_empty() {
            return;
        }
        surface.set_stringn(
            area.x,
            area.y,
            super::literal(&self.title),
            area.width as usize,
            cx.editor.theme.get("ui.text.focus"),
        );
        let viewport = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if viewport.is_empty() {
            return;
        }
        let doc = &self.doc;
        let format = doc.text_format(viewport.width, Some(&cx.editor.theme));
        let mut annotations = TextAnnotations::default();
        doc.annotate_conflicts(
            view::ViewId::default(),
            Some(&cx.editor.theme),
            &mut annotations,
        );
        let (anchor, _) = char_idx_at_visual_offset(
            doc.text().slice(..),
            0,
            scroll.min(isize::MAX as usize) as isize,
            0,
            &format,
            &annotations,
        );
        let row = visual_offset_from_block(doc.text().slice(..), 0, anchor, &format, &annotations)
            .0
            .row;
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(text_decorations::conflicts::line_backgrounds(
            doc,
            &cx.editor.theme,
        ));
        render_document(
            surface,
            doc,
            &view::view::ViewLayout {
                text: doc.text().slice(..),
                viewport,
                offset: view::view::ViewPosition {
                    anchor,
                    vertical_offset: scroll.saturating_sub(row),
                    ..Default::default()
                },
                format,
                annotations,
            },
            Vec::new(),
            &cx.editor.theme,
            decorations,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn preview_height_tracks_wrapped_headings_and_format_changes() {
        let settings: editor_config::Config =
            toml::from_str("[soft-wrap]\nenable = true\n").unwrap();
        let config = Arc::new(arc_swap::ArcSwap::from_pointee(settings));
        let doc = Document::from(
            editor_core::Rope::from("prefix\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n"),
            None,
            config.clone(),
            Arc::new(language::syntax::Loader::default()),
        );
        let view = ConflictView::new("Conflicts".into(), doc);
        let theme = theme::Theme::default();
        assert_eq!(view.visual_height(0, &theme), 0);
        let narrow = view.visual_height(12, &theme);
        assert!(narrow > view.doc.text().len_lines(stdx::rope::LINE_TYPE));
        assert_eq!(view.visual_height(12, &theme), narrow);
        assert!(view.visual_height(120, &theme) < narrow);
        let settings: editor_config::Config =
            toml::from_str("[soft-wrap]\nenable = false\n").unwrap();
        config.store(Arc::new(settings));
        assert!(view.visual_height(12, &theme) < narrow);
    }
}

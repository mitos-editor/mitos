use editor_core::Position;
use view::{
    config::{InlineBlameConfig, InlineBlameShow},
    theme::Style,
    Document, ViewId,
};

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Format only rendered lines, without allocating storage for the entire document.
pub struct InlineBlame<'a> {
    doc: &'a Document,
    config: &'a InlineBlameConfig,
    cursor_line: usize,
    style: Style,
}

impl<'a> InlineBlame<'a> {
    pub fn new(
        doc: &'a Document,
        view: ViewId,
        config: &'a InlineBlameConfig,
        style: Style,
    ) -> Self {
        Self {
            doc,
            config,
            cursor_line: doc.cursor_line(view),
            style,
        }
    }
}

impl Decoration for InlineBlame<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        if !pos.is_last_visual_line
            || self.config.show == InlineBlameShow::Never
            || (self.config.show == InlineBlameShow::CursorLine && pos.doc_line != self.cursor_line)
            || !matches!(self.doc.file_blame(), Some(Ok(_)))
        {
            return Position::new(0, 0);
        }
        let start = virt_off.col.saturating_add(6);
        if !renderer.column_in_bounds(start, 1)
            || self
                .doc
                .text()
                .line(pos.doc_line)
                .chars()
                .all(char::is_whitespace)
        {
            return Position::new(0, 0);
        }
        let Ok(blame) = self
            .doc
            .line_blame(pos.doc_line as u32, &self.config.format)
        else {
            return Position::new(0, 0);
        };
        if blame.is_empty() {
            return Position::new(0, 0);
        }
        let col = (start - renderer.offset.col) as u16;
        let x = renderer.viewport.x + col;
        let (end, _) = renderer.set_string_truncated(
            x,
            pos.visual_line,
            &blame,
            renderer.viewport.width.saturating_sub(col) as usize,
            |_| self.style,
            true,
            false,
        );
        Position::new(0, 6 + end.saturating_sub(x) as usize)
    }
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use std::{process::Command, sync::Arc};

    use arc_swap::ArcSwap;
    use editor_core::{syntax, Selection};
    use tui::buffer::Buffer;
    use view::{
        config::Config,
        graphics::{Rect, RectExt},
        theme,
        view::ViewPosition,
    };

    use super::*;
    use crate::ui::{document::render_document, text_decorations::DecorationManager};

    fn render(
        doc: &Document,
        config: &InlineBlameConfig,
        offset: ViewPosition,
        area: Rect,
    ) -> Vec<String> {
        let mut surface = Buffer::empty(area);
        let theme = theme::Loader::new(loader::theme::Resources::new(vec![])).default_theme();
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(InlineBlame::new(
            doc,
            ViewId::default(),
            config,
            Style::default(),
        ));
        render_document(
            &mut surface,
            area,
            doc,
            offset,
            &Default::default(),
            None,
            vec![],
            &theme,
            decorations,
        );
        surface
            .content
            .chunks(area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    #[tokio::test]
    async fn renders_visible_blame_once_per_document_line_and_clips_scrolled_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "one\n\nthree\n").unwrap();
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec!["commit", "-m", "COMMIT"],
        ] {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env_remove("GIT_DIR")
                .env("GIT_AUTHOR_NAME", "Author")
                .env("GIT_AUTHOR_EMAIL", "author@example.com")
                .env("GIT_COMMITTER_NAME", "Author")
                .env("GIT_COMMITTER_EMAIL", "author@example.com")
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
                .env("GIT_CONFIG_VALUE_0", "false")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let config = Arc::new(ArcSwap::from_pointee(Config::default()));
        let mut doc = Document::open(
            &path,
            None,
            true,
            config.clone(),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        )
        .unwrap();
        doc.set_selection(ViewId::default(), Selection::point(0));
        doc.set_file_blame(vcs::FileBlame::try_new(path.clone(), false));
        let mut blame = InlineBlameConfig {
            show: InlineBlameShow::CursorLine,
            format: "{title}".into(),
        };
        let area = Rect::new(3, 2, 40, 4);
        let rows = render(&doc, &blame, ViewPosition::default(), area);
        assert!(rows[0].contains("one       COMMIT"));
        assert!(!rows[1].contains("COMMIT"));
        assert!(!rows[2].contains("COMMIT"));
        blame.show = InlineBlameShow::AllLines;
        let rows = render(&doc, &blame, ViewPosition::default(), area);
        assert!(rows[0].contains("COMMIT"));
        assert!(!rows[1].contains("COMMIT"));
        assert!(rows[2].contains("COMMIT"));
        let offset = ViewPosition {
            horizontal_offset: 30,
            ..Default::default()
        };
        assert!(render(&doc, &blame, offset, area)
            .iter()
            .all(|row| !row.contains("COMMIT")));

        // Soft wrapping must draw blame only when the document line ends onscreen.
        std::fs::write(&path, "1234567890".repeat(6)).unwrap();
        let mut settings = Config::default();
        settings.soft_wrap.enable = Some(true);
        config.store(Arc::new(settings));
        let mut wrapped = Document::open(
            &path,
            None,
            true,
            config,
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        )
        .unwrap();
        wrapped.set_selection(ViewId::default(), Selection::point(0));
        wrapped.set_file_blame(vcs::FileBlame::try_new(path, false));
        let rows = render(&wrapped, &blame, ViewPosition::default(), area);
        assert!(!rows[0].contains("COMMIT"));
        assert_eq!(rows.iter().filter(|row| row.contains("COMMIT")).count(), 1);
        assert!(!render(
            &wrapped,
            &blame,
            ViewPosition::default(),
            area.with_height(1)
        )[0]
        .contains("COMMIT"));
    }
}

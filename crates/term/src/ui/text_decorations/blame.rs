use editor_config::{InlineBlameConfig, InlineBlameShow};
use text_layout::DisplayPosition;
use theme::Style;
use view::{Document, ViewId};

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
        virt_off: DisplayPosition,
    ) -> DisplayPosition {
        if !pos.is_last_visual_line
            || self.config.show == InlineBlameShow::Never
            || (self.config.show == InlineBlameShow::CursorLine && pos.doc_line != self.cursor_line)
            || !matches!(self.doc.file_blame(), Some(Ok(_)))
        {
            return DisplayPosition::new(0, 0);
        }
        let start = virt_off.col.saturating_add(6);
        if !renderer.column_in_bounds(start, 1)
            || self
                .doc
                .text()
                .line(pos.doc_line, stdx::rope::LINE_TYPE)
                .chars()
                .all(char::is_whitespace)
        {
            return DisplayPosition::new(0, 0);
        }
        let Ok(blame) = self
            .doc
            .line_blame(pos.doc_line as u32, &self.config.format)
        else {
            return DisplayPosition::new(0, 0);
        };
        if blame.is_empty() {
            return DisplayPosition::new(0, 0);
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
        DisplayPosition::new(0, 6 + end.saturating_sub(x) as usize)
    }
}

#[cfg(all(test, feature = "git"))]
mod tests {
    use std::{process::Command, sync::Arc};

    use arc_swap::ArcSwap;
    use editor_config::Config;
    use editor_core::Selection;
    use language::syntax;
    use theme;
    use tui::buffer::Buffer;
    use view::{
        graphics::{Rect, RectExt},
        view::ViewPosition,
    };

    use super::*;
    use crate::ui::{document::render_document, text_decorations::DecorationManager};

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
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

    async fn prepare_blame(doc: &mut Document, path: &std::path::Path) {
        doc.refresh_vcs(&vcs::DiffProviderRegistry::default(), false);
        doc.set_file_blame(vcs::FileBlame::try_new(path.to_path_buf(), false));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !doc.diff_handle().unwrap().load().is_current() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

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
            doc,
            &view::view::ViewLayout {
                text: doc.text().slice(..),
                viewport: area,
                offset,
                format: doc.text_format(area.width, Some(&theme)),
                annotations: Default::default(),
            },
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
        std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n").unwrap();
        git(dir.path(), &["init"]);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "COMMIT"]);
        let config = Arc::new(ArcSwap::from_pointee(Config::default()));
        let mut doc = Document::open(
            &path,
            None,
            true,
            config.clone(),
            Arc::new(syntax::Loader::default()),
        )
        .unwrap();
        doc.set_selection(ViewId::default(), Selection::point(0));
        doc.set_file_blame(vcs::FileBlame::try_new(path.clone(), false));
        assert!(matches!(
            doc.line_blame(0, "{title}"),
            Err(view::document::LineBlameError::NotReadyYet)
        ));
        prepare_blame(&mut doc, &path).await;
        let mut blame = InlineBlameConfig {
            show: InlineBlameShow::CursorLine,
            format: "{title}".into(),
            ..Default::default()
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
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "COMMIT"]);
        let mut settings = Config::default();
        settings.soft_wrap.enable = Some(true);
        config.store(Arc::new(settings));
        let mut wrapped = Document::open(
            &path,
            None,
            true,
            config,
            Arc::new(syntax::Loader::default()),
        )
        .unwrap();
        wrapped.set_selection(ViewId::default(), Selection::point(0));
        prepare_blame(&mut wrapped, &path).await;
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

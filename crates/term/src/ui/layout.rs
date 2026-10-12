use tui::layout::{Constraint, Layout};
use view::graphics::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationLayout {
    pub bufferline: Option<Rect>,
    pub editor: Rect,
    pub statusline: Rect,
    pub commandline: Rect,
}

impl ApplicationLayout {
    pub fn new(area: Rect, use_bufferline: bool, use_global_statusline: bool) -> Self {
        // Split from the bottom in two stages so the command line keeps priority
        // over the status line when the terminal is only one row tall.
        let [above_commandline, commandline] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let [main, statusline] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(u16::from(use_global_statusline)),
        ])
        .areas(above_commandline);

        let (bufferline, editor) = if use_bufferline {
            let [bufferline, editor] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(main);
            (Some(bufferline), editor)
        } else {
            (None, main)
        };

        Self {
            bufferline,
            editor,
            statusline,
            commandline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_application_chrome() {
        let area = Rect::new(3, 5, 80, 24);

        let layout = ApplicationLayout::new(area, false, true);
        assert_eq!(None, layout.bufferline);
        assert_eq!(Rect::new(3, 5, 80, 22), layout.editor);
        assert_eq!(Rect::new(3, 27, 80, 1), layout.statusline);
        assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);

        let layout = ApplicationLayout::new(area, true, true);
        assert_eq!(Some(Rect::new(3, 5, 80, 1)), layout.bufferline);
        assert_eq!(Rect::new(3, 6, 80, 21), layout.editor);
        assert_eq!(Rect::new(3, 27, 80, 1), layout.statusline);
        assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);
    }

    #[test]
    fn per_view_statuslines_leave_the_application_row_to_the_views() {
        let area = Rect::new(3, 5, 80, 24);
        for use_bufferline in [false, true] {
            let layout = ApplicationLayout::new(area, use_bufferline, false);
            let offset = u16::from(use_bufferline);
            assert_eq!(Rect::new(3, 5 + offset, 80, 23 - offset), layout.editor);
            assert!(layout.statusline.is_empty());
            assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);
        }
    }

    #[test]
    fn handles_tiny_areas() {
        let area = Rect::new(3, 5, 80, 1);
        for global in [false, true] {
            let layout = ApplicationLayout::new(area, true, global);
            assert_eq!(Some(Rect::new(3, 5, 80, 0)), layout.bufferline);
            assert_eq!(Rect::new(3, 5, 80, 0), layout.editor);
            assert_eq!(Rect::new(3, 5, 80, 0), layout.statusline);
            assert_eq!(Rect::new(3, 5, 80, 1), layout.commandline);
        }
    }
}

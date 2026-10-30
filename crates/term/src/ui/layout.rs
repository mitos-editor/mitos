use editor_config::CommandLineConfig;
use tui::layout::{Constraint, Layout};
use view::graphics::{Rect, RectExt as _};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationLayout {
    pub bufferline: Option<Rect>,
    pub editor: Rect,
    pub statusline: Rect,
    pub commandline: Rect,
}

impl ApplicationLayout {
    pub fn new(
        area: Rect,
        use_bufferline: bool,
        use_global_statusline: bool,
        command_line: CommandLineConfig,
    ) -> Self {
        // The command line keeps the bottom row in tiny terminals. In auto mode
        // it shares that row with the statusline instead of reserving space.
        let commandline = area.clip_top(area.height.saturating_sub(1));
        let above_commandline = area.clip_bottom(command_line.reserved_height());
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

        let layout = ApplicationLayout::new(area, false, true, CommandLineConfig::default());
        assert_eq!(None, layout.bufferline);
        assert_eq!(Rect::new(3, 5, 80, 22), layout.editor);
        assert_eq!(Rect::new(3, 27, 80, 1), layout.statusline);
        assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);

        let layout = ApplicationLayout::new(area, true, true, CommandLineConfig::default());
        assert_eq!(Some(Rect::new(3, 5, 80, 1)), layout.bufferline);
        assert_eq!(Rect::new(3, 6, 80, 21), layout.editor);
        assert_eq!(Rect::new(3, 27, 80, 1), layout.statusline);
        assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);
    }

    #[test]
    fn per_view_statuslines_leave_the_application_row_to_the_views() {
        let area = Rect::new(3, 5, 80, 24);
        for use_bufferline in [false, true] {
            let layout =
                ApplicationLayout::new(area, use_bufferline, false, CommandLineConfig::default());
            let offset = u16::from(use_bufferline);
            assert_eq!(Rect::new(3, 5 + offset, 80, 23 - offset), layout.editor);
            assert!(layout.statusline.is_empty());
            assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);
        }
    }

    #[test]
    fn auto_command_line_shares_the_bottom_statusline() {
        let area = Rect::new(3, 5, 80, 24);
        let command_line = CommandLineConfig {
            show: editor_config::CommandLineShow::Auto,
        };
        for bufferline in [false, true] {
            for global in [false, true] {
                let layout = ApplicationLayout::new(area, bufferline, global, command_line);
                let top = u16::from(bufferline);
                assert_eq!(
                    Rect::new(3, 5 + top, 80, 24 - top - u16::from(global)),
                    layout.editor
                );
                assert_eq!(Rect::new(3, 28, 80, 1), layout.commandline);
                if global {
                    assert_eq!(layout.commandline, layout.statusline);
                } else {
                    assert!(layout.statusline.is_empty());
                    assert_eq!(layout.commandline.bottom(), layout.editor.bottom());
                }
            }
        }
    }

    #[test]
    fn auto_command_line_stays_inside_tiny_areas() {
        let command_line = CommandLineConfig {
            show: editor_config::CommandLineShow::Auto,
        };
        for width in [0, 1, 80] {
            for height in [0, 1, 2, 3] {
                let area = Rect::new(3, 5, width, height);
                for bufferline in [false, true] {
                    for global in [false, true] {
                        let layout = ApplicationLayout::new(area, bufferline, global, command_line);
                        for rect in [layout.editor, layout.statusline, layout.commandline] {
                            assert!(rect.x >= area.x && rect.right() <= area.right());
                            assert!(rect.y >= area.y && rect.bottom() <= area.bottom());
                        }
                        assert_eq!(layout.commandline.height, height.min(1));
                    }
                }
            }
        }
    }

    #[test]
    fn handles_tiny_areas() {
        let area = Rect::new(3, 5, 80, 1);
        for global in [false, true] {
            let layout = ApplicationLayout::new(area, true, global, CommandLineConfig::default());
            assert_eq!(Some(Rect::new(3, 5, 80, 0)), layout.bufferline);
            assert_eq!(Rect::new(3, 5, 80, 0), layout.editor);
            assert_eq!(Rect::new(3, 5, 80, 0), layout.statusline);
            assert_eq!(Rect::new(3, 5, 80, 1), layout.commandline);
        }
    }
}

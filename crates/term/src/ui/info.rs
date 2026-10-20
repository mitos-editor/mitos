use crate::compositor::{Component, Context};
use tui::buffer::Buffer as Surface;
use tui::layout::{Constraint, Layout};
use tui::text::Text;
use tui::widgets::{Paragraph, Widget};
use view::graphics::Rect;
use view::info::Info;

use super::panel;

impl Component for Info {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let text_style = cx.editor.theme.get("ui.text.info");
        let popup_style = cx.editor.theme.get("ui.popup.info");

        let width = self.width + 2 + 2; // +2 for border, +2 for margin
        let height = self.height + 2; // +2 for border
        let [_left, area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(viewport);
        let [_top, area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(area);
        surface.clear_with(area, popup_style);

        let block = panel::horizontally_padded(&cx.editor.theme, cx.editor.config().border_style)
            .title(self.title.as_ref());

        let inner = block.inner(area);
        block.render(area, surface);

        Paragraph::new(Text::from(self.text.as_str()))
            .style(text_style)
            .render(inner, surface);
    }
}
use tui::buffer::BufferExt as _;

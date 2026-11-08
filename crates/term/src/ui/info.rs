use crate::compositor::Component;
use crate::compositor::DrawContext;
use tui::buffer::Buffer as Surface;
use tui::layout::{Constraint, Layout};
use tui::text::Text;
use tui::widgets::{Paragraph, Widget};
use view::graphics::Rect;
use view::info::Info;

use super::panel;

impl Component for Info {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        render(self, viewport, surface, cx);
    }
}
use tui::buffer::BufferExt as _;

pub(super) fn render(info: &Info, viewport: Rect, surface: &mut Surface, cx: &DrawContext) {
    let text_style = cx.editor.theme.get("ui.text.info");
    let popup_style = cx.editor.theme.get("ui.popup.info");

    let width = info.width + 2 + 2; // +2 for border, +2 for margin
    let height = info.height + 2; // +2 for border
    let [_left, area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(viewport);
    let [_top, area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(area);
    cx.hyperlinks.borrow_mut().clear(area);
    surface.clear_with(area, popup_style);

    let block = panel::horizontally_padded(
        &cx.editor.theme,
        panel::PopupBorder::Info.resolve(&cx.editor.config()),
    )
    .title(info.title.as_ref());

    let inner = block.inner(area);
    block.render(area, surface);

    Paragraph::new(Text::from(info.text.as_str()))
        .style(text_style)
        .render(inner, surface);
}

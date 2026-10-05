use std::borrow::Cow;

use tui::{
    buffer::Buffer as Surface,
    layout::{Constraint, Layout},
    widgets::Widget as _,
};
use view::{graphics::Rect, Editor};

use crate::compositor::{Component, Context, Event, EventResult};

use super::{menu::Item, panel, Menu, PromptEvent, Text};

pub struct Select<T: Item> {
    message: Text,
    options: Menu<T>,
}

impl<T: Item> Select<T> {
    pub fn new<M, I, F>(message: M, options: I, data: T::Data, callback: F) -> Self
    where
        M: Into<Cow<'static, str>>,
        I: IntoIterator<Item = T>,
        F: Fn(&mut Editor, &T, PromptEvent) + 'static,
    {
        let message = tui::text::Text::from(message.into()).into();
        let options: Vec<_> = options.into_iter().collect();
        assert!(!options.is_empty());
        let mut menu = Menu::new(options, data, move |editor, option, event| {
            // Options are non-empty (asserted above) and an option is selected by default,
            // so `option` must be Some here.
            let option = &option.unwrap();
            callback(editor, option, event)
        })
        .auto_close(true);
        // Select the first option by default.
        menu.move_down();

        Self {
            message,
            options: menu,
        }
    }
}

impl<T: Item> Component for Select<T> {
    fn owns_cursor(&self) -> bool {
        // Selection is shown by the menu highlight; hide the editor's cursor.
        true
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        self.options.handle_event(event, cx)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let (message_width, message_height) = self.message.required_size(viewport).unwrap();
        let (menu_width, menu_height) = self.options.required_size(viewport).unwrap();
        Some((
            menu_width.max(message_width + 2),
            message_height + menu_height + 2,
        ))
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // +---------------------+
        // | message             |
        // +---------------------+
        //   options menu
        //
        //

        // Limit the text width to 80% of the screen or 80 columns, whichever is
        // smaller.
        let max_width = 80.min(((area.width as u32) * 80u32 / 100) as u16);
        let (message_width, message_height) = self
            .message
            .required_size((max_width, area.height))
            .unwrap();
        let (_, menu_height) = self
            .options
            .required_size((max_width, area.height))
            .unwrap();
        // + 2 for borders and another + 2 for horizontal padding
        let width = message_width + 4;
        let height = message_height + 2 + menu_height;
        let area = area.centered(Constraint::Length(width), Constraint::Length(height));
        let [message_box, menu_area] = Layout::vertical([
            Constraint::Length(message_height + 2),
            Constraint::Length(menu_height),
        ])
        .areas(area);

        // Message
        let background = cx.editor.theme.get("ui.background");
        let text = cx.editor.theme.get("ui.text");
        surface.clear_with(message_box, background.patch(text));
        let block = panel::horizontally_padded(&cx.editor.theme);
        let message_area = block.inner(message_box);
        block.render(message_box, surface);
        self.message.render(message_area, surface, cx);

        // Options menu
        self.options.render(menu_area, surface, cx);
    }
}
use tui::buffer::BufferExt as _;

use crate::keymap::component::MenuAction;
use crate::{
    compositor::{Callback, Component, Compositor, Context, Event, EventResult},
    key, shift,
};
use editor_core::movement::Direction;
use tui::{
    buffer::Buffer as Surface,
    scrollbar,
    widgets::{StatefulWidget, Table},
};
use ui_core::keymap::{KeymapState, Lookup};

pub use tui::widgets::{Cell, Row};

use editor_config::SmartTabConfig;
use tui::layout::Constraint;
use view::{graphics::Rect, Editor};

pub trait Item: Sync + Send + 'static {
    /// Additional editor state that is used for label calculation.
    type Data: Sync + Send + 'static;

    fn format(&self, data: &Self::Data) -> Row<'_>;
}

pub type MenuCallback<T> = Box<dyn Fn(&mut Editor, Option<&T>, MenuEvent)>;

fn close_callback() -> Option<Callback> {
    Some(Box::new(|compositor: &mut Compositor, _| {
        compositor.pop();
    }))
}

pub struct Menu<T: Item> {
    keys: KeymapState,
    options: Vec<T>,
    editor_data: T::Data,

    cursor: Option<usize>,

    /// (index, score)
    matches: Vec<(u32, u32)>,

    widths: Vec<Constraint>,

    callback_fn: MenuCallback<T>,

    scroll: usize,
    size: (u16, u16),
    viewport: (u16, u16),
    recalculate: bool,
    auto_close: bool,
}

impl<T: Item> Menu<T> {
    const LEFT_PADDING: usize = 1;

    // TODO: it's like a slimmed down picker, share code? (picker = menu + prompt with different
    // rendering)
    pub fn new(
        options: Vec<T>,
        editor_data: <T as Item>::Data,
        callback_fn: impl Fn(&mut Editor, Option<&T>, MenuEvent) + 'static,
    ) -> Self {
        let matches = (0..options.len() as u32).map(|i| (i, 0)).collect();
        Self {
            options,
            editor_data,
            matches,
            cursor: None,
            keys: KeymapState::default(),
            widths: Vec::new(),
            callback_fn: Box::new(callback_fn),
            scroll: 0,
            size: (0, 0),
            viewport: (0, 0),
            recalculate: true,
            auto_close: false,
        }
    }

    /// Execute an action directly when the caller already knows the intent.
    /// Internal acceptance must not depend on the user's Enter binding.
    pub(crate) fn execute_action(&mut self, action: MenuAction, cx: &mut Context) -> EventResult {
        let half_page = (self.size.1 as usize / 2).max(1);
        match action {
            MenuAction::Cancel => {
                (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Abort);
                return EventResult::Consumed(close_callback());
            }
            MenuAction::Accept => {
                if let Some(selection) = self.selection() {
                    (self.callback_fn)(cx.editor, Some(selection), MenuEvent::Validate);
                    return EventResult::Consumed(close_callback());
                }
                return EventResult::Ignored(close_callback());
            }
            MenuAction::MoveUp => self.move_by(1, Direction::Backward),
            MenuAction::MoveDown => self.move_down(),
            MenuAction::PageUp => {
                self.move_by(half_page.min(self.matches.len()), Direction::Backward)
            }
            MenuAction::PageDown => self.move_by(half_page, Direction::Forward),
        }
        (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Update);
        EventResult::Consumed(None)
    }

    pub fn reset_cursor(&mut self) {
        self.keys.clear();
        self.cursor = None;
        self.scroll = 0;
        self.recalculate = true;
    }

    pub fn update_options(&mut self) -> (&mut Vec<(u32, u32)>, &mut Vec<T>) {
        self.recalculate = true;
        (&mut self.matches, &mut self.options)
    }

    pub fn ensure_cursor_in_bounds(&mut self) {
        if self.matches.is_empty() {
            self.cursor = None;
            self.scroll = 0;
        } else {
            self.scroll = 0;
            self.recalculate = true;
            if let Some(cursor) = &mut self.cursor {
                *cursor = (*cursor).min(self.matches.len() - 1)
            }
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.matches.clear();

        // reset cursor position
        self.cursor = None;
        self.scroll = 0;
    }

    fn move_by(&mut self, amount: usize, direction: Direction) {
        if self.matches.is_empty() {
            return;
        }
        self.cursor = super::list::move_cursor(
            self.cursor.map(|cursor| cursor as u32),
            self.matches.len() as u32,
            amount as u32,
            direction,
        )
        .map(|cursor| cursor as usize);
        self.adjust_scroll();
    }

    pub fn move_down(&mut self) {
        self.move_by(1, Direction::Forward);
    }

    pub fn auto_close(mut self, auto_close: bool) -> Self {
        self.auto_close = auto_close;
        self
    }

    fn recalculate_size(&mut self, viewport: (u16, u16)) {
        let n = self
            .options
            .first()
            .map(|option| option.format(&self.editor_data).cells.len())
            .unwrap_or_default();
        let max_lens = self.options.iter().fold(vec![0; n], |mut acc, option| {
            let row = option.format(&self.editor_data);
            // maintain max for each column
            for (acc, cell) in acc.iter_mut().zip(row.cells.iter()) {
                let width = cell.content.width();
                if width > *acc {
                    *acc = width;
                }
            }

            acc
        });

        let height = self.matches.len().min(10).min(viewport.1 as usize);
        // do all the matches fit on a single screen?
        let fits = self.matches.len() <= height;

        let mut len = max_lens.iter().sum::<usize>() + n;

        if !fits {
            len += 1; // +1: reserve some space for scrollbar
        }

        len += Self::LEFT_PADDING;
        let width = len.min(viewport.0 as usize);

        self.widths = max_lens
            .into_iter()
            .map(|len| Constraint::Length(len as u16))
            .collect();

        self.size = (width as u16, height as u16);

        // adjust scroll offsets if size changed
        self.adjust_scroll();
        self.recalculate = false;
    }

    fn adjust_scroll(&mut self) {
        if let Some(cursor) = self.cursor {
            self.scroll = super::list::scroll_offset(cursor, self.scroll, self.size.1 as usize);
        }
    }

    pub fn selection(&self) -> Option<&T> {
        self.cursor.and_then(|cursor| {
            self.matches
                .get(cursor)
                .map(|(index, _score)| &self.options[*index as usize])
        })
    }

    pub fn selection_mut(&mut self) -> Option<&mut T> {
        self.cursor.and_then(|cursor| {
            self.matches
                .get(cursor)
                .map(|(index, _score)| &mut self.options[*index as usize])
        })
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }
}

impl<T: Item + PartialEq> Menu<T> {
    pub fn replace_option(&mut self, old_option: &impl PartialEq<T>, new_option: T) {
        for option in &mut self.options {
            if old_option == option {
                *option = new_option;
                break;
            }
        }
    }
}

use super::PromptEvent as MenuEvent;

impl<T: Item + 'static> Component for Menu<T> {
    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let event = match event {
            Event::Key(event) => *event,
            // Menu is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(_) => return EventResult::Consumed(None),
            Event::Paste(_) => {
                self.keys.clear();
                return EventResult::Ignored(None);
            }
            _ => return EventResult::Ignored(None),
        };

        let pending = self.keys.is_pending();
        let actions = match self
            .keys
            .lookup(&cx.config.current.component_keys.menu, event)
        {
            Lookup::Actions(actions) => actions,
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound => {
                if self.auto_close {
                    (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Abort);
                    return EventResult::Ignored(close_callback());
                }
                return EventResult::Ignored(None);
            }
        };

        // Smart tab supersedes the default tab navigation, but explicit actions
        // such as accept or no_op still belong to the menu.
        if !pending
            && (event == key!(Tab) || event == shift!(Tab))
            && matches!(actions, [MenuAction::MoveUp | MenuAction::MoveDown])
            && cx.editor.config().completion.auto_trigger
            && matches!(
                cx.editor.config().indent.smart_tab,
                Some(SmartTabConfig {
                    enable: true,
                    supersede_menu: true,
                })
            )
        {
            return EventResult::Ignored(None);
        }

        for &action in actions {
            match self.execute_action(action, cx) {
                EventResult::Consumed(None) => (),
                result => return result,
            }
        }
        EventResult::Consumed(None)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        if viewport != self.viewport || self.recalculate {
            self.recalculate_size(viewport);
        }

        Some(self.size)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let theme = &cx.editor.theme;
        let style = theme
            .try_get("ui.menu")
            .unwrap_or_else(|| theme.get("ui.text"));
        let selected = theme.get("ui.menu.selected");

        surface.clear_with(area, style);

        let scroll = self.scroll;

        let len = self.matches.len();

        let win_height = area.height as usize;

        let rows = self
            .matches
            .iter()
            .map(|(index, _score)| self.options[*index as usize].format(&self.editor_data));
        let table = Table::new(rows, &self.widths)
            .style(style)
            .row_highlight_style(selected)
            .column_spacing(1)
            .flex(tui::layout::Flex::Legacy);

        use tui::widgets::TableState;

        let mut table_state = TableState::default()
            .with_offset(scroll)
            .with_selected(self.cursor);

        StatefulWidget::render(
            table,
            area.clip_left(Self::LEFT_PADDING as u16).clip_right(1),
            surface,
            &mut table_state,
        );

        if len > win_height {
            scrollbar::render(
                surface,
                area,
                area.height,
                len,
                scroll,
                true,
                theme.get("ui.menu.scroll"),
            );
        }
    }
}
use tui::buffer::BufferExt as _;
use view::graphics::RectExt as _;

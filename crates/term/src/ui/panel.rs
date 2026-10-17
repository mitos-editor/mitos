use theme::Theme;
use tui::widgets::Block;

pub use tui::panel::{
    content_area, top_border, BORDER_INSET, HORIZONTAL_INSET, PADDED_HORIZONTAL_INSET,
    VERTICAL_INSET,
};

pub fn bordered(theme: &Theme) -> Block<'static> {
    tui::panel::bordered(theme.get("ui.text"))
}

pub fn horizontally_padded(theme: &Theme) -> Block<'static> {
    tui::panel::horizontally_padded(theme.get("ui.text"))
}

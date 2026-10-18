//! Convert shared icon data into terminal text.

use theme::icons::Icon;
use tui::text::Span;

pub(crate) fn icon_span(icon: Icon) -> Span<'static> {
    Span::styled(icon.to_string(), icon.style())
}

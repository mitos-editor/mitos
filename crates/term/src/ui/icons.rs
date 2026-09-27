//! Convert shared icon data into terminal text.

use tui::text::Span;
use view::icons::Icon;

pub(crate) fn icon_span(icon: Icon) -> Span<'static> {
    Span::styled(icon.to_string(), icon.style())
}

use editor_config::{BorderStyle, BorderStyleOverride, Config};
use theme::Theme;
use tui::{symbols::border, widgets::Block};
use ui_core::graphics::Style;

pub use tui::panel::{
    content_area, BORDER_INSET, HORIZONTAL_INSET, PADDED_HORIZONTAL_INSET, VERTICAL_INSET,
};

/// The feature owning a floating panel's border, independent of its compositor ID.
#[derive(Debug, Default, Clone, Copy)]
pub enum PopupBorder {
    #[default]
    Global,
    Hover,
    Completion,
    SignatureHelp,
    CodeActions,
    CodeLenses,
    Info,
    InlineBlame,
    CommandLine,
    ShellOutput,
    Search,
}

impl PopupBorder {
    pub fn resolve(self, config: &Config) -> BorderStyle {
        let preference = match self {
            Self::Global => BorderStyleOverride::Inherit,
            Self::Hover => config.lsp.hover.border_style,
            Self::Completion => config.completion.border_style,
            Self::SignatureHelp => config.lsp.signature_help.border_style,
            Self::CodeActions => config.lsp.code_actions.border_style,
            Self::CodeLenses => config.lsp.code_lenses.border_style,
            Self::Info => config.info.border_style,
            Self::InlineBlame => config.inline_blame.border_style,
            Self::CommandLine => config.command_line.border_style,
            Self::ShellOutput => config.shell_output.border_style,
            Self::Search => config.search.border_style,
        };
        preference.resolve(config.border_style)
    }
}

fn border_set(style: BorderStyle) -> border::Set<'static> {
    match style {
        // Keep the border cells as padding so panel layout and scrolling stay consistent.
        BorderStyle::None => border::EMPTY,
        BorderStyle::Plain => border::PLAIN,
        BorderStyle::Rounded => border::ROUNDED,
        BorderStyle::Double => border::DOUBLE,
        BorderStyle::Thick => border::THICK,
        BorderStyle::QuadrantInside => border::QUADRANT_INSIDE,
        BorderStyle::QuadrantOutside => border::QUADRANT_OUTSIDE,
    }
}

fn border_style(theme: &Theme) -> Style {
    theme
        .try_get_exact("ui.border")
        .unwrap_or_else(|| theme.get("ui.text"))
}

pub fn bordered(theme: &Theme, style: BorderStyle) -> Block<'static> {
    tui::panel::bordered(border_style(theme), border_set(style))
}

pub fn horizontally_padded(theme: &Theme, style: BorderStyle) -> Block<'static> {
    tui::panel::horizontally_padded(border_style(theme), border_set(style))
}

pub fn top_border(style: Style, border_style: BorderStyle) -> Block<'static> {
    tui::panel::top_border(style, border_set(border_style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{buffer::Buffer, layout::Rect, widgets::Widget};
    use ui_core::graphics::Color;

    #[test]
    fn panel_border_scope_overrides_text_with_compatible_fallback() {
        let area = Rect::new(0, 0, 7, 5);
        for (scopes, expected) in [
            (toml::toml! { "ui.text" = "red" }, Color::Red),
            (
                toml::toml! {
                    "ui" = "green"
                    "ui.text" = "red"
                },
                Color::Red,
            ),
            (
                toml::toml! {
                    "ui.text" = "red"
                    "ui.border" = "blue"
                },
                Color::Blue,
            ),
        ] {
            let theme = Theme::from(toml::Value::Table(scopes));
            for block in [
                bordered(&theme, BorderStyle::Rounded),
                horizontally_padded(&theme, BorderStyle::Rounded),
            ] {
                let mut surface = Buffer::empty(area);
                surface.set_style(area, theme.get("ui.text"));
                block.render(area, &mut surface);
                assert_eq!(surface[(0, 0)].symbol(), "╭");
                assert_eq!(surface[(0, 0)].fg, expected);
                assert_eq!(surface[(3, 2)].fg, Color::Red);
            }
        }
    }

    #[test]
    fn border_styles_select_the_requested_symbol_set() {
        assert_eq!(border_set(BorderStyle::None), border::EMPTY);
        assert_eq!(border_set(BorderStyle::Double), border::DOUBLE);
    }
}

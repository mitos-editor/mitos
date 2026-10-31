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

pub fn bordered(theme: &Theme, style: BorderStyle) -> Block<'static> {
    tui::panel::bordered(theme.get("ui.text"), border_set(style))
}

pub fn horizontally_padded(theme: &Theme, style: BorderStyle) -> Block<'static> {
    tui::panel::horizontally_padded(theme.get("ui.text"), border_set(style))
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
    fn border_choice_survives_theme_changes_and_uses_theme_colors() {
        let area = Rect::new(0, 0, 5, 3);
        let config = editor_config::Config {
            border_style: BorderStyle::Rounded,
            ..editor_config::Config::default()
        };

        for (name, color) in [("red", Color::Red), ("blue", Color::Blue)] {
            let theme = Theme::from(toml::Value::Table(toml::toml! {
                "ui.text" = name
                "ui.window" = name
            }));
            let mut surface = Buffer::empty(area);
            bordered(&theme, config.border_style).render(area, &mut surface);
            assert_eq!(surface[(0, 0)].symbol(), "╭");
            assert_eq!(surface[(0, 0)].fg, color);

            top_border(theme.get("ui.window"), config.border_style).render(area, &mut surface);
            assert_eq!(surface[(0, 0)].symbol(), "─");
            assert_eq!(surface[(0, 0)].fg, color);
        }
    }

    #[test]
    fn panels_follow_border_setting_changes() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 5, 3);
        for (style, corner, horizontal) in [
            (BorderStyle::None, " ", " "),
            (BorderStyle::Plain, "┌", "─"),
            (BorderStyle::Rounded, "╭", "─"),
            (BorderStyle::Double, "╔", "═"),
            (BorderStyle::Thick, "┏", "━"),
            (BorderStyle::QuadrantInside, "▗", "▄"),
            (BorderStyle::QuadrantOutside, "▛", "▀"),
        ] {
            let mut surface = Buffer::empty(area);
            let block = horizontally_padded(&theme, style);
            assert_eq!(block.inner(area), Rect::new(2, 1, 1, 1));
            block.render(area, &mut surface);
            assert_eq!(surface[(0, 0)].symbol(), corner);
            assert_eq!(surface[(1, 0)].symbol(), horizontal);

            top_border(Style::default(), style).render(area, &mut surface);
            assert_eq!(surface[(0, 0)].symbol(), horizontal);
        }
    }
}

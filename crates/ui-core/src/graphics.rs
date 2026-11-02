pub use ratatui_core::layout::{Margin, Rect};
pub use ratatui_core::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
/// A visible cursor shape. Visibility and rendering ownership are separate.
#[derive(Default)]
pub enum CursorKind {
    /// █
    #[default]
    Block,
    /// |
    Bar,
    /// _
    Underline,
}

/// Mitos-specific rectangle conveniences layered on Ratatui's geometry type.
pub trait RectExt {
    fn clip_left(self, width: u16) -> Rect;
    fn clip_right(self, width: u16) -> Rect;
    fn clip_top(self, height: u16) -> Rect;
    fn clip_bottom(self, height: u16) -> Rect;
    fn with_height(self, height: u16) -> Rect;
}

impl RectExt for Rect {
    fn clip_left(self, width: u16) -> Rect {
        let width = width.min(self.width);
        Rect {
            x: self.x.saturating_add(width),
            width: self.width.saturating_sub(width),
            ..self
        }
    }

    fn clip_right(self, width: u16) -> Rect {
        Rect {
            width: self.width.saturating_sub(width),
            ..self
        }
    }

    fn clip_top(self, height: u16) -> Rect {
        let height = height.min(self.height);
        Rect {
            y: self.y.saturating_add(height),
            height: self.height.saturating_sub(height),
            ..self
        }
    }

    fn clip_bottom(self, height: u16) -> Rect {
        Rect {
            height: self.height.saturating_sub(height),
            ..self
        }
    }

    fn with_height(self, height: u16) -> Rect {
        Self::new(self.x, self.y, self.width, height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedHex {
    NoHash,
    LenOOB,
    NotANibble,
}
impl fmt::Display for MalformedHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Malformed hex color code: {}",
            match self {
                Self::NoHash => "Missing hash prefix",
                Self::LenOOB => "Must be 12 or 24 bit RGB",
                Self::NotANibble => "One or more chars is not hex digit (nibble)",
            }
        )
    }
}

/// Parse the hex color forms supported by Mitos themes.
pub trait ColorExt {
    fn from_hex(value: &str) -> Result<Self, MalformedHex>
    where
        Self: Sized;
}

impl ColorExt for Color {
    /// Creates a `Color` from a hex string of the form
    /// "#RRGGBB" or "#RGB"
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ui_core::graphics::{Color, ColorExt};
    ///
    /// let color1 = Color::from_hex("#c0ffee").unwrap();
    /// let color2 = Color::Rgb(192, 255, 238);
    ///
    /// assert_eq!(color1, color2);
    ///
    /// let color3 = Color::from_hex("#012").unwrap();
    /// assert_eq!(color3, Color::Rgb(0, 17, 34));
    /// ```
    fn from_hex(value: &str) -> Result<Self, MalformedHex> {
        let digits = value.strip_prefix('#').ok_or(MalformedHex::NoHash)?;
        if !matches!(digits.len(), 3 | 6) {
            return Err(MalformedHex::LenOOB);
        }
        // Integer parsing also accepts signs; theme colors accept only ASCII hex digits.
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(MalformedHex::NotANibble);
        }
        let rgb = u32::from_str_radix(digits, 16).map_err(|_| MalformedHex::NotANibble)?;
        let (red, green, blue) = if digits.len() == 3 {
            (
                ((rgb >> 8) & 0xf) * 17,
                ((rgb >> 4) & 0xf) * 17,
                (rgb & 0xf) * 17,
            )
        } else {
            (rgb >> 16, (rgb >> 8) & 0xff, rgb & 0xff)
        };
        Ok(Self::Rgb(red as u8, green as u8, blue as u8))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    Reset,
    Line,
    Curl,
    Dotted,
    Dashed,
    DoubleLine,
}

// Ratatui models whether text is underlined and its color, but not the shape of
// the underline. Preserve Mitos's richer underline styles in currently-unused
// modifier bits so the Mitos terminal backends can render them.
const RATATUI_UNDERLINE_STYLE_MASK: ratatui_core::style::Modifier =
    ratatui_core::style::Modifier::from_bits_retain(0x0e00);

impl UnderlineStyle {
    fn ratatui_modifier(self) -> ratatui_core::style::Modifier {
        let bits = match self {
            Self::Reset | Self::Line => 0,
            Self::Curl => 0x0200,
            Self::Dotted => 0x0400,
            Self::Dashed => 0x0600,
            Self::DoubleLine => 0x0800,
        };
        let shape = ratatui_core::style::Modifier::from_bits_retain(bits);
        if self == Self::Reset {
            shape
        } else {
            shape | ratatui_core::style::Modifier::UNDERLINED
        }
    }

    pub fn from_ratatui_modifier(modifier: ratatui_core::style::Modifier) -> Self {
        if !modifier.contains(ratatui_core::style::Modifier::UNDERLINED) {
            return Self::Reset;
        }
        match (modifier & RATATUI_UNDERLINE_STYLE_MASK).bits() {
            0x0200 => Self::Curl,
            0x0400 => Self::Dotted,
            0x0600 => Self::Dashed,
            0x0800 => Self::DoubleLine,
            _ => Self::Line,
        }
    }
}

impl FromStr for UnderlineStyle {
    type Err = &'static str;

    fn from_str(modifier: &str) -> Result<Self, Self::Err> {
        match modifier {
            "line" => Ok(Self::Line),
            "curl" => Ok(Self::Curl),
            "dotted" => Ok(Self::Dotted),
            "dashed" => Ok(Self::Dashed),
            "double_line" => Ok(Self::DoubleLine),
            _ => Err("Invalid underline style"),
        }
    }
}

#[cfg(feature = "term")]
impl From<UnderlineStyle> for termina::style::Underline {
    fn from(style: UnderlineStyle) -> Self {
        match style {
            UnderlineStyle::Reset => Self::None,
            UnderlineStyle::Line => Self::Single,
            UnderlineStyle::Curl => Self::Curly,
            UnderlineStyle::Dotted => Self::Dotted,
            UnderlineStyle::Dashed => Self::Dashed,
            UnderlineStyle::DoubleLine => Self::Double,
        }
    }
}

#[cfg(all(feature = "term", windows))]
impl From<UnderlineStyle> for crossterm::style::Attribute {
    fn from(style: UnderlineStyle) -> Self {
        match style {
            UnderlineStyle::Line => crossterm::style::Attribute::Underlined,
            UnderlineStyle::Curl => crossterm::style::Attribute::Undercurled,
            UnderlineStyle::Dotted => crossterm::style::Attribute::Underdotted,
            UnderlineStyle::Dashed => crossterm::style::Attribute::Underdashed,
            UnderlineStyle::DoubleLine => crossterm::style::Attribute::DoubleUnderlined,
            UnderlineStyle::Reset => crossterm::style::Attribute::NoUnderline,
        }
    }
}

/// Rich underline shapes transported through reserved Ratatui modifier bits.
/// All ordinary color and style composition is owned by Ratatui.
pub trait StyleExt {
    fn underline_style(self, style: UnderlineStyle) -> Self;
}

impl StyleExt for Style {
    fn underline_style(self, style: UnderlineStyle) -> Self {
        let underline_bits = Modifier::UNDERLINED | RATATUI_UNDERLINE_STYLE_MASK;
        self.remove_modifier(underline_bits)
            .add_modifier(style.ratatui_modifier())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_chop_from_left() {
        let rect = Rect::new(0, 0, 20, 30);
        assert_eq!(Rect::new(10, 0, 10, 30), rect.clip_left(10));
        assert_eq!(
            Rect::new(20, 0, 0, 30),
            rect.clip_left(40),
            "x should be clamped to original width if new width is bigger"
        );
    }

    #[test]
    fn test_rect_chop_from_right() {
        let rect = Rect::new(0, 0, 20, 30);
        assert_eq!(Rect::new(0, 0, 10, 30), rect.clip_right(10));
    }

    #[test]
    fn test_rect_chop_from_top() {
        let rect = Rect::new(0, 0, 20, 30);
        assert_eq!(Rect::new(0, 10, 20, 20), rect.clip_top(10));
        assert_eq!(
            Rect::new(0, 30, 20, 0),
            rect.clip_top(50),
            "y should be clamped to original height if new height is bigger"
        );
    }

    #[test]
    fn test_rect_chop_from_bottom() {
        let rect = Rect::new(0, 0, 20, 30);
        assert_eq!(Rect::new(0, 0, 20, 20), rect.clip_bottom(10));
    }

    #[test]
    fn ratatui_style_preserves_underline_shapes() {
        let styles = [
            UnderlineStyle::Reset,
            UnderlineStyle::Line,
            UnderlineStyle::Curl,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
            UnderlineStyle::DoubleLine,
        ];

        for underline_style in styles {
            let style = Style::default().underline_style(underline_style);
            assert_eq!(
                UnderlineStyle::from_ratatui_modifier(style.add_modifier),
                underline_style
            );
        }

        assert!(
            (ratatui_core::style::Modifier::all() & RATATUI_UNDERLINE_STYLE_MASK).is_empty(),
            "Ratatui assigned one of Mitos's underline transport bits"
        );

        let mut cell = ratatui_core::buffer::Cell::default();
        cell.set_style(Style::default().underline_style(UnderlineStyle::Curl));
        cell.set_style(Style::default().underline_style(UnderlineStyle::Dotted));
        assert_eq!(
            UnderlineStyle::from_ratatui_modifier(cell.modifier),
            UnderlineStyle::Dotted
        );
        cell.set_style(Style::default().underline_style(UnderlineStyle::Reset));
        assert_eq!(
            UnderlineStyle::from_ratatui_modifier(cell.modifier),
            UnderlineStyle::Reset
        );
    }

    #[test]
    fn hex_colors_accept_ascii_hex_and_reject_malformed_values() {
        for (value, expected) in [
            ("#01fe3a", Ok(Color::Rgb(0x01, 0xfe, 0x3a))),
            ("#c3Ff00", Ok(Color::Rgb(0xc3, 0xff, 0))),
            ("#abc", Ok(Color::Rgb(0xaa, 0xbb, 0xcc))),
            ("#ABC", Ok(Color::Rgb(0xaa, 0xbb, 0xcc))),
            ("#000", Ok(Color::Rgb(0, 0, 0))),
            ("#fff", Ok(Color::Rgb(255, 255, 255))),
            ("abc", Err(MalformedHex::NoHash)),
            ("", Err(MalformedHex::NoHash)),
            ("#+a+b+c", Err(MalformedHex::NotANibble)),
            ("#+0+1+2", Err(MalformedHex::NotANibble)),
            ("#+ab", Err(MalformedHex::NotANibble)),
            ("#-ab", Err(MalformedHex::NotANibble)),
            ("#gGz", Err(MalformedHex::NotANibble)),
            ("#00x", Err(MalformedHex::NotANibble)),
            ("#é0", Err(MalformedHex::NotANibble)),
        ] {
            assert_eq!(Color::from_hex(value), expected, "{value}");
        }
        for digits in 0..=10 {
            if !matches!(digits, 3 | 6) {
                assert_eq!(
                    Color::from_hex(&format!("#{}", "0".repeat(digits))),
                    Err(MalformedHex::LenOOB)
                );
            }
        }
    }
}

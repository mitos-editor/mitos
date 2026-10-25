pub use ratatui_core::layout::{Margin, Rect};
pub use ratatui_core::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[must_use]
const fn from_nibble(h: u8) -> u8 {
    match h {
        b'A'..=b'F' => h - b'A' + 10,
        b'a'..=b'f' => h - b'a' + 10,
        b'0'..=b'9' => h - b'0',
        _ => 0xff, // Err
    }
}

/// Decodes nibble, repeating its value on each half,
/// i.e. the value is its own padding.
///
/// # Errors
/// If `h` isn't a nibble
#[must_use]
const fn dupe_from_nibble(mut h: u8) -> Option<u8> {
    h = from_nibble(h);
    if h > 0xf {
        return None;
    }
    Some((h << 4) | h)
}

/// Decodes big-endian nibble-pair.
///
/// # Errors
/// If any byte isn't a nibble
const fn byte_from_hex(mut h: [u8; 2]) -> Option<u8> {
    // reuse memory
    h[0] = from_nibble(h[0]);
    h[1] = from_nibble(h[1]);
    // we could split this in 2 `if`s,
    // to avoid calling `from_nibble`,
    // but that might be slower
    if h[0] > 0xf || h[1] > 0xf {
        return None;
    }
    Some((h[0] << 4) | h[1])
}

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
    fn with_width(self, width: u16) -> Rect;
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

    fn with_width(self, width: u16) -> Rect {
        Self::new(self.x, self.y, width, self.height)
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
    fn from_hex(h: &str) -> Result<Self, MalformedHex> {
        let h = h.as_bytes();
        if !h.starts_with(b"#") {
            return Err(MalformedHex::NoHash);
        }

        use byte_from_hex as pair;
        use dupe_from_nibble as nibble;

        match h.len() {
            7 => match (|| {
                Some(Self::Rgb(
                    pair([h[1], h[2]])?,
                    pair([h[3], h[4]])?,
                    pair([h[5], h[6]])?,
                ))
            })() {
                Some(c) => Ok(c),
                None => Err(MalformedHex::NotANibble),
            },
            4 => match (|| Some(Self::Rgb(nibble(h[1])?, nibble(h[2])?, nibble(h[3])?)))() {
                Some(c) => Ok(c),
                None => Err(MalformedHex::NotANibble),
            },
            _ => Err(MalformedHex::LenOOB),
        }
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
    fn test_rect_size_preservation() {
        for width in 0..256u16 {
            for height in 0..256u16 {
                let rect = Rect::new(0, 0, width, height);
                rect.area(); // Should not panic.
                assert_eq!(rect.width, width);
                assert_eq!(rect.height, height);
            }
        }

        // One dimension below 255, one above. Area below max u16.
        let rect = Rect::new(0, 0, 300, 100);
        assert_eq!(rect.width, 300);
        assert_eq!(rect.height, 100);
    }

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

    fn styles() -> Vec<Style> {
        vec![
            Style::default(),
            Style::default().fg(Color::Yellow),
            Style::default().bg(Color::Yellow),
            Style::default().add_modifier(Modifier::BOLD),
            Style::default().remove_modifier(Modifier::BOLD),
            Style::default().add_modifier(Modifier::ITALIC),
            Style::default().remove_modifier(Modifier::ITALIC),
            Style::default().add_modifier(Modifier::ITALIC | Modifier::BOLD),
            Style::default().remove_modifier(Modifier::ITALIC | Modifier::BOLD),
        ]
    }

    #[test]
    fn combined_patch_gives_same_result_as_individual_patch() {
        let styles = styles();
        for &a in &styles {
            for &b in &styles {
                for &c in &styles {
                    for &d in &styles {
                        let combined = a.patch(b.patch(c.patch(d)));

                        assert_eq!(
                            Style::default().patch(a).patch(b).patch(c).patch(d),
                            Style::default().patch(combined)
                        );
                    }
                }
            }
        }
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
    fn sanity_nibble_lowercase() {
        for i in 0..0x10_u8 {
            let c = format!("{:x}", i);
            assert_eq!(c.len(), 1);
            assert_eq!(
                u8::from_str_radix(&c, 0x10).unwrap(),
                from_nibble(c.as_bytes()[0])
            );
        }
    }
    #[test]
    fn sanity_nibble_uppercase() {
        for i in 0..0x10_u8 {
            let c = format!("{:X}", i);
            assert_eq!(c.len(), 1);
            assert_eq!(
                u8::from_str_radix(&c, 0x10).unwrap(),
                from_nibble(c.as_bytes()[0])
            );
        }
    }

    #[test]
    fn sanity_nibble2() {
        assert_eq!(dupe_from_nibble(b'0'), Some(0));
        assert_eq!(dupe_from_nibble(b'1'), Some(0x11));
        assert_eq!(dupe_from_nibble(b'7'), Some(0x77));
        assert_eq!(dupe_from_nibble(b'a'), Some(0xaa));
        assert_eq!(dupe_from_nibble(b'f'), Some(0xff));
    }

    #[test]
    fn invalid_nibble() {
        for c in *b"gGzZ+-" {
            assert_eq!(from_nibble(c), 0xff);
        }
    }

    #[test]
    fn pair_endian() {
        assert_eq!(byte_from_hex(*b"00"), Some(0));
        assert_eq!(byte_from_hex(*b"fF"), Some(0xff));
        assert_eq!(byte_from_hex(*b"c3"), Some(0xc3));
    }
    #[test]
    fn invalid_pair() {
        assert!(byte_from_hex(*b"+1").is_none());
        assert!(byte_from_hex(*b"-1").is_none());
        assert!(byte_from_hex(*b"Gg").is_none());
        assert!(byte_from_hex(*b"0x").is_none());
    }

    #[test]
    fn hex_color_no_regress() {
        assert_eq!(Color::from_hex("#+a+b+c"), Err(MalformedHex::NotANibble));
        assert_eq!(Color::from_hex("#+0+1+2"), Err(MalformedHex::NotANibble));
    }
    #[test]
    fn hex_color_sanity() {
        assert_eq!(Color::from_hex("#01fe3a"), Ok(Color::Rgb(0x01, 0xfe, 0x3a)));
        assert_eq!(Color::from_hex("#abc"), Ok(Color::Rgb(0xaa, 0xbb, 0xcc)));
    }
    #[test]
    fn hex_color_invalid_len() {
        for h in [
            "#0",
            "#00",
            "#0000",
            "#00000",
            "#0000000",
            "#00000000",
            "#000000000",
            "#0000000000",
        ] {
            assert_eq!(Color::from_hex(h), Err(MalformedHex::LenOOB));
        }
    }
}

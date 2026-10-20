use bitflags::bitflags;
pub use ratatui::layout::{Margin, Rect};
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
pub enum Color {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    LightGray,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
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

impl Color {
    /// Creates a `Color` from a hex string of the form
    /// "#RRGGBB" or "#RGB"
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ui_core::theme::Color;
    ///
    /// let color1 = Color::from_hex("#c0ffee").unwrap();
    /// let color2 = Color::Rgb(192, 255, 238);
    ///
    /// assert_eq!(color1, color2);
    ///
    /// let color3 = Color::from_hex("#012").unwrap();
    /// assert_eq!(color3, Color::Rgb(0, 17, 34));
    /// ```
    pub fn from_hex(h: &str) -> Result<Self, MalformedHex> {
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

impl From<Color> for ratatui::style::Color {
    fn from(color: Color) -> Self {
        match color {
            Color::Reset => Self::Reset,
            Color::Black => Self::Black,
            Color::Red => Self::Red,
            Color::Green => Self::Green,
            Color::Yellow => Self::Yellow,
            Color::Blue => Self::Blue,
            Color::Magenta => Self::Magenta,
            Color::Cyan => Self::Cyan,
            Color::Gray => Self::DarkGray,
            Color::LightRed => Self::LightRed,
            Color::LightGreen => Self::LightGreen,
            Color::LightYellow => Self::LightYellow,
            Color::LightBlue => Self::LightBlue,
            Color::LightMagenta => Self::LightMagenta,
            Color::LightCyan => Self::LightCyan,
            Color::LightGray => Self::Gray,
            Color::White => Self::White,
            Color::Rgb(r, g, b) => Self::Rgb(r, g, b),
            Color::Indexed(index) => Self::Indexed(index),
        }
    }
}

#[cfg(feature = "term")]
impl From<Color> for termina::style::ColorSpec {
    fn from(color: Color) -> Self {
        match color {
            Color::Reset => Self::Reset,
            Color::Black => Self::BLACK,
            Color::Red => Self::RED,
            Color::Green => Self::GREEN,
            Color::Yellow => Self::YELLOW,
            Color::Blue => Self::BLUE,
            Color::Magenta => Self::MAGENTA,
            Color::Cyan => Self::CYAN,
            Color::Gray => Self::BRIGHT_BLACK,
            Color::White => Self::BRIGHT_WHITE,
            Color::LightRed => Self::BRIGHT_RED,
            Color::LightGreen => Self::BRIGHT_GREEN,
            Color::LightBlue => Self::BRIGHT_BLUE,
            Color::LightYellow => Self::BRIGHT_YELLOW,
            Color::LightMagenta => Self::BRIGHT_MAGENTA,
            Color::LightCyan => Self::BRIGHT_CYAN,
            Color::LightGray => Self::WHITE,
            Color::Indexed(i) => Self::PaletteIndex(i),
            Color::Rgb(r, g, b) => termina::style::RgbColor::new(r, g, b).into(),
        }
    }
}

#[cfg(all(feature = "term", windows))]
impl From<Color> for crossterm::style::Color {
    fn from(color: Color) -> Self {
        use crossterm::style::Color as CColor;

        match color {
            Color::Reset => CColor::Reset,
            Color::Black => CColor::Black,
            Color::Red => CColor::DarkRed,
            Color::Green => CColor::DarkGreen,
            Color::Yellow => CColor::DarkYellow,
            Color::Blue => CColor::DarkBlue,
            Color::Magenta => CColor::DarkMagenta,
            Color::Cyan => CColor::DarkCyan,
            Color::Gray => CColor::DarkGrey,
            Color::LightRed => CColor::Red,
            Color::LightGreen => CColor::Green,
            Color::LightBlue => CColor::Blue,
            Color::LightYellow => CColor::Yellow,
            Color::LightMagenta => CColor::Magenta,
            Color::LightCyan => CColor::Cyan,
            Color::LightGray => CColor::Grey,
            Color::White => CColor::White,
            Color::Indexed(i) => CColor::AnsiValue(i),
            Color::Rgb(r, g, b) => CColor::Rgb { r, g, b },
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
const RATATUI_UNDERLINE_STYLE_MASK: ratatui::style::Modifier =
    ratatui::style::Modifier::from_bits_retain(0x0e00);

impl UnderlineStyle {
    fn ratatui_modifier(self) -> ratatui::style::Modifier {
        let bits = match self {
            Self::Reset | Self::Line => 0,
            Self::Curl => 0x0200,
            Self::Dotted => 0x0400,
            Self::Dashed => 0x0600,
            Self::DoubleLine => 0x0800,
        };
        let shape = ratatui::style::Modifier::from_bits_retain(bits);
        if self == Self::Reset {
            shape
        } else {
            shape | ratatui::style::Modifier::UNDERLINED
        }
    }

    pub fn from_ratatui_modifier(modifier: ratatui::style::Modifier) -> Self {
        if !modifier.contains(ratatui::style::Modifier::UNDERLINED) {
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

bitflags! {
    /// Modifier changes the way a piece of text is displayed.
    ///
    /// They are bitflags so they can easily be composed.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::Modifier;
    ///
    /// let m = Modifier::BOLD | Modifier::ITALIC;
    /// ```
    #[derive(PartialEq, Eq, Debug, Clone, Copy)]
    pub struct Modifier: u16 {
        const BOLD              = 0b0000_0000_0001;
        const DIM               = 0b0000_0000_0010;
        const ITALIC            = 0b0000_0000_0100;
        const SLOW_BLINK        = 0b0000_0001_0000;
        const RAPID_BLINK       = 0b0000_0010_0000;
        const REVERSED          = 0b0000_0100_0000;
        const HIDDEN            = 0b0000_1000_0000;
        const CROSSED_OUT       = 0b0001_0000_0000;
    }
}

impl From<Modifier> for ratatui::style::Modifier {
    fn from(modifier: Modifier) -> Self {
        let mut result = Self::empty();
        if modifier.contains(Modifier::BOLD) {
            result.insert(Self::BOLD);
        }
        if modifier.contains(Modifier::DIM) {
            result.insert(Self::DIM);
        }
        if modifier.contains(Modifier::ITALIC) {
            result.insert(Self::ITALIC);
        }
        if modifier.contains(Modifier::SLOW_BLINK) {
            result.insert(Self::SLOW_BLINK);
        }
        if modifier.contains(Modifier::RAPID_BLINK) {
            result.insert(Self::RAPID_BLINK);
        }
        if modifier.contains(Modifier::REVERSED) {
            result.insert(Self::REVERSED);
        }
        if modifier.contains(Modifier::HIDDEN) {
            result.insert(Self::HIDDEN);
        }
        if modifier.contains(Modifier::CROSSED_OUT) {
            result.insert(Self::CROSSED_OUT);
        }
        result
    }
}

impl FromStr for Modifier {
    type Err = &'static str;

    fn from_str(modifier: &str) -> Result<Self, Self::Err> {
        match modifier {
            "bold" => Ok(Self::BOLD),
            "dim" => Ok(Self::DIM),
            "italic" => Ok(Self::ITALIC),
            "slow_blink" => Ok(Self::SLOW_BLINK),
            "rapid_blink" => Ok(Self::RAPID_BLINK),
            "reversed" => Ok(Self::REVERSED),
            "hidden" => Ok(Self::HIDDEN),
            "crossed_out" => Ok(Self::CROSSED_OUT),
            _ => Err("Invalid modifier"),
        }
    }
}

/// Style let you control the main characteristics of the displayed elements.
///
/// ```rust
/// # use ui_core::graphics::{Color, Modifier, Style};
/// Style::default()
///     .fg(Color::Black)
///     .bg(Color::Green)
///     .add_modifier(Modifier::ITALIC | Modifier::BOLD);
/// ```
///
/// It represents an incremental change. If you apply the styles S1, S2, S3 to a cell of the
/// terminal buffer, the style of this cell will be the result of the merge of S1, S2 and S3, not
/// just S3.
///
/// ```rust
/// # use ui_core::graphics::{Rect, Color, Modifier, Style};
/// # use ratatui::buffer::Buffer;
/// # use ratatui::style::{Modifier as RatatuiModifier, Style as RatatuiStyle};
/// let styles = [
///     Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD | Modifier::ITALIC),
///     Style::default().bg(Color::Red),
///     Style::default().fg(Color::Yellow).remove_modifier(Modifier::ITALIC),
/// ];
/// let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
/// for style in &styles {
///   buffer[(0, 0)].set_style(*style);
/// }
/// assert_eq!(
///     RatatuiStyle::default()
///         .fg(Color::Yellow.into())
///         .bg(Color::Red.into())
///         .underline_color(Color::Reset.into())
///         .add_modifier(RatatuiModifier::BOLD),
///     buffer[(0, 0)].style(),
/// );
/// ```
///
/// The default implementation returns a `Style` that does not modify anything. If you wish to
/// reset all properties until that point use [`Style::reset`].
///
/// ```
/// # use ui_core::graphics::{Rect, Color, Modifier, Style};
/// # use ratatui::{buffer::Buffer, style::Style as RatatuiStyle};
/// let styles = [
///     Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD | Modifier::ITALIC),
///     Style::reset().fg(Color::Yellow),
/// ];
/// let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
/// for style in &styles {
///   buffer[(0, 0)].set_style(*style);
/// }
/// assert_eq!(
///     RatatuiStyle::default()
///         .fg(Color::Yellow.into())
///         .bg(Color::Reset.into())
///         .underline_color(Color::Reset.into()),
///     buffer[(0, 0)].style(),
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub underline_style: Option<UnderlineStyle>,
    pub add_modifier: Modifier,
    pub sub_modifier: Modifier,
}

impl Default for Style {
    fn default() -> Self {
        Self::new()
    }
}

impl Style {
    pub const fn new() -> Self {
        Style {
            fg: None,
            bg: None,
            underline_color: None,
            underline_style: None,
            add_modifier: Modifier::empty(),
            sub_modifier: Modifier::empty(),
        }
    }

    /// Returns a `Style` resetting all properties.
    pub const fn reset() -> Self {
        Self {
            fg: Some(Color::Reset),
            bg: Some(Color::Reset),
            underline_color: None,
            underline_style: None,
            add_modifier: Modifier::empty(),
            sub_modifier: Modifier::all(),
        }
    }

    /// Changes the foreground color.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{Color, Style};
    /// let style = Style::default().fg(Color::Blue);
    /// let diff = Style::default().fg(Color::Red);
    /// assert_eq!(style.patch(diff), Style::default().fg(Color::Red));
    /// ```
    pub const fn fg(mut self, color: Color) -> Style {
        self.fg = Some(color);
        self
    }

    /// Changes the background color.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{Color, Style};
    /// let style = Style::default().bg(Color::Blue);
    /// let diff = Style::default().bg(Color::Red);
    /// assert_eq!(style.patch(diff), Style::default().bg(Color::Red));
    /// ```
    pub const fn bg(mut self, color: Color) -> Style {
        self.bg = Some(color);
        self
    }

    /// Changes the underline color.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{Color, Style};
    /// let style = Style::default().underline_color(Color::Blue);
    /// let diff = Style::default().underline_color(Color::Red);
    /// assert_eq!(style.patch(diff), Style::default().underline_color(Color::Red));
    /// ```
    pub const fn underline_color(mut self, color: Color) -> Style {
        self.underline_color = Some(color);
        self
    }

    /// Changes the underline style.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{UnderlineStyle, Style};
    /// let style = Style::default().underline_style(UnderlineStyle::Line);
    /// let diff = Style::default().underline_style(UnderlineStyle::Curl);
    /// assert_eq!(style.patch(diff), Style::default().underline_style(UnderlineStyle::Curl));
    /// ```
    pub const fn underline_style(mut self, style: UnderlineStyle) -> Style {
        self.underline_style = Some(style);
        self
    }

    /// Changes the text emphasis.
    ///
    /// When applied, it adds the given modifier to the `Style` modifiers.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{Color, Modifier, Style};
    /// let style = Style::default().add_modifier(Modifier::BOLD);
    /// let diff = Style::default().add_modifier(Modifier::ITALIC);
    /// let patched = style.patch(diff);
    /// assert_eq!(patched.add_modifier, Modifier::BOLD | Modifier::ITALIC);
    /// assert_eq!(patched.sub_modifier, Modifier::empty());
    /// ```
    pub fn add_modifier(mut self, modifier: Modifier) -> Style {
        self.sub_modifier.remove(modifier);
        self.add_modifier.insert(modifier);
        self
    }

    /// Changes the text emphasis.
    ///
    /// When applied, it removes the given modifier from the `Style` modifiers.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// # use ui_core::graphics::{Color, Modifier, Style};
    /// let style = Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC);
    /// let diff = Style::default().remove_modifier(Modifier::ITALIC);
    /// let patched = style.patch(diff);
    /// assert_eq!(patched.add_modifier, Modifier::BOLD);
    /// assert_eq!(patched.sub_modifier, Modifier::ITALIC);
    /// ```
    pub fn remove_modifier(mut self, modifier: Modifier) -> Style {
        self.add_modifier.remove(modifier);
        self.sub_modifier.insert(modifier);
        self
    }

    /// Results in a combined style that is equivalent to applying the two individual styles to
    /// a style one after the other.
    ///
    /// ## Examples
    /// ```
    /// # use ui_core::graphics::{Color, Modifier, Style};
    /// let style_1 = Style::default().fg(Color::Yellow);
    /// let style_2 = Style::default().bg(Color::Red);
    /// let combined = style_1.patch(style_2);
    /// assert_eq!(
    ///     Style::default().patch(style_1).patch(style_2),
    ///     Style::default().patch(combined));
    /// ```
    pub fn patch(mut self, other: Style) -> Style {
        self.fg = other.fg.or(self.fg);
        self.bg = other.bg.or(self.bg);
        self.underline_color = other.underline_color.or(self.underline_color);
        self.underline_style = other.underline_style.or(self.underline_style);

        self.add_modifier.remove(other.sub_modifier);
        self.add_modifier.insert(other.add_modifier);
        self.sub_modifier.remove(other.add_modifier);
        self.sub_modifier.insert(other.sub_modifier);

        self
    }
}

impl From<Style> for ratatui::style::Style {
    fn from(style: Style) -> Self {
        let mut add_modifier: ratatui::style::Modifier = style.add_modifier.into();
        let mut sub_modifier: ratatui::style::Modifier = style.sub_modifier.into();
        match style.underline_style {
            Some(UnderlineStyle::Reset) => {
                add_modifier
                    .remove(ratatui::style::Modifier::UNDERLINED | RATATUI_UNDERLINE_STYLE_MASK);
                sub_modifier
                    .insert(ratatui::style::Modifier::UNDERLINED | RATATUI_UNDERLINE_STYLE_MASK);
            }
            Some(underline_style) => {
                let encoded = underline_style.ratatui_modifier();
                let underline_bits =
                    ratatui::style::Modifier::UNDERLINED | RATATUI_UNDERLINE_STYLE_MASK;
                add_modifier.remove(underline_bits);
                add_modifier.insert(encoded);
                sub_modifier.insert(underline_bits);
                sub_modifier.remove(encoded);
            }
            None => {}
        }

        ratatui::style::Style {
            fg: style.fg.map(Into::into),
            bg: style.bg.map(Into::into),
            underline_color: style.underline_color.map(Into::into),
            add_modifier,
            sub_modifier,
        }
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
            let style =
                ratatui::style::Style::from(Style::default().underline_style(underline_style));
            assert_eq!(
                UnderlineStyle::from_ratatui_modifier(style.add_modifier),
                underline_style
            );
        }

        assert!(
            (ratatui::style::Modifier::all() & RATATUI_UNDERLINE_STYLE_MASK).is_empty(),
            "Ratatui assigned one of Mitos's underline transport bits"
        );

        let mut cell = ratatui::buffer::Cell::default();
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

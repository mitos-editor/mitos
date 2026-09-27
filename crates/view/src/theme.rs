use std::{collections::HashMap, sync::LazyLock};

use anyhow::Result;
use editor_core::{hashmap, syntax::Highlight};
use loader::theme::Resources;
pub use loader::theme::{BASE16_DEFAULT_THEME_DATA, DEFAULT_THEME_DATA};
use log::warn;
use serde::{Deserialize, Deserializer};
use toml::{map::Map, Value};

use ui_core::graphics::UnderlineStyle;
pub use ui_core::{
    graphics::{Color, Modifier, Style},
    theme::Mode,
};

pub static DEFAULT_THEME: LazyLock<Theme> = LazyLock::new(|| Theme {
    name: "default".into(),
    ..Theme::from(DEFAULT_THEME_DATA.clone())
});

pub static BASE16_DEFAULT_THEME: LazyLock<Theme> = LazyLock::new(|| Theme {
    name: "base16_default".into(),
    ..Theme::from(BASE16_DEFAULT_THEME_DATA.clone())
});

pub fn symbol_kind_scope(kind: lsp_client::lsp::SymbolKind) -> &'static str {
    use lsp_client::lsp::SymbolKind;

    match kind {
        SymbolKind::FILE => "ui.text.directory",
        SymbolKind::MODULE | SymbolKind::NAMESPACE | SymbolKind::PACKAGE => "namespace",
        SymbolKind::OBJECT | SymbolKind::STRUCT | SymbolKind::INTERFACE | SymbolKind::CLASS => {
            "type"
        }
        SymbolKind::METHOD => "function.method",
        SymbolKind::FUNCTION => "function",
        SymbolKind::ENUM => "type.enum",
        SymbolKind::ENUM_MEMBER => "type.enum.variant",
        SymbolKind::FIELD | SymbolKind::PROPERTY => "variable.other.member",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "constant.numeric",
        SymbolKind::BOOLEAN => "constant.builtin.boolean",
        SymbolKind::ARRAY => "punctuation.bracket",
        SymbolKind::KEY => "label",
        SymbolKind::NULL => "constant.builtin",
        SymbolKind::EVENT => "function",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type.parameter",
        _ => "ui.text",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged, deny_unknown_fields, rename_all = "kebab-case")]
pub enum Config {
    Constant(String),
    Adaptive {
        light: String,
        dark: String,
        /// A theme to choose when the terminal did not declare either light or dark mode.
        /// When not specified the dark theme is preferred.
        fallback: Option<String>,
    },
}

impl Default for Config {
    fn default() -> Self {
        Self::Adaptive {
            light: "modus_operandi".into(),
            dark: "modus_vivendi".into(),
            fallback: None,
        }
    }
}

impl Config {
    pub fn choose(&self, preference: Option<Mode>) -> &str {
        match self {
            Config::Constant(theme) => theme,
            Config::Adaptive {
                light,
                dark,
                fallback,
            } => match preference {
                Some(Mode::Light) => light,
                Some(Mode::Dark) => dark,
                None => fallback.as_ref().unwrap_or(dark),
            },
        }
    }

    pub fn is_adaptive(&self) -> bool {
        matches!(self, Self::Adaptive { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Loader {
    resources: Resources,
}
impl Loader {
    /// Interpret themes loaded from an explicit set of resource paths.
    pub fn new(resources: Resources) -> Self {
        Self { resources }
    }

    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    /// Loads a theme searching directories in priority order.
    pub fn load(&self, name: &str) -> Result<Theme> {
        let (theme, warnings) = self.load_with_warnings(name)?;

        for warning in warnings {
            warn!("Theme '{}': {}", name, warning);
        }

        Ok(theme)
    }

    /// Loads a theme searching directories in priority order, returning any warnings
    pub fn load_with_warnings(&self, name: &str) -> Result<(Theme, Vec<String>)> {
        if name == "default" {
            return Ok((self.default(), Vec::new()));
        }
        if name == "base16_default" {
            return Ok((self.base16_default(), Vec::new()));
        }

        let (theme, warnings) = Theme::from_toml(self.resources.load(name)?);

        let theme = Theme {
            name: name.into(),
            ..theme
        };
        Ok((theme, warnings))
    }

    pub fn default_theme(&self) -> Theme {
        self.default()
    }

    /// Returns the default theme
    pub fn default(&self) -> Theme {
        DEFAULT_THEME.clone()
    }

    /// Returns the alternative 16-color default theme
    pub fn base16_default(&self) -> Theme {
        BASE16_DEFAULT_THEME.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Theme {
    name: String,

    // UI styles are stored in a HashMap
    styles: HashMap<String, Style>,
    // tree-sitter highlight styles are stored in a Vec to optimize lookups
    scopes: Vec<String>,
    highlights: Vec<Style>,
    /// Reverse map from scope string to its `Highlight` index. `find_highlight_exact`
    /// is called many times per frame, so we optimize lookups.
    scope_index: HashMap<String, Highlight>,
    rainbow_length: usize,
}

impl From<Value> for Theme {
    fn from(value: Value) -> Self {
        let (theme, warnings) = Theme::from_toml(value);
        for warning in warnings {
            warn!("{}", warning);
        }
        theme
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Map::<String, Value>::deserialize(deserializer)?;
        let (theme, warnings) = Theme::from_keys(values);
        for warning in warnings {
            warn!("{}", warning);
        }
        Ok(theme)
    }
}

#[allow(clippy::type_complexity)]
fn build_theme_values(
    mut values: Map<String, Value>,
) -> (
    HashMap<String, Style>,
    Vec<String>,
    Vec<Style>,
    usize,
    Vec<String>,
) {
    let mut styles = HashMap::new();
    let mut scopes = Vec::new();
    let mut highlights = Vec::new();
    let mut rainbow_length = 0;

    let mut warnings = Vec::new();

    // TODO: alert user of parsing failures in editor
    let palette = values
        .remove("palette")
        .map(|value| {
            ThemePalette::try_from(value).unwrap_or_else(|err| {
                warnings.push(err);
                ThemePalette::default()
            })
        })
        .unwrap_or_default();
    // remove inherits from value to prevent errors
    let _ = values.remove("inherits");
    styles.reserve(values.len());
    scopes.reserve(values.len());
    highlights.reserve(values.len());

    for (i, style) in values
        .remove("rainbow")
        .and_then(|value| match palette.parse_style_array(value) {
            Ok(styles) => Some(styles),
            Err(err) => {
                warnings.push(err);
                None
            }
        })
        .unwrap_or_else(default_rainbow)
        .into_iter()
        .enumerate()
    {
        let name = format!("rainbow.{i}");
        styles.insert(name.clone(), style);
        scopes.push(name);
        highlights.push(style);
        rainbow_length += 1;
    }

    for (name, style_value) in values {
        let mut style = Style::default();
        if let Err(err) = palette.parse_style(&mut style, style_value) {
            warnings.push(format!("Failed to parse style for key {name:?}. {err}"));
        }

        // these are used both as UI and as highlights
        styles.insert(name.clone(), style);
        scopes.push(name);
        highlights.push(style);
    }

    (styles, scopes, highlights, rainbow_length, warnings)
}

fn default_rainbow() -> Vec<Style> {
    vec![
        Style::default().fg(Color::Red),
        Style::default().fg(Color::Yellow),
        Style::default().fg(Color::Green),
        Style::default().fg(Color::Blue),
        Style::default().fg(Color::Cyan),
        Style::default().fg(Color::Magenta),
    ]
}
impl Theme {
    /// To allow `Highlight` to represent arbitrary RGB colors without turning it into an enum,
    /// we interpret the last 256^3 numbers as RGB.
    const RGB_START: u32 = (u32::MAX << (8 + 8 + 8)) - 1 - (u32::MAX - Highlight::MAX);

    /// Interpret a Highlight with the RGB foreground
    fn decode_rgb_highlight(highlight: Highlight) -> Option<(u8, u8, u8)> {
        (highlight.get() > Self::RGB_START).then(|| {
            let [b, g, r, ..] = (highlight.get() + 1).to_le_bytes();
            (r, g, b)
        })
    }

    /// Create a Highlight that represents an RGB color
    pub fn rgb_highlight(r: u8, g: u8, b: u8) -> Highlight {
        // -1 because highlight is "non-max": u32::MAX is reserved for the null pointer
        // optimization.
        Highlight::new(u32::from_le_bytes([b, g, r, u8::MAX]) - 1)
    }

    #[inline]
    pub fn highlight(&self, highlight: Highlight) -> Style {
        if let Some((red, green, blue)) = Self::decode_rgb_highlight(highlight) {
            Style::new().fg(Color::Rgb(red, green, blue))
        } else {
            self.highlights[highlight.idx()]
        }
    }

    #[inline]
    pub fn scope(&self, highlight: Highlight) -> &str {
        &self.scopes[highlight.idx()]
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn get(&self, scope: &str) -> Style {
        self.try_get(scope).unwrap_or_default()
    }

    /// Get the style of a scope, falling back to dot separated broader
    /// scopes. For example if `ui.text.focus` is not defined in the theme,
    /// `ui.text` is tried and then `ui` is tried.
    pub fn try_get(&self, scope: &str) -> Option<Style> {
        std::iter::successors(Some(scope), |s| Some(s.rsplit_once('.')?.0))
            .find_map(|s| self.styles.get(s).copied())
    }

    /// Get the style of a scope, without falling back to dot separated broader
    /// scopes. For example if `ui.text.focus` is not defined in the theme, it
    /// will return `None`, even if `ui.text` is.
    pub fn try_get_exact(&self, scope: &str) -> Option<Style> {
        self.styles.get(scope).copied()
    }

    #[inline]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    pub fn find_highlight_exact(&self, scope: &str) -> Option<Highlight> {
        self.scope_index.get(scope).copied()
    }

    pub fn find_highlight(&self, mut scope: &str) -> Option<Highlight> {
        loop {
            if let Some(highlight) = self.find_highlight_exact(scope) {
                return Some(highlight);
            }
            let new_end = scope.rfind('.')?;
            scope = &scope[..new_end];
        }
    }

    pub fn is_16_color(&self) -> bool {
        self.styles.iter().all(|(_, style)| {
            [style.fg, style.bg]
                .into_iter()
                .all(|color| !matches!(color, Some(Color::Rgb(..))))
        })
    }

    pub fn rainbow_length(&self) -> usize {
        self.rainbow_length
    }

    fn from_toml(value: Value) -> (Self, Vec<String>) {
        if let Value::Table(table) = value {
            Theme::from_keys(table)
        } else {
            warn!("Expected theme TOML value to be a table, found {:?}", value);
            Default::default()
        }
    }

    fn from_keys(toml_keys: Map<String, Value>) -> (Self, Vec<String>) {
        let (styles, scopes, highlights, rainbow_length, load_errors) =
            build_theme_values(toml_keys);

        let scope_index = scopes
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), Highlight::new(i as u32)))
            .collect();

        let theme = Self {
            styles,
            scopes,
            highlights,
            scope_index,
            rainbow_length,
            ..Default::default()
        };
        (theme, load_errors)
    }
}

struct ThemePalette {
    palette: HashMap<String, Color>,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            palette: hashmap! {
                "default".to_string() => Color::Reset,
                "black".to_string() => Color::Black,
                "red".to_string() => Color::Red,
                "green".to_string() => Color::Green,
                "yellow".to_string() => Color::Yellow,
                "blue".to_string() => Color::Blue,
                "magenta".to_string() => Color::Magenta,
                "cyan".to_string() => Color::Cyan,
                "gray".to_string() => Color::Gray,
                "light-red".to_string() => Color::LightRed,
                "light-green".to_string() => Color::LightGreen,
                "light-yellow".to_string() => Color::LightYellow,
                "light-blue".to_string() => Color::LightBlue,
                "light-magenta".to_string() => Color::LightMagenta,
                "light-cyan".to_string() => Color::LightCyan,
                "light-gray".to_string() => Color::LightGray,
                "white".to_string() => Color::White,
            },
        }
    }
}

impl ThemePalette {
    pub fn new(palette: HashMap<String, Color>) -> Self {
        let ThemePalette {
            palette: mut default,
        } = ThemePalette::default();

        default.extend(palette);
        Self { palette: default }
    }

    pub fn string_to_rgb(s: &str) -> Result<Color, String> {
        if s.starts_with('#') {
            Color::from_hex(s).map_err(|e| format!("{e}: {s}"))
        } else {
            Self::ansi_string_to_rgb(s)
        }
    }

    fn ansi_string_to_rgb(s: &str) -> Result<Color, String> {
        if let Ok(index) = s.parse::<u8>() {
            return Ok(Color::Indexed(index));
        }
        Err(format!("Malformed ANSI: {}", s))
    }

    fn parse_value_as_str(value: &Value) -> Result<&str, String> {
        value
            .as_str()
            .ok_or(format!("Unrecognized value: {}", value))
    }

    pub fn parse_color(&self, value: Value) -> Result<Color, String> {
        let value = Self::parse_value_as_str(&value)?;

        self.palette
            .get(value)
            .copied()
            .ok_or("")
            .or_else(|_| Self::string_to_rgb(value))
    }

    pub fn parse_modifier(value: &Value) -> Result<Modifier, String> {
        value
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or(format!("Invalid modifier: {}", value))
    }

    pub fn parse_underline_style(value: &Value) -> Result<UnderlineStyle, String> {
        value
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or(format!("Invalid underline style: {}", value))
    }

    pub fn parse_style(&self, style: &mut Style, value: Value) -> Result<(), String> {
        if let Value::Table(entries) = value {
            for (name, mut value) in entries {
                match name.as_str() {
                    "fg" => *style = style.fg(self.parse_color(value)?),
                    "bg" => *style = style.bg(self.parse_color(value)?),
                    "underline" => {
                        let table = value.as_table_mut().ok_or("Underline must be table")?;
                        if let Some(value) = table.remove("color") {
                            *style = style.underline_color(self.parse_color(value)?);
                        }
                        if let Some(value) = table.remove("style") {
                            *style = style.underline_style(Self::parse_underline_style(&value)?);
                        }

                        if let Some(attr) = table.keys().next() {
                            return Err(format!("Invalid underline attribute: {attr}"));
                        }
                    }
                    "modifiers" => {
                        let modifiers = value.as_array().ok_or("Modifiers should be an array")?;

                        for modifier in modifiers {
                            if modifier.as_str() == Some("underlined") {
                                *style = style.underline_style(UnderlineStyle::Line);
                            } else {
                                *style = style.add_modifier(Self::parse_modifier(modifier)?);
                            }
                        }
                    }
                    _ => return Err(format!("Invalid style attribute: {}", name)),
                }
            }
        } else {
            *style = style.fg(self.parse_color(value)?);
        }
        Ok(())
    }

    fn parse_style_array(&self, value: Value) -> Result<Vec<Style>, String> {
        let mut styles = Vec::new();

        for v in value
            .as_array()
            .ok_or_else(|| format!("Could not parse value as an array: '{value}'"))?
        {
            let mut style = Style::default();
            self.parse_style(&mut style, v.clone())?;
            styles.push(style);
        }

        Ok(styles)
    }
}

impl TryFrom<Value> for ThemePalette {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let map = match value {
            Value::Table(entries) => entries,
            _ => return Ok(Self::default()),
        };

        let mut palette = HashMap::with_capacity(map.len());
        for (name, value) in map {
            let value = Self::parse_value_as_str(&value)?;
            let color = Self::string_to_rgb(value)?;
            palette.insert(name, color);
        }

        Ok(Self::new(palette))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_compile_inherited_styles_and_return_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir(&themes).unwrap();
        std::fs::write(
            themes.join("parent.toml"),
            r##"
            inherits = "default"
            keyword = { fg = "accent", bg = "black", modifiers = ["bold"] }
            string = "base"
            [palette]
            accent = "#112233"
            base = "#445566"
        "##,
        )
        .unwrap();
        std::fs::write(
            themes.join("child.toml"),
            r##"
            inherits = "parent"
            keyword = { fg = "accent" }
            invalid = { nonexistent = "red" }
            [palette]
            accent = "#aabbcc"
        "##,
        )
        .unwrap();
        let loader = Loader::new(Resources::new(vec![dir.path().into()]));
        let (theme, warnings) = loader.load_with_warnings("child").unwrap();
        assert_eq!(theme.name(), "child");
        assert_eq!(
            theme.get("keyword"),
            Style::default().fg(Color::Rgb(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            theme.get("string"),
            Style::default().fg(Color::Rgb(0x44, 0x55, 0x66))
        );
        assert_eq!(
            theme.get("ui.selection"),
            loader.default_theme().get("ui.selection")
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("nonexistent"));
        let highlight = theme.find_highlight_exact("keyword").unwrap();
        assert_eq!(theme.highlight(highlight), theme.get("keyword"));
    }

    #[test]
    fn builtin_themes_keep_names_styles_and_warning_behavior() {
        let loader = Loader::new(Resources::new(vec![]));
        for (name, expected) in [
            ("default", loader.default()),
            ("base16_default", loader.base16_default()),
        ] {
            let (theme, warnings) = loader.load_with_warnings(name).unwrap();
            assert_eq!(theme.name(), name);
            assert_eq!(theme.styles, expected.styles);
            assert_eq!(theme.scopes(), expected.scopes());
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn default_theme_follows_terminal_mode() {
        let config = Config::default();
        assert!(config.is_adaptive());
        assert_eq!(config.choose(Some(Mode::Light)), "modus_operandi");
        assert_eq!(config.choose(Some(Mode::Dark)), "modus_vivendi");
        assert_eq!(config.choose(None), "modus_vivendi");
    }

    #[test]
    fn test_parse_style_string() {
        let fg = Value::String("#ffffff".to_string());

        let mut style = Style::default();
        let palette = ThemePalette::default();
        palette.parse_style(&mut style, fg).unwrap();

        assert_eq!(style, Style::default().fg(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn test_palette() {
        use editor_core::hashmap;
        let fg = Value::String("my_color".to_string());

        let mut style = Style::default();
        let palette =
            ThemePalette::new(hashmap! { "my_color".to_string() => Color::Rgb(255, 255, 255) });
        palette.parse_style(&mut style, fg).unwrap();

        assert_eq!(style, Style::default().fg(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn test_parse_style_table() {
        let table = toml::toml! {
            "keyword" = {
                fg = "#ffffff",
                bg = "#000000",
                modifiers = ["bold"],
            }
        };

        let mut style = Style::default();
        let palette = ThemePalette::default();
        for (_name, value) in table {
            palette.parse_style(&mut style, value).unwrap();
        }

        assert_eq!(
            style,
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .bg(Color::Rgb(0, 0, 0))
                .add_modifier(Modifier::BOLD)
        );
    }

    // tests for parsing an RGB `Highlight`

    #[test]
    fn convert_to_and_from() {
        let (r, g, b) = (0xFF, 0xFE, 0xFA);
        let highlight = Theme::rgb_highlight(r, g, b);
        assert_eq!(Theme::decode_rgb_highlight(highlight), Some((r, g, b)));
    }

    /// make sure we can store all the colors at the end
    #[test]
    fn full_numeric_range() {
        assert_eq!(Highlight::MAX - Theme::RGB_START, 256_u32.pow(3));
    }

    #[test]
    fn retrieve_color() {
        // color in the middle
        let (r, g, b) = (0x14, 0xAA, 0xF7);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
        // pure black
        let (r, g, b) = (0x00, 0x00, 0x00);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
        // pure white
        let (r, g, b) = (0xff, 0xff, 0xff);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
    }

    #[test]
    #[should_panic(expected = "index out of bounds: the len is 0 but the index is 4278190078")]
    fn out_of_bounds() {
        let highlight = Highlight::new(Theme::rgb_highlight(0, 0, 0).get() - 1);
        Theme::default().highlight(highlight);
    }
}

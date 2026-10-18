/// Syntax features shown in health reports and language documentation.
#[derive(Debug, Copy, Clone)]
pub enum QueryFeature {
    Highlight,
    TextObject,
    AutoIndent,
    Tags,
    RainbowBracket,
}

impl QueryFeature {
    /// Features in their display order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Highlight,
            Self::TextObject,
            Self::AutoIndent,
            Self::Tags,
            Self::RainbowBracket,
        ]
    }

    /// Runtime query source used to detect support for this feature.
    pub fn runtime_filename(&self) -> &'static str {
        match *self {
            Self::Highlight => "highlights.scm",
            Self::TextObject => "textobjects.scm",
            Self::AutoIndent => "indents.scm",
            Self::Tags => "tags.scm",
            Self::RainbowBracket => "rainbows.scm",
        }
    }

    /// Heading used in language documentation.
    pub fn long_title(&self) -> &'static str {
        match *self {
            Self::Highlight => "Syntax Highlighting",
            Self::TextObject => "Treesitter Textobjects",
            Self::AutoIndent => "Auto Indent",
            Self::Tags => "Code Navigation Tags",
            Self::RainbowBracket => "Rainbow Brackets",
        }
    }

    /// Compact heading used in terminal health reports.
    pub fn short_title(&self) -> &'static str {
        match *self {
            Self::Highlight => "Highlight",
            Self::TextObject => "Textobject",
            Self::AutoIndent => "Indent",
            Self::Tags => "Tags",
            Self::RainbowBracket => "Rainbow",
        }
    }
}

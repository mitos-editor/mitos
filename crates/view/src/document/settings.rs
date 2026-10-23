//! Document setting resolution from text, language configuration, and EditorConfig.

use super::{editorconfig::EditorConfig, Document};
use editor_core::{
    indent::{auto_detect_indent_style, IndentStyle},
    line_ending::auto_detect_line_ending,
};
use std::collections::HashSet;
use text_layout::doc_formatter::TextFormat;
use theme::Theme;

pub(super) const DEFAULT_INDENT: IndentStyle = IndentStyle::Tabs;
const DEFAULT_TAB_WIDTH: usize = 4;

impl Document {
    /// Re-read path-based settings before resolving indentation, line endings, and spelling.
    pub(crate) fn refresh_settings(&mut self) {
        self.detect_editor_config();
        self.detect_indent_and_line_ending();
    }

    /// Detect the indentation used in the file, or otherwise defaults to the language indentation
    /// configured in `languages.toml`, with a fallback to tabs if it isn't specified. Line ending
    /// is likewise auto-detected, and will remain unchanged if no line endings were detected.
    pub fn detect_indent_and_line_ending(&mut self) {
        self.indent_style = if let Some(indent_style) = self.editor_config.indent_style {
            indent_style
        } else {
            self.language_config()
                .is_none_or(|config| config.auto_detect_indent)
                .then(|| auto_detect_indent_style(self.buffer.text()))
                .flatten()
                .unwrap_or_else(|| {
                    self.language_config()
                        .and_then(|config| config.indent.as_ref())
                        .map_or(DEFAULT_INDENT, |config| IndentStyle::from_str(&config.unit))
                })
        };
        if let Some(line_ending) = self
            .editor_config
            .line_ending
            .or_else(|| auto_detect_line_ending(self.buffer.text()))
        {
            self.line_ending = line_ending;
        }
        self.detect_spelling_languages();
    }

    /// Resolves the languages this document is spell-checked against, in precedence order: a manual
    /// `:set-spelling-language` override, then the `.editorconfig` `spelling_language`, then the
    /// resolved config (`languages.toml` over `[editor.spelling]`). Re-run when any of these change.
    pub fn detect_spelling_languages(&mut self) {
        if self.is_binary() {
            self.spelling_languages.clear();
            return;
        }
        let config = self.config.load();
        self.spelling_languages = if let Some(languages) = &self.spelling_language_override {
            languages.clone()
        } else if let Some(language) = self
            .editor_config
            .spelling_language
            .as_ref()
            .filter(|_| config.files.editor_config)
        {
            vec![language.clone()]
        } else {
            config
                .spelling
                .merged(
                    self.language_config()
                        .and_then(|config| config.spelling.as_ref()),
                )
                .languages()
                .to_vec()
        };
        let mut seen = HashSet::new();
        self.spelling_languages
            .retain(|language| seen.insert(language.clone()));
    }

    pub fn detect_editor_config(&mut self) {
        self.editor_config = if self.config.load().files.editor_config {
            self.path().map(EditorConfig::find).unwrap_or_default()
        } else {
            EditorConfig::default()
        };
    }

    /// The width that the tab character is rendered at
    pub fn tab_width(&self) -> usize {
        self.editor_config
            .tab_width
            .map(|n| n.get() as usize)
            .unwrap_or_else(|| {
                self.language_config()
                    .and_then(|config| config.indent.as_ref())
                    .map_or(DEFAULT_TAB_WIDTH, |config| config.tab_width)
            })
    }

    // The width (in spaces) of a level of indentation.
    pub fn indent_width(&self) -> usize {
        self.indent_style.indent_width(self.tab_width())
    }

    /// Whether the document should have a trailing line ending appended on save.
    pub fn insert_final_newline(&self) -> bool {
        self.editor_config
            .insert_final_newline
            .unwrap_or_else(|| self.config.load().files.save.insert_final_newline)
    }

    /// Whether the document should trim whitespace preceding line endings on save.
    pub fn trim_trailing_whitespace(&self) -> bool {
        self.editor_config
            .trim_trailing_whitespace
            .unwrap_or_else(|| self.config.load().files.save.trim_trailing_whitespace)
    }

    pub fn text_width(&self) -> usize {
        self.editor_config
            .max_line_length
            .map(|n| n.get() as usize)
            .or_else(|| self.language_config().and_then(|config| config.text_width))
            .unwrap_or_else(|| self.config.load().text_width)
    }

    pub fn text_format(&self, mut viewport_width: u16, theme: Option<&Theme>) -> TextFormat {
        let config = self.config.load();
        let text_width = self.text_width();
        let mut soft_wrap_at_text_width = self
            .language_config()
            .and_then(|config| {
                config
                    .soft_wrap
                    .as_ref()
                    .and_then(|soft_wrap| soft_wrap.wrap_at_text_width)
            })
            .or(config.soft_wrap.wrap_at_text_width)
            .unwrap_or(false);
        if soft_wrap_at_text_width {
            // if the viewport is smaller than the specified
            // width then this setting has no effect
            if text_width >= viewport_width as usize {
                soft_wrap_at_text_width = false;
            } else {
                viewport_width = text_width as u16;
            }
        }
        let editor_soft_wrap = &config.soft_wrap;
        let language_soft_wrap = self
            .language
            .as_ref()
            .and_then(|config| config.soft_wrap.as_ref());
        let enable_soft_wrap = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.enable)
            .or(editor_soft_wrap.enable)
            .unwrap_or(false);
        let max_wrap = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.max_wrap)
            .or(config.soft_wrap.max_wrap)
            .unwrap_or(20);
        let max_indent_retain = language_soft_wrap
            .and_then(|soft_wrap| soft_wrap.max_indent_retain)
            .or(editor_soft_wrap.max_indent_retain)
            .unwrap_or(40);
        let tab_width = self.tab_width() as u16;
        TextFormat {
            soft_wrap: enable_soft_wrap && viewport_width > 10,
            tab_width,
            max_wrap: max_wrap.min(viewport_width / 4),
            max_indent_retain: max_indent_retain.min(viewport_width * 2 / 5),
            wrap_indicator: Box::from(""),
            // avoid spinning forever when the window manager
            // sets the size to something tiny
            viewport_width,
            wrap_indicator_highlight: theme
                .and_then(|theme| theme.find_highlight("ui.virtual.wrap")),
            soft_wrap_at_text_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::tests::document_and_view;
    use std::sync::Arc;

    #[tokio::test]
    async fn indent_detection_opt_out_keeps_editorconfig_precedence() {
        for setting in ["", "auto-detect-indent = false"] {
            let loader = Arc::new(language::syntax::Loader::new(toml::from_str(&format!("[[language]]\nname = 'test'\nscope = 'source.test'\nfile-types = []\nindent = {{ unit = '    ', tab-width = 4 }}\n{setting}")).unwrap(), loader::syntax::Resources::default()).unwrap());
            let (mut doc, _) = document_and_view("#[|a]#\n  b\n    c\n  d\n");
            let lang = loader.language_for_name("test").unwrap();
            doc.set_language(Some(loader.language(lang).config().clone()), &loader);
            doc.detect_indent_and_line_ending();
            assert_eq!(
                doc.indent_style,
                IndentStyle::Spaces(if setting.is_empty() { 2 } else { 4 })
            );
            doc.editor_config.indent_style = Some(IndentStyle::Tabs);
            doc.detect_indent_and_line_ending();
            assert_eq!(doc.indent_style, IndentStyle::Tabs);
        }
    }
}

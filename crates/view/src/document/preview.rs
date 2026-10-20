//! Prepare file-backed previews on workers and publish them with live configuration.

use super::{
    file::{DocumentOpenError, FileState},
    Document, EditorConfig,
};
use arc_swap::access::DynAccess;
use editor_config::Config;
use editor_core::Rope;
use language::syntax;
use std::{path::Path, sync::Arc};

/// Decoded preview state that can cross from an I/O worker to the editor thread.
/// The application supplies its live configuration when publishing the document.
pub struct PreparedPreview {
    text: Rope,
    file: FileState,
    editor_config: EditorConfig,
    language: Option<Arc<language_config::LanguageConfiguration>>,
    loader: Arc<syntax::Loader>,
    indent_style: editor_core::indent::IndentStyle,
    line_ending: editor_core::line_ending::LineEnding,
}

impl PreparedPreview {
    /// Perform preview file I/O, configuration discovery, and decoding on a worker.
    pub fn load(
        path: &Path,
        config: Config,
        loader: Arc<syntax::Loader>,
    ) -> Result<Self, DocumentOpenError> {
        if !path.metadata()?.is_file() {
            return Err(DocumentOpenError::IrregularFile);
        }
        let doc = Document::open_for_preview(
            path,
            Arc::new(arc_swap::ArcSwap::from_pointee(config)),
            loader.clone(),
        )?;
        Ok(Self {
            text: doc.text().clone(),
            file: doc.file,
            editor_config: doc.editor_config,
            language: doc.language,
            loader,
            indent_style: doc.indent_style,
            line_ending: doc.line_ending,
        })
    }

    /// Publish without reading the filesystem or parsing syntax.
    pub fn finish(self, config: Arc<dyn DynAccess<Config>>) -> Document {
        let mut doc = Document::from(self.text, None, config, self.loader.clone());
        doc.file = self.file;
        doc.editor_config = self.editor_config;
        doc.language = self.language;
        doc.indent_style = self.indent_style;
        doc.line_ending = self.line_ending;
        doc.initialize_syntax(self.loader);
        doc.detect_spelling_languages();
        doc
    }
}

#[cfg(test)]
mod prepared_preview_tests {
    use super::*;

    #[test]
    fn prepared_preview_is_sendable_and_publishes_without_reopening_files() {
        fn assert_send<T: Send>() {}
        assert_send::<PreparedPreview>();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.json");
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\n",
        )
        .unwrap();
        std::fs::write(&path, "{\r\n  \"key\": 1\r\n}\r\n").unwrap();
        let loader = Arc::new(language::loader::default_lang_loader(
            loader::syntax::Resources::default(),
        ));
        let config = Config::default();
        let expected = Document::open_for_preview(
            &path,
            Arc::new(arc_swap::ArcSwap::from_pointee(config.clone())),
            loader.clone(),
        )
        .unwrap();
        let prepared = PreparedPreview::load(&path, config.clone(), loader).unwrap();
        std::fs::remove_file(&path).unwrap();
        let actual = prepared.finish(Arc::new(arc_swap::ArcSwap::from_pointee(config)));
        assert_eq!(actual.text(), expected.text());
        assert_eq!(actual.path(), expected.path());
        assert_eq!(actual.encoding(), expected.encoding());
        assert_eq!(actual.indent_style, expected.indent_style);
        assert_eq!(actual.line_ending, expected.line_ending);
        assert_eq!(actual.editor_config, expected.editor_config);
        assert_eq!(actual.is_readonly(), expected.is_readonly());
        assert!(actual.is_syntax_pending());
    }
}

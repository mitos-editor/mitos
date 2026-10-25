//! Editor adapters around upstream LSP wire types.

use crate::lsp;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum UriError {
    #[error(transparent)]
    Parse(#[from] stdx::uri::ParseError),
    #[error(transparent)]
    Conversion(#[from] editor_core::uri::UrlConversionError),
}

/// File URI conversion retains Mitos's RFC3986 path encoding on every platform.
pub trait UriExt: Sized {
    #[allow(clippy::result_unit_err)]
    fn from_file_path(path: impl AsRef<Path>) -> Result<Self, ()>;
    #[allow(clippy::result_unit_err)]
    fn from_directory_path(path: impl AsRef<Path>) -> Result<Self, ()>;
    #[allow(clippy::result_unit_err)]
    fn to_file_path(&self) -> Result<PathBuf, ()>;
    fn parse(input: &str) -> Result<Self, <Self as std::str::FromStr>::Err>
    where
        Self: std::str::FromStr,
    {
        input.parse()
    }
    fn to_editor_uri(&self) -> Result<editor_core::Uri, UriError>;
    fn to_editor_url(&self) -> Result<stdx::Url, stdx::uri::ParseError>;
}

impl UriExt for lsp::Uri {
    fn from_file_path(path: impl AsRef<Path>) -> Result<Self, ()> {
        stdx::Url::from_file_path(path)?
            .as_str()
            .parse()
            .map_err(|_| ())
    }
    fn from_directory_path(path: impl AsRef<Path>) -> Result<Self, ()> {
        stdx::Url::from_directory_path(path)?
            .as_str()
            .parse()
            .map_err(|_| ())
    }
    fn to_file_path(&self) -> Result<PathBuf, ()> {
        self.to_editor_url().map_err(|_| ())?.to_file_path()
    }
    fn to_editor_uri(&self) -> Result<editor_core::Uri, UriError> {
        Ok(editor_core::Uri::try_from(self.to_editor_url()?)?)
    }
    fn to_editor_url(&self) -> Result<stdx::Url, stdx::uri::ParseError> {
        stdx::Url::parse(self.as_str())
    }
}

/// Stable icon names and the supported wire values, including unknown-kind fallback.
pub trait SymbolKindExt {
    fn as_str(&self) -> &'static str;
    fn all() -> Vec<lsp::SymbolKind>;
}

impl SymbolKindExt for lsp::SymbolKind {
    #[inline]
    fn as_str(&self) -> &'static str {
        match *self {
            Self::FILE => "file",
            Self::MODULE => "module",
            Self::NAMESPACE => "namespace",
            Self::PACKAGE => "package",
            Self::CLASS => "class",
            Self::METHOD => "method",
            Self::PROPERTY => "property",
            Self::FIELD => "field",
            Self::CONSTRUCTOR => "constructor",
            Self::ENUM => "enum",
            Self::INTERFACE => "interface",
            Self::FUNCTION => "function",
            Self::VARIABLE => "variable",
            Self::CONSTANT => "constant",
            Self::STRING => "string",
            Self::NUMBER => "number",
            Self::BOOLEAN => "boolean",
            Self::ARRAY => "array",
            Self::OBJECT => "object",
            Self::KEY => "key",
            Self::NULL => "null",
            Self::ENUM_MEMBER => "enum_member",
            Self::STRUCT => "struct",
            Self::EVENT => "event",
            Self::OPERATOR => "operator",
            Self::TYPE_PARAMETER => "type_param",
            _ => "",
        }
    }

    fn all() -> Vec<Self> {
        vec![
            Self::FILE,
            Self::MODULE,
            Self::NAMESPACE,
            Self::PACKAGE,
            Self::CLASS,
            Self::METHOD,
            Self::PROPERTY,
            Self::FIELD,
            Self::CONSTRUCTOR,
            Self::ENUM,
            Self::INTERFACE,
            Self::FUNCTION,
            Self::VARIABLE,
            Self::CONSTANT,
            Self::STRING,
            Self::NUMBER,
            Self::BOOLEAN,
            Self::ARRAY,
            Self::OBJECT,
            Self::KEY,
            Self::NULL,
            Self::ENUM_MEMBER,
            Self::STRUCT,
            Self::EVENT,
            Self::OPERATOR,
            Self::TYPE_PARAMETER,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_uris_keep_editor_encoding_and_round_trip_through_wire_types() {
        let path = std::env::temp_dir().join("[brackets] space#é.rs");
        let expected = stdx::Url::from_file_path(&path).unwrap();
        let uri = lsp::Uri::from_file_path(&path).unwrap();
        assert_eq!(uri.as_str(), expected.as_str());
        assert!(uri.as_str().contains("%5Bbrackets%5D%20space%23%C3%A9.rs"));
        let location = lsp::Location::new(uri, lsp::Range::default());
        let encoded = serde_json::to_value(&location).unwrap();
        let decoded: lsp::Location = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.uri.to_file_path().unwrap(), path);
        // Editor URIs normalize Windows short names such as RUNNER~1 in TEMP.
        let normalized = stdx::path::normalize(&path);
        assert_eq!(
            decoded.uri.to_editor_uri().unwrap().as_path(),
            Some(normalized.as_path())
        );
    }

    #[test]
    fn uri_conversion_preserves_queries_fragments_and_non_file_schemes() {
        for value in [
            "FILE:///tmp/a%20b.rs?query=value#line-1",
            "csharp:/metadata/foo/Bar.cs",
            "untitled:document-1",
        ] {
            let uri: lsp::Uri = serde_json::from_value(json!(value)).unwrap();
            assert_eq!(uri.to_editor_url().unwrap().as_str(), value);
            assert_eq!(serde_json::to_value(&uri).unwrap(), json!(value));
        }
        let relative: lsp::Uri = "relative/path.rs".parse().unwrap();
        assert!(relative.to_editor_url().is_err());
        assert!(relative.to_editor_uri().is_err());
        assert!(relative.to_file_path().is_err());
        assert!(lsp::Uri::from_file_path("relative/path.rs").is_err());
    }

    #[test]
    fn workspace_edits_and_proposed_features_keep_wire_shapes() {
        let path = std::env::temp_dir().join("[wire] é.rs");
        let uri = lsp::Uri::from_file_path(path).unwrap();
        let value = json!({"changes": { uri.as_str(): [{"range": {"start": {"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"text"}] }});
        let edit: lsp::WorkspaceEdit = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(edit).unwrap(), value);
        let capabilities: lsp::ServerCapabilities = serde_json::from_value(json!({"inlineCompletionProvider":true,"diagnosticProvider":{"identifier":"checker","interFileDependencies":false,"workspaceDiagnostics":false}})).unwrap();
        assert!(capabilities.inline_completion_provider.is_some());
        let value = serde_json::to_value(capabilities).unwrap();
        assert_eq!(value["diagnosticProvider"]["identifier"], "checker");
        let completion: lsp::InlineCompletionResponse =
            serde_json::from_value(json!({"items":[{"insertText":"answer"}]})).unwrap();
        assert!(
            matches!(completion,lsp::InlineCompletionResponse::List(list) if list.items.len()==1)
        );
    }

    #[test]
    fn upstream_wire_types_reject_malformed_uris_previously_stored_opaquely() {
        // The old fork checked only the scheme. Upstream validates the complete
        // URI, so a server must percent-encode spaces in file locations.
        let malformed = "file:///tmp/unescaped space.rs";
        assert!(stdx::Url::parse(malformed).is_ok());
        let location = json!({"uri":malformed,"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}});
        assert!(serde_json::from_value::<lsp::Location>(location).is_err());
        let encoded = "file:///tmp/unescaped%20space.rs";
        let uri: lsp::Uri = serde_json::from_value(json!(encoded)).unwrap();
        assert_eq!(uri.as_str(), encoded);
    }

    #[test]
    fn unknown_symbol_kinds_remain_distinct_and_have_no_icon_name() {
        let first: lsp::SymbolKind = serde_json::from_value(json!(100)).unwrap();
        let second: lsp::SymbolKind = serde_json::from_value(json!(101)).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str(), "");
        let kinds = lsp::SymbolKind::all();
        assert_eq!(kinds.len(), 26);
        assert_eq!(
            serde_json::to_value(kinds).unwrap(),
            json!((1..=26).collect::<Vec<_>>())
        );
    }
}

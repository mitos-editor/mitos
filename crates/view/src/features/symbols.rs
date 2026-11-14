//! Symbol data shared by document and workspace queries after protocol conversion.

use lsp_client::{lsp, Client, OffsetEncoding, UriExt as _};

use super::navigation::Location;

/// A symbol with one editor location, independent of the server's response shape.
#[derive(Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: lsp::SymbolKind,
    pub container_name: Option<String>,
    pub location: Location,
}

impl Symbol {
    pub(super) fn from_lsp_with_server(
        name: String,
        kind: lsp::SymbolKind,
        container_name: Option<String>,
        location: lsp::Location,
        encoding: OffsetEncoding,
        server: Option<&Client>,
    ) -> Option<Self> {
        Some(Self {
            name,
            kind,
            container_name,
            location: match server {
                Some(server) => {
                    if location.uri.to_file_path().is_err()
                        && !server.supports_text_document_content(&location.uri)
                    {
                        return None;
                    }
                    Location::from_server(location, encoding, server.id())?
                }
                None => Location::from_lsp(location, encoding)?,
            },
        })
    }
}

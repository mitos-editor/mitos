//! Symbol data shared by document and workspace queries after protocol conversion.

use lsp_client::{lsp, OffsetEncoding};

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
    pub(super) fn from_lsp(
        name: String,
        kind: lsp::SymbolKind,
        container_name: Option<String>,
        location: lsp::Location,
        encoding: OffsetEncoding,
    ) -> Option<Self> {
        Some(Self {
            name,
            kind,
            container_name,
            location: Location::from_lsp(location, encoding)?,
        })
    }
}

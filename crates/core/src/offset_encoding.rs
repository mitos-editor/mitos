//! Encoding for external line and column positions.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
/// Unit used by the server for the `character` component of LSP positions.
///
/// UTF-16 is the protocol default when position encoding negotiation is not
/// supported.
pub enum OffsetEncoding {
    /// UTF-8 code units aka bytes
    Utf8,
    /// UTF-32 code units aka chars
    Utf32,
    /// UTF-16 code units
    #[default]
    Utf16,
}

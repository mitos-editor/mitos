/// Heuristic "is this a binary (non-text) file?" check over a leading chunk of a
/// file. Replaces the `content_inspector` crate — we only need the binary/text
/// verdict, not its encoding classification.
///
/// A leading byte-order mark marks the content as text (UTF-16/32 text
/// legitimately contains NUL bytes, so it must be excluded before the NUL scan);
/// otherwise a NUL byte in the first kilobyte — or a known binary magic number —
/// means binary.
pub fn is_binary(buffer: &[u8]) -> bool {
    // UTF-32 BOMs must be checked before UTF-16 (their BOMs overlap).
    const BYTE_ORDER_MARKS: &[&[u8]] = &[
        &[0xEF, 0xBB, 0xBF],       // UTF-8
        &[0x00, 0x00, 0xFE, 0xFF], // UTF-32BE
        &[0xFF, 0xFE, 0x00, 0x00], // UTF-32LE
        &[0xFE, 0xFF],             // UTF-16BE
        &[0xFF, 0xFE],             // UTF-16LE
    ];

    if BYTE_ORDER_MARKS.iter().any(|bom| buffer.starts_with(bom)) {
        return false;
    }

    let scan = &buffer[..buffer.len().min(1024)];
    scan.contains(&0) || has_binary_signature(buffer)
}

/// Recognize binary headers even when an explicit text encoding was requested.
pub fn has_binary_signature(buffer: &[u8]) -> bool {
    [
        b"%PDF".as_slice(),
        b"\x89PNG",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
    ]
    .iter()
    .any(|magic| buffer.starts_with(magic))
        || (buffer.starts_with(b"RIFF") && buffer.get(8..12) == Some(b"WEBP"))
}
